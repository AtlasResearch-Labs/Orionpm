use anyhow::Result;
use clap::Args;
use service_manager::{ServiceInstallCtx, ServiceLabel, ServiceManager, ServiceUninstallCtx};

#[derive(Args)]
pub struct StartupArgs {
    /// Remove the installed service instead of installing it.
    #[arg(long)]
    pub delete: bool,

    /// Specify the user to run the service as (defaults to SUDO_USER or current user).
    #[arg(long, short)]
    pub user: Option<String>,
}

pub async fn run(args: StartupArgs) -> Result<()> {
    let manager = <dyn ServiceManager>::native()?;
    let label: ServiceLabel = "dev.atlasresearch.orion".parse()?;

    if args.delete {
        manager.uninstall(ServiceUninstallCtx { label })?;
        println!("orion daemon startup service uninstalled.");
    } else {
        let mut exe = std::env::current_exe()?;
        exe.set_file_name(if cfg!(windows) {
            "oriond.exe"
        } else {
            "oriond"
        });

        let target_user = args.user.or_else(|| std::env::var("SUDO_USER").ok());

        let (username, environment) = if let Some(u) = target_user {
            let home_dir = if cfg!(unix) {
                if u == "root" {
                    Some("/root".to_string())
                } else {
                    let candidate = format!("/home/{}", u);
                    if std::path::Path::new(&candidate).exists() {
                        Some(candidate)
                    } else {
                        std::env::var("HOME").ok()
                    }
                }
            } else {
                std::env::var("USERPROFILE").ok()
            };

            let env = home_dir.map(|h| vec![("HOME".to_string(), h)]);
            (Some(u), env)
        } else {
            (None, None)
        };

        manager.install(ServiceInstallCtx {
            label,
            program: exe,
            args: vec![],
            contents: None,
            username,
            working_directory: None,
            environment,
            autostart: true,
        })?;
        println!("orion daemon installed as startup service.");
    }
    Ok(())
}
