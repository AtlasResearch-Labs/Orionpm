use anyhow::Result;
use orion_core::protocol::{EmptyParams, Method, Params, ResponseResult};

use super::client;

pub async fn run() -> Result<()> {
    let result = client::rpc(Method::DaemonSave, Params::DaemonSave(EmptyParams {})).await?;
    if let ResponseResult::Ack { message } = result {
        println!("{}", message);
    }
    Ok(())
}
