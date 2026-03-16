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
    use crate::websocket::{ClientSubscription, ConnectedClient, WsMessage};
    use tempfile::TempDir;
    use tokio::sync::mpsc;
    use variance_p2p::EventChannels;

    /// Shared test harness: state + WS client receiver + event channels.
    /// Keeps `TempDir` alive so the sled DB isn't dropped.
    struct TestHarness {
        _dir: TempDir,
        state: AppState,
        rx: mpsc::UnboundedReceiver<WsMessage>,
        events: EventChannels,
    }

    impl TestHarness {
        fn new() -> Self {
            Self::with_did("did:variance:test")
        }

        fn with_did(did: &str) -> Self {
            let dir = tempfile::tempdir().unwrap();
            let db_path = dir.path().join("test.db");
            let state = AppState::with_db_path(did.to_string(), db_path.to_str().unwrap());
            let (tx, rx) = mpsc::unbounded_channel();

            state.ws_manager.register(
                "test_client".to_string(),
                ConnectedClient {
                    did: None,
                    tx,
                    subscriptions: ClientSubscription::default(),
                },
            );

            let events = EventChannels::default();
            Self {
                _dir: dir,
                state,
                rx,
                events,
            }
        }

        /// Build and start the event router, returning Self for further interaction.
        fn start(self) -> Self {
            let router = EventRouter::new(EventRouterDeps {
                ws_manager: self.state.ws_manager.clone(),
                direct_messaging: self.state.direct_messaging.clone(),
                mls_groups: self.state.mls_groups.clone(),
                call_manager: self.state.calls.clone(),
                signaling: self.state.signaling.clone(),
                node_handle: self.state.node_handle.clone(),
                username_registry: self.state.username_registry.clone(),
                typing: self.state.typing.clone(),
                storage: self.state.storage.clone(),
                local_did: self.state.local_did.clone(),
                identity_cache: self.state.identity_cache.clone(),
                receipts: self.state.receipts.clone(),
            });
            router.start(self.events.clone());
            self
        }

        /// Wait for a WsMessage to arrive within the given timeout.
        async fn recv_timeout(&mut self, ms: u64) -> Option<WsMessage> {
            tokio::time::timeout(std::time::Duration::from_millis(ms), self.rx.recv())
                .await
                .ok()
                .flatten()
        }

        /// Drain all pending messages (non-blocking).
        fn drain(&mut self) -> Vec<WsMessage> {
            let mut msgs = Vec::new();
            while let Ok(msg) = self.rx.try_recv() {
                msgs.push(msg);
            }
            msgs
        }
    }

    // ── Construction & startup ───────────────────────────────────────────

    #[tokio::test]
    async fn test_event_router_creation() {
        let h = TestHarness::new();
        let _router = EventRouter::new(EventRouterDeps {
            ws_manager: h.state.ws_manager.clone(),
            direct_messaging: h.state.direct_messaging.clone(),
            mls_groups: h.state.mls_groups.clone(),
            call_manager: h.state.calls.clone(),
            signaling: h.state.signaling.clone(),
            node_handle: h.state.node_handle.clone(),
            username_registry: h.state.username_registry.clone(),
            typing: h.state.typing.clone(),
            storage: h.state.storage.clone(),
            local_did: h.state.local_did.clone(),
            identity_cache: h.state.identity_cache.clone(),
            receipts: h.state.receipts.clone(),
        });
    }

    #[tokio::test]
    async fn test_event_router_start() {
        let _h = TestHarness::new().start();
        // Give tasks a moment to start
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    // ── Signaling events → WsMessage ─────────────────────────────────────

    #[tokio::test]
    async fn test_signaling_call_ended_routing() {
        use variance_p2p::SignalingEvent;

        let mut h = TestHarness::new().start();
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;

        h.events.send_signaling(SignalingEvent::CallEnded {
            call_id: "test123".to_string(),
            reason: "Test ended".to_string(),
        });

        if let Some(msg) = h.recv_timeout(200).await {
            match msg {
                WsMessage::CallEnded { call_id, reason } => {
                    assert_eq!(call_id, "test123");
                    assert_eq!(reason, "Test ended");
                }
                other => panic!("Expected CallEnded, got {:?}", other),
            }
        }
    }

    // ── Rename events → WsMessage ────────────────────────────────────────

    #[tokio::test]
    async fn test_rename_event_routing() {
        use variance_p2p::RenameEvent;

        let mut h = TestHarness::new().start();
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;

        h.events.send_rename(RenameEvent::PeerRenamed {
            did: "did:variance:alice".to_string(),
            username: "Alice".to_string(),
            discriminator: 42,
        });

        if let Some(msg) = h.recv_timeout(200).await {
            match msg {
                WsMessage::PeerRenamed { did, display_name } => {
                    assert_eq!(did, "did:variance:alice");
                    assert_eq!(display_name, "alice#0042");
                }
                other => panic!("Expected PeerRenamed, got {:?}", other),
            }
        }

        // Also verify the username registry was updated
        let display = h
            .state
            .username_registry
            .get_display_name("did:variance:alice");
        assert_eq!(display, Some("alice#0042".to_string()));
    }

    #[tokio::test]
    async fn test_rename_persisted_to_storage() {
        use variance_messaging::storage::MessageStorage;
        use variance_p2p::RenameEvent;

        let h = TestHarness::new().start();
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;

        h.events.send_rename(RenameEvent::PeerRenamed {
            did: "did:variance:bob".to_string(),
            username: "Bob".to_string(),
            discriminator: 7,
        });

        // Wait for processing
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // Verify persisted via load_all_peer_names
        let names = h.state.storage.load_all_peer_names().await.unwrap();
        let bob_entry = names.iter().find(|(did, _, _)| did == "did:variance:bob");
        assert!(bob_entry.is_some(), "Bob should be persisted in storage");
        let (_, username, disc) = bob_entry.unwrap();
        assert_eq!(username, "Bob");
        assert_eq!(*disc, 7);
    }

    // ── Identity events → WsMessage (PeerOffline / DidCached) ────────────

    #[tokio::test]
    async fn test_peer_offline_routing() {
        use variance_p2p::IdentityEvent;

        let mut h = TestHarness::new().start();
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;

        h.events.send_identity(IdentityEvent::PeerOffline {
            did: "did:variance:alice".to_string(),
        });

        if let Some(msg) = h.recv_timeout(200).await {
            match msg {
                WsMessage::PresenceUpdated { did, online, .. } => {
                    assert_eq!(did, "did:variance:alice");
                    assert!(!online);
                }
                other => panic!("Expected PresenceUpdated (offline), got {:?}", other),
            }
        }
    }

    #[tokio::test]
    async fn test_peer_offline_evicts_identity_cache() {
        use variance_p2p::IdentityEvent;

        let h = TestHarness::new().start();

        // Pre-populate the identity cache with a minimal Did
        let did = variance_identity::did::Did {
            id: "did:variance:alice".to_string(),
            document: variance_identity::did::DidDocument {
                id: "did:variance:alice".to_string(),
                authentication: vec![],
                key_agreement: vec![],
                service: vec![],
                created_at: 0,
                updated_at: 0,
                display_name: None,
                avatar_cid: None,
                bio: None,
            },
            signing_key: None,
            x25519_secret: None,
            document_signature: None,
        };
        h.state
            .identity_cache
            .insert("did:variance:alice", did)
            .unwrap();
        assert!(h
            .state
            .identity_cache
            .get("did:variance:alice")
            .unwrap()
            .is_some());

        tokio::time::sleep(std::time::Duration::from_millis(20)).await;

        h.events.send_identity(IdentityEvent::PeerOffline {
            did: "did:variance:alice".to_string(),
        });

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // Cache should be evicted
        assert!(h
            .state
            .identity_cache
            .get("did:variance:alice")
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn test_did_cached_broadcasts_presence_online() {
        use variance_p2p::IdentityEvent;

        let mut h = TestHarness::new().start();
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;

        h.events.send_identity(IdentityEvent::DidCached {
            did: "did:variance:charlie".to_string(),
        });

        if let Some(msg) = h.recv_timeout(200).await {
            match msg {
                WsMessage::PresenceUpdated { did, online, .. } => {
                    assert_eq!(did, "did:variance:charlie");
                    assert!(online);
                }
                other => panic!("Expected PresenceUpdated (online), got {:?}", other),
            }
        }
    }

    // ── Offline message events → WsMessage ───────────────────────────────

    #[tokio::test]
    async fn test_offline_messages_received_routing() {
        use libp2p_identity::PeerId;
        use variance_p2p::OfflineMessageEvent;
        use variance_proto::messaging_proto::OfflineMessageEnvelope;

        let mut h = TestHarness::new().start();
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;

        let envelopes = vec![
            OfflineMessageEnvelope::default(),
            OfflineMessageEnvelope::default(),
            OfflineMessageEnvelope::default(),
        ];

        h.events
            .send_offline_message(OfflineMessageEvent::MessagesReceived {
                peer: PeerId::random(),
                messages: envelopes,
                has_more: false,
            });

        if let Some(msg) = h.recv_timeout(200).await {
            match msg {
                WsMessage::OfflineMessagesReceived { count } => {
                    assert_eq!(count, 3);
                }
                other => panic!("Expected OfflineMessagesReceived, got {:?}", other),
            }
        }
    }

    #[tokio::test]
    async fn test_offline_message_stored_not_forwarded() {
        use libp2p_identity::PeerId;
        use variance_p2p::OfflineMessageEvent;

        let mut h = TestHarness::new().start();
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;

        // MessageStored and FetchRequested are relay-side events that should
        // NOT produce any WsMessage to frontend clients.
        h.events
            .send_offline_message(OfflineMessageEvent::MessageStored {
                message_id: "msg-relay".to_string(),
                recipient: "did:variance:bob".to_string(),
            });

        h.events
            .send_offline_message(OfflineMessageEvent::FetchRequested {
                peer: PeerId::random(),
                mailbox_token: vec![1, 2, 3],
                limit: 10,
            });

        let msg = h.recv_timeout(100).await;
        assert!(
            msg.is_none(),
            "MessageStored/FetchRequested should not produce WsMessage"
        );
    }

    // ── Direct message events ────────────────────────────────────────────

    #[tokio::test]
    async fn test_delivery_nack_routing() {
        use libp2p_identity::PeerId;
        use variance_p2p::events::DirectMessageEvent;

        let mut h = TestHarness::new().start();
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;

        h.events
            .send_direct_message(DirectMessageEvent::DeliveryNack {
                peer: PeerId::random(),
                message_id: "msg-nack-1".to_string(),
                error: "Rate limited".to_string(),
            });

        if let Some(msg) = h.recv_timeout(200).await {
            match msg {
                WsMessage::DirectMessageNack { message_id, error } => {
                    assert_eq!(message_id, "msg-nack-1");
                    assert_eq!(error, "Rate limited");
                }
                other => panic!("Expected DirectMessageNack, got {:?}", other),
            }
        }
    }

    #[tokio::test]
    async fn test_delivery_failed_routing() {
        use variance_p2p::events::DirectMessageEvent;

        let mut h = TestHarness::new().start();
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;

        h.events
            .send_direct_message(DirectMessageEvent::DeliveryFailed {
                message_id: "msg-fail-1".to_string(),
                recipient: "did:variance:bob".to_string(),
            });

        if let Some(msg) = h.recv_timeout(200).await {
            match msg {
                WsMessage::DirectMessageStatusChanged { message_id, status } => {
                    assert_eq!(message_id, "msg-fail-1");
                    assert_eq!(status, "pending");
                }
                other => panic!("Expected DirectMessageStatusChanged, got {:?}", other),
            }
        }
    }

    #[tokio::test]
    async fn test_message_sent_is_noop() {
        use variance_p2p::events::DirectMessageEvent;

        let mut h = TestHarness::new().start();
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;

        // MessageSent is handled by the API layer directly, so the event router
        // should not produce any WsMessage for it.
        h.events
            .send_direct_message(DirectMessageEvent::MessageSent {
                message_id: "msg-sent-1".to_string(),
                recipient: "did:variance:bob".to_string(),
            });

        let msg = h.recv_timeout(100).await;
        assert!(
            msg.is_none(),
            "MessageSent should not produce a WsMessage from event router"
        );
    }

    // ── persist_mls_state_async utility ──────────────────────────────────

    #[tokio::test]
    async fn test_persist_mls_state_roundtrip() {
        use variance_messaging::storage::MessageStorage;

        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let state =
            AppState::with_db_path("did:variance:test".to_string(), db_path.to_str().unwrap());

        // Persist state (empty, but should succeed)
        persist_mls_state_async(&state.mls_groups, &state.storage, &state.local_did).await;

        // Verify state was persisted (can be retrieved)
        let stored = state
            .storage
            .fetch_mls_state(&state.local_did)
            .await
            .unwrap();
        assert!(stored.is_some(), "MLS state should be persisted");
    }

    // ── Subscription filtering ───────────────────────────────────────────

    #[tokio::test]
    async fn test_signaling_not_delivered_when_unsubscribed() {
        use variance_p2p::SignalingEvent;

        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let state =
            AppState::with_db_path("did:variance:test".to_string(), db_path.to_str().unwrap());
        let (tx, mut rx) = mpsc::unbounded_channel();

        // Register client with signaling disabled
        state.ws_manager.register(
            "no_sig_client".to_string(),
            ConnectedClient {
                did: None,
                tx,
                subscriptions: ClientSubscription {
                    signaling: false,
                    messages: true,
                    presence: true,
                },
            },
        );

        let events = EventChannels::default();
        let router = EventRouter::new(EventRouterDeps {
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
        });
        router.start(events.clone());
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;

        events.send_signaling(SignalingEvent::CallEnded {
            call_id: "c1".to_string(),
            reason: "done".to_string(),
        });

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // Should NOT receive signaling event
        assert!(
            rx.try_recv().is_err(),
            "Signaling-unsubscribed client should not receive CallEnded"
        );
    }

    #[tokio::test]
    async fn test_presence_not_delivered_when_unsubscribed() {
        use variance_p2p::IdentityEvent;

        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let state =
            AppState::with_db_path("did:variance:test".to_string(), db_path.to_str().unwrap());
        let (tx, mut rx) = mpsc::unbounded_channel();

        // Register client with presence disabled
        state.ws_manager.register(
            "no_pres_client".to_string(),
            ConnectedClient {
                did: None,
                tx,
                subscriptions: ClientSubscription {
                    signaling: true,
                    messages: true,
                    presence: false,
                },
            },
        );

        let events = EventChannels::default();
        let router = EventRouter::new(EventRouterDeps {
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
        });
        router.start(events.clone());
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;

        events.send_identity(IdentityEvent::PeerOffline {
            did: "did:variance:alice".to_string(),
        });

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // Should NOT receive presence event
        assert!(
            rx.try_recv().is_err(),
            "Presence-unsubscribed client should not receive PresenceUpdated"
        );
    }

    // ── Multiple events in sequence ──────────────────────────────────────

    #[tokio::test]
    async fn test_multiple_signaling_events_in_sequence() {
        use variance_p2p::SignalingEvent;

        let mut h = TestHarness::new().start();
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;

        for i in 0..5 {
            h.events.send_signaling(SignalingEvent::CallEnded {
                call_id: format!("call-{}", i),
                reason: format!("Reason {}", i),
            });
        }

        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        let msgs = h.drain();
        let call_ended_count = msgs
            .iter()
            .filter(|m| matches!(m, WsMessage::CallEnded { .. }))
            .count();
        assert_eq!(call_ended_count, 5, "Should receive all 5 CallEnded events");
    }

    #[tokio::test]
    async fn test_mixed_event_types() {
        use variance_p2p::{RenameEvent, SignalingEvent, TypingEvent};

        let mut h = TestHarness::new().start();
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;

        h.events.send_signaling(SignalingEvent::CallEnded {
            call_id: "c1".to_string(),
            reason: "bye".to_string(),
        });
        h.events.send_typing(TypingEvent::IndicatorReceived {
            sender_did: "did:variance:alice".to_string(),
            recipient: "did:variance:test".to_string(),
            is_typing: true,
        });
        h.events.send_rename(RenameEvent::PeerRenamed {
            did: "did:variance:bob".to_string(),
            username: "Bobby".to_string(),
            discriminator: 99,
        });

        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        let msgs = h.drain();
        assert!(
            msgs.iter()
                .any(|m| matches!(m, WsMessage::CallEnded { .. })),
            "Should have CallEnded"
        );
        assert!(
            msgs.iter()
                .any(|m| matches!(m, WsMessage::TypingStarted { .. })),
            "Should have TypingStarted"
        );
        assert!(
            msgs.iter()
                .any(|m| matches!(m, WsMessage::PeerRenamed { .. })),
            "Should have PeerRenamed"
        );
    }
}
