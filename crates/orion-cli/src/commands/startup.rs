use anyhow::Result;
use clap::Args;
use service_manager::{ServiceInstallCtx, ServiceLabel, ServiceManager, ServiceUninstallCtx};

#[derive(Args)]
pub struct StartupArgs {
    /// Remove the installed service instead of installing it.
    #[arg(long)]
    pub delete: bool,
}

pub async fn run(args: StartupArgs) -> Result<()> {
    let manager = <dyn ServiceManager>::native()?;
    let label: ServiceLabel = "dev.atlasresearch.orion".parse()?;

    if args.delete {
        manager.uninstall(ServiceUninstallCtx { label })?;
        println!("orion daemon startup service uninstalled.");
    } else {
        let mut exe = std::env::current_exe()?;
        exe.set_file_name(if cfg!(windows) { "oriond.exe" } else { "oriond" });
        manager.install(ServiceInstallCtx {
            label,
            program: exe,
            args: vec![],
            contents: None,
            username: None,
            working_directory: None,
            environment: None,
            autostart: true,
        })?;
        println!("orion daemon installed as startup service.");
    }
    Ok(())
}
