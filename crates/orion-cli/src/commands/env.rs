use anyhow::Result;
use clap::Args;
use orion_core::protocol::{Method, NameParams, Params, ResponseResult};

use super::client;

#[derive(Args)]
pub struct EnvArgs {
    /// Name or numeric ID of the process.
    pub target: String,
}

pub async fn run(args: EnvArgs) -> Result<()> {
    let result = client::rpc(
        Method::ProcessInfo,
        Params::ProcessInfo(NameParams {
            name: args.target.clone(),
        }),
    )
    .await?;

    if let ResponseResult::ProcessInfo { process } = result {
        println!(
            "Environment for process '{}' (id: {}):",
            process.name, process.id
        );
        println!("  Command: {}", process.command);
        println!("  Mode:    {}", process.mode);
        println!("  Status:  {}", process.state.label());
    }

    Ok(())
}
