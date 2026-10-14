mod commands;
mod tui;

use anyhow::Result;
use clap::{ArgAction, Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "orion",
    version = env!("CARGO_PKG_VERSION"),
    about = "orion process manager — Atlas Research Labs",
    long_about = None,
    disable_version_flag = true,
    arg_required_else_help = true,
)]
struct Cli {
    /// Print version information
    #[arg(short = 'v', short_alias = 'V', long = "version", action = ArgAction::Version)]
    version: Option<bool>,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Start a process from a config file or a script path.
    Start(commands::start::StartArgs),

    /// Stop a running process.
    Stop(commands::stop::StopArgs),

    /// Restart a process (stop then start).
    #[command(alias = "res")]
    Restart(commands::restart::RestartArgs),

    /// Zero-downtime reload (send SIGUSR2 and replace workers).
    #[command(alias = "gracefulReload")]
    Reload(commands::reload::ReloadArgs),

    /// Remove a process from orion management.
    #[command(alias = "del", alias = "rm")]
    Delete(commands::delete::DeleteArgs),

    /// List all managed processes with status and metrics.
    #[command(alias = "ls", alias = "status")]
    List(commands::list::ListArgs),

    /// Show detailed info about a single process.
    Info(commands::info::InfoArgs),

    /// Show or tail logs for a process.
    Logs(commands::logs::LogsArgs),

    /// Launch the interactive terminal monitor.
    Monit,

    /// Persist the current process list to disk.
    Save,

    /// Restore the last saved process list.
    Restore,

    /// Install or remove the oriond system service (startup on boot).
    Startup(commands::startup::StartupArgs),

    /// Show environment variables for a process.
    Env(commands::env::EnvArgs),

    /// Control the oriond daemon process.
    Daemon(commands::daemon::DaemonArgs),

    /// Import and convert PM2 ecosystem configurations (ecosystem.config.js, ecosystem.json).
    Import(commands::import::ImportArgs),
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    let command = match cli.command {
        Some(cmd) => cmd,
        None => return Ok(()),
    };

    let res = match command {
        Command::Start(args) => commands::start::run(args).await,
        Command::Stop(args) => commands::stop::run(args).await,
        Command::Restart(args) => commands::restart::run(args).await,
        Command::Reload(args) => commands::reload::run(args).await,
        Command::Delete(args) => commands::delete::run(args).await,
        Command::List(args) => commands::list::run(args).await,
        Command::Info(args) => commands::info::run(args).await,
        Command::Logs(args) => commands::logs::run(args).await,
        Command::Monit => tui::run().await,
        Command::Save => commands::save::run().await,
        Command::Restore => commands::restore::run().await,
        Command::Startup(args) => commands::startup::run(args).await,
        Command::Env(args) => commands::env::run(args).await,
        Command::Daemon(args) => commands::daemon::run(args).await,
        Command::Import(args) => commands::import::run(args).await,
    };

    if let Err(e) = res {
        eprintln!("Error: {:#}", e);
        std::process::exit(1);
    }

    Ok(())
}
