use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Top-level orion.toml structure.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrionConfig {
    /// Map of app name -> app config block.
    #[serde(rename = "app", default)]
    pub apps: HashMap<String, AppConfig>,

    /// Optional SMTP config for email alerts.
    pub smtp: Option<SmtpConfig>,
}

/// Configuration for a single managed process.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    /// The executable to run (e.g. "node", "python3", "/usr/bin/my-app").
    pub command: String,

    /// Arguments passed to the command.
    #[serde(default)]
    pub args: Vec<String>,

    /// Working directory. If not set, defaults to the directory of the config file.
    pub cwd: Option<PathBuf>,

    /// Human-readable display name. Defaults to the map key (app name).
    pub name: Option<String>,

    /// Number of instances to spawn. Use 0 to mean "number of logical CPUs".
    /// Default: 1.
    #[serde(default = "default_instances")]
    pub instances: u32,

    /// Execution mode. Only "cluster" mode applies to Node.js multi-process load balancing.
    /// All other runtimes must use "fork".
    #[serde(default)]
    pub mode: ExecutionMode,

    /// Environment variables injected into the process.
    #[serde(default)]
    pub env: HashMap<String, String>,

    /// Restart policy controlling when oriond will restart the process after exit.
    #[serde(default)]
    pub restart_policy: RestartPolicy,

    /// Milliseconds to wait before attempting a restart. Default: 1000.
    #[serde(default = "default_restart_delay_ms")]
    pub restart_delay_ms: u64,

    /// Maximum number of restarts before oriond stops trying. 0 means unlimited.
    /// Default: 10.
    #[serde(default = "default_max_restarts")]
    pub max_restarts: u32,

    /// Rolling window in seconds for counting restarts. If max_restarts is reached
    /// within this window, oriond stops. 0 means count restarts cumulatively forever.
    /// Default: 60.
    #[serde(default = "default_restart_window_secs")]
    pub restart_window_secs: u64,

    /// Watch source paths for file changes and auto-restart on change.
    #[serde(default)]
    pub watch: bool,

    /// Paths to watch when watch = true. Defaults to ["."].
    #[serde(default = "default_watch_paths")]
    pub watch_paths: Vec<PathBuf>,

    /// Glob patterns to exclude from watch.
    #[serde(default = "default_watch_ignore")]
    pub watch_ignore: Vec<String>,

    /// Path to write stdout. If None, oriond manages an internal ring buffer only.
    pub stdout_log: Option<PathBuf>,

    /// Path to write stderr. If None, oriond manages an internal ring buffer only.
    pub stderr_log: Option<PathBuf>,

    /// Maximum number of log lines to keep in the in-memory ring buffer per process.
    /// Default: 1000.
    #[serde(default = "default_log_lines")]
    pub log_lines: usize,

    /// Maximum RSS memory in MB before oriond sends SIGTERM and restarts the process.
    /// 0 means no memory limit enforced. Default: 0.
    #[serde(default)]
    pub max_memory_mb: u64,

    /// Maximum CPU percentage (averaged over 30 seconds) before an alert fires.
    /// 0 means no CPU alert threshold. Default: 0.
    #[serde(default)]
    pub max_cpu_pct: f32,

    /// Alert configuration for this app.
    #[serde(default)]
    pub alerts: AlertConfig,
}

/// Execution mode for a managed process.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ExecutionMode {
    /// Standard single-process execution. Works for all runtimes.
    #[default]
    Fork,

    /// Node.js cluster module for multi-process load balancing.
    /// Only valid when command resolves to a Node.js runtime.
    Cluster,
}

/// Restart policy controlling how oriond handles process exits.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum RestartPolicy {
    /// Always restart, regardless of exit code. Default.
    #[default]
    Always,

    /// Restart only when the process exits with a non-zero exit code.
    OnFailure,

    /// Never restart. Process is managed but not recovered.
    Never,
}

/// Alert rules for a single app.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AlertConfig {
    /// HTTP webhook URL to POST to when this process crashes.
    pub on_crash_webhook: Option<String>,

    /// Email address to notify when this process crashes.
    /// Requires a valid [smtp] block in the config.
    pub on_crash_email: Option<String>,

    /// HTTP webhook URL to POST to when memory exceeds max_memory_mb.
    pub on_high_memory_webhook: Option<String>,

    /// HTTP webhook URL to POST to when CPU exceeds max_cpu_pct for sustained period.
    pub on_high_cpu_webhook: Option<String>,
}

/// SMTP configuration for email alert delivery.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SmtpConfig {
    /// SMTP server hostname.
    pub host: String,

    /// SMTP server port.
    pub port: u16,

    /// SMTP authentication username.
    pub username: String,

    /// Name of the environment variable holding the SMTP password.
    /// The password is never read from the config file directly.
    pub password_env: String,

    /// Sender address shown in the From header.
    pub from: String,

    /// Whether to use TLS. Default: true.
    #[serde(default = "default_true")]
    pub tls: bool,
}

// --- Default value functions used by serde ---

fn default_instances() -> u32 {
    1
}
fn default_restart_delay_ms() -> u64 {
    1000
}
fn default_max_restarts() -> u32 {
    10
}
fn default_restart_window_secs() -> u64 {
    60
}
fn default_log_lines() -> usize {
    1000
}
fn default_true() -> bool {
    true
}

fn default_watch_paths() -> Vec<PathBuf> {
    vec![PathBuf::from(".")]
}

fn default_watch_ignore() -> Vec<String> {
    vec![
        "node_modules".to_string(),
        "target".to_string(),
        "dist".to_string(),
        ".git".to_string(),
        "*.log".to_string(),
    ]
}

impl OrionConfig {
    /// Parse a orion.toml from a file path.
    pub fn from_file(path: &std::path::Path) -> Result<Self> {
        let raw = std::fs::read_to_string(path)
            .with_context(|| format!("Cannot read config file: {}", path.display()))?;
        Self::from_str(&raw)
    }

    /// Parse a orion.toml from a raw TOML string.
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(raw: &str) -> Result<Self> {
        toml::from_str(raw).context("Failed to parse orion.toml")
    }

    /// Validate all app configs for logical consistency.
    /// Returns a list of human-readable error strings.
    pub fn validate(&self) -> Vec<String> {
        let mut errors = Vec::new();

        for (key, app) in &self.apps {
            if app.command.trim().is_empty() {
                errors.push(format!("[app.{}] command must not be empty", key));
            }

            if app.instances == 0 {
                // 0 is reserved for "number of CPUs" — valid, not an error.
            }

            if app.mode == ExecutionMode::Cluster && app.instances == 1 {
                errors.push(format!(
                    "[app.{}] cluster mode with instances=1 has no effect. Use fork mode or set instances > 1.",
                    key
                ));
            }

            if app.max_cpu_pct > 100.0 {
                errors.push(format!(
                    "[app.{}] max_cpu_pct cannot exceed 100.0 (got {})",
                    key, app.max_cpu_pct
                ));
            }

            if let Some(ref email) = app.alerts.on_crash_email {
                if self.smtp.is_none() {
                    errors.push(format!(
                        "[app.{}] on_crash_email is set to '{}' but no [smtp] block is configured",
                        key, email
                    ));
                }
            }
        }

        errors
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_minimal_config() {
        let raw = r#"
            [app.my-service]
            command = "node"
            args = ["dist/server.js"]
        "#;
        let config = OrionConfig::from_str(raw).expect("parse failed");
        let app = config.apps.get("my-service").expect("app not found");
        assert_eq!(app.command, "node");
        assert_eq!(app.args, vec!["dist/server.js"]);
        assert_eq!(app.instances, 1);
        assert_eq!(app.restart_policy, RestartPolicy::Always);
    }

    #[test]
    fn validate_catches_empty_command() {
        let raw = r#"
            [app.bad]
            command = ""
        "#;
        let config = OrionConfig::from_str(raw).expect("parse failed");
        let errors = config.validate();
        assert!(
            !errors.is_empty(),
            "expected validation error for empty command"
        );
    }

    #[test]
    fn validate_catches_email_without_smtp() {
        let raw = r#"
            [app.my-service]
            command = "node"

            [app.my-service.alerts]
            on_crash_email = "ops@example.com"
        "#;
        let config = OrionConfig::from_str(raw).expect("parse failed");
        let errors = config.validate();
        assert!(
            errors.iter().any(|e| e.contains("smtp")),
            "expected smtp validation error"
        );
    }
}
