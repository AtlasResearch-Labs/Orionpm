use anyhow::Result;
use interprocess::local_socket::tokio::Stream;
use interprocess::local_socket::traits::tokio::Stream as _;
use orion_core::protocol::{Request, Response};
use serde_json;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tracing::{debug, error};

use crate::get_socket_name;

/// Connects to the running oriond daemon and sends a single request,
/// returning the parsed Response.
///
/// If the daemon is not running, this returns an error with a clear message.
pub async fn send(request: &Request) -> Result<Response> {
    let name = get_socket_name()?;
    let stream = Stream::connect(name).await.map_err(|e| {
        anyhow::anyhow!("Cannot connect to orion daemon. Is oriond running? ({})", e)
    })?;

    let (reader, mut writer) = stream.split();
    let mut buf_reader = BufReader::new(reader);

    // Serialize request as a single JSON line.
    let mut payload = serde_json::to_string(request)?;
    payload.push('\n');

    writer.write_all(payload.as_bytes()).await?;
    writer.flush().await?;

    debug!(method = ?request.method, id = request.id, "Sent IPC request");

    // Read one response line.
    let mut line = String::new();
    buf_reader.read_line(&mut line).await?;

    if line.is_empty() {
        return Err(anyhow::anyhow!(
            "Daemon closed the connection without responding"
        ));
    }

    let response: Response = serde_json::from_str(line.trim()).map_err(|e| {
        error!(raw = %line.trim(), error = %e, "Failed to parse daemon response");
        anyhow::anyhow!("Malformed response from daemon: {}", e)
    })?;

    Ok(response)
}

/// Connects to the daemon and opens a streaming log connection.
/// Calls `on_line` for each log line until the stream ends or an error occurs.
pub async fn stream_logs<F>(request: &Request, mut on_line: F) -> Result<()>
where
    F: FnMut(Response),
{
    let name = get_socket_name()?;
    let stream = Stream::connect(name).await.map_err(|e| {
        anyhow::anyhow!("Cannot connect to orion daemon. Is oriond running? ({})", e)
    })?;

    let (reader, mut writer) = stream.split();
    let mut buf_reader = BufReader::new(reader);

    let mut payload = serde_json::to_string(request)?;
    payload.push('\n');

    writer.write_all(payload.as_bytes()).await?;
    writer.flush().await?;

    loop {
        let mut line = String::new();
        let n = buf_reader.read_line(&mut line).await?;

        if n == 0 {
            // Connection closed by daemon.
            break;
        }

        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        match serde_json::from_str::<Response>(trimmed) {
            Ok(response) => {
                let is_end = matches!(
                    &response.body,
                    orion_core::protocol::ResponseBody::Result(
                        orion_core::protocol::ResponseResult::StreamEnd
                    )
                );
                on_line(response);
                if is_end {
                    break;
                }
            }
            Err(e) => {
                error!(raw = %trimmed, error = %e, "Failed to parse streaming response line");
                break;
            }
        }
    }

    Ok(())
}
