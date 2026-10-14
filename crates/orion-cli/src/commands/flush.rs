use anyhow::Result;
use clap::Args;
use orion_core::protocol::{Method, NameParams, Params, ResponseResult};

use super::client;

#[derive(Args)]
pub struct FlushArgs {
    /// Name, numeric ID of the process, or "all" to flush logs of all processes.
    pub target: String,
}

pub async fn run(args: FlushArgs) -> Result<()> {
    client::ensure_daemon().await?;

    let result = client::rpc(
        Method::ProcessFlush,
        Params::ProcessFlush(NameParams { name: args.target }),
    )
    .await?;

    if let ResponseResult::Ack { message } = result {
        println!("{}", message);
    }

    Ok(())
}
