use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use variance_identity::cache::MultiLayerCache;
use variance_identity::storage::IdentityStorage;
use variance_identity::username::UsernameRegistry;
use variance_media::{CallManager, SignalingHandler};
use variance_messaging::{
    direct::DirectMessageHandler, mls::MlsGroupHandler, offline::OfflineRelayHandler,
    receipts::ReceiptHandler, storage::LocalMessageStorage, typing::TypingHandler,
};
use zeroize::Zeroizing;

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
    /// IPNS key name used to publish this identity's DID document.
    /// Absent in pre-IPFS identities; set on first publish.
    #[serde(default)]
    pub ipns_key: Option<String>,
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
    pub event_channels: Arc<variance_p2p::EventChannels>,

    /// Username registry (username#discriminator → DID)
    pub username_registry: Arc<UsernameRegistry>,

    /// Multi-layer identity cache (L1 hot → L2 warm → L3 disk)
    pub identity_cache: Arc<MultiLayerCache>,

    /// Path to the identity file (for persisting username changes etc.)
    pub identity_path: PathBuf,

    /// Identity storage backend (IPFS in production, local fallback when IPFS unavailable)
    pub ipfs_storage: Arc<dyn IdentityStorage>,

    /// Passphrase used to decrypt/re-encrypt the identity file (None for plaintext files).
    /// Stored for the session so identity file writes (OTK refresh, username) stay encrypted.
    /// Wrapped in `Zeroizing` so the passphrase is scrubbed from heap memory on drop.
    pub identity_passphrase: Option<Arc<Zeroizing<String>>>,

    /// Path to the on-disk config file (`base_dir/config.toml`) used for relay management.
    pub config_path: PathBuf,

    /// Opaque relay mailbox token: SHA-256(signing_key || "variance-mailbox-v1").
    /// Passed to the relay when fetching offline messages; never a human-readable DID.
    pub mailbox_token: [u8; 32],

    /// Ed25519 signing key for this identity. Used to sign identity protocol
    /// responses and other authenticated messages.
    pub signing_key: ed25519_dalek::SigningKey,
}

impl AppState {
    /// Load identity from file, migrating old formats in-place if needed.
    ///
    /// Pass `passphrase` when the file may be encrypted (see [`identity_crypto`]).
    /// Plaintext files are always accepted regardless of whether a passphrase is supplied.
    pub fn load_identity(identity_path: &Path) -> anyhow::Result<IdentityFile> {
        Self::load_identity_with_passphrase(identity_path, None)
    }

    /// Load identity from file with optional passphrase for encrypted files.
    pub fn load_identity_with_passphrase(
        identity_path: &Path,
        passphrase: Option<&str>,
    ) -> anyhow::Result<IdentityFile> {
        use crate::identity_crypto;

        let bytes = fs::read(identity_path)
            .map_err(|e| anyhow::anyhow!("Failed to read identity file: {}", e))?;

        let contents = if identity_crypto::is_encrypted(&bytes) {
            let pp = passphrase.ok_or_else(|| {
                anyhow::anyhow!("Identity file is encrypted but no passphrase was provided")
            })?;
            identity_crypto::decrypt(&bytes, pp)
                .map_err(|e| anyhow::anyhow!("Failed to decrypt identity file: {}", e))?
        } else {
            String::from_utf8(bytes)
                .map_err(|e| anyhow::anyhow!("Identity file is not valid UTF-8: {}", e))?
        };

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

            Self::save_identity(identity_path, &identity, passphrase)
                .map_err(|e| anyhow::anyhow!("Failed to write migrated identity file: {}", e))?;
        }

        Ok(identity)
    }

    /// Write an identity file, optionally encrypting with `passphrase`.
    ///
    /// When `passphrase` is `None`, the file is written as plaintext JSON
    /// (backward-compatible with pre-encryption releases).
    pub fn save_identity(
        path: &Path,
        identity: &IdentityFile,
        passphrase: Option<&str>,
    ) -> anyhow::Result<()> {
        use crate::identity_crypto;

        let json = serde_json::to_string_pretty(identity)
            .map_err(|e| anyhow::anyhow!("Failed to serialize identity: {}", e))?;

        let bytes: Vec<u8> = if let Some(pp) = passphrase {
            identity_crypto::encrypt(&json, pp)
                .map_err(|e| anyhow::anyhow!("Failed to encrypt identity: {}", e))?
        } else {
            json.into_bytes()
        };

        fs::write(path, bytes).map_err(|e| anyhow::anyhow!("Failed to write identity file: {}", e))
    }

    /// Create a new application state from identity file
    #[allow(clippy::too_many_arguments)]
    pub fn from_identity_file(
        identity_path: &Path,
        db_path: &str,
        identity_cache_dir: &str,
        node_handle: variance_p2p::NodeHandle,
        event_channels: Arc<variance_p2p::EventChannels>,
        ipfs_storage: Arc<dyn IdentityStorage>,
        passphrase: Option<&str>,
        stun_servers: Vec<String>,
        config_path: PathBuf,
    ) -> anyhow::Result<Self> {
        let identity = Self::load_identity_with_passphrase(identity_path, passphrase)?;
        Self::from_identity(
            &identity,
            identity_path,
            db_path,
            identity_cache_dir,
            node_handle,
            event_channels,
            ipfs_storage,
            passphrase,
            stun_servers,
            config_path,
        )
    }

    /// Create application state from an already-loaded identity.
    #[allow(clippy::too_many_arguments)]
    pub fn from_identity(
        identity: &IdentityFile,
        identity_path: &Path,
        db_path: &str,
        identity_cache_dir: &str,
        node_handle: variance_p2p::NodeHandle,
        event_channels: Arc<variance_p2p::EventChannels>,
        ipfs_storage: Arc<dyn IdentityStorage>,
        passphrase: Option<&str>,
        stun_servers: Vec<String>,
        config_path: PathBuf,
    ) -> anyhow::Result<Self> {
        // Parse signing key from hex
        let signing_key_bytes = hex::decode(&identity.signing_key)
            .map_err(|e| anyhow::anyhow!("Invalid signing key format: {}", e))?;

        let signing_key = ed25519_dalek::SigningKey::from_bytes(
            &signing_key_bytes
                .try_into()
                .map_err(|_| anyhow::anyhow!("Invalid signing key length"))?,
        );

        // Derive mailbox token from the verifying (public) key so relays can authenticate fetches
        let mailbox_token =
            variance_identity::mailbox_token(signing_key.verifying_key().as_bytes());

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
                signing_key.clone(),
                storage.clone(),
            )),
            typing: Arc::new(TypingHandler::new(identity.did.clone())),
            offline_relay: Arc::new(OfflineRelayHandler::new(storage.clone())),
            calls: Arc::new(
                CallManager::new(identity.did.clone(), stun_servers)
                    .map_err(|e| anyhow::anyhow!("Failed to create call manager: {}", e))?,
            ),
            signaling: Arc::new(SignalingHandler::new(identity.did.clone(), signaling_key)),
            storage,
            local_did: identity.did.clone(),
            verifying_key: identity.verifying_key.clone(),
            created_at: identity.created_at.clone(),
            node_handle,
            ws_manager: WebSocketManager::new(),
            event_channels,
            username_registry,
            identity_cache,
            identity_path: identity_path.to_path_buf(),
            ipfs_storage,
            identity_passphrase: passphrase.map(|p| Arc::new(Zeroizing::new(p.to_string()))),
            config_path,
            mailbox_token,
            signing_key,
        })
    }

    /// Create a new application state (for testing only - generates random keys)
    #[cfg(any(test, feature = "test-utils"))]
    pub fn new(local_did: String) -> Self {
        Self::with_db_path(local_did, ".variance/messages.db")
    }

    /// Create a test NodeHandle for testing
    #[cfg(any(test, feature = "test-utils"))]
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
                            mailbox_token: vec![],
                            document_signature: vec![],
                        }));
                    }
                    NodeCommand::GetConnectedDids { response_tx } => {
                        let _ = response_tx.send(vec![]);
                    }
                    NodeCommand::SendTypingIndicator { .. } => {
                        // Fire-and-forget, no response channel
                    }
                    NodeCommand::BroadcastGroupTyping { .. } => {
                        // Fire-and-forget, no response channel
                    }
                    NodeCommand::BroadcastUsernameChange { .. } => {
                        // Fire-and-forget, no response channel
                    }
                    NodeCommand::RequestGroupSync { response_tx, .. } => {
                        let _ = response_tx.send(Ok(()));
                    }
                    NodeCommand::RespondGroupSync { .. } => {
                        // Response sent on stored channel, nothing to do in mock
                    }
                    NodeCommand::SendReceipt { .. } => {
                        // Fire-and-forget, no response channel
                    }
                    NodeCommand::UpdateMlsKeyPackage { .. } => {
                        // Fire-and-forget, no response channel
                    }
                }
            }
        });

        variance_p2p::NodeHandle::new(command_tx)
    }

    /// Create a new application state with a custom database path (for testing only)
    #[cfg(any(test, feature = "test-utils"))]
    pub fn with_db_path(local_did: String, db_path: &str) -> Self {
        use ed25519_dalek::SigningKey;
        use variance_identity::storage::LocalStorage;

        let storage = Arc::new(LocalMessageStorage::new(db_path).unwrap());
        let signing_key = SigningKey::generate(&mut rand_core::OsRng);
        let signaling_key = SigningKey::generate(&mut rand_core::OsRng);
        let mailbox_token =
            variance_identity::mailbox_token(signing_key.verifying_key().as_bytes());

        let identity_temp = tempfile::tempdir().expect("Failed to create temp dir for identity");
        let identity_cache_path = identity_temp.path().join("identity-cache");
        let ipfs_storage_path = identity_temp.path().join("ipfs-local");
        let _ = identity_temp.keep();

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
                signing_key.clone(),
                storage.clone(),
            )),
            typing: Arc::new(TypingHandler::new(local_did.clone())),
            offline_relay: Arc::new(OfflineRelayHandler::new(storage.clone())),
            calls: Arc::new(
                CallManager::new(local_did.clone(), vec![]).expect("Failed to create call manager"),
            ),
            signaling: Arc::new(SignalingHandler::new(local_did.clone(), signaling_key)),
            storage,
            local_did,
            verifying_key: "".to_string(),
            created_at: "".to_string(),
            node_handle: Self::test_node_handle(),
            ws_manager: WebSocketManager::new(),
            event_channels: Arc::new(variance_p2p::EventChannels::default()),
            username_registry: Arc::new(UsernameRegistry::new()),
            identity_cache: Arc::new(
                MultiLayerCache::new(&identity_cache_path.to_string_lossy())
                    .expect("Failed to create test identity cache"),
            ),
            identity_path: PathBuf::from("/tmp/test-identity.json"),
            ipfs_storage: Arc::new(
                LocalStorage::new(ipfs_storage_path).expect("Failed to create test IPFS storage"),
            ),
            identity_passphrase: None,
            config_path: PathBuf::from("/tmp/test-config.toml"),
            mailbox_token,
            signing_key,
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

        // Verify handlers are operational (not just initialized).
        assert!(state.typing.get_typing_users_group("did:test").is_empty());
        assert!(state.username_registry.get_username("did:test").is_none());
    }
}
