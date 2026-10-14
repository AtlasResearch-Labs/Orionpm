pub mod app;
pub mod render;

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use orion_core::protocol::{EmptyParams, LogsParams, Method, NameParams, Params, ResponseResult};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::io::stdout;
use std::time::{Duration, Instant};

use crate::commands::client;
use app::App;

/// Launch the interactive OLED black terminal monitor.
pub async fn run() -> Result<()> {
    client::ensure_daemon().await?;

    enable_raw_mode()?;
    let mut stdout = stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new();
    let mut last_poll = Instant::now() - Duration::from_secs(10);

    let res = run_loop(&mut terminal, &mut app, &mut last_poll).await;

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    res
}

async fn run_loop<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
    last_poll: &mut Instant,
) -> Result<()> {
    loop {
        // Poll daemon for live metrics and status
        if last_poll.elapsed() >= Duration::from_millis(1000) {
            *last_poll = Instant::now();
            if let Ok(ResponseResult::ProcessList { processes }) =
                client::rpc(Method::ProcessList, Params::ProcessList(EmptyParams {})).await
            {
                app.update_processes(processes);
            }

            if let Some(proc) = app.selected_process() {
                let name = proc.name.clone();
                if let Ok(ResponseResult::Logs { lines, .. }) = client::rpc(
                    Method::ProcessLogs,
                    Params::ProcessLogs(LogsParams {
                        name,
                        lines: 30,
                        follow: false,
                    }),
                )
                .await
                {
                    app.logs = lines;
                }
            }
        }

        terminal.draw(|f| render::render(f, app))?;

        if event::poll(Duration::from_millis(200))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    match key.code {
                        KeyCode::Char('q') | KeyCode::Esc => break,
                        KeyCode::Down | KeyCode::Char('j') => {
                            app.select_next();
                            *last_poll = Instant::now() - Duration::from_secs(10);
                        }
                        KeyCode::Up | KeyCode::Char('k') => {
                            app.select_prev();
                            *last_poll = Instant::now() - Duration::from_secs(10);
                        }
                        KeyCode::Char('r') => {
                            if let Some(proc) = app.selected_process() {
                                let name = proc.name.clone();
                                let _ = client::rpc(
                                    Method::ProcessRestart,
                                    Params::ProcessRestart(NameParams { name: name.clone() }),
                                )
                                .await;
                                app.status_message = Some(format!("Restarted process '{}'", name));
                                *last_poll = Instant::now() - Duration::from_secs(10);
                            }
                        }
                        KeyCode::Char('s') => {
                            if let Some(proc) = app.selected_process() {
                                let name = proc.name.clone();
                                let _ = client::rpc(
                                    Method::ProcessStop,
                                    Params::ProcessStop(NameParams { name: name.clone() }),
                                )
                                .await;
                                app.status_message = Some(format!("Stopped process '{}'", name));
                                *last_poll = Instant::now() - Duration::from_secs(10);
                            }
                        }
                        _ => {}
                    }
                }
            }
        }

        if app.should_quit {
            break;
        }
    }

    Ok(())
}
