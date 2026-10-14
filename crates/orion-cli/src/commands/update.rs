use anyhow::{bail, Context, Result};
use orion_core::protocol::{EmptyParams, Method, Params, ResponseResult};
use std::time::Duration;
use tokio::time::sleep;

use super::client;
use crate::commands::daemon;
use crate::commands::list::{self, ListArgs};

pub async fn run() -> Result<()> {
    println!("Initiating in-place daemon upgrade...");

    // 1. Snapshot all active processes to SQLite persistence.
    println!("Saving active process state to disk...");
    match client::rpc(Method::DaemonSave, Params::DaemonSave(EmptyParams {})).await {
        Ok(ResponseResult::Ack { message }) => println!("{}", message),
        Ok(_) => bail!("Unexpected response saving process state"),
        Err(e) => eprintln!("Warning saving state: {}", e),
    }

    // 2. Instruct existing daemon to shut down cleanly.
    println!("Shutting down existing daemon...");
    let _ = client::rpc(Method::DaemonKill, Params::DaemonKill(EmptyParams {})).await;
    sleep(Duration::from_millis(600)).await;

    // 3. Restart daemon.
    println!("Starting updated daemon...");
    daemon::run(daemon::DaemonArgs {
        command: daemon::DaemonCommand::Start,
    })
    .await
    .context("Failed to start updated daemon")?;

    sleep(Duration::from_millis(400)).await;

    // 4. Restore processes.
    println!("Restoring managed processes...");
    match client::rpc(Method::DaemonRestore, Params::DaemonRestore(EmptyParams {})).await {
        Ok(ResponseResult::Ack { message }) => println!("{}", message),
        Ok(_) => bail!("Unexpected response restoring processes"),
        Err(e) => eprintln!("Warning restoring processes: {}", e),
    }

    sleep(Duration::from_millis(200)).await;
    println!("\nOrion daemon successfully updated and all processes restored.");
    list::run(ListArgs::default()).await?;

    Ok(())
}
