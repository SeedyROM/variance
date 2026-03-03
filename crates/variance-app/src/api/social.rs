//! Receipt, typing indicator, and presence HTTP handlers.

use crate::{state::AppState, Error, Result};
use axum::{
    extract::{Path, State},
    response::Json,
};
use serde::Serialize;

use super::helpers::receipt_status_to_string;
use super::types::{ReceiptResponse, SendReceiptRequest, TypingRequest, TypingUsersResponse};

// ===== Receipt Handlers =====

pub(super) async fn send_delivered_receipt(
    State(state): State<AppState>,
    axum::Json(req): axum::Json<SendReceiptRequest>,
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

pub(super) async fn send_read_receipt(
    State(state): State<AppState>,
    axum::Json(req): axum::Json<SendReceiptRequest>,
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

pub(super) async fn get_receipts(
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

pub(super) async fn start_typing(
    State(state): State<AppState>,
    axum::Json(req): axum::Json<TypingRequest>,
) -> Result<Json<serde_json::Value>> {
    if req.is_group {
        // Rate-limit + sustained-composition threshold for group typing
        let indicator = state.typing.try_start_typing_group(req.recipient.clone());

        if let Some(indicator) = indicator {
            // Resolve group member DIDs for unicast fan-out
            let member_dids = state
                .mls_groups
                .list_members(&req.recipient)
                .unwrap_or_default();

            if let Err(e) = state
                .node_handle
                .broadcast_group_typing(member_dids, indicator)
                .await
            {
                tracing::debug!("Failed to broadcast group typing (best-effort): {}", e);
            }
        }
    } else {
        // Rate-limit outbound typing-start for direct messages
        let indicator = state.typing.try_start_typing_direct(req.recipient.clone());

        if let Some(indicator) = indicator {
            if let Err(e) = state
                .node_handle
                .send_typing_indicator(req.recipient, indicator)
                .await
            {
                tracing::debug!("Failed to deliver typing indicator (best-effort): {}", e);
            }
        }
    }

    Ok(Json(serde_json::json!({
        "success": true,
        "message": "Typing indicator sent"
    })))
}

pub(super) async fn stop_typing(
    State(state): State<AppState>,
    axum::Json(req): axum::Json<TypingRequest>,
) -> Result<Json<serde_json::Value>> {
    if req.is_group {
        // Privacy mitigation: suppress explicit stop-typing broadcast for groups.
        // Let the 5s timeout expire naturally on recipients instead of revealing
        // "composed but didn't send" intent. Only clear local state + cooldown so
        // the next typing-start fires immediately when the user types again.
        let group_key = format!("group:{}", req.recipient);
        state.typing.clear_cooldown(&group_key);
        state.typing.clear_compose_start(&group_key);
    } else {
        let indicator = state
            .typing
            .send_typing_direct(req.recipient.clone(), false);

        // Clear cooldown so the next typing-start sends immediately
        state.typing.clear_cooldown(&req.recipient);

        if let Err(e) = state
            .node_handle
            .send_typing_indicator(req.recipient, indicator)
            .await
        {
            tracing::debug!("Failed to deliver typing stop (best-effort): {}", e);
        }
    }

    Ok(Json(serde_json::json!({
        "success": true,
        "message": "Typing stopped"
    })))
}

pub(super) async fn get_typing_users(
    State(state): State<AppState>,
    Path(recipient): Path<String>,
) -> Json<TypingUsersResponse> {
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
pub(super) struct PresenceResponse {
    /// DIDs of all currently connected peers
    online: Vec<String>,
}

/// Returns the list of peer DIDs that are currently connected via P2P.
pub(super) async fn get_presence(State(state): State<AppState>) -> Result<Json<PresenceResponse>> {
    let online = state
        .node_handle
        .get_connected_dids()
        .await
        .map_err(|e| Error::App {
            message: format!("Failed to get connected peers: {}", e),
        })?;

    Ok(Json(PresenceResponse { online }))
}
