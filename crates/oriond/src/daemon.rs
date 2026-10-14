use anyhow::Result;
use tokio::signal;
use tokio_util::sync::CancellationToken;
use tracing::info;

use crate::logger::LogBus;
use crate::registry::ProcessRegistry;
use crate::storage::Storage;

/// Start the daemon: open storage, set up the process registry,
/// launch the metrics collector and IPC server, and wait for a shutdown signal.
pub async fn run() -> Result<()> {
    let storage = Storage::open()?;
    let logger = LogBus::new(storage.clone());
    let registry = ProcessRegistry::new();
    let shutdown = CancellationToken::new();

    // Spawn the system metrics collector task.
    let metrics_registry = registry.clone();
    let metrics_shutdown = shutdown.clone();
    let metrics_logger = logger.clone();
    tokio::spawn(async move {
        crate::metrics::run(metrics_registry, metrics_logger, metrics_shutdown).await;
    });

    // Spawn the IPC server task.
    let ipc_shutdown = shutdown.clone();
    let ipc_registry = registry.clone();
    let ipc_storage = storage.clone();
    let ipc_logger = logger.clone();
    tokio::spawn(async move {
        if let Err(e) = crate::handlers::serve(ipc_registry, ipc_storage, ipc_logger, ipc_shutdown).await {
            tracing::error!(error = %e, "IPC server exited with error");
        }
    });

    info!("oriond ready");

    // Block until SIGTERM or SIGINT or daemon kill request.
    tokio::select! {
        _ = shutdown_signal() => {
            info!("OS shutdown signal received.");
        }
        _ = shutdown.cancelled() => {
            info!("Daemon shutdown cancellation triggered.");
        }
    }

    info!("Stopping all managed processes.");
    shutdown.cancel();

    // Give supervised processes a grace window before the process exits.
    // The supervisor tasks will handle sending SIGTERM to children.
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

    info!("oriond exiting");
    Ok(())
}

/// Waits for Ctrl+C (SIGINT) or SIGTERM (Unix only).
async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};

        let mut sigterm = signal(SignalKind::terminate())
            .expect("Failed to install SIGTERM handler");

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
