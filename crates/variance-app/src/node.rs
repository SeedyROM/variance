//! Node startup and lifecycle management
//!
//! Provides a unified API for starting Variance nodes that is used by both
//! the CLI (standalone server) and desktop app (embedded in Tauri).

use crate::event_router::EventRouterDeps;
use crate::{create_router, AppConfig, AppState, EventRouter, Result};
use axum::Router;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::task::JoinHandle;
use variance_identity::storage::{IdentityStorage, IpfsStorage, LocalStorage};
use variance_messaging::storage::MessageStorage;

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

        match tokio::time::timeout(Duration::from_secs(5), self.node_task).await {
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
pub async fn start_node(
    config: &AppConfig,
    identity_path: &Path,
    passphrase: Option<&str>,
) -> Result<RunningNode> {
    tracing::info!("Initializing Variance node...");

    // Load identity early to derive a stable libp2p PeerId from the Ed25519 signing key.
    // Without this, Node::new() generates a fresh ephemeral keypair each restart, causing
    // PeerStore cache misses (stale PeerId) and "known but not connected" send failures.
    let libp2p_keypair = match AppState::load_identity_with_passphrase(identity_path, passphrase) {
        Ok(identity) => match hex::decode(&identity.signing_key) {
            Ok(bytes) => match variance_p2p::keypair_from_ed25519(bytes) {
                Some(kp) => {
                    tracing::debug!("Derived stable libp2p PeerId from identity key");
                    Some(kp)
                }
                None => {
                    tracing::warn!("Failed to derive libp2p keypair from signing key bytes; PeerId will be ephemeral");
                    None
                }
            },
            Err(e) => {
                tracing::warn!("Cannot hex-decode signing key for keypair derivation: {}; PeerId will be ephemeral", e);
                None
            }
        },
        Err(e) => {
            tracing::warn!(
                "Cannot load identity for keypair derivation: {}; PeerId will be ephemeral",
                e
            );
            None
        }
    };

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

    let mut relay_peers = Vec::new();
    for relay in &config.p2p.relay_peers {
        relay_peers.push(variance_p2p::BootstrapPeer {
            peer_id: relay.peer_id.clone(),
            multiaddr: relay.multiaddr.parse().map_err(|e| crate::Error::App {
                message: format!("Invalid relay peer address {}: {}", relay.multiaddr, e),
            })?,
        });
    }

    let p2p_config = variance_p2p::Config {
        listen_addresses,
        bootstrap_peers,
        relay_peers,
        enable_mdns: true,
        storage_path: config.storage.base_dir.clone(),
        keypair: libp2p_keypair,
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

    // Connect to identity storage (IPFS in production, local fallback when IPFS unavailable).
    let ipfs_storage: Arc<dyn IdentityStorage> = match IpfsStorage::new(&config.identity.ipfs_api) {
        Ok(s) => {
            tracing::info!("IPFS storage connected at {}", config.identity.ipfs_api);
            Arc::new(s)
        }
        Err(e) => {
            tracing::warn!("IPFS unavailable ({}), using local identity storage", e);
            Arc::new(
                LocalStorage::new(config.storage.identity_cache_dir.join("ipfs-local")).map_err(
                    |e| crate::Error::App {
                        message: format!("Failed to create local identity storage: {}", e),
                    },
                )?,
            )
        }
    };

    // Load identity and create application state
    tracing::debug!("Loading identity from: {}", identity_path.display());
    let app_state = AppState::from_identity_file(
        identity_path,
        config.storage.message_db_path.to_str().unwrap(),
        config.storage.identity_cache_dir.to_str().unwrap(),
        node_handle,
        event_channels.clone(),
        ipfs_storage,
        passphrase,
    )
    .map_err(|e| crate::Error::App {
        message: format!("Failed to load identity: {}", e),
    })?;

    tracing::info!("Identity loaded: {}", app_state.local_did);

    // Seed username registry from persisted peer names so display names are
    // available immediately without needing a P2P identity resolution first.
    match app_state.storage.load_all_peer_names().await {
        Ok(peer_names) => {
            for (did, username, discriminator) in peer_names {
                app_state
                    .username_registry
                    .cache_mapping(username, discriminator, did);
            }
        }
        Err(e) => tracing::warn!("Failed to load persisted peer names: {}", e),
    }

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

    // Persist the account (now holding the generated OTKs) back to identity.json.
    // Without this, OTKs are in-memory only: a restart reverts to the zero-OTK
    // initial pickle, making any queued PreKey messages impossible to decrypt.
    match app_state.direct_messaging.account_pickle().await {
        Ok(pickle_json) => {
            match AppState::load_identity_with_passphrase(identity_path, passphrase) {
                Ok(mut identity_file) => {
                    identity_file.olm_account_pickle = pickle_json;
                    if let Err(e) =
                        AppState::save_identity(identity_path, &identity_file, passphrase)
                    {
                        tracing::warn!("Failed to persist Olm OTKs to identity file: {}", e);
                    }
                }
                Err(e) => {
                    tracing::warn!("Failed to reload identity file for OTK persistence: {}", e)
                }
            }
        }
        Err(e) => tracing::warn!("Failed to serialize Olm account pickle: {}", e),
    }

    // Publish local DID document to IPFS/IPNS so remote peers can resolve us
    // by DID when P2P is unavailable. Best-effort: failure doesn't block startup.
    publish_local_identity_to_ipfs(&app_state, identity_path, passphrase).await;

    // Restore any previously established Olm sessions from disk
    if let Err(e) = app_state.direct_messaging.restore_sessions().await {
        tracing::warn!("Failed to restore Olm sessions: {} (starting fresh)", e);
    }

    // Restore persisted MLS group state (ratchet trees, epoch keys, group membership).
    // Must run before the re-subscribe loop so group_ids() returns the restored groups.
    match app_state
        .storage
        .fetch_mls_state(&app_state.local_did)
        .await
    {
        Ok(Some(state_bytes)) => match app_state.mls_groups.restore_in_place(&state_bytes) {
            Ok(n) => tracing::info!("Restored {} MLS group(s) from persistent storage", n),
            Err(e) => tracing::warn!(
                "Failed to restore MLS groups: {} — starting with empty group state",
                e
            ),
        },
        Ok(None) => tracing::debug!("No persisted MLS state found (first run or no groups yet)"),
        Err(e) => tracing::warn!("Failed to fetch persisted MLS state: {}", e),
    }

    // Re-subscribe to GossipSub topics for all MLS groups
    for group_id in app_state.mls_groups.group_ids() {
        let topic = format!("/variance/group/{}", group_id);
        if let Err(e) = app_state
            .node_handle
            .subscribe_to_topic(topic.clone())
            .await
        {
            tracing::warn!(
                "Failed to re-subscribe to group topic {} at startup: {}",
                topic,
                e
            );
        }
    }

    if let Err(e) = app_state
        .node_handle
        .set_local_identity(
            app_state.local_did.clone(),
            olm_identity_key,
            one_time_keys,
            generate_mls_key_package(&app_state),
        )
        .await
    {
        tracing::warn!("Failed to register local identity with P2P handler: {}", e);
    }

    // If a username was persisted from a previous session, restore it in the P2P
    // identity handler (so responses include the discriminator) and re-announce
    // to the DHT so other peers can find us by username after a restart.
    if let Some((username, disc)) = app_state
        .username_registry
        .get_username(&app_state.local_did)
    {
        if let Err(e) = app_state
            .node_handle
            .set_local_username(username.clone(), disc)
            .await
        {
            tracing::warn!("Failed to set local username in P2P handler: {}", e);
        }
        if let Err(e) = app_state.node_handle.provide_username(&username).await {
            tracing::warn!("Failed to re-publish username to DHT on startup: {}", e);
        }
    }

    // Spawn a background task to periodically clean up expired/old messages.
    let cleanup_storage = app_state.storage.clone();
    let cleanup_identity_cache = app_state.identity_cache.clone();
    let group_max_age = Duration::from_secs(config.storage.group_message_max_age_days * 86400);
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(3600));
        interval.tick().await; // skip the immediate first tick
        loop {
            interval.tick().await;

            // Evict expired offline messages
            use variance_messaging::storage::MessageStorage;
            match cleanup_storage.cleanup_expired().await {
                Ok(n) if n > 0 => {
                    tracing::info!("Cleaned up {} expired offline messages", n)
                }
                Ok(_) => {}
                Err(e) => tracing::warn!("Offline message cleanup failed: {}", e),
            }

            // Evict old group messages beyond the configured max age
            match cleanup_storage
                .cleanup_old_group_messages(group_max_age)
                .await
            {
                Ok(n) if n > 0 => {
                    tracing::info!("Cleaned up {} old group messages", n)
                }
                Ok(_) => {}
                Err(e) => tracing::warn!("Group message cleanup failed: {}", e),
            }

            // Evict expired identity cache entries (L1 + L2 + L3)
            cleanup_identity_cache.evict_expired();
        }
    });

    // Start event router to bridge P2P events to WebSocket clients
    let event_router = EventRouter::new(EventRouterDeps {
        ws_manager: app_state.ws_manager.clone(),
        direct_messaging: app_state.direct_messaging.clone(),
        mls_groups: app_state.mls_groups.clone(),
        call_manager: app_state.calls.clone(),
        signaling: app_state.signaling.clone(),
        node_handle: app_state.node_handle.clone(),
        username_registry: app_state.username_registry.clone(),
        typing: app_state.typing.clone(),
        storage: app_state.storage.clone(),
        local_did: app_state.local_did.clone(),
    });
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

/// Publish our DID document to IPFS and record the IPNS key name in the identity file.
///
/// Updates `ipns_key` in the identity file when the key name changes so the
/// next startup skips the publish if the file is already up to date.
async fn publish_local_identity_to_ipfs(
    app_state: &AppState,
    identity_path: &Path,
    passphrase: Option<&str>,
) {
    use variance_identity::did::{Did, DidDocument};

    let did_str = &app_state.local_did;

    // Build a minimal Did document from the known DID string.
    // signing_key/x25519_secret are skipped by serde, so only the document matters.
    let now = chrono::Utc::now().timestamp();
    let did = Did {
        id: did_str.clone(),
        document: DidDocument {
            id: did_str.clone(),
            authentication: vec![],
            key_agreement: vec![],
            service: vec![],
            created_at: now,
            updated_at: now,
            display_name: app_state
                .username_registry
                .get_username(did_str)
                .map(|(name, disc)| {
                    variance_identity::username::UsernameRegistry::format_username(&name, disc)
                }),
            avatar_cid: None,
            bio: None,
        },
        signing_key: None,
        x25519_secret: None,
    };

    // Store DID document in IPFS.
    let cid = match app_state.ipfs_storage.store(&did).await {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("Failed to store DID in IPFS: {}", e);
            return;
        }
    };

    // Derive a stable IPNS key name from the leading bytes of the hex-encoded DID.
    let did_hex = hex::encode(did_str.as_bytes());
    let key_name = format!("variance-{}", &did_hex[..16.min(did_hex.len())]);

    if let Err(e) = app_state.ipfs_storage.publish(&key_name, &cid).await {
        tracing::warn!("Failed to publish DID to IPNS: {}", e);
        return;
    }

    tracing::info!(
        "Published DID {} to IPFS (cid={}, ipns_key={})",
        did_str,
        cid,
        key_name
    );

    // Persist the IPNS key name to the identity file if it changed.
    match AppState::load_identity_with_passphrase(identity_path, passphrase) {
        Ok(mut identity_file) if identity_file.ipns_key.as_deref() != Some(&key_name) => {
            identity_file.ipns_key = Some(key_name);
            if let Err(e) = AppState::save_identity(identity_path, &identity_file, passphrase) {
                tracing::warn!("Failed to persist ipns_key to identity file: {}", e);
            }
        }
        Ok(_) => {} // key unchanged, no write needed
        Err(e) => tracing::warn!("Failed to reload identity for ipns_key update: {}", e),
    }
}

/// Generate a TLS-serialized MLS KeyPackage for advertising in identity responses.
///
/// Returns `None` (with a warning) if generation fails — MLS is additive, so a
/// missing key package degrades gracefully to the legacy group crypto path.
fn generate_mls_key_package(app_state: &AppState) -> Option<Vec<u8>> {
    use variance_messaging::mls::MlsGroupHandler;

    match app_state.mls_groups.generate_key_package() {
        Ok(kp) => match MlsGroupHandler::serialize_message_bytes(&kp) {
            Ok(bytes) => Some(bytes),
            Err(e) => {
                tracing::warn!("Failed to serialize MLS KeyPackage: {}", e);
                None
            }
        },
        Err(e) => {
            tracing::warn!("Failed to generate MLS KeyPackage: {}", e);
            None
        }
    }
}
