//! Node startup and lifecycle management
//!
//! Provides a unified API for starting Variance nodes that is used by both
//! the CLI (standalone server) and desktop app (embedded in Tauri).

use crate::{create_router, AppConfig, AppState, EventRouter, Result};
use axum::Router;
use std::path::Path;
use std::sync::Arc;
use tokio::task::JoinHandle;

/// A fully initialized Variance node ready to serve HTTP requests
///
/// Contains all the components needed to run a Variance node. The HTTP server
/// is **not** started - consumers should create their own `TcpListener` and
/// use the provided `router` with `axum::serve()`.
pub struct RunningNode {
    /// Application state (can be cloned for other uses)
    pub app_state: AppState,

    /// Axum router ready to be served
    pub router: Router,

    /// Sender to trigger node shutdown
    pub shutdown_tx: tokio::sync::mpsc::Sender<()>,

    /// Background task running the P2P node
    pub node_task: JoinHandle<Result<(), variance_p2p::Error>>,
}

impl RunningNode {
    /// Gracefully shutdown the P2P node
    ///
    /// Sends shutdown signal and waits for the node task to complete.
    /// Returns an error if the node task panicked or encountered an error.
    pub async fn shutdown(self) -> Result<()> {
        let _ = self.shutdown_tx.send(()).await;

        match tokio::time::timeout(std::time::Duration::from_secs(5), self.node_task).await {
            Ok(Ok(Ok(_))) => {
                tracing::info!("P2P node shut down successfully");
                Ok(())
            }
            Ok(Ok(Err(e))) => {
                tracing::error!("P2P node error during shutdown: {}", e);
                Err(crate::Error::App {
                    message: format!("P2P node shutdown error: {}", e),
                })
            }
            Ok(Err(e)) => {
                tracing::error!("P2P node task panicked: {}", e);
                Err(crate::Error::App {
                    message: format!("Node task panicked: {}", e),
                })
            }
            Err(_) => {
                tracing::warn!("P2P node shutdown timed out");
                Err(crate::Error::App {
                    message: "Node shutdown timed out".to_string(),
                })
            }
        }
    }
}

/// Start a Variance node with all components initialized
///
/// This function:
/// 1. Creates and configures the P2P node
/// 2. Spawns the P2P event loop in a background task
/// 3. Loads identity from the specified file
/// 4. Creates application state with message storage
/// 5. Starts the event router (bridges P2P events to WebSocket)
/// 6. Creates the HTTP API router
///
/// # Returns
///
/// A `RunningNode` containing all components. The caller is responsible for:
/// - Creating a `TcpListener` (with desired bind address)
/// - Starting the HTTP server with `axum::serve(listener, node.router)`
/// - Calling `node.shutdown()` when done
///
/// # Example (CLI)
///
/// ```ignore
/// let config = AppConfig::default();
/// let node = start_node(&config, Path::new("identity.json")).await?;
/// let listener = TcpListener::bind("127.0.0.1:8080").await?;
/// axum::serve(listener, node.router)
///     .with_graceful_shutdown(shutdown_signal())
///     .await?;
/// node.shutdown().await?;
/// ```
///
/// # Example (Desktop)
///
/// ```ignore
/// let config = AppConfig::default();
/// let node = start_node(&config, Path::new("identity.json")).await?;
/// let listener = TcpListener::bind("127.0.0.1:0").await?;
/// let port = listener.local_addr()?.port();
/// tokio::spawn(async move {
///     axum::serve(listener, node.router).await
/// });
/// // Store node.shutdown_tx to stop later
/// ```
pub async fn start_node(config: &AppConfig, identity_path: &Path) -> Result<RunningNode> {
    tracing::info!("Initializing Variance node...");

    // Create P2P node configuration
    let mut listen_addresses = Vec::new();
    for addr_str in &config.p2p.listen_addrs {
        let addr = addr_str.parse().map_err(|e| crate::Error::App {
            message: format!("Invalid listen address {}: {}", addr_str, e),
        })?;
        listen_addresses.push(addr);
    }

    let mut bootstrap_peers = Vec::new();
    for peer_str in &config.p2p.bootstrap_peers {
        let parts: Vec<&str> = peer_str.split('@').collect();
        if parts.len() == 2 {
            bootstrap_peers.push(variance_p2p::BootstrapPeer {
                peer_id: parts[0].to_string(),
                multiaddr: parts[1].parse().map_err(|e| crate::Error::App {
                    message: format!("Invalid bootstrap peer address {}: {}", parts[1], e),
                })?,
            });
        } else {
            tracing::warn!("Skipping invalid bootstrap peer format: {}", peer_str);
        }
    }

    let p2p_config = variance_p2p::Config {
        listen_addresses,
        bootstrap_peers,
        enable_mdns: true,
        storage_path: config.storage.base_dir.clone(),
        ..Default::default()
    };

    // Create P2P node and get handle
    tracing::debug!("Creating P2P node...");
    let (mut node, node_handle) =
        variance_p2p::Node::new(p2p_config.clone()).map_err(|e| crate::Error::App {
            message: format!("Failed to create P2P node: {}", e),
        })?;

    // Get EventChannels reference before spawning node
    let event_channels = Arc::new(node.events().clone());

    // Spawn P2P node in background task
    let (shutdown_tx, shutdown_rx) = tokio::sync::mpsc::channel::<()>(1);
    let p2p_config_clone = p2p_config.clone();
    let node_task = tokio::spawn(async move {
        // Start listening on configured addresses
        if let Err(e) = node.listen(&p2p_config_clone).await {
            tracing::error!("Failed to start listening: {}", e);
            return Err(e);
        }
        // Run event loop
        node.run(shutdown_rx).await
    });

    tracing::debug!("P2P node task spawned");

    // Load identity and create application state
    tracing::debug!("Loading identity from: {}", identity_path.display());
    let app_state = AppState::from_identity_file(
        identity_path,
        config.storage.message_db_path.to_str().unwrap(),
        node_handle,
        Some(event_channels.clone()),
    )
    .map_err(|e| crate::Error::App {
        message: format!("Failed to load identity: {}", e),
    })?;

    tracing::info!("Identity loaded: {}", app_state.local_did);

    // Generate initial batch of one-time pre-keys so peers can establish Olm sessions.
    app_state.direct_messaging.generate_one_time_keys(50).await;

    // Register our own identity with the P2P identity handler so we can respond to
    // inbound DID queries with our Olm keys. Peers need these to open outbound sessions.
    let olm_identity_key = app_state
        .direct_messaging
        .identity_key()
        .to_bytes()
        .to_vec();
    let one_time_keys = app_state
        .direct_messaging
        .one_time_keys()
        .await
        .values()
        .map(|k| k.to_bytes().to_vec())
        .collect::<Vec<_>>();

    // Mark keys as published so vodozemac moves them into its published pool.
    // create_inbound_session() only searches published keys — calling this is what
    // makes inbound PreKey messages decryptable.
    app_state
        .direct_messaging
        .mark_one_time_keys_as_published()
        .await;

    if let Err(e) = app_state
        .node_handle
        .set_local_identity(app_state.local_did.clone(), olm_identity_key, one_time_keys)
        .await
    {
        tracing::warn!("Failed to register local identity with P2P handler: {}", e);
    }

    // Start event router to bridge P2P events to WebSocket clients
    let event_router = EventRouter::new(
        app_state.ws_manager.clone(),
        app_state.direct_messaging.clone(),
        app_state.group_messaging.clone(),
    );
    event_router.start((*event_channels).clone());
    tracing::debug!("EventRouter started");

    // Create HTTP API router
    let router = create_router(app_state.clone());

    tracing::info!("✓ Variance node initialized successfully");

    Ok(RunningNode {
        app_state,
        router,
        shutdown_tx,
        node_task,
    })
}
