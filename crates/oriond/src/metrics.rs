use orion_core::process_state::ProcessState;
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use sysinfo::{Pid, ProcessRefreshKind, RefreshKind, System};
use tokio::time::sleep;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use crate::logger::LogBus;
use crate::registry::ProcessRegistry;

/// Background task that collects CPU and RSS memory metrics for all running processes.
/// Fix P1 Bug 6: Sums metrics across the entire process tree (descendant processes)
/// so that apps launched via `npm start`, `yarn`, or shell scripts reflect their actual footprint.
pub async fn run(registry: ProcessRegistry, _logger: LogBus, shutdown: CancellationToken) {
    info!("Metrics collector started");
    let mut system = System::new_with_specifics(
        RefreshKind::new().with_processes(ProcessRefreshKind::everything()),
    );
    let mut last_cpu_alert: HashMap<String, Instant> = HashMap::new();

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
            if system.process(sysinfo_pid).is_some() {
                // Sum metrics across the entire process tree
                let (cpu, mem) = aggregate_tree_metrics(&system, sysinfo_pid);
                registry.update_metrics(&name, cpu, mem);
                debug!(name = %name, pid = pid, cpu = cpu, memory_bytes = mem, "Updated process tree metrics");

                if let Some(config) = registry.get_config(&name) {
                    // Check max memory limit auto-restart
                    if config.max_memory_mb > 0 {
                        let limit_bytes = config.max_memory_mb * 1024 * 1024;
                        if mem > limit_bytes {
                            let mem_mb = mem / (1024 * 1024);
                            let triggered = registry.trigger_memory_restart(
                                &name,
                                mem_mb,
                                config.max_memory_mb,
                            );
                            if triggered {
                                warn!(
                                    name = %name,
                                    mem_mb = mem_mb,
                                    limit_mb = config.max_memory_mb,
                                    "Process exceeded memory threshold; triggered supervisor restart"
                                );
                            }
                        }
                    }

                    // Check high CPU alert threshold with 60-second cooldown (Fix P1 Bug 3)
                    if config.max_cpu_pct > 0.0 && cpu > config.max_cpu_pct {
                        let now = Instant::now();
                        let should_fire = last_cpu_alert
                            .get(&name)
                            .map(|last| now.duration_since(*last) >= Duration::from_secs(60))
                            .unwrap_or(true);

                        if should_fire {
                            last_cpu_alert.insert(name.clone(), now);
                            if let Some(ref webhook_url) = config.alerts.on_high_cpu_webhook {
                                let url = webhook_url.clone();
                                let app_name = name.clone();
                                let current_cpu = cpu;
                                let threshold = config.max_cpu_pct;
                                tokio::spawn(async move {
                                    send_high_cpu_webhook(&url, &app_name, current_cpu, threshold)
                                        .await;
                                });
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Aggregates CPU and memory across root_pid and all its child/grandchild processes.
fn aggregate_tree_metrics(system: &System, root_pid: Pid) -> (f32, u64) {
    let mut pids_in_tree = HashSet::new();
    pids_in_tree.insert(root_pid);

    let mut changed = true;
    while changed {
        changed = false;
        for (pid, proc) in system.processes() {
            if !pids_in_tree.contains(pid) {
                if let Some(parent) = proc.parent() {
                    if pids_in_tree.contains(&parent) {
                        pids_in_tree.insert(*pid);
                        changed = true;
                    }
                }
            }
        }
    }

    let mut total_cpu = 0.0;
    let mut total_mem = 0u64;

    for pid in pids_in_tree {
        if let Some(proc) = system.process(pid) {
            total_cpu += proc.cpu_usage();
            total_mem += proc.memory();
        }
    }

    (total_cpu, total_mem)
}

pub async fn send_high_memory_webhook(url: &str, app_name: &str, memory_bytes: u64, limit_mb: u64) {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap_or_default();
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
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap_or_default();
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
