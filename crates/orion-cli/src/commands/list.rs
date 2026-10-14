use anyhow::Result;
use clap::Args;
use orion_core::protocol::{EmptyParams, Method, Params, ResponseResult};

use super::client;
use super::table;

#[derive(Args)]
pub struct ListArgs {
    /// Show full command path instead of truncating.
    #[arg(long)]
    pub full: bool,
}

pub async fn run(args: ListArgs) -> Result<()> {
    let result = client::rpc(Method::ProcessList, Params::ProcessList(EmptyParams {})).await?;

    if let ResponseResult::ProcessList { processes } = result {
        table::print_process_table(&processes, args.full);
    }

    Ok(())
}
