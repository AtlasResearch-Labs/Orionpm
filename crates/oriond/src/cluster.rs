use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, RwLock};
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
    backend_ports: Arc<RwLock<Vec<u16>>>,
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
            backend_ports: Arc::new(RwLock::new(backend_ports)),
            index: AtomicUsize::new(0),
            cancel_token,
        }
    }

    /// Dynamically update all backend ports without tearing down the listening socket.
    pub fn update_backends(&self, new_backends: Vec<u16>) {
        if let Ok(mut guard) = self.backend_ports.write() {
            *guard = new_backends;
            info!(
                cluster = %self.cluster_name,
                backends = ?*guard,
                "Universal Cluster Balancer backends updated in-place"
            );
        }
    }

    /// Atomically swap an old worker port for a new worker port (zero-downtime rolling reload).
    pub fn swap_backend(&self, old_port: u16, new_port: u16) {
        if let Ok(mut guard) = self.backend_ports.write() {
            let mut swapped = false;
            for port in guard.iter_mut() {
                if *port == old_port {
                    *port = new_port;
                    swapped = true;
                    break;
                }
            }
            if !swapped {
                guard.push(new_port);
            }
            info!(
                cluster = %self.cluster_name,
                old_port,
                new_port,
                active_backends = ?*guard,
                "Universal Cluster Balancer atomically swapped backend port"
            );
        }
    }

    pub fn start(self: Arc<Self>) {
        let this = self.clone();
        tokio::spawn(async move {
            let addr = format!("0.0.0.0:{}", this.public_port);
            let listener = match TcpListener::bind(&addr).await {
                Ok(l) => {
                    let backends = this.backend_ports.read().map(|b| b.clone()).unwrap_or_default();
                    info!(
                        cluster = %this.cluster_name,
                        public_port = this.public_port,
                        backends = ?backends,
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
        let backends = match self.backend_ports.read() {
            Ok(b) => b.clone(),
            Err(_) => return,
        };
        let num_backends = backends.len();
        if num_backends == 0 {
            return;
        }

        // Try up to `num_backends` times in round-robin to find an active responding worker
        let start_idx = self.index.fetch_add(1, Ordering::Relaxed);
        for attempt in 0..num_backends {
            let backend_port = backends[(start_idx + attempt) % num_backends];
            let backend_addr = format!("127.0.0.1:{}", backend_port);

            if let Ok(mut backend_stream) = TcpStream::connect(&backend_addr).await {
                let _ = copy_bidirectional(&mut client_stream, &mut backend_stream).await;
                return;
            }
        }
    }
}

type BalancerEntry = (u16, Arc<ClusterBalancer>, CancellationToken);

/// Global registry of active cluster load balancers.
#[derive(Clone, Default)]
pub struct ClusterRegistry {
    balancers: Arc<Mutex<HashMap<String, BalancerEntry>>>,
}

impl ClusterRegistry {
    pub fn new() -> Self {
        Self {
            balancers: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Registers and starts a load balancer for a cluster if not already active on the port.
    /// If already running on the same port, dynamically updates backends without dropping connections.
    pub async fn register_cluster(
        &self,
        cluster_name: &str,
        public_port: u16,
        backend_ports: Vec<u16>,
    ) {
        let mut map = self.balancers.lock().await;

        // If a balancer is already running for this cluster on the same public port,
        // update backend ports in place to guarantee zero-downtime without rebinding!
        if let Some((existing_port, existing_balancer, _)) = map.get(cluster_name) {
            if *existing_port == public_port {
                existing_balancer.update_backends(backend_ports);
                return;
            }
        }

        // Otherwise stop old balancer if port changed
        if let Some((_, _, cancel)) = map.remove(cluster_name) {
            cancel.cancel();
        }

        let cancel_token = CancellationToken::new();
        let balancer = Arc::new(ClusterBalancer::new(
            cluster_name.to_string(),
            public_port,
            backend_ports,
            cancel_token.clone(),
        ));
        balancer.clone().start();
        map.insert(
            cluster_name.to_string(),
            (public_port, balancer, cancel_token),
        );
    }

    /// Atomically swap an old worker port for a new worker port in an active cluster balancer.
    pub async fn swap_backend(&self, cluster_name: &str, old_port: u16, new_port: u16) {
        let map = self.balancers.lock().await;
        if let Some((_, balancer, _)) = map.get(cluster_name) {
            balancer.swap_backend(old_port, new_port);
        }
    }

    /// Stops the load balancer for a cluster.
    pub async fn unregister_cluster(&self, cluster_name: &str) {
        let mut map = self.balancers.lock().await;
        if let Some((_, _, cancel)) = map.remove(cluster_name) {
            cancel.cancel();
        }
    }
}
