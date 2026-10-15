use anyhow::{bail, Context, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::config::{AppConfig, ExecutionMode, RestartPolicy};

/// Parse a Procfile from disk into a list of (process_name, AppConfig) tuples.
pub fn parse_procfile(path: &Path) -> Result<Vec<(String, AppConfig)>> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read Procfile: {}", path.display()))?;
    let parent = path.parent().filter(|p| !p.as_os_str().is_empty());
    parse_procfile_str(&content, parent)
}

/// Parse the raw contents of a Procfile into a list of (process_name, AppConfig) tuples.
pub fn parse_procfile_str(
    content: &str,
    base_dir: Option<&Path>,
) -> Result<Vec<(String, AppConfig)>> {
    let cwd_val = match base_dir {
        Some(b) if !b.as_os_str().is_empty() => Some(b.to_path_buf()),
        _ => std::env::current_dir().ok().or_else(|| Some(PathBuf::from("."))),
    };
    let mut apps = Vec::new();

    for (line_num, raw_line) in content.lines().enumerate() {
        let line = raw_line.trim();

        // Skip blank lines and comment lines
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        // Procfile line format: <name>: <command...>
        let (name, command_str) = match line.split_once(':') {
            Some((n, c)) => (n.trim(), c.trim()),
            None => {
                bail!(
                    "Invalid Procfile line {}: expected '<name>: <command>', got '{}'",
                    line_num + 1,
                    line
                );
            }
        };

        if name.is_empty() {
            bail!(
                "Invalid Procfile line {}: process name cannot be empty",
                line_num + 1
            );
        }

        if command_str.is_empty() {
            bail!(
                "Invalid Procfile line {}: command cannot be empty for '{}'",
                line_num + 1,
                name
            );
        }

        let words = split_shell_words(command_str);
        if words.is_empty() {
            bail!(
                "Invalid Procfile line {}: command is empty after parsing",
                line_num + 1
            );
        }

        // Support inline environment variables:
        // e.g.: web: PORT=5000 NODE_ENV=production node server.js
        let mut env = HashMap::new();
        let mut command = String::new();
        let mut args = Vec::new();
        for token in words {
            if command.is_empty() {
                if let Some((k, v)) = token.split_once('=') {
                    if !k.is_empty() && k.chars().all(|c| c.is_alphanumeric() || c == '_') {
                        env.insert(k.to_string(), v.to_string());
                        continue;
                    }
                }
                command = token;
            } else {
                args.push(token);
            }
        }

        if command.is_empty() {
            bail!(
                "Invalid Procfile line {}: no executable command found for '{}'",
                line_num + 1,
                name
            );
        }

        let app_config = AppConfig {
            command,
            args,
            cwd: cwd_val.clone(),
            name: Some(name.to_string()),
            instances: 1,
            mode: ExecutionMode::Fork,
            env,
            restart_policy: RestartPolicy::Always,
            restart_delay_ms: 1000,
            max_restarts: 10,
            restart_window_secs: 60,
            watch: false,
            watch_paths: vec![PathBuf::from(".")],
            watch_ignore: vec![
                "node_modules".into(),
                "target".into(),
                ".git".into(),
                "*.log".into(),
            ],
            stdout_log: None,
            stderr_log: None,
            log_lines: 1000,
            max_memory_mb: 0,
            max_cpu_pct: 0.0,
            alerts: Default::default(),
            health: None,
            smtp: None,
        };

        apps.push((name.to_string(), app_config));
    }

    Ok(apps)
}

/// Tokenize a shell command string into words, respecting single and double quotes.
pub fn split_shell_words(s: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut in_single_quote = false;
    let mut in_double_quote = false;
    let mut escaped = false;

    for ch in s.chars() {
        if escaped {
            current.push(ch);
            escaped = false;
            continue;
        }

        match ch {
            '\\' if !in_single_quote => {
                escaped = true;
            }
            '\'' if !in_double_quote => {
                in_single_quote = !in_single_quote;
            }
            '"' if !in_single_quote => {
                in_double_quote = !in_double_quote;
            }
            c if c.is_whitespace() && !in_single_quote && !in_double_quote => {
                if !current.is_empty() {
                    words.push(current.clone());
                    current.clear();
                }
            }
            _ => {
                current.push(ch);
            }
        }
    }

    if !current.is_empty() {
        words.push(current);
    }

    words
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_split_shell_words() {
        let words = split_shell_words("node server.js --port 3000");
        assert_eq!(words, vec!["node", "server.js", "--port", "3000"]);

        let quoted = split_shell_words("python app.py --msg \"hello world\" --flag 'simple value'");
        assert_eq!(
            quoted,
            vec![
                "python",
                "app.py",
                "--msg",
                "hello world",
                "--flag",
                "simple value"
            ]
        );
    }

    #[test]
    fn test_parse_procfile_basic() {
        let content = r#"
# Standard web service
web: npm start

# Background worker
worker: python -u worker.py --concurrency 4
"#;
        let apps = parse_procfile_str(content, None).expect("failed to parse Procfile");
        assert_eq!(apps.len(), 2);

        assert_eq!(apps[0].0, "web");
        assert_eq!(apps[0].1.command, "npm");
        assert_eq!(apps[0].1.args, vec!["start"]);

        assert_eq!(apps[1].0, "worker");
        assert_eq!(apps[1].1.command, "python");
        assert_eq!(
            apps[1].1.args,
            vec!["-u", "worker.py", "--concurrency", "4"]
        );
    }

    #[test]
    fn test_parse_procfile_inline_env() {
        let content = "web: PORT=8080 NODE_ENV=production node server.js\n";
        let apps = parse_procfile_str(content, None).expect("failed to parse Procfile");
        assert_eq!(apps.len(), 1);

        let (name, app) = &apps[0];
        assert_eq!(name, "web");
        assert_eq!(app.command, "node");
        assert_eq!(app.args, vec!["server.js"]);
        assert_eq!(app.env.get("PORT").map(|s| s.as_str()), Some("8080"));
        assert_eq!(
            app.env.get("NODE_ENV").map(|s| s.as_str()),
            Some("production")
        );
    }

    #[test]
    fn test_parse_procfile_invalid_line() {
        let content = "just a random line without colon\n";
        let result = parse_procfile_str(content, None);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_procfile_cwd_not_empty() {
        let content = "web: npm start\n";
        // Passing Some(Path::new("")) should be sanitized to current directory, never empty string
        let apps = parse_procfile_str(content, Some(Path::new(""))).expect("failed to parse");
        assert_eq!(apps.len(), 1);
        let cwd = apps[0].1.cwd.as_ref().expect("cwd must be set");
        assert!(!cwd.as_os_str().is_empty());
    }
}
