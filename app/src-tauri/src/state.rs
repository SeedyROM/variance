use std::sync::Arc;
use tokio::sync::RwLock;
use variance_app::AppState;

pub struct NodeState {
    pub app_state: Arc<RwLock<Option<AppState>>>,
    pub server_port: Arc<RwLock<Option<u16>>>,
    pub shutdown_tx: Arc<RwLock<Option<tokio::sync::mpsc::Sender<()>>>>,
}

impl Default for NodeState {
    fn default() -> Self {
        Self {
            app_state: Arc::new(RwLock::new(None)),
            server_port: Arc::new(RwLock::new(None)),
            shutdown_tx: Arc::new(RwLock::new(None)),
        }
    }
}
