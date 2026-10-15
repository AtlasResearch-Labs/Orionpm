use anyhow::{Context, Result};
use clap::Args;
use orion_core::config::{AppConfig, ExecutionMode, RestartPolicy};
use orion_core::process_state::ProcessInfo;
use orion_core::protocol::{
    EmptyParams, LogsParams, Method, NameParams, Params, ResponseResult, ScaleParams, StartParams,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;
use sysinfo::{CpuRefreshKind, MemoryRefreshKind, RefreshKind, System};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::sleep;

use super::client;

const DASHBOARD_HTML: &str = include_str!("dashboard.html");

#[derive(Args, Debug, Clone)]
pub struct WebArgs {
    /// Port to bind the web dashboard server to.
    #[arg(short, long, default_value = "9000")]
    pub port: u16,

    /// Host interface to bind to.
    #[arg(short = 'H', long, default_value = "127.0.0.1")]
    pub host: String,

    /// Do not automatically open the browser.
    #[arg(long)]
    pub no_open: bool,
}

#[derive(Debug, Serialize, Deserialize)]
struct SystemStats {
    pub cpu_pct: f32,
    pub memory_used_mb: u64,
    pub memory_total_mb: u64,
    pub uptime: String,
    pub os: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct StatusPayload {
    pub processes: Vec<ProcessInfo>,
    pub system: SystemStats,
}

#[derive(Debug, Deserialize)]
struct StartRequest {
    pub script: String,
    pub name: Option<String>,
    pub instances: Option<u32>,
    pub cwd: Option<String>,
    pub watch: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct ScaleRequest {
    pub count: usize,
}

pub async fn run(args: WebArgs) -> Result<()> {
    // 1. Ensure oriond daemon is online
    client::ensure_daemon()
        .await
        .context("Failed to connect to oriond daemon")?;

    let addr = format!("{}:{}", args.host, args.port);
    let listener = TcpListener::bind(&addr)
        .await
        .with_context(|| format!("Failed to bind web dashboard to {}", addr))?;

    println!("\n  ___  ____  ___ ___  _   _ ");
    println!(" / _ \\|  _ \\|_ _/ _ \\| \\ | |");
    println!("| | | | |_) || | | | |  \\| |");
    println!("| |_| |  _ < | | |_| | |\\  |");
    println!(" \\___/|_| \\_\\___\\___/|_| \\_|\n");
    println!(" Orion Web Dashboard v{}", env!("CARGO_PKG_VERSION"));
    println!(" Built by Atlas Research Labs. Built for the World.\n");
    println!(" ⚡ Dashboard:  http://{}", addr);
    println!(" 📡 Realtime:   Server-Sent Events (SSE)");
    println!(" 🔒 Daemon:     Connected to oriond\n");
    println!(" Press Ctrl+C to stop.\n");

    if !args.no_open {
        let url = format!("http://{}", addr);
        tokio::spawn(async move {
            sleep(Duration::from_millis(300)).await;
            #[cfg(windows)]
            let _ = std::process::Command::new("cmd")
                .args(["/C", "start", &url])
                .spawn();
            #[cfg(target_os = "macos")]
            let _ = std::process::Command::new("open").arg(&url).spawn();
            #[cfg(target_os = "linux")]
            let _ = std::process::Command::new("xdg-open").arg(&url).spawn();
        });
    }

    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                tokio::spawn(async move {
                    if let Err(e) = handle_connection(stream).await {
                        tracing::debug!("Connection error: {:#}", e);
                    }
                });
            }
            Err(e) => {
                tracing::warn!("Accept failed: {}", e);
                sleep(Duration::from_millis(100)).await;
            }
        }
    }
}

async fn handle_connection(mut stream: TcpStream) -> Result<()> {
    let mut buf = [0u8; 8192];
    let n = stream.read(&mut buf).await?;
    if n == 0 {
        return Ok(());
    }

    let request_str = String::from_utf8_lossy(&buf[..n]);
    let mut lines = request_str.lines();
    let req_line = lines.next().unwrap_or_default();
    let mut parts = req_line.split_whitespace();
    let method = parts.next().unwrap_or("GET");
    let full_path = parts.next().unwrap_or("/");

    let (path, query) = match full_path.split_once('?') {
        Some((p, q)) => (p, q),
        None => (full_path, ""),
    };

    // Extract body if any
    let body = match request_str.find("\r\n\r\n") {
        Some(idx) => &request_str[idx + 4..],
        None => "",
    };

    // Route requests
    match (method, path) {
        ("OPTIONS", _) => {
            let res = "HTTP/1.1 204 No Content\r\n\
                       Access-Control-Allow-Origin: *\r\n\
                       Access-Control-Allow-Methods: GET, POST, OPTIONS\r\n\
                       Access-Control-Allow-Headers: Content-Type\r\n\
                       Connection: close\r\n\r\n";
            stream.write_all(res.as_bytes()).await?;
        }
        ("GET", "/") | ("GET", "/index.html") | ("HEAD", "/") | ("HEAD", "/index.html") => {
            let body_content = if method == "HEAD" { "" } else { DASHBOARD_HTML };
            let res = format!(
                "HTTP/1.1 200 OK\r\n\
                 Content-Type: text/html; charset=utf-8\r\n\
                 Content-Length: {}\r\n\
                 Connection: close\r\n\r\n{}",
                DASHBOARD_HTML.len(),
                body_content
            );
            stream.write_all(res.as_bytes()).await?;
        }
        ("GET", "/api/status") | ("GET", "/api/processes") => {
            let payload = get_status_payload().await?;
            let json = serde_json::to_string(&payload)?;
            send_json(&mut stream, 200, &json).await?;
        }
        ("GET", "/api/events") => {
            handle_sse_stream(stream).await?;
        }
        ("POST", "/api/processes/start") => {
            let req: StartRequest = serde_json::from_str(body).unwrap_or(StartRequest {
                script: "".to_string(),
                name: None,
                instances: None,
                cwd: None,
                watch: None,
            });

            if req.script.is_empty() {
                send_json(
                    &mut stream,
                    400,
                    r#"{"ok":false,"error":"Script or command is required"}"#,
                )
                .await?;
                return Ok(());
            }

            let (cmd, args) = if req.script.ends_with(".js") {
                ("node".to_string(), vec![req.script.clone()])
            } else if req.script.ends_with(".py") {
                let py = if cfg!(windows) { "python" } else { "python3" };
                (py.to_string(), vec![req.script.clone()])
            } else if req.script.ends_with(".sh") {
                ("bash".to_string(), vec![req.script.clone()])
            } else {
                (req.script.clone(), vec![])
            };

            let app_name = req.name.unwrap_or_else(|| {
                std::path::Path::new(&req.script)
                    .file_stem()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_else(|| "app".to_string())
            });

            let app_config = AppConfig {
                command: cmd,
                args,
                cwd: req.cwd.map(PathBuf::from),
                name: Some(app_name.clone()),
                instances: req.instances.unwrap_or(1),
                mode: ExecutionMode::Fork,
                env: HashMap::new(),
                restart_policy: RestartPolicy::Always,
                restart_delay_ms: 1000,
                max_restarts: 10,
                restart_window_secs: 60,
                max_memory_mb: 0,
                max_cpu_pct: 0.0,
                watch: req.watch.unwrap_or(false),
                watch_paths: vec![],
                watch_ignore: vec![
                    "node_modules".into(),
                    "target".into(),
                    ".git".into(),
                    "*.log".into(),
                ],
                stdout_log: None,
                stderr_log: None,
                log_lines: 1000,
                health: None,
                alerts: Default::default(),
                smtp: None,
            };

            match client::rpc(
                Method::ProcessStart,
                Params::ProcessStart(StartParams {
                    config: app_config,
                    name: app_name,
                }),
            )
            .await
            {
                Ok(_) => send_json(&mut stream, 200, r#"{"ok":true}"#).await?,
                Err(e) => {
                    let err = serde_json::json!({ "ok": false, "error": e.to_string() });
                    send_json(&mut stream, 500, &err.to_string()).await?;
                }
            }
        }
        ("POST", p) if p.starts_with("/api/processes/") => {
            let tail = &p["/api/processes/".len()..];
            let segments: Vec<&str> = tail.split('/').collect();

            if segments.len() == 2 {
                let name = urlencoding_decode(segments[0]);
                let action = segments[1];

                let res = match action {
                    "restart" => {
                        client::rpc(
                            Method::ProcessRestart,
                            Params::ProcessRestart(NameParams { name: name.clone() }),
                        )
                        .await
                    }
                    "reload" => {
                        client::rpc(
                            Method::ProcessReload,
                            Params::ProcessReload(NameParams { name: name.clone() }),
                        )
                        .await
                    }
                    "stop" => {
                        client::rpc(
                            Method::ProcessStop,
                            Params::ProcessStop(NameParams { name: name.clone() }),
                        )
                        .await
                    }
                    "delete" => {
                        client::rpc(
                            Method::ProcessDelete,
                            Params::ProcessDelete(NameParams { name: name.clone() }),
                        )
                        .await
                    }
                    "flush" => {
                        client::rpc(
                            Method::ProcessFlush,
                            Params::ProcessFlush(NameParams { name: name.clone() }),
                        )
                        .await
                    }
                    "scale" => {
                        let scale_req: ScaleRequest =
                            serde_json::from_str(body).unwrap_or(ScaleRequest { count: 1 });
                        client::rpc(
                            Method::ProcessScale,
                            Params::ProcessScale(ScaleParams {
                                target: name.clone(),
                                count: scale_req.count.to_string(),
                            }),
                        )
                        .await
                    }
                    _ => {
                        send_json(&mut stream, 404, r#"{"ok":false,"error":"Unknown action"}"#)
                            .await?;
                        return Ok(());
                    }
                };

                match res {
                    Ok(_) => send_json(&mut stream, 200, r#"{"ok":true}"#).await?,
                    Err(e) => {
                        let err = serde_json::json!({ "ok": false, "error": e.to_string() });
                        send_json(&mut stream, 500, &err.to_string()).await?;
                    }
                }
            } else {
                send_json(&mut stream, 404, r#"{"ok":false,"error":"Not Found"}"#).await?;
            }
        }
        ("GET", p) if p.starts_with("/api/processes/") && p.ends_with("/logs") => {
            let tail = &p["/api/processes/".len()..p.len() - "/logs".len()];
            let name = urlencoding_decode(tail);

            let lines_count: usize = query
                .split('&')
                .find_map(|pair| {
                    let (k, v) = pair.split_once('=')?;
                    if k == "lines" {
                        v.parse().ok()
                    } else {
                        None
                    }
                })
                .unwrap_or(100);

            match client::rpc(
                Method::ProcessLogs,
                Params::ProcessLogs(LogsParams {
                    name,
                    lines: lines_count,
                    follow: false,
                }),
            )
            .await
            {
                Ok(ResponseResult::Logs { lines, .. }) => {
                    let json = serde_json::json!({ "lines": lines });
                    send_json(&mut stream, 200, &json.to_string()).await?;
                }
                Ok(_) => {
                    send_json(&mut stream, 200, r#"{"lines":[]}"#).await?;
                }
                Err(e) => {
                    let err = serde_json::json!({ "ok": false, "error": e.to_string() });
                    send_json(&mut stream, 500, &err.to_string()).await?;
                }
            }
        }
        _ => {
            let not_found =
                "HTTP/1.1 404 Not Found\r\nContent-Length: 9\r\nConnection: close\r\n\r\nNot Found";
            stream.write_all(not_found.as_bytes()).await?;
        }
    }

    Ok(())
}

async fn handle_sse_stream(mut stream: TcpStream) -> Result<()> {
    let header = "HTTP/1.1 200 OK\r\n\
                  Content-Type: text/event-stream\r\n\
                  Cache-Control: no-cache\r\n\
                  Connection: keep-alive\r\n\
                  Access-Control-Allow-Origin: *\r\n\r\n";
    stream.write_all(header.as_bytes()).await?;

    let mut system_tracker = System::new_with_specifics(
        RefreshKind::new()
            .with_cpu(CpuRefreshKind::everything())
            .with_memory(MemoryRefreshKind::everything()),
    );

    loop {
        system_tracker.refresh_cpu();
        system_tracker.refresh_memory();

        let processes =
            match client::rpc(Method::ProcessList, Params::ProcessList(EmptyParams {})).await {
                Ok(ResponseResult::ProcessList { processes }) => processes,
                _ => vec![],
            };

        let cpu_pct = system_tracker.global_cpu_info().cpu_usage();
        let memory_total_mb = system_tracker.total_memory() / (1024 * 1024);
        let memory_used_mb = system_tracker.used_memory() / (1024 * 1024);
        let uptime_secs = System::uptime();
        let uptime_str = format!("{}h {}m", uptime_secs / 3600, (uptime_secs % 3600) / 60);

        let os_str = format!(
            "{} {}",
            System::name().unwrap_or_else(|| "Unknown".to_string()),
            System::cpu_arch().unwrap_or_default()
        );

        let payload = StatusPayload {
            processes,
            system: SystemStats {
                cpu_pct,
                memory_used_mb,
                memory_total_mb,
                uptime: uptime_str,
                os: os_str,
            },
        };

        let json = serde_json::to_string(&payload).unwrap_or_default();
        let sse_msg = format!("data: {}\n\n", json);

        if stream.write_all(sse_msg.as_bytes()).await.is_err() {
            // Client closed connection
            break;
        }

        sleep(Duration::from_millis(1000)).await;
    }

    Ok(())
}

async fn get_status_payload() -> Result<StatusPayload> {
    let processes =
        match client::rpc(Method::ProcessList, Params::ProcessList(EmptyParams {})).await {
            Ok(ResponseResult::ProcessList { processes }) => processes,
            _ => vec![],
        };

    let mut sys = System::new_with_specifics(
        RefreshKind::new()
            .with_cpu(CpuRefreshKind::everything())
            .with_memory(MemoryRefreshKind::everything()),
    );
    sys.refresh_cpu();
    sys.refresh_memory();

    let cpu_pct = sys.global_cpu_info().cpu_usage();
    let memory_total_mb = sys.total_memory() / (1024 * 1024);
    let memory_used_mb = sys.used_memory() / (1024 * 1024);
    let uptime_secs = System::uptime();
    let uptime = format!("{}h {}m", uptime_secs / 3600, (uptime_secs % 3600) / 60);
    let os = format!(
        "{} {}",
        System::name().unwrap_or_else(|| "Unknown".to_string()),
        System::cpu_arch().unwrap_or_default()
    );

    Ok(StatusPayload {
        processes,
        system: SystemStats {
            cpu_pct,
            memory_used_mb,
            memory_total_mb,
            uptime,
            os,
        },
    })
}

async fn send_json(stream: &mut TcpStream, status: u16, json: &str) -> Result<()> {
    let status_line = match status {
        200 => "200 OK",
        400 => "400 Bad Request",
        404 => "404 Not Found",
        500 => "500 Internal Server Error",
        _ => "200 OK",
    };

    let res = format!(
        "HTTP/1.1 {}\r\n\
         Content-Type: application/json\r\n\
         Access-Control-Allow-Origin: *\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\r\n{}",
        status_line,
        json.len(),
        json
    );

    stream.write_all(res.as_bytes()).await?;
    Ok(())
}

fn urlencoding_decode(val: &str) -> String {
    let mut out = String::new();
    let mut bytes = val.bytes();
    while let Some(b) = bytes.next() {
        if b == b'%' {
            let h1 = bytes.next().unwrap_or(b'0');
            let h2 = bytes.next().unwrap_or(b'0');
            if let Ok(num) = u8::from_str_radix(&format!("{}{}", h1 as char, h2 as char), 16) {
                out.push(num as char);
            }
        } else if b == b'+' {
            out.push(' ');
        } else {
            out.push(b as char);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_urlencoding_decode() {
        assert_eq!(urlencoding_decode("my-app"), "my-app");
        assert_eq!(urlencoding_decode("my%20app"), "my app");
        assert_eq!(urlencoding_decode("my+app"), "my app");
        assert_eq!(urlencoding_decode("app%2Fservice"), "app/service");
    }
}
