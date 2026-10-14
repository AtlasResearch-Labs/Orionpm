use anyhow::Result;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use tracing::info;
use orion_core::protocol::error_codes;
use orion_core::protocol::{
    LogsParams, Method, NameParams, Request, Response, ResponseResult, StartParams,
};
use orion_ipc::server::ResponseWriter;

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
                if name == "all" {
                    for proc in registry.list() {
                        if let Some(cancel) = registry.supervisor_cancel(&proc.name) {
                            cancel.cancel();
                        }
                    }
                    let resp = Response::ok(
                        req_id,
                        ResponseResult::Ack {
                            message: "All processes stopping".to_string(),
                        },
                    );
                    writer.lock().await.send(&resp).await?;
                } else if let Some(target_name) = registry.find_name_by_id_or_name(&name) {
                    if let Some(cancel) = registry.supervisor_cancel(&target_name) {
                        cancel.cancel();
                    }
                    let resp = Response::ok(
                        req_id,
                        ResponseResult::Ack {
                            message: format!("Process '{}' stopping", target_name),
                        },
                    );
                    writer.lock().await.send(&resp).await?;
                } else {
                    send_not_found(req_id, &name, writer).await?;
                }
            } else {
                send_invalid_params(req_id, writer).await?;
            }
        }

        Method::ProcessRestart => {
            if let Ok(NameParams { name }) = request.parse_params() {
                if name == "all" {
                    for proc in registry.list() {
                        if let Some(config) = registry.get_config(&proc.name) {
                            if let Some(cancel) = registry.supervisor_cancel(&proc.name) {
                                cancel.cancel();
                            }
                            if let Some(new_token) = registry.reset_cancel_token(&proc.name) {
                                crate::supervisor::spawn(
                                    proc.name.clone(),
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
                            message: "All processes restarted".to_string(),
                        },
                    );
                    writer.lock().await.send(&resp).await?;
                } else if let Some(target_name) = registry.find_name_by_id_or_name(&name) {
                    if let Some(config) = registry.get_config(&target_name) {
                        if let Some(cancel) = registry.supervisor_cancel(&target_name) {
                            cancel.cancel();
                        }
                        // Brief pause to allow child process to exit before re-spawning.
                        tokio::time::sleep(tokio::time::Duration::from_millis(300)).await;
                        if let Some(new_token) = registry.reset_cancel_token(&target_name) {
                            crate::supervisor::spawn(
                                target_name.clone(),
                                config,
                                registry.clone(),
                                logger.clone(),
                                new_token,
                            );
                        }
                        let resp = Response::ok(
                            req_id,
                            ResponseResult::Ack {
                                message: format!("Process '{}' restarted", target_name),
                            },
                        );
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

        // Zero-downtime rolling reload:
        //   1. Spawn a fresh supervisor with a new cancellation token.
        //   2. Wait up to 2 s for the new process to reach Running state.
        //   3. Cancel the old supervisor (graceful stop of the old process).
        // For fork-mode this ensures the new process is up before the old one dies.
        // For cluster-mode with instances > 1, each slot is rotated sequentially.
        Method::ProcessReload => {
            if let Ok(NameParams { name }) = request.parse_params() {
                let target_name = registry.find_name_by_id_or_name(&name);
                let config = target_name.as_deref().and_then(|n| registry.get_config(n));

                match (target_name, config) {
                    (Some(target_name), Some(config)) => {
                        let old_cancel = registry.supervisor_cancel(&target_name);

                        // Spawn the new supervisor with a fresh token.
                        if let Some(new_token) = registry.reset_cancel_token(&target_name) {
                            crate::supervisor::spawn(
                                target_name.clone(),
                                config,
                                registry.clone(),
                                logger.clone(),
                                new_token,
                            );
                        }

                        // Give the new process time to start (max 2 s).
                        let mut elapsed = 0u64;
                        loop {
                            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                            elapsed += 100;
                            let is_running = registry
                                .get(&target_name)
                                .map(|p| p.state.is_active())
                                .unwrap_or(false);
                            if is_running || elapsed >= 2000 {
                                break;
                            }
                        }

                        // Now gracefully stop the old supervisor.
                        if let Some(cancel) = old_cancel {
                            cancel.cancel();
                        }

                        let resp = Response::ok(
                            req_id,
                            ResponseResult::Ack {
                                message: format!("Process '{}' reloaded (zero-downtime)", target_name),
                            },
                        );
                        writer.lock().await.send(&resp).await?;
                    }
                    _ => {
                        send_not_found(req_id, &name, writer).await?;
                    }
                }
            } else {
                send_invalid_params(req_id, writer).await?;
            }
        }

        Method::ProcessDelete => {
            if let Ok(NameParams { name }) = request.parse_params() {
                if name == "all" {
                    for proc in registry.list() {
                        if let Some(cancel) = registry.supervisor_cancel(&proc.name) {
                            cancel.cancel();
                        }
                        registry.remove(&proc.name);
                        // Remove from saved state too.
                        let _ = storage.remove_saved_process(&proc.name);
                    }
                    let resp = Response::ok(
                        req_id,
                        ResponseResult::Ack {
                            message: "All processes deleted".to_string(),
                        },
                    );
                    writer.lock().await.send(&resp).await?;
                } else if let Some(target_name) = registry.find_name_by_id_or_name(&name) {
                    if let Some(cancel) = registry.supervisor_cancel(&target_name) {
                        cancel.cancel();
                    }
                    registry.remove(&target_name);
                    let _ = storage.remove_saved_process(&target_name);
                    let resp = Response::ok(
                        req_id,
                        ResponseResult::Ack {
                            message: format!("Process '{}' deleted", target_name),
                        },
                    );
                    writer.lock().await.send(&resp).await?;
                } else {
                    send_not_found(req_id, &name, writer).await?;
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
            if let Ok(LogsParams { name, lines, follow }) = request.parse_params() {
                let target_name = registry
                    .find_name_by_id_or_name(&name)
                    .unwrap_or_else(|| name.clone());

                let historical_lines = storage
                    .get_logs(&target_name, lines)
                    .unwrap_or_default();

                if !follow {
                    let resp = Response::ok(
                        req_id,
                        ResponseResult::Logs {
                            name: target_name,
                            lines: historical_lines,
                        },
                    );
                    writer.lock().await.send(&resp).await?;
                } else {
                    // Send historical lines first
                    for line in historical_lines {
                        let resp = Response::ok(
                            req_id,
                            ResponseResult::LogLine {
                                name: target_name.clone(),
                                line,
                            },
                        );
                        if writer.lock().await.send(&resp).await.is_err() {
                            return Ok(());
                        }
                    }

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
                if name == "all" {
                    // Flush logs for all known processes.
                    let processes = registry.list();
                    let mut total: u64 = 0;
                    for p in &processes {
                        total += storage.flush_logs(&p.name).unwrap_or(0);
                    }
                    let resp = Response::ok(
                        req_id,
                        ResponseResult::Ack {
                            message: format!("Flushed {} log lines across all processes", total),
                        },
                    );
                    writer.lock().await.send(&resp).await?;
                } else if let Some(target_name) = registry.find_name_by_id_or_name(&name) {
                    let deleted = storage.flush_logs(&target_name).unwrap_or(0);
                    let resp = Response::ok(
                        req_id,
                        ResponseResult::Ack {
                            message: format!("Flushed {} log lines for '{}'", deleted, target_name),
                        },
                    );
                    writer.lock().await.send(&resp).await?;
                } else {
                    send_not_found(req_id, &name, writer).await?;
                }
            } else {
                send_invalid_params(req_id, writer).await?;
            }
        }

        Method::ProcessReset => {
            if let Ok(NameParams { name }) = request.parse_params() {
                if name == "all" {
                    for proc in registry.list() {
                        registry.reset_restart_count(&proc.name);
                    }
                    let resp = Response::ok(
                        req_id,
                        ResponseResult::Ack {
                            message: "Restart counters reset for all processes".to_string(),
                        },
                    );
                    writer.lock().await.send(&resp).await?;
                } else if let Some(target_name) = registry.find_name_by_id_or_name(&name) {
                    registry.reset_restart_count(&target_name);
                    let resp = Response::ok(
                        req_id,
                        ResponseResult::Ack {
                            message: format!("Restart counter reset for '{}'", target_name),
                        },
                    );
                    writer.lock().await.send(&resp).await?;
                } else {
                    send_not_found(req_id, &name, writer).await?;
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
                    Err(e) => tracing::warn!(name = %name, error = %e, "Failed to save process state"),
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

async fn send_not_found(
    req_id: u64,
    name: &str,
    writer: Arc<Mutex<ResponseWriter>>,
) -> Result<()> {
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
