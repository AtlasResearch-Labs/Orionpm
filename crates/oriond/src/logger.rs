use orion_core::protocol::LogLine;
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc};

use crate::storage::Storage;

/// Non-blocking log bus that receives stdout/stderr lines from supervised
/// processes, commits them asynchronously in batches to the SQLite storage layer,
/// and broadcasts them to active IPC streaming clients.
#[derive(Clone)]
pub struct LogBus {
    storage: Storage,
    sender: Arc<broadcast::Sender<(String, LogLine)>>,
    batch_tx: mpsc::Sender<(String, LogLine)>,
}

impl std::fmt::Debug for LogBus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "LogBus")
    }
}

impl LogBus {
    /// Create a new log bus with a buffer capacity of 2048 log lines
    /// and an async batch writer channel of 10,000 entries.
    pub fn new(storage: Storage) -> Self {
        let (sender, _) = broadcast::channel(2048);
        let (batch_tx, mut batch_rx) = mpsc::channel::<(String, LogLine)>(10_000);

        // Fix P0 Bug 7: Dedicated background worker thread for batched SQLite writes.
        // Drains up to 100 entries or writes every 50ms in a single transaction,
        // eliminating lock contention and preventing stdout/stderr backpressure.
        let storage_clone = storage.clone();
        tokio::spawn(async move {
            let mut buffer = Vec::with_capacity(128);
            let mut interval = tokio::time::interval(tokio::time::Duration::from_millis(50));

            loop {
                tokio::select! {
                    Some(item) = batch_rx.recv() => {
                        buffer.push(item);
                        if buffer.len() >= 100 {
                            let to_write = std::mem::replace(&mut buffer, Vec::with_capacity(128));
                            let storage_ref = storage_clone.clone();
                            let _ = tokio::task::spawn_blocking(move || {
                                storage_ref.insert_logs_batch(&to_write)
                            }).await;
                        }
                    }
                    _ = interval.tick() => {
                        if !buffer.is_empty() {
                            let to_write = std::mem::replace(&mut buffer, Vec::with_capacity(128));
                            let storage_ref = storage_clone.clone();
                            let _ = tokio::task::spawn_blocking(move || {
                                storage_ref.insert_logs_batch(&to_write)
                            }).await;
                        }
                    }
                    else => break,
                }
            }
        });

        Self {
            storage,
            sender: Arc::new(sender),
            batch_tx,
        }
    }

    /// Record a log line: write to batch queue and broadcast to subscribers.
    pub fn log(&self, name: &str, line: LogLine) {
        let _ = self.batch_tx.try_send((name.to_string(), line.clone()));
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
