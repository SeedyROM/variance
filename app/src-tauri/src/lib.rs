mod commands;
mod state;

use commands::*;
use state::NodeState;
use tauri::Manager;
#[cfg(target_os = "macos")]
use tauri_plugin_decorum::WebviewWindowExt;
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

    info!("Variance initializing...");

    let mut builder = tauri::Builder::default()
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init());

    #[cfg(target_os = "macos")]
    {
        builder = builder.plugin(tauri_plugin_decorum::init());
    }

    builder
        .setup(|_app| {
            #[cfg(target_os = "macos")]
            {
                let win = _app.get_webview_window("main").unwrap();
                win.set_traffic_lights_inset(12.0, 16.0).unwrap();
            }
            Ok(())
        })
        .manage(NodeState::default())
        .invoke_handler(tauri::generate_handler![
            has_identity,
            check_identity_encrypted,
            generate_identity,
            recover_identity,
            default_identity_path,
            start_node,
            stop_node,
            get_api_port,
            get_node_status,
            change_passphrase,
        ])
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::Destroyed = event {
                info!("Window destroyed, shutting down node...");
                let state = window.state::<NodeState>();
                // Clone the Arcs so we can move them into the async block.
                let shutdown_tx = state.shutdown_tx.clone();
                let node_task = state.node_task.clone();
                let app_state = state.app_state.clone();
                let server_port = state.server_port.clone();
                tauri::async_runtime::spawn(async move {
                    if let Some(tx) = shutdown_tx.write().await.take() {
                        info!("Sending shutdown signal to node");
                        let _ = tx.send(()).await;
                    }
                    if let Some(task) = node_task.write().await.take() {
                        match tokio::time::timeout(std::time::Duration::from_secs(5), task).await {
                            Ok(Ok(_)) => info!("P2P node shut down cleanly"),
                            Ok(Err(e)) => tracing::warn!("Node error during shutdown: {}", e),
                            Err(_) => tracing::warn!("Node shutdown timed out"),
                        }
                    }
                    *app_state.write().await = None;
                    *server_port.write().await = None;
                });
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running Tauri application");

    info!("Variance shutting down");
}
