use anyhow::Result;
use clap::{Args, Subcommand};
use orion_core::protocol::{EmptyParams, Method, Params, ResponseResult};

use super::client;

#[derive(Args)]
pub struct DaemonArgs {
    #[command(subcommand)]
    pub command: DaemonCommand,
}

#[derive(Subcommand)]
pub enum DaemonCommand {
    /// Check if oriond is running.
    Status,
    /// Start oriond in the background.
    Start,
    /// Stop oriond and all managed processes.
    Kill,
}

pub async fn run(args: DaemonArgs) -> Result<()> {
    match args.command {
        DaemonCommand::Status => {
            let req = orion_core::protocol::Request::new(
                1,
                Method::DaemonStatus,
                Params::DaemonStatus(EmptyParams {}),
            );
            match orion_ipc::client::send(&req).await {
                Ok(resp) => {
                    if let orion_core::protocol::ResponseBody::Result(
                        ResponseResult::DaemonStatus {
                            version,
                            uptime_secs,
                            process_count,
                        },
                    ) = resp.body
                    {
                        println!("oriond status: online");
                        println!("  Version:       {}", version);
                        println!("  Uptime:        {}s", uptime_secs);
                        println!("  Processes:     {}", process_count);
                    }
                }
                Err(_) => {
                    println!("oriond status: offline");
                }
            }
        }
        DaemonCommand::Start => {
            client::ensure_daemon().await?;
        }
        DaemonCommand::Kill => {
            let ping = orion_core::protocol::Request::new(
                0,
                Method::DaemonStatus,
                Params::DaemonStatus(EmptyParams {}),
            );
            if orion_ipc::client::send(&ping).await.is_err() {
                println!("oriond is not running.");
                return Ok(());
            }
            let req = orion_core::protocol::Request::new(
                1,
                Method::DaemonKill,
                Params::DaemonKill(EmptyParams {}),
            );
            match orion_ipc::client::send(&req).await {
                Ok(resp) => {
                    if let orion_core::protocol::ResponseBody::Result(ResponseResult::Ack {
                        message,
                    }) = resp.body
                    {
                        println!("{}", message);
                    }
                }
                Err(_) => {
                    println!("oriond stopped.");
                }
            }
        }
    }
    Ok(())
}
