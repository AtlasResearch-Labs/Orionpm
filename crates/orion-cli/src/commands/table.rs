use orion_core::process_state::ProcessInfo;
use std::time::{SystemTime, UNIX_EPOCH};

/// Format and display a clean OLED black table of managed processes.
/// Strict rules: no emojis, no pulsing dots, no color badges, clean borders.
pub fn print_process_table(processes: &[ProcessInfo], full_cmd: bool) {
    if processes.is_empty() {
        println!(
            "No managed processes found. Run 'orion start <script|config>' to start a process."
        );
        return;
    }

    let mut rows: Vec<[String; 9]> = Vec::new();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    for p in processes {
        let uptime_str = match (p.started_at, p.state.is_active()) {
            (Some(started), true) if now >= started => format_duration(now - started),
            _ => "-".to_string(),
        };

        let cpu_str = match p.cpu_pct {
            Some(cpu) => format!("{:.1}%", cpu),
            None => "-".to_string(),
        };

        let cmd_str = if full_cmd || p.command.len() <= 28 {
            p.command.clone()
        } else {
            format!("{}...", &p.command[..25])
        };

        rows.push([
            p.id.to_string(),
            p.name.clone(),
            p.state.label().to_string(),
            p.mode.clone(),
            cpu_str,
            p.memory_display(),
            uptime_str,
            p.restart_count.to_string(),
            cmd_str,
        ]);
    }

    let headers = [
        "ID", "NAME", "STATUS", "MODE", "CPU", "MEM", "UPTIME", "RESTARTS", "COMMAND",
    ];
    let mut widths = [0usize; 9];
    for (i, h) in headers.iter().enumerate() {
        widths[i] = h.len();
    }
    for row in &rows {
        for (i, col) in row.iter().enumerate() {
            if col.len() > widths[i] {
                widths[i] = col.len();
            }
        }
    }

    print_border(&widths, "┌", "┬", "┐");
    print_row(&headers.map(|s| s.to_string()), &widths);
    print_border(&widths, "├", "┼", "┤");
    for row in &rows {
        print_row(row, &widths);
    }
    print_border(&widths, "└", "┴", "┘");
}

fn print_row(cols: &[String; 9], widths: &[usize; 9]) {
    print!("│");
    for (i, col) in cols.iter().enumerate() {
        print!(" {:<width$} │", col, width = widths[i]);
    }
    println!();
}

fn print_border(widths: &[usize; 9], left: &str, mid: &str, right: &str) {
    print!("{}", left);
    for (i, w) in widths.iter().enumerate() {
        print!("{:─<width$}", "", width = w + 2);
        if i + 1 < widths.len() {
            print!("{}", mid);
        }
    }
    println!("{}", right);
}

fn format_duration(secs: u64) -> String {
    if secs < 60 {
        format!("{}s", secs)
    } else if secs < 3600 {
        format!("{}m {}s", secs / 60, secs % 60)
    } else if secs < 86400 {
        format!("{}h {}m", secs / 3600, (secs % 3600) / 60)
    } else {
        format!("{}d {}h", secs / 86400, (secs % 86400) / 3600)
    }
}
