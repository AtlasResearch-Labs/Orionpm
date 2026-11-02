use anyhow::Result;
use orion_core::config::AppConfig;
use orion_core::process_state::ProcessState;
use orion_core::protocol::LogStream;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;
use tracing::info;

use crate::logger::LogBus;
use crate::registry::ProcessRegistry;
use crate::storage::Storage;
use crate::supervisor;

/// Options for configuring the foreground container runtime.
#[derive(Debug, Clone, Default)]
pub struct RuntimeOptions {
    /// Emit raw stdout/stderr without timestamp or process prefix.
    pub raw: bool,
    /// Emit JSON-formatted lines for cloud container logging.
    pub json: bool,
    /// Optional HTTP port to expose Kubernetes `/healthz` probe.
    pub health_port: Option<u16>,
}

/// Runs the supervised applications directly in the foreground as PID 1.
///
/// This mode never detaches or forks a background daemon. It handles
/// Docker and Kubernetes container signals (SIGTERM, SIGINT), streams
/// logs directly to standard output, and guarantees clean termination.
pub async fn run_foreground(apps: Vec<(String, AppConfig)>, options: RuntimeOptions) -> Result<()> {
    if apps.is_empty() {
        eprintln!("[orion-runtime] No applications specified to run.");
        return Ok(());
    }

    info!(
        count = apps.len(),
        "orion-runtime starting in foreground (PID 1)"
    );

    let storage = Storage::open_in_memory()?;
    let logger = LogBus::new(storage);
    let registry = ProcessRegistry::new();
    let shutdown_token = CancellationToken::new();

    // ── 1. Log Forwarder: Stream child output directly to container stdout ────
    let mut log_rx = logger.subscribe();
    let is_raw = options.raw;
    let is_json = options.json;
    let log_shutdown = shutdown_token.clone();

    tokio::spawn(async move {
        loop {
            tokio::select! {
                biased;
                _ = log_shutdown.cancelled() => break,
                res = log_rx.recv() => {
                    match res {
                        Ok((name, line)) => {
                            if is_raw {
                                if line.stream == LogStream::Stderr {
                                    eprintln!("{}", line.text);
                                } else {
                                    println!("{}", line.text);
                                }
                            } else if is_json {
                                let json_obj = serde_json::json!({
                                    "timestamp": line.timestamp,
                                    "process": name,
                                    "stream": if line.stream == LogStream::Stderr { "stderr" } else { "stdout" },
                                    "message": line.text,
                                });
                                if let Ok(serialized) = serde_json::to_string(&json_obj) {
                                    println!("{}", serialized);
                                }
                            } else {
                                let time_str = chrono::DateTime::from_timestamp(line.timestamp as i64, 0)
                                    .map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
                                    .unwrap_or_else(|| line.timestamp.to_string());
                                let stream_tag = if line.stream == LogStream::Stderr { "ERR" } else { "OUT" };
                                println!("[{}] [{}] [{}] {}", time_str, name, stream_tag, line.text);
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                            eprintln!("[orion-runtime] Warning: Output stream buffer lagged ({} lines dropped)", skipped);
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
            }
        }
    });

    // ── 2. Kubernetes /healthz HTTP Server (Optional) ─────────────────────────
    if let Some(health_port) = options.health_port {
        let health_registry = registry.clone();
        let health_shutdown = shutdown_token.clone();
        tokio::spawn(async move {
            let addr = format!("0.0.0.0:{}", health_port);
            if let Ok(listener) = TcpListener::bind(&addr).await {
                info!(addr = %addr, "Kubernetes health probe listening on /healthz");
                loop {
                    tokio::select! {
                        _ = health_shutdown.cancelled() => break,
                        accept_res = listener.accept() => {
                            if let Ok((mut socket, _)) = accept_res {
                                let mut buf = [0u8; 1024];
                                if let Ok(n) = socket.read(&mut buf).await {
                                    if n > 0 {
                                        let req_str = String::from_utf8_lossy(&buf[..n]);
                                        if req_str.contains("GET /healthz") || req_str.contains("GET / ") {
                                            let procs = health_registry.list();
                                            let has_error = procs.iter().any(|p| matches!(p.state, ProcessState::Errored { .. }));
                                            let (status_line, body) = if has_error {
                                                ("HTTP/1.1 503 Service Unavailable\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n", "{\"status\":\"degraded\"}\n")
                                            } else {
                                                ("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n", "{\"status\":\"healthy\"}\n")
                                            };
                                            let _ = socket.write_all(status_line.as_bytes()).await;
                                            let _ = socket.write_all(body.as_bytes()).await;
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        });
    }

    // ── 3. Universal Cluster Balancer (if multi-instance with ports) ─────────
    let cluster_registry = crate::cluster::ClusterRegistry::new();
    let mut cluster_ports_started = std::collections::HashSet::new();

    for (name, config) in &apps {
        if let Some(port) = config.port {
            let base_name = name.split('-').next().unwrap_or(name);
            if !cluster_ports_started.contains(base_name) {
                cluster_ports_started.insert(base_name.to_string());
                let backend_ports: Vec<u16> = apps
                    .iter()
                    .filter(|(n, _)| n.starts_with(base_name))
                    .filter_map(|(_, c)| c.env.get("PORT").and_then(|p| p.parse().ok()))
                    .collect();

                if backend_ports.len() > 1 {
                    cluster_registry
                        .register_cluster(base_name, port, backend_ports)
                        .await;
                }
            }
        }
    }

    // ── 4. Spawn Supervised Applications ─────────────────────────────────────
    let mut app_cancel_tokens = Vec::new();
    let mut max_kill_timeout = 5u64;

    for (name, config) in apps {
        if config.kill_timeout_secs > max_kill_timeout {
            max_kill_timeout = config.kill_timeout_secs;
        }

        let _ = registry.register(name.clone(), config.clone());
        let cancel_token = registry.supervisor_cancel(&name).unwrap_or_default();
        app_cancel_tokens.push(cancel_token.clone());

        supervisor::spawn(name, config, registry.clone(), logger.clone(), cancel_token);
    }

    // ── 4. Intercept Container Signals (PID 1) ────────────────────────────────
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut sigterm = signal(SignalKind::terminate())?;
        let mut sigint = signal(SignalKind::interrupt())?;

        tokio::select! {
            _ = sigterm.recv() => {
                println!("[orion-runtime] Caught SIGTERM from container host. Initiating graceful shutdown...");
            }
            _ = sigint.recv() => {
                println!("[orion-runtime] Caught SIGINT (Ctrl+C). Initiating graceful shutdown...");
            }
        }
    }

    #[cfg(windows)]
    {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                println!("[orion-runtime] Caught Ctrl+C. Initiating graceful shutdown...");
            }
        }
    }

    // ── 5. Graceful Teardown of Supervised Applications ───────────────────────
    shutdown_token.cancel();
    for token in app_cancel_tokens {
        token.cancel();
    }

    let wait_deadline = tokio::time::Instant::now() + Duration::from_secs(max_kill_timeout + 1);
    while tokio::time::Instant::now() < wait_deadline {
        let any_alive = registry.list().iter().any(|p| p.state.is_alive());

        if !any_alive {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    println!("[orion-runtime] All processes stopped cleanly. Exiting.");
    Ok(())
}
