use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

use orion_core::config::AppConfig;
use orion_core::process_state::{ProcessInfo, ProcessState};

/// A handle to a running supervised process, stored in the registry.
#[derive(Debug)]
pub struct ProcessHandle {
    /// Sequential ID assigned at registration time.
    pub id: u32,
    /// The name key used to look up this process.
    pub name: String,
    /// The configuration that was used to start this process.
    pub config: AppConfig,
    /// Current state of the process.
    pub state: ProcessState,
    /// How many times this process has been restarted by oriond.
    pub restart_count: u32,
    /// Unix timestamp when this process was first registered.
    pub registered_at: u64,
    /// Unix timestamp when the process last transitioned to Running.
    pub started_at: Option<u64>,
    /// Most recent CPU usage reading (0.0–100.0). Updated by the metrics task.
    pub cpu_pct: Option<f32>,
    /// Most recent RSS memory in bytes. Updated by the metrics task.
    pub memory_bytes: Option<u64>,
    /// Cancellation token to shut down the supervisor task for this process.
    pub supervisor_cancel: tokio_util::sync::CancellationToken,
}

impl ProcessHandle {
    /// Produce a point-in-time snapshot suitable for sending over IPC.
    pub fn to_info(&self) -> ProcessInfo {
        ProcessInfo {
            id: self.id,
            name: self.name.clone(),
            state: self.state.clone(),
            restart_count: self.restart_count,
            registered_at: self.registered_at,
            started_at: self.started_at,
            cpu_pct: self.cpu_pct,
            memory_bytes: self.memory_bytes,
            mode: format!("{:?}", self.config.mode).to_lowercase(),
            command: if !self.config.args.is_empty() {
                format!("{} {}", self.config.command, self.config.args.join(" "))
            } else {
                self.config.command.clone()
            },
        }
    }
}

/// The shared process registry.
/// All access is through Arc<RwLock<>> so supervisor tasks and the IPC handler
/// can safely read and write from different threads.
#[derive(Clone, Debug)]
pub struct ProcessRegistry {
    inner: Arc<RwLock<RegistryInner>>,
}

#[derive(Debug, Default)]
struct RegistryInner {
    /// Processes keyed by name.
    processes: HashMap<String, ProcessHandle>,
    /// Next ID to assign.
    next_id: u32,
}

impl ProcessRegistry {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(RegistryInner::default())),
        }
    }

    /// Register a new process. Returns the assigned numeric ID.
    pub fn register(&self, name: String, config: AppConfig) -> u32 {
        let mut inner = self.inner.write().expect("registry lock poisoned");
        let id = inner.next_id;
        inner.next_id += 1;

        let now = unix_now();
        let cancel = tokio_util::sync::CancellationToken::new();

        inner.processes.insert(
            name.clone(),
            ProcessHandle {
                id,
                name,
                config,
                state: ProcessState::Starting,
                restart_count: 0,
                registered_at: now,
                started_at: None,
                cpu_pct: None,
                memory_bytes: None,
                supervisor_cancel: cancel,
            },
        );

        id
    }

    /// Update the state of a named process.
    /// Returns false if the process was not found.
    pub fn set_state(&self, name: &str, state: ProcessState) -> bool {
        let mut inner = self.inner.write().expect("registry lock poisoned");
        match inner.processes.get_mut(name) {
            Some(handle) => {
                if matches!(state, ProcessState::Running { .. }) {
                    handle.started_at = Some(unix_now());
                }
                handle.state = state;
                true
            }
            None => false,
        }
    }

    /// Increment the restart counter for a named process.
    pub fn increment_restarts(&self, name: &str) {
        let mut inner = self.inner.write().expect("registry lock poisoned");
        if let Some(handle) = inner.processes.get_mut(name) {
            handle.restart_count += 1;
        }
    }

    /// Reset the restart counter for a named process to zero.
    pub fn reset_restart_count(&self, name: &str) {
        let mut inner = self.inner.write().expect("registry lock poisoned");
        if let Some(handle) = inner.processes.get_mut(name) {
            handle.restart_count = 0;
        }
    }

    /// Update CPU and memory metrics for a named process.
    pub fn update_metrics(&self, name: &str, cpu_pct: f32, memory_bytes: u64) {
        let mut inner = self.inner.write().expect("registry lock poisoned");
        if let Some(handle) = inner.processes.get_mut(name) {
            handle.cpu_pct = Some(cpu_pct);
            handle.memory_bytes = Some(memory_bytes);
        }
    }

    /// Retrieve a snapshot of all processes, sorted by ID ascending.
    pub fn list(&self) -> Vec<ProcessInfo> {
        let inner = self.inner.read().expect("registry lock poisoned");
        let mut procs: Vec<ProcessInfo> = inner.processes.values().map(|h| h.to_info()).collect();
        procs.sort_by_key(|p| p.id);
        procs
    }

    /// Retrieve a snapshot of a single process by name.
    pub fn get(&self, name: &str) -> Option<ProcessInfo> {
        let inner = self.inner.read().expect("registry lock poisoned");
        inner.processes.get(name).map(|h| h.to_info())
    }

    /// Retrieve the current ProcessState of a named process.
    pub fn get_state(&self, name: &str) -> Option<ProcessState> {
        let inner = self.inner.read().expect("registry lock poisoned");
        inner.processes.get(name).map(|h| h.state.clone())
    }

    /// Returns the cancellation token for a named process's supervisor task.
    pub fn supervisor_cancel(&self, name: &str) -> Option<tokio_util::sync::CancellationToken> {
        let inner = self.inner.read().expect("registry lock poisoned");
        inner
            .processes
            .get(name)
            .map(|h| h.supervisor_cancel.clone())
    }

    /// Remove a process from the registry. Returns false if not found.
    pub fn remove(&self, name: &str) -> bool {
        let mut inner = self.inner.write().expect("registry lock poisoned");
        inner.processes.remove(name).is_some()
    }

    /// Retrieve the config of a named process.
    pub fn get_config(&self, name: &str) -> Option<AppConfig> {
        let inner = self.inner.read().expect("registry lock poisoned");
        inner.processes.get(name).map(|h| h.config.clone())
    }

    /// Reset the cancellation token for a process, returning the new token.
    pub fn reset_cancel_token(&self, name: &str) -> Option<tokio_util::sync::CancellationToken> {
        let mut inner = self.inner.write().expect("registry lock poisoned");
        if let Some(handle) = inner.processes.get_mut(name) {
            let new_token = tokio_util::sync::CancellationToken::new();
            handle.supervisor_cancel = new_token.clone();
            Some(new_token)
        } else {
            None
        }
    }

    /// Resolve target (name, numeric ID, cluster prefix, or "all") to matching process names.
    pub fn resolve_targets(&self, target: &str) -> Vec<String> {
        let inner = self.inner.read().expect("registry lock poisoned");
        if target == "all" {
            let mut all: Vec<_> = inner.processes.keys().cloned().collect();
            all.sort();
            return all;
        }

        // Direct exact match
        if inner.processes.contains_key(target) {
            return vec![target.to_string()];
        }

        // Numeric ID match
        if let Ok(id) = target.parse::<u32>() {
            for (name, handle) in &inner.processes {
                if handle.id == id {
                    return vec![name.clone()];
                }
            }
        }

        // Cluster prefix match: e.g. target "web" matches "web-0", "web-1", "web#0"
        let prefix_dash = format!("{}-", target);
        let prefix_hash = format!("{}#", target);
        let mut matches: Vec<String> = inner
            .processes
            .keys()
            .filter(|name| name.starts_with(&prefix_dash) || name.starts_with(&prefix_hash))
            .cloned()
            .collect();

        if !matches.is_empty() {
            matches.sort();
            return matches;
        }

        Vec::new()
    }

    /// Retrieve the OS PID for a process if it is currently running.
    pub fn get_pid(&self, name: &str) -> Option<u32> {
        let inner = self.inner.read().expect("registry lock poisoned");
        inner.processes.get(name).and_then(|h| match h.state {
            ProcessState::Running { pid } => Some(pid),
            _ => None,
        })
    }

    /// Resolve target (name or numeric ID) to the process name.
    pub fn find_name_by_id_or_name(&self, target: &str) -> Option<String> {
        let inner = self.inner.read().expect("registry lock poisoned");
        if inner.processes.contains_key(target) {
            return Some(target.to_string());
        }
        if let Ok(id) = target.parse::<u32>() {
            for (name, handle) in &inner.processes {
                if handle.id == id {
                    return Some(name.clone());
                }
            }
        }
        None
    }

    /// Returns true if a process with this name is registered.
    pub fn contains(&self, name: &str) -> bool {
        let inner = self.inner.read().expect("registry lock poisoned");
        inner.processes.contains_key(name)
    }

    /// Returns raw handle data for all processes: (name, config, restart_count, registered_at).
    /// Used by DaemonSave to persist the current process list.
    pub fn list_with_handles(&self) -> Vec<(String, orion_core::config::AppConfig, u32, u64)> {
        let inner = self.inner.read().expect("registry lock poisoned");
        let mut result: Vec<_> = inner
            .processes
            .values()
            .map(|h| {
                (
                    h.name.clone(),
                    h.config.clone(),
                    h.restart_count,
                    h.registered_at,
                )
            })
            .collect();
        // Sort by registration time to preserve original order.
        result.sort_by_key(|(_, _, _, registered_at)| *registered_at);
        result
    }
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
