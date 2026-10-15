use std::time::SystemTime;

use serde::{Deserialize, Serialize};

/// The live runtime state of a supervised process.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "state", rename_all = "lowercase")]
pub enum ProcessState {
    /// oriond is in the process of launching the child.
    Starting,

    /// The child is running.
    Running {
        /// OS-assigned process ID.
        pid: u32,
    },

    /// oriond sent SIGTERM and is waiting for the child to exit.
    Stopping,

    /// The child exited cleanly (or was stopped by the user).
    Stopped,

    /// The child exited unexpectedly or max_restarts was exceeded.
    Errored {
        /// The exit code returned by the child, if available.
        exit_code: Option<i32>,
        /// Human-readable reason for entering the errored state.
        message: String,
    },

    /// oriond is waiting through backoff delay before restarting the child.
    Restarting,
}

impl ProcessState {
    /// Returns a short ASCII label for display in the TUI and CLI list.
    /// No emojis. No color codes. Plain text.
    pub fn label(&self) -> &'static str {
        match self {
            ProcessState::Starting => "starting",
            ProcessState::Running { .. } => "online",
            ProcessState::Stopping => "stopping",
            ProcessState::Stopped => "stopped",
            ProcessState::Errored { .. } => "errored",
            ProcessState::Restarting => "restarting",
        }
    }

    /// Returns true if the process is currently expected to be consuming resources.
    pub fn is_active(&self) -> bool {
        matches!(
            self,
            ProcessState::Starting | ProcessState::Running { .. } | ProcessState::Restarting
        )
    }
}

/// A snapshot of a managed process at a point in time.
/// This is the structure serialized and sent over IPC in response to
/// `process.list` and `process.info` requests.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessInfo {
    /// Internal numeric ID assigned by oriond (monotonically increasing).
    pub id: u32,

    /// The name key from the config (or the --name flag).
    pub name: String,

    /// The state of the process at the time of this snapshot.
    pub state: ProcessState,

    /// Number of times oriond has restarted this process since it was registered.
    pub restart_count: u32,

    /// Wall-clock time when oriond registered this process (Unix timestamp seconds).
    pub registered_at: u64,

    /// Wall-clock time when the process last started (Unix timestamp seconds).
    /// None if the process has never successfully started.
    pub started_at: Option<u64>,

    /// Most recent CPU usage percentage (0.0 to 100.0) as measured by sysinfo.
    /// None if the process is not running or metrics are not yet available.
    pub cpu_pct: Option<f32>,

    /// Most recent RSS memory usage in bytes as measured by sysinfo.
    /// None if the process is not running or metrics are not yet available.
    pub memory_bytes: Option<u64>,

    /// Execution mode (fork or cluster).
    pub mode: String,

    /// The command being run.
    pub command: String,
}

impl ProcessInfo {
    /// Returns memory formatted as a human-readable string (e.g. "128.4 MB").
    /// Returns "-" when memory data is unavailable.
    pub fn memory_display(&self) -> String {
        match self.memory_bytes {
            Some(bytes) if bytes > 0 => {
                let mb = bytes as f64 / 1_048_576.0;
                format!("{:.1} MB", mb)
            }
            _ => "-".to_string(),
        }
    }

    /// Returns CPU formatted as a string (e.g. "1.2%").
    /// Returns "-" when CPU data is unavailable.
    pub fn cpu_display(&self) -> String {
        match self.cpu_pct {
            Some(pct) => format!("{:.1}%", pct),
            None => "-".to_string(),
        }
    }

    /// Returns uptime as a human-readable string (e.g. "2h 14m").
    /// Returns "-" when started_at is not available.
    pub fn uptime_display(&self) -> String {
        let Some(started) = self.started_at else {
            return "-".to_string();
        };

        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        if now < started {
            return "-".to_string();
        }

        let secs = now - started;
        if secs < 60 {
            format!("{}s", secs)
        } else if secs < 3600 {
            format!("{}m {}s", secs / 60, secs % 60)
        } else {
            format!("{}h {}m", secs / 3600, (secs % 3600) / 60)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_labels_are_ascii() {
        let states = [
            ProcessState::Starting,
            ProcessState::Running { pid: 1 },
            ProcessState::Stopping,
            ProcessState::Stopped,
            ProcessState::Errored {
                exit_code: Some(1),
                message: "crash".to_string(),
            },
            ProcessState::Restarting,
        ];
        for state in &states {
            let label = state.label();
            assert!(
                label.is_ascii(),
                "label '{}' contains non-ASCII characters",
                label
            );
            assert!(!label.is_empty(), "label must not be empty");
        }
    }

    #[test]
    fn memory_display_formats_correctly() {
        let mut info = ProcessInfo {
            id: 0,
            name: "test".to_string(),
            state: ProcessState::Running { pid: 1 },
            restart_count: 0,
            registered_at: 0,
            started_at: None,
            cpu_pct: None,
            memory_bytes: Some(134_217_728), // 128 MB exactly
            mode: "fork".to_string(),
            command: "node".to_string(),
        };
        assert_eq!(info.memory_display(), "128.0 MB");

        info.memory_bytes = None;
        assert_eq!(info.memory_display(), "-");
    }
}
