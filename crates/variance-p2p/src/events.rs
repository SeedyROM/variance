//! Event types for P2P protocols
//!
//! Provides event channels for protocol events that the application layer can subscribe to.

use libp2p::PeerId;
use tokio::sync::broadcast;
use variance_proto::identity_proto::{IdentityRequest, IdentityResponse};
use variance_proto::media_proto::SignalingMessage;
use variance_proto::messaging_proto::{
    DirectMessage, GroupMessage, OfflineMessageEnvelope, ReadReceipt,
};

/// Events from the identity protocol
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum IdentityEvent {
    /// Received an identity request from a peer
    RequestReceived {
        peer: PeerId,
        request: IdentityRequest,
    },

    /// Received an identity response from a peer
    ResponseReceived {
        peer: PeerId,
        response: IdentityResponse,
    },

    /// DID was cached locally
    DidCached { did: String },

    /// Peer went offline (connection closed and DID mapping removed)
    PeerOffline { did: String },
}

/// Events from the offline message protocol
#[derive(Debug, Clone)]
pub enum OfflineMessageEvent {
    /// Received a fetch request from a peer
    FetchRequested {
        peer: PeerId,
        mailbox_token: Vec<u8>,
        limit: u32,
    },

    /// Received offline messages (as the recipient)
    MessagesReceived {
        peer: PeerId,
        messages: Vec<OfflineMessageEnvelope>,
        has_more: bool,
    },

    /// Successfully stored an offline message for relay
    MessageStored {
        message_id: String,
        recipient: String,
    },
}

/// Events from the WebRTC signaling protocol
#[derive(Debug, Clone)]
pub enum SignalingEvent {
    /// Received an offer to start a call
    OfferReceived {
        peer: PeerId,
        call_id: String,
        message: SignalingMessage,
    },

    /// Received an answer to our call offer
    AnswerReceived {
        peer: PeerId,
        call_id: String,
        message: SignalingMessage,
    },

    /// Received an ICE candidate
    IceCandidateReceived {
        peer: PeerId,
        call_id: String,
        message: SignalingMessage,
    },

    /// Received a call control message (ring, accept, reject, hangup, etc.)
    ControlReceived {
        peer: PeerId,
        call_id: String,
        message: SignalingMessage,
    },

    /// Call ended
    CallEnded { call_id: String, reason: String },
}

/// Events from direct messaging
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum DirectMessageEvent {
    /// Received a direct message
    MessageReceived {
        peer: PeerId,
        message: DirectMessage,
    },

    /// Message sent successfully
    MessageSent {
        message_id: String,
        recipient: String,
    },

    /// Remote peer rejected delivery (e.g. rate limited). Sender should retry.
    DeliveryNack {
        peer: PeerId,
        message_id: String,
        error: String,
    },

    /// Outbound message delivery failed (peer unreachable after send was initiated).
    /// The `message` is included so the app layer can re-queue it as pending.
    DeliveryFailed {
        message_id: String,
        recipient: String,
    },
}

/// Events from group messaging
#[derive(Debug, Clone)]
pub enum GroupMessageEvent {
    /// Received a group message
    MessageReceived { message: GroupMessage },

    /// Message sent successfully
    MessageSent {
        message_id: String,
        group_id: String,
    },
}

/// Events from typing indicators
#[derive(Debug, Clone)]
pub enum TypingEvent {
    /// Received a typing indicator from a peer
    IndicatorReceived {
        sender_did: String,
        /// Peer DID for direct messages, or "group:{id}" for group chats
        recipient: String,
        is_typing: bool,
    },
}

/// Events from read receipt delivery
#[derive(Debug, Clone)]
pub enum ReceiptEvent {
    /// A peer sent us a read receipt for one of our messages.
    Received { receipt: ReadReceipt },
}

/// Events from username rename notifications
#[derive(Debug, Clone)]
pub enum RenameEvent {
    /// A connected peer changed their username.
    PeerRenamed {
        did: String,
        username: String,
        discriminator: u32,
    },
}

/// Events from group history sync
#[derive(Debug, Clone)]
pub enum GroupSyncEvent {
    /// Received group messages from a peer in response to our sync request
    SyncReceived {
        group_id: String,
        messages: Vec<GroupMessage>,
        has_more: bool,
    },

    /// Inbound sync request from a peer wanting our messages
    SyncRequested {
        peer: PeerId,
        group_id: String,
        requester_did: String,
        since_timestamp: i64,
        limit: u32,
        request_id: libp2p::request_response::InboundRequestId,
    },

    /// Sync request failed
    SyncFailed { group_id: String, error: String },
}

/// Combined event type for all protocols
#[derive(Debug, Clone)]
pub enum P2pEvent {
    Identity(IdentityEvent),
    OfflineMessage(OfflineMessageEvent),
    Signaling(SignalingEvent),
    DirectMessage(DirectMessageEvent),
    GroupMessage(GroupMessageEvent),
    Typing(TypingEvent),
    Rename(RenameEvent),
    GroupSync(GroupSyncEvent),
}

/// Event channels for protocol events
#[derive(Clone)]
pub struct EventChannels {
    pub identity: broadcast::Sender<IdentityEvent>,
    pub offline_messages: broadcast::Sender<OfflineMessageEvent>,
    pub signaling: broadcast::Sender<SignalingEvent>,
    pub direct_messages: broadcast::Sender<DirectMessageEvent>,
    pub group_messages: broadcast::Sender<GroupMessageEvent>,
    pub typing: broadcast::Sender<TypingEvent>,
    pub rename: broadcast::Sender<RenameEvent>,
    pub group_sync: broadcast::Sender<GroupSyncEvent>,
    pub receipts: broadcast::Sender<ReceiptEvent>,
}

impl EventChannels {
    /// Create new event channels with the given buffer size
    pub fn new(buffer_size: usize) -> Self {
        let (identity_tx, _) = broadcast::channel(buffer_size);
        let (offline_tx, _) = broadcast::channel(buffer_size);
        let (signaling_tx, _) = broadcast::channel(buffer_size);
        let (direct_tx, _) = broadcast::channel(buffer_size);
        let (group_tx, _) = broadcast::channel(buffer_size);
        let (typing_tx, _) = broadcast::channel(buffer_size);
        let (rename_tx, _) = broadcast::channel(buffer_size);
        let (group_sync_tx, _) = broadcast::channel(buffer_size);
        let (receipts_tx, _) = broadcast::channel(buffer_size);

        Self {
            identity: identity_tx,
            offline_messages: offline_tx,
            signaling: signaling_tx,
            direct_messages: direct_tx,
            group_messages: group_tx,
            typing: typing_tx,
            rename: rename_tx,
            group_sync: group_sync_tx,
            receipts: receipts_tx,
        }
    }

    /// Subscribe to identity events
    pub fn subscribe_identity(&self) -> broadcast::Receiver<IdentityEvent> {
        self.identity.subscribe()
    }

    /// Subscribe to offline message events
    pub fn subscribe_offline_messages(&self) -> broadcast::Receiver<OfflineMessageEvent> {
        self.offline_messages.subscribe()
    }

    /// Subscribe to signaling events
    pub fn subscribe_signaling(&self) -> broadcast::Receiver<SignalingEvent> {
        self.signaling.subscribe()
    }

    /// Subscribe to direct message events
    pub fn subscribe_direct_messages(&self) -> broadcast::Receiver<DirectMessageEvent> {
        self.direct_messages.subscribe()
    }

    /// Subscribe to group message events
    pub fn subscribe_group_messages(&self) -> broadcast::Receiver<GroupMessageEvent> {
        self.group_messages.subscribe()
    }

    /// Subscribe to typing events
    pub fn subscribe_typing(&self) -> broadcast::Receiver<TypingEvent> {
        self.typing.subscribe()
    }

    /// Subscribe to rename events
    pub fn subscribe_rename(&self) -> broadcast::Receiver<RenameEvent> {
        self.rename.subscribe()
    }

    /// Subscribe to group sync events
    pub fn subscribe_group_sync(&self) -> broadcast::Receiver<GroupSyncEvent> {
        self.group_sync.subscribe()
    }

    /// Subscribe to receipt events
    pub fn subscribe_receipts(&self) -> broadcast::Receiver<ReceiptEvent> {
        self.receipts.subscribe()
    }

    /// Send an identity event
    pub fn send_identity(&self, event: IdentityEvent) {
        let _ = self.identity.send(event);
    }

    /// Send an offline message event
    pub fn send_offline_message(&self, event: OfflineMessageEvent) {
        let _ = self.offline_messages.send(event);
    }

    /// Send a signaling event
    pub fn send_signaling(&self, event: SignalingEvent) {
        let _ = self.signaling.send(event);
    }

    /// Send a direct message event
    pub fn send_direct_message(&self, event: DirectMessageEvent) {
        let _ = self.direct_messages.send(event);
    }

    /// Send a group message event
    pub fn send_group_message(&self, event: GroupMessageEvent) {
        let _ = self.group_messages.send(event);
    }

    /// Send a typing event
    pub fn send_typing(&self, event: TypingEvent) {
        let _ = self.typing.send(event);
    }

    /// Send a rename event
    pub fn send_rename(&self, event: RenameEvent) {
        let _ = self.rename.send(event);
    }

    /// Send a group sync event
    pub fn send_group_sync(&self, event: GroupSyncEvent) {
        let _ = self.group_sync.send(event);
    }

    /// Send a receipt event
    pub fn send_receipt(&self, event: ReceiptEvent) {
        let _ = self.receipts.send(event);
    }
}

impl Default for EventChannels {
    fn default() -> Self {
        Self::new(100)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_create_channels() {
        let channels = EventChannels::new(10);
        let _rx = channels.subscribe_identity();
    }

    #[tokio::test]
    async fn test_identity_event() {
        let channels = EventChannels::new(10);
        let mut rx = channels.subscribe_identity();

        let event = IdentityEvent::DidCached {
            did: "did:peer:123".to_string(),
        };

        channels.send_identity(event.clone());

        let received = rx.recv().await.unwrap();
        match received {
            IdentityEvent::DidCached { did } => {
                assert_eq!(did, "did:peer:123");
            }
            _ => panic!("Wrong event type"),
        }
    }

    #[tokio::test]
    async fn test_offline_message_event() {
        let channels = EventChannels::new(10);
        let mut rx = channels.subscribe_offline_messages();

        let event = OfflineMessageEvent::MessageStored {
            message_id: "msg123".to_string(),
            recipient: "did:peer:bob".to_string(),
        };

        channels.send_offline_message(event);

        let received = rx.recv().await.unwrap();
        match received {
            OfflineMessageEvent::MessageStored {
                message_id,
                recipient,
            } => {
                assert_eq!(message_id, "msg123");
                assert_eq!(recipient, "did:peer:bob");
            }
            _ => panic!("Wrong event type"),
        }
    }

    #[tokio::test]
    async fn test_signaling_event() {
        let channels = EventChannels::new(10);
        let mut rx = channels.subscribe_signaling();

        let event = SignalingEvent::CallEnded {
            call_id: "call123".to_string(),
            reason: "User hung up".to_string(),
        };

        channels.send_signaling(event);

        let received = rx.recv().await.unwrap();
        match received {
            SignalingEvent::CallEnded { call_id, reason } => {
                assert_eq!(call_id, "call123");
                assert_eq!(reason, "User hung up");
            }
            _ => panic!("Wrong event type"),
        }
    }

    #[tokio::test]
    async fn test_multiple_subscribers() {
        let channels = EventChannels::new(10);
        let mut rx1 = channels.subscribe_identity();
        let mut rx2 = channels.subscribe_identity();

        let event = IdentityEvent::DidCached {
            did: "did:peer:123".to_string(),
        };

        channels.send_identity(event);

        // Both subscribers should receive the event
        let _ = rx1.recv().await.unwrap();
        let _ = rx2.recv().await.unwrap();
    }

    #[tokio::test]
    async fn test_delivery_failed_event() {
        let channels = EventChannels::new(10);
        let mut rx = channels.subscribe_direct_messages();

        channels.send_direct_message(DirectMessageEvent::DeliveryFailed {
            message_id: "msg-001".to_string(),
            recipient: "did:variance:bob".to_string(),
        });

        let received = rx.recv().await.unwrap();
        match received {
            DirectMessageEvent::DeliveryFailed {
                message_id,
                recipient,
            } => {
                assert_eq!(message_id, "msg-001");
                assert_eq!(recipient, "did:variance:bob");
            }
            _ => panic!("Expected DeliveryFailed event"),
        }
    }

    #[tokio::test]
    async fn test_delivery_failed_isolated_from_identity() {
        let channels = EventChannels::new(10);
        let mut identity_rx = channels.subscribe_identity();
        let mut dm_rx = channels.subscribe_direct_messages();

        // Send a DeliveryFailed event
        channels.send_direct_message(DirectMessageEvent::DeliveryFailed {
            message_id: "msg-002".to_string(),
            recipient: "did:variance:bob".to_string(),
        });

        // DM subscriber should receive it
        let received = dm_rx.recv().await.unwrap();
        assert!(matches!(
            received,
            DirectMessageEvent::DeliveryFailed { .. }
        ));

        // Identity subscriber should NOT receive anything
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(50), identity_rx.recv())
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn test_group_sync_event() {
        let channels = EventChannels::new(10);
        let mut rx = channels.group_sync.subscribe();

        channels.send_group_sync(GroupSyncEvent::SyncReceived {
            group_id: "group-123".to_string(),
            messages: vec![],
            has_more: false,
        });

        let received = rx.recv().await.unwrap();
        match received {
            GroupSyncEvent::SyncReceived {
                group_id,
                messages,
                has_more,
            } => {
                assert_eq!(group_id, "group-123");
                assert!(messages.is_empty());
                assert!(!has_more);
            }
            _ => panic!("Wrong event type"),
        }
    }

    #[tokio::test]
    async fn test_group_sync_failed_event() {
        let channels = EventChannels::new(10);
        let mut rx = channels.group_sync.subscribe();

        channels.send_group_sync(GroupSyncEvent::SyncFailed {
            group_id: "group-456".to_string(),
            error: "Peer not found".to_string(),
        });

        let received = rx.recv().await.unwrap();
        assert!(matches!(received, GroupSyncEvent::SyncFailed { .. }));
    }

    // ── New tests for previously uncovered event channels ──

    #[tokio::test]
    async fn group_message_event_received() {
        let channels = EventChannels::new(10);
        let mut rx = channels.subscribe_group_messages();

        channels.send_group_message(GroupMessageEvent::MessageReceived {
            message: GroupMessage {
                id: "gm-001".to_string(),
                group_id: "group-1".to_string(),
                ..Default::default()
            },
        });

        let received = rx.recv().await.unwrap();
        match received {
            GroupMessageEvent::MessageReceived { message } => {
                assert_eq!(message.id, "gm-001");
                assert_eq!(message.group_id, "group-1");
            }
            _ => panic!("Expected MessageReceived"),
        }
    }

    #[tokio::test]
    async fn group_message_sent_event() {
        let channels = EventChannels::new(10);
        let mut rx = channels.subscribe_group_messages();

        channels.send_group_message(GroupMessageEvent::MessageSent {
            message_id: "gm-002".to_string(),
            group_id: "group-2".to_string(),
        });

        let received = rx.recv().await.unwrap();
        match received {
            GroupMessageEvent::MessageSent {
                message_id,
                group_id,
            } => {
                assert_eq!(message_id, "gm-002");
                assert_eq!(group_id, "group-2");
            }
            _ => panic!("Expected MessageSent"),
        }
    }

    #[tokio::test]
    async fn typing_event_channel() {
        let channels = EventChannels::new(10);
        let mut rx = channels.subscribe_typing();

        channels.send_typing(TypingEvent::IndicatorReceived {
            sender_did: "did:variance:alice".to_string(),
            recipient: "did:variance:bob".to_string(),
            is_typing: true,
        });

        let received = rx.recv().await.unwrap();
        match received {
            TypingEvent::IndicatorReceived {
                sender_did,
                recipient,
                is_typing,
            } => {
                assert_eq!(sender_did, "did:variance:alice");
                assert_eq!(recipient, "did:variance:bob");
                assert!(is_typing);
            }
        }
    }

    #[tokio::test]
    async fn rename_event_channel() {
        let channels = EventChannels::new(10);
        let mut rx = channels.subscribe_rename();

        channels.send_rename(RenameEvent::PeerRenamed {
            did: "did:variance:alice".to_string(),
            username: "alice_new".to_string(),
            discriminator: 1234,
        });

        let received = rx.recv().await.unwrap();
        match received {
            RenameEvent::PeerRenamed {
                did,
                username,
                discriminator,
            } => {
                assert_eq!(did, "did:variance:alice");
                assert_eq!(username, "alice_new");
                assert_eq!(discriminator, 1234);
            }
        }
    }

    #[tokio::test]
    async fn receipt_event_channel() {
        let channels = EventChannels::new(10);
        let mut rx = channels.subscribe_receipts();

        channels.send_receipt(ReceiptEvent::Received {
            receipt: ReadReceipt {
                message_id: "msg-100".to_string(),
                reader_did: "did:variance:bob".to_string(),
                ..Default::default()
            },
        });

        let received = rx.recv().await.unwrap();
        match received {
            ReceiptEvent::Received { receipt } => {
                assert_eq!(receipt.message_id, "msg-100");
                assert_eq!(receipt.reader_did, "did:variance:bob");
            }
        }
    }

    #[tokio::test]
    async fn identity_request_received_event() {
        let channels = EventChannels::new(10);
        let mut rx = channels.subscribe_identity();

        let peer = PeerId::random();
        channels.send_identity(IdentityEvent::RequestReceived {
            peer,
            request: IdentityRequest {
                query: Some(
                    variance_proto::identity_proto::identity_request::Query::Did(
                        "did:peer:xyz".to_string(),
                    ),
                ),
                requester_did: None,
                timestamp: 42,
            },
        });

        let received = rx.recv().await.unwrap();
        match received {
            IdentityEvent::RequestReceived {
                peer: p,
                request: req,
            } => {
                assert_eq!(p, peer);
                assert_eq!(req.timestamp, 42);
            }
            _ => panic!("Expected RequestReceived"),
        }
    }

    #[tokio::test]
    async fn identity_response_received_event() {
        let channels = EventChannels::new(10);
        let mut rx = channels.subscribe_identity();

        let peer = PeerId::random();
        channels.send_identity(IdentityEvent::ResponseReceived {
            peer,
            response: IdentityResponse {
                result: None,
                timestamp: 99,
            },
        });

        let received = rx.recv().await.unwrap();
        match received {
            IdentityEvent::ResponseReceived {
                peer: p,
                response: resp,
            } => {
                assert_eq!(p, peer);
                assert_eq!(resp.timestamp, 99);
            }
            _ => panic!("Expected ResponseReceived"),
        }
    }

    #[tokio::test]
    async fn identity_peer_offline_event() {
        let channels = EventChannels::new(10);
        let mut rx = channels.subscribe_identity();

        channels.send_identity(IdentityEvent::PeerOffline {
            did: "did:variance:gone".to_string(),
        });

        let received = rx.recv().await.unwrap();
        match received {
            IdentityEvent::PeerOffline { did } => {
                assert_eq!(did, "did:variance:gone");
            }
            _ => panic!("Expected PeerOffline"),
        }
    }

    #[tokio::test]
    async fn direct_message_received_event() {
        let channels = EventChannels::new(10);
        let mut rx = channels.subscribe_direct_messages();

        let peer = PeerId::random();
        channels.send_direct_message(DirectMessageEvent::MessageReceived {
            peer,
            message: DirectMessage {
                id: "dm-001".to_string(),
                sender_did: "did:variance:alice".to_string(),
                recipient_did: "did:variance:bob".to_string(),
                ciphertext: vec![0xAB],
                ..Default::default()
            },
        });

        let received = rx.recv().await.unwrap();
        match received {
            DirectMessageEvent::MessageReceived { peer: p, message } => {
                assert_eq!(p, peer);
                assert_eq!(message.id, "dm-001");
            }
            _ => panic!("Expected MessageReceived"),
        }
    }

    #[tokio::test]
    async fn direct_message_sent_event() {
        let channels = EventChannels::new(10);
        let mut rx = channels.subscribe_direct_messages();

        channels.send_direct_message(DirectMessageEvent::MessageSent {
            message_id: "dm-002".to_string(),
            recipient: "did:variance:bob".to_string(),
        });

        let received = rx.recv().await.unwrap();
        match received {
            DirectMessageEvent::MessageSent {
                message_id,
                recipient,
            } => {
                assert_eq!(message_id, "dm-002");
                assert_eq!(recipient, "did:variance:bob");
            }
            _ => panic!("Expected MessageSent"),
        }
    }

    #[tokio::test]
    async fn delivery_nack_event() {
        let channels = EventChannels::new(10);
        let mut rx = channels.subscribe_direct_messages();

        let peer = PeerId::random();
        channels.send_direct_message(DirectMessageEvent::DeliveryNack {
            peer,
            message_id: "dm-003".to_string(),
            error: "rate limited".to_string(),
        });

        let received = rx.recv().await.unwrap();
        match received {
            DirectMessageEvent::DeliveryNack {
                peer: p,
                message_id,
                error,
            } => {
                assert_eq!(p, peer);
                assert_eq!(message_id, "dm-003");
                assert_eq!(error, "rate limited");
            }
            _ => panic!("Expected DeliveryNack"),
        }
    }

    #[tokio::test]
    async fn event_channels_default_buffer_size() {
        let channels = EventChannels::default();
        // Default should create working channels (buffer_size = 100)
        let mut rx = channels.subscribe_typing();
        channels.send_typing(TypingEvent::IndicatorReceived {
            sender_did: "a".to_string(),
            recipient: "b".to_string(),
            is_typing: false,
        });
        let _ = rx.recv().await.unwrap();
    }

    #[tokio::test]
    async fn send_without_subscribers_does_not_panic() {
        let channels = EventChannels::new(10);
        // No subscribers — send should silently drop
        channels.send_identity(IdentityEvent::DidCached {
            did: "did:peer:orphan".to_string(),
        });
        channels.send_direct_message(DirectMessageEvent::DeliveryFailed {
            message_id: "x".to_string(),
            recipient: "y".to_string(),
        });
        channels.send_group_message(GroupMessageEvent::MessageSent {
            message_id: "x".to_string(),
            group_id: "y".to_string(),
        });
        channels.send_typing(TypingEvent::IndicatorReceived {
            sender_did: "a".to_string(),
            recipient: "b".to_string(),
            is_typing: true,
        });
        channels.send_rename(RenameEvent::PeerRenamed {
            did: "a".to_string(),
            username: "b".to_string(),
            discriminator: 0,
        });
        channels.send_receipt(ReceiptEvent::Received {
            receipt: ReadReceipt::default(),
        });
        channels.send_group_sync(GroupSyncEvent::SyncFailed {
            group_id: "g".to_string(),
            error: "e".to_string(),
        });
        channels.send_offline_message(OfflineMessageEvent::MessageStored {
            message_id: "m".to_string(),
            recipient: "r".to_string(),
        });
        channels.send_signaling(SignalingEvent::CallEnded {
            call_id: "c".to_string(),
            reason: "r".to_string(),
        });
        // If we get here without panic, the test passes
    }
}
