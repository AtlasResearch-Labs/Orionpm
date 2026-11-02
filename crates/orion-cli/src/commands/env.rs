use anyhow::Result;
use clap::Args;
use orion_core::protocol::{Method, NameParams, Params, ResponseResult};

use super::client;

#[derive(Args)]
pub struct EnvArgs {
    /// Name or numeric ID of the process.
    pub target: String,

    /// Output environment variables as JSON.
    #[arg(long, short = 'j')]
    pub json: bool,
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
        if args.json {
            println!("{}", serde_json::to_string_pretty(&process.env)?);
            return Ok(());
        }

        println!(
            "Environment for process '{}' (id: {}):",
            process.name, process.id
        );
        if process.env.is_empty() {
            println!("  (No custom environment variables configured)");
        } else {
            let mut keys: Vec<_> = process.env.keys().collect();
            keys.sort();
            for k in keys {
                println!("  {}={}", k, process.env[k]);
            }
        }
    }

    Ok(())
}
