use anyhow::Result;
use orion_core::protocol::{EmptyParams, Method, Params, ResponseResult};
use std::time::Instant;

use super::client;

pub async fn run() -> Result<()> {
    client::ensure_daemon().await?;

    let start = Instant::now();
    let result = client::rpc(Method::DaemonStatus, Params::DaemonStatus(EmptyParams {})).await?;
    let elapsed = start.elapsed();

    if let ResponseResult::DaemonStatus {
        version,
        uptime_secs,
        process_count,
    } = result
    {
        let uptime_str = if uptime_secs < 60 {
            format!("{}s", uptime_secs)
        } else if uptime_secs < 3600 {
            format!("{}m {}s", uptime_secs / 60, uptime_secs % 60)
        } else {
            format!("{}h {}m", uptime_secs / 3600, (uptime_secs % 3600) / 60)
        };

        println!(
            "pong from oriond v{} (latency: {:.2?}, uptime: {}, processes: {})",
            version, elapsed, uptime_str, process_count
        );
    }

    Ok(())
}
