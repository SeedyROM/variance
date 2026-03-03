use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, RwLock};
use tokio::task::JoinHandle;
use variance_app::AppState;

pub struct NodeState {
    pub app_state: Arc<RwLock<Option<AppState>>>,
    pub server_port: Arc<RwLock<Option<u16>>>,
    pub shutdown_tx: Arc<RwLock<Option<tokio::sync::mpsc::Sender<()>>>>,
    /// Background task running the P2P node; awaited on shutdown for clean sled sync.
    pub node_task: Arc<RwLock<Option<JoinHandle<anyhow::Result<()>>>>>,
    /// Held for the duration of node startup to prevent concurrent start attempts
    /// (React StrictMode mounts effects twice in dev, causing a race otherwise).
    pub start_lock: Arc<Mutex<()>>,
}

impl NodeState {
    /// Gracefully stop the running node.
    ///
    /// Sends the shutdown signal, then waits up to 5 s for the P2P task to finish.
    pub async fn stop(&self) {
        if let Some(tx) = self.shutdown_tx.write().await.take() {
            let _ = tx.send(()).await;
        }
        if let Some(task) = self.node_task.write().await.take() {
            match tokio::time::timeout(Duration::from_secs(5), task).await {
                Ok(Ok(_)) => tracing::info!("P2P node shut down cleanly"),
                Ok(Err(e)) => tracing::warn!("Node task error during shutdown: {}", e),
                Err(_) => tracing::warn!("Node shutdown timed out after 5 s"),
            }
        }
        *self.app_state.write().await = None;
        *self.server_port.write().await = None;
    }
}

impl Default for NodeState {
    fn default() -> Self {
        Self {
            app_state: Arc::new(RwLock::new(None)),
            server_port: Arc::new(RwLock::new(None)),
            shutdown_tx: Arc::new(RwLock::new(None)),
            node_task: Arc::new(RwLock::new(None)),
            start_lock: Arc::new(Mutex::new(())),
        }
    }
}
