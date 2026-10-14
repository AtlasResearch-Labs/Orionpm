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

    daemon::run().await
}
