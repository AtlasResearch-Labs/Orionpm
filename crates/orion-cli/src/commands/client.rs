use anyhow::{bail, Context, Result};
use std::time::{Duration, Instant};
use orion_core::protocol::{EmptyParams, Method, Params, Request, ResponseBody, ResponseResult};

/// Ensure that oriond is running; if not, spawn it in the background
/// and wait until the IPC socket is ready.
pub async fn ensure_daemon() -> Result<()> {
    let ping = Request::new(0, Method::DaemonStatus, Params::DaemonStatus(EmptyParams {}));
    if orion_ipc::client::send(&ping).await.is_ok() {
        return Ok(());
    }

    println!("oriond is not running. Starting in background...");
    spawn_daemon()?;

    let start = Instant::now();
    let timeout = Duration::from_secs(4);
    while start.elapsed() < timeout {
        tokio::time::sleep(Duration::from_millis(200)).await;
        if orion_ipc::client::send(&ping).await.is_ok() {
            println!("oriond daemon connected.");
            return Ok(());
        }
    }

    bail!("Failed to connect to oriond daemon. Verify oriond binary exists.")
}

fn spawn_daemon() -> Result<()> {
    let mut exe = std::env::current_exe()?;
    exe.set_file_name(if cfg!(windows) { "oriond.exe" } else { "oriond" });

    let mut cmd = if exe.exists() {
        std::process::Command::new(exe)
    } else {
        std::process::Command::new("oriond")
    };

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW (0x08000000) | DETACHED_PROCESS (0x00000008) | CREATE_NEW_PROCESS_GROUP (0x00000200)
        cmd.creation_flags(0x08000000 | 0x00000008 | 0x00000200);
    }

    let log_path = std::env::temp_dir().join("oriond.log");
    let log_file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)?;
    let err_file = log_file.try_clone()?;

    cmd.stdin(std::process::Stdio::null())
        .stdout(log_file)
        .stderr(err_file)
        .spawn()
        .context("Could not launch oriond daemon")?;

    Ok(())
}

/// Send an RPC request to the daemon, ensuring the daemon is running first.
pub async fn rpc(method: Method, params: Params) -> Result<ResponseResult> {
    ensure_daemon().await?;
    let req = Request::new(1, method, params);
    let resp = orion_ipc::client::send(&req).await?;
    match resp.body {
        ResponseBody::Result(res) => Ok(res),
        ResponseBody::Error(err) => bail!("{}", err.message),
    }
}
