use oriond::cluster::ClusterRegistry;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn test_rolling_reload_under_live_traffic() {
    let cluster_registry = ClusterRegistry::new();
    let cancel = CancellationToken::new();

    let public_port = 19700u16;
    let backend_ports = vec![19701u16, 19702u16];

    let worker1_online = Arc::new(AtomicBool::new(true));
    let worker2_online = Arc::new(AtomicBool::new(true));

    // Spawn Worker 1 on port 19701
    let w1_online = worker1_online.clone();
    let cancel_w1 = cancel.clone();
    tokio::spawn(async move {
        if let Ok(listener) = TcpListener::bind("127.0.0.1:19701").await {
            loop {
                tokio::select! {
                    _ = cancel_w1.cancelled() => break,
                    res = listener.accept() => {
                        if let Ok((mut socket, _)) = res {
                            if !w1_online.load(Ordering::SeqCst) {
                                drop(socket);
                                continue;
                            }
                            let mut buf = [0u8; 64];
                            let _ = socket.read(&mut buf).await;
                            let _ = socket.write_all(b"HTTP/1.1 200 OK\r\n\r\nWORKER_1").await;
                        }
                    }
                }
            }
        }
    });

    // Spawn Worker 2 on port 19702
    let w2_online = worker2_online.clone();
    let cancel_w2 = cancel.clone();
    tokio::spawn(async move {
        if let Ok(listener) = TcpListener::bind("127.0.0.1:19702").await {
            loop {
                tokio::select! {
                    _ = cancel_w2.cancelled() => break,
                    res = listener.accept() => {
                        if let Ok((mut socket, _)) = res {
                            if !w2_online.load(Ordering::SeqCst) {
                                drop(socket);
                                continue;
                            }
                            let mut buf = [0u8; 64];
                            let _ = socket.read(&mut buf).await;
                            let _ = socket.write_all(b"HTTP/1.1 200 OK\r\n\r\nWORKER_2").await;
                        }
                    }
                }
            }
        }
    });

    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

    // Register cluster on port 19700
    cluster_registry
        .register_cluster("reload-test", public_port, backend_ports)
        .await;

    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

    let successful_requests = Arc::new(AtomicU32::new(0));
    let traffic_done = Arc::new(AtomicBool::new(false));

    // Traffic generator task: continuously sends requests through port 19700
    let success_counter = successful_requests.clone();
    let traffic_flag = traffic_done.clone();
    let traffic_task = tokio::spawn(async move {
        while !traffic_flag.load(Ordering::SeqCst) {
            if let Ok(mut stream) = TcpStream::connect(format!("127.0.0.1:{}", public_port)).await {
                let _ = stream.write_all(b"GET / HTTP/1.1\r\n\r\n").await;
                let mut buf = [0u8; 128];
                if let Ok(n) = stream.read(&mut buf).await {
                    let resp = String::from_utf8_lossy(&buf[..n]);
                    if resp.contains("200 OK") {
                        success_counter.fetch_add(1, Ordering::SeqCst);
                    }
                }
            }
            tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
        }
    });

    // Simulate rolling reload:
    // 1. Reload worker 1 (temporarily down for 100ms)
    worker1_online.store(false, Ordering::SeqCst);
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    worker1_online.store(true, Ordering::SeqCst);

    // 2. Reload worker 2 (temporarily down for 100ms)
    worker2_online.store(false, Ordering::SeqCst);
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    worker2_online.store(true, Ordering::SeqCst);

    // Let traffic run a bit longer with both online
    tokio::time::sleep(tokio::time::Duration::from_millis(150)).await;

    traffic_done.store(true, Ordering::SeqCst);
    let _ = traffic_task.await;

    let total_success = successful_requests.load(Ordering::SeqCst);
    println!("Total successful requests during rolling reload: {}", total_success);
    assert!(total_success >= 5, "Expected at least 5 continuous successful requests");

    cluster_registry.unregister_cluster("reload-test").await;
    cancel.cancel();
}
