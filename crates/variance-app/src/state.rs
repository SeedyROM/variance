use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use variance_identity::cache::MultiLayerCache;
use variance_identity::username::UsernameRegistry;
use variance_media::{CallManager, SignalingHandler};
use variance_messaging::{
    direct::DirectMessageHandler, mls::MlsGroupHandler, offline::OfflineRelayHandler,
    receipts::ReceiptHandler, storage::LocalMessageStorage, typing::TypingHandler,
};

use crate::websocket::WebSocketManager;

/// Identity file format (DID + signing keys)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdentityFile {
    pub did: String,
    pub signing_key: String,
    pub verifying_key: String,
    pub signaling_key: String,
    /// JSON-serialized vodozemac `AccountPickle` for the Olm account.
    ///
    /// Added in the vodozemac migration. Absent in old files; `load_identity`
    /// auto-migrates by generating a fresh account and rewriting the file.
    #[serde(default)]
    pub olm_account_pickle: String,
    /// Registered username (lowercase, without discriminator).
    /// `None` if the user hasn't registered a username yet.
    #[serde(default)]
    pub username: Option<String>,
    /// 4-digit discriminator (1–9999) paired with username.
    #[serde(default)]
    pub discriminator: Option<u32>,
    pub created_at: String,
}

/// Application state shared across HTTP handlers
///
/// This holds all the core components needed for the application:
/// - Messaging handlers (direct, group, receipts, typing)
/// - Media handlers (calls, signaling)
/// - Storage
/// - P2P node handle for network communication
#[derive(Clone)]
pub struct AppState {
    /// Direct messaging handler
    pub direct_messaging: Arc<DirectMessageHandler>,

    /// MLS group handler (RFC 9420 path)
    pub mls_groups: Arc<MlsGroupHandler>,

    /// Read receipt handler
    pub receipts: Arc<ReceiptHandler>,

    /// Typing indicator handler
    pub typing: Arc<TypingHandler>,

    /// Offline relay handler
    pub offline_relay: Arc<OfflineRelayHandler>,

    /// Call manager
    pub calls: Arc<CallManager>,

    /// WebRTC signaling handler
    pub signaling: Arc<SignalingHandler>,

    /// Message storage
    pub storage: Arc<LocalMessageStorage>,

    /// Local DID
    pub local_did: String,

    /// Hex-encoded verifying key for this identity
    pub verifying_key: String,

    /// RFC3339 timestamp when this identity was created
    pub created_at: String,

    /// P2P node handle for sending messages over the network
    pub node_handle: variance_p2p::NodeHandle,

    /// WebSocket connection manager
    pub ws_manager: WebSocketManager,

    /// P2P event channels for real-time updates
    pub event_channels: Option<Arc<variance_p2p::EventChannels>>,

    /// Username registry (username#discriminator → DID)
    pub username_registry: Arc<UsernameRegistry>,

    /// Multi-layer identity cache (L1 hot → L2 warm → L3 disk)
    pub identity_cache: Arc<MultiLayerCache>,

    /// Path to the identity file (for persisting username changes etc.)
    pub identity_path: PathBuf,
}

impl AppState {
    /// Load identity from file, migrating old formats in-place if needed.
    pub fn load_identity(identity_path: &Path) -> anyhow::Result<IdentityFile> {
        let contents = fs::read_to_string(identity_path)
            .map_err(|e| anyhow::anyhow!("Failed to read identity file: {}", e))?;

        let mut identity: IdentityFile = serde_json::from_str(&contents)
            .map_err(|e| anyhow::anyhow!("Failed to parse identity file: {}", e))?;

        // Migrate pre-vodozemac identity files that lack an Olm account pickle.
        // The Olm account is not derived from the mnemonic, so we generate a fresh
        // one. The user's DID is preserved (it comes from the signing key).
        if identity.olm_account_pickle.is_empty() {
            tracing::warn!(
                "Identity file is missing olm_account_pickle (pre-vodozemac format); \
                 generating a fresh Olm account and migrating the file"
            );
            let account = vodozemac::olm::Account::new();
            identity.olm_account_pickle = serde_json::to_string(&account.pickle())
                .map_err(|e| anyhow::anyhow!("Failed to serialize Olm account: {}", e))?;

            let migrated = serde_json::to_string_pretty(&identity)
                .map_err(|e| anyhow::anyhow!("Failed to serialize migrated identity: {}", e))?;
            fs::write(identity_path, migrated)
                .map_err(|e| anyhow::anyhow!("Failed to write migrated identity file: {}", e))?;
        }

        Ok(identity)
    }

    /// Create a new application state from identity file
    pub fn from_identity_file(
        identity_path: &Path,
        db_path: &str,
        identity_cache_dir: &str,
        node_handle: variance_p2p::NodeHandle,
        event_channels: Option<Arc<variance_p2p::EventChannels>>,
    ) -> anyhow::Result<Self> {
        let identity = Self::load_identity(identity_path)?;

        // Parse signing key from hex
        let signing_key_bytes = hex::decode(&identity.signing_key)
            .map_err(|e| anyhow::anyhow!("Invalid signing key format: {}", e))?;

        let signing_key = ed25519_dalek::SigningKey::from_bytes(
            &signing_key_bytes
                .try_into()
                .map_err(|_| anyhow::anyhow!("Invalid signing key length"))?,
        );

        // Parse signaling key from hex
        let signaling_key_bytes = hex::decode(&identity.signaling_key)
            .map_err(|e| anyhow::anyhow!("Invalid signaling key format: {}", e))?;

        let signaling_key = ed25519_dalek::SigningKey::from_bytes(
            &signaling_key_bytes
                .try_into()
                .map_err(|_| anyhow::anyhow!("Invalid signaling key length"))?,
        );

        // Restore the Olm account from its persisted pickle.
        let olm_pickle: vodozemac::olm::AccountPickle =
            serde_json::from_str(&identity.olm_account_pickle)
                .map_err(|e| anyhow::anyhow!("Failed to deserialize Olm account: {}", e))?;
        let olm_account = vodozemac::olm::Account::from_pickle(olm_pickle);

        let storage = Arc::new(LocalMessageStorage::new(db_path)?);

        // Build identity cache
        let identity_cache = Arc::new(
            MultiLayerCache::new(identity_cache_dir)
                .map_err(|e| anyhow::anyhow!("Failed to create identity cache: {}", e))?,
        );

        // Build username registry and seed with persisted username if present
        let username_registry = Arc::new(UsernameRegistry::new());
        if let (Some(ref username), Some(discriminator)) =
            (&identity.username, identity.discriminator)
        {
            username_registry
                .register_with_discriminator(username.clone(), discriminator, identity.did.clone())
                .map_err(|e| anyhow::anyhow!("Failed to seed username registry: {}", e))?;
        }

        Ok(Self {
            direct_messaging: Arc::new(DirectMessageHandler::new(
                identity.did.clone(),
                signing_key.clone(),
                olm_account,
                storage.clone(),
            )),
            mls_groups: Arc::new(
                MlsGroupHandler::new(identity.did.clone(), &signing_key)
                    .map_err(|e| anyhow::anyhow!("Failed to create MLS group handler: {}", e))?,
            ),
            receipts: Arc::new(ReceiptHandler::new(
                identity.did.clone(),
                signing_key,
                storage.clone(),
            )),
            typing: Arc::new(TypingHandler::new(identity.did.clone())),
            offline_relay: Arc::new(OfflineRelayHandler::new(
                identity.did.clone(),
                storage.clone(),
            )),
            calls: Arc::new(
                CallManager::new(
                    identity.did.clone(),
                    vec!["stun:stun.l.google.com:19302".to_string()],
                )
                .map_err(|e| anyhow::anyhow!("Failed to create call manager: {}", e))?,
            ),
            signaling: Arc::new(SignalingHandler::new(identity.did.clone(), signaling_key)),
            storage,
            local_did: identity.did,
            verifying_key: identity.verifying_key,
            created_at: identity.created_at,
            node_handle,
            ws_manager: WebSocketManager::new(),
            event_channels,
            username_registry,
            identity_cache,
            identity_path: identity_path.to_path_buf(),
        })
    }

    /// Create a new application state (for testing only - generates random keys)
    #[cfg(test)]
    pub fn new(local_did: String) -> Self {
        Self::with_db_path(local_did, ".variance/messages.db")
    }

    /// Create a test NodeHandle for testing
    #[cfg(test)]
    fn test_node_handle() -> variance_p2p::NodeHandle {
        let (command_tx, mut command_rx) = tokio::sync::mpsc::channel(100);

        // Spawn a background task to receive and respond to commands
        tokio::spawn(async move {
            while let Some(command) = command_rx.recv().await {
                // Respond to all commands with success

                use variance_p2p::NodeCommand;
                match command {
                    NodeCommand::SendIdentityRequest { response_tx, .. } => {
                        let _ = response_tx.send(Ok(
                            variance_proto::identity_proto::IdentityResponse {
                                result: None,
                                timestamp: 0,
                            },
                        ));
                    }
                    NodeCommand::SendSignalingMessage { response_tx, .. } => {
                        let _ = response_tx.send(Ok(()));
                    }
                    NodeCommand::PublishGroupMessage { response_tx, .. } => {
                        let _ = response_tx.send(Ok(()));
                    }
                    NodeCommand::SubscribeToTopic { response_tx, .. } => {
                        let _ = response_tx.send(Ok(()));
                    }
                    NodeCommand::UnsubscribeFromTopic { response_tx, .. } => {
                        let _ = response_tx.send(Ok(()));
                    }
                    NodeCommand::ProvideUsername { response_tx, .. } => {
                        let _ = response_tx.send(Ok(()));
                    }
                    NodeCommand::FindUsernameProviders { response_tx, .. } => {
                        let _ = response_tx.send(Ok(vec![]));
                    }
                    NodeCommand::SendDirectMessage { response_tx, .. } => {
                        let _ = response_tx.send(Ok(()));
                    }
                    NodeCommand::SetLocalIdentity { .. } => {
                        // No response channel, nothing to do
                    }
                    NodeCommand::UpdateOneTimeKeys { .. } => {
                        // No response channel, nothing to do
                    }
                    NodeCommand::SetLocalUsername { .. } => {
                        // No response channel, nothing to do
                    }
                    NodeCommand::ResolveIdentityByDid { response_tx, .. } => {
                        use variance_proto::identity_proto;

                        let _ = response_tx.send(Ok(identity_proto::IdentityFound {
                            did_document: None,
                            ipns_key: None,
                            multiaddrs: vec![],
                            discriminator: None,
                            olm_identity_key: vec![],
                            one_time_keys: vec![],
                            mls_key_package: None,
                            username: None,
                        }));
                    }
                    NodeCommand::GetConnectedDids { response_tx } => {
                        let _ = response_tx.send(vec![]);
                    }
                    NodeCommand::SendTypingIndicator { .. } => {
                        // Fire-and-forget, no response channel
                    }
                    NodeCommand::BroadcastUsernameChange { .. } => {
                        // Fire-and-forget, no response channel
                    }
                }
            }
        });

        variance_p2p::NodeHandle::new(command_tx)
    }

    /// Create a new application state with a custom database path (for testing only)
    #[cfg(test)]
    pub fn with_db_path(local_did: String, db_path: &str) -> Self {
        use ed25519_dalek::SigningKey;

        let storage = Arc::new(LocalMessageStorage::new(db_path).unwrap());
        let signing_key = SigningKey::generate(&mut rand::rngs::OsRng);
        let signaling_key = SigningKey::generate(&mut rand::rngs::OsRng);

        Self {
            direct_messaging: Arc::new(DirectMessageHandler::new(
                local_did.clone(),
                signing_key.clone(),
                vodozemac::olm::Account::new(),
                storage.clone(),
            )),
            mls_groups: Arc::new(
                MlsGroupHandler::new(local_did.clone(), &signing_key)
                    .expect("Failed to create MLS group handler"),
            ),
            receipts: Arc::new(ReceiptHandler::new(
                local_did.clone(),
                signing_key,
                storage.clone(),
            )),
            typing: Arc::new(TypingHandler::new(local_did.clone())),
            offline_relay: Arc::new(OfflineRelayHandler::new(local_did.clone(), storage.clone())),
            calls: Arc::new(
                CallManager::new(
                    local_did.clone(),
                    vec!["stun:stun.l.google.com:19302".to_string()],
                )
                .expect("Failed to create call manager"),
            ),
            signaling: Arc::new(SignalingHandler::new(local_did.clone(), signaling_key)),
            storage,
            local_did,
            verifying_key: "".to_string(),
            created_at: "".to_string(),
            node_handle: Self::test_node_handle(),
            ws_manager: WebSocketManager::new(),
            event_channels: None,
            username_registry: Arc::new(UsernameRegistry::new()),
            identity_cache: Arc::new({
                let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
                let temp_dir_path = temp_dir.path().join("identity-cache");
                // Keep the temp dir alive for the lifetime of the cache (test scope)
                let _ = temp_dir.keep();
                MultiLayerCache::new(&temp_dir_path.to_string_lossy())
                    .expect("Failed to create test identity cache")
            }),
            identity_path: PathBuf::from("/tmp/test-identity.json"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_create_state() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let state =
            AppState::with_db_path("did:variance:test".to_string(), db_path.to_str().unwrap());

        assert_eq!(state.local_did, "did:variance:test");
    }

    #[tokio::test]
    async fn test_state_clone() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let state =
            AppState::with_db_path("did:variance:test".to_string(), db_path.to_str().unwrap());
        let cloned = state.clone();

        assert_eq!(state.local_did, cloned.local_did);
    }

    #[tokio::test]
    async fn test_state_components() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let state =
            AppState::with_db_path("did:variance:alice".to_string(), db_path.to_str().unwrap());

        // Verify all components are initialized
        assert_eq!(Arc::strong_count(&state.direct_messaging), 1);
        assert_eq!(Arc::strong_count(&state.calls), 1);
        assert_eq!(Arc::strong_count(&state.mls_groups), 1);
        assert_eq!(Arc::strong_count(&state.storage), 4); // Shared by receipts, offline_relay, direct_messaging, and state itself
    }
}
