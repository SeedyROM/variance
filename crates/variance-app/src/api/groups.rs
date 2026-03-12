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
    /// `"admin"`, `"moderator"`, or `"member"`.
    pub role: String,
}

#[derive(Debug, Deserialize)]
pub struct MlsAcceptWelcomeRequest {
    /// Hex-encoded TLS-serialized MLS Welcome message.
    pub mls_welcome: String,
}

// ===== Helpers =====

/// Soft cap on MLS group members. MLS commits scale quadratically, so large
/// groups hit practical performance limits. This can be raised later as the
/// protocol and implementation mature.
const MAX_GROUP_MEMBERS: usize = 100;

/// Convert a protobuf `GroupRole` integer to a human-readable label.
fn role_label(role_i32: i32) -> &'static str {
    match role_i32 {
        3 => "admin",
        2 => "moderator",
        _ => "member", // 0 (unspecified) and 1 (member) both map to "member"
    }
}

/// Look up a member's role from stored `Group` metadata.
///
/// Falls back to `"member"` when the metadata doesn't include the DID.
fn member_role_from_metadata(
    group_meta: Option<&variance_proto::messaging_proto::Group>,
    did: &str,
) -> &'static str {
    group_meta
        .and_then(|g| g.members.iter().find(|m| m.did == did))
        .map(|m| role_label(m.role))
        .unwrap_or("member")
}

/// Numeric rank for role comparisons. Higher = more privileged.
fn role_rank(role: &str) -> u8 {
    match role {
        "admin" => 3,
        "moderator" => 2,
        _ => 1,
    }
}

/// Require the local user to have at least `min_role` in the given group.
///
/// Returns `Err(Forbidden)` if the user's role is below `min_role`.
async fn require_role(state: &AppState, group_id: &str, min_role: &str) -> crate::Result<()> {
    let meta = state
        .storage
        .fetch_group_metadata(group_id)
        .await
        .ok()
        .flatten();
    let actual = member_role_from_metadata(meta.as_ref(), &state.local_did);
    if role_rank(actual) < role_rank(min_role) {
        return Err(crate::Error::Forbidden {
            message: format!("Requires {} role, but you are {}", min_role, actual),
        });
    }
    Ok(())
}

/// Check that `actor_role` outranks `target_role` (strictly higher).
fn outranks(actor_role: &str, target_role: &str) -> bool {
    role_rank(actor_role) > role_rank(target_role)
}

/// Persist the current MLS group state to storage after a mutation.
pub(super) async fn persist_mls_state(state: &AppState) {
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

        let admin_did = metadata_by_id
            .get(&group_id)
            .map(|g| g.admin_did.clone())
            .filter(|s| !s.is_empty());

        let your_role = member_role_from_metadata(metadata_by_id.get(&group_id), &state.local_did);

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
            admin_did,
            your_role: your_role.to_string(),
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

    // Load stored metadata for role info.
    let group_meta = state.storage.fetch_group_metadata(&id).await.ok().flatten();

    let members: Vec<GroupMemberInfo> = dids
        .into_iter()
        .map(|did| {
            let display_name = state.username_registry.get_display_name(&did);
            let role = member_role_from_metadata(group_meta.as_ref(), &did).to_string();
            GroupMemberInfo {
                did,
                display_name,
                role,
            }
        })
        .collect();

    Ok(Json(members))
}

/// Create a new MLS group. The local user is the sole initial member.
pub(super) async fn mls_create_group(
    State(state): State<AppState>,
    Json(req): Json<MlsCreateGroupRequest>,
) -> Result<Json<serde_json::Value>> {
    let trimmed_name = req.name.trim();
    if trimmed_name.is_empty() || trimmed_name.len() > 100 {
        return Err(Error::BadRequest {
            message: "Group name must be 1–100 characters".to_string(),
        });
    }

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
        admin_did: state.local_did.clone(),
        members: vec![variance_proto::messaging_proto::GroupMember {
            did: state.local_did.clone(),
            role: variance_proto::messaging_proto::GroupRole::Admin.into(),
            joined_at: chrono::Utc::now().timestamp_millis(),
            nickname: None,
        }],
        created_at: chrono::Utc::now().timestamp_millis(),
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

/// Invite a member to an MLS group (deferred two-phase commit flow).
///
/// The `invitee` field accepts either a DID (`did:...`) or a username
/// (e.g. `alice` or `alice#0042`). The handler:
/// 1. Resolves the invitee to a DID (if a username was given).
/// 2. Resolves the peer's identity via P2P to obtain their MLS KeyPackage.
/// 3. Performs a *deferred* MLS `add_member` (commit is NOT merged yet).
/// 4. Builds a `GroupInvitation` proto with Welcome + commit bytes.
/// 5. Stores the invite as an outbound invite (for confirm/cancel later).
/// 6. Sends the `GroupInvitation` to the invitee via Olm-encrypted DM.
///
/// The commit is **not** broadcast to existing members yet. The group stays
/// in `PendingCommit` state (blocking other MLS operations) until the
/// invitee accepts (triggers `confirm_add_member` + commit broadcast) or
/// declines / times out (triggers `cancel_add_member` rollback).
pub(super) async fn mls_invite_to_group(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<MlsInviteRequest>,
) -> Result<Json<serde_json::Value>> {
    use super::helpers::{ensure_olm_session, resolve_invitee_did, send_dm_to_peer};
    use variance_messaging::mls::MlsGroupHandler;

    if req.invitee.trim().is_empty() {
        return Err(Error::BadRequest {
            message: "invitee must not be empty".to_string(),
        });
    }

    // Only admins can invite new members.
    require_role(&state, &id, "admin").await?;

    // Enforce group size cap to avoid quadratic MLS commit overhead.
    let current_count = state
        .mls_groups
        .list_members(&id)
        .map(|m| m.len())
        .unwrap_or(0);
    if current_count >= MAX_GROUP_MEMBERS {
        return Err(Error::BadRequest {
            message: format!(
                "Group has reached the maximum of {} members",
                MAX_GROUP_MEMBERS
            ),
        });
    }

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

    // ── Deferred add: leave group in PendingCommit state ────────────
    let result = state
        .mls_groups
        .add_member_deferred(&id, key_package)
        .map_err(|e| Error::App {
            message: format!("Failed to add member to MLS group: {}", e),
        })?;

    persist_mls_state(&state).await;

    // Do NOT update group metadata yet — member is added only after accept.

    let welcome_bytes =
        MlsGroupHandler::serialize_message(&result.welcome).map_err(|e| Error::App {
            message: format!("Failed to serialize Welcome: {}", e),
        })?;

    let commit_bytes =
        MlsGroupHandler::serialize_message(&result.commit).map_err(|e| Error::App {
            message: format!("Failed to serialize commit: {}", e),
        })?;

    // ── Build GroupInvitation proto ──────────────────────────────────
    let group_meta = state.storage.fetch_group_metadata(&id).await.ok().flatten();
    let group_name = group_meta
        .as_ref()
        .map(|g| g.name.clone())
        .unwrap_or_default();
    let members = group_meta
        .as_ref()
        .map(|g| g.members.clone())
        .unwrap_or_default();

    let now = chrono::Utc::now().timestamp_millis();
    let invitation = variance_proto::messaging_proto::GroupInvitation {
        group_id: id.clone(),
        group_name: group_name.clone(),
        inviter_did: state.local_did.clone(),
        invitee_did: invitee_did.clone(),
        timestamp: now,
        members,
        mls_welcome: welcome_bytes,
        mls_commit: commit_bytes,
    };

    // ── Store as outbound invite (for confirm/cancel on response) ───
    if let Err(e) = state
        .storage
        .store_outbound_invite(&id, &invitee_did, &invitation, now)
        .await
    {
        tracing::warn!("Failed to store outbound invite: {}", e);
    }

    // ── Send GroupInvitation to invitee via Olm-encrypted DM ────────
    // Encode the proto as hex in a DM with type=mls_group_invitation.
    let invitation_hex = hex::encode(prost::Message::encode_to_vec(&invitation));

    if let Err(e) = ensure_olm_session(&state, &invitee_did).await {
        tracing::warn!(
            "Could not establish Olm session with invitee {} — \
             invitation must be delivered manually: {}",
            invitee_did,
            e
        );
    } else {
        let mut metadata = HashMap::new();
        metadata.insert("type".to_string(), "mls_group_invitation".to_string());
        metadata.insert("group_id".to_string(), id.clone());
        metadata.insert("invitation".to_string(), invitation_hex);

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
                // Control message: must not appear in conversation history.
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
                        "Failed to delete sent invitation DM from local storage: {}",
                        e
                    );
                }
            }
            Err(e) => {
                tracing::warn!("Failed to send invitation DM to {}: {}", invitee_did, e);
            }
        }
    }

    Ok(Json(serde_json::json!({
        "success": true,
        "group_id": id,
        "invitee_did": invitee_did,
        "pending": true,
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

    // Only the admin (creator) can delete a group.
    require_role(&state, &id, "admin").await?;

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

    // Only admins can kick members.
    require_role(&state, &id, "admin").await?;

    // Can't remove someone of equal or higher role.
    let meta = state.storage.fetch_group_metadata(&id).await.ok().flatten();
    let my_role = member_role_from_metadata(meta.as_ref(), &state.local_did);
    let target_role = member_role_from_metadata(meta.as_ref(), &member_did);
    if !outranks(my_role, target_role) {
        return Err(Error::Forbidden {
            message: format!(
                "Cannot remove {} (role: {}) — your role ({}) does not outrank theirs",
                member_did, target_role, my_role
            ),
        });
    }

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

    // Remove the member from stored metadata.
    if let Ok(Some(mut group_meta)) = state.storage.fetch_group_metadata(&id).await {
        group_meta.members.retain(|m| m.did != member_did);
        if let Err(e) = state.storage.store_group_metadata(&group_meta).await {
            tracing::warn!(
                "Failed to update group metadata after member removal: {}",
                e
            );
        }
    }

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
    if req.text.trim().is_empty() || req.text.len() > 4096 {
        return Err(Error::BadRequest {
            message: "Message must be 1–4096 characters".to_string(),
        });
    }

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

    // Store metadata with self as MEMBER. We don't know admin_did here
    // (the inviter may fill it later via the GroupInvitation proto).
    // Only store if no metadata exists yet (to avoid overwriting richer data).
    if let Ok(None) = state.storage.fetch_group_metadata(&group_id).await {
        let group_meta = variance_proto::messaging_proto::Group {
            id: group_id.clone(),
            members: vec![variance_proto::messaging_proto::GroupMember {
                did: state.local_did.clone(),
                role: variance_proto::messaging_proto::GroupRole::Member.into(),
                joined_at: chrono::Utc::now().timestamp_millis(),
                nickname: None,
            }],
            ..Default::default()
        };
        if let Err(e) = state.storage.store_group_metadata(&group_meta).await {
            tracing::warn!("Failed to persist group metadata on Welcome accept: {}", e);
        }
    }

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
    if req.emoji.is_empty() || req.emoji.len() > 8 {
        return Err(Error::BadRequest {
            message: "emoji must be 1–8 characters".to_string(),
        });
    }

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
