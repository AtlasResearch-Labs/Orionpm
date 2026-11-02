use oriond::cluster::ClusterRegistry;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn test_universal_cluster_load_balancer() {
    let cluster_registry = ClusterRegistry::new();
    let cancel = CancellationToken::new();

    let public_port = 19500u16;
    let backend_ports = vec![19501u16, 19502u16, 19503u16];

    let counts = Arc::new([
        AtomicUsize::new(0),
        AtomicUsize::new(0),
        AtomicUsize::new(0),
    ]);

    // Spawn 3 mock backend worker servers
    for (i, &port) in backend_ports.iter().enumerate() {
        let cancel_child = cancel.clone();
        let count_ref = counts.clone();
        tokio::spawn(async move {
            let addr = format!("127.0.0.1:{}", port);
            if let Ok(listener) = TcpListener::bind(&addr).await {
                loop {
                    tokio::select! {
                        _ = cancel_child.cancelled() => break,
                        res = listener.accept() => {
                            if let Ok((mut socket, _)) = res {
                                count_ref[i].fetch_add(1, Ordering::SeqCst);
                                let mut buf = [0u8; 128];
                                let _ = socket.read(&mut buf).await;
                                let resp = format!("HELLO FROM BACKEND {}", port);
                                let _ = socket.write_all(resp.as_bytes()).await;
                            }
                        }
                    }
                }
            }
        });
    }

    // Give mock workers time to bind
    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

    // Register cluster on public port 19500
    cluster_registry
        .register_cluster("web-cluster", public_port, backend_ports.clone())
        .await;

    // Give cluster balancer time to bind
    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

    // Fire 15 requests into the public port
    for _ in 0..15 {
        let mut client = TcpStream::connect(format!("127.0.0.1:{}", public_port))
            .await
            .expect("Must connect to public cluster port");

        client.write_all(b"PING").await.unwrap();
        let mut buf = [0u8; 128];
        let n = client.read(&mut buf).await.unwrap();
        let response = String::from_utf8_lossy(&buf[..n]);
        assert!(response.starts_with("HELLO FROM BACKEND"));
    }

    // Verify round-robin connection distribution across all 3 backends
    assert_eq!(counts[0].load(Ordering::SeqCst), 5);
    assert_eq!(counts[1].load(Ordering::SeqCst), 5);
    assert_eq!(counts[2].load(Ordering::SeqCst), 5);

    // Unregister cluster and verify shutdown
    cluster_registry.unregister_cluster("web-cluster").await;
    cancel.cancel();
}
