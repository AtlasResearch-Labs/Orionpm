use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{anyhow, Context, Result};
use serde_json::Value;

use crate::config::{AlertConfig, AppConfig, ExecutionMode, RestartPolicy, OrionConfig};

/// Parse a PM2 ecosystem file (ecosystem.config.js, ecosystem.json, pm2.json, etc.)
/// into a map of app name to orion AppConfig.
pub fn parse_pm2_ecosystem(path: &Path) -> Result<HashMap<String, AppConfig>> {
    let json_val = read_as_json(path)?;
    parse_apps_from_json(&json_val, path)
}

/// Convert a set of AppConfigs into a valid orion.toml configuration string.
pub fn to_orion_toml(apps: &HashMap<String, AppConfig>) -> Result<String> {
    let orion_config = OrionConfig {
        apps: apps.clone(),
        smtp: None,
    };
    toml::to_string_pretty(&orion_config).context("Failed to serialize to orion.toml format")
}

fn read_as_json(path: &Path) -> Result<Value> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    if ext == "json" {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read PM2 JSON file: {}", path.display()))?;
        let val: Value = serde_json::from_str(&content)
            .with_context(|| format!("Invalid JSON in PM2 file: {}", path.display()))?;
        return Ok(val);
    }

    // For .js / .cjs / .mjs files, try executing node if available
    let abs_path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let node_script = format!(
        r#"try {{ const conf = require({}); console.log(JSON.stringify(conf)); }} catch(e) {{ console.error(e.message); process.exit(1); }}"#,
        serde_json::to_string(&abs_path.to_string_lossy())?
    );

    let output = Command::new("node").arg("-e").arg(&node_script).output();

    if let Ok(out) = output {
        if out.status.success() {
            let stdout_str = String::from_utf8_lossy(&out.stdout);
            if let Ok(val) = serde_json::from_str::<Value>(stdout_str.trim()) {
                return Ok(val);
            }
        }
    }

    // Fallback: read raw file and try extracting JSON-like apps array
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read PM2 file: {}", path.display()))?;

    fallback_extract_apps(&content)
        .ok_or_else(|| anyhow!("Failed to parse PM2 file '{}'. Ensure Node.js is installed or convert to JSON.", path.display()))
}

fn fallback_extract_apps(content: &str) -> Option<Value> {
    // Look for apps: [ ... ]
    let apps_marker = "apps";
    if let Some(pos) = content.find(apps_marker) {
        if let Some(bracket_start) = content[pos..].find('[') {
            let start = pos + bracket_start;
            if let Some(bracket_end) = content[start..].rfind(']') {
                let end = start + bracket_end + 1;
                let slice = &content[start..end];
                // Try parsing directly as JSON
                if let Ok(v) = serde_json::from_str::<Value>(slice) {
                    return Some(serde_json::json!({ "apps": v }));
                }
            }
        }
    }
    None
}

fn parse_apps_from_json(root: &Value, source_path: &Path) -> Result<HashMap<String, AppConfig>> {
    let apps_array = if let Some(arr) = root.get("apps").and_then(|v| v.as_array()) {
        arr
    } else if let Some(arr) = root.as_array() {
        arr
    } else {
        return Err(anyhow!(
            "PM2 ecosystem file must contain an 'apps' array or be a root array of apps"
        ));
    };

    let base_dir = source_path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));

    let mut result = HashMap::new();

    for (idx, app_val) in apps_array.iter().enumerate() {
        let script = app_val
            .get("script")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("App at index {} is missing required 'script' field", idx))?;

        let name = app_val
            .get("name")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| {
                Path::new(script)
                    .file_stem()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_else(|| format!("app-{}", idx))
            });

        // Determine command and args
        let mut extra_args = Vec::new();
        if let Some(args_val) = app_val.get("args") {
            if let Some(arr) = args_val.as_array() {
                for a in arr {
                    if let Some(s) = a.as_str() {
                        extra_args.push(s.to_string());
                    }
                }
            } else if let Some(s) = args_val.as_str() {
                extra_args.extend(split_args(s));
            }
        }

        let (command, args) = if script.ends_with(".js") || script.ends_with(".cjs") || script.ends_with(".mjs") {
            let mut a = vec![script.to_string()];
            a.extend(extra_args);
            ("node".to_string(), a)
        } else if script.ends_with(".ts") {
            let mut a = vec![script.to_string()];
            a.extend(extra_args);
            ("ts-node".to_string(), a)
        } else if script.ends_with(".py") {
            let mut a = vec![script.to_string()];
            a.extend(extra_args);
            ("python".to_string(), a)
        } else if script.ends_with(".sh") {
            let mut a = vec![script.to_string()];
            a.extend(extra_args);
            ("bash".to_string(), a)
        } else {
            (script.to_string(), extra_args)
        };

        // Instances
        let instances = match app_val.get("instances") {
            Some(Value::String(s)) if s == "max" || s == "all" => 0,
            Some(Value::Number(n)) => n.as_u64().unwrap_or(1) as u32,
            _ => 1,
        };

        // Execution mode
        let mode = match app_val.get("exec_mode").and_then(|v| v.as_str()) {
            Some("cluster") => ExecutionMode::Cluster,
            _ => ExecutionMode::Fork,
        };

        // CWD
        let cwd = app_val
            .get("cwd")
            .and_then(|v| v.as_str())
            .map(|c| {
                let p = Path::new(c);
                if p.is_absolute() {
                    p.to_path_buf()
                } else {
                    clean_join(&base_dir, p)
                }
            })
            .or_else(|| Some(base_dir.clone()));

        // Environment variables
        let mut env = HashMap::new();
        if let Some(env_obj) = app_val.get("env").and_then(|v| v.as_object()) {
            for (k, v) in env_obj {
                if let Some(s) = v.as_str() {
                    env.insert(k.clone(), s.to_string());
                } else if let Some(n) = v.as_i64() {
                    env.insert(k.clone(), n.to_string());
                } else if let Some(b) = v.as_bool() {
                    env.insert(k.clone(), b.to_string());
                }
            }
        }

        // Watch settings
        let watch = match app_val.get("watch") {
            Some(Value::Bool(b)) => *b,
            Some(Value::Array(_)) => true,
            _ => false,
        };

        let mut watch_paths = vec![base_dir.clone()];
        if let Some(Value::Array(arr)) = app_val.get("watch") {
            watch_paths.clear();
            for p in arr {
                if let Some(s) = p.as_str() {
                    watch_paths.push(base_dir.join(s));
                }
            }
        }

        let mut watch_ignore = vec![
            "node_modules".to_string(),
            "target".to_string(),
            "dist".to_string(),
            ".git".to_string(),
            "*.log".to_string(),
        ];
        if let Some(Value::Array(arr)) = app_val.get("ignore_watch") {
            for p in arr {
                if let Some(s) = p.as_str() {
                    watch_ignore.push(s.to_string());
                }
            }
        }

        // Memory limit
        let max_memory_mb = app_val
            .get("max_memory_restart")
            .and_then(|v| match v {
                Value::String(s) => parse_memory_string(s),
                Value::Number(n) => {
                    let val = n.as_u64().unwrap_or(0);
                    if val > 10_000_000 {
                        Some(val / (1024 * 1024))
                    } else {
                        Some(val)
                    }
                }
                _ => None,
            })
            .unwrap_or(0);

        // Logs
        let stdout_log = app_val
            .get("out_file")
            .or_else(|| app_val.get("output"))
            .and_then(|v| v.as_str())
            .map(|s| clean_join(&base_dir, Path::new(s)));

        let stderr_log = app_val
            .get("error_file")
            .or_else(|| app_val.get("error"))
            .and_then(|v| v.as_str())
            .map(|s| clean_join(&base_dir, Path::new(s)));

        // Restart policy
        let autorestart = app_val
            .get("autorestart")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);

        let restart_policy = if autorestart {
            RestartPolicy::Always
        } else {
            RestartPolicy::Never
        };

        let restart_delay_ms = app_val
            .get("restart_delay")
            .and_then(|v| v.as_u64())
            .unwrap_or(1000);

        let max_restarts = app_val
            .get("max_restarts")
            .and_then(|v| v.as_u64())
            .unwrap_or(10) as u32;

        let app_config = AppConfig {
            command,
            args,
            cwd,
            name: Some(name.clone()),
            instances,
            mode,
            env,
            restart_policy,
            restart_delay_ms,
            max_restarts,
            restart_window_secs: 60,
            watch,
            watch_paths,
            watch_ignore,
            stdout_log,
            stderr_log,
            log_lines: 1000,
            max_memory_mb,
            max_cpu_pct: 0.0,
            alerts: AlertConfig::default(),
        };

        result.insert(name, app_config);
    }

    Ok(result)
}

/// Parses strings like "500M", "500MB", "1G", "1GB", "1024K" into MB integers.
pub fn parse_memory_string(s: &str) -> Option<u64> {
    let clean = s.trim().to_uppercase();
    if clean.ends_with("GB") || clean.ends_with('G') {
        let num_str = clean.trim_end_matches("GB").trim_end_matches('G').trim();
        num_str.parse::<u64>().ok().map(|g| g * 1024)
    } else if clean.ends_with("MB") || clean.ends_with('M') {
        let num_str = clean.trim_end_matches("MB").trim_end_matches('M').trim();
        num_str.parse::<u64>().ok()
    } else if clean.ends_with("KB") || clean.ends_with('K') {
        let num_str = clean.trim_end_matches("KB").trim_end_matches('K').trim();
        num_str.parse::<u64>().ok().map(|k| (k / 1024).max(1))
    } else {
        clean.parse::<u64>().ok()
    }
}

/// Splits a command line argument string into distinct arguments respecting quotes.
pub fn split_args(input: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut in_single = false;
    let mut in_double = false;
    let mut escape = false;

    for ch in input.chars() {
        if escape {
            current.push(ch);
            escape = false;
        } else if ch == '\\' {
            escape = true;
        } else if ch == '\'' && !in_double {
            in_single = !in_single;
        } else if ch == '"' && !in_single {
            in_double = !in_double;
        } else if ch.is_whitespace() && !in_single && !in_double {
            if !current.is_empty() {
                args.push(current.clone());
                current.clear();
            }
        } else {
            current.push(ch);
        }
    }
    if !current.is_empty() {
        args.push(current);
    }
    args
}

/// Safely joins a relative path with a base directory, normalizing leading dots.
pub fn clean_join(base: &Path, rel: &Path) -> PathBuf {
    if rel.is_absolute() {
        return rel.to_path_buf();
    }
    let mut clean_rel = rel;
    // Strip leading current dir component if present
    if let Ok(stripped) = clean_rel.strip_prefix(".") {
        clean_rel = stripped;
    }
    // If clean_rel starts with the base directory name itself, don't duplicate
    if let Some(base_name) = base.file_name() {
        if let Ok(stripped) = clean_rel.strip_prefix(base_name) {
            clean_rel = stripped;
        }
    }
    base.join(clean_rel)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_memory_strings() {
        assert_eq!(parse_memory_string("500M"), Some(500));
        assert_eq!(parse_memory_string("500MB"), Some(500));
        assert_eq!(parse_memory_string("1G"), Some(1024));
        assert_eq!(parse_memory_string("2GB"), Some(2048));
        assert_eq!(parse_memory_string("2048K"), Some(2));
        assert_eq!(parse_memory_string("150"), Some(150));
    }

    #[test]
    fn parse_json_ecosystem() {
        let json_raw = r#"{
            "apps": [
                {
                    "name": "api-service",
                    "script": "./server.js",
                    "args": "--port 8080 --cluster",
                    "instances": "max",
                    "exec_mode": "cluster",
                    "watch": true,
                    "max_memory_restart": "512M",
                    "out_file": "./logs/out.log",
                    "error_file": "./logs/err.log",
                    "env": {
                        "NODE_ENV": "production",
                        "PORT": "8080"
                    }
                }
            ]
        }"#;

        let val: Value = serde_json::from_str(json_raw).unwrap();
        let apps = parse_apps_from_json(&val, Path::new(".")).unwrap();
        assert_eq!(apps.len(), 1);

        let app = apps.get("api-service").expect("app missing");
        assert_eq!(app.command, "node");
        assert_eq!(app.args, vec!["./server.js", "--port", "8080", "--cluster"]);
        assert_eq!(app.instances, 0);
        assert_eq!(app.mode, ExecutionMode::Cluster);
        assert!(app.watch);
        assert_eq!(app.max_memory_mb, 512);
        assert_eq!(app.env.get("NODE_ENV"), Some(&"production".to_string()));
        assert_eq!(app.stdout_log, Some(PathBuf::from("logs/out.log")));
        assert_eq!(app.stderr_log, Some(PathBuf::from("logs/err.log")));
    }

    #[test]
    fn to_orion_toml_serialization() {
        let mut apps = HashMap::new();
        apps.insert(
            "worker".to_string(),
            AppConfig {
                command: "python".to_string(),
                args: vec!["worker.py".to_string()],
                cwd: None,
                name: Some("worker".to_string()),
                instances: 1,
                mode: ExecutionMode::Fork,
                env: HashMap::new(),
                restart_policy: RestartPolicy::Always,
                restart_delay_ms: 1000,
                max_restarts: 10,
                restart_window_secs: 60,
                watch: false,
                watch_paths: vec![PathBuf::from(".")],
                watch_ignore: vec![],
                stdout_log: None,
                stderr_log: None,
                log_lines: 1000,
                max_memory_mb: 256,
                max_cpu_pct: 0.0,
                alerts: AlertConfig::default(),
            },
        );

        let toml_str = to_orion_toml(&apps).expect("toml serialization failed");
        assert!(toml_str.contains("[app.worker]"));
        assert!(toml_str.contains(r#"command = "python""#));
        assert!(toml_str.contains("max_memory_mb = 256"));
    }
}
