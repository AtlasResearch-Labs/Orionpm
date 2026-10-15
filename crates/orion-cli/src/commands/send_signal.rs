use super::client;
use anyhow::{bail, Result};
use clap::Args;
use orion_core::protocol::{Method, Params, ResponseResult, SendSignalParams};

#[derive(Args)]
pub struct SendSignalArgs {
    /// POSIX signal name (e.g. SIGUSR1, SIGHUP, SIGTERM, SIGINT, SIGKILL) or numeric code.
    pub signal: String,

    /// Target process name, numeric ID, or "all".
    pub target: String,
}

pub async fn run(args: SendSignalArgs) -> Result<()> {
    let params = SendSignalParams {
        signal: args.signal,
        target: args.target,
    };

    let res = client::rpc(Method::ProcessSendSignal, Params::ProcessSendSignal(params)).await?;

    match res {
        ResponseResult::Ack { message } => {
            println!("{}", message);
            Ok(())
        }
        _ => bail!("Unexpected response from daemon"),
    }
}
