use serde::{Deserialize, Serialize};
use std::collections::HashMap;

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

// ===== MLS Types =====

/// Response type for `GET /mls/groups`.
#[derive(Debug, Serialize)]
pub struct MlsGroupInfo {
    pub id: String,
    pub name: String,
    pub member_count: usize,
    pub last_message_timestamp: Option<i64>,
    pub has_unread: bool,
}

#[derive(Deserialize)]
pub struct DirectMessagesParams {
    /// Exclusive upper bound on timestamp (ms) for cursor-based pagination.
    /// Pass the oldest message's timestamp from the current page to load the page before it.
    pub before: Option<i64>,
    /// Max messages to return. Defaults to 1024.
    pub limit: Option<usize>,
}

/// Response for the /presence endpoint
#[derive(Debug, Serialize)]
pub struct PresenceResponse {
    /// DIDs of all currently connected peers
    pub online: Vec<String>,
}
