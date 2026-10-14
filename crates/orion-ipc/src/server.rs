use anyhow::Result;
use interprocess::local_socket::tokio::{SendHalf, Stream};
use interprocess::local_socket::traits::tokio::{Listener as _, Stream as _};
use interprocess::local_socket::ListenerOptions;
use orion_core::protocol::{Request, Response};
use serde_json;
use std::future::Future;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::Mutex;
use tracing::{debug, error, info};

use crate::get_socket_name;

/// A handle for writing NDJSON response lines back to the connected CLI client.
pub struct ResponseWriter {
    writer: SendHalf,
}

impl ResponseWriter {
    pub fn new(writer: SendHalf) -> Self {
        Self { writer }
    }

    /// Send one response line to the client.
    pub async fn send(&mut self, response: &Response) -> Result<()> {
        let mut line = serde_json::to_string(response)?;
        line.push('\n');
        self.writer.write_all(line.as_bytes()).await?;
        self.writer.flush().await?;
        Ok(())
    }
}

/// Start the IPC server. Accepts connections in a loop and dispatches each
/// incoming request to the provided handler function.
///
/// This function runs indefinitely until cancelled via the shutdown token.
pub async fn run<H, Fut>(handler: H, shutdown: tokio_util::sync::CancellationToken) -> Result<()>
where
    H: Fn(Request, Arc<Mutex<ResponseWriter>>) -> Fut + Send + Sync + Clone + 'static,
    Fut: Future<Output = Result<()>> + Send + 'static,
{
    let name = get_socket_name()?;
    let listener = ListenerOptions::new().name(name).create_tokio()?;
    info!("IPC server listening");

    loop {
        tokio::select! {
            accept = listener.accept() => {
                match accept {
                    Ok(stream) => {
                        let handler = handler.clone();
                        tokio::spawn(async move {
                            if let Err(e) = handle_connection(stream, handler).await {
                                error!(error = %e, "IPC connection error");
                            }
                        });
                    }
                    Err(e) => {
                        error!(error = %e, "IPC accept error");
                    }
                }
            }
            _ = shutdown.cancelled() => {
                info!("IPC server shutting down");
                break;
            }
        }
    }

    Ok(())
}

async fn handle_connection<H, Fut>(stream: Stream, handler: H) -> Result<()>
where
    H: Fn(Request, Arc<Mutex<ResponseWriter>>) -> Fut + Send + Sync + Clone + 'static,
    Fut: Future<Output = Result<()>> + Send + 'static,
{
    let (reader, writer) = stream.split();
    let mut buf_reader = BufReader::new(reader);
    let writer = Arc::new(Mutex::new(ResponseWriter::new(writer)));
    let mut line = String::new();

    loop {
        line.clear();
        let n = buf_reader.read_line(&mut line).await?;

        if n == 0 {
            // Client disconnected.
            break;
        }

        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        debug!(raw = %trimmed, "Received IPC request");

        match serde_json::from_str::<Request>(trimmed) {
            Ok(request) => {
                let handler = handler.clone();
                let writer_clone = Arc::clone(&writer);
                tokio::spawn(async move {
                    if let Err(e) = handler(request, writer_clone).await {
                        error!(error = %e, "Handler error processing request");
                    }
                });
            }
            Err(e) => {
                error!(raw = %trimmed, error = %e, "Failed to parse IPC request");
                let error_response = Response::err(0, -32700, format!("Parse error: {}", e));
                let mut w = writer.lock().await;
                let _ = w.send(&error_response).await;
            }
        }
    }

    Ok(())
}
