use std::time::Duration;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info};

use crate::storage::Storage;

/// Background task that periodically prunes old log lines from SQLite.
/// Runs every 5 minutes and keeps at most 10,000 lines per process.
/// This prevents the orion.db from growing unbounded on long-running systems.
pub async fn run(storage: Storage, shutdown: CancellationToken) {
    const PRUNE_INTERVAL: Duration = Duration::from_secs(300); // 5 minutes
    const KEEP_LINES_PER_PROCESS: usize = 10_000;

    info!("Log pruner started (interval=5min, keep={} lines/process)", KEEP_LINES_PER_PROCESS);

    loop {
        tokio::select! {
            _ = tokio::time::sleep(PRUNE_INTERVAL) => {
                let storage_clone = storage.clone();
                // Run the blocking SQLite operation on a dedicated thread
                // so we don't stall the async executor.
                let result = tokio::task::spawn_blocking(move || {
                    storage_clone.prune_logs(KEEP_LINES_PER_PROCESS)
                })
                .await;

                match result {
                    Ok(Ok(deleted)) if deleted > 0 => {
                        info!(deleted = deleted, "Log pruner removed old log lines");
                    }
                    Ok(Ok(_)) => {
                        debug!("Log pruner ran — no lines needed removal");
                    }
                    Ok(Err(e)) => {
                        tracing::warn!(error = %e, "Log pruner encountered a storage error");
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "Log pruner task panicked");
                    }
                }
            }
            _ = shutdown.cancelled() => {
                info!("Log pruner shutting down");
                break;
            }
        }
    }
}
