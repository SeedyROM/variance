use crate::{state::AppState, Error, Result};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Json, Response},
    routing::{get, post},
    Router,
};
use serde::{Deserialize, Serialize};
use tower_http::cors::{Any, CorsLayer};
use variance_media::Call;
use variance_messaging::storage::MessageStorage;
use variance_p2p::events::{DirectMessageEvent, GroupMessageEvent};
use variance_proto::media_proto::{CallControlType, CallType};
use variance_proto::messaging_proto::{MessageContent, ReceiptStatus};
use vodozemac::Curve25519PublicKey;

/// Create the HTTP API router
pub fn create_router(state: AppState) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    Router::new()
        // Health check
        .route("/health", get(health_check))
        // WebSocket endpoint
        .route("/ws", get(crate::websocket::websocket_handler))
        // Identity endpoints
        .route("/identity", get(get_identity))
        .route("/identity/resolve/{did}", get(resolve_identity))
        .route("/identity/username", post(register_username))
        // Conversation endpoints
        .route("/conversations", get(list_conversations))
        .route("/conversations", post(start_conversation))
        .route(
            "/conversations/{peer_did}",
            axum::routing::delete(delete_conversation),
        )
        // Message endpoints
        .route("/messages/direct", post(send_direct_message))
        .route("/messages/direct/{did}", get(get_direct_messages))
        .route("/messages/group", post(send_group_message))
        .route("/messages/group/{group_id}", get(get_group_messages))
        // Call endpoints
        .route("/calls/create", post(create_call))
        .route("/calls/active", get(list_active_calls))
        .route("/calls/{id}/accept", post(accept_call))
        .route("/calls/{id}/reject", post(reject_call))
        .route("/calls/{id}/end", post(end_call))
        // Signaling endpoints
        .route("/signaling/offer", post(send_offer))
        .route("/signaling/answer", post(send_answer))
        .route("/signaling/ice", post(send_ice_candidate))
        .route("/signaling/control", post(send_control))
        // Receipt endpoints
        .route("/receipts/delivered", post(send_delivered_receipt))
        .route("/receipts/read", post(send_read_receipt))
        .route("/receipts/{message_id}", get(get_receipts))
        // Typing endpoints
        .route("/typing/start", post(start_typing))
        .route("/typing/stop", post(stop_typing))
        .route("/typing/{recipient}", get(get_typing_users))
        .layer(cors)
        .with_state(state)
}

// ===== Request/Response Types =====

#[derive(Debug, Serialize, Deserialize)]
pub struct CreateCallRequest {
    pub recipient_did: String,
    pub call_type: String, // "audio", "video", or "screen"
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CallResponse {
    pub call_id: String,
    pub participants: Vec<String>,
    pub call_type: String,
    pub status: String,
    pub started_at: i64,
    pub ended_at: Option<i64>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SendOfferRequest {
    pub call_id: String,
    pub recipient_did: String,
    pub sdp: String,
    pub call_type: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SendAnswerRequest {
    pub call_id: String,
    pub recipient_did: String,
    pub sdp: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SendIceCandidateRequest {
    pub call_id: String,
    pub recipient_did: String,
    pub candidate: String,
    pub sdp_mid: String,
    pub sdp_m_line_index: Option<u32>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SendControlRequest {
    pub call_id: String,
    pub recipient_did: String,
    pub control_type: String, // "ring", "accept", "reject", "hangup"
    pub reason: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SignalingResponse {
    pub success: bool,
    pub message: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SendReceiptRequest {
    pub message_id: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ReceiptResponse {
    pub message_id: String,
    pub reader_did: String,
    pub status: String,
    pub timestamp: i64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TypingRequest {
    pub recipient: String, // DID or group ID
    pub is_group: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TypingUsersResponse {
    pub users: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SendDirectMessageRequest {
    pub recipient_did: String,
    pub text: String,
    pub reply_to: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SendGroupMessageRequest {
    pub group_id: String,
    pub text: String,
    pub reply_to: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct MessageResponse {
    pub message_id: String,
    pub success: bool,
    pub message: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DirectMessageResponse {
    pub id: String,
    pub sender_did: String,
    pub recipient_did: String,
    pub text: String,
    pub timestamp: i64,
    pub reply_to: Option<String>,
    pub status: Option<String>, // "sent", "pending", or "failed"
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GroupMessageResponse {
    pub id: String,
    pub sender_did: String,
    pub group_id: String,
    pub text: String,
    pub timestamp: i64,
    pub reply_to: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct IdentityStatusResponse {
    pub did: String,
    /// Hex-encoded Ed25519 verifying key (for message signature verification)
    pub verifying_key: String,
    pub created_at: String,
    /// Hex-encoded Curve25519 Olm identity key (pass as recipient_identity_key when starting a conversation)
    pub olm_identity_key: String,
    /// Hex-encoded one-time pre-keys available for Olm session establishment (pick one as recipient_one_time_key)
    pub one_time_keys: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ConversationResponse {
    pub id: String,
    pub peer_did: String,
    pub last_message_timestamp: i64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct StartConversationRequest {
    pub recipient_did: String,
    pub text: String,
    /// Hex-encoded Curve25519 identity key of the recipient (from their DID document).
    /// Required when starting a new conversation (no existing Olm session).
    pub recipient_identity_key: Option<String>,
    /// Hex-encoded Curve25519 one-time pre-key of the recipient (from their DID document).
    /// Required alongside `recipient_identity_key` for initial session establishment.
    pub recipient_one_time_key: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct StartConversationResponse {
    pub conversation_id: String,
    pub message_id: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RegisterUsernameRequest {
    pub username: String,
}

// ===== Health Check =====

async fn health_check() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "status": "ok",
        "service": "variance-app"
    }))
}

// ===== Identity Handlers =====

async fn get_identity(State(state): State<AppState>) -> Json<IdentityStatusResponse> {
    let olm_identity_key = hex::encode(state.direct_messaging.identity_key().to_bytes());
    let one_time_keys = state
        .direct_messaging
        .one_time_keys()
        .await
        .values()
        .map(|k| hex::encode(k.to_bytes()))
        .collect();

    Json(IdentityStatusResponse {
        did: state.local_did.clone(),
        verifying_key: state.verifying_key.clone(),
        created_at: state.created_at.clone(),
        olm_identity_key,
        one_time_keys,
    })
}

/// Resolve a DID to its identity document.
///
/// Full resolution requires the peer to be reachable via P2P. Currently returns
/// the DID as-is since DHT-to-PeerId lookup is not yet wired to the API layer.
async fn resolve_identity(
    State(state): State<AppState>,
    Path(did): Path<String>,
) -> Result<Json<serde_json::Value>> {
    // Self-resolution: return our own identity without network round-trip
    if did == state.local_did {
        return Ok(Json(serde_json::json!({
            "did": did,
            "verifying_key": state.verifying_key,
            "created_at": state.created_at,
            "resolved": true,
        })));
    }

    // Remote resolution requires mapping DID → PeerId which needs DHT integration.
    // Return the DID with a flag indicating it is not yet resolved.
    Ok(Json(serde_json::json!({
        "did": did,
        "resolved": false,
    })))
}

async fn register_username(
    State(state): State<AppState>,
    Json(req): Json<RegisterUsernameRequest>,
) -> Result<Json<serde_json::Value>> {
    variance_identity::username::UsernameRegistry::validate_username(&req.username).map_err(
        |e| Error::BadRequest {
            message: format!("Invalid username: {}", e),
        },
    )?;

    state
        .node_handle
        .provide_username(&req.username)
        .await
        .map_err(|e| Error::App {
            message: format!("Failed to publish username to DHT: {}", e),
        })?;

    Ok(Json(serde_json::json!({
        "username": req.username,
        "did": state.local_did,
    })))
}

// ===== Conversation Handlers =====

async fn list_conversations(
    State(state): State<AppState>,
) -> Result<Json<Vec<ConversationResponse>>> {
    use variance_messaging::storage::MessageStorage;

    let conversations = state
        .storage
        .list_direct_conversations(&state.local_did)
        .await
        .map_err(|e| Error::App {
            message: format!("Failed to list conversations: {}", e),
        })?;

    let responses = conversations
        .into_iter()
        .map(|(peer_did, last_message_timestamp)| {
            let id = conversation_id(&state.local_did, &peer_did);
            ConversationResponse {
                id,
                peer_did,
                last_message_timestamp,
            }
        })
        .collect();

    Ok(Json(responses))
}

async fn start_conversation(
    State(state): State<AppState>,
    Json(req): Json<StartConversationRequest>,
) -> Result<Json<StartConversationResponse>> {
    // Skip Olm session setup for self-messaging (messages to yourself are unencrypted)
    let is_self_message = req.recipient_did == state.local_did;

    // Establish an Olm session with the recipient if we don't already have one.
    // Priority: caller-supplied keys (manual/test) → P2P auto-resolve → error.
    if !is_self_message && !state.direct_messaging.has_session(&req.recipient_did).await {
        let (identity_key, one_time_key) = if let (Some(ik_hex), Some(otk_hex)) =
            (&req.recipient_identity_key, &req.recipient_one_time_key)
        {
            // Keys supplied explicitly by the caller (e.g. during testing).
            let ik_bytes: [u8; 32] = hex::decode(ik_hex)
                .map_err(|_| Error::BadRequest {
                    message: "recipient_identity_key must be hex-encoded".to_string(),
                })?
                .try_into()
                .map_err(|_| Error::BadRequest {
                    message: "recipient_identity_key must be exactly 32 bytes".to_string(),
                })?;
            let otk_bytes: [u8; 32] = hex::decode(otk_hex)
                .map_err(|_| Error::BadRequest {
                    message: "recipient_one_time_key must be hex-encoded".to_string(),
                })?
                .try_into()
                .map_err(|_| Error::BadRequest {
                    message: "recipient_one_time_key must be exactly 32 bytes".to_string(),
                })?;
            (
                Curve25519PublicKey::from_bytes(ik_bytes),
                Curve25519PublicKey::from_bytes(otk_bytes),
            )
        } else {
            // Auto-resolve via P2P: ask connected peers for the recipient's Olm keys.
            let found = state
                .node_handle
                .resolve_identity_by_did(req.recipient_did.clone())
                .await
                .map_err(|e| Error::SessionRequired {
                    message: format!(
                        "Cannot start conversation: peer not reachable via P2P. \
                         Make sure both nodes are running and try again. ({})",
                        e
                    ),
                })?;

            let ik_bytes: [u8; 32] =
                found
                    .olm_identity_key
                    .try_into()
                    .map_err(|_| Error::SessionRequired {
                        message: "Peer did not provide a valid Olm identity key".to_string(),
                    })?;

            if found.one_time_keys.is_empty() {
                return Err(Error::SessionRequired {
                    message: "Peer has no one-time pre-keys available".to_string(),
                });
            }

            let identity_key_parsed = Curve25519PublicKey::from_bytes(ik_bytes);

            // Try each OTK until one succeeds (handles stale/consumed keys)
            let mut last_error = None;
            for otk in found.one_time_keys {
                let otk_bytes: [u8; 32] = match otk.try_into() {
                    Ok(b) => b,
                    Err(_) => continue, // Skip invalid keys
                };
                let one_time_key = Curve25519PublicKey::from_bytes(otk_bytes);

                match state
                    .direct_messaging
                    .init_session_if_needed(&req.recipient_did, identity_key_parsed, one_time_key)
                    .await
                {
                    Ok(_) => {
                        // Success! Session established.
                        break;
                    }
                    Err(e) => {
                        // If the error is about an unknown/consumed OTK, try the next one
                        let err_msg = e.to_string();
                        if err_msg.contains("unknown one-time key")
                            || err_msg.contains("BAD_MESSAGE_KEY_ID")
                        {
                            tracing::debug!(
                                "OTK failed (likely already consumed), trying next: {}",
                                err_msg
                            );
                            last_error = Some(e);
                            continue;
                        }
                        // For other errors, fail immediately
                        return Err(Error::App {
                            message: format!("Failed to initialize Olm session: {}", e),
                        });
                    }
                }
            }

            // If we exhausted all keys without success, return the last error
            if let Some(e) = last_error {
                if !state.direct_messaging.has_session(&req.recipient_did).await {
                    return Err(Error::SessionRequired {
                        message: format!(
                            "Failed to establish session: all provided OTKs were invalid ({}). \
                                Peer may need to refresh their keys.",
                            e
                        ),
                    });
                }
            }

            // Session should be established now (either new or existing)
            (
                identity_key_parsed,
                Curve25519PublicKey::from_bytes([0u8; 32]),
            ) // Dummy OTK
        };

        // Only call init_session_if_needed for manually-supplied keys
        if req.recipient_identity_key.is_some() {
            state
                .direct_messaging
                .init_session_if_needed(&req.recipient_did, identity_key, one_time_key)
                .await
                .map_err(|e| Error::App {
                    message: format!("Failed to initialize Olm session: {}", e),
                })?;
        }
    }

    let content = variance_proto::messaging_proto::MessageContent {
        text: req.text,
        attachments: vec![],
        mentions: vec![],
        reply_to: None,
        metadata: Default::default(),
    };

    let message = state
        .direct_messaging
        .send_message(req.recipient_did.clone(), content)
        .await
        .map_err(|e| match &e {
            variance_messaging::Error::DoubleRatchet { .. } => Error::SessionRequired {
                message: format!(
                    "No Olm session with peer. Ensure both nodes are running and retry: {}",
                    e
                ),
            },
            _ => Error::App {
                message: format!("Failed to send message: {}", e),
            },
        })?;

    // Transmit over P2P (queue if peer offline)
    match state
        .node_handle
        .send_direct_message(req.recipient_did.clone(), message.clone())
        .await
    {
        Ok(_) => {
            tracing::debug!("P2P direct message delivered to {}", req.recipient_did);
        }
        Err(e) => {
            let err_msg = e.to_string();
            if err_msg.contains("Unknown peer DID") {
                tracing::debug!(
                    "Peer {} is offline, queuing message for later delivery",
                    req.recipient_did
                );
                if let Err(queue_err) = state
                    .direct_messaging
                    .queue_pending_message(&req.recipient_did, message.clone())
                    .await
                {
                    tracing::warn!(
                        "Failed to queue pending message for {}: {}",
                        req.recipient_did,
                        queue_err
                    );
                }
            } else {
                tracing::debug!("P2P direct message delivery failed: {}", e);
            }
        }
    }

    if let Some(ref channels) = state.event_channels {
        channels.send_direct_message(variance_p2p::events::DirectMessageEvent::MessageSent {
            message_id: message.id.clone(),
            recipient: req.recipient_did.clone(),
        });
    }

    let conversation_id = conversation_id(&state.local_did, &req.recipient_did);

    Ok(Json(StartConversationResponse {
        conversation_id,
        message_id: message.id,
    }))
}

async fn delete_conversation(
    State(state): State<AppState>,
    Path(peer_did): Path<String>,
) -> Result<Json<serde_json::Value>> {
    use variance_messaging::storage::MessageStorage;

    state
        .storage
        .delete_direct_conversation(&state.local_did, &peer_did)
        .await
        .map_err(|e| Error::App {
            message: format!("Failed to delete conversation: {}", e),
        })?;

    Ok(Json(serde_json::json!({ "success": true })))
}

fn conversation_id(did1: &str, did2: &str) -> String {
    let mut dids = [did1, did2];
    dids.sort();
    format!("{}:{}", dids[0], dids[1])
}

// ===== Message Handlers =====

async fn send_direct_message(
    State(state): State<AppState>,
    Json(req): Json<SendDirectMessageRequest>,
) -> Result<Json<MessageResponse>> {
    // Skip Olm session setup for self-messaging
    let is_self_message = req.recipient_did == state.local_did;

    // Ensure Olm session exists with the recipient (auto-initialize if needed)
    if !is_self_message && !state.direct_messaging.has_session(&req.recipient_did).await {
        tracing::debug!(
            "No session exists with {}, auto-initializing via P2P...",
            req.recipient_did
        );

        // Auto-resolve via P2P: ask connected peers for the recipient's Olm keys
        let found = state
            .node_handle
            .resolve_identity_by_did(req.recipient_did.clone())
            .await
            .map_err(|e| Error::SessionRequired {
                message: format!(
                    "Cannot send message: peer not reachable via P2P. \
                     Make sure both nodes are running and connected. ({})",
                    e
                ),
            })?;

        let ik_bytes: [u8; 32] =
            found
                .olm_identity_key
                .try_into()
                .map_err(|_| Error::SessionRequired {
                    message: "Peer did not provide a valid Olm identity key".to_string(),
                })?;

        if found.one_time_keys.is_empty() {
            return Err(Error::SessionRequired {
                message: "Peer has no one-time pre-keys available".to_string(),
            });
        }

        let identity_key_parsed = Curve25519PublicKey::from_bytes(ik_bytes);

        // Try each OTK until one succeeds (handles stale/consumed keys)
        let mut last_error = None;
        for otk in found.one_time_keys {
            let otk_bytes: [u8; 32] = match otk.try_into() {
                Ok(b) => b,
                Err(_) => continue, // Skip invalid keys
            };
            let one_time_key = Curve25519PublicKey::from_bytes(otk_bytes);

            match state
                .direct_messaging
                .init_session_if_needed(&req.recipient_did, identity_key_parsed, one_time_key)
                .await
            {
                Ok(_) => {
                    tracing::debug!(
                        "Session initialized successfully with {}",
                        req.recipient_did
                    );
                    break;
                }
                Err(e) => {
                    // If the error is about an unknown/consumed OTK, try the next one
                    let err_msg = e.to_string();
                    if err_msg.contains("unknown one-time key")
                        || err_msg.contains("BAD_MESSAGE_KEY_ID")
                    {
                        tracing::debug!(
                            "OTK failed (likely already consumed), trying next: {}",
                            err_msg
                        );
                        last_error = Some(e);
                        continue;
                    }
                    // For other errors, fail immediately
                    return Err(Error::App {
                        message: format!("Failed to initialize Olm session: {}", e),
                    });
                }
            }
        }

        // If we exhausted all keys without success, return the last error
        if let Some(e) = last_error {
            if !state.direct_messaging.has_session(&req.recipient_did).await {
                return Err(Error::SessionRequired {
                    message: format!(
                        "Failed to establish session: all provided OTKs were invalid ({}). \
                            Peer may need to refresh their keys.",
                        e
                    ),
                });
            }
        }
    }

    // Create message content
    let content = MessageContent {
        text: req.text.clone(),
        attachments: vec![],
        mentions: vec![],
        reply_to: req.reply_to.clone(),
        metadata: Default::default(),
    };

    // Send message (encrypts and stores locally)
    let message = state
        .direct_messaging
        .send_message(req.recipient_did.clone(), content)
        .await
        .map_err(|e| Error::App {
            message: format!("Failed to send message: {}", e),
        })?;

    // Transmit over P2P
    let status = match state
        .node_handle
        .send_direct_message(req.recipient_did.clone(), message.clone())
        .await
    {
        Ok(_) => {
            tracing::debug!("P2P direct message delivered to {}", req.recipient_did);
            "sent"
        }
        Err(e) => {
            let err_msg = e.to_string();
            // If peer is offline (unknown DID), queue for later delivery
            if err_msg.contains("Unknown peer DID") {
                tracing::debug!(
                    "Peer {} is offline, queuing message for later delivery",
                    req.recipient_did
                );
                if let Err(queue_err) = state
                    .direct_messaging
                    .queue_pending_message(&req.recipient_did, message.clone())
                    .await
                {
                    tracing::warn!(
                        "Failed to queue pending message for {}: {}",
                        req.recipient_did,
                        queue_err
                    );
                }
                "pending"
            } else {
                // Other P2P errors (transient network issues, etc.)
                tracing::debug!("P2P direct message delivery failed: {}", e);
                "sent" // Message is stored locally, will sync later
            }
        }
    };

    // Emit event if event channels are available
    if let Some(ref channels) = state.event_channels {
        channels.send_direct_message(DirectMessageEvent::MessageSent {
            message_id: message.id.clone(),
            recipient: req.recipient_did.clone(),
        });
    }

    // Broadcast the sent message via WebSocket with full content (we already have the plaintext)
    state
        .ws_manager
        .broadcast(crate::websocket::WsMessage::DirectMessageSent {
            recipient: req.recipient_did.clone(),
            message_id: message.id.clone(),
            text: req.text.clone(),
            timestamp: message.timestamp,
            reply_to: req.reply_to.clone(),
        });

    Ok(Json(MessageResponse {
        message_id: message.id.clone(),
        success: true,
        message: format!("Message {}", status),
    }))
}

async fn send_group_message(
    State(state): State<AppState>,
    Json(req): Json<SendGroupMessageRequest>,
) -> Result<Json<MessageResponse>> {
    // Create message content
    let content = MessageContent {
        text: req.text,
        attachments: vec![],
        mentions: vec![],
        reply_to: req.reply_to,
        metadata: Default::default(),
    };

    // Send message
    let message = state
        .group_messaging
        .send_message(req.group_id.clone(), content)
        .await
        .map_err(|e| Error::App {
            message: format!("Failed to send message: {}", e),
        })?;

    // Emit event if event channels are available
    if let Some(ref channels) = state.event_channels {
        channels.send_group_message(GroupMessageEvent::MessageSent {
            message_id: message.id.clone(),
            group_id: req.group_id,
        });
    }

    Ok(Json(MessageResponse {
        message_id: message.id,
        success: true,
        message: "Message sent successfully".to_string(),
    }))
}

#[derive(Deserialize)]
struct DirectMessagesParams {
    /// Exclusive upper bound on timestamp (ms) for cursor-based pagination.
    /// Pass the oldest message's timestamp from the current page to load the page before it.
    before: Option<i64>,
    /// Max messages to return. Defaults to 1024.
    limit: Option<usize>,
}

async fn get_direct_messages(
    State(state): State<AppState>,
    Path(did): Path<String>,
    Query(params): Query<DirectMessagesParams>,
) -> Result<Json<Vec<DirectMessageResponse>>> {
    let limit = params.limit.unwrap_or(1024);
    let messages = state
        .storage
        .as_ref()
        .fetch_direct(&state.local_did, &did, limit, params.before)
        .await
        .map_err(|e| Error::App {
            message: format!("Failed to get messages: {}", e),
        })?;

    // Decrypt each message (uses cache for sent messages, decrypts received messages)
    let mut responses = Vec::new();
    for m in messages {
        let text = match state.direct_messaging.get_message_content(&m).await {
            Ok(content) => content.text,
            Err(e) => {
                tracing::warn!("Failed to get message content for {}: {}", m.id, e);
                "[decryption failed]".to_string()
            }
        };

        // Check if message is pending (only relevant for sent messages)
        let status = if m.sender_did == state.local_did {
            match state.direct_messaging.is_message_pending(&m.id).await {
                Ok(true) => Some("pending".to_string()),
                Ok(false) => Some("sent".to_string()),
                Err(e) => {
                    tracing::warn!("Failed to check pending status for {}: {}", m.id, e);
                    Some("sent".to_string()) // Default to sent on error
                }
            }
        } else {
            None // Received messages don't have status
        };

        responses.push(DirectMessageResponse {
            id: m.id.clone(),
            sender_did: m.sender_did.clone(),
            recipient_did: m.recipient_did.clone(),
            text,
            timestamp: m.timestamp,
            reply_to: m.reply_to.clone(),
            status,
        });
    }

    Ok(Json(responses))
}

async fn get_group_messages(
    State(state): State<AppState>,
    Path(group_id): Path<String>,
) -> Result<Json<Vec<GroupMessageResponse>>> {
    // Get messages from storage
    let messages = state
        .storage
        .as_ref()
        .fetch_group(&group_id, 50, None)
        .await
        .map_err(|e| Error::App {
            message: format!("Failed to get messages: {}", e),
        })?;

    // Decrypt each message
    let mut responses = Vec::new();
    for m in messages {
        let text = match state.group_messaging.receive_message(m.clone()).await {
            Ok(content) => content.text,
            Err(e) => {
                tracing::warn!("Failed to decrypt group message {}: {}", m.id, e);
                "[decryption failed]".to_string()
            }
        };

        responses.push(GroupMessageResponse {
            id: m.id.clone(),
            sender_did: m.sender_did.clone(),
            group_id: m.group_id.clone(),
            text,
            timestamp: m.timestamp,
            reply_to: m.reply_to.clone(),
        });
    }

    Ok(Json(responses))
}

// ===== Call Handlers =====

async fn create_call(
    State(state): State<AppState>,
    Json(req): Json<CreateCallRequest>,
) -> Result<Json<CallResponse>> {
    let call_type = match req.call_type.as_str() {
        "audio" => CallType::Audio,
        "video" => CallType::Video,
        "screen" => CallType::ScreenShare,
        _ => {
            return Err(Error::BadRequest {
                message: format!(
                    "Invalid call type '{}'. Expected: audio, video, screen",
                    req.call_type
                ),
            })
        }
    };

    let call = state.calls.create_call(req.recipient_did, call_type);

    Ok(Json(call_to_response(&call)))
}

async fn list_active_calls(State(state): State<AppState>) -> Json<Vec<CallResponse>> {
    let calls = state.calls.list_active_calls();
    Json(calls.iter().map(call_to_response).collect())
}

async fn accept_call(
    State(state): State<AppState>,
    Path(call_id): Path<String>,
) -> Result<Json<CallResponse>> {
    let call = state.calls.accept_call(&call_id).map_err(|e| Error::App {
        message: e.to_string(),
    })?;

    Ok(Json(call_to_response(&call)))
}

async fn reject_call(
    State(state): State<AppState>,
    Path(call_id): Path<String>,
) -> Result<Json<CallResponse>> {
    let call = state
        .calls
        .reject_call(&call_id)
        .await
        .map_err(|e| Error::App {
            message: e.to_string(),
        })?;

    Ok(Json(call_to_response(&call)))
}

async fn end_call(
    State(state): State<AppState>,
    Path(call_id): Path<String>,
) -> Result<Json<CallResponse>> {
    let call = state
        .calls
        .end_call(&call_id)
        .await
        .map_err(|e| Error::App {
            message: e.to_string(),
        })?;

    Ok(Json(call_to_response(&call)))
}

// ===== Signaling Handlers =====

async fn send_offer(
    State(state): State<AppState>,
    Json(req): Json<SendOfferRequest>,
) -> Result<Json<SignalingResponse>> {
    let call_type = match req.call_type.as_str() {
        "audio" => CallType::Audio,
        "video" => CallType::Video,
        "screen" => CallType::ScreenShare,
        _ => {
            return Err(Error::BadRequest {
                message: format!(
                    "Invalid call type '{}'. Expected: audio, video, screen",
                    req.call_type
                ),
            })
        }
    };

    // Create the signaling message
    let message = state
        .signaling
        .send_offer(
            req.call_id.clone(),
            req.recipient_did.clone(),
            req.sdp,
            call_type,
        )
        .map_err(|e| Error::App {
            message: e.to_string(),
        })?;

    // Send via P2P node
    state
        .node_handle
        .send_signaling_message(req.recipient_did, message)
        .await
        .map_err(|e| Error::App {
            message: format!("Failed to send signaling message: {}", e),
        })?;

    Ok(Json(SignalingResponse {
        success: true,
        message: "Offer sent successfully".to_string(),
    }))
}

async fn send_answer(
    State(state): State<AppState>,
    Json(req): Json<SendAnswerRequest>,
) -> Result<Json<SignalingResponse>> {
    // Create the signaling message
    let message = state
        .signaling
        .send_answer(req.call_id.clone(), req.recipient_did.clone(), req.sdp)
        .map_err(|e| Error::App {
            message: e.to_string(),
        })?;

    // Send via P2P node
    state
        .node_handle
        .send_signaling_message(req.recipient_did, message)
        .await
        .map_err(|e| Error::App {
            message: format!("Failed to send signaling message: {}", e),
        })?;

    Ok(Json(SignalingResponse {
        success: true,
        message: "Answer sent successfully".to_string(),
    }))
}

async fn send_ice_candidate(
    State(state): State<AppState>,
    Json(req): Json<SendIceCandidateRequest>,
) -> Result<Json<SignalingResponse>> {
    // Create the signaling message
    let message = state
        .signaling
        .send_ice_candidate(
            req.call_id.clone(),
            req.recipient_did.clone(),
            req.candidate,
            req.sdp_mid,
            req.sdp_m_line_index,
        )
        .map_err(|e| Error::App {
            message: e.to_string(),
        })?;

    // Send via P2P node
    state
        .node_handle
        .send_signaling_message(req.recipient_did, message)
        .await
        .map_err(|e| Error::App {
            message: format!("Failed to send signaling message: {}", e),
        })?;

    Ok(Json(SignalingResponse {
        success: true,
        message: "ICE candidate sent successfully".to_string(),
    }))
}

async fn send_control(
    State(state): State<AppState>,
    Json(req): Json<SendControlRequest>,
) -> Result<Json<SignalingResponse>> {
    let control_type = match req.control_type.as_str() {
        "ring" => CallControlType::Ring,
        "accept" => CallControlType::Accept,
        "reject" => CallControlType::Reject,
        "hangup" => CallControlType::Hangup,
        _ => {
            return Err(Error::App {
                message: format!("Invalid control type: {}", req.control_type),
            })
        }
    };

    // Create the signaling message
    let message = state
        .signaling
        .send_control(
            req.call_id.clone(),
            req.recipient_did.clone(),
            control_type,
            req.reason,
        )
        .map_err(|e| Error::App {
            message: e.to_string(),
        })?;

    // Send via P2P node
    state
        .node_handle
        .send_signaling_message(req.recipient_did, message)
        .await
        .map_err(|e| Error::App {
            message: format!("Failed to send signaling message: {}", e),
        })?;

    Ok(Json(SignalingResponse {
        success: true,
        message: "Control message sent successfully".to_string(),
    }))
}

// ===== Receipt Handlers =====

async fn send_delivered_receipt(
    State(state): State<AppState>,
    Json(req): Json<SendReceiptRequest>,
) -> Result<Json<ReceiptResponse>> {
    let receipt = state
        .receipts
        .send_delivered(req.message_id)
        .await
        .map_err(|e| Error::App {
            message: e.to_string(),
        })?;

    Ok(Json(ReceiptResponse {
        message_id: receipt.message_id,
        reader_did: receipt.reader_did,
        status: receipt_status_to_string(receipt.status),
        timestamp: receipt.timestamp,
    }))
}

async fn send_read_receipt(
    State(state): State<AppState>,
    Json(req): Json<SendReceiptRequest>,
) -> Result<Json<ReceiptResponse>> {
    let receipt = state
        .receipts
        .send_read(req.message_id)
        .await
        .map_err(|e| Error::App {
            message: e.to_string(),
        })?;

    Ok(Json(ReceiptResponse {
        message_id: receipt.message_id,
        reader_did: receipt.reader_did,
        status: receipt_status_to_string(receipt.status),
        timestamp: receipt.timestamp,
    }))
}

async fn get_receipts(
    State(state): State<AppState>,
    Path(message_id): Path<String>,
) -> Result<Json<Vec<ReceiptResponse>>> {
    let receipts = state
        .receipts
        .get_receipts(&message_id)
        .await
        .map_err(|e| Error::App {
            message: e.to_string(),
        })?;

    let responses = receipts
        .iter()
        .map(|r| ReceiptResponse {
            message_id: r.message_id.clone(),
            reader_did: r.reader_did.clone(),
            status: receipt_status_to_string(r.status),
            timestamp: r.timestamp,
        })
        .collect();

    Ok(Json(responses))
}

// ===== Typing Handlers =====

async fn start_typing(
    State(state): State<AppState>,
    Json(req): Json<TypingRequest>,
) -> Result<Json<serde_json::Value>> {
    if req.is_group {
        state.typing.send_typing_group(req.recipient, true);
    } else {
        state.typing.send_typing_direct(req.recipient, true);
    }

    Ok(Json(serde_json::json!({
        "success": true,
        "message": "Typing indicator sent"
    })))
}

async fn stop_typing(
    State(state): State<AppState>,
    Json(req): Json<TypingRequest>,
) -> Result<Json<serde_json::Value>> {
    if req.is_group {
        state.typing.send_typing_group(req.recipient, false);
    } else {
        state.typing.send_typing_direct(req.recipient, false);
    }

    Ok(Json(serde_json::json!({
        "success": true,
        "message": "Typing stopped"
    })))
}

async fn get_typing_users(
    State(state): State<AppState>,
    Path(recipient): Path<String>,
) -> Json<TypingUsersResponse> {
    // Try both direct and group - in a real implementation you'd know which one to use
    let users = if recipient.starts_with("group:") {
        state.typing.get_typing_users_group(&recipient)
    } else {
        state.typing.get_typing_users_direct(&recipient)
    };

    Json(TypingUsersResponse { users })
}

// ===== Helper Functions =====

fn call_to_response(call: &Call) -> CallResponse {
    CallResponse {
        call_id: call.id.clone(),
        participants: call.participants.clone(),
        call_type: call_type_to_string(call.call_type),
        status: call_status_to_string(call.status),
        started_at: call.started_at,
        ended_at: call.ended_at,
    }
}

fn call_type_to_string(call_type: CallType) -> String {
    match call_type {
        CallType::Unspecified => "unspecified".to_string(),
        CallType::Audio => "audio".to_string(),
        CallType::Video => "video".to_string(),
        CallType::ScreenShare => "screen".to_string(),
    }
}

fn call_status_to_string(status: variance_proto::media_proto::CallStatus) -> String {
    match status {
        variance_proto::media_proto::CallStatus::Unspecified => "unspecified".to_string(),
        variance_proto::media_proto::CallStatus::Ringing => "ringing".to_string(),
        variance_proto::media_proto::CallStatus::Connecting => "connecting".to_string(),
        variance_proto::media_proto::CallStatus::Active => "active".to_string(),
        variance_proto::media_proto::CallStatus::Ended => "ended".to_string(),
        variance_proto::media_proto::CallStatus::Failed => "failed".to_string(),
    }
}

fn receipt_status_to_string(status: i32) -> String {
    if status == ReceiptStatus::Delivered as i32 {
        "delivered".to_string()
    } else if status == ReceiptStatus::Read as i32 {
        "read".to_string()
    } else {
        "unknown".to_string()
    }
}

// ===== Error Handling =====

impl IntoResponse for Error {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            Error::BadRequest { message } => (StatusCode::BAD_REQUEST, message),
            Error::NotFound { message } => (StatusCode::NOT_FOUND, message),
            Error::SessionRequired { message } => (StatusCode::UNPROCESSABLE_ENTITY, message),
            Error::App { message } => (StatusCode::INTERNAL_SERVER_ERROR, message),
        };

        let body = Json(serde_json::json!({
            "error": message,
        }));

        (status, body).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tempfile::tempdir;
    use tower::ServiceExt;

    fn test_state() -> AppState {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        AppState::with_db_path("did:variance:test".to_string(), db_path.to_str().unwrap())
    }

    #[tokio::test]
    async fn test_health_check() {
        let app = create_router(test_state());

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_create_call() {
        let app = create_router(test_state());

        let req_body = serde_json::json!({
            "recipient_did": "did:variance:bob",
            "call_type": "audio"
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/calls/create")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&req_body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_list_active_calls() {
        let app = create_router(test_state());

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/calls/active")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_send_offer() {
        let app = create_router(test_state());

        let req_body = serde_json::json!({
            "call_id": "call123",
            "recipient_did": "did:variance:bob",
            "sdp": "v=0\r\no=- 1234 1234 IN IP4 0.0.0.0\r\n",
            "call_type": "video"
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/signaling/offer")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&req_body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_send_delivered_receipt() {
        let app = create_router(test_state());

        let req_body = serde_json::json!({
            "message_id": "msg123"
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/receipts/delivered")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&req_body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_start_typing() {
        let app = create_router(test_state());

        let req_body = serde_json::json!({
            "recipient": "did:variance:bob",
            "is_group": false
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/typing/start")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&req_body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_get_typing_users() {
        let app = create_router(test_state());

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/typing/did:variance:bob")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_invalid_call_type() {
        let app = create_router(test_state());

        let req_body = serde_json::json!({
            "recipient_did": "did:variance:bob",
            "call_type": "invalid"
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/calls/create")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&req_body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_get_identity() {
        let app = create_router(test_state());

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/identity")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["did"], "did:variance:test");
        // Olm keys must be present so peers can establish sessions
        assert!(json["olm_identity_key"].as_str().is_some());
        assert!(json["one_time_keys"].as_array().is_some());
    }

    #[tokio::test]
    async fn test_list_conversations_empty() {
        let app = create_router(test_state());

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/conversations")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(json.as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_list_conversations_with_messages() {
        use variance_messaging::storage::MessageStorage;
        use variance_proto::messaging_proto::{DirectMessage, MessageType};

        let state = test_state();

        // Store a message directly in storage
        let msg = DirectMessage {
            id: "test-msg-001".to_string(),
            sender_did: "did:variance:test".to_string(),
            recipient_did: "did:variance:peer".to_string(),
            ciphertext: vec![],
            olm_message_type: 0,
            signature: vec![],
            timestamp: 9999,
            r#type: MessageType::Text.into(),
            reply_to: None,
            sender_identity_key: None,
        };
        state.storage.store_direct(&msg).await.unwrap();

        let app = create_router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/conversations")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let arr = json.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["peer_did"], "did:variance:peer");
        assert_eq!(arr[0]["last_message_timestamp"], 9999);
    }

    #[tokio::test]
    async fn test_delete_conversation() {
        use variance_messaging::storage::MessageStorage;
        use variance_proto::messaging_proto::{DirectMessage, MessageType};

        let state = test_state();

        let msg = DirectMessage {
            id: "test-msg-002".to_string(),
            sender_did: "did:variance:test".to_string(),
            recipient_did: "did:variance:deleteme".to_string(),
            ciphertext: vec![],
            olm_message_type: 0,
            signature: vec![],
            timestamp: 5000,
            r#type: MessageType::Text.into(),
            reply_to: None,
            sender_identity_key: None,
        };
        state.storage.store_direct(&msg).await.unwrap();

        let app = create_router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/conversations/did:variance:deleteme")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_start_conversation() {
        let app = create_router(test_state());

        // Simulate Bob's Olm keys (in real usage, fetched from his DID document).
        let mut recipient_account = vodozemac::olm::Account::new();
        recipient_account.generate_one_time_keys(1);
        let identity_key_hex = hex::encode(recipient_account.curve25519_key().to_bytes());
        let otk_hex = hex::encode(
            recipient_account
                .one_time_keys()
                .values()
                .next()
                .unwrap()
                .to_bytes(),
        );

        let req_body = serde_json::json!({
            "recipient_did": "did:variance:bob",
            "text": "Hello!",
            "recipient_identity_key": identity_key_hex,
            "recipient_one_time_key": otk_hex,
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/conversations")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&req_body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(json["conversation_id"].as_str().is_some());
        assert!(json["message_id"].as_str().is_some());
    }

    #[tokio::test]
    async fn test_register_username() {
        let app = create_router(test_state());
        let req_body = serde_json::json!({ "username": "alice" });
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/identity/username")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&req_body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["username"], "alice");
    }

    #[tokio::test]
    async fn test_register_invalid_username() {
        let app = create_router(test_state());
        let req_body = serde_json::json!({ "username": "alice@bad" });
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/identity/username")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&req_body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_start_conversation_without_key_fails() {
        let app = create_router(test_state());

        // No Olm keys and no existing session: must reject with 422.
        let req_body = serde_json::json!({
            "recipient_did": "did:variance:bob",
            "text": "Hello!",
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/conversations")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&req_body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }
}
