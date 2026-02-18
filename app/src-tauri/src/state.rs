use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};
use variance_app::AppState;

pub struct NodeState {
    pub app_state: Arc<RwLock<Option<AppState>>>,
    pub server_port: Arc<RwLock<Option<u16>>>,
    pub shutdown_tx: Arc<RwLock<Option<tokio::sync::mpsc::Sender<()>>>>,
    /// Held for the duration of node startup to prevent concurrent start attempts
    /// (React StrictMode mounts effects twice in dev, causing a race otherwise).
    pub start_lock: Arc<Mutex<()>>,
}

impl Default for NodeState {
    fn default() -> Self {
        Self {
            app_state: Arc::new(RwLock::new(None)),
            server_port: Arc::new(RwLock::new(None)),
            shutdown_tx: Arc::new(RwLock::new(None)),
            start_lock: Arc::new(Mutex::new(())),
        }
    }
}
