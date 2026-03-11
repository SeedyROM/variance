//! Media-related event listeners: call events, signaling, offline messages.

use crate::websocket::{WebSocketManager, WsMessage};
use std::sync::Arc;
use tracing::{debug, warn};
use variance_media::{CallManager, SignalingHandler};
use variance_p2p::{EventChannels, NodeHandle, OfflineMessageEvent, SignalingEvent};

/// Spawn all media-related event listeners (call events, signaling, offline messages).
pub(super) fn spawn_media_listeners(
    ws_manager: WebSocketManager,
    call_manager: Arc<CallManager>,
    signaling: Arc<SignalingHandler>,
    node_handle: NodeHandle,
    events: EventChannels,
) {
    spawn_call_event_listener(
        ws_manager.clone(),
        call_manager.clone(),
        signaling,
        node_handle,
    );
    spawn_signaling_event_listener(ws_manager.clone(), events.clone());
    spawn_offline_message_listener(ws_manager, events);
}

fn spawn_call_event_listener(
    ws_manager: WebSocketManager,
    call_manager: Arc<CallManager>,
    signaling: Arc<SignalingHandler>,
    node_handle: NodeHandle,
) {
    let mut call_rx = call_manager.subscribe();
    tokio::spawn(async move {
        use variance_media::CallEvent;
        debug!("EventRouter: Started call event listener");

        while let Ok(event) = call_rx.recv().await {
            debug!("EventRouter: Received call event: {:?}", event);

            match event {
                CallEvent::StateChanged { call_id, status } => {
                    let status_str = match status {
                        variance_proto::media_proto::CallStatus::Active => "active",
                        variance_proto::media_proto::CallStatus::Failed => "failed",
                        variance_proto::media_proto::CallStatus::Ended => "ended",
                        _ => "unknown",
                    };
                    ws_manager.broadcast(WsMessage::CallStateChanged {
                        call_id,
                        status: status_str.to_string(),
                    });
                }
                CallEvent::IceCandidateGathered {
                    call_id,
                    candidate,
                    sdp_mid,
                    sdp_mline_index,
                } => {
                    // Send local ICE candidate to remote peer via P2P signaling
                    let remote_peer = call_manager.get_remote_peer(&call_id);
                    if let Some(recipient_did) = remote_peer {
                        match signaling.send_ice_candidate(
                            call_id.clone(),
                            recipient_did.clone(),
                            candidate,
                            sdp_mid.unwrap_or_default(),
                            sdp_mline_index.map(|i| i as u32),
                        ) {
                            Ok(message) => {
                                if let Err(e) = node_handle
                                    .send_signaling_message(recipient_did, message)
                                    .await
                                {
                                    warn!(
                                        "Failed to send ICE candidate for call {}: {}",
                                        call_id, e
                                    );
                                }
                            }
                            Err(e) => {
                                warn!(
                                    "Failed to create ICE candidate message for call {}: {}",
                                    call_id, e
                                );
                            }
                        }
                    } else {
                        warn!(
                            "No remote peer found for call {} to send ICE candidate",
                            call_id
                        );
                    }
                }
            }
        }

        warn!("EventRouter: Call event listener ended");
    });
}

fn spawn_signaling_event_listener(ws_manager: WebSocketManager, events: EventChannels) {
    tokio::spawn(async move {
        let mut rx = events.subscribe_signaling();
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
}

fn spawn_offline_message_listener(ws_manager: WebSocketManager, events: EventChannels) {
    tokio::spawn(async move {
        let mut rx = events.subscribe_offline_messages();
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
}
