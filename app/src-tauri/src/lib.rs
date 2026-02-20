mod commands;
mod state;

use commands::*;
use state::NodeState;
use tauri::Manager;
use tracing::info;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Initialize tracing subscriber for logging
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    info!("Variance Desktop initializing...");

    tauri::Builder::default()
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(NodeState::default())
        .invoke_handler(tauri::generate_handler![
            has_identity,
            generate_identity,
            recover_identity,
            default_identity_path,
            start_node,
            stop_node,
            get_api_port,
            get_node_status,
        ])
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::Destroyed = event {
                info!("Window destroyed, initiating shutdown...");
                // Send shutdown signal to P2P node when the window is destroyed
                let state = window.state::<NodeState>();
                let shutdown_tx = state.shutdown_tx.clone();
                tauri::async_runtime::spawn(async move {
                    let tx: Option<tokio::sync::mpsc::Sender<()>> =
                        shutdown_tx.write().await.take();
                    if let Some(tx) = tx {
                        info!("Sending shutdown signal to node");
                        let _ = tx.send(()).await;
                    }
                });
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running Tauri application");

    info!("Variance Desktop shutting down");
}
