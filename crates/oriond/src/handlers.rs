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
                        // Brief pause to allow graceful stop
                        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
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

        Method::ProcessReload => {
            if let Ok(NameParams { name }) = request.parse_params() {
                if let Some(target_name) = registry.find_name_by_id_or_name(&name) {
                    if let Some(config) = registry.get_config(&target_name) {
                        if let Some(cancel) = registry.supervisor_cancel(&target_name) {
                            cancel.cancel();
                        }
                        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
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
                                message: format!("Process '{}' reloaded", target_name),
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

        Method::ProcessDelete => {
            if let Ok(NameParams { name }) = request.parse_params() {
                if name == "all" {
                    for proc in registry.list() {
                        if let Some(cancel) = registry.supervisor_cancel(&proc.name) {
                            cancel.cancel();
                        }
                        registry.remove(&proc.name);
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
            let processes = registry.list();
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

        Method::DaemonSave => {
            let resp = Response::ok(
                req_id,
                ResponseResult::Ack {
                    message: "Process list saved".to_string(),
                },
            );
            writer.lock().await.send(&resp).await?;
        }

        Method::DaemonRestore => {
            let resp = Response::ok(
                req_id,
                ResponseResult::Ack {
                    message: "Process list restored".to_string(),
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
