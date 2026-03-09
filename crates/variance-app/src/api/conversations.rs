//! Conversation, direct message, and reaction HTTP handlers.
//!
//! Olm session establishment and the "send over P2P, queue if offline" pattern
//! are delegated to [`super::helpers`] to avoid duplication.

use crate::{state::AppState, Error, Result};
use axum::{
    extract::{Path, Query, State},
    response::Json,
};
use serde::Deserialize;
use std::collections::HashMap;
use variance_messaging::storage::MessageStorage;
use variance_proto::messaging_proto::MessageContent;

use super::helpers::{
    conversation_id, emit_dm_sent_event, ensure_olm_session, ensure_olm_session_with_keys,
    send_dm_to_peer,
};
use super::types::{
    AddReactionRequest, ConversationResponse, DirectMessageResponse, MessageResponse,
    RemoveReactionParams, SendDirectMessageRequest, StartConversationRequest,
    StartConversationResponse,
};

// ===== Conversation Handlers =====

pub(super) async fn list_conversations(
    State(state): State<AppState>,
) -> Result<Json<Vec<ConversationResponse>>> {
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

pub(super) async fn start_conversation(
    State(state): State<AppState>,
    Json(req): Json<StartConversationRequest>,
) -> Result<Json<StartConversationResponse>> {
    if req.recipient_did.is_empty() || !req.recipient_did.starts_with("did:") {
        return Err(Error::BadRequest {
            message: "recipient_did must be a valid DID (starts with \"did:\")".to_string(),
        });
    }
    let text_len = req.text.len();
    if req.text.trim().is_empty() || text_len > 4096 {
        return Err(Error::BadRequest {
            message: "Message must be 1–4096 characters".to_string(),
        });
    }

    // If a conversation already exists (we have messages with this peer),
    // skip Olm session setup entirely — just send via the existing session.
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

        let content = MessageContent {
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

        send_dm_to_peer(&state, &req.recipient_did, &message).await;
        emit_dm_sent_event(&state, &message.id, &req.recipient_did);

        let conv_id = conversation_id(&state.local_did, &req.recipient_did);
        return Ok(Json(StartConversationResponse {
            conversation_id: conv_id,
            message_id: message.id,
        }));
    }

    // Establish an Olm session with the recipient if we don't already have one.
    // Priority: caller-supplied keys (manual/test) → P2P auto-resolve → error.
    ensure_olm_session_with_keys(
        &state,
        &req.recipient_did,
        req.recipient_identity_key.as_deref(),
        req.recipient_one_time_key.as_deref(),
    )
    .await?;

    let content = MessageContent {
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

    send_dm_to_peer(&state, &req.recipient_did, &message).await;
    emit_dm_sent_event(&state, &message.id, &req.recipient_did);

    let conv_id = conversation_id(&state.local_did, &req.recipient_did);

    Ok(Json(StartConversationResponse {
        conversation_id: conv_id,
        message_id: message.id,
    }))
}

pub(super) async fn delete_conversation(
    State(state): State<AppState>,
    Path(peer_did): Path<String>,
) -> Result<Json<serde_json::Value>> {
    state
        .storage
        .delete_direct_conversation(&state.local_did, &peer_did)
        .await
        .map_err(|e| Error::App {
            message: format!("Failed to delete conversation: {}", e),
        })?;

    Ok(Json(serde_json::json!({ "success": true })))
}

// ===== Message Handlers =====

pub(super) async fn send_direct_message(
    State(state): State<AppState>,
    Json(req): Json<SendDirectMessageRequest>,
) -> Result<Json<MessageResponse>> {
    if req.text.trim().is_empty() || req.text.len() > 4096 {
        return Err(Error::BadRequest {
            message: "Message must be 1–4096 characters".to_string(),
        });
    }

    // Ensure Olm session exists with the recipient (auto-initialize if needed)
    ensure_olm_session(&state, &req.recipient_did).await?;

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

    // Transmit over P2P (queue if peer offline)
    let status = send_dm_to_peer(&state, &req.recipient_did, &message).await;

    emit_dm_sent_event(&state, &message.id, &req.recipient_did);

    // Broadcast the sent message via WebSocket with full content
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
pub(super) struct DirectMessagesParams {
    /// Exclusive upper bound on timestamp (ms) for cursor-based pagination.
    before: Option<i64>,
    /// Max messages to return. Defaults to 1024.
    limit: Option<usize>,
}

pub(super) async fn get_direct_messages(
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

    // Opening a conversation marks it read.
    let now = chrono::Utc::now().timestamp_millis();
    let _ = state
        .storage
        .store_last_read_at(&state.local_did, &did, now)
        .await;

    // Decrypt each message
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
                Ok(false) => {
                    // Check receipt status: read > delivered > sent
                    let receipt_status = state
                        .receipts
                        .get_receipts(&m.id)
                        .await
                        .unwrap_or_default();
                    if receipt_status
                        .iter()
                        .any(|r| r.status == variance_proto::messaging_proto::ReceiptStatus::Read as i32)
                    {
                        Some("read".to_string())
                    } else if receipt_status
                        .iter()
                        .any(|r| r.status == variance_proto::messaging_proto::ReceiptStatus::Delivered as i32)
                    {
                        Some("delivered".to_string())
                    } else {
                        Some("sent".to_string())
                    }
                }
                Err(e) => {
                    tracing::warn!("Failed to check pending status for {}: {}", m.id, e);
                    Some("sent".to_string())
                }
            }
        } else {
            None
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
/// through the same Olm path and get stored in the same sled tree.
pub(super) async fn add_reaction(
    State(state): State<AppState>,
    Path(message_id): Path<String>,
    Json(req): Json<AddReactionRequest>,
) -> Result<Json<MessageResponse>> {
    if req.emoji.is_empty() || req.emoji.len() > 8 {
        return Err(Error::BadRequest {
            message: "emoji must be 1–8 characters".to_string(),
        });
    }
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

    let status = send_dm_to_peer(&state, &req.recipient_did, &message).await;
    emit_dm_sent_event(&state, &message.id, &req.recipient_did);

    Ok(Json(MessageResponse {
        message_id: message.id,
        success: true,
        message: format!("Reaction {}", status),
    }))
}

/// Remove a reaction from a direct message (sends a reaction message with action="remove").
pub(super) async fn remove_reaction(
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

    let status = send_dm_to_peer(&state, &params.recipient_did, &message).await;
    emit_dm_sent_event(&state, &message.id, &params.recipient_did);

    Ok(Json(MessageResponse {
        message_id: message.id,
        success: true,
        message: format!("Reaction removed {}", status),
    }))
}
