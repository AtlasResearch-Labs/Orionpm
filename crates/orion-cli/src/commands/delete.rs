use anyhow::Result;
use clap::Args;
use orion_core::protocol::{Method, NameParams, Params, ResponseResult};

use super::client;
use super::list::{self, ListArgs};

#[derive(Args)]
pub struct DeleteArgs {
    /// Name, numeric ID, or "all".
    pub target: String,
}

pub async fn run(args: DeleteArgs) -> Result<()> {
    let result = client::rpc(
        Method::ProcessDelete,
        Params::ProcessDelete(NameParams {
            name: args.target.clone(),
        }),
    )
    .await?;

    if let ResponseResult::Ack { message } = result {
        println!("{}", message);
    }

    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    list::run(ListArgs::default()).await?;
    Ok(())
}
