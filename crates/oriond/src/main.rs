mod daemon;
mod handlers;
mod logger;
mod metrics;
mod pruner;
mod registry;
mod storage;
mod supervisor;

use anyhow::Result;
use tracing::info;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize structured logging.
    // ORION_LOG env var controls log level (e.g. ORION_LOG=debug).
    // Default: info level, plain text format for terminal readability.
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_env("ORION_LOG").unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_target(false)
        .with_thread_ids(false)
        .compact()
        .init();

    info!(version = env!("CARGO_PKG_VERSION"), "oriond starting");

    // ── Fix Issue 3: Singleton OS-level master lock (flock) ───────────────────
    // Prevents two oriond instances from running simultaneously under the same
    // user (e.g. systemd service racing against a CLI-spawned daemon).
    // The lock is held for the lifetime of this process and released by the OS
    // on exit — even if we are killed with SIGKILL or Task Manager.
    let _lock_guard = {
        use fs2::FileExt;
        let data_dir = storage::Storage::data_dir();
        std::fs::create_dir_all(&data_dir)?;
        let lock_path = data_dir.join("orion.lock");
        let lock_file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)?;
        if lock_file.try_lock_exclusive().is_err() {
            eprintln!(
                "[oriond] Another Orion daemon is already running \
                 (could not acquire master lock at {}). Exiting.",
                lock_path.display()
            );
            std::process::exit(0);
        }
        // Keep the file handle alive — dropping it releases the lock.
        lock_file
    };

    daemon::run().await
}
