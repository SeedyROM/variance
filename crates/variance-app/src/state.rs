use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Arc;
use variance_media::{CallManager, SignalingHandler};
use variance_messaging::{
    direct::DirectMessageHandler, group::GroupMessageHandler, offline::OfflineRelayHandler,
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
    pub olm_account_pickle: String,
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

    /// Group messaging handler
    pub group_messaging: Arc<GroupMessageHandler>,

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
}

impl AppState {
    /// Load identity from file
    pub fn load_identity(identity_path: &Path) -> anyhow::Result<IdentityFile> {
        let contents = std::fs::read_to_string(identity_path)
            .map_err(|e| anyhow::anyhow!("Failed to read identity file: {}", e))?;

        let identity: IdentityFile = serde_json::from_str(&contents)
            .map_err(|e| anyhow::anyhow!("Failed to parse identity file: {}", e))?;

        Ok(identity)
    }

    /// Create a new application state from identity file
    pub fn from_identity_file(
        identity_path: &Path,
        db_path: &str,
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

        Ok(Self {
            direct_messaging: Arc::new(DirectMessageHandler::new(
                identity.did.clone(),
                signing_key.clone(),
                olm_account,
                storage.clone(),
            )),
            group_messaging: Arc::new(GroupMessageHandler::new(
                identity.did.clone(),
                signing_key.clone(),
                storage.clone(),
            )),
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
                match command {
                    variance_p2p::NodeCommand::SendIdentityRequest { response_tx, .. } => {
                        let _ = response_tx.send(Ok(
                            variance_proto::identity_proto::IdentityResponse {
                                result: None,
                                timestamp: 0,
                            },
                        ));
                    }
                    variance_p2p::NodeCommand::SendSignalingMessage { response_tx, .. } => {
                        let _ = response_tx.send(Ok(()));
                    }
                    variance_p2p::NodeCommand::PublishGroupMessage { response_tx, .. } => {
                        let _ = response_tx.send(Ok(()));
                    }
                    variance_p2p::NodeCommand::SubscribeToTopic { response_tx, .. } => {
                        let _ = response_tx.send(Ok(()));
                    }
                    variance_p2p::NodeCommand::UnsubscribeFromTopic { response_tx, .. } => {
                        let _ = response_tx.send(Ok(()));
                    }
                    variance_p2p::NodeCommand::ProvideUsername { response_tx, .. } => {
                        let _ = response_tx.send(Ok(()));
                    }
                    variance_p2p::NodeCommand::FindUsernameProviders { response_tx, .. } => {
                        let _ = response_tx.send(Ok(vec![]));
                    }
                    variance_p2p::NodeCommand::SendDirectMessage { response_tx, .. } => {
                        let _ = response_tx.send(Ok(()));
                    }
                }
            }
        });

        variance_p2p::NodeHandle::new(command_tx)
    }

    /// Create a new application state with a custom database path (for testing only)
    #[cfg(test)]
    pub fn with_db_path(local_did: String, db_path: &str) -> Self {
        let storage = Arc::new(LocalMessageStorage::new(db_path).unwrap());
        let signing_key = ed25519_dalek::SigningKey::generate(&mut rand::rngs::OsRng);
        let signaling_key = ed25519_dalek::SigningKey::generate(&mut rand::rngs::OsRng);

        Self {
            direct_messaging: Arc::new(DirectMessageHandler::new(
                local_did.clone(),
                signing_key.clone(),
                vodozemac::olm::Account::new(),
                storage.clone(),
            )),
            group_messaging: Arc::new(GroupMessageHandler::new(
                local_did.clone(),
                signing_key.clone(),
                storage.clone(),
            )),
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
        assert_eq!(Arc::strong_count(&state.storage), 5); // Shared by receipts, offline_relay, direct_messaging, group_messaging, and state itself
    }
}
