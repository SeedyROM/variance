//! WebSocket support for real-time event delivery
//!
//! This module provides WebSocket connectivity for clients to receive real-time
//! events from the P2P network (incoming messages, calls, typing indicators, etc.)

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::IntoResponse;
use dashmap::DashMap;
use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{debug, warn};
use variance_proto::media_proto::SignalingMessage;

use crate::state::AppState;

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

/// WebSocket message sent to clients
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", content = "data")]
pub enum WsMessage {
    // Signaling events
    CallIncoming {
        call_id: String,
        from: String,
        #[serde(skip)]
        message: SignalingMessage,
    },
    CallAnswer {
        call_id: String,
        from: String,
        #[serde(skip)]
        message: SignalingMessage,
    },
    IceCandidate {
        call_id: String,
        from: String,
        #[serde(skip)]
        message: SignalingMessage,
    },
    CallControl {
        call_id: String,
        from: String,
        #[serde(skip)]
        message: SignalingMessage,
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
    GroupMessageReceived {
        group_id: String,
        from: String,
        message_id: String,
        timestamp: i64,
    },
    OfflineMessagesReceived {
        count: usize,
    },

    // Presence/identity
    PresenceUpdated {
        did: String,
        online: bool,
    },

    // Connection management
    Connected {
        client_id: String,
    },
    Ping,
    Pong,
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
        let count = self.clients.len();
        debug!("Broadcasting message to {} clients", count);

        for entry in self.clients.iter() {
            let _ = entry.value().tx.send(message.clone());
        }
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
                let _ = client
                    .tx
                    .send(WsMessage::PresenceUpdated { did, online: true });
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

                    // Emit event if channels available
                    if let Some(ref channels) = state.event_channels {
                        use variance_p2p::events::DirectMessageEvent;
                        channels.send_direct_message(DirectMessageEvent::MessageSent {
                            message_id: message.id.clone(),
                            recipient: recipient_did,
                        });
                    }
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

            use variance_proto::messaging_proto::MessageContent;
            let content = MessageContent {
                text,
                attachments: vec![],
                mentions: vec![],
                reply_to,
                metadata: Default::default(),
            };

            match state
                .group_messaging
                .send_message(group_id.clone(), content)
                .await
            {
                Ok(message) => {
                    debug!("Group message sent: {}", message.id);

                    // Emit event if channels available
                    if let Some(ref channels) = state.event_channels {
                        use variance_p2p::events::GroupMessageEvent;
                        channels.send_group_message(GroupMessageEvent::MessageSent {
                            message_id: message.id.clone(),
                            group_id: group_id.clone(),
                        });
                    }

                    // Echo the sent message back to the sender so the UI updates immediately.
                    if let Some(client) = state.ws_manager.clients.get(client_id) {
                        let _ = client.tx.send(WsMessage::GroupMessageReceived {
                            group_id,
                            from: state.local_did.clone(),
                            message_id: message.id.clone(),
                            timestamp: message.timestamp,
                        });
                    }
                }
                Err(e) => {
                    warn!(
                        "Failed to send group message from client {}: {}",
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
            Ok(WsMessage::PresenceUpdated { did, online }) => {
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
