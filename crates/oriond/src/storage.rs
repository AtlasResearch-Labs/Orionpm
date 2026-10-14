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

        // Enable WAL mode for better concurrent read performance and
        // reduced contention between the log writer and IPC reader.
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")?;

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

        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS logs (
                id        INTEGER PRIMARY KEY AUTOINCREMENT,
                name      TEXT    NOT NULL,
                timestamp INTEGER NOT NULL,
                stream    TEXT    NOT NULL CHECK (stream IN ('stdout', 'stderr')),
                text      TEXT    NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_logs_name_timestamp
                ON logs (name, timestamp);

            CREATE INDEX IF NOT EXISTS idx_logs_name_id
                ON logs (name, id);

            -- Persisted process snapshot for save/restore across daemon restarts.
            CREATE TABLE IF NOT EXISTS process_state (
                name           TEXT    PRIMARY KEY,
                config_json    TEXT    NOT NULL,
                restart_count  INTEGER NOT NULL DEFAULT 0,
                registered_at  INTEGER NOT NULL
            );
        ",
        )?;

        Ok(())
    }

    // ─── Log operations ────────────────────────────────────────────────────────

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
    #[allow(dead_code)]
    pub fn get_logs(&self, name: &str, limit: usize) -> Result<Vec<LogLine>> {
        let entries = self.get_logs_with_process(name, limit)?;
        Ok(entries.into_iter().map(|(_, l)| l).collect())
    }

    /// Retrieve the most recent `limit` log lines, returning `(process_name, LogLine)` tuples.
    /// If `name` is "all" or empty, retrieves up to `limit` lines per process, sorted chronologically.
    pub fn get_logs_with_process(&self, name: &str, limit: usize) -> Result<Vec<(String, LogLine)>> {
        let conn = self.conn.lock().expect("storage lock poisoned");
        if name == "all" || name.is_empty() {
            let mut name_stmt = conn.prepare("SELECT DISTINCT name FROM logs")?;
            let names_iter = name_stmt.query_map([], |row| row.get::<_, String>(0))?;
            let distinct_names: Vec<String> = names_iter.flatten().collect();

            let mut all_lines = Vec::new();
            for proc_name in distinct_names {
                let mut stmt = conn.prepare(
                    "SELECT id, timestamp, stream, text
                     FROM (
                         SELECT id, timestamp, stream, text
                         FROM logs
                         WHERE name = ?1
                         ORDER BY id DESC
                         LIMIT ?2
                     )
                     ORDER BY id ASC",
                )?;

                let rows = stmt.query_map(params![proc_name, limit as i64], |row| {
                    let id: i64 = row.get(0)?;
                    let stream_str: String = row.get(2)?;
                    let stream = if stream_str == "stdout" {
                        LogStream::Stdout
                    } else {
                        LogStream::Stderr
                    };
                    Ok((
                        id,
                        proc_name.clone(),
                        LogLine {
                            timestamp: row.get::<_, i64>(1)? as u64,
                            stream,
                            text: row.get(3)?,
                        },
                    ))
                })?;

                for entry in rows.flatten() {
                    all_lines.push(entry);
                }
            }

            all_lines.sort_by_key(|(id, _, _)| *id);
            Ok(all_lines.into_iter().map(|(_, name, line)| (name, line)).collect())
        } else {
            let mut stmt = conn.prepare(
                "SELECT id, timestamp, stream, text
                 FROM (
                     SELECT id, timestamp, stream, text
                     FROM logs
                     WHERE name = ?1
                     ORDER BY id DESC
                     LIMIT ?2
                 )
                 ORDER BY id ASC",
            )?;

            let rows = stmt.query_map(params![name, limit as i64], |row| {
                let stream_str: String = row.get(2)?;
                let stream = if stream_str == "stdout" {
                    LogStream::Stdout
                } else {
                    LogStream::Stderr
                };
                Ok((
                    name.to_string(),
                    LogLine {
                        timestamp: row.get::<_, i64>(1)? as u64,
                        stream,
                        text: row.get(3)?,
                    },
                ))
            })?;

            let mut lines = Vec::new();
            for r in rows {
                lines.push(r?);
            }
            Ok(lines)
        }
    }

    /// Prune log lines for all processes, keeping only the most recent `keep`
    /// lines per process. Returns the total number of rows deleted.
    /// Called periodically by the background log-pruner task in daemon.rs.
    pub fn prune_logs(&self, keep: usize) -> Result<u64> {
        let conn = self.conn.lock().expect("storage lock poisoned");

        // Collect distinct process names.
        let mut names_stmt = conn.prepare("SELECT DISTINCT name FROM logs")?;
        let names: Vec<String> = names_stmt
            .query_map([], |row| row.get(0))?
            .filter_map(|r| r.ok())
            .collect();

        let mut total_deleted: u64 = 0;
        for name in names {
            let deleted = conn.execute(
                "DELETE FROM logs
                 WHERE name = ?1
                   AND id NOT IN (
                       SELECT id FROM logs WHERE name = ?1 ORDER BY id DESC LIMIT ?2
                   )",
                params![name, keep as i64],
            )?;
            total_deleted += deleted as u64;
        }

        Ok(total_deleted)
    }

    /// Delete all log lines for a specific process (orion flush <name>).
    pub fn flush_logs(&self, name: &str) -> Result<u64> {
        let conn = self.conn.lock().expect("storage lock poisoned");
        let deleted = conn.execute("DELETE FROM logs WHERE name = ?1", params![name])?;
        Ok(deleted as u64)
    }

    // ─── Process state persistence (save / restore) ────────────────────────────

    /// Persist one process configuration to the process_state table.
    /// Uses UPSERT so repeated saves overwrite previous data cleanly.
    pub fn save_process(
        &self,
        name: &str,
        config: &orion_core::config::AppConfig,
        restart_count: u32,
        registered_at: u64,
    ) -> Result<()> {
        let config_json = serde_json::to_string(config)?;
        let conn = self.conn.lock().expect("storage lock poisoned");
        conn.execute(
            "INSERT INTO process_state (name, config_json, restart_count, registered_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(name) DO UPDATE SET
                 config_json   = excluded.config_json,
                 restart_count = excluded.restart_count,
                 registered_at = excluded.registered_at",
            params![name, config_json, restart_count, registered_at as i64],
        )?;
        Ok(())
    }

    /// Remove a single process entry from the saved state (called on delete).
    pub fn remove_saved_process(&self, name: &str) -> Result<()> {
        let conn = self.conn.lock().expect("storage lock poisoned");
        conn.execute("DELETE FROM process_state WHERE name = ?1", params![name])?;
        Ok(())
    }

    /// Wipe all saved process state (called when 'orion delete all' is run).
    #[allow(dead_code)]
    pub fn clear_all_saved_processes(&self) -> Result<()> {
        let conn = self.conn.lock().expect("storage lock poisoned");
        conn.execute("DELETE FROM process_state", [])?;
        Ok(())
    }

    /// Load all persisted process configurations.
    /// Returns a list of (name, config, restart_count, registered_at).
    /// Entries with corrupt JSON are skipped with a warning rather than aborting.
    pub fn load_saved_processes(
        &self,
    ) -> Result<Vec<(String, orion_core::config::AppConfig, u32, u64)>> {
        let conn = self.conn.lock().expect("storage lock poisoned");
        let mut stmt = conn.prepare(
            "SELECT name, config_json, restart_count, registered_at
             FROM process_state
             ORDER BY registered_at ASC",
        )?;

        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, u32>(2)?,
                row.get::<_, i64>(3)? as u64,
            ))
        })?;

        let mut result = Vec::new();
        for row in rows {
            let (name, config_json, restart_count, registered_at) = row?;
            match serde_json::from_str::<orion_core::config::AppConfig>(&config_json) {
                Ok(config) => result.push((name, config, restart_count, registered_at)),
                Err(e) => {
                    tracing::warn!(
                        name = %name,
                        error = %e,
                        "Skipping saved process with unparseable config"
                    );
                }
            }
        }

        Ok(result)
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
