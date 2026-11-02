use orion_core::config::{AppConfig, ExecutionMode, RestartPolicy};
use oriond::runtime::{run_foreground, RuntimeOptions};
use std::collections::HashMap;
use std::path::PathBuf;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

#[tokio::test]
async fn test_foreground_runtime_and_healthz() {
    let health_port = 19600u16;

    let app_config = AppConfig {
        command: "node".to_string(),
        args: vec!["-e".to_string(), "setInterval(()=>{}, 1000)".to_string()],
        cwd: Some(PathBuf::from(".")),
        name: Some("test-runtime-app".to_string()),
        namespace: None,
        mode: ExecutionMode::Fork,
        instances: 1,
        env: HashMap::new(),
        restart_policy: RestartPolicy::Never,
        restart_delay_ms: 1000,
        restart_backoff_multiplier: 2.0,
        max_restarts: 0,
        restart_window_secs: 60,
        cron_restart: None,
        watch: false,
        watch_paths: Vec::new(),
        watch_ignore: Vec::new(),
        stdout_log: None,
        stderr_log: None,
        log_lines: 100,
        max_memory_mb: 0,
        max_cpu_pct: 0.0,
        alerts: Default::default(),
        health: None,
        smtp: None,
        kill_timeout_secs: 2,
        port: None,
    };

    let apps = vec![("test-runtime-app".to_string(), app_config)];
    let opts = RuntimeOptions {
        raw: false,
        json: true,
        health_port: Some(health_port),
    };

    // Run foreground runtime in background task
    let runtime_handle = tokio::spawn(async move {
        let _ = run_foreground(apps, opts).await;
    });

    // Give runtime server time to bind
    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

    // Send HTTP GET /healthz
    if let Ok(mut stream) = TcpStream::connect(format!("127.0.0.1:{}", health_port)).await {
        stream
            .write_all(b"GET /healthz HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n")
            .await
            .unwrap();

        let mut buf = [0u8; 512];
        let n = stream.read(&mut buf).await.unwrap();
        let resp = String::from_utf8_lossy(&buf[..n]);
        assert!(resp.contains("200 OK"));
        assert!(resp.contains("healthy"));
    }

    runtime_handle.abort();
}
