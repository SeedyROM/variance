//! WebSocket support for real-time event delivery
//!
//! This module provides WebSocket connectivity for clients to receive real-time
//! events from the P2P network (incoming messages, calls, typing indicators, etc.)

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::IntoResponse;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use dashmap::DashMap;
use futures::{SinkExt, StreamExt};
use prost::Message as ProstMessage;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{debug, warn};
use variance_proto::media_proto::SignalingMessage;

use crate::state::AppState;

/// Encode a prost `SignalingMessage` as base64 for JSON transport.
pub fn encode_signaling(msg: &SignalingMessage) -> String {
    BASE64.encode(msg.encode_to_vec())
}

/// Messages sent FROM clients TO server
#[derive(Debug, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum ClientMessage {
    /// Authenticate with a DID
    Authenticate { did: String },
    /// Update subscription preferences
    Subscribe {
        signaling: Option<bool>,
        messages: Option<bool>,
        presence: Option<bool>,
    },
    /// Ping to keep connection alive
    Ping,
    /// Send a direct message
    SendDirectMessage {
        recipient_did: String,
        text: String,
        reply_to: Option<String>,
    },
    /// Send a group message
    SendGroupMessage {
        group_id: String,
        text: String,
        reply_to: Option<String>,
    },
}

/// Subscription category for a [`WsMessage`].
///
/// Each variant maps to one of the boolean flags in [`ClientSubscription`].
/// Messages that don't belong to any optional category (e.g. `Connected`,
/// `Ping`, `Pong`) are always delivered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MessageCategory {
    Signaling,
    Messages,
    Presence,
    /// Always delivered regardless of subscription flags.
    System,
}

/// WebSocket message sent to clients
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", content = "data")]
pub enum WsMessage {
    // Signaling events — `signaling_payload` is base64-encoded protobuf bytes
    // of the full `SignalingMessage` so the frontend has access to SDP/ICE data.
    CallIncoming {
        call_id: String,
        from: String,
        signaling_payload: String,
    },
    CallAnswer {
        call_id: String,
        from: String,
        signaling_payload: String,
    },
    IceCandidate {
        call_id: String,
        from: String,
        signaling_payload: String,
    },
    CallControl {
        call_id: String,
        from: String,
        signaling_payload: String,
    },
    CallEnded {
        call_id: String,
        reason: String,
    },
    CallStateChanged {
        call_id: String,
        status: String,
    },

    // Message events
    DirectMessageReceived {
        from: String,
        message_id: String,
        text: String,
        timestamp: i64,
        reply_to: Option<String>,
    },
    DirectMessageSent {
        recipient: String,
        message_id: String,
        text: String,
        timestamp: i64,
        reply_to: Option<String>,
    },
    /// The remote peer rejected our message (e.g. rate limited). Frontend should retry.
    DirectMessageNack {
        message_id: String,
        error: String,
    },
    /// A message's delivery status changed (e.g. OutboundFailure after send_request).
    DirectMessageStatusChanged {
        message_id: String,
        status: String,
    },
    GroupMessageReceived {
        group_id: String,
        from: String,
        message_id: String,
        timestamp: i64,
    },
    /// Auto-joined an MLS group after receiving a Welcome via DM.
    MlsGroupJoined {
        group_id: String,
        group_name: Option<String>,
        inviter: String,
    },
    /// A new group invitation was received (pending user accept/decline).
    GroupInvitationReceived {
        group_id: String,
        group_name: String,
        inviter_did: String,
        inviter_display_name: Option<String>,
    },
    /// An outbound invite was accepted — the invitee joined the group.
    GroupInvitationAccepted {
        group_id: String,
        invitee_did: String,
        invitee_display_name: Option<String>,
    },
    /// An outbound invite was declined — the pending commit was rolled back.
    GroupInvitationDeclined {
        group_id: String,
        invitee_did: String,
    },
    /// An outbound invite expired (5-minute timeout) — the pending commit was rolled back.
    GroupInvitationExpired {
        group_id: String,
        invitee_did: String,
    },
    /// A member's role was changed (promote/demote).
    RoleChanged {
        group_id: String,
        target_did: String,
        new_role: String,
        changed_by: String,
    },
    /// The local user was removed from a group (kicked).
    MlsGroupRemoved {
        group_id: String,
        reason: String,
    },
    /// A member was removed from a group (visible to remaining members).
    GroupMemberRemoved {
        group_id: String,
        member_did: String,
    },
    OfflineMessagesReceived {
        count: usize,
    },

    // Typing indicators
    TypingStarted {
        from: String,
        recipient: String,
    },
    TypingStopped {
        from: String,
        recipient: String,
    },

    // Read receipts
    ReceiptRead {
        message_id: String,
    },
    ReceiptDelivered {
        message_id: String,
    },

    // Presence/identity
    PresenceUpdated {
        did: String,
        online: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        display_name: Option<String>,
    },
    PeerRenamed {
        did: String,
        /// Formatted as "name#0042"
        display_name: String,
    },

    // Connection management
    Connected {
        client_id: String,
    },
    Ping,
    Pong,
}

impl WsMessage {
    /// Return the subscription category this message belongs to.
    fn category(&self) -> MessageCategory {
        match self {
            // Signaling
            Self::CallIncoming { .. }
            | Self::CallAnswer { .. }
            | Self::IceCandidate { .. }
            | Self::CallControl { .. }
            | Self::CallEnded { .. }
            | Self::CallStateChanged { .. } => MessageCategory::Signaling,

            // Messages (direct, group, offline, typing, receipts)
            Self::DirectMessageReceived { .. }
            | Self::DirectMessageSent { .. }
            | Self::DirectMessageNack { .. }
            | Self::DirectMessageStatusChanged { .. }
            | Self::GroupMessageReceived { .. }
            | Self::MlsGroupJoined { .. }
            | Self::GroupInvitationReceived { .. }
            | Self::GroupInvitationAccepted { .. }
            | Self::GroupInvitationDeclined { .. }
            | Self::GroupInvitationExpired { .. }
            | Self::RoleChanged { .. }
            | Self::MlsGroupRemoved { .. }
            | Self::GroupMemberRemoved { .. }
            | Self::OfflineMessagesReceived { .. }
            | Self::TypingStarted { .. }
            | Self::TypingStopped { .. }
            | Self::ReceiptRead { .. }
            | Self::ReceiptDelivered { .. } => MessageCategory::Messages,

            // Presence / identity
            Self::PresenceUpdated { .. } | Self::PeerRenamed { .. } => MessageCategory::Presence,

            // System — always delivered
            Self::Connected { .. } | Self::Ping | Self::Pong => MessageCategory::System,
        }
    }
}

/// Client subscription preferences
#[derive(Debug, Clone, Deserialize)]
pub struct ClientSubscription {
    #[serde(default = "default_true")]
    pub signaling: bool,
    #[serde(default = "default_true")]
    pub messages: bool,
    #[serde(default = "default_true")]
    pub presence: bool,
}

fn default_true() -> bool {
    true
}

impl Default for ClientSubscription {
    fn default() -> Self {
        Self {
            signaling: true,
            messages: true,
            presence: true,
        }
    }
}

impl ClientSubscription {
    /// Whether this subscription accepts messages of the given category.
    fn accepts(&self, category: MessageCategory) -> bool {
        match category {
            MessageCategory::Signaling => self.signaling,
            MessageCategory::Messages => self.messages,
            MessageCategory::Presence => self.presence,
            MessageCategory::System => true,
        }
    }
}

/// Connected WebSocket client
pub struct ConnectedClient {
    pub did: Option<String>,
    pub tx: mpsc::UnboundedSender<WsMessage>,
    pub subscriptions: ClientSubscription,
}

/// Manages all WebSocket connections
#[derive(Clone)]
pub struct WebSocketManager {
    clients: Arc<DashMap<String, ConnectedClient>>,
}

impl WebSocketManager {
    pub fn new() -> Self {
        Self {
            clients: Arc::new(DashMap::new()),
        }
    }

    /// Register a new client connection
    pub fn register(&self, client_id: String, client: ConnectedClient) {
        debug!("Registering WebSocket client: {}", client_id);
        self.clients.insert(client_id, client);
    }

    /// Remove disconnected client
    pub fn unregister(&self, client_id: &str) {
        debug!("Unregistering WebSocket client: {}", client_id);
        self.clients.remove(client_id);
    }

    /// Broadcast message to all subscribed clients
    pub fn broadcast(&self, message: WsMessage) {
        let category = message.category();
        let mut sent = 0u32;

        for entry in self.clients.iter() {
            let client = entry.value();
            if client.subscriptions.accepts(category) {
                let _ = client.tx.send(message.clone());
                sent += 1;
            }
        }

        debug!(
            "Broadcast {:?} message to {}/{} clients",
            category,
            sent,
            self.clients.len()
        );
    }

    /// Send message to specific client
    pub fn send_to(&self, client_id: &str, message: WsMessage) {
        if let Some(client) = self.clients.get(client_id) {
            if let Err(e) = client.tx.send(message) {
                warn!("Failed to send to client {}: {}", client_id, e);
            }
        } else {
            warn!("Client not found: {}", client_id);
        }
    }

    /// Get number of connected clients
    pub fn client_count(&self) -> usize {
        self.clients.len()
    }
}

impl Default for WebSocketManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Process incoming client messages
async fn handle_client_message(client_id: &str, msg: ClientMessage, state: &AppState) {
    match msg {
        ClientMessage::Authenticate { did } => {
            debug!("Client {} authenticating as {}", client_id, did);
            if let Some(mut client) = state.ws_manager.clients.get_mut(client_id) {
                client.did = Some(did.clone());
                debug!("Client {} authenticated as {}", client_id, did);

                // Send confirmation
                let _ = client.tx.send(WsMessage::PresenceUpdated {
                    did,
                    online: true,
                    display_name: None,
                });
            }
        }
        ClientMessage::Subscribe {
            signaling,
            messages,
            presence,
        } => {
            debug!(
                "Client {} updating subscriptions: signaling={:?}, messages={:?}, presence={:?}",
                client_id, signaling, messages, presence
            );
            if let Some(mut client) = state.ws_manager.clients.get_mut(client_id) {
                if let Some(sig) = signaling {
                    client.subscriptions.signaling = sig;
                }
                if let Some(msg) = messages {
                    client.subscriptions.messages = msg;
                }
                if let Some(pres) = presence {
                    client.subscriptions.presence = pres;
                }
                debug!("Client {} subscriptions updated", client_id);
            }
        }
        ClientMessage::Ping => {
            debug!("Client {} sent ping", client_id);
            if let Some(client) = state.ws_manager.clients.get(client_id) {
                let _ = client.tx.send(WsMessage::Pong);
            }
        }
        ClientMessage::SendDirectMessage {
            recipient_did,
            text,
            reply_to,
        } => {
            debug!(
                "Client {} sending direct message to {}",
                client_id, recipient_did
            );

            use variance_proto::messaging_proto::MessageContent;
            let content = MessageContent {
                text,
                attachments: vec![],
                mentions: vec![],
                reply_to,
                metadata: Default::default(),
            };

            match state
                .direct_messaging
                .send_message(recipient_did.clone(), content)
                .await
            {
                Ok(message) => {
                    debug!("Message sent: {}", message.id);

                    // Transmit over P2P (best-effort)
                    if let Err(e) = state
                        .node_handle
                        .send_direct_message(recipient_did.clone(), message.clone())
                        .await
                    {
                        debug!(
                            "P2P direct message delivery failed (will rely on offline relay): {}",
                            e
                        );
                    }

                    // Emit event
                    use variance_p2p::events::DirectMessageEvent;
                    state
                        .event_channels
                        .send_direct_message(DirectMessageEvent::MessageSent {
                            message_id: message.id.clone(),
                            recipient: recipient_did,
                        });
                }
                Err(e) => {
                    warn!("Failed to send message from client {}: {}", client_id, e);
                }
            }
        }
        ClientMessage::SendGroupMessage {
            group_id,
            text,
            reply_to,
        } => {
            debug!("Client {} sending group message to {}", client_id, group_id);

            use variance_messaging::mls::MlsGroupHandler;
            use variance_messaging::storage::MessageStorage;
            use variance_proto::messaging_proto::MessageContent;

            let content = MessageContent {
                text,
                attachments: vec![],
                mentions: vec![],
                reply_to: reply_to.clone(),
                metadata: Default::default(),
            };
            let plaintext = prost::Message::encode_to_vec(&content);

            match state.mls_groups.encrypt_message(&group_id, &plaintext) {
                Ok(mls_msg) => match MlsGroupHandler::serialize_message(&mls_msg) {
                    Ok(mls_bytes) => {
                        let message_id = ulid::Ulid::new().to_string();
                        let timestamp = chrono::Utc::now().timestamp_millis();

                        let message = variance_proto::messaging_proto::GroupMessage {
                            id: message_id.clone(),
                            sender_did: state.local_did.clone(),
                            group_id: group_id.clone(),
                            timestamp,
                            r#type: variance_proto::messaging_proto::MessageType::Text.into(),
                            reply_to,
                            mls_ciphertext: mls_bytes,
                        };

                        let topic = format!("/variance/group/{}", group_id);
                        if let Err(e) = state
                            .node_handle
                            .publish_group_message(topic, message.clone())
                            .await
                        {
                            warn!("Failed to publish MLS group message: {}", e);
                        }

                        if let Err(e) = state.storage.store_group(&message).await {
                            warn!("Failed to store MLS group message locally: {}", e);
                        }

                        // Mark the group as read so our own sent message doesn't
                        // appear as unread when the groups list is next fetched.
                        let _ = state
                            .storage
                            .store_group_last_read_at(&state.local_did, &group_id, timestamp)
                            .await;

                        use variance_p2p::events::GroupMessageEvent;
                        state
                            .event_channels
                            .send_group_message(GroupMessageEvent::MessageSent {
                                message_id: message_id.clone(),
                                group_id: group_id.clone(),
                            });

                        if let Some(client) = state.ws_manager.clients.get(client_id) {
                            let _ = client.tx.send(WsMessage::GroupMessageReceived {
                                group_id,
                                from: state.local_did.clone(),
                                message_id,
                                timestamp,
                            });
                        }
                    }
                    Err(e) => {
                        warn!(
                            "Failed to serialize MLS message from client {}: {}",
                            client_id, e
                        );
                    }
                },
                Err(e) => {
                    warn!(
                        "Failed to encrypt MLS group message from client {}: {}",
                        client_id, e
                    );
                }
            }
        }
    }
}

/// WebSocket upgrade handler
pub async fn websocket_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

/// Handle individual WebSocket connection
async fn handle_socket(socket: WebSocket, state: AppState) {
    let client_id = uuid::Uuid::new_v4().to_string();
    debug!("New WebSocket connection: {}", client_id);

    let (mut sender, mut receiver) = socket.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<WsMessage>();

    // Register client
    let client = ConnectedClient {
        did: None,
        tx,
        subscriptions: ClientSubscription::default(),
    };
    state.ws_manager.register(client_id.clone(), client);

    // Send connection confirmation
    let welcome = WsMessage::Connected {
        client_id: client_id.clone(),
    };
    if let Ok(json) = serde_json::to_string(&welcome) {
        let _ = sender.send(Message::Text(json.into())).await;
    }

    // Send current presence state for all known connected peers so the client
    // doesn't start with everyone showing as offline.
    if let Ok(connected_dids) = state.node_handle.get_connected_dids().await {
        for did in connected_dids {
            let display_name = state.username_registry.get_display_name(&did);
            let msg = WsMessage::PresenceUpdated {
                did,
                online: true,
                display_name,
            };
            if let Ok(json) = serde_json::to_string(&msg) {
                if sender.send(Message::Text(json.into())).await.is_err() {
                    debug!("Client disconnected during initial presence sync");
                    state.ws_manager.unregister(&client_id);
                    return;
                }
            }
        }
    }

    // Spawn task to forward messages to WebSocket
    let send_task = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            let json = match serde_json::to_string(&msg) {
                Ok(j) => j,
                Err(e) => {
                    warn!("Failed to serialize message: {}", e);
                    continue;
                }
            };

            if sender.send(Message::Text(json.into())).await.is_err() {
                debug!("Client disconnected (send failed)");
                break;
            }
        }
    });

    // Handle incoming client messages (subscriptions, ping/pong, etc.)
    let client_id_clone = client_id.clone();
    let state_clone = state.clone();
    let recv_task = tokio::spawn(async move {
        while let Some(result) = receiver.next().await {
            match result {
                Ok(Message::Text(text)) => {
                    debug!("Received text from {}: {}", client_id_clone, text);

                    // Parse client message
                    match serde_json::from_str::<ClientMessage>(&text) {
                        Ok(msg) => {
                            handle_client_message(&client_id_clone, msg, &state_clone).await;
                        }
                        Err(e) => {
                            warn!(
                                "Failed to parse client message from {}: {}",
                                client_id_clone, e
                            );
                        }
                    }
                }
                Ok(Message::Ping(_)) => {
                    debug!("Received ping from {}", client_id_clone);
                }
                Ok(Message::Pong(_)) => {
                    debug!("Received pong from {}", client_id_clone);
                }
                Ok(Message::Close(_)) => {
                    debug!("Received close from {}", client_id_clone);
                    break;
                }
                Err(e) => {
                    warn!("WebSocket error from {}: {}", client_id_clone, e);
                    break;
                }
                _ => {}
            }
        }
    });

    // Wait for disconnection
    tokio::select! {
        _ = send_task => {
            debug!("Send task completed for {}", client_id);
        }
        _ = recv_task => {
            debug!("Receive task completed for {}", client_id);
        }
    }

    // Cleanup
    state.ws_manager.unregister(&client_id);
    debug!("WebSocket connection closed: {}", client_id);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_websocket_manager_new() {
        let manager = WebSocketManager::new();
        assert_eq!(manager.client_count(), 0);
    }

    #[test]
    fn test_client_subscription_default() {
        let sub = ClientSubscription::default();
        assert!(sub.signaling);
        assert!(sub.messages);
        assert!(sub.presence);
    }

    #[tokio::test]
    async fn test_register_unregister() {
        let manager = WebSocketManager::new();
        let (tx, _rx) = mpsc::unbounded_channel();

        let client = ConnectedClient {
            did: Some("did:variance:test".to_string()),
            tx,
            subscriptions: ClientSubscription::default(),
        };

        manager.register("client1".to_string(), client);
        assert_eq!(manager.client_count(), 1);

        manager.unregister("client1");
        assert_eq!(manager.client_count(), 0);
    }

    #[tokio::test]
    async fn test_broadcast() {
        let manager = WebSocketManager::new();
        let (tx1, mut rx1) = mpsc::unbounded_channel();
        let (tx2, mut rx2) = mpsc::unbounded_channel();

        manager.register(
            "client1".to_string(),
            ConnectedClient {
                did: None,
                tx: tx1,
                subscriptions: ClientSubscription::default(),
            },
        );

        manager.register(
            "client2".to_string(),
            ConnectedClient {
                did: None,
                tx: tx2,
                subscriptions: ClientSubscription::default(),
            },
        );

        let msg = WsMessage::Ping;
        manager.broadcast(msg);

        // Both clients should receive the message
        assert!(rx1.try_recv().is_ok());
        assert!(rx2.try_recv().is_ok());
    }

    #[tokio::test]
    async fn test_broadcast_respects_subscriptions() {
        let manager = WebSocketManager::new();
        let (tx_all, mut rx_all) = mpsc::unbounded_channel();
        let (tx_no_sig, mut rx_no_sig) = mpsc::unbounded_channel();
        let (tx_no_msg, mut rx_no_msg) = mpsc::unbounded_channel();

        // Client with all subscriptions enabled
        manager.register(
            "all".to_string(),
            ConnectedClient {
                did: None,
                tx: tx_all,
                subscriptions: ClientSubscription::default(),
            },
        );

        // Client with signaling disabled
        manager.register(
            "no_sig".to_string(),
            ConnectedClient {
                did: None,
                tx: tx_no_sig,
                subscriptions: ClientSubscription {
                    signaling: false,
                    messages: true,
                    presence: true,
                },
            },
        );

        // Client with messages disabled
        manager.register(
            "no_msg".to_string(),
            ConnectedClient {
                did: None,
                tx: tx_no_msg,
                subscriptions: ClientSubscription {
                    signaling: true,
                    messages: false,
                    presence: true,
                },
            },
        );

        // Broadcast a signaling message
        manager.broadcast(WsMessage::CallEnded {
            call_id: "c1".into(),
            reason: "bye".into(),
        });
        assert!(rx_all.try_recv().is_ok(), "all-sub client gets signaling");
        assert!(rx_no_sig.try_recv().is_err(), "no-signaling client skipped");
        assert!(rx_no_msg.try_recv().is_ok(), "no-msg client gets signaling");

        // Broadcast a message event
        manager.broadcast(WsMessage::DirectMessageReceived {
            from: "did:test".into(),
            message_id: "m1".into(),
            text: "hi".into(),
            timestamp: 0,
            reply_to: None,
        });
        assert!(rx_all.try_recv().is_ok(), "all-sub client gets messages");
        assert!(rx_no_sig.try_recv().is_ok(), "no-sig client gets messages");
        assert!(rx_no_msg.try_recv().is_err(), "no-msg client skipped");

        // System messages always delivered
        manager.broadcast(WsMessage::Ping);
        assert!(rx_all.try_recv().is_ok());
        assert!(rx_no_sig.try_recv().is_ok());
        assert!(rx_no_msg.try_recv().is_ok());
    }

    #[tokio::test]
    async fn test_client_authentication() {
        use crate::state::AppState;
        use tempfile::tempdir;

        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let state =
            AppState::with_db_path("did:variance:test".to_string(), db_path.to_str().unwrap());
        let (tx, mut rx) = mpsc::unbounded_channel();

        state.ws_manager.register(
            "client1".to_string(),
            ConnectedClient {
                did: None,
                tx,
                subscriptions: ClientSubscription::default(),
            },
        );

        // Client sends authenticate message
        let auth_msg = ClientMessage::Authenticate {
            did: "did:variance:alice".to_string(),
        };
        handle_client_message("client1", auth_msg, &state).await;

        // Check DID was set
        let client = state.ws_manager.clients.get("client1").unwrap();
        assert_eq!(client.did, Some("did:variance:alice".to_string()));

        // Check confirmation was sent
        match rx.try_recv() {
            Ok(WsMessage::PresenceUpdated { did, online, .. }) => {
                assert_eq!(did, "did:variance:alice");
                assert!(online);
            }
            _ => panic!("Expected PresenceUpdated message"),
        }
    }

    #[tokio::test]
    async fn test_client_subscription_update() {
        use crate::state::AppState;
        use tempfile::tempdir;

        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let state =
            AppState::with_db_path("did:variance:test".to_string(), db_path.to_str().unwrap());
        let (tx, _rx) = mpsc::unbounded_channel();

        state.ws_manager.register(
            "client1".to_string(),
            ConnectedClient {
                did: None,
                tx,
                subscriptions: ClientSubscription::default(),
            },
        );

        // Update subscriptions
        let sub_msg = ClientMessage::Subscribe {
            signaling: Some(false),
            messages: Some(true),
            presence: Some(false),
        };
        handle_client_message("client1", sub_msg, &state).await;

        // Check subscriptions were updated
        let client = state.ws_manager.clients.get("client1").unwrap();
        assert!(!client.subscriptions.signaling);
        assert!(client.subscriptions.messages);
        assert!(!client.subscriptions.presence);
    }

    #[tokio::test]
    async fn test_client_ping_pong() {
        use crate::state::AppState;
        use tempfile::tempdir;

        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let state =
            AppState::with_db_path("did:variance:test".to_string(), db_path.to_str().unwrap());
        let (tx, mut rx) = mpsc::unbounded_channel();

        state.ws_manager.register(
            "client1".to_string(),
            ConnectedClient {
                did: None,
                tx,
                subscriptions: ClientSubscription::default(),
            },
        );

        // Send ping
        handle_client_message("client1", ClientMessage::Ping, &state).await;

        // Should receive pong
        match rx.try_recv() {
            Ok(WsMessage::Pong) => {}
            _ => panic!("Expected Pong message"),
        }
    }
}
