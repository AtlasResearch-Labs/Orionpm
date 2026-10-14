use anyhow::Result;
use clap::Args;
use std::time::{SystemTime, UNIX_EPOCH};
use orion_core::protocol::{Method, NameParams, Params, ResponseResult};

use super::client;

#[derive(Args)]
pub struct InfoArgs {
    /// Name or numeric ID of the process.
    pub target: String,
}

pub async fn run(args: InfoArgs) -> Result<()> {
    let result = client::rpc(
        Method::ProcessInfo,
        Params::ProcessInfo(NameParams {
            name: args.target.clone(),
        }),
    )
    .await?;

    if let ResponseResult::ProcessInfo { process } = result {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let uptime = match (process.started_at, process.state.is_active()) {
            (Some(s), true) if now >= s => format!("{}s", now - s),
            _ => "-".to_string(),
        };

        let cpu = match process.cpu_pct {
            Some(c) => format!("{:.1}%", c),
            None => "-".to_string(),
        };

        println!("Process Details: {}", process.name);
        println!("  ID:            {}", process.id);
        println!("  Status:        {}", process.state.label());
        println!("  Mode:          {}", process.mode);
        println!("  Command:       {}", process.command);
        println!("  Restarts:      {}", process.restart_count);
        println!("  Uptime:        {}", uptime);
        println!("  Memory:        {}", process.memory_display());
        println!("  CPU:           {}", cpu);
    }

    Ok(())
}
