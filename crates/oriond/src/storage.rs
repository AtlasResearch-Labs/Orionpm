use anyhow::Result;
use rusqlite::{params, Connection};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use orion_core::protocol::{LogLine, LogStream};

/// The embedded SQLite storage for oriond.
/// Wrapped in Arc<Mutex<>> so it can be shared across async tasks.
#[derive(Clone)]
pub struct Storage {
    conn: Arc<Mutex<Connection>>,
}

impl std::fmt::Debug for Storage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Storage(SQLite)")
    }
}

impl Storage {
    /// Open (or create) the orion database at the platform-appropriate data directory.
    /// The database is created if it does not exist.
    pub fn open() -> Result<Self> {
        let path = data_dir().join("orion.db");

        // Ensure the directory exists.
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let conn = Connection::open(&path)?;
        let storage = Self {
            conn: Arc::new(Mutex::new(conn)),
        };
        storage.initialize_schema()?;
        tracing::info!(path = %path.display(), "Storage opened");
        Ok(storage)
    }

    /// Create tables if they do not already exist.
    fn initialize_schema(&self) -> Result<()> {
        let conn = self.conn.lock().expect("storage lock poisoned");

        conn.execute_batch("
            CREATE TABLE IF NOT EXISTS logs (
                id        INTEGER PRIMARY KEY AUTOINCREMENT,
                name      TEXT    NOT NULL,
                timestamp INTEGER NOT NULL,
                stream    TEXT    NOT NULL CHECK (stream IN ('stdout', 'stderr')),
                text      TEXT    NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_logs_name_timestamp
                ON logs (name, timestamp);

            CREATE TABLE IF NOT EXISTS process_state (
                name           TEXT    PRIMARY KEY,
                config_json    TEXT    NOT NULL,
                restart_count  INTEGER NOT NULL DEFAULT 0,
                registered_at  INTEGER NOT NULL
            );
        ")?;

        Ok(())
    }

    /// Insert a log line for a named process.
    pub fn insert_log(&self, name: &str, line: &LogLine) -> Result<()> {
        let conn = self.conn.lock().expect("storage lock poisoned");
        let stream = match line.stream {
            LogStream::Stdout => "stdout",
            LogStream::Stderr => "stderr",
        };
        conn.execute(
            "INSERT INTO logs (name, timestamp, stream, text) VALUES (?1, ?2, ?3, ?4)",
            params![name, line.timestamp, stream, line.text],
        )?;
        Ok(())
    }

    /// Retrieve the most recent `limit` log lines for a named process.
    /// Lines are returned in chronological order (oldest first).
    pub fn get_logs(&self, name: &str, limit: usize) -> Result<Vec<LogLine>> {
        let conn = self.conn.lock().expect("storage lock poisoned");
        let mut stmt = conn.prepare(
            "SELECT timestamp, stream, text
             FROM (
                 SELECT timestamp, stream, text
                 FROM logs
                 WHERE name = ?1
                 ORDER BY id DESC
                 LIMIT ?2
             )
             ORDER BY timestamp ASC",
        )?;

        let rows = stmt.query_map(params![name, limit as i64], |row| {
            let stream_str: String = row.get(1)?;
            let stream = if stream_str == "stdout" {
                LogStream::Stdout
            } else {
                LogStream::Stderr
            };
            Ok(LogLine {
                timestamp: row.get::<_, i64>(0)? as u64,
                stream,
                text: row.get(2)?,
            })
        })?;

        let mut lines = Vec::new();
        for row in rows {
            lines.push(row?);
        }
        Ok(lines)
    }

    /// Prune log lines for a process, keeping only the most recent `keep` lines.
    /// This should be called periodically to prevent unbounded growth.
    #[allow(dead_code)]
    pub fn prune_logs(&self, name: &str, keep: usize) -> Result<()> {
        let conn = self.conn.lock().expect("storage lock poisoned");
        conn.execute(
            "DELETE FROM logs
             WHERE name = ?1
               AND id NOT IN (
                   SELECT id FROM logs WHERE name = ?1 ORDER BY id DESC LIMIT ?2
               )",
            params![name, keep as i64],
        )?;
        Ok(())
    }
}

/// Returns the platform-appropriate data directory for oriond state files.
fn data_dir() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        let appdata = std::env::var("APPDATA").unwrap_or_else(|_| ".".to_string());
        PathBuf::from(appdata).join("orion")
    }

    #[cfg(target_os = "macos")]
    {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        PathBuf::from(home)
            .join("Library")
            .join("Application Support")
            .join("orion")
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        // XDG-compliant: respect XDG_DATA_HOME, fall back to ~/.local/share/orion
        let base = std::env::var("XDG_DATA_HOME").unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
            format!("{}/.local/share", home)
        });
        PathBuf::from(base).join("orion")
    }
}
