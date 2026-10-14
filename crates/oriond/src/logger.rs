use std::sync::Arc;
use tokio::sync::broadcast;
use orion_core::protocol::LogLine;

use crate::storage::Storage;

/// Non-blocking log bus that receives stdout/stderr lines from supervised
/// processes, commits them to the SQLite storage layer, and broadcasts
/// them to active IPC streaming clients.
#[derive(Clone)]
pub struct LogBus {
    storage: Storage,
    sender: Arc<broadcast::Sender<(String, LogLine)>>,
}

impl std::fmt::Debug for LogBus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "LogBus")
    }
}

impl LogBus {
    /// Create a new log bus with a buffer capacity of 2048 log lines.
    pub fn new(storage: Storage) -> Self {
        let (sender, _) = broadcast::channel(2048);
        Self {
            storage,
            sender: Arc::new(sender),
        }
    }

    /// Record a log line: write to SQLite storage and broadcast to subscribers.
    pub fn log(&self, name: &str, line: LogLine) {
        if let Err(e) = self.storage.insert_log(name, &line) {
            tracing::error!(name = %name, error = %e, "Failed to persist log line");
        }
        let _ = self.sender.send((name.to_string(), line));
    }

    /// Subscribe to live log lines for all processes.
    pub fn subscribe(&self) -> broadcast::Receiver<(String, LogLine)> {
        self.sender.subscribe()
    }

    /// Direct reference to underlying SQLite storage.
    #[allow(dead_code)]
    pub fn storage(&self) -> &Storage {
        &self.storage
    }
}
