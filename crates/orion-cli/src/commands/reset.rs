use anyhow::Result;
use clap::Args;
use orion_core::protocol::{Method, NameParams, Params, ResponseResult};

use super::client;

#[derive(Args)]
pub struct ResetArgs {
    /// Name, numeric ID of the process, or "all" to reset all restart counters.
    #[arg(default_value = "all")]
    pub target: String,
}

pub async fn run(args: ResetArgs) -> Result<()> {
    client::ensure_daemon().await?;

    let result = client::rpc(
        Method::ProcessReset,
        Params::ProcessReset(NameParams { name: args.target }),
    )
    .await?;

    if let ResponseResult::Ack { message } = result {
        println!("{}", message);
    }

    Ok(())
}
