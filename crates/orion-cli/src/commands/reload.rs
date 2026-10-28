use anyhow::Result;
use clap::Args;
use orion_core::protocol::{Method, NameParams, Params, ResponseResult};

use super::client;
use super::list::{self, ListArgs};

#[derive(Args)]
pub struct ReloadArgs {
    /// Name, numeric ID, or "all".
    pub target: String,
}

pub async fn run(args: ReloadArgs) -> Result<()> {
    let path = std::path::Path::new(&args.target);
    if path.is_file()
        && (args.target.ends_with(".toml")
            || args.target.ends_with(".js")
            || args.target.ends_with(".cjs")
            || args.target.ends_with(".mjs")
            || args.target.ends_with(".json")
            || args.target.ends_with("Procfile"))
    {
        println!("Reloading configuration from '{}'...", args.target);
        return super::start::run(super::start::StartArgs {
            target: args.target,
            name: None,
            instances: None,
            watch: false,
            watch_paths: Vec::new(),
            no_restart: false,
            env: Vec::new(),
            cwd: None,
            max_memory: None,
            stdout_log: None,
            stderr_log: None,
            health_http: None,
            health_tcp: None,
            health_interval: None,
            health_max_failures: None,
        })
        .await;
    }

    let result = client::rpc(
        Method::ProcessReload,
        Params::ProcessReload(NameParams {
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
