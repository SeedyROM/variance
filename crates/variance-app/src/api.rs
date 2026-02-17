use crate::{state::AppState, Error, Result};
use axum::{
    extract::{Path, State},
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
    pub verifying_key: String,
    pub created_at: String,
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
}

#[derive(Debug, Serialize, Deserialize)]
pub struct StartConversationResponse {
    pub conversation_id: String,
    pub message_id: String,
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
    Json(IdentityStatusResponse {
        did: state.local_did.clone(),
        verifying_key: state.verifying_key.clone(),
        created_at: state.created_at.clone(),
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
        .direct_messaging
        .send_message(req.recipient_did.clone(), content)
        .await
        .map_err(|e| Error::App {
            message: format!("Failed to send message: {}", e),
        })?;

    // Emit event if event channels are available
    if let Some(ref channels) = state.event_channels {
        channels.send_direct_message(DirectMessageEvent::MessageSent {
            message_id: message.id.clone(),
            recipient: req.recipient_did,
        });
    }

    Ok(Json(MessageResponse {
        message_id: message.id,
        success: true,
        message: "Message sent successfully".to_string(),
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

async fn get_direct_messages(
    State(state): State<AppState>,
    Path(did): Path<String>,
) -> Result<Json<Vec<DirectMessageResponse>>> {
    // Get messages from storage
    let messages = state
        .storage
        .as_ref()
        .fetch_direct(&state.local_did, &did, 50, None)
        .await
        .map_err(|e| Error::App {
            message: format!("Failed to get messages: {}", e),
        })?;

    // Convert to response format
    // Note: We can't decrypt here without the session, so just return metadata
    let responses = messages
        .iter()
        .map(|m| DirectMessageResponse {
            id: m.id.clone(),
            sender_did: m.sender_did.clone(),
            recipient_did: m.recipient_did.clone(),
            text: "[encrypted]".to_string(), // Would need decryption
            timestamp: m.timestamp,
            reply_to: m.reply_to.clone(),
        })
        .collect();

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

    // Convert to response format
    let responses = messages
        .iter()
        .map(|m| GroupMessageResponse {
            id: m.id.clone(),
            sender_did: m.sender_did.clone(),
            group_id: m.group_id.clone(),
            text: "[encrypted]".to_string(), // Would need decryption
            timestamp: m.timestamp,
            reply_to: m.reply_to.clone(),
        })
        .collect();

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
            return Err(Error::App {
                message: format!("Invalid call type: {}", req.call_type),
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
            return Err(Error::App {
                message: format!("Invalid call type: {}", req.call_type),
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
            Error::App { message } => (StatusCode::BAD_REQUEST, message),
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
            nonce: vec![],
            signature: vec![],
            timestamp: 9999,
            r#type: MessageType::Text.into(),
            reply_to: None,
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
            nonce: vec![],
            signature: vec![],
            timestamp: 5000,
            r#type: MessageType::Text.into(),
            reply_to: None,
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

        let req_body = serde_json::json!({
            "recipient_did": "did:variance:bob",
            "text": "Hello!"
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
}
