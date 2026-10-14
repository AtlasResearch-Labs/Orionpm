use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table};
use ratatui::Frame;
use std::time::{SystemTime, UNIX_EPOCH};

use super::app::App;
use orion_core::protocol::LogStream;

pub fn render(f: &mut Frame, app: &App) {
    let size = f.size();

    // Background fill (OLED Black)
    let bg_block = Block::default().style(Style::default().bg(Color::Rgb(0, 0, 0)));
    f.render_widget(bg_block, size);

    // Vertical layout: Header (3), Process Table (flex), Logs (flex), Footer (1)
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Percentage(45),
            Constraint::Percentage(50),
            Constraint::Length(1),
        ])
        .split(size);

    // 1. Header
    render_header(f, chunks[0]);

    // 2. Processes Table
    render_processes_table(f, app, chunks[1]);

    // 3. Logs View
    render_logs_view(f, app, chunks[2]);

    // 4. Footer Status Bar
    render_footer(f, app, chunks[3]);
}

fn render_header(f: &mut Frame, area: ratatui::layout::Rect) {
    let header_text = vec![Line::from(vec![
        Span::styled(
            "orion MONITOR",
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "  —  Atlas Research Labs",
            Style::default().fg(Color::Rgb(120, 120, 120)),
        ),
    ])];

    let header_widget = Paragraph::new(header_text).block(
        Block::default()
            .borders(Borders::BOTTOM)
            .border_style(Style::default().fg(Color::Rgb(40, 40, 40))),
    );
    f.render_widget(header_widget, area);
}

fn render_processes_table(f: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let header_cells = [
        "ID", "NAME", "STATUS", "CPU", "MEMORY", "UPTIME", "RESTARTS", "MODE",
    ]
    .iter()
    .map(|h| {
        Cell::from(*h).style(
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )
    });
    let header_row = Row::new(header_cells)
        .style(Style::default().bg(Color::Rgb(20, 20, 20)))
        .height(1);

    let rows: Vec<Row> = app
        .processes
        .iter()
        .enumerate()
        .map(|(idx, p)| {
            let uptime = match (p.started_at, p.state.is_active()) {
                (Some(s), true) if now >= s => format_duration(now - s),
                _ => "-".to_string(),
            };

            let cpu = match p.cpu_pct {
                Some(c) => format!("{:.1}%", c),
                None => "-".to_string(),
            };

            let is_selected = idx == app.selected_index;
            let row_style = if is_selected {
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::White)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Rgb(200, 200, 200))
            };

            Row::new(vec![
                Cell::from(p.id.to_string()),
                Cell::from(p.name.clone()),
                Cell::from(p.state.label()),
                Cell::from(cpu),
                Cell::from(p.memory_display()),
                Cell::from(uptime),
                Cell::from(p.restart_count.to_string()),
                Cell::from(p.mode.clone()),
            ])
            .style(row_style)
        })
        .collect();

    let widths = [
        Constraint::Length(5),
        Constraint::Length(18),
        Constraint::Length(12),
        Constraint::Length(8),
        Constraint::Length(12),
        Constraint::Length(10),
        Constraint::Length(10),
        Constraint::Length(8),
    ];

    let table = Table::new(rows, widths)
        .header(header_row)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Managed Processes ")
                .title_style(Style::default().fg(Color::Rgb(160, 160, 160)))
                .border_style(Style::default().fg(Color::Rgb(50, 50, 50))),
        );

    f.render_widget(table, area);
}

fn render_logs_view(f: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let title = if let Some(proc) = app.selected_process() {
        format!(" Logs: {} ", proc.name)
    } else {
        " Logs ".to_string()
    };

    let log_lines: Vec<Line> = if app.logs.is_empty() {
        vec![Line::from(Span::styled(
            "No log output available for selected process.",
            Style::default().fg(Color::Rgb(100, 100, 100)),
        ))]
    } else {
        app.logs
            .iter()
            .map(|l| {
                let stream_tag = match l.stream {
                    LogStream::Stdout => "out",
                    LogStream::Stderr => "err",
                };
                let secs = l.timestamp % 86400;
                let ts = format!("{:02}:{:02}:{:02}", secs / 3600, (secs % 3600) / 60, secs % 60);

                Line::from(vec![
                    Span::styled(format!("[{}] ", ts), Style::default().fg(Color::Rgb(90, 90, 90))),
                    Span::styled(format!("[{}] ", stream_tag), Style::default().fg(Color::Rgb(130, 130, 130))),
                    Span::styled(&l.text, Style::default().fg(Color::Rgb(210, 210, 210))),
                ])
            })
            .collect()
    };

    let logs_widget = Paragraph::new(log_lines).block(
        Block::default()
            .borders(Borders::ALL)
            .title(title)
            .title_style(Style::default().fg(Color::Rgb(160, 160, 160)))
            .border_style(Style::default().fg(Color::Rgb(50, 50, 50))),
    );

    f.render_widget(logs_widget, area);
}

fn render_footer(f: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let status_text = app
        .status_message
        .as_deref()
        .unwrap_or("q: quit | j/k: navigate | r: restart | s: stop");

    let footer = Paragraph::new(Line::from(Span::styled(
        status_text,
        Style::default().fg(Color::Rgb(120, 120, 120)),
    )));

    f.render_widget(footer, area);
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
