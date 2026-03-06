//! MLS group message handlers and helpers.

use crate::{state::AppState, Error, Result};
use axum::{
    extract::{Path, Query, State},
    response::Json,
};
use serde::Deserialize;
use std::collections::HashMap;
use variance_messaging::storage::MessageStorage;
use variance_p2p::events::GroupMessageEvent;
use variance_proto::messaging_proto::MessageContent;

use super::types::{MessageResponse, MlsGroupInfo};

// ===== Types local to this module =====

#[derive(Deserialize)]
pub(super) struct GroupMessagesParams {
    /// Cursor: exclusive upper bound key for pagination.
    before: Option<String>,
    /// Max messages to return. Defaults to 50.
    limit: Option<usize>,
}

#[derive(Debug, serde::Serialize)]
pub struct GroupMessageResponse {
    pub id: String,
    pub group_id: String,
    pub sender_did: String,
    pub text: String,
    pub timestamp: i64,
    pub reply_to: Option<String>,
    pub sender_username: Option<String>,
    pub metadata: HashMap<String, String>,
}

#[derive(Debug, Deserialize)]
pub struct MlsCreateGroupRequest {
    pub name: String,
    #[allow(dead_code)]
    pub description: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct MlsInviteRequest {
    /// DID or username of the invitee. Usernames are resolved to DIDs automatically.
    pub invitee: String,
}

#[derive(Debug, Deserialize)]
pub struct SendGroupMessageRequest {
    pub group_id: String,
    pub text: String,
    pub reply_to: Option<String>,
    #[serde(default)]
    pub metadata: HashMap<String, String>,
}

#[derive(Debug, Deserialize)]
pub struct AddGroupReactionRequest {
    pub group_id: String,
    pub emoji: String,
}

#[derive(Debug, Deserialize)]
pub struct RemoveGroupReactionRequest {
    pub group_id: String,
}

#[derive(Debug, serde::Serialize)]
pub struct GroupMemberInfo {
    pub did: String,
    pub display_name: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct MlsAcceptWelcomeRequest {
    /// Hex-encoded TLS-serialized MLS Welcome message.
    pub mls_welcome: String,
}

// ===== Helpers =====

/// Persist the current MLS group state to storage after a mutation.
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

// ===== Handlers =====

/// Fetch group messages for `GET /messages/group/{id}`.
///
/// Plaintext is served from the local AES-256-GCM cache written at send/receive
/// time. MLS forward secrecy means historical ciphertexts cannot be re-decrypted.
pub(super) async fn get_group_messages(
    State(state): State<AppState>,
    Path(group_id): Path<String>,
    Query(params): Query<GroupMessagesParams>,
) -> Result<Json<Vec<GroupMessageResponse>>> {
    let limit = params.limit.unwrap_or(50);

    // Pre-warm DID→PeerId mappings for all group members so that typing indicators
    // work immediately when the user opens the chat, without waiting for a message
    // exchange to establish the mapping.
    if let Ok(member_dids) = state.mls_groups.list_members(&group_id) {
        let node_handle = state.node_handle.clone();
        let local_did = state.local_did.clone();
        tokio::spawn(async move {
            for did in member_dids {
                if did != local_did {
                    let _ = node_handle.resolve_identity_by_did(did).await;
                }
            }
        });
    }

    let messages = state
        .storage
        .fetch_group(&group_id, limit, params.before)
        .await
        .map_err(|e| Error::App {
            message: format!("Failed to fetch group messages: {}", e),
        })?;

    // Opening a group marks it read.
    let now = chrono::Utc::now().timestamp_millis();
    let _ = state
        .storage
        .store_group_last_read_at(&state.local_did, &group_id, now)
        .await;

    let mut responses = Vec::new();
    for m in messages {
        let (text, reply_to, metadata) = match state
            .mls_groups
            .load_group_plaintext(&state.storage, &m.id)
            .await
        {
            Ok(Some(content)) => (content.text, content.reply_to, content.metadata),
            _ => (String::new(), None, Default::default()),
        };

        responses.push(GroupMessageResponse {
            id: m.id,
            group_id: m.group_id.clone(),
            sender_did: m.sender_did.clone(),
            text,
            timestamp: m.timestamp,
            reply_to,
            sender_username: state.username_registry.get_display_name(&m.sender_did),
            metadata,
        });
    }

    Ok(Json(responses))
}

/// List all MLS groups the local user is a member of.
pub(super) async fn mls_list_groups(
    State(state): State<AppState>,
) -> Result<Json<Vec<MlsGroupInfo>>> {
    let mut infos: Vec<MlsGroupInfo> = Vec::new();

    let metadata = state
        .storage
        .fetch_all_group_metadata()
        .await
        .unwrap_or_default();
    let metadata_by_id: std::collections::HashMap<String, variance_proto::messaging_proto::Group> =
        metadata.into_iter().map(|g| (g.id.clone(), g)).collect();

    for group_id in state.mls_groups.group_ids() {
        let member_count = state
            .mls_groups
            .list_members(&group_id)
            .map(|m| m.len())
            .unwrap_or(0);

        let name = metadata_by_id
            .get(&group_id)
            .map(|g| g.name.clone())
            .unwrap_or_else(|| group_id.clone());

        let last_message = state
            .storage
            .fetch_group_latest(&group_id)
            .await
            .ok()
            .flatten();
        let last_message_timestamp = last_message.as_ref().map(|m| m.timestamp);

        let last_read = state
            .storage
            .fetch_group_last_read_at(&state.local_did, &group_id)
            .await
            .unwrap_or(None)
            .unwrap_or(0);
        let has_unread = last_message_timestamp.is_some_and(|ts| ts > last_read);

        infos.push(MlsGroupInfo {
            id: group_id,
            name,
            member_count,
            last_message_timestamp,
            has_unread,
        });
    }

    infos.sort_by(|a, b| b.last_message_timestamp.cmp(&a.last_message_timestamp));

    Ok(Json(infos))
}

/// List members of a specific MLS group.
pub(super) async fn mls_list_members(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Vec<GroupMemberInfo>>> {
    let dids = state.mls_groups.list_members(&id).map_err(|e| Error::App {
        message: format!("Failed to list group members: {}", e),
    })?;

    let members: Vec<GroupMemberInfo> = dids
        .into_iter()
        .map(|did| {
            let display_name = state.username_registry.get_display_name(&did);
            GroupMemberInfo { did, display_name }
        })
        .collect();

    Ok(Json(members))
}

/// Create a new MLS group. The local user is the sole initial member.
pub(super) async fn mls_create_group(
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

    let group_meta = variance_proto::messaging_proto::Group {
        id: group_id.clone(),
        name: req.name.clone(),
        ..Default::default()
    };
    if let Err(e) = state.storage.store_group_metadata(&group_meta).await {
        tracing::warn!("Failed to persist group metadata: {}", e);
    }

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

/// Invite a member to an MLS group.
///
/// The `invitee` field accepts either a DID (`did:...`) or a username
/// (e.g. `alice` or `alice#0042`). The handler:
/// 1. Resolves the invitee to a DID (if a username was given).
/// 2. Resolves the peer's identity via P2P to obtain their MLS KeyPackage.
/// 3. Performs the MLS `add_member` operation.
/// 4. Broadcasts the commit to existing group members via GossipSub.
/// 5. Sends the Welcome to the invitee as an Olm-encrypted DM.
///
/// The invitee's event_router detects the `mls_welcome` metadata on the
/// incoming DM and calls `join_group_from_welcome` automatically.
pub(super) async fn mls_invite_to_group(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<MlsInviteRequest>,
) -> Result<Json<serde_json::Value>> {
    use super::helpers::{ensure_olm_session, resolve_invitee_did, send_dm_to_peer};
    use variance_messaging::mls::MlsGroupHandler;

    // ── Resolve invitee to a DID ────────────────────────────────────
    let invitee_did = resolve_invitee_did(&state, &req.invitee).await?;

    // ── Fetch the invitee's MLS KeyPackage via P2P identity resolution ──
    let found = state
        .node_handle
        .resolve_identity_by_did(invitee_did.clone())
        .await
        .map_err(|e| Error::App {
            message: format!(
                "Cannot reach {} via P2P to fetch their MLS KeyPackage: {}",
                invitee_did, e
            ),
        })?;

    let kp_bytes = found.mls_key_package.ok_or_else(|| Error::App {
        message: format!(
            "Peer {} does not have an MLS KeyPackage available",
            invitee_did
        ),
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

    // ── Broadcast commit to existing group members via GossipSub ────
    let topic = format!("/variance/group/{}", id);
    let commit_proto = variance_proto::messaging_proto::GroupMessage {
        id: ulid::Ulid::new().to_string(),
        sender_did: state.local_did.clone(),
        group_id: id.clone(),
        timestamp: chrono::Utc::now().timestamp_millis(),
        r#type: 0,
        reply_to: None,
        mls_ciphertext: commit_bytes.clone(),
    };
    if let Err(e) = state
        .node_handle
        .publish_group_message(topic, commit_proto)
        .await
    {
        tracing::warn!("Failed to publish MLS add-member commit: {}", e);
    }

    // ── Send Welcome to the invitee via encrypted DM ────────────────
    let welcome_hex = hex::encode(&welcome_bytes);
    if let Err(e) = ensure_olm_session(&state, &invitee_did).await {
        tracing::warn!(
            "Could not establish Olm session with invitee {} — \
             Welcome must be delivered manually: {}",
            invitee_did,
            e
        );
    } else {
        let mut metadata = HashMap::new();
        metadata.insert("type".to_string(), "mls_welcome".to_string());
        metadata.insert("group_id".to_string(), id.clone());
        metadata.insert("mls_welcome".to_string(), welcome_hex.clone());

        // Include the group name so the invitee can display it immediately.
        if let Ok(all_meta) = state.storage.fetch_all_group_metadata().await {
            if let Some(group_meta) = all_meta.into_iter().find(|g| g.id == id) {
                if !group_meta.name.is_empty() {
                    metadata.insert("group_name".to_string(), group_meta.name);
                }
            }
        }

        let content = MessageContent {
            text: String::new(),
            attachments: vec![],
            mentions: vec![],
            reply_to: None,
            metadata,
        };

        match state
            .direct_messaging
            .send_message(invitee_did.clone(), content)
            .await
        {
            Ok(message) => {
                send_dm_to_peer(&state, &invitee_did, &message).await;
                // The mls_welcome DM is a control message: it must not appear in
                // the sender's conversation history. Delete our local copy now
                // (the recipient's copy is deleted by their event_router).
                if let Err(e) = state
                    .storage
                    .delete_direct_by_id(
                        &message.sender_did,
                        &message.recipient_did,
                        message.timestamp,
                        &message.id,
                    )
                    .await
                {
                    tracing::warn!(
                        "Failed to delete sent mls_welcome DM from local storage: {}",
                        e
                    );
                }
            }
            Err(e) => {
                tracing::warn!("Failed to send Welcome DM to {}: {}", invitee_did, e);
            }
        }
    }

    Ok(Json(serde_json::json!({
        "success": true,
        "group_id": id,
        "invitee_did": invitee_did,
    })))
}

/// Leave an MLS group (sends a leave proposal).
pub(super) async fn mls_leave_group(
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
        .publish_group_message(topic.clone(), leave_proto)
        .await
    {
        tracing::warn!("Failed to publish MLS leave proposal: {}", e);
    }

    if let Err(e) = state.node_handle.unsubscribe_from_topic(topic).await {
        tracing::warn!("Failed to unsubscribe from group topic: {}", e);
    }

    state.mls_groups.remove_group(&id);
    persist_mls_state(&state).await;

    Ok(Json(serde_json::json!({
        "success": true,
        "group_id": id,
    })))
}

/// Delete a group: announce departure, unsubscribe from the topic, and purge
/// all local messages and metadata. Unlike leave, this also clears history.
pub(super) async fn mls_delete_group(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>> {
    use variance_messaging::mls::MlsGroupHandler;
    use variance_messaging::storage::MessageStorage;

    // Best-effort: send leave proposal so remaining members know we departed.
    if let Ok(leave_msg) = state.mls_groups.leave_group(&id) {
        if let Ok(leave_bytes) = MlsGroupHandler::serialize_message(&leave_msg) {
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
                tracing::warn!("Failed to publish leave proposal on delete: {}", e);
            }
        }
    }

    let topic = format!("/variance/group/{}", id);
    if let Err(e) = state.node_handle.unsubscribe_from_topic(topic).await {
        tracing::warn!("Failed to unsubscribe from group topic on delete: {}", e);
    }

    state.mls_groups.remove_group(&id);
    persist_mls_state(&state).await;

    if let Err(e) = state.storage.delete_group_messages(&id).await {
        tracing::warn!("Failed to delete group messages: {}", e);
    }
    if let Err(e) = state.storage.delete_group_metadata(&id).await {
        tracing::warn!("Failed to delete group metadata: {}", e);
    }

    Ok(Json(serde_json::json!({
        "success": true,
        "group_id": id,
    })))
}

/// Remove a member from an MLS group.
pub(super) async fn mls_remove_member(
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
pub(super) async fn mls_send_group_message(
    State(state): State<AppState>,
    Json(req): Json<SendGroupMessageRequest>,
) -> Result<Json<MessageResponse>> {
    let content = MessageContent {
        text: req.text,
        attachments: vec![],
        mentions: vec![],
        reply_to: req.reply_to,
        metadata: req.metadata,
    };

    let message_id = send_group_content(&state, &req.group_id, content).await?;

    Ok(Json(MessageResponse {
        message_id,
        success: true,
        message: "MLS message sent successfully".to_string(),
    }))
}

/// Accept an MLS Welcome to join a group.
pub(super) async fn mls_accept_welcome(
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

// ===== Group Reaction Handlers =====

/// Helper: encrypt and publish a group message carrying the given `MessageContent`.
///
/// Shared by `mls_send_group_message`, `add_group_reaction`, and
/// `remove_group_reaction` to avoid duplicating the MLS encrypt → publish →
/// persist → cache → event pipeline.
async fn send_group_content(
    state: &AppState,
    group_id: &str,
    content: MessageContent,
) -> Result<String> {
    use variance_messaging::mls::MlsGroupHandler;

    let plaintext = prost::Message::encode_to_vec(&content);

    let mls_msg = state
        .mls_groups
        .encrypt_message(group_id, &plaintext)
        .map_err(|e| Error::App {
            message: format!("Failed to encrypt MLS message: {}", e),
        })?;

    let mls_bytes = MlsGroupHandler::serialize_message(&mls_msg).map_err(|e| Error::App {
        message: format!("Failed to serialize MLS ciphertext: {}", e),
    })?;

    let message_id = ulid::Ulid::new().to_string();
    let timestamp = chrono::Utc::now().timestamp_millis();

    let message = variance_proto::messaging_proto::GroupMessage {
        id: message_id.clone(),
        sender_did: state.local_did.clone(),
        group_id: group_id.to_string(),
        timestamp,
        r#type: variance_proto::messaging_proto::MessageType::Text.into(),
        reply_to: None,
        mls_ciphertext: mls_bytes,
    };

    let topic = format!("/variance/group/{}", group_id);
    if let Err(e) = state
        .node_handle
        .publish_group_message(topic, message.clone())
        .await
    {
        tracing::warn!("Failed to publish MLS group message to GossipSub: {}", e);
    }

    if let Err(e) = state.storage.store_group(&message).await {
        tracing::warn!("Failed to store MLS group message locally: {}", e);
    }

    if let Err(e) = state
        .mls_groups
        .persist_group_plaintext(&state.storage, &message_id, &content)
        .await
    {
        tracing::warn!("Failed to cache group message plaintext: {}", e);
    }

    persist_mls_state(state).await;

    state
        .event_channels
        .send_group_message(GroupMessageEvent::MessageSent {
            message_id: message_id.clone(),
            group_id: group_id.to_string(),
        });

    Ok(message_id)
}

/// Add a reaction to a group message.
///
/// Reactions are regular MLS-encrypted group messages with special metadata,
/// mirroring the same technique used for DM reactions.
pub(super) async fn add_group_reaction(
    State(state): State<AppState>,
    Path(message_id): Path<String>,
    Json(req): Json<AddGroupReactionRequest>,
) -> Result<Json<MessageResponse>> {
    let mut metadata = HashMap::new();
    metadata.insert("type".to_string(), "reaction".to_string());
    metadata.insert("message_id".to_string(), message_id);
    metadata.insert("emoji".to_string(), req.emoji);
    metadata.insert("action".to_string(), "add".to_string());

    let content = MessageContent {
        text: String::new(),
        attachments: vec![],
        mentions: vec![],
        reply_to: None,
        metadata,
    };

    let id = send_group_content(&state, &req.group_id, content).await?;

    Ok(Json(MessageResponse {
        message_id: id,
        success: true,
        message: "Reaction sent".to_string(),
    }))
}

/// Remove a reaction from a group message.
pub(super) async fn remove_group_reaction(
    State(state): State<AppState>,
    Path((message_id, emoji)): Path<(String, String)>,
    Json(req): Json<RemoveGroupReactionRequest>,
) -> Result<Json<MessageResponse>> {
    let mut metadata = HashMap::new();
    metadata.insert("type".to_string(), "reaction".to_string());
    metadata.insert("message_id".to_string(), message_id);
    metadata.insert("emoji".to_string(), emoji);
    metadata.insert("action".to_string(), "remove".to_string());

    let content = MessageContent {
        text: String::new(),
        attachments: vec![],
        mentions: vec![],
        reply_to: None,
        metadata,
    };

    let id = send_group_content(&state, &req.group_id, content).await?;

    Ok(Json(MessageResponse {
        message_id: id,
        success: true,
        message: "Reaction removed".to_string(),
    }))
}
