use anyhow::{Context, Result};
use clap::Args;
use orion_core::pm2::{parse_pm2_ecosystem, to_orion_toml};
use orion_core::protocol::{Method, Params, StartParams};
use std::path::PathBuf;

use super::client;
use super::list::{self, ListArgs};

#[derive(Args)]
pub struct ImportArgs {
    /// Path to PM2 configuration file (ecosystem.config.js, ecosystem.json, pm2.json, etc.).
    pub source: PathBuf,

    /// Output path for the generated orion.toml configuration.
    #[arg(short, long, default_value = "orion.toml")]
    pub output: PathBuf,

    /// Immediately start all imported processes after generating orion.toml.
    #[arg(long)]
    pub start: bool,
}

pub async fn run(args: ImportArgs) -> Result<()> {
    if !args.source.exists() {
        anyhow::bail!(
            "Source configuration file '{}' does not exist",
            args.source.display()
        );
    }

    println!(
        "Importing PM2 configuration from '{}'...",
        args.source.display()
    );
    let apps = parse_pm2_ecosystem(&args.source).with_context(|| {
        format!(
            "Failed to parse PM2 configuration from '{}'",
            args.source.display()
        )
    })?;

    if apps.is_empty() {
        anyhow::bail!("No applications found in '{}'", args.source.display());
    }

    println!("Discovered {} managed application(s):", apps.len());
    for (name, app) in &apps {
        println!(
            "  - {} (command: '{}', instances: {}, mode: {:?})",
            name, app.command, app.instances, app.mode
        );
    }

    let toml_content = to_orion_toml(&apps)?;
    std::fs::write(&args.output, toml_content).with_context(|| {
        format!(
            "Failed to write output config to '{}'",
            args.output.display()
        )
    })?;

    println!(
        "Successfully exported native configuration to '{}'.",
        args.output.display()
    );

    if args.start {
        println!("Starting all imported applications...");
        for (name, app_config) in apps {
            let start_params = StartParams {
                name: name.clone(),
                config: app_config,
            };
            client::rpc(Method::ProcessStart, Params::ProcessStart(start_params)).await?;
            println!("Started process '{}'", name);
        }

        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        list::run(ListArgs::default()).await?;
    }

    Ok(())
}
