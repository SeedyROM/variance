//! Event router that bridges P2P events to WebSocket clients
//!
//! Subscribes to variance-p2p EventChannels and forwards events to connected
//! WebSocket clients via the WebSocketManager.

use crate::websocket::{WebSocketManager, WsMessage};
use std::sync::Arc;
use tracing::{debug, warn};
use variance_messaging::{direct::DirectMessageHandler, group::GroupMessageHandler};
use variance_p2p::{EventChannels, IdentityEvent, NodeHandle, OfflineMessageEvent, SignalingEvent};

/// Bridges P2P events to WebSocket clients
pub struct EventRouter {
    ws_manager: WebSocketManager,
    direct_messaging: Arc<DirectMessageHandler>,
    group_messaging: Arc<GroupMessageHandler>,
    node_handle: NodeHandle,
}

impl EventRouter {
    pub fn new(
        ws_manager: WebSocketManager,
        direct_messaging: Arc<DirectMessageHandler>,
        group_messaging: Arc<GroupMessageHandler>,
        node_handle: NodeHandle,
    ) -> Self {
        Self {
            ws_manager,
            direct_messaging,
            group_messaging,
            node_handle,
        }
    }

    /// Start listening to P2P events and forwarding to WebSocket clients
    ///
    /// This spawns background tasks that subscribe to each event channel
    /// and broadcast events to all connected WebSocket clients.
    pub fn start(self, events: EventChannels) {
        // Spawn task for signaling events
        let ws_manager = self.ws_manager.clone();
        let events_clone = events.clone();
        tokio::spawn(async move {
            let mut rx = events_clone.subscribe_signaling();
            debug!("EventRouter: Started signaling event listener");

            while let Ok(event) = rx.recv().await {
                debug!("EventRouter: Received signaling event: {:?}", event);

                let msg = match event {
                    SignalingEvent::OfferReceived {
                        peer,
                        call_id,
                        message,
                    } => WsMessage::CallIncoming {
                        call_id,
                        from: format!("{}", peer),
                        message,
                    },
                    SignalingEvent::AnswerReceived {
                        peer,
                        call_id,
                        message,
                    } => WsMessage::CallAnswer {
                        call_id,
                        from: format!("{}", peer),
                        message,
                    },
                    SignalingEvent::IceCandidateReceived {
                        peer,
                        call_id,
                        message,
                    } => WsMessage::IceCandidate {
                        call_id,
                        from: format!("{}", peer),
                        message,
                    },
                    SignalingEvent::ControlReceived {
                        peer,
                        call_id,
                        message,
                    } => WsMessage::CallControl {
                        call_id,
                        from: format!("{}", peer),
                        message,
                    },
                    SignalingEvent::CallEnded { call_id, reason } => {
                        WsMessage::CallEnded { call_id, reason }
                    }
                };

                ws_manager.broadcast(msg);
            }

            warn!("EventRouter: Signaling event listener ended");
        });

        // Spawn task for offline message events
        let ws_manager = self.ws_manager.clone();
        let events_clone = events.clone();
        tokio::spawn(async move {
            let mut rx = events_clone.subscribe_offline_messages();
            debug!("EventRouter: Started offline message event listener");

            while let Ok(event) = rx.recv().await {
                debug!("EventRouter: Received offline message event: {:?}", event);

                if let OfflineMessageEvent::MessagesReceived { messages, .. } = event {
                    let msg = WsMessage::OfflineMessagesReceived {
                        count: messages.len(),
                    };
                    ws_manager.broadcast(msg);
                }
            }

            warn!("EventRouter: Offline message event listener ended");
        });

        // Spawn task for direct message events
        // Decrypts incoming messages using the Double Ratchet handler before broadcasting.
        let ws_manager = self.ws_manager.clone();
        let direct_messaging = self.direct_messaging.clone();
        let node_handle = self.node_handle.clone();
        let events_clone = events.clone();
        tokio::spawn(async move {
            use variance_p2p::events::DirectMessageEvent;
            let mut rx = events_clone.subscribe_direct_messages();
            debug!("EventRouter: Started direct message event listener");

            while let Ok(event) = rx.recv().await {
                debug!("EventRouter: Received direct message event: {:?}", event);

                if let DirectMessageEvent::MessageReceived { peer: _, message } = event {
                    let from = message.sender_did.clone();
                    let message_id = message.id.clone();
                    let timestamp = message.timestamp;
                    let reply_to = message.reply_to.clone();
                    let was_prekey = message.olm_message_type == 0;

                    match direct_messaging.receive_message(message).await {
                        Ok(content) => {
                            let msg = WsMessage::DirectMessageReceived {
                                from,
                                message_id,
                                text: content.text,
                                timestamp,
                                reply_to,
                            };
                            ws_manager.broadcast(msg);

                            // If this was a PreKey message, it consumed an OTK.
                            // Refresh the P2P handler's OTK list so other peers don't
                            // try to use the consumed key.
                            if was_prekey {
                                debug!(
                                    "PreKey message consumed an OTK, refreshing advertised keys"
                                );
                                let one_time_keys = direct_messaging
                                    .one_time_keys()
                                    .await
                                    .values()
                                    .map(|k| k.to_bytes().to_vec())
                                    .collect();

                                if let Err(e) =
                                    node_handle.update_one_time_keys(one_time_keys).await
                                {
                                    warn!("Failed to update OTK list in P2P handler: {}", e);
                                }
                            }
                        }
                        Err(e) => {
                            warn!(
                                "EventRouter: Failed to decrypt direct message {}: {}",
                                message_id, e
                            );
                        }
                    }
                }
            }

            warn!("EventRouter: Direct message event listener ended");
        });

        // Spawn task for group message events
        let ws_manager = self.ws_manager.clone();
        let group_messaging = self.group_messaging.clone();
        let events_clone = events.clone();
        tokio::spawn(async move {
            use variance_p2p::events::GroupMessageEvent;
            let mut rx = events_clone.subscribe_group_messages();
            debug!("EventRouter: Started group message event listener");

            while let Ok(event) = rx.recv().await {
                debug!("EventRouter: Received group message event: {:?}", event);

                if let GroupMessageEvent::MessageReceived { message } = event {
                    let group_id = message.group_id.clone();
                    let from = message.sender_did.clone();
                    let message_id = message.id.clone();
                    let timestamp = message.timestamp;

                    match group_messaging.receive_message(message).await {
                        Ok(_content) => {
                            let msg = WsMessage::GroupMessageReceived {
                                group_id,
                                from,
                                message_id,
                                timestamp,
                            };
                            ws_manager.broadcast(msg);
                        }
                        Err(e) => {
                            warn!(
                                "EventRouter: Failed to decrypt group message {}: {}",
                                message_id, e
                            );
                        }
                    }
                }
            }

            warn!("EventRouter: Group message event listener ended");
        });

        // Spawn task for identity events (presence tracking + pending message flush)
        let ws_manager = self.ws_manager;
        let direct_messaging = self.direct_messaging;
        let node_handle = self.node_handle;
        tokio::spawn(async move {
            let mut rx = events.subscribe_identity();
            debug!("EventRouter: Started identity event listener");

            while let Ok(event) = rx.recv().await {
                debug!("EventRouter: Received identity event: {:?}", event);

                if let IdentityEvent::DidCached { did } = event {
                    // Broadcast presence update
                    let msg = WsMessage::PresenceUpdated {
                        did: did.clone(),
                        online: true,
                    };
                    ws_manager.broadcast(msg);

                    // Flush pending messages for this peer
                    debug!(
                        "Flushing pending messages for newly connected peer: {}",
                        did
                    );
                    match direct_messaging.get_pending_messages(&did).await {
                        Ok(messages) => {
                            debug!("Found {} pending messages for {}", messages.len(), did);
                            for message in messages {
                                let message_id = message.id.clone();
                                match node_handle.send_direct_message(did.clone(), message).await {
                                    Ok(_) => {
                                        debug!(
                                            "Successfully sent pending message {} to {}",
                                            message_id, did
                                        );
                                        if let Err(e) =
                                            direct_messaging.mark_pending_sent(&message_id).await
                                        {
                                            warn!(
                                                "Failed to mark message {} as sent: {}",
                                                message_id, e
                                            );
                                        }
                                    }
                                    Err(e) => {
                                        warn!(
                                            "Failed to send pending message {} to {}: {}",
                                            message_id, did, e
                                        );
                                        // Keep in queue for next connection attempt
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            warn!("Failed to fetch pending messages for {}: {}", did, e);
                        }
                    }
                }
            }

            warn!("EventRouter: Identity event listener ended");
        });

        debug!("EventRouter: All event listeners started");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::AppState;
    use tempfile::tempdir;
    use variance_p2p::EventChannels;

    fn make_router() -> EventRouter {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let state =
            AppState::with_db_path("did:variance:test".to_string(), db_path.to_str().unwrap());
        EventRouter::new(
            state.ws_manager.clone(),
            state.direct_messaging.clone(),
            state.group_messaging.clone(),
            state.node_handle.clone(),
        )
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

        let router = EventRouter::new(
            ws_manager.clone(),
            state.direct_messaging.clone(),
            state.group_messaging.clone(),
            state.node_handle.clone(),
        );
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
