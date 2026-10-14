use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use orion_core::config::{AppConfig, HealthConfig, RestartPolicy};
use orion_core::process_state::ProcessState;
use orion_core::protocol::{LogLine, LogStream};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::time::sleep;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use crate::logger::LogBus;
use crate::registry::ProcessRegistry;

/// Spawn a dedicated supervisor task for a managed process.
/// This task manages the process lifecycle, restarts, file watching,
/// stdout/stderr streaming, log file mirroring, and graceful shutdown.
pub fn spawn(
    name: String,
    config: AppConfig,
    registry: ProcessRegistry,
    logger: LogBus,
    cancel_token: CancellationToken,
) {
    tokio::spawn(async move {
        run_supervisor_loop(name, config, registry, logger, cancel_token).await;
    });
}

async fn run_supervisor_loop(
    name: String,
    config: AppConfig,
    registry: ProcessRegistry,
    logger: LogBus,
    cancel_token: CancellationToken,
) {
    info!(name = %name, "Supervisor task started");
    let mut restart_count = 0;
    let mut restart_timestamps: Vec<u64> = Vec::new();

    // Initialize file watcher channel if watch mode is enabled
    let mut watch_rx = if config.watch {
        let (tx, rx) = tokio::sync::mpsc::channel(32);
        let watch_paths = config.watch_paths.clone();
        let watch_ignore = config.watch_ignore.clone();
        let watcher_cancel = cancel_token.clone();
        let app_name = name.clone();
        tokio::spawn(async move {
            run_file_watcher(app_name, watch_paths, watch_ignore, tx, watcher_cancel).await;
        });
        Some(rx)
    } else {
        None
    };

    loop {
        // Check if supervisor was cancelled before starting
        if cancel_token.is_cancelled() {
            info!(name = %name, "Supervisor cancelled before process start");
            registry.set_state(&name, ProcessState::Stopped);
            break;
        }

        registry.set_state(&name, ProcessState::Starting);

        // Prepare Command
        let mut cmd = Command::new(&config.command);
        cmd.args(&config.args);
        if let Some(ref cwd) = config.cwd {
            cmd.current_dir(cwd);
        }
        for (k, v) in &config.env {
            cmd.env(k, v);
        }

        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        // Under Windows, ensure process can be cleanly terminated without popping a console window
        #[cfg(windows)]
        {
            // CREATE_NO_WINDOW = 0x08000000
            cmd.creation_flags(0x08000000);
        }

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                error!(name = %name, error = %e, "Failed to spawn process");
                let error_msg = format!("Failed to spawn executable '{}': {}", config.command, e);
                registry.set_state(
                    &name,
                    ProcessState::Errored {
                        exit_code: None,
                        message: error_msg.clone(),
                    },
                );
                logger.log(
                    &name,
                    LogLine {
                        timestamp: unix_now(),
                        stream: LogStream::Stderr,
                        text: error_msg,
                    },
                );

                // If watch is active, allow file changes to re-trigger spawn attempts
                if config.watch && watch_rx.is_some() {
                    tokio::select! {
                        _ = cancel_token.cancelled() => break,
                        Some(changed_path) = async {
                            match watch_rx.as_mut() {
                                Some(rx) => rx.recv().await,
                                None => std::future::pending().await,
                            }
                        } => {
                            logger.log(
                                &name,
                                LogLine {
                                    timestamp: unix_now(),
                                    stream: LogStream::Stdout,
                                    text: format!("[orion] File change detected: {}. Retrying spawn...", changed_path.display()),
                                },
                            );
                            restart_count = 0;
                            continue;
                        }
                    }
                }

                // Check restart policy on spawn failure
                if config.restart_policy == RestartPolicy::Never {
                    break;
                }

                restart_count += 1;
                if config.max_restarts > 0 && restart_count >= config.max_restarts {
                    warn!(name = %name, "Max restarts reached on spawn failure");
                    break;
                }

                tokio::select! {
                    _ = sleep(Duration::from_millis(config.restart_delay_ms)) => continue,
                    _ = cancel_token.cancelled() => break,
                }
            }
        };

        let pid = child.id().unwrap_or(0);
        info!(name = %name, pid = pid, "Process is running");
        registry.set_state(&name, ProcessState::Running { pid });

        // Stream stdout (mirrored to file if configured, and written to SQLite logger)
        if let Some(stdout) = child.stdout.take() {
            let logger_clone = logger.clone();
            let name_clone = name.clone();
            let stdout_log_path = config.stdout_log.clone();
            tokio::spawn(async move {
                let mut file_sink = if let Some(ref path) = stdout_log_path {
                    if let Some(parent) = path.parent() {
                        let _ = tokio::fs::create_dir_all(parent).await;
                    }
                    tokio::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(path)
                        .await
                        .ok()
                } else {
                    None
                };

                let mut reader = BufReader::new(stdout).lines();
                while let Ok(Some(line)) = reader.next_line().await {
                    if let Some(ref mut f) = file_sink {
                        let _ = f.write_all(line.as_bytes()).await;
                        let _ = f.write_all(b"\n").await;
                        let _ = f.flush().await;
                    }

                    logger_clone.log(
                        &name_clone,
                        LogLine {
                            timestamp: unix_now(),
                            stream: LogStream::Stdout,
                            text: line,
                        },
                    );
                }
            });
        }

        // Stream stderr (mirrored to file if configured, and written to SQLite logger)
        if let Some(stderr) = child.stderr.take() {
            let logger_clone = logger.clone();
            let name_clone = name.clone();
            let stderr_log_path = config.stderr_log.clone();
            tokio::spawn(async move {
                let mut file_sink = if let Some(ref path) = stderr_log_path {
                    if let Some(parent) = path.parent() {
                        let _ = tokio::fs::create_dir_all(parent).await;
                    }
                    tokio::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(path)
                        .await
                        .ok()
                } else {
                    None
                };

                let mut reader = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = reader.next_line().await {
                    if let Some(ref mut f) = file_sink {
                        let _ = f.write_all(line.as_bytes()).await;
                        let _ = f.write_all(b"\n").await;
                        let _ = f.flush().await;
                    }

                    logger_clone.log(
                        &name_clone,
                        LogLine {
                            timestamp: unix_now(),
                            stream: LogStream::Stderr,
                            text: line,
                        },
                    );
                }
            });
        }

        // Initialize health probe if configured
        let probe_cancel = CancellationToken::new();
        let (unhealthy_tx, mut unhealthy_rx) = tokio::sync::mpsc::channel(1);
        if let Some(ref health) = config.health {
            let h = health.clone();
            let tx = unhealthy_tx.clone();
            let cancel = probe_cancel.clone();
            tokio::spawn(async move {
                run_health_probe(h, tx, cancel).await;
            });
        }

        // Wait for process exit OR cancellation OR file change (when watch is enabled) OR health failure
        let mut file_change_triggered = false;
        let mut health_failure_triggered = false;
        let exit_status = tokio::select! {
            res = child.wait() => {
                match res {
                    Ok(status) => Some(status),
                    Err(e) => {
                        error!(name = %name, error = %e, "Error waiting for child process");
                        None
                    }
                }
            }
            _ = cancel_token.cancelled() => {
                probe_cancel.cancel();
                info!(name = %name, pid = pid, "Received stop signal, terminating process");
                registry.set_state(&name, ProcessState::Stopping);

                // Gracefully kill process
                let _ = child.kill().await;
                registry.set_state(&name, ProcessState::Stopped);
                info!(name = %name, "Process stopped gracefully");
                break;
            }
            Some(changed_path) = async {
                match watch_rx.as_mut() {
                    Some(rx) => rx.recv().await,
                    None => std::future::pending().await,
                }
            } => {
                probe_cancel.cancel();
                info!(name = %name, path = %changed_path.display(), "File change detected; restarting child");
                logger.log(
                    &name,
                    LogLine {
                        timestamp: unix_now(),
                        stream: LogStream::Stdout,
                        text: format!("[orion] File change detected: {}. Restarting...", changed_path.display()),
                    },
                );
                let _ = child.kill().await;
                let _ = child.wait().await;
                file_change_triggered = true;
                None
            }
            _ = unhealthy_rx.recv() => {
                probe_cancel.cancel();
                let failures = config.health.as_ref().map(|h| h.max_failures).unwrap_or(3);
                let err_msg = format!("[orion] Health check failed for '{}' ({} consecutive probe failures). Triggering automated recovery restart...", name, failures);
                warn!(name = %name, "{}", err_msg);
                logger.log(
                    &name,
                    LogLine {
                        timestamp: unix_now(),
                        stream: LogStream::Stderr,
                        text: err_msg,
                    },
                );
                let _ = child.kill().await;
                let _ = child.wait().await;
                health_failure_triggered = true;
                None
            }
        };

        probe_cancel.cancel();

        if file_change_triggered {
            restart_count = 0;
            restart_timestamps.clear();
            continue;
        }

        let exit_code = exit_status.and_then(|s| s.code());
        let success = !health_failure_triggered && exit_status.map(|s| s.success()).unwrap_or(false);

        info!(name = %name, exit_code = ?exit_code, success = success, "Process exited");

        // Fire alert webhook if configured and process crashed or failed health check
        if !success {
            if let Some(ref webhook_url) = config.alerts.on_crash_webhook {
                let url = webhook_url.clone();
                let app_name = name.clone();
                let code = exit_code;
                tokio::spawn(async move {
                    send_crash_webhook(&url, &app_name, code).await;
                });
            }
        }

        // Evaluate restart policy
        let should_restart = if health_failure_triggered {
            true
        } else {
            match config.restart_policy {
                RestartPolicy::Never => false,
                RestartPolicy::OnFailure => !success,
                RestartPolicy::Always => true,
            }
        };

        if !should_restart {
            if success {
                registry.set_state(&name, ProcessState::Stopped);
            } else {
                registry.set_state(
                    &name,
                    ProcessState::Errored {
                        exit_code,
                        message: format!("Process exited with status {:?}", exit_code),
                    },
                );
            }

            // If watch mode is active, stay alive waiting for subsequent file changes
            if config.watch && watch_rx.is_some() {
                info!(name = %name, "Process halted; waiting for file change to resume supervision");
                tokio::select! {
                    _ = cancel_token.cancelled() => break,
                    Some(changed_path) = async {
                        match watch_rx.as_mut() {
                            Some(rx) => rx.recv().await,
                            None => std::future::pending().await,
                        }
                    } => {
                        logger.log(
                            &name,
                            LogLine {
                                timestamp: unix_now(),
                                stream: LogStream::Stdout,
                                text: format!("[orion] File change detected: {}. Resuming supervision...", changed_path.display()),
                            },
                        );
                        restart_count = 0;
                        restart_timestamps.clear();
                        continue;
                    }
                }
            } else {
                break;
            }
        }

        // Sliding restart window enforcement
        let now = unix_now();
        if config.restart_window_secs > 0 {
            let window_start = now.saturating_sub(config.restart_window_secs);
            restart_timestamps.retain(|&t| t >= window_start);
        }
        restart_timestamps.push(now);
        restart_count = restart_timestamps.len() as u32;
        registry.increment_restarts(&name);

        if config.max_restarts > 0 && restart_count >= config.max_restarts {
            let msg = if config.restart_window_secs > 0 {
                format!(
                    "Exceeded max restarts limit ({}) within {}s window",
                    config.max_restarts, config.restart_window_secs
                )
            } else {
                format!("Exceeded max restarts limit ({})", config.max_restarts)
            };
            warn!(name = %name, message = %msg);
            registry.set_state(
                &name,
                ProcessState::Errored {
                    exit_code,
                    message: msg,
                },
            );

            // In watch mode, wait for a file change to reset the restart counter
            if config.watch && watch_rx.is_some() {
                info!(name = %name, "Max restarts reached; waiting for file change to reset and restart");
                tokio::select! {
                    _ = cancel_token.cancelled() => break,
                    Some(changed_path) = async {
                        match watch_rx.as_mut() {
                            Some(rx) => rx.recv().await,
                            None => std::future::pending().await,
                        }
                    } => {
                        logger.log(
                            &name,
                            LogLine {
                                timestamp: unix_now(),
                                stream: LogStream::Stdout,
                                text: format!("[orion] File change detected: {}. Resetting restarts...", changed_path.display()),
                            },
                        );
                        restart_count = 0;
                        restart_timestamps.clear();
                        continue;
                    }
                }
            } else {
                break;
            }
        }

        // Exponential backoff with a cap of 30 seconds
        let backoff_factor = 1u64 << (restart_count.saturating_sub(1).min(4));
        let effective_delay_ms = (config.restart_delay_ms * backoff_factor).min(30_000);

        warn!(
            name = %name,
            restart_count = restart_count,
            delay_ms = effective_delay_ms,
            "Restarting process after backoff delay"
        );

        tokio::select! {
            _ = sleep(Duration::from_millis(effective_delay_ms)) => {},
            _ = cancel_token.cancelled() => {
                registry.set_state(&name, ProcessState::Stopped);
                break;
            }
            Some(changed_path) = async {
                match watch_rx.as_mut() {
                    Some(rx) => rx.recv().await,
                    None => std::future::pending().await,
                }
            } => {
                logger.log(
                    &name,
                    LogLine {
                        timestamp: unix_now(),
                        stream: LogStream::Stdout,
                        text: format!("[orion] File change detected: {}. Restarting immediately...", changed_path.display()),
                    },
                );
                restart_count = 0;
                restart_timestamps.clear();
                continue;
            }
        }
    }

    info!(name = %name, "Supervisor task finished");
}

/// Dedicated background file watcher task that monitors configured directories
/// and debounces change events by 300ms before notifying the supervisor.
async fn run_file_watcher(
    app_name: String,
    paths: Vec<PathBuf>,
    ignore_patterns: Vec<String>,
    notify_tx: tokio::sync::mpsc::Sender<PathBuf>,
    cancel_token: CancellationToken,
) {
    let (raw_tx, mut raw_rx) = tokio::sync::mpsc::unbounded_channel::<notify::Result<Event>>();

    let watcher_res = RecommendedWatcher::new(
        move |res| {
            let _ = raw_tx.send(res);
        },
        Config::default(),
    );

    let mut watcher = match watcher_res {
        Ok(w) => w,
        Err(e) => {
            error!(app = %app_name, error = %e, "Failed to initialize file watcher");
            return;
        }
    };

    let watch_targets = if paths.is_empty() {
        vec![PathBuf::from(".")]
    } else {
        paths
    };

    for target in &watch_targets {
        if target.exists() {
            if let Err(e) = watcher.watch(target, RecursiveMode::Recursive) {
                warn!(app = %app_name, path = %target.display(), error = %e, "Failed to watch path");
            } else {
                info!(app = %app_name, path = %target.display(), "Watching path for file changes");
            }
        } else {
            warn!(app = %app_name, path = %target.display(), "Watch path does not exist");
        }
    }

    let mut debounce_path: Option<PathBuf> = None;
    let debounce_duration = Duration::from_millis(300);

    loop {
        tokio::select! {
            _ = cancel_token.cancelled() => {
                break;
            }
            res = raw_rx.recv() => {
                match res {
                    Some(Ok(event)) => {
                        if matches!(
                            event.kind,
                            EventKind::Modify(_) | EventKind::Create(_) | EventKind::Remove(_)
                        ) {
                            for path in event.paths {
                                if !is_ignored(&path, &ignore_patterns) {
                                    debounce_path = Some(path);
                                }
                            }
                        }
                    }
                    Some(Err(e)) => {
                        debug!(app = %app_name, error = %e, "File watcher error");
                    }
                    None => break,
                }
            }
            _ = sleep(debounce_duration), if debounce_path.is_some() => {
                if let Some(path) = debounce_path.take() {
                    info!(app = %app_name, path = %path.display(), "File change detected (debounced)");
                    if notify_tx.send(path).await.is_err() {
                        break;
                    }
                }
            }
        }
    }
}

/// Checks whether a given path matches built-in system ignores or custom user ignore patterns.
fn is_ignored(path: &Path, ignore_patterns: &[String]) -> bool {
    let path_str = path.to_string_lossy();
    if path_str.contains(".git")
        || path_str.contains("node_modules")
        || path_str.contains("target")
        || path_str.contains(".orion")
        || path_str.ends_with(".db")
        || path_str.ends_with(".db-journal")
        || path_str.ends_with(".log")
        || path_str.ends_with('~')
        || path_str.ends_with(".tmp")
    {
        return true;
    }

    for pat in ignore_patterns {
        if pat.starts_with("*.") {
            let ext = &pat[1..];
            if path_str.ends_with(ext) {
                return true;
            }
        } else if path_str.contains(pat) {
            return true;
        }
    }
    false
}

async fn send_crash_webhook(url: &str, app_name: &str, exit_code: Option<i32>) {
    let client = reqwest::Client::new();
    let payload = serde_json::json!({
        "event": "process_crash",
        "app": app_name,
        "exit_code": exit_code,
        "timestamp": unix_now(),
    });

    if let Err(e) = client.post(url).json(&payload).send().await {
        error!(url = %url, error = %e, "Failed to deliver crash webhook");
    }
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Runs periodic HTTP or TCP readiness and liveness probes against a supervised process.
/// If max_failures consecutive checks fail, it notifies the supervisor via unhealthy_tx.
async fn run_health_probe(
    health: HealthConfig,
    unhealthy_tx: tokio::sync::mpsc::Sender<()>,
    cancel_token: CancellationToken,
) {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(health.timeout_secs.max(1)))
        .build()
        .unwrap_or_default();

    let mut consecutive_failures = 0;
    let interval = Duration::from_secs(health.interval_secs.max(1));

    // Initial 2-second grace period to allow the application to bind its sockets
    tokio::select! {
        _ = cancel_token.cancelled() => return,
        _ = sleep(Duration::from_secs(2)) => {}
    }

    loop {
        tokio::select! {
            _ = cancel_token.cancelled() => return,
            _ = sleep(interval) => {}
        }

        let is_healthy = if let Some(ref http_url) = health.http {
            match client.get(http_url).send().await {
                Ok(resp) => resp.status().is_success() || resp.status().is_redirection(),
                Err(_) => false,
            }
        } else if let Some(ref tcp_addr) = health.tcp {
            matches!(
                tokio::time::timeout(
                    Duration::from_secs(health.timeout_secs.max(1)),
                    tokio::net::TcpStream::connect(tcp_addr),
                )
                .await,
                Ok(Ok(_))
            )
        } else {
            true
        };

        if is_healthy {
            consecutive_failures = 0;
        } else {
            consecutive_failures += 1;
            if consecutive_failures >= health.max_failures {
                let _ = unhealthy_tx.send(()).await;
                return;
            }
        }
    }
}

