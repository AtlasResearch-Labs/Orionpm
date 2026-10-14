use anyhow::Result;
use clap::Args;
use orion_core::protocol::{Method, NameParams, Params, ResponseResult};

use super::client;
use super::list::{self, ListArgs};

#[derive(Args)]
pub struct StopArgs {
    /// Name, numeric ID, or "all" to stop every managed process.
    pub target: String,
}

pub async fn run(args: StopArgs) -> Result<()> {
    let result = client::rpc(
        Method::ProcessStop,
        Params::ProcessStop(NameParams {
            name: args.target.clone(),
        }),
    )
    .await?;

    if let ResponseResult::Ack { message } = result {
        println!("{}", message);
    }

    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    list::run(ListArgs { full: false }).await?;
    Ok(())
}
