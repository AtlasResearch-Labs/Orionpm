use anyhow::Result;
use tokio::signal;
use tokio_util::sync::CancellationToken;
use tracing::info;

use crate::logger::LogBus;
use crate::registry::ProcessRegistry;
use crate::storage::Storage;

/// Start the daemon: open storage, set up the process registry,
/// launch background tasks and IPC server, and wait for a shutdown signal.
pub async fn run() -> Result<()> {
    let storage = Storage::open()?;
    let logger = LogBus::new(storage.clone());
    let registry = ProcessRegistry::new();
    let shutdown = CancellationToken::new();

    // ── Spawn the system metrics collector task ───────────────────────────────
    let metrics_registry = registry.clone();
    let metrics_shutdown = shutdown.clone();
    let metrics_logger = logger.clone();
    tokio::spawn(async move {
        crate::metrics::run(metrics_registry, metrics_logger, metrics_shutdown).await;
    });

    // ── Spawn the log pruner task (runs every 5 minutes) ─────────────────────
    let pruner_storage = storage.clone();
    let pruner_shutdown = shutdown.clone();
    tokio::spawn(async move {
        crate::pruner::run(pruner_storage, pruner_shutdown).await;
    });

    // ── Fix Issue 1 & 2: Bind IPC socket FIRST, before restoring any processes.
    // If the socket cannot be bound (e.g. another live daemon holds it), abort
    // immediately rather than becoming a headless ghost daemon that aggressively
    // re-spawns processes it cannot control.
    let ipc_shutdown = shutdown.clone();
    let ipc_registry = registry.clone();
    let ipc_storage = storage.clone();
    let ipc_logger = logger.clone();
    let ipc_ready = tokio::sync::oneshot::channel::<std::result::Result<(), String>>();
    let (ipc_ready_tx, ipc_ready_rx) = ipc_ready;

    tokio::spawn(async move {
        if let Err(e) = crate::handlers::serve(
            ipc_registry,
            ipc_storage,
            ipc_logger,
            ipc_shutdown.clone(),
            ipc_ready_tx,
        )
        .await
        {
            // IPC server failed to start or crashed — cancel the shutdown token
            // so the daemon exits cleanly instead of becoming a headless ghost.
            tracing::error!(error = %e, "Fatal: IPC server failed. Triggering daemon abort.");
            ipc_shutdown.cancel();
        }
    });

    // Wait for the IPC server to confirm it is listening (or fail) before
    // spawning any managed processes.  This is the key ordering fix.
    match ipc_ready_rx.await {
        Ok(Ok(())) => {
            info!("IPC server confirmed ready; restoring saved processes");
        }
        Ok(Err(e)) => {
            tracing::error!("IPC server failed to bind: {}. Aborting daemon.", e);
            std::process::exit(1);
        }
        Err(_) => {
            tracing::error!("IPC server task dropped before signalling ready. Aborting daemon.");
            std::process::exit(1);
        }
    }

    // ── Stable IDs: seed next_id from persisted counter before registering ────
    // This ensures numeric process IDs are stable across daemon restarts —
    // a process that was ID 3 before a daemon restart will still be ID 3 after.
    let persisted_next_id = storage.load_next_id();
    if persisted_next_id > 0 {
        registry.restore_next_id(persisted_next_id);
        info!(
            next_id = persisted_next_id,
            "Restored stable process ID counter"
        );
    }

    // ── Fix Issue 2: Restore saved processes ONLY after IPC is bound ──────────
    let saved_processes = storage.load_saved_processes().unwrap_or_default();
    if saved_processes.is_empty() {
        info!("No saved process state found — starting clean");
    } else {
        info!(
            count = saved_processes.len(),
            "Restoring saved processes from disk"
        );
        for (name, config, _restart_count, _registered_at) in saved_processes {
            let _ = registry.register(name.clone(), config.clone());
            if let Some(cancel_token) = registry.supervisor_cancel(&name) {
                crate::supervisor::spawn(
                    name.clone(),
                    config,
                    registry.clone(),
                    logger.clone(),
                    cancel_token,
                );
                info!(name = %name, "Process restored");
            }
        }
        // Persist the updated next_id so future restarts continue from the correct slot.
        let _ = storage.save_next_id(registry.current_next_id());
    }

    info!("oriond ready");

    // ── Phase 3: Systemd Type=notify support ──────────────────────────────────
    // Notify systemd that initialization is complete and daemon is fully ready.
    #[cfg(target_os = "linux")]
    notify_systemd("READY=1");

    // Block until SIGTERM or SIGINT or daemon kill request.
    tokio::select! {
        _ = shutdown_signal() => {
            info!("OS shutdown signal received.");
        }
        _ = shutdown.cancelled() => {
            info!("Daemon shutdown cancellation triggered.");
        }
    }

    #[cfg(target_os = "linux")]
    notify_systemd("STOPPING=1");

    info!("Stopping all managed processes.");
    for proc_info in registry.list() {
        if let Some(cancel) = registry.supervisor_cancel(&proc_info.name) {
            cancel.cancel();
        }
    }
    shutdown.cancel();

    // Give supervised processes a grace window before the process exits.
    tokio::time::sleep(tokio::time::Duration::from_millis(800)).await;

    info!("oriond exiting");
    Ok(())
}

/// Waits for Ctrl+C (SIGINT) or SIGTERM (Unix only).
async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};

        let mut sigterm =
            signal(SignalKind::terminate()).expect("Failed to install SIGTERM handler");

        tokio::select! {
            _ = signal::ctrl_c() => {}
            _ = sigterm.recv() => {}
        }
    }

    #[cfg(not(unix))]
    {
        match signal::ctrl_c().await {
            Ok(()) => {}
            Err(_) => {
                // Detached process without attached console.
                // Wait indefinitely; shutdown will occur via IPC DaemonKill.
                std::future::pending::<()>().await;
            }
        }
    }
}

/// Sends readiness or stopping states to systemd via $NOTIFY_SOCKET datagram socket.
#[cfg(target_os = "linux")]
fn notify_systemd(state: &str) {
    if let Ok(socket_path) = std::env::var("NOTIFY_SOCKET") {
        use std::os::unix::net::UnixDatagram;
        if let Ok(socket) = UnixDatagram::unbound() {
            let msg = format!("{}\n", state);
            let _ = socket.send_to(msg.as_bytes(), socket_path);
        }
    }
}
