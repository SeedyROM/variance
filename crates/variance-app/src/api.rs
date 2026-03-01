use crate::{state::AppState, Error, Result};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Json, Response},
    routing::{get, post},
    Router,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::TraceLayer;
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
        .route(
            "/identity/username/resolve/{username}",
            get(resolve_username),
        )
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
        .route(
            "/messages/direct/{message_id}/reactions",
            post(add_reaction),
        )
        .route(
            "/messages/direct/{message_id}/reactions/{emoji}",
            axum::routing::delete(remove_reaction),
        )
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
        // MLS group endpoints (RFC 9420)
        .route("/mls/groups", post(mls_create_group))
        .route("/mls/groups/{id}/invite", post(mls_invite_to_group))
        .route("/mls/groups/{id}/leave", post(mls_leave_group))
        .route(
            "/mls/groups/{id}/members/{member_did}",
            axum::routing::delete(mls_remove_member),
        )
        .route("/mls/messages/group", post(mls_send_group_message))
        .route("/mls/welcome/accept", post(mls_accept_welcome))
        // Receipt endpoints
        .route("/receipts/delivered", post(send_delivered_receipt))
        .route("/receipts/read", post(send_read_receipt))
        .route("/receipts/{message_id}", get(get_receipts))
        // Typing endpoints
        .route("/typing/start", post(start_typing))
        .route("/typing/stop", post(stop_typing))
        .route("/typing/{recipient}", get(get_typing_users))
        // Presence endpoint
        .route("/presence", get(get_presence))
        .layer(cors)
        .layer(TraceLayer::new_for_http())
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
    pub sender_username: Option<String>,
    pub metadata: HashMap<String, String>,
}

#[derive(Debug, Deserialize)]
pub struct AddReactionRequest {
    pub emoji: String,
    pub recipient_did: String,
}

#[derive(Debug, Deserialize)]
pub struct RemoveReactionParams {
    pub recipient_did: String,
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
    /// Username (if registered)
    pub username: Option<String>,
    /// Discriminator (if registered), e.g. 1234 for name#1234
    pub discriminator: Option<u32>,
    /// Full display name, e.g. "alice#1234"
    pub display_name: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ConversationResponse {
    pub id: String,
    pub peer_did: String,
    pub last_message_timestamp: i64,
    pub peer_username: Option<String>,
    pub has_unread: bool,
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

    let (username, discriminator, display_name) =
        match state.username_registry.get_username(&state.local_did) {
            Some((name, disc)) => (
                Some(name.clone()),
                Some(disc),
                Some(variance_identity::username::UsernameRegistry::format_username(&name, disc)),
            ),
            None => (None, None, None),
        };

    Json(IdentityStatusResponse {
        did: state.local_did.clone(),
        verifying_key: state.verifying_key.clone(),
        created_at: state.created_at.clone(),
        olm_identity_key,
        one_time_keys,
        username,
        discriminator,
        display_name,
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

    // Register locally with auto-assigned discriminator
    let (display_name, discriminator) = state
        .username_registry
        .register_local(req.username.clone(), state.local_did.clone())
        .map_err(|e| Error::App {
            message: format!("Failed to register username: {}", e),
        })?;

    // Persist username + discriminator to the identity file so it survives restarts
    if let Err(e) = persist_username_to_identity(&state.identity_path, &req.username, discriminator)
    {
        tracing::warn!("Failed to persist username to identity file: {}", e);
    }

    // Publish to DHT so other peers can find us
    state
        .node_handle
        .provide_username(&req.username)
        .await
        .map_err(|e| Error::App {
            message: format!("Failed to publish username to DHT: {}", e),
        })?;

    // Tell the P2P identity handler so responses to remote peers include our discriminator
    if let Err(e) = state
        .node_handle
        .set_local_username(req.username.clone(), discriminator)
        .await
    {
        tracing::warn!("Failed to update P2P handler with new username: {}", e);
    }

    // Notify connected peers of the rename so they update their cached display names
    if let Err(e) = state
        .node_handle
        .broadcast_username_change(state.local_did.clone(), req.username.clone(), discriminator)
        .await
    {
        tracing::warn!("Failed to broadcast username change: {}", e);
    }

    Ok(Json(serde_json::json!({
        "username": req.username,
        "discriminator": discriminator,
        "display_name": display_name,
        "did": state.local_did,
    })))
}

/// Write username + discriminator into the identity JSON file.
fn persist_username_to_identity(
    identity_path: &std::path::Path,
    username: &str,
    discriminator: u32,
) -> anyhow::Result<()> {
    let mut identity = AppState::load_identity(identity_path)?;
    identity.username = Some(username.to_string());
    identity.discriminator = Some(discriminator);
    let json = serde_json::to_string_pretty(&identity)
        .map_err(|e| anyhow::anyhow!("Failed to serialize identity: {}", e))?;
    fs::write(identity_path, json)
        .map_err(|e| anyhow::anyhow!("Failed to write identity file: {}", e))?;
    Ok(())
}

/// Resolve a username (with or without discriminator) to a DID.
///
/// Accepts formats: `name%230001` (URL-encoded `name#0001`) or just `name`.
/// Checks local registry first, then queries DHT for providers and asks them
/// for their identity via the P2P identity protocol.
async fn resolve_username(
    State(state): State<AppState>,
    Path(username): Path<String>,
) -> Result<Json<serde_json::Value>> {
    use variance_identity::username::UsernameRegistry;

    // Extract base name (strip discriminator for DHT lookup)
    let (base_name, requested_disc) = match UsernameRegistry::parse_username(&username) {
        Some((name, disc)) => (name, Some(disc)),
        None => (username.clone(), None),
    };

    // --- 1. Check local registry first ---
    if let Some(disc) = requested_disc {
        if let Some(did) = state.username_registry.lookup_exact(&base_name, disc) {
            return Ok(Json(serde_json::json!({
                "did": did,
                "username": base_name,
                "discriminator": disc,
                "display_name": UsernameRegistry::format_username(&base_name, disc),
            })));
        }
    } else {
        let local_matches = state.username_registry.lookup_all(&base_name);
        if local_matches.len() == 1 {
            let (disc, did) = &local_matches[0];
            return Ok(Json(serde_json::json!({
                "did": did,
                "username": base_name,
                "discriminator": disc,
                "display_name": UsernameRegistry::format_username(&base_name, *disc),
            })));
        } else if local_matches.len() > 1 {
            let results: Vec<serde_json::Value> = local_matches
                .iter()
                .map(|(disc, did)| {
                    serde_json::json!({
                        "did": did,
                        "username": base_name,
                        "discriminator": disc,
                        "display_name": UsernameRegistry::format_username(&base_name, *disc),
                    })
                })
                .collect();
            return Ok(Json(serde_json::json!({ "matches": results })));
        }
    }

    // --- 2. Not found locally — query DHT for username providers ---
    let providers = state
        .node_handle
        .find_username_providers(&base_name)
        .await
        .map_err(|e| Error::App {
            message: format!("DHT lookup failed: {}", e),
        })?;

    if providers.is_empty() {
        return Err(Error::NotFound {
            message: format!("No user found with username '{}'", username),
        });
    }

    // --- 3. Query each provider for their identity via P2P protocol ---
    // Build a PeerId query to ask the provider peer for their DID
    for peer_id in &providers {
        tracing::debug!(
            "Querying DHT provider {} for username '{}'",
            peer_id,
            base_name
        );

        let request = variance_proto::identity_proto::IdentityRequest {
            query: Some(
                variance_proto::identity_proto::identity_request::Query::PeerId(
                    peer_id.to_string(),
                ),
            ),
            timestamp: chrono::Utc::now().timestamp(),
            requester_did: Some(state.local_did.clone()),
        };

        match state
            .node_handle
            .send_identity_request(*peer_id, request)
            .await
        {
            Ok(response) => {
                if let Some(variance_proto::identity_proto::identity_response::Result::Found(
                    found,
                )) = response.result
                {
                    if let Some(ref doc) = found.did_document {
                        let did = doc.id.clone();

                        // Cache in the local registry so future lookups are instant.
                        // The peer includes their real discriminator in the response;
                        // fall back to 1 only if the peer hasn't registered one yet.
                        let disc = found.discriminator.unwrap_or(1);
                        let _ = state.username_registry.register_with_discriminator(
                            base_name.clone(),
                            disc,
                            did.clone(),
                        );

                        // If a specific discriminator was requested, check it matches
                        if let Some(req_disc) = requested_disc {
                            if disc != req_disc {
                                continue; // Try next provider
                            }
                        }

                        return Ok(Json(serde_json::json!({
                            "did": did,
                            "username": base_name,
                            "discriminator": disc,
                            "display_name": UsernameRegistry::format_username(&base_name, disc),
                        })));
                    }
                }
            }
            Err(e) => {
                tracing::debug!(
                    "Failed to query provider {} for username '{}': {}",
                    peer_id,
                    base_name,
                    e
                );
            }
        }
    }

    Err(Error::NotFound {
        message: format!(
            "Found {} provider(s) for '{}' but none responded with a valid identity",
            providers.len(),
            username
        ),
    })
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

    let mut responses = Vec::with_capacity(conversations.len());
    for (peer_did, last_message_timestamp, last_peer_timestamp) in conversations {
        let id = conversation_id(&state.local_did, &peer_did);
        let peer_username = state.username_registry.get_display_name(&peer_did);
        let last_read = state
            .storage
            .fetch_last_read_at(&state.local_did, &peer_did)
            .await
            .unwrap_or(None)
            .unwrap_or(0);
        // Only count messages FROM the peer — never flag our own sent messages as unread.
        let has_unread = last_peer_timestamp.is_some_and(|ts| ts > last_read);
        responses.push(ConversationResponse {
            id,
            peer_did,
            last_message_timestamp,
            peer_username,
            has_unread,
        });
    }

    Ok(Json(responses))
}

async fn start_conversation(
    State(state): State<AppState>,
    Json(req): Json<StartConversationRequest>,
) -> Result<Json<StartConversationResponse>> {
    // If a conversation already exists (we have messages with this peer),
    // skip Olm session setup entirely — just send via the existing session.
    // Re-establishing a session when one exists can overwrite the old one,
    // making the peer unable to decrypt prior messages.
    let existing = state
        .storage
        .list_direct_conversations(&state.local_did)
        .await
        .unwrap_or_default();
    let conversation_exists = existing.iter().any(|(did, _, _)| did == &req.recipient_did);

    if conversation_exists && state.direct_messaging.has_session(&req.recipient_did).await {
        tracing::debug!(
            "Conversation with {} already exists, sending via existing session",
            req.recipient_did
        );

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
            .map_err(|e| Error::App {
                message: format!("Failed to send message: {}", e),
            })?;

        // Transmit over P2P (queue if peer offline)
        if let Err(e) = state
            .node_handle
            .send_direct_message(req.recipient_did.clone(), message.clone())
            .await
        {
            let err_msg = e.to_string();
            if err_msg.contains("Unknown peer DID") {
                tracing::debug!(
                    "Peer {} is offline, queuing message for later delivery",
                    req.recipient_did
                );
                let _ = state
                    .direct_messaging
                    .queue_pending_message(&req.recipient_did, message.clone())
                    .await;
            }
        }

        if let Some(ref channels) = state.event_channels {
            channels.send_direct_message(variance_p2p::events::DirectMessageEvent::MessageSent {
                message_id: message.id.clone(),
                recipient: req.recipient_did.clone(),
            });
        }

        let conversation_id = conversation_id(&state.local_did, &req.recipient_did);
        return Ok(Json(StartConversationResponse {
            conversation_id,
            message_id: message.id,
        }));
    }

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
    use variance_messaging::storage::MessageStorage;

    let limit = params.limit.unwrap_or(1024);
    let messages = state
        .storage
        .as_ref()
        .fetch_direct(&state.local_did, &did, limit, params.before)
        .await
        .map_err(|e| Error::App {
            message: format!("Failed to get messages: {}", e),
        })?;

    // Opening a conversation marks it read.
    let now = chrono::Utc::now().timestamp_millis();
    let _ = state
        .storage
        .store_last_read_at(&state.local_did, &did, now)
        .await;

    // Decrypt each message (uses cache for sent messages, decrypts received messages)
    let mut responses = Vec::new();
    for m in messages {
        let (text, metadata) = match state.direct_messaging.get_message_content(&m).await {
            Ok(content) => (content.text, content.metadata),
            Err(e) => {
                tracing::warn!("Failed to get message content for {}: {}", m.id, e);
                ("[decryption failed]".to_string(), Default::default())
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
            sender_username: state.username_registry.get_display_name(&m.sender_did),
            metadata,
        });
    }

    Ok(Json(responses))
}

// ===== Reaction Handlers =====

/// Send a reaction to a direct message.
///
/// Reactions are regular encrypted messages with special metadata so they travel
/// through the same Olm path and get stored in the same sled tree. The frontend
/// squashes them into per-emoji counts when rendering.
async fn add_reaction(
    State(state): State<AppState>,
    Path(message_id): Path<String>,
    Json(req): Json<AddReactionRequest>,
) -> Result<Json<MessageResponse>> {
    if !state.direct_messaging.has_session(&req.recipient_did).await {
        return Err(Error::SessionRequired {
            message: "No session with peer — open a conversation first".to_string(),
        });
    }

    let mut metadata = HashMap::new();
    metadata.insert("type".to_string(), "reaction".to_string());
    metadata.insert("message_id".to_string(), message_id.clone());
    metadata.insert("emoji".to_string(), req.emoji.clone());
    metadata.insert("action".to_string(), "add".to_string());

    let content = MessageContent {
        text: String::new(),
        attachments: vec![],
        mentions: vec![],
        reply_to: None,
        metadata,
    };

    let message = state
        .direct_messaging
        .send_message(req.recipient_did.clone(), content)
        .await
        .map_err(|e| Error::App {
            message: format!("Failed to send reaction: {}", e),
        })?;

    let status = match state
        .node_handle
        .send_direct_message(req.recipient_did.clone(), message.clone())
        .await
    {
        Ok(_) => "sent",
        Err(e) => {
            if e.to_string().contains("Unknown peer DID") {
                let _ = state
                    .direct_messaging
                    .queue_pending_message(&req.recipient_did, message.clone())
                    .await;
                "pending"
            } else {
                "sent"
            }
        }
    };

    if let Some(ref channels) = state.event_channels {
        channels.send_direct_message(DirectMessageEvent::MessageSent {
            message_id: message.id.clone(),
            recipient: req.recipient_did.clone(),
        });
    }

    Ok(Json(MessageResponse {
        message_id: message.id,
        success: true,
        message: format!("Reaction {}", status),
    }))
}

/// Remove a reaction from a direct message (sends a reaction message with action="remove").
async fn remove_reaction(
    State(state): State<AppState>,
    Path((message_id, emoji)): Path<(String, String)>,
    Query(params): Query<RemoveReactionParams>,
) -> Result<Json<MessageResponse>> {
    if !state
        .direct_messaging
        .has_session(&params.recipient_did)
        .await
    {
        return Err(Error::SessionRequired {
            message: "No session with peer".to_string(),
        });
    }

    let mut metadata = HashMap::new();
    metadata.insert("type".to_string(), "reaction".to_string());
    metadata.insert("message_id".to_string(), message_id.clone());
    metadata.insert("emoji".to_string(), emoji.clone());
    metadata.insert("action".to_string(), "remove".to_string());

    let content = MessageContent {
        text: String::new(),
        attachments: vec![],
        mentions: vec![],
        reply_to: None,
        metadata,
    };

    let message = state
        .direct_messaging
        .send_message(params.recipient_did.clone(), content)
        .await
        .map_err(|e| Error::App {
            message: format!("Failed to send reaction removal: {}", e),
        })?;

    let status = match state
        .node_handle
        .send_direct_message(params.recipient_did.clone(), message.clone())
        .await
    {
        Ok(_) => "sent",
        Err(e) => {
            if e.to_string().contains("Unknown peer DID") {
                let _ = state
                    .direct_messaging
                    .queue_pending_message(&params.recipient_did, message.clone())
                    .await;
                "pending"
            } else {
                "sent"
            }
        }
    };

    if let Some(ref channels) = state.event_channels {
        channels.send_direct_message(DirectMessageEvent::MessageSent {
            message_id: message.id.clone(),
            recipient: params.recipient_did.clone(),
        });
    }

    Ok(Json(MessageResponse {
        message_id: message.id,
        success: true,
        message: format!("Reaction removed {}", status),
    }))
}

// ===== MLS Group Handlers =====

#[derive(Debug, Deserialize)]
pub struct MlsCreateGroupRequest {
    pub name: String,
    pub description: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct MlsInviteRequest {
    pub invitee_did: String,
    /// Hex-encoded TLS-serialized MLS KeyPackage from the invitee's identity response.
    pub mls_key_package: String,
}

#[derive(Debug, Deserialize)]
pub struct MlsSendGroupMessageRequest {
    pub group_id: String,
    pub text: String,
    pub reply_to: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct MlsAcceptWelcomeRequest {
    /// Hex-encoded TLS-serialized MLS Welcome message.
    pub mls_welcome: String,
}

/// Persist the current MLS group state to storage after a mutation.
///
/// Called after every operation that changes the openmls key store (create, add, remove,
/// join, send, process). Logs a warning on failure — the group still works, it just won't
/// survive a restart until the next successful persist.
async fn persist_mls_state(state: &AppState) {
    match state.mls_groups.export_state() {
        Ok(bytes) => {
            if let Err(e) = state
                .storage
                .store_mls_state(&state.local_did, &bytes)
                .await
            {
                tracing::warn!("Failed to persist MLS state: {}", e);
            }
        }
        Err(e) => tracing::warn!("Failed to export MLS state for persistence: {}", e),
    }
}

/// Create a new MLS group. The local user is the sole initial member.
async fn mls_create_group(
    State(state): State<AppState>,
    Json(req): Json<MlsCreateGroupRequest>,
) -> Result<Json<serde_json::Value>> {
    let group_id = ulid::Ulid::new().to_string();

    state
        .mls_groups
        .create_group(&group_id)
        .map_err(|e| Error::App {
            message: format!("Failed to create MLS group: {}", e),
        })?;

    persist_mls_state(&state).await;

    let topic = format!("/variance/group/{}", group_id);
    if let Err(e) = state.node_handle.subscribe_to_topic(topic).await {
        tracing::warn!("Failed to subscribe to MLS group topic: {}", e);
    }

    Ok(Json(serde_json::json!({
        "success": true,
        "group_id": group_id,
        "name": req.name,
        "mls": true,
    })))
}

/// Invite a member to an MLS group using their KeyPackage.
async fn mls_invite_to_group(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<MlsInviteRequest>,
) -> Result<Json<serde_json::Value>> {
    use variance_messaging::mls::MlsGroupHandler;

    let kp_bytes = hex::decode(&req.mls_key_package).map_err(|_| Error::BadRequest {
        message: "Invalid hex-encoded MLS KeyPackage".to_string(),
    })?;

    let kp_in = MlsGroupHandler::deserialize_key_package(&kp_bytes).map_err(|e| Error::App {
        message: format!("Failed to deserialize KeyPackage: {}", e),
    })?;

    let key_package = state
        .mls_groups
        .validate_key_package(kp_in)
        .map_err(|e| Error::App {
            message: format!("Invalid KeyPackage: {}", e),
        })?;

    let result = state
        .mls_groups
        .add_member(&id, key_package)
        .map_err(|e| Error::App {
            message: format!("Failed to add member to MLS group: {}", e),
        })?;

    persist_mls_state(&state).await;

    let welcome_bytes =
        MlsGroupHandler::serialize_message(&result.welcome).map_err(|e| Error::App {
            message: format!("Failed to serialize Welcome: {}", e),
        })?;

    let commit_bytes =
        MlsGroupHandler::serialize_message(&result.commit).map_err(|e| Error::App {
            message: format!("Failed to serialize commit: {}", e),
        })?;

    Ok(Json(serde_json::json!({
        "success": true,
        "group_id": id,
        "invitee_did": req.invitee_did,
        "mls_welcome": hex::encode(&welcome_bytes),
        "mls_commit": hex::encode(&commit_bytes),
    })))
}

/// Leave an MLS group (sends a leave proposal).
async fn mls_leave_group(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>> {
    use variance_messaging::mls::MlsGroupHandler;

    let leave_msg = state.mls_groups.leave_group(&id).map_err(|e| Error::App {
        message: format!("Failed to leave MLS group: {}", e),
    })?;

    let leave_bytes = MlsGroupHandler::serialize_message(&leave_msg).map_err(|e| Error::App {
        message: format!("Failed to serialize leave proposal: {}", e),
    })?;

    // Publish leave proposal to GossipSub so remaining members process it
    let topic = format!("/variance/group/{}", id);
    let leave_proto = variance_proto::messaging_proto::GroupMessage {
        id: ulid::Ulid::new().to_string(),
        sender_did: state.local_did.clone(),
        group_id: id.clone(),
        timestamp: chrono::Utc::now().timestamp_millis(),
        r#type: 0,
        reply_to: None,
        mls_ciphertext: leave_bytes,
    };
    if let Err(e) = state
        .node_handle
        .publish_group_message(topic, leave_proto)
        .await
    {
        tracing::warn!("Failed to publish MLS leave proposal: {}", e);
    }

    state.mls_groups.remove_group(&id);
    persist_mls_state(&state).await;

    Ok(Json(serde_json::json!({
        "success": true,
        "group_id": id,
    })))
}

/// Remove a member from an MLS group.
async fn mls_remove_member(
    State(state): State<AppState>,
    Path((id, member_did)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>> {
    use variance_messaging::mls::MlsGroupHandler;

    let member_index = state
        .mls_groups
        .find_member_index(&id, &member_did)
        .map_err(|e| Error::App {
            message: format!("Failed to find member: {}", e),
        })?
        .ok_or_else(|| Error::NotFound {
            message: format!("Member {} not found in group {}", member_did, id),
        })?;

    let result = state
        .mls_groups
        .remove_member(&id, member_index)
        .map_err(|e| Error::App {
            message: format!("Failed to remove member from MLS group: {}", e),
        })?;

    persist_mls_state(&state).await;

    let commit_bytes =
        MlsGroupHandler::serialize_message(&result.commit).map_err(|e| Error::App {
            message: format!("Failed to serialize remove commit: {}", e),
        })?;

    // Publish remove commit to GossipSub
    let topic = format!("/variance/group/{}", id);
    let remove_proto = variance_proto::messaging_proto::GroupMessage {
        id: ulid::Ulid::new().to_string(),
        sender_did: state.local_did.clone(),
        group_id: id.clone(),
        timestamp: chrono::Utc::now().timestamp_millis(),
        r#type: 0,
        reply_to: None,
        mls_ciphertext: commit_bytes,
    };
    if let Err(e) = state
        .node_handle
        .publish_group_message(topic, remove_proto)
        .await
    {
        tracing::warn!("Failed to publish MLS remove commit: {}", e);
    }

    Ok(Json(serde_json::json!({
        "success": true,
        "group_id": id,
        "removed_did": member_did,
    })))
}

/// Send a message to an MLS group.
async fn mls_send_group_message(
    State(state): State<AppState>,
    Json(req): Json<MlsSendGroupMessageRequest>,
) -> Result<Json<MessageResponse>> {
    use variance_messaging::mls::MlsGroupHandler;

    // Serialize the plaintext content as protobuf bytes
    let content = MessageContent {
        text: req.text,
        attachments: vec![],
        mentions: vec![],
        reply_to: req.reply_to,
        metadata: Default::default(),
    };
    let plaintext = prost::Message::encode_to_vec(&content);

    // Encrypt via MLS
    let mls_msg = state
        .mls_groups
        .encrypt_message(&req.group_id, &plaintext)
        .map_err(|e| Error::App {
            message: format!("Failed to encrypt MLS message: {}", e),
        })?;

    let mls_bytes = MlsGroupHandler::serialize_message(&mls_msg).map_err(|e| Error::App {
        message: format!("Failed to serialize MLS ciphertext: {}", e),
    })?;

    let message_id = ulid::Ulid::new().to_string();
    let timestamp = chrono::Utc::now().timestamp_millis();

    // Build the wire message with MLS ciphertext
    let message = variance_proto::messaging_proto::GroupMessage {
        id: message_id.clone(),
        sender_did: state.local_did.clone(),
        group_id: req.group_id.clone(),
        timestamp,
        r#type: variance_proto::messaging_proto::MessageType::Text.into(),
        reply_to: None,
        mls_ciphertext: mls_bytes,
    };

    // Publish to GossipSub
    let topic = format!("/variance/group/{}", req.group_id);
    if let Err(e) = state
        .node_handle
        .publish_group_message(topic, message.clone())
        .await
    {
        tracing::warn!("Failed to publish MLS group message to GossipSub: {}", e);
    }

    // Store in local DB so it appears in message history
    if let Err(e) = state.storage.store_group(&message).await {
        tracing::warn!("Failed to store MLS group message locally: {}", e);
    }

    // Persist MLS state — encrypt_message advances the ratchet.
    persist_mls_state(&state).await;

    if let Some(ref channels) = state.event_channels {
        channels.send_group_message(GroupMessageEvent::MessageSent {
            message_id: message_id.clone(),
            group_id: req.group_id,
        });
    }

    Ok(Json(MessageResponse {
        message_id,
        success: true,
        message: "MLS message sent successfully".to_string(),
    }))
}

/// Accept an MLS Welcome to join a group.
async fn mls_accept_welcome(
    State(state): State<AppState>,
    Json(req): Json<MlsAcceptWelcomeRequest>,
) -> Result<Json<serde_json::Value>> {
    use variance_messaging::mls::MlsGroupHandler;

    let welcome_bytes = hex::decode(&req.mls_welcome).map_err(|_| Error::BadRequest {
        message: "Invalid hex-encoded MLS Welcome".to_string(),
    })?;

    let welcome_msg =
        MlsGroupHandler::deserialize_message(&welcome_bytes).map_err(|e| Error::App {
            message: format!("Failed to deserialize MLS Welcome: {}", e),
        })?;

    let group_id = state
        .mls_groups
        .join_group_from_welcome(welcome_msg)
        .map_err(|e| Error::App {
            message: format!("Failed to join group from MLS Welcome: {}", e),
        })?;

    persist_mls_state(&state).await;

    // Subscribe to the group's GossipSub topic
    let topic = format!("/variance/group/{}", group_id);
    if let Err(e) = state.node_handle.subscribe_to_topic(topic).await {
        tracing::warn!("Failed to subscribe to MLS group topic: {}", e);
    }

    Ok(Json(serde_json::json!({
        "success": true,
        "group_id": group_id,
        "mls": true,
    })))
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
    // Rate-limit outbound typing-start to prevent per-keystroke P2P traffic.
    // Returns None (and we skip the send) if we already sent recently.
    let indicator = if req.is_group {
        state.typing.try_start_typing_group(req.recipient.clone())
    } else {
        state.typing.try_start_typing_direct(req.recipient.clone())
    };

    if let Some(indicator) = indicator {
        if let Err(e) = state
            .node_handle
            .send_typing_indicator(req.recipient, indicator)
            .await
        {
            tracing::debug!("Failed to deliver typing indicator (best-effort): {}", e);
        }
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
    let indicator = if req.is_group {
        state.typing.send_typing_group(req.recipient.clone(), false)
    } else {
        state
            .typing
            .send_typing_direct(req.recipient.clone(), false)
    };

    // Clear cooldown so the next typing-start sends immediately
    state.typing.clear_cooldown(&req.recipient);

    if let Err(e) = state
        .node_handle
        .send_typing_indicator(req.recipient, indicator)
        .await
    {
        tracing::debug!("Failed to deliver typing stop (best-effort): {}", e);
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

// ===== Presence =====

/// Response for the /presence endpoint
#[derive(Debug, Serialize)]
struct PresenceResponse {
    /// DIDs of all currently connected peers
    online: Vec<String>,
}

/// Returns the list of peer DIDs that are currently connected via P2P.
async fn get_presence(State(state): State<AppState>) -> Result<Json<PresenceResponse>> {
    let online = state
        .node_handle
        .get_connected_dids()
        .await
        .map_err(|e| Error::App {
            message: format!("Failed to get connected peers: {}", e),
        })?;

    Ok(Json(PresenceResponse { online }))
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
        // Local user sent this message, so no unread badge
        assert_eq!(arr[0]["has_unread"], false);
    }

    #[tokio::test]
    async fn test_has_unread_before_and_after_fetch() {
        use variance_messaging::storage::MessageStorage;
        use variance_proto::messaging_proto::{DirectMessage, MessageType};

        let state = test_state();

        // Simulate a message received from "did:variance:peer"
        let msg = DirectMessage {
            id: "test-unread-001".to_string(),
            sender_did: "did:variance:peer".to_string(),
            recipient_did: "did:variance:test".to_string(),
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

        // Before opening the conversation: has_unread should be true
        let response = app
            .clone()
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
        assert_eq!(arr[0]["has_unread"], true);

        // Open the conversation — GET /messages/direct/:did marks it read
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/messages/direct/did:variance:peer")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        // After opening: has_unread should be false
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
        assert_eq!(arr[0]["has_unread"], false);
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

    #[tokio::test]
    async fn test_get_identity_with_username() {
        let state = test_state();
        // Register a username
        state
            .username_registry
            .register_local("alice".to_string(), "did:variance:test".to_string())
            .unwrap();

        let app = create_router(state);

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
        assert_eq!(json["username"], "alice");
        assert!(json["discriminator"].as_u64().is_some());
        assert!(json["display_name"].as_str().unwrap().starts_with("alice#"));
    }
}
