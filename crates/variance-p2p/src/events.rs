//! Event types for P2P protocols
//!
//! Provides event channels for protocol events that the application layer can subscribe to.

use libp2p::PeerId;
use tokio::sync::broadcast;
use variance_proto::identity_proto::{IdentityRequest, IdentityResponse};
use variance_proto::media_proto::SignalingMessage;
use variance_proto::messaging_proto::{DirectMessage, GroupMessage, OfflineMessageEnvelope};

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
}

/// Events from the offline message protocol
#[derive(Debug, Clone)]
pub enum OfflineMessageEvent {
    /// Received a fetch request from a peer
    FetchRequested {
        peer: PeerId,
        did: String,
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

/// Combined event type for all protocols
#[derive(Debug, Clone)]
pub enum P2pEvent {
    Identity(IdentityEvent),
    OfflineMessage(OfflineMessageEvent),
    Signaling(SignalingEvent),
    DirectMessage(DirectMessageEvent),
    GroupMessage(GroupMessageEvent),
}

/// Event channels for protocol events
#[derive(Clone)]
pub struct EventChannels {
    pub identity: broadcast::Sender<IdentityEvent>,
    pub offline_messages: broadcast::Sender<OfflineMessageEvent>,
    pub signaling: broadcast::Sender<SignalingEvent>,
    pub direct_messages: broadcast::Sender<DirectMessageEvent>,
    pub group_messages: broadcast::Sender<GroupMessageEvent>,
}

impl EventChannels {
    /// Create new event channels with the given buffer size
    pub fn new(buffer_size: usize) -> Self {
        let (identity_tx, _) = broadcast::channel(buffer_size);
        let (offline_tx, _) = broadcast::channel(buffer_size);
        let (signaling_tx, _) = broadcast::channel(buffer_size);
        let (direct_tx, _) = broadcast::channel(buffer_size);
        let (group_tx, _) = broadcast::channel(buffer_size);

        Self {
            identity: identity_tx,
            offline_messages: offline_tx,
            signaling: signaling_tx,
            direct_messages: direct_tx,
            group_messages: group_tx,
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
}
