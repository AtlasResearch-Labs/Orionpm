use anyhow::Result;
use clap::Args;
use orion_core::protocol::{EmptyParams, Method, Params, ResponseResult};

use super::client;
use super::table;

#[derive(Args, Default)]
pub struct ListArgs {
    /// Show full command path instead of truncating.
    #[arg(long)]
    pub full: bool,

    /// Output process list as JSON (compatible with pm2 jlist).
    #[arg(long, short = 'j')]
    pub json: bool,
}

pub async fn run(args: ListArgs) -> Result<()> {
    let result = client::rpc(Method::ProcessList, Params::ProcessList(EmptyParams {})).await?;

    if let ResponseResult::ProcessList { processes } = result {
        if args.json {
            println!("{}", serde_json::to_string_pretty(&processes)?);
        } else {
            table::print_process_table(&processes, args.full);
        }
    }

    Ok(())
}
