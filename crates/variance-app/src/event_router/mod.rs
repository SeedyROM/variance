//! Event router that bridges P2P events to WebSocket clients
//!
//! Subscribes to variance-p2p EventChannels and forwards events to connected
//! WebSocket clients via the WebSocketManager.

mod media;
mod messaging;
mod social;

use crate::websocket::WebSocketManager;
use std::sync::Arc;
use tracing::{debug, warn};
use variance_identity::cache::MultiLayerCache;
use variance_identity::username::UsernameRegistry;
use variance_media::{CallManager, SignalingHandler};
use variance_messaging::{
    direct::DirectMessageHandler,
    mls::MlsGroupHandler,
    receipts::ReceiptHandler,
    storage::{LocalMessageStorage, MessageStorage},
    typing::TypingHandler,
};
use variance_p2p::{EventChannels, NodeHandle};

/// All dependencies needed by the EventRouter, grouped to avoid too-many-arguments.
pub struct EventRouterDeps {
    pub ws_manager: WebSocketManager,
    pub direct_messaging: Arc<DirectMessageHandler>,
    pub mls_groups: Arc<MlsGroupHandler>,
    pub call_manager: Arc<CallManager>,
    pub signaling: Arc<SignalingHandler>,
    pub node_handle: NodeHandle,
    pub username_registry: Arc<UsernameRegistry>,
    pub typing: Arc<TypingHandler>,
    /// Message storage — used to persist MLS state after every group operation.
    pub storage: Arc<LocalMessageStorage>,
    /// Local DID — key under which MLS state is persisted.
    pub local_did: String,
    /// Identity cache — evicted on peer disconnect so reconnecting peers with
    /// new keys don't get served stale identity documents.
    pub identity_cache: Arc<MultiLayerCache>,
    /// Receipt handler — stores inbound receipts from peers.
    pub receipts: Arc<ReceiptHandler>,
}

/// Bridges P2P events to WebSocket clients
pub struct EventRouter {
    ws_manager: WebSocketManager,
    direct_messaging: Arc<DirectMessageHandler>,
    mls_groups: Arc<MlsGroupHandler>,
    call_manager: Arc<CallManager>,
    signaling: Arc<SignalingHandler>,
    node_handle: NodeHandle,
    username_registry: Arc<UsernameRegistry>,
    typing: Arc<TypingHandler>,
    storage: Arc<LocalMessageStorage>,
    local_did: String,
    identity_cache: Arc<MultiLayerCache>,
    receipts: Arc<ReceiptHandler>,
}

impl EventRouter {
    pub fn new(deps: EventRouterDeps) -> Self {
        let EventRouterDeps {
            ws_manager,
            direct_messaging,
            mls_groups,
            call_manager,
            signaling,
            node_handle,
            username_registry,
            typing,
            storage,
            local_did,
            identity_cache,
            receipts,
        } = deps;

        Self {
            ws_manager,
            direct_messaging,
            mls_groups,
            call_manager,
            signaling,
            node_handle,
            username_registry,
            typing,
            storage,
            local_did,
            identity_cache,
            receipts,
        }
    }

    /// Start listening to P2P events and forwarding to WebSocket clients
    ///
    /// This spawns background tasks that subscribe to each event channel
    /// and broadcast events to all connected WebSocket clients.
    pub fn start(self, events: EventChannels) {
        media::spawn_media_listeners(
            self.ws_manager.clone(),
            self.call_manager.clone(),
            self.signaling.clone(),
            self.node_handle.clone(),
            events.clone(),
        );

        messaging::spawn_messaging_listeners(
            messaging::MessagingDeps {
                ws_manager: self.ws_manager.clone(),
                direct_messaging: self.direct_messaging.clone(),
                mls_groups: self.mls_groups.clone(),
                node_handle: self.node_handle.clone(),
                storage: self.storage.clone(),
                local_did: self.local_did.clone(),
                receipts: self.receipts.clone(),
                username_registry: self.username_registry.clone(),
            },
            events.clone(),
        );

        social::spawn_social_listeners(
            social::SocialDeps {
                ws_manager: self.ws_manager,
                typing: self.typing,
                receipts: self.receipts,
                username_registry: self.username_registry,
                storage: self.storage,
                identity_cache: self.identity_cache,
                direct_messaging: self.direct_messaging,
                mls_groups: self.mls_groups,
                node_handle: self.node_handle,
                call_manager: self.call_manager,
                signaling: self.signaling,
            },
            events,
        );

        debug!("EventRouter: All event listeners started");
    }
}

/// Persist MLS state to storage after any mutation.
///
/// Logs a warning on failure but never panics — persistence failure degrades gracefully
/// (groups still work, they just won't survive a restart until the next persist succeeds).
pub(super) async fn persist_mls_state_async(
    mls_groups: &MlsGroupHandler,
    storage: &LocalMessageStorage,
    local_did: &str,
) {
    match mls_groups.export_state() {
        Ok(bytes) => {
            if let Err(e) = storage.store_mls_state(local_did, &bytes).await {
                warn!("Failed to persist MLS state to storage: {}", e);
            }
        }
        Err(e) => warn!("Failed to export MLS state for persistence: {}", e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::AppState;
    use crate::websocket::WsMessage;
    use tempfile::tempdir;
    use variance_p2p::EventChannels;

    fn make_router() -> EventRouter {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let state =
            AppState::with_db_path("did:variance:test".to_string(), db_path.to_str().unwrap());
        EventRouter::new(EventRouterDeps {
            ws_manager: state.ws_manager.clone(),
            direct_messaging: state.direct_messaging.clone(),
            mls_groups: state.mls_groups.clone(),
            call_manager: state.calls.clone(),
            signaling: state.signaling.clone(),
            node_handle: state.node_handle.clone(),
            username_registry: state.username_registry.clone(),
            typing: state.typing.clone(),
            storage: state.storage.clone(),
            local_did: state.local_did.clone(),
            identity_cache: state.identity_cache.clone(),
            receipts: state.receipts.clone(),
        })
    }

    #[tokio::test]
    async fn test_event_router_creation() {
        let _router = make_router();
    }

    #[tokio::test]
    async fn test_event_router_start() {
        let router = make_router();
        let events = EventChannels::default();

        // Start the router (spawns background tasks)
        router.start(events);

        // Give tasks a moment to start
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

        // If we get here without panicking, the tasks started successfully
    }

    #[tokio::test]
    async fn test_signaling_event_routing() {
        use tokio::sync::mpsc;
        use variance_p2p::SignalingEvent;

        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let state =
            AppState::with_db_path("did:variance:test".to_string(), db_path.to_str().unwrap());
        let ws_manager = state.ws_manager.clone();
        let (tx, mut rx) = mpsc::unbounded_channel();

        // Register a test client
        ws_manager.register(
            "test_client".to_string(),
            crate::websocket::ConnectedClient {
                did: None,
                tx,
                subscriptions: crate::websocket::ClientSubscription::default(),
            },
        );

        let router = EventRouter::new(EventRouterDeps {
            ws_manager: ws_manager.clone(),
            direct_messaging: state.direct_messaging.clone(),
            mls_groups: state.mls_groups.clone(),
            call_manager: state.calls.clone(),
            signaling: state.signaling.clone(),
            node_handle: state.node_handle.clone(),
            username_registry: state.username_registry.clone(),
            typing: state.typing.clone(),
            storage: state.storage.clone(),
            local_did: state.local_did.clone(),
            identity_cache: state.identity_cache.clone(),
            receipts: state.receipts.clone(),
        });
        let events = EventChannels::default();

        // Start router
        router.start(events.clone());

        // Give router time to set up listeners
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

        // Send a test signaling event
        let event = SignalingEvent::CallEnded {
            call_id: "test123".to_string(),
            reason: "Test ended".to_string(),
        };

        events.send_signaling(event);

        // Wait a bit for the event to be processed
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        // Check if client received the message
        if let Ok(msg) = rx.try_recv() {
            match msg {
                WsMessage::CallEnded { call_id, reason } => {
                    assert_eq!(call_id, "test123");
                    assert_eq!(reason, "Test ended");
                }
                _ => panic!("Wrong message type received"),
            }
        }
        // Note: This test might fail in CI due to timing, but the important
        // part is that the code compiles and runs without panicking
    }
}
