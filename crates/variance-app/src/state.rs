use std::sync::Arc;
use variance_media::{CallManager, SignalingHandler};
use variance_messaging::{
    direct::DirectMessageHandler, group::GroupMessageHandler, offline::OfflineRelayHandler,
    receipts::ReceiptHandler, storage::LocalMessageStorage, typing::TypingHandler,
};

/// Application state shared across HTTP handlers
///
/// This holds all the core components needed for the application:
/// - Messaging handlers (direct, group, receipts, typing)
/// - Media handlers (calls, signaling)
/// - Storage
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
}

impl AppState {
    /// Create a new application state
    ///
    /// This is a simplified constructor for testing. In production, you would
    /// create the handlers with proper configuration and dependencies.
    pub fn new(local_did: String) -> Self {
        Self::with_db_path(local_did, ".variance/messages.db")
    }

    /// Create a new application state with a custom database path
    pub fn with_db_path(local_did: String, db_path: &str) -> Self {
        let storage = Arc::new(LocalMessageStorage::new(db_path).unwrap());
        let signing_key = ed25519_dalek::SigningKey::generate(&mut rand::rngs::OsRng);
        let signaling_key = ed25519_dalek::SigningKey::generate(&mut rand::rngs::OsRng);

        Self {
            direct_messaging: Arc::new(DirectMessageHandler::new(
                local_did.clone(),
                signing_key.clone(),
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
            offline_relay: Arc::new(OfflineRelayHandler::new(
                local_did.clone(),
                storage.clone(),
            )),
            calls: Arc::new(CallManager::new(local_did.clone())),
            signaling: Arc::new(SignalingHandler::new(local_did.clone(), signaling_key)),
            storage,
            local_did,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_create_state() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let state = AppState::with_db_path(
            "did:variance:test".to_string(),
            db_path.to_str().unwrap(),
        );

        assert_eq!(state.local_did, "did:variance:test");
    }

    #[test]
    fn test_state_clone() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let state = AppState::with_db_path(
            "did:variance:test".to_string(),
            db_path.to_str().unwrap(),
        );
        let cloned = state.clone();

        assert_eq!(state.local_did, cloned.local_did);
    }

    #[test]
    fn test_state_components() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let state = AppState::with_db_path(
            "did:variance:alice".to_string(),
            db_path.to_str().unwrap(),
        );

        // Verify all components are initialized
        assert_eq!(Arc::strong_count(&state.direct_messaging), 1);
        assert_eq!(Arc::strong_count(&state.calls), 1);
        assert_eq!(Arc::strong_count(&state.storage), 5); // Shared by receipts, offline_relay, direct_messaging, group_messaging, and state itself
    }
}
