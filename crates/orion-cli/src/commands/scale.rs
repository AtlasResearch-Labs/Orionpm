use anyhow::{bail, Result};
use clap::Args;
use orion_core::protocol::{Method, Params, ResponseResult, ScaleParams};
use super::client;

use crate::commands::list::{self, ListArgs};

#[derive(Args)]
pub struct ScaleArgs {
    /// Name of the application/process to scale.
    pub target: String,

    /// Target instance count (e.g. 4) or delta (e.g. +2, -1).
    pub count: String,
}

pub async fn run(args: ScaleArgs) -> Result<()> {
    let params = ScaleParams {
        target: args.target,
        count: args.count,
    };

    let res = client::rpc(Method::ProcessScale, Params::ProcessScale(params)).await?;

    match res {
        ResponseResult::Ack { message } => {
            println!("{}", message);
            // Show updated process list table
            tokio::time::sleep(std::time::Duration::from_millis(150)).await;
            list::run(ListArgs::default()).await?;
            Ok(())
        }
        _ => bail!("Unexpected response from daemon"),
    }
}
