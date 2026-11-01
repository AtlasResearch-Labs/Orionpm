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
/// `ready_tx` is fired with `Ok(())` once the listener is successfully bound,
/// or `Err(reason)` if binding fails, allowing the caller to gate further
/// startup work (e.g. restoring saved processes) on a live socket.
///
/// This function runs indefinitely until cancelled via the shutdown token.
pub async fn run<H, Fut>(
    handler: H,
    shutdown: tokio_util::sync::CancellationToken,
    ready_tx: tokio::sync::oneshot::Sender<std::result::Result<(), String>>,
) -> Result<()>
where
    H: Fn(Request, Arc<Mutex<ResponseWriter>>) -> Fut + Send + Sync + Clone + 'static,
    Fut: Future<Output = Result<()>> + Send + 'static,
{
    #[cfg(not(windows))]
    {
        let socket_path = crate::get_socket_path();
        if socket_path.exists() {
            // Check if another live daemon is currently answering on this socket.
            let is_alive = if let Ok(socket_name) = get_socket_name() {
                Stream::connect(socket_name).await.is_ok()
            } else {
                false
            };
            if is_alive {
                // A live daemon is already running — signal failure and abort.
                let _ = ready_tx.send(Err(format!(
                    "Address already in use: a live oriond is already listening on {:?}",
                    socket_path
                )));
                return Err(anyhow::anyhow!(
                    "IPC socket already in use by a live daemon: {:?}",
                    socket_path
                ));
            }
            debug!("Removing stale IPC socket: {:?}", socket_path);
            let _ = std::fs::remove_file(&socket_path);
        }
    }

    let name = match get_socket_name() {
        Ok(n) => n,
        Err(e) => {
            let _ = ready_tx.send(Err(format!("Failed to resolve socket name: {}", e)));
            return Err(e);
        }
    };
    let listener = match ListenerOptions::new().name(name).create_tokio() {
        Ok(l) => l,
        Err(e) => {
            let _ = ready_tx.send(Err(format!("Failed to bind IPC socket: {}", e)));
            return Err(e.into());
        }
    };

    // Signal to daemon.rs that the socket is live and processes can be restored.
    let _ = ready_tx.send(Ok(()));
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

    #[cfg(not(windows))]
    {
        let socket_path = crate::get_socket_path();
        let _ = std::fs::remove_file(&socket_path);
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
        // Fix Issue 7: treat broken-pipe / EPIPE / POLLHUP (read returning
        // error or 0 bytes) as a clean client disconnect rather than an error.
        let n = match buf_reader.read_line(&mut line).await {
            Ok(n) => n,
            Err(e) => {
                // EPIPE / connection reset / terminal closed — not a real error.
                debug!(error = %e, "IPC connection read error (client disconnected)");
                return Ok(());
            }
        };

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
                        // Suppress broken pipe errors from log streaming tasks
                        // that are terminated by the client closing the terminal.
                        let is_pipe_error = e
                            .downcast_ref::<std::io::Error>()
                            .map(|io| {
                                matches!(
                                    io.kind(),
                                    std::io::ErrorKind::BrokenPipe
                                        | std::io::ErrorKind::ConnectionReset
                                )
                            })
                            .unwrap_or(false);
                        if !is_pipe_error {
                            error!(error = %e, "Handler error processing request");
                        }
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
