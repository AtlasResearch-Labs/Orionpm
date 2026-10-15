use anyhow::{bail, Context, Result};
use clap::Args;
use orion_core::config::{AppConfig, ExecutionMode, HealthConfig, OrionConfig, RestartPolicy};
use orion_core::pm2::{parse_memory_string, parse_pm2_ecosystem};
use orion_core::procfile::parse_procfile;
use orion_core::protocol::{Method, Params, StartParams};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::client;
use super::list::{self, ListArgs};

#[derive(Args)]
pub struct StartArgs {
    /// Path to a orion.toml config file, PM2 ecosystem file, Procfile, or script to run directly.
    pub target: String,

    /// Name for the process (required when target is a script, not a config file).
    #[arg(long)]
    pub name: Option<String>,

    /// Number of instances to spawn. Overrides the config file value.
    #[arg(long, short = 'i')]
    pub instances: Option<u32>,

    /// Watch for file changes and auto-restart.
    #[arg(long)]
    pub watch: bool,

    /// Specific file or directory paths to watch for auto-restart (repeatable).
    #[arg(long = "watch-path", value_name = "PATH")]
    pub watch_paths: Vec<PathBuf>,

    /// Never restart this process automatically.
    #[arg(long, conflicts_with = "watch")]
    pub no_restart: bool,

    /// Environment variables in KEY=VALUE format (repeatable).
    #[arg(long = "env", value_name = "KEY=VALUE")]
    pub env: Vec<String>,

    /// Working directory for the process.
    #[arg(long)]
    pub cwd: Option<String>,

    /// Maximum memory limit (e.g. 500M, 1G) before automatic restart.
    #[arg(long = "max-memory")]
    pub max_memory: Option<String>,

    /// Path to mirror stdout logs.
    #[arg(long = "out")]
    pub stdout_log: Option<PathBuf>,

    /// Path to mirror stderr logs.
    #[arg(long = "error")]
    pub stderr_log: Option<PathBuf>,

    /// HTTP health check probe URL (e.g. "http://127.0.0.1:3000/healthz").
    #[arg(long = "health-http")]
    pub health_http: Option<String>,

    /// TCP health check probe target (e.g. "127.0.0.1:5432").
    #[arg(long = "health-tcp")]
    pub health_tcp: Option<String>,

    /// Health check interval in seconds (default: 10s).
    #[arg(long = "health-interval")]
    pub health_interval: Option<u64>,

    /// Health check max consecutive failures before restart (default: 3).
    #[arg(long = "health-max-failures")]
    pub health_max_failures: Option<u32>,
}

pub async fn run(args: StartArgs) -> Result<()> {
    let target_path = Path::new(&args.target);

    // 1. If target is a .toml file, parse as native multi-app config
    if target_path.extension().and_then(|s| s.to_str()) == Some("toml") && target_path.exists() {
        let config = OrionConfig::from_file(target_path)
            .with_context(|| format!("Failed to parse config file: {}", args.target))?;

        if config.apps.is_empty() {
            bail!("Config file '{}' does not define any apps", args.target);
        }

        for (name, mut app_config) in config.apps {
            if args.watch || !args.watch_paths.is_empty() {
                app_config.watch = true;
            }
            if !args.watch_paths.is_empty() {
                app_config.watch_paths = args.watch_paths.clone();
            }
            if args.no_restart {
                app_config.restart_policy = RestartPolicy::Never;
            }
            if let Some(i) = args.instances {
                app_config.instances = i;
            }
            if let Some(ref m) = args.max_memory {
                if let Some(mb) = parse_memory_string(m) {
                    app_config.max_memory_mb = mb;
                }
            }
            if args.health_http.is_some() || args.health_tcp.is_some() {
                app_config.health = Some(HealthConfig {
                    http: args.health_http.clone(),
                    tcp: args.health_tcp.clone(),
                    interval_secs: args.health_interval.unwrap_or(10),
                    timeout_secs: 3,
                    max_failures: args.health_max_failures.unwrap_or(3),
                });
            }

            spawn_app_instances(name, app_config).await?;
        }
    } else if target_path.exists()
        && (args.target.contains("ecosystem")
            || args.target.ends_with(".config.js")
            || args.target.ends_with(".config.json")
            || args.target.ends_with("pm2.json"))
    {
        // 2. Target is a recognized PM2 ecosystem file
        println!(
            "Detected PM2 configuration in '{}'. Parsing applications...",
            args.target
        );
        let apps = parse_pm2_ecosystem(target_path)
            .with_context(|| format!("Failed to parse PM2 configuration: {}", args.target))?;

        if apps.is_empty() {
            bail!(
                "PM2 configuration '{}' does not define any apps",
                args.target
            );
        }

        for (name, mut app_config) in apps {
            if args.watch || !args.watch_paths.is_empty() {
                app_config.watch = true;
            }
            if !args.watch_paths.is_empty() {
                app_config.watch_paths = args.watch_paths.clone();
            }
            if args.no_restart {
                app_config.restart_policy = RestartPolicy::Never;
            }
            if let Some(i) = args.instances {
                app_config.instances = i;
            }
            if let Some(ref m) = args.max_memory {
                if let Some(mb) = parse_memory_string(m) {
                    app_config.max_memory_mb = mb;
                }
            }
            if args.health_http.is_some() || args.health_tcp.is_some() {
                app_config.health = Some(HealthConfig {
                    http: args.health_http.clone(),
                    tcp: args.health_tcp.clone(),
                    interval_secs: args.health_interval.unwrap_or(10),
                    timeout_secs: 3,
                    max_failures: args.health_max_failures.unwrap_or(3),
                });
            }

            spawn_app_instances(name, app_config).await?;
        }
    } else if target_path.exists()
        && (target_path
            .file_name()
            .and_then(|s| s.to_str())
            .map(|n| {
                n.starts_with("Procfile") || n.ends_with(".procfile") || n.ends_with(".Procfile")
            })
            .unwrap_or(false))
    {
        // 3. Target is a recognized Procfile (Procfile, Procfile.dev, Procfile.local, etc.)
        println!(
            "Detected Procfile in '{}'. Parsing services...",
            args.target
        );
        let apps = parse_procfile(target_path)
            .with_context(|| format!("Failed to parse Procfile: {}", args.target))?;

        if apps.is_empty() {
            bail!("Procfile '{}' does not define any services", args.target);
        }

        for (name, mut app_config) in apps {
            if args.watch || !args.watch_paths.is_empty() {
                app_config.watch = true;
            }
            if !args.watch_paths.is_empty() {
                app_config.watch_paths = args.watch_paths.clone();
            }
            if args.no_restart {
                app_config.restart_policy = RestartPolicy::Never;
            }
            if let Some(i) = args.instances {
                app_config.instances = i;
            }
            if let Some(ref m) = args.max_memory {
                if let Some(mb) = parse_memory_string(m) {
                    app_config.max_memory_mb = mb;
                }
            }
            if args.health_http.is_some() || args.health_tcp.is_some() {
                app_config.health = Some(HealthConfig {
                    http: args.health_http.clone(),
                    tcp: args.health_tcp.clone(),
                    interval_secs: args.health_interval.unwrap_or(10),
                    timeout_secs: 3,
                    max_failures: args.health_max_failures.unwrap_or(3),
                });
            }

            spawn_app_instances(name, app_config).await?;
        }
    } else {
        // 4. Direct script or binary execution
        let (command, cmd_args) = if args.target.ends_with(".js") {
            ("node".to_string(), vec![args.target.clone()])
        } else if args.target.ends_with(".py") {
            let py = if cfg!(windows) { "python" } else { "python3" };
            (py.to_string(), vec![args.target.clone()])
        } else if args.target.ends_with(".sh") {
            ("bash".to_string(), vec![args.target.clone()])
        } else {
            (args.target.clone(), vec![])
        };

        let app_name = args.name.unwrap_or_else(|| {
            target_path
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| "app".to_string())
        });

        let mut env_map = HashMap::new();
        for item in &args.env {
            if let Some((k, v)) = item.split_once('=') {
                env_map.insert(k.to_string(), v.to_string());
            }
        }

        let max_memory_mb = args
            .max_memory
            .as_deref()
            .and_then(parse_memory_string)
            .unwrap_or(0);

        let effective_cwd = args
            .cwd
            .map(PathBuf::from)
            .or_else(|| std::env::current_dir().ok());

        let watch_paths = if args.watch_paths.is_empty() {
            vec![effective_cwd.clone().unwrap_or_else(|| PathBuf::from("."))]
        } else {
            args.watch_paths
                .iter()
                .map(|p| {
                    if p.is_relative() {
                        if let Some(ref base) = effective_cwd {
                            base.join(p)
                        } else {
                            p.clone()
                        }
                    } else {
                        p.clone()
                    }
                })
                .collect()
        };

        let mut app_config = AppConfig {
            command,
            args: cmd_args,
            cwd: effective_cwd,
            name: Some(app_name.clone()),
            instances: args.instances.unwrap_or(1),
            mode: ExecutionMode::Fork,
            env: env_map,
            restart_policy: if args.no_restart {
                RestartPolicy::Never
            } else {
                RestartPolicy::Always
            },
            restart_delay_ms: 1000,
            max_restarts: 10,
            restart_window_secs: 60,
            watch: args.watch || !args.watch_paths.is_empty(),
            watch_paths,
            watch_ignore: vec![
                "node_modules".into(),
                "target".into(),
                ".git".into(),
                "*.log".into(),
            ],
            stdout_log: args.stdout_log,
            stderr_log: args.stderr_log,
            log_lines: 1000,
            max_memory_mb,
            max_cpu_pct: 0.0,
            alerts: Default::default(),
            health: None,
            smtp: None,
        };

        if args.health_http.is_some() || args.health_tcp.is_some() {
            app_config.health = Some(HealthConfig {
                http: args.health_http.clone(),
                tcp: args.health_tcp.clone(),
                interval_secs: args.health_interval.unwrap_or(10),
                timeout_secs: 3,
                max_failures: args.health_max_failures.unwrap_or(3),
            });
        }

        spawn_app_instances(app_name, app_config).await?;
    }

    // Display status table after start
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    list::run(ListArgs::default()).await?;

    Ok(())
}

async fn spawn_app_instances(name: String, app_config: AppConfig) -> Result<()> {
    let num_instances = if app_config.instances == 0 {
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1) as u32
    } else {
        app_config.instances
    };

    if num_instances <= 1 {
        let start_params = StartParams {
            name: name.clone(),
            config: app_config,
        };
        client::rpc(Method::ProcessStart, Params::ProcessStart(start_params)).await?;
        println!("Started process '{}'", name);
    } else {
        for i in 0..num_instances {
            let instance_name = format!("{}-{}", name, i);
            let start_params = StartParams {
                name: instance_name.clone(),
                config: app_config.clone(),
            };
            client::rpc(Method::ProcessStart, Params::ProcessStart(start_params)).await?;
            println!("Started instance '{}'", instance_name);
        }
    }
    Ok(())
}
