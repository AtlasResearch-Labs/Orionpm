use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{anyhow, Context, Result};
use serde_json::Value;

use crate::config::{AlertConfig, AppConfig, ExecutionMode, OrionConfig, RestartPolicy};

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

    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read PM2 file: {}", path.display()))?;

    // 1. Pure native Rust parser: zero runtime dependencies, no Node.js required
    if let Some(val) = parse_js_ecosystem_native(&content) {
        return Ok(val);
    }

    // 2. Fallback: if dynamic JS code (e.g. function calls, runtime eval) is used, execute node if installed
    let abs_path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    if let Ok(path_json) = serde_json::to_string(&abs_path.to_string_lossy()) {
        let node_script = format!(
            r#"try {{ const conf = require({}); console.log(JSON.stringify(conf)); }} catch(e) {{ console.error(e.message); process.exit(1); }}"#,
            path_json
        );

        if let Ok(out) = Command::new("node").arg("-e").arg(&node_script).output() {
            if out.status.success() {
                let stdout_str = String::from_utf8_lossy(&out.stdout);
                if let Ok(val) = serde_json::from_str::<Value>(stdout_str.trim()) {
                    return Ok(val);
                }
            }
        }
    }

    Err(anyhow!(
        "Failed to parse PM2 file '{}'. Ensure it defines a valid 'apps' array.",
        path.display()
    ))
}

fn strip_js_comments(content: &str) -> String {
    let mut result = String::with_capacity(content.len());
    let chars: Vec<char> = content.chars().collect();
    let len = chars.len();
    let mut i = 0;
    let mut in_single_quote = false;
    let mut in_double_quote = false;
    let mut in_backtick = false;

    while i < len {
        let c = chars[i];
        let next = if i + 1 < len {
            Some(chars[i + 1])
        } else {
            None
        };

        // Handle string literals
        if !in_single_quote && !in_double_quote && !in_backtick {
            if c == '/' && next == Some('/') {
                // Line comment: skip until newline
                i += 2;
                while i < len && chars[i] != '\n' {
                    i += 1;
                }
                continue;
            } else if c == '/' && next == Some('*') {
                // Block comment: skip until */
                i += 2;
                while i + 1 < len && !(chars[i] == '*' && chars[i + 1] == '/') {
                    i += 1;
                }
                i += 2; // skip */
                continue;
            }
        }

        if c == '\'' && !in_double_quote && !in_backtick && (i == 0 || chars[i - 1] != '\\') {
            in_single_quote = !in_single_quote;
        } else if c == '"' && !in_single_quote && !in_backtick && (i == 0 || chars[i - 1] != '\\') {
            in_double_quote = !in_double_quote;
        } else if c == '`'
            && !in_single_quote
            && !in_double_quote
            && (i == 0 || chars[i - 1] != '\\')
        {
            in_backtick = !in_backtick;
        }

        result.push(c);
        i += 1;
    }

    result
}

fn normalize_js_object_literal(input: &str) -> String {
    let mut result = String::with_capacity(input.len() + 64);
    let chars: Vec<char> = input.chars().collect();
    let len = chars.len();
    let mut i = 0;

    let mut in_single_quote = false;
    let mut in_double_quote = false;

    while i < len {
        let c = chars[i];

        if in_single_quote {
            if c == '\'' && (i == 0 || chars[i - 1] != '\\') {
                in_single_quote = false;
                result.push('"');
            } else if c == '"' && (i == 0 || chars[i - 1] != '\\') {
                result.push('\\');
                result.push('"');
            } else {
                result.push(c);
            }
            i += 1;
            continue;
        }

        if in_double_quote {
            if c == '"' && (i == 0 || chars[i - 1] != '\\') {
                in_double_quote = false;
            }
            result.push(c);
            i += 1;
            continue;
        }

        if c == '\'' {
            in_single_quote = true;
            result.push('"');
            i += 1;
            continue;
        }

        if c == '"' {
            in_double_quote = true;
            result.push('"');
            i += 1;
            continue;
        }

        if c == '`' {
            // Treat backtick as double quote
            result.push('"');
            i += 1;
            while i < len && chars[i] != '`' {
                if chars[i] == '"' {
                    result.push('\\');
                }
                result.push(chars[i]);
                i += 1;
            }
            result.push('"');
            if i < len {
                i += 1;
            }
            continue;
        }

        // Check for unquoted identifier key before ':'
        if (c.is_ascii_alphabetic() || c == '_' || c == '$')
            && (i == 0
                || (!chars[i - 1].is_ascii_alphanumeric()
                    && chars[i - 1] != '_'
                    && chars[i - 1] != '$'))
        {
            let start = i;
            while i < len
                && (chars[i].is_ascii_alphanumeric()
                    || chars[i] == '_'
                    || chars[i] == '$'
                    || chars[i] == '-')
            {
                i += 1;
            }
            let ident: String = chars[start..i].iter().collect();

            // Look ahead past whitespace to see if followed by ':'
            let mut peek = i;
            while peek < len && chars[peek].is_whitespace() {
                peek += 1;
            }

            if peek < len && chars[peek] == ':' && !(peek + 1 < len && chars[peek + 1] == ':') {
                // It's a key! Wrap in quotes
                result.push('"');
                result.push_str(&ident);
                result.push('"');
                continue;
            } else {
                // Normal identifier (e.g. true, false, null)
                result.push_str(&ident);
                continue;
            }
        }

        // Check for trailing comma before } or ]
        if c == ',' {
            let mut peek = i + 1;
            while peek < len && chars[peek].is_whitespace() {
                peek += 1;
            }
            if peek < len && (chars[peek] == '}' || chars[peek] == ']') {
                // Skip trailing comma
                i += 1;
                continue;
            }
        }

        result.push(c);
        i += 1;
    }

    result
}

fn parse_js_ecosystem_native(content: &str) -> Option<Value> {
    let clean = strip_js_comments(content);

    // Look for `apps:` array
    if let Some(pos) = clean.find("apps") {
        if let Some(bracket_start) = clean[pos..].find('[') {
            let start = pos + bracket_start;
            // Find matching closing bracket
            let mut depth = 0;
            let mut end = None;
            for (idx, ch) in clean[start..].char_indices() {
                if ch == '[' {
                    depth += 1;
                } else if ch == ']' {
                    depth -= 1;
                    if depth == 0 {
                        end = Some(start + idx + 1);
                        break;
                    }
                }
            }

            if let Some(end_idx) = end {
                let slice = &clean[start..end_idx];
                let normalized = normalize_js_object_literal(slice);
                if let Ok(v) = serde_json::from_str::<Value>(&normalized) {
                    return Some(serde_json::json!({ "apps": v }));
                }
            }
        }
    }

    // Try finding root object { ... }
    if let Some(start) = clean.find('{') {
        if let Some(end) = clean.rfind('}') {
            let slice = &clean[start..=end];
            let normalized = normalize_js_object_literal(slice);
            if let Ok(v) = serde_json::from_str::<Value>(&normalized) {
                if v.get("apps").is_some() {
                    return Some(v);
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

        let (command, args) =
            if script.ends_with(".js") || script.ends_with(".cjs") || script.ends_with(".mjs") {
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
            health: None,
            smtp: None,
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
                health: None,
                smtp: None,
            },
        );

        let toml_str = to_orion_toml(&apps).expect("toml serialization failed");
        assert!(toml_str.contains("[app.worker]"));
        assert!(toml_str.contains(r#"command = "python""#));
        assert!(toml_str.contains("max_memory_mb = 256"));
    }

    #[test]
    fn test_parse_js_ecosystem_native() {
        let js_raw = r#"
            // PM2 Ecosystem file with comments
            module.exports = {
                apps: [
                    {
                        name: 'my-service',
                        script: './app.js',
                        instances: 2,
                        exec_mode: 'cluster',
                        watch: false,
                        env: {
                            NODE_ENV: 'production',
                            PORT: 3000,
                        },
                    }
                ],
            };
        "#;

        let val = parse_js_ecosystem_native(js_raw).expect("native parsing failed");
        let apps = parse_apps_from_json(&val, Path::new(".")).unwrap();
        assert_eq!(apps.len(), 1);

        let app = apps.get("my-service").expect("app missing");
        assert_eq!(app.command, "node");
        assert_eq!(app.args, vec!["./app.js"]);
        assert_eq!(app.instances, 2);
        assert_eq!(app.mode, ExecutionMode::Cluster);
        assert_eq!(app.env.get("PORT"), Some(&"3000".to_string()));
    }
}
