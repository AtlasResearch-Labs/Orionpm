use std::time::{Duration, SystemTime, UNIX_EPOCH};
use sysinfo::{Pid, ProcessRefreshKind, RefreshKind, System};
use tokio::time::sleep;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};
use orion_core::process_state::ProcessState;
use orion_core::protocol::{LogLine, LogStream};

use crate::logger::LogBus;
use crate::registry::ProcessRegistry;

/// Background task that collects CPU and RSS memory metrics for all
/// running supervised processes using real OS data from sysinfo.
/// Also enforces max_memory_mb thresholds and triggers auto-restarts and alerts.
pub async fn run(registry: ProcessRegistry, logger: LogBus, shutdown: CancellationToken) {
    info!("Metrics collector started");
    let mut system = System::new_with_specifics(
        RefreshKind::new().with_processes(ProcessRefreshKind::everything()),
    );

    loop {
        tokio::select! {
            _ = sleep(Duration::from_millis(1500)) => {}
            _ = shutdown.cancelled() => {
                info!("Metrics collector shutting down");
                break;
            }
        }

        let processes = registry.list();
        let active_pids: Vec<(String, u32)> = processes
            .into_iter()
            .filter_map(|p| match p.state {
                ProcessState::Running { pid } => Some((p.name, pid)),
                _ => None,
            })
            .collect();

        if active_pids.is_empty() {
            continue;
        }

        system.refresh_processes_specifics(ProcessRefreshKind::everything());

        for (name, pid) in active_pids {
            let sysinfo_pid = Pid::from_u32(pid);
            if let Some(proc) = system.process(sysinfo_pid) {
                let cpu = proc.cpu_usage();
                let mem = proc.memory(); // RSS in bytes
                registry.update_metrics(&name, cpu, mem);
                debug!(name = %name, pid = pid, cpu = cpu, memory_bytes = mem, "Updated process metrics");

                if let Some(config) = registry.get_config(&name) {
                    // Check max memory limit auto-restart
                    if config.max_memory_mb > 0 {
                        let limit_bytes = config.max_memory_mb * 1024 * 1024;
                        if mem > limit_bytes {
                            let mem_mb = mem / (1024 * 1024);
                            warn!(
                                name = %name,
                                mem_mb = mem_mb,
                                limit_mb = config.max_memory_mb,
                                "Process exceeded memory threshold; initiating automatic restart"
                            );

                            logger.log(
                                &name,
                                LogLine {
                                    timestamp: unix_now(),
                                    stream: LogStream::Stderr,
                                    text: format!(
                                        "[orion] Memory threshold exceeded ({} MB > {} MB). Automatic restart triggered.",
                                        mem_mb, config.max_memory_mb
                                    ),
                                },
                            );

                            // Send high memory webhook if configured
                            if let Some(ref webhook_url) = config.alerts.on_high_memory_webhook {
                                let url = webhook_url.clone();
                                let app_name = name.clone();
                                let current_mem = mem;
                                let limit = config.max_memory_mb;
                                tokio::spawn(async move {
                                    send_high_memory_webhook(&url, &app_name, current_mem, limit).await;
                                });
                            }

                            // Trigger process restart
                            if let Some(cancel) = registry.supervisor_cancel(&name) {
                                cancel.cancel();
                            }
                            if let Some(new_token) = registry.reset_cancel_token(&name) {
                                crate::supervisor::spawn(
                                    name.clone(),
                                    config.clone(),
                                    registry.clone(),
                                    logger.clone(),
                                    new_token,
                                );
                            }
                        }
                    }

                    // Check high CPU alert threshold
                    if config.max_cpu_pct > 0.0 && cpu > config.max_cpu_pct {
                        if let Some(ref webhook_url) = config.alerts.on_high_cpu_webhook {
                            let url = webhook_url.clone();
                            let app_name = name.clone();
                            let current_cpu = cpu;
                            let threshold = config.max_cpu_pct;
                            tokio::spawn(async move {
                                send_high_cpu_webhook(&url, &app_name, current_cpu, threshold).await;
                            });
                        }
                    }
                }
            }
        }
    }
}

async fn send_high_memory_webhook(url: &str, app_name: &str, memory_bytes: u64, limit_mb: u64) {
    let client = reqwest::Client::new();
    let payload = serde_json::json!({
        "event": "high_memory_limit_exceeded",
        "app": app_name,
        "memory_mb": memory_bytes / (1024 * 1024),
        "limit_mb": limit_mb,
        "timestamp": unix_now(),
    });

    if let Err(e) = client.post(url).json(&payload).send().await {
        error!(url = %url, error = %e, "Failed to deliver high memory webhook");
    }
}

async fn send_high_cpu_webhook(url: &str, app_name: &str, cpu_pct: f32, threshold_pct: f32) {
    let client = reqwest::Client::new();
    let payload = serde_json::json!({
        "event": "high_cpu_threshold_exceeded",
        "app": app_name,
        "cpu_pct": cpu_pct,
        "threshold_pct": threshold_pct,
        "timestamp": unix_now(),
    });

    if let Err(e) = client.post(url).json(&payload).send().await {
        error!(url = %url, error = %e, "Failed to deliver high CPU webhook");
    }
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
