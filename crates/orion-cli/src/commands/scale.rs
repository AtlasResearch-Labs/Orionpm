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
    #[arg(allow_hyphen_values = true)]
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

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Parser)]
    struct Cli {
        #[command(flatten)]
        scale: ScaleArgs,
    }

    #[test]
    fn test_scale_negative_arg_parsing() {
        let args = Cli::try_parse_from(["orion", "app", "-1"]).expect("failed to parse -1");
        assert_eq!(args.scale.target, "app");
        assert_eq!(args.scale.count, "-1");

        let args_pos = Cli::try_parse_from(["orion", "app", "+2"]).expect("failed to parse +2");
        assert_eq!(args_pos.scale.count, "+2");
    }
}
