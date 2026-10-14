use anyhow::Result;
use orion_core::protocol::error_codes;
use orion_core::protocol::{
    LogsParams, Method, NameParams, Request, Response, ResponseResult, ScaleParams,
    SendSignalParams, StartParams,
};
use orion_ipc::server::ResponseWriter;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use tracing::info;

use crate::logger::LogBus;
use crate::registry::ProcessRegistry;
use crate::storage::Storage;

/// Start the IPC server and dispatch incoming client requests.
pub async fn serve(
    registry: ProcessRegistry,
    storage: Storage,
    logger: LogBus,
    shutdown: CancellationToken,
) -> Result<()> {
    let start_time = Instant::now();

    let handler_registry = registry.clone();
    let handler_storage = storage.clone();
    let handler_logger = logger.clone();
    let handler_shutdown = shutdown.clone();

    let handler = move |request: Request, writer: Arc<Mutex<ResponseWriter>>| {
        let registry = handler_registry.clone();
        let storage = handler_storage.clone();
        let logger = handler_logger.clone();
        let daemon_shutdown = handler_shutdown.clone();

        async move {
            dispatch_request(
                request,
                writer,
                registry,
                storage,
                logger,
                start_time,
                daemon_shutdown,
            )
            .await
        }
    };

    orion_ipc::server::run(handler, shutdown).await
}

async fn dispatch_request(
    request: Request,
    writer: Arc<Mutex<ResponseWriter>>,
    registry: ProcessRegistry,
    storage: Storage,
    logger: LogBus,
    start_time: Instant,
    daemon_shutdown: CancellationToken,
) -> Result<()> {
    let req_id = request.id;

    match request.method {
        Method::ProcessStart => {
            if let Ok(StartParams { config, name }) = request.parse_params() {
                let cancel_token = if registry.contains(&name) {
                    // If process is already registered and running, stop the old one first
                    if let Some(cancel) = registry.supervisor_cancel(&name) {
                        cancel.cancel();
                    }
                    registry.reset_cancel_token(&name).unwrap_or_default()
                } else {
                    let _ = registry.register(name.clone(), config.clone());
                    registry.supervisor_cancel(&name).unwrap_or_default()
                };

                crate::supervisor::spawn(
                    name.clone(),
                    config,
                    registry.clone(),
                    logger.clone(),
                    cancel_token,
                );

                let resp = Response::ok(
                    req_id,
                    ResponseResult::Ack {
                        message: format!("Process '{}' started", name),
                    },
                );
                writer.lock().await.send(&resp).await?;
            } else {
                send_invalid_params(req_id, writer).await?;
            }
        }

        Method::ProcessStop => {
            if let Ok(NameParams { name }) = request.parse_params() {
                let targets = registry.resolve_targets(&name);
                if targets.is_empty() {
                    send_not_found(req_id, &name, writer).await?;
                } else {
                    for target_name in &targets {
                        if let Some(cancel) = registry.supervisor_cancel(target_name) {
                            cancel.cancel();
                        }
                    }
                    let resp = Response::ok(
                        req_id,
                        ResponseResult::Ack {
                            message: if targets.len() == 1 {
                                format!("Process '{}' stopping", targets[0])
                            } else {
                                format!("{} processes stopping", targets.len())
                            },
                        },
                    );
                    writer.lock().await.send(&resp).await?;
                }
            } else {
                send_invalid_params(req_id, writer).await?;
            }
        }

        Method::ProcessRestart => {
            if let Ok(NameParams { name }) = request.parse_params() {
                let targets = registry.resolve_targets(&name);
                if targets.is_empty() {
                    send_not_found(req_id, &name, writer).await?;
                } else {
                    for target_name in &targets {
                        if let Some(config) = registry.get_config(target_name) {
                            if let Some(cancel) = registry.supervisor_cancel(target_name) {
                                cancel.cancel();
                            }
                            // Pause to allow socket release
                            tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
                            if let Some(new_token) = registry.reset_cancel_token(target_name) {
                                crate::supervisor::spawn(
                                    target_name.clone(),
                                    config,
                                    registry.clone(),
                                    logger.clone(),
                                    new_token,
                                );
                            }
                        }
                    }
                    let resp = Response::ok(
                        req_id,
                        ResponseResult::Ack {
                            message: if targets.len() == 1 {
                                format!("Process '{}' restarted", targets[0])
                            } else {
                                format!("{} processes restarted", targets.len())
                            },
                        },
                    );
                    writer.lock().await.send(&resp).await?;
                }
            } else {
                send_invalid_params(req_id, writer).await?;
            }
        }

        // Graceful reload:
        //   1. Gracefully stop old supervisor so network ports and file locks are released.
        //   2. Wait for the old process to fully exit (up to 2 s).
        //   3. Spawn fresh supervisor with new cancellation token.
        //   4. Wait for new instance to reach active state before replying.
        Method::ProcessReload => {
            if let Ok(NameParams { name }) = request.parse_params() {
                let targets = registry.resolve_targets(&name);
                if targets.is_empty() {
                    send_not_found(req_id, &name, writer).await?;
                } else {
                    for target_name in &targets {
                        if let Some(config) = registry.get_config(target_name) {
                            let old_cancel = registry.supervisor_cancel(target_name);

                            // 1. Gracefully stop the old supervisor.
                            if let Some(cancel) = old_cancel {
                                cancel.cancel();
                            }

                            // 2. Wait up to 2 seconds for previous process to release ports and exit.
                            let mut elapsed = 0u64;
                            loop {
                                tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                                elapsed += 100;
                                let still_running = registry
                                    .get(target_name)
                                    .map(|p| p.state.is_active())
                                    .unwrap_or(false);
                                if !still_running || elapsed >= 2000 {
                                    break;
                                }
                            }

                            // 3. Reset cancel token and spawn fresh supervisor.
                            if let Some(new_token) = registry.reset_cancel_token(target_name) {
                                crate::supervisor::spawn(
                                    target_name.clone(),
                                    config,
                                    registry.clone(),
                                    logger.clone(),
                                    new_token,
                                );
                            }

                            // 4. Wait for new process to become active.
                            let mut elapsed = 0u64;
                            loop {
                                tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                                elapsed += 100;
                                let is_running = registry
                                    .get(target_name)
                                    .map(|p| p.state.is_active())
                                    .unwrap_or(false);
                                if is_running || elapsed >= 2000 {
                                    break;
                                }
                            }
                        }
                    }

                    let resp = Response::ok(
                        req_id,
                        ResponseResult::Ack {
                            message: if targets.len() == 1 {
                                format!("Process '{}' reloaded gracefully", targets[0])
                            } else {
                                format!("{} processes reloaded gracefully", targets.len())
                            },
                        },
                    );
                    writer.lock().await.send(&resp).await?;
                }
            } else {
                send_invalid_params(req_id, writer).await?;
            }
        }

        Method::ProcessDelete => {
            if let Ok(NameParams { name }) = request.parse_params() {
                let targets = registry.resolve_targets(&name);
                if targets.is_empty() {
                    send_not_found(req_id, &name, writer).await?;
                } else {
                    for target_name in &targets {
                        if let Some(cancel) = registry.supervisor_cancel(target_name) {
                            cancel.cancel();
                        }
                        registry.remove(target_name);
                        let _ = storage.remove_saved_process(target_name);
                        let _ = storage.flush_logs(target_name);
                    }
                    let resp = Response::ok(
                        req_id,
                        ResponseResult::Ack {
                            message: if targets.len() == 1 {
                                format!("Process '{}' deleted", targets[0])
                            } else {
                                format!("{} processes deleted", targets.len())
                            },
                        },
                    );
                    writer.lock().await.send(&resp).await?;
                }
            } else {
                send_invalid_params(req_id, writer).await?;
            }
        }

        Method::ProcessList => {
            let mut processes = registry.list();
            // Sort by numeric ID so output is stable and ascending.
            processes.sort_by_key(|p| p.id);
            let resp = Response::ok(req_id, ResponseResult::ProcessList { processes });
            writer.lock().await.send(&resp).await?;
        }

        Method::ProcessInfo => {
            if let Ok(NameParams { name }) = request.parse_params() {
                if let Some(target_name) = registry.find_name_by_id_or_name(&name) {
                    if let Some(process) = registry.get(&target_name) {
                        let resp = Response::ok(req_id, ResponseResult::ProcessInfo { process });
                        writer.lock().await.send(&resp).await?;
                    } else {
                        send_not_found(req_id, &name, writer).await?;
                    }
                } else {
                    send_not_found(req_id, &name, writer).await?;
                }
            } else {
                send_invalid_params(req_id, writer).await?;
            }
        }

        Method::ProcessLogs => {
            if let Ok(LogsParams {
                name,
                lines,
                follow,
            }) = request.parse_params()
            {
                let target_name = if name == "all" || name.is_empty() {
                    "all".to_string()
                } else {
                    registry
                        .find_name_by_id_or_name(&name)
                        .unwrap_or_else(|| name.clone())
                };

                let historical_entries = if target_name == "all" {
                    let active_names: Vec<String> =
                        registry.list().into_iter().map(|p| p.name).collect();
                    if active_names.is_empty() {
                        storage
                            .get_logs_with_process("all", lines)
                            .unwrap_or_default()
                    } else {
                        storage
                            .get_logs_for_processes(&active_names, lines)
                            .unwrap_or_default()
                    }
                } else {
                    storage
                        .get_logs_with_process(&target_name, lines)
                        .unwrap_or_default()
                };

                for (proc_name, line) in historical_entries {
                    let resp = Response::ok(
                        req_id,
                        ResponseResult::LogLine {
                            name: proc_name,
                            line,
                        },
                    );
                    if writer.lock().await.send(&resp).await.is_err() {
                        return Ok(());
                    }
                }

                if !follow {
                    let resp = Response::ok(req_id, ResponseResult::StreamEnd);
                    let _ = writer.lock().await.send(&resp).await;
                } else {
                    // Subscribe to live broadcast
                    let mut rx = logger.subscribe();
                    while let Ok((proc_name, log_line)) = rx.recv().await {
                        if target_name == "all" || proc_name == target_name {
                            let resp = Response::ok(
                                req_id,
                                ResponseResult::LogLine {
                                    name: proc_name,
                                    line: log_line,
                                },
                            );
                            if writer.lock().await.send(&resp).await.is_err() {
                                break;
                            }
                        }
                    }
                }
            } else {
                send_invalid_params(req_id, writer).await?;
            }
        }

        Method::ProcessFlush => {
            if let Ok(NameParams { name }) = request.parse_params() {
                if name == "all" || name.is_empty() {
                    let total = storage.flush_all_logs().unwrap_or(0);
                    let resp = Response::ok(
                        req_id,
                        ResponseResult::Ack {
                            message: format!("Flushed {} log lines across all processes", total),
                        },
                    );
                    writer.lock().await.send(&resp).await?;
                } else {
                    let targets = registry.resolve_targets(&name);
                    if targets.is_empty() {
                        send_not_found(req_id, &name, writer).await?;
                    } else {
                        let mut total: u64 = 0;
                        for target_name in &targets {
                            total += storage.flush_logs(target_name).unwrap_or(0);
                        }
                        let resp = Response::ok(
                            req_id,
                            ResponseResult::Ack {
                                message: if targets.len() == 1 {
                                    format!("Flushed {} log lines for '{}'", total, targets[0])
                                } else {
                                    format!("Flushed {} log lines across {} processes", total, targets.len())
                                },
                            },
                        );
                        writer.lock().await.send(&resp).await?;
                    }
                }
            } else {
                send_invalid_params(req_id, writer).await?;
            }
        }

        Method::ProcessReset => {
            if let Ok(NameParams { name }) = request.parse_params() {
                let targets = registry.resolve_targets(&name);
                if targets.is_empty() {
                    send_not_found(req_id, &name, writer).await?;
                } else {
                    for target_name in &targets {
                        registry.reset_restart_count(target_name);
                    }
                    let resp = Response::ok(
                        req_id,
                        ResponseResult::Ack {
                            message: if targets.len() == 1 {
                                format!("Restart counter reset for '{}'", targets[0])
                            } else {
                                format!("Restart counters reset for {} processes", targets.len())
                            },
                        },
                    );
                    writer.lock().await.send(&resp).await?;
                }
            } else {
                send_invalid_params(req_id, writer).await?;
            }
        }

        Method::ProcessSendSignal => {
            if let Ok(SendSignalParams { target, signal }) = request.parse_params() {
                let targets = registry.resolve_targets(&target);
                if targets.is_empty() {
                    send_not_found(req_id, &target, writer).await?;
                } else {
                    let mut delivered = 0usize;
                    for name in targets {
                        if let Some(pid) = registry.get_pid(&name) {
                            if send_system_signal(pid, &signal) {
                                delivered += 1;
                                info!(name = %name, pid = pid, signal = %signal, "Delivered signal to process");
                            }
                        }
                    }
                    let resp = Response::ok(
                        req_id,
                        ResponseResult::Ack {
                            message: format!("Delivered signal {} to {} process(es)", signal, delivered),
                        },
                    );
                    writer.lock().await.send(&resp).await?;
                }
            } else {
                send_invalid_params(req_id, writer).await?;
            }
        }

        Method::ProcessScale => {
            if let Ok(ScaleParams { target, count }) = request.parse_params() {
                let matches = registry.resolve_targets(&target);
                if matches.is_empty() {
                    send_not_found(req_id, &target, writer).await?;
                } else {
                    let current_count = matches.len() as u32;
                    let target_count = if let Some(delta) = count.strip_prefix('+') {
                        let add: u32 = delta.parse().unwrap_or(1);
                        current_count + add
                    } else if let Some(delta) = count.strip_prefix('-') {
                        let sub: u32 = delta.parse().unwrap_or(1);
                        current_count.saturating_sub(sub).max(1)
                    } else {
                        count.parse::<u32>().unwrap_or(current_count).max(1)
                    };

                    let base_config = registry.get_config(&matches[0]);
                    if let Some(mut config) = base_config {
                        config.instances = target_count;

                        if target_count > current_count {
                            // Scale up: spawn additional instances
                            for i in current_count..target_count {
                                let instance_name = format!("{}-{}", target, i);
                                let cancel_token = if registry.contains(&instance_name) {
                                    if let Some(cancel) = registry.supervisor_cancel(&instance_name) {
                                        cancel.cancel();
                                    }
                                    registry.reset_cancel_token(&instance_name).unwrap_or_default()
                                } else {
                                    let _ = registry.register(instance_name.clone(), config.clone());
                                    registry.supervisor_cancel(&instance_name).unwrap_or_default()
                                };
                                crate::supervisor::spawn(
                                    instance_name,
                                    config.clone(),
                                    registry.clone(),
                                    logger.clone(),
                                    cancel_token,
                                );
                            }
                            let resp = Response::ok(
                                req_id,
                                ResponseResult::Ack {
                                    message: format!("Scaled '{}' from {} to {} instances", target, current_count, target_count),
                                },
                            );
                            writer.lock().await.send(&resp).await?;
                        } else if target_count < current_count {
                            // Scale down: stop and remove excess instances from highest index
                            let to_remove = (current_count - target_count) as usize;
                            for proc_name in matches.iter().rev().take(to_remove) {
                                if let Some(cancel) = registry.supervisor_cancel(proc_name) {
                                    cancel.cancel();
                                }
                                registry.remove(proc_name);
                                let _ = storage.remove_saved_process(proc_name);
                            }
                            let resp = Response::ok(
                                req_id,
                                ResponseResult::Ack {
                                    message: format!("Scaled '{}' from {} down to {} instances", target, current_count, target_count),
                                },
                            );
                            writer.lock().await.send(&resp).await?;
                        } else {
                            let resp = Response::ok(
                                req_id,
                                ResponseResult::Ack {
                                    message: format!("'{}' is already at {} instances", target, target_count),
                                },
                            );
                            writer.lock().await.send(&resp).await?;
                        }
                    } else {
                        send_not_found(req_id, &target, writer).await?;
                    }
                }
            } else {
                send_invalid_params(req_id, writer).await?;
            }
        }

        Method::DaemonStatus => {
            let uptime_secs = start_time.elapsed().as_secs();
            let process_count = registry.list().len() as u32;
            let resp = Response::ok(
                req_id,
                ResponseResult::DaemonStatus {
                    version: env!("CARGO_PKG_VERSION").to_string(),
                    uptime_secs,
                    process_count,
                },
            );
            writer.lock().await.send(&resp).await?;
        }

        Method::DaemonKill => {
            info!("Received daemon kill request");
            let resp = Response::ok(
                req_id,
                ResponseResult::Ack {
                    message: "orion daemon shutting down".to_string(),
                },
            );
            writer.lock().await.send(&resp).await?;
            daemon_shutdown.cancel();
        }

        // Save: snapshot every currently registered process into SQLite.
        Method::DaemonSave => {
            let processes = registry.list_with_handles();
            let mut saved = 0usize;
            for (name, config, restart_count, registered_at) in processes {
                match storage.save_process(&name, &config, restart_count, registered_at) {
                    Ok(()) => saved += 1,
                    Err(e) => {
                        tracing::warn!(name = %name, error = %e, "Failed to save process state")
                    }
                }
            }
            let resp = Response::ok(
                req_id,
                ResponseResult::Ack {
                    message: format!("Saved {} process(es) to disk", saved),
                },
            );
            writer.lock().await.send(&resp).await?;
        }

        // Restore: load all saved processes from SQLite and re-register + re-spawn them.
        Method::DaemonRestore => {
            let saved = storage.load_saved_processes().unwrap_or_default();
            let count = saved.len();
            for (name, config, _restart_count, _registered_at) in saved {
                if registry.contains(&name) {
                    // Already running — skip to avoid double-spawn.
                    continue;
                }
                let _ = registry.register(name.clone(), config.clone());
                if let Some(cancel_token) = registry.supervisor_cancel(&name) {
                    crate::supervisor::spawn(
                        name.clone(),
                        config,
                        registry.clone(),
                        logger.clone(),
                        cancel_token,
                    );
                    info!(name = %name, "Process restored from saved state");
                }
            }
            let resp = Response::ok(
                req_id,
                ResponseResult::Ack {
                    message: format!("Restored {} process(es) from disk", count),
                },
            );
            writer.lock().await.send(&resp).await?;
        }
    }

    Ok(())
}

async fn send_not_found(req_id: u64, name: &str, writer: Arc<Mutex<ResponseWriter>>) -> Result<()> {
    let resp = Response::err(
        req_id,
        error_codes::PROCESS_NOT_FOUND,
        format!("Process '{}' not found", name),
    );
    writer.lock().await.send(&resp).await
}

async fn send_invalid_params(req_id: u64, writer: Arc<Mutex<ResponseWriter>>) -> Result<()> {
    let resp = Response::err(req_id, -32602, "Invalid method parameters");
    writer.lock().await.send(&resp).await
}

fn send_system_signal(pid: u32, signal: &str) -> bool {
    let clean_sig = signal.trim().to_uppercase();
    let sig_name = clean_sig.strip_prefix("SIG").unwrap_or(&clean_sig);

    #[cfg(unix)]
    {
        let status = std::process::Command::new("kill")
            .arg(format!("-{}", sig_name))
            .arg(pid.to_string())
            .status();
        match status {
            Ok(s) => s.success(),
            Err(e) => {
                tracing::error!(pid = pid, error = %e, "Failed to execute kill command");
                false
            }
        }
    }

    #[cfg(windows)]
    {
        match sig_name {
            "KILL" | "9" => std::process::Command::new("taskkill")
                .args(["/F", "/PID", &pid.to_string()])
                .status()
                .map(|s| s.success())
                .unwrap_or(false),
            "TERM" | "15" | "INT" | "2" => std::process::Command::new("taskkill")
                .args(["/PID", &pid.to_string()])
                .status()
                .map(|s| s.success())
                .unwrap_or(false),
            _ => {
                tracing::warn!(pid = pid, signal = %signal, "POSIX signal not supported natively on Windows");
                false
            }
        }
    }
}

