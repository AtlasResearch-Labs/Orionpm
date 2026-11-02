use anyhow::{bail, Context, Result};
use clap::Args;
use orion_core::config::{AppConfig, ExecutionMode, OrionConfig, RestartPolicy};
use orion_core::pm2::{parse_memory_string, parse_pm2_ecosystem_with_profile};
use orion_core::procfile::parse_procfile;
use oriond::runtime::{run_foreground, RuntimeOptions};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Args, Debug, Clone)]
pub struct RuntimeArgs {
    /// Path to a orion.toml config file, PM2 ecosystem file, Procfile, or script to run directly.
    pub target: String,

    /// Name for the process (required when target is a script, not a config file).
    #[arg(long)]
    pub name: Option<String>,

    /// Namespace / group name for the process.
    #[arg(long, short = 'n', alias = "ns")]
    pub namespace: Option<String>,

    /// Number of instances to spawn in foreground.
    #[arg(long, short = 'i')]
    pub instances: Option<u32>,

    /// Emit raw stdout/stderr directly without timestamp or process name prefixes.
    #[arg(long)]
    pub raw: bool,

    /// Emit JSON-formatted log lines for container log collectors (Loki, Fluentd, CloudWatch).
    #[arg(long)]
    pub json: bool,

    /// HTTP port to serve Kubernetes readiness and liveness probe at `/healthz`.
    #[arg(long = "health-port")]
    pub health_port: Option<u16>,

    /// Working directory for the process.
    #[arg(long)]
    pub cwd: Option<String>,

    /// Environment variables in KEY=VALUE format (repeatable).
    #[arg(long = "env", value_name = "KEY=VALUE|PROFILE")]
    pub env: Vec<String>,

    /// Environment profile to activate from PM2 ecosystem config (e.g. "production", "staging").
    #[arg(long = "env-profile", alias = "profile")]
    pub env_profile: Option<String>,

    /// Maximum memory limit (e.g. 500M, 1G) before automatic restart.
    #[arg(long = "max-memory")]
    pub max_memory: Option<String>,

    /// Optional public port for universal cluster load balancing (e.g. 3000).
    #[arg(long = "port", short = 'p')]
    pub port: Option<u16>,

    /// Timeout in seconds to await graceful child exit on SIGTERM before SIGKILL escalation (default: 5).
    #[arg(long = "kill-timeout", default_value = "5")]
    pub kill_timeout: u64,
}

pub async fn run(args: RuntimeArgs) -> Result<()> {
    let target_path = Path::new(&args.target);
    let mut apps: Vec<(String, AppConfig)> = Vec::new();

    // 1. If target is a .toml file, parse as native multi-app config
    if target_path.extension().and_then(|s| s.to_str()) == Some("toml") && target_path.exists() {
        let config = OrionConfig::from_file(target_path)
            .with_context(|| format!("Failed to parse config file: {}", args.target))?;

        if config.apps.is_empty() {
            bail!("Config file '{}' does not define any apps", args.target);
        }

        for (name, mut app_config) in config.apps {
            if let Some(i) = args.instances {
                app_config.instances = i;
            }
            if let Some(ref ns) = args.namespace {
                app_config.namespace = Some(ns.clone());
            }
            if let Some(ref m) = args.max_memory {
                if let Some(mb) = parse_memory_string(m) {
                    app_config.max_memory_mb = mb;
                }
            }
            app_config.kill_timeout_secs = args.kill_timeout;
            expand_instances(&mut apps, name, app_config);
        }
    }
    // 2. If target is a JS/JSON PM2 ecosystem file
    else if target_path.exists()
        && (target_path
            .file_name()
            .and_then(|s| s.to_str())
            .map(|n| n.contains("ecosystem"))
            .unwrap_or(false)
            || target_path.extension().and_then(|s| s.to_str()) == Some("cjs")
            || target_path.extension().and_then(|s| s.to_str()) == Some("mjs"))
    {
        println!("Detected PM2 ecosystem file '{}'. Parsing...", args.target);
        let parsed_apps =
            parse_pm2_ecosystem_with_profile(target_path, args.env_profile.as_deref())
                .with_context(|| format!("Failed to parse PM2 ecosystem file: {}", args.target))?;

        if parsed_apps.is_empty() {
            bail!(
                "PM2 ecosystem file '{}' did not yield any apps",
                args.target
            );
        }

        for (name, mut app_config) in parsed_apps {
            if let Some(i) = args.instances {
                app_config.instances = i;
            }
            if let Some(ref ns) = args.namespace {
                app_config.namespace = Some(ns.clone());
            }
            if let Some(ref m) = args.max_memory {
                if let Some(mb) = parse_memory_string(m) {
                    app_config.max_memory_mb = mb;
                }
            }
            app_config.kill_timeout_secs = args.kill_timeout;
            expand_instances(&mut apps, name, app_config);
        }
    }
    // 3. If target is a Procfile
    else if target_path.exists()
        && (target_path
            .file_name()
            .and_then(|s| s.to_str())
            .map(|n| n.starts_with("Procfile") || n.ends_with(".procfile"))
            .unwrap_or(false))
    {
        println!(
            "Detected Procfile in '{}'. Parsing services...",
            args.target
        );
        let parsed_apps = parse_procfile(target_path)
            .with_context(|| format!("Failed to parse Procfile: {}", args.target))?;

        if parsed_apps.is_empty() {
            bail!("Procfile '{}' does not define any services", args.target);
        }

        for (name, mut app_config) in parsed_apps {
            if let Some(i) = args.instances {
                app_config.instances = i;
            }
            if let Some(ref ns) = args.namespace {
                app_config.namespace = Some(ns.clone());
            }
            app_config.kill_timeout_secs = args.kill_timeout;
            expand_instances(&mut apps, name, app_config);
        }
    }
    // 4. Single target script or executable
    else {
        let app_name = args.name.clone().unwrap_or_else(|| {
            target_path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("app")
                .to_string()
        });

        let mut env_map = HashMap::new();
        for kv in &args.env {
            if let Some((k, v)) = kv.split_once('=') {
                env_map.insert(k.trim().to_string(), v.trim().to_string());
            }
        }

        let ext = target_path
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("");
        let command = match ext {
            "js" | "mjs" | "cjs" => format!("node {}", args.target),
            "py" => {
                #[cfg(windows)]
                {
                    format!("python {}", args.target)
                }
                #[cfg(not(windows))]
                {
                    format!("python3 {}", args.target)
                }
            }
            "sh" => format!("bash {}", args.target),
            _ => args.target.clone(),
        };

        let cwd = args
            .cwd
            .map(PathBuf::from)
            .or_else(|| target_path.parent().map(|p| p.to_path_buf()))
            .unwrap_or_else(|| PathBuf::from("."));

        let max_memory_mb = args.max_memory.as_deref().and_then(parse_memory_string);

        let app_config = AppConfig {
            command,
            args: Vec::new(),
            cwd: Some(cwd),
            name: Some(app_name.clone()),
            namespace: args.namespace.clone(),
            mode: ExecutionMode::Fork,
            env: env_map,
            instances: args.instances.unwrap_or(1),
            restart_policy: RestartPolicy::Always,
            restart_delay_ms: 1000,
            restart_backoff_multiplier: 2.0,
            max_restarts: 10,
            restart_window_secs: 300,
            cron_restart: None,
            watch: false,
            watch_paths: Vec::new(),
            watch_ignore: Vec::new(),
            stdout_log: None,
            stderr_log: None,
            log_lines: 1000,
            max_memory_mb: max_memory_mb.unwrap_or(0),
            max_cpu_pct: 0.0,
            alerts: Default::default(),
            health: None,
            smtp: None,
            kill_timeout_secs: args.kill_timeout,
            port: args.port,
        };

        expand_instances(&mut apps, app_name, app_config);
    }

    let runtime_options = RuntimeOptions {
        raw: args.raw,
        json: args.json,
        health_port: args.health_port,
    };

    run_foreground(apps, runtime_options).await
}

fn expand_instances(apps: &mut Vec<(String, AppConfig)>, name: String, config: AppConfig) {
    let count = if config.instances == 0 {
        1
    } else {
        config.instances
    };
    let base_port = config
        .port
        .or_else(|| config.env.get("PORT").and_then(|p| p.parse().ok()));

    if count <= 1 {
        apps.push((name, config));
    } else {
        for i in 0..count {
            let instance_name = format!("{}-{}", name, i);
            let mut worker_config = config.clone();
            if let Some(port) = base_port {
                let worker_port = port + 1 + (i as u16);
                worker_config
                    .env
                    .insert("PORT".to_string(), worker_port.to_string());
                worker_config.port = Some(port);
            }
            apps.push((instance_name, worker_config));
        }
    }
}
