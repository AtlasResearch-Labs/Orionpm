use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::io::copy_bidirectional;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

/// High-performance TCP round-robin load balancer for multi-instance clusters.
/// Proxies incoming traffic on a public port (e.g. 3000) to private worker ports.
pub struct ClusterBalancer {
    pub cluster_name: String,
    pub public_port: u16,
    pub backend_ports: Vec<u16>,
    index: AtomicUsize,
    cancel_token: CancellationToken,
}

impl ClusterBalancer {
    pub fn new(
        cluster_name: String,
        public_port: u16,
        backend_ports: Vec<u16>,
        cancel_token: CancellationToken,
    ) -> Self {
        Self {
            cluster_name,
            public_port,
            backend_ports,
            index: AtomicUsize::new(0),
            cancel_token,
        }
    }

    pub fn start(self: Arc<Self>) {
        let this = self.clone();
        tokio::spawn(async move {
            let addr = format!("0.0.0.0:{}", this.public_port);
            let listener = match TcpListener::bind(&addr).await {
                Ok(l) => {
                    info!(
                        cluster = %this.cluster_name,
                        public_port = this.public_port,
                        backends = ?this.backend_ports,
                        "Universal Cluster Balancer listening"
                    );
                    l
                }
                Err(e) => {
                    error!(
                        cluster = %this.cluster_name,
                        public_port = this.public_port,
                        error = %e,
                        "Failed to bind Universal Cluster Balancer public port"
                    );
                    return;
                }
            };

            loop {
                tokio::select! {
                    _ = this.cancel_token.cancelled() => {
                        info!(
                            cluster = %this.cluster_name,
                            public_port = this.public_port,
                            "Universal Cluster Balancer shutting down"
                        );
                        break;
                    }
                    res = listener.accept() => {
                        match res {
                            Ok((client_stream, _)) => {
                                let balancer = this.clone();
                                tokio::spawn(async move {
                                    balancer.handle_connection(client_stream).await;
                                });
                            }
                            Err(e) => {
                                warn!(error = %e, "Cluster balancer accept error");
                            }
                        }
                    }
                }
            }
        });
    }

    async fn handle_connection(&self, mut client_stream: TcpStream) {
        let num_backends = self.backend_ports.len();
        if num_backends == 0 {
            return;
        }

        // Try up to `num_backends` times in round-robin to find an active responding worker
        let start_idx = self.index.fetch_add(1, Ordering::Relaxed);
        for attempt in 0..num_backends {
            let backend_port = self.backend_ports[(start_idx + attempt) % num_backends];
            let backend_addr = format!("127.0.0.1:{}", backend_port);

            if let Ok(mut backend_stream) = TcpStream::connect(&backend_addr).await {
                let _ = copy_bidirectional(&mut client_stream, &mut backend_stream).await;
                return;
            }
        }
    }
}

/// Global registry of active cluster load balancers.
#[derive(Clone, Default)]
pub struct ClusterRegistry {
    balancers: Arc<Mutex<HashMap<String, (u16, CancellationToken)>>>,
}

impl ClusterRegistry {
    pub fn new() -> Self {
        Self {
            balancers: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Registers and starts a load balancer for a cluster if not already active on the port.
    pub async fn register_cluster(
        &self,
        cluster_name: &str,
        public_port: u16,
        backend_ports: Vec<u16>,
    ) {
        let mut map = self.balancers.lock().await;
        // Stop old balancer if reconfiguring
        if let Some((_, cancel)) = map.remove(cluster_name) {
            cancel.cancel();
        }

        let cancel_token = CancellationToken::new();
        let balancer = Arc::new(ClusterBalancer::new(
            cluster_name.to_string(),
            public_port,
            backend_ports,
            cancel_token.clone(),
        ));
        balancer.start();
        map.insert(cluster_name.to_string(), (public_port, cancel_token));
    }

    /// Stops the load balancer for a cluster.
    pub async fn unregister_cluster(&self, cluster_name: &str) {
        let mut map = self.balancers.lock().await;
        if let Some((_, cancel)) = map.remove(cluster_name) {
            cancel.cancel();
        }
    }
}
