use orion_core::process_state::ProcessInfo;
use orion_core::protocol::LogLine;

/// Main state holder for the interactive OLED black TUI monitor.
pub struct App {
    /// Currently tracked processes.
    pub processes: Vec<ProcessInfo>,
    /// Index of currently selected process.
    pub selected_index: usize,
    /// Recent log lines for the selected process.
    pub logs: Vec<LogLine>,
    /// Flag indicating whether the TUI should exit.
    pub should_quit: bool,
    /// Transient status message displayed in footer (e.g. "Restarted process 'api'").
    pub status_message: Option<String>,
}

impl App {
    pub fn new() -> Self {
        Self {
            processes: Vec::new(),
            selected_index: 0,
            logs: Vec::new(),
            should_quit: false,
            status_message: None,
        }
    }

    pub fn selected_process(&self) -> Option<&ProcessInfo> {
        self.processes.get(self.selected_index)
    }

    pub fn select_next(&mut self) {
        if !self.processes.is_empty() {
            self.selected_index = (self.selected_index + 1).min(self.processes.len() - 1);
        }
    }

    pub fn select_prev(&mut self) {
        if self.selected_index > 0 {
            self.selected_index -= 1;
        }
    }

    pub fn update_processes(&mut self, list: Vec<ProcessInfo>) {
        self.processes = list;
        if self.selected_index >= self.processes.len() && !self.processes.is_empty() {
            self.selected_index = self.processes.len() - 1;
        }
    }
}
