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
use variance_identity::cache::MultiLayerCache;
use variance_identity::storage::{IdentityStorage, IpfsStorage, LocalStorage};
use variance_messaging::storage::{LocalMessageStorage, MessageStorage};

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

    // Load identity once — used for keypair derivation and AppState construction.
    let mut identity =
        AppState::load_identity_with_passphrase(identity_path, passphrase).map_err(|e| {
            crate::Error::App {
                message: format!("Failed to load identity: {}", e),
            }
        })?;

    let p2p_config = build_p2p_config(&config, &identity)?;

    // Create P2P node and get handle
    tracing::debug!("Creating P2P node...");
    let (mut node, node_handle) =
        variance_p2p::Node::new(p2p_config.clone()).map_err(|e| crate::Error::App {
            message: format!("Failed to create P2P node: {}", e),
        })?;

    let event_channels = Arc::new(node.events().clone());

    // Spawn P2P node in background task
    let (shutdown_tx, shutdown_rx) = tokio::sync::mpsc::channel::<()>(1);
    let p2p_config_clone = p2p_config.clone();
    let node_task = tokio::spawn(async move {
        if let Err(e) = node.listen(&p2p_config_clone).await {
            tracing::error!("Failed to start listening: {}", e);
            return Err(e);
        }
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

    tracing::debug!("Loading identity from: {}", identity_path.display());
    let app_state = AppState::from_identity(
        &identity,
        identity_path,
        config
            .storage
            .message_db_path
            .to_str()
            .ok_or_else(|| crate::Error::App {
                message: "Message DB path contains invalid UTF-8".to_string(),
            })?,
        config
            .storage
            .identity_cache_dir
            .to_str()
            .ok_or_else(|| crate::Error::App {
                message: "Identity cache dir contains invalid UTF-8".to_string(),
            })?,
        node_handle,
        event_channels.clone(),
        ipfs_storage,
        passphrase,
        config.media.stun_servers.clone(),
        config.storage.base_dir.clone(),
    )
    .map_err(|e| crate::Error::App {
        message: format!("Failed to initialize app state: {}", e),
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

    // Restore crypto state and get the Olm keys needed for P2P registration.
    let (olm_identity_key, one_time_keys) = restore_crypto_state(&app_state, &mut identity).await;

    // Register our identity and username with the P2P network.
    register_with_network(&app_state, olm_identity_key, one_time_keys).await;

    // Publish local DID document to IPFS/IPNS; updates identity.ipns_key in place.
    publish_local_identity_to_ipfs(&app_state, &mut identity).await;

    // Single identity file write — persists OTK pickle and IPNS key together.
    if let Err(e) = AppState::save_identity(identity_path, &identity, passphrase) {
        tracing::warn!("Failed to persist identity file after startup: {}", e);
    }

    let group_max_age = Duration::from_secs(config.storage.group_message_max_age_days * 86400);
    start_maintenance_task(
        app_state.storage.clone(),
        app_state.identity_cache.clone(),
        group_max_age,
    );

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
        identity_cache: app_state.identity_cache.clone(),
        receipts: app_state.receipts.clone(),
    });
    event_router.start((*event_channels).clone());
    tracing::debug!("EventRouter started");

    let router = create_router(app_state.clone());

    tracing::info!("✓ Variance node initialized successfully");

    Ok(RunningNode {
        app_state,
        router,
        shutdown_tx,
        node_task,
    })
}

/// Build the P2P node configuration from the app config.
///
/// Derives a stable libp2p PeerId from `identity`'s Ed25519 signing key so the
/// PeerId is consistent across restarts. Falls back to an ephemeral keypair if
/// the signing key can't be decoded.
fn build_p2p_config(
    config: &AppConfig,
    identity: &crate::state::IdentityFile,
) -> Result<variance_p2p::Config> {
    let keypair = hex::decode(&identity.signing_key)
        .ok()
        .and_then(variance_p2p::keypair_from_ed25519);

    if keypair.is_some() {
        tracing::debug!("Derived stable libp2p PeerId from identity key");
    } else {
        tracing::warn!(
            "Could not derive libp2p keypair from signing key; PeerId will be ephemeral"
        );
    }

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

    Ok(variance_p2p::Config {
        listen_addresses,
        bootstrap_peers,
        relay_peers,
        enable_mdns: true,
        storage_path: config.storage.base_dir.clone(),
        keypair,
        ..Default::default()
    })
}

/// Generate OTKs, restore sessions and MLS state, re-subscribe GossipSub topics.
///
/// Mutates `identity.olm_account_pickle` with the freshly generated OTKs so the
/// caller can persist everything in a single `save_identity` call.
///
/// Returns `(olm_identity_key, one_time_keys)` for subsequent `set_local_identity`.
async fn restore_crypto_state(
    state: &AppState,
    identity: &mut crate::state::IdentityFile,
) -> (Vec<u8>, Vec<Vec<u8>>) {
    // Generate initial batch of one-time pre-keys so peers can establish Olm sessions.
    state.direct_messaging.generate_one_time_keys(50).await;

    let olm_identity_key = state.direct_messaging.identity_key().to_bytes().to_vec();
    let one_time_keys = state
        .direct_messaging
        .one_time_keys()
        .await
        .values()
        .map(|k| k.to_bytes().to_vec())
        .collect::<Vec<_>>();

    // Mark keys as published so vodozemac moves them into its published pool.
    // create_inbound_session() only searches published keys — calling this is what
    // makes inbound PreKey messages decryptable.
    state
        .direct_messaging
        .mark_one_time_keys_as_published()
        .await;

    // Update the in-memory identity with the new pickle so the caller can persist once.
    match state.direct_messaging.account_pickle().await {
        Ok(pickle_json) => identity.olm_account_pickle = pickle_json,
        Err(e) => tracing::warn!("Failed to serialize Olm account pickle: {}", e),
    }

    // Restore any previously established Olm sessions from disk.
    if let Err(e) = state.direct_messaging.restore_sessions().await {
        tracing::warn!("Failed to restore Olm sessions: {} (starting fresh)", e);
    }

    // Restore persisted MLS group state (ratchet trees, epoch keys, group membership).
    // Must run before the re-subscribe loop so group_ids() returns the restored groups.
    match state.storage.fetch_mls_state(&state.local_did).await {
        Ok(Some(state_bytes)) => match state.mls_groups.restore_in_place(&state_bytes) {
            Ok(n) => tracing::info!("Restored {} MLS group(s) from persistent storage", n),
            Err(e) => tracing::warn!(
                "Failed to restore MLS groups: {} — starting with empty group state",
                e
            ),
        },
        Ok(None) => tracing::debug!("No persisted MLS state found (first run or no groups yet)"),
        Err(e) => tracing::warn!("Failed to fetch persisted MLS state: {}", e),
    }

    // Re-subscribe to GossipSub topics for all MLS groups.
    for group_id in state.mls_groups.group_ids() {
        let topic = format!("/variance/group/{}", group_id);
        if let Err(e) = state.node_handle.subscribe_to_topic(topic.clone()).await {
            tracing::warn!(
                "Failed to re-subscribe to group topic {} at startup: {}",
                topic,
                e
            );
        }
    }

    (olm_identity_key, one_time_keys)
}

/// Register local identity and username with the P2P network.
async fn register_with_network(
    state: &AppState,
    olm_identity_key: Vec<u8>,
    one_time_keys: Vec<Vec<u8>>,
) {
    let mls_key_package = generate_mls_key_package(state);

    // The KeyPackage's private HPKE init key lives only in the in-memory
    // MemoryStorage. Persist immediately so restarts don't lose it — otherwise
    // incoming Welcomes that reference this KeyPackage would fail with
    // NoMatchingKeyPackage after a restart.
    if mls_key_package.is_some() {
        match state.mls_groups.export_state() {
            Ok(bytes) => {
                if let Err(e) = state
                    .storage
                    .store_mls_state(&state.local_did, &bytes)
                    .await
                {
                    tracing::warn!(
                        "Failed to persist MLS state after KeyPackage generation: {}",
                        e
                    );
                }
            }
            Err(e) => tracing::warn!(
                "Failed to export MLS state after KeyPackage generation: {}",
                e
            ),
        }
    }

    // Build a signed Did struct to pass to the identity handler for authenticated responses.
    // Derive the PeerId from the signing key (same derivation used in build_p2p_config).
    let did_struct = {
        let peer_id = variance_p2p::keypair_from_ed25519(state.signing_key.to_bytes().to_vec())
            .map(|kp| kp.public().to_peer_id());

        match peer_id {
            Some(peer_id) => {
                match variance_identity::did::Did::from_signing_key(
                    state.local_did.clone(),
                    state.signing_key.clone(),
                    &peer_id,
                ) {
                    Ok(did) => Some(did),
                    Err(e) => {
                        tracing::warn!("Failed to create Did for identity handler: {}", e);
                        None
                    }
                }
            }
            None => {
                tracing::warn!(
                    "Could not derive PeerId from signing key; identity responses will be unsigned"
                );
                None
            }
        }
    };

    if let Err(e) = state
        .node_handle
        .set_local_identity(
            state.local_did.clone(),
            olm_identity_key,
            one_time_keys,
            mls_key_package,
            state.mailbox_token.to_vec(),
            did_struct,
        )
        .await
    {
        tracing::warn!("Failed to register local identity with P2P handler: {}", e);
    }

    // If a username was persisted from a previous session, restore it in the P2P
    // identity handler and re-announce to the DHT so other peers can find us by
    // username after a restart.
    if let Some((username, disc)) = state.username_registry.get_username(&state.local_did) {
        if let Err(e) = state
            .node_handle
            .set_local_username(username.clone(), disc)
            .await
        {
            tracing::warn!("Failed to set local username in P2P handler: {}", e);
        }
        if let Err(e) = state.node_handle.provide_username(&username).await {
            tracing::warn!("Failed to re-publish username to DHT on startup: {}", e);
        }
    }
}

/// Spawn a background task to periodically clean up expired/old messages.
fn start_maintenance_task(
    storage: Arc<LocalMessageStorage>,
    cache: Arc<MultiLayerCache>,
    group_max_age: Duration,
) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(3600));
        interval.tick().await; // skip the immediate first tick
        loop {
            interval.tick().await;

            match storage.cleanup_expired().await {
                Ok(n) if n > 0 => tracing::info!("Cleaned up {} expired offline messages", n),
                Ok(_) => {}
                Err(e) => tracing::warn!("Offline message cleanup failed: {}", e),
            }

            if group_max_age > Duration::ZERO {
                match storage.cleanup_old_group_messages(group_max_age).await {
                    Ok(n) if n > 0 => tracing::info!("Cleaned up {} old group messages", n),
                    Ok(_) => {}
                    Err(e) => tracing::warn!("Group message cleanup failed: {}", e),
                }
                match storage.cleanup_old_direct_messages(group_max_age).await {
                    Ok(n) if n > 0 => tracing::info!("Cleaned up {} old direct messages", n),
                    Ok(_) => {}
                    Err(e) => tracing::warn!("Direct message cleanup failed: {}", e),
                }
            }

            cache.evict_expired();
        }
    });
}

/// Publish our DID document to IPFS/IPNS.
///
/// Updates `identity.ipns_key` in place when the key name changes. The caller
/// persists both this field and the OTK pickle in a single `save_identity` call.
async fn publish_local_identity_to_ipfs(
    app_state: &AppState,
    identity: &mut crate::state::IdentityFile,
) {
    use variance_identity::did::{Did, DidDocument};

    let did_str = &app_state.local_did;

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
        document_signature: None,
    };

    let cid = match app_state.ipfs_storage.store(&did).await {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("Failed to store DID in IPFS: {}", e);
            return;
        }
    };

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

    // Update in-memory identity so the caller can persist everything in one write.
    if identity.ipns_key.as_deref() != Some(&key_name) {
        identity.ipns_key = Some(key_name);
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
