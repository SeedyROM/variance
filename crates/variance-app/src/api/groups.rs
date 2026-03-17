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
    /// Receipt status for own sent messages: "sent", "delivered", or "read".
    /// `None` for messages from other members.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
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

/// A pending outbound invitation visible to group admins.
#[derive(Debug, serde::Serialize)]
pub struct OutboundInvitationInfo {
    pub invitee_did: String,
    pub invitee_display_name: Option<String>,
    pub created_at: i64,
    pub expires_at: i64,
}

#[derive(Debug, Deserialize)]
pub struct ChangeRoleRequest {
    /// New role: `"moderator"` or `"member"`.
    pub new_role: String,
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

/// Invitation timeout in ms (must match event_router::messaging::INVITE_TIMEOUT_MS).
const INVITE_TIMEOUT_MS: i64 = 5 * 60 * 1000;

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

/// Reject the request if the group is frozen (admin abandoned without succession).
async fn require_not_frozen(state: &AppState, group_id: &str) -> crate::Result<()> {
    let frozen = state
        .storage
        .fetch_group_metadata(group_id)
        .await
        .ok()
        .flatten()
        .map(|g| g.frozen)
        .unwrap_or(false);
    if frozen {
        return Err(crate::Error::Forbidden {
            message: "This group is frozen — the admin left without transferring the role. \
                      No messages, invites, kicks, or role changes are allowed."
                .to_string(),
        });
    }
    Ok(())
}

/// Return `true` if the local user is the **only** admin in the group.
async fn is_sole_admin(state: &AppState, group_id: &str) -> bool {
    let meta = state
        .storage
        .fetch_group_metadata(group_id)
        .await
        .ok()
        .flatten();
    let Some(group) = meta else { return false };
    let admin_count = group
        .members
        .iter()
        .filter(|m| m.role == variance_proto::messaging_proto::GroupRole::Admin as i32)
        .count();
    let my_role = member_role_from_metadata(Some(&group), &state.local_did);
    my_role == "admin" && admin_count == 1
}

/// Return `true` if the group has other members besides the local user.
async fn has_other_members(state: &AppState, group_id: &str) -> bool {
    let meta = state
        .storage
        .fetch_group_metadata(group_id)
        .await
        .ok()
        .flatten();
    let Some(group) = meta else { return false };
    group.members.iter().any(|m| m.did != state.local_did)
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

    // Collect other member DIDs for receipt aggregate computation.
    let other_members: Vec<String> = state
        .mls_groups
        .list_members(&group_id)
        .unwrap_or_default()
        .into_iter()
        .filter(|did| *did != state.local_did)
        .collect();

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

        // Compute receipt status for own sent messages.
        let status = if m.sender_did == state.local_did {
            Some(
                state
                    .receipts
                    .get_group_aggregate_status(&group_id, &m.id, &other_members)
                    .await
                    .unwrap_or_else(|_| "sent".to_string()),
            )
        } else {
            None
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
            status,
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

        let is_frozen = metadata_by_id
            .get(&group_id)
            .map(|g| g.frozen)
            .unwrap_or(false);

        infos.push(MlsGroupInfo {
            id: group_id,
            name,
            member_count,
            last_message_timestamp,
            has_unread,
            admin_did,
            your_role: your_role.to_string(),
            is_frozen,
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

/// List outbound (pending) invitations for a group. Admin-only.
pub(super) async fn mls_list_outbound_invitations(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Vec<OutboundInvitationInfo>>> {
    require_role(&state, &id, "admin").await?;

    let invites = state
        .storage
        .fetch_outbound_invites_for_group(&id)
        .await
        .map_err(|e| Error::App {
            message: format!("Failed to fetch outbound invitations: {}", e),
        })?;

    let results: Vec<OutboundInvitationInfo> = invites
        .into_iter()
        .map(|(invitee_did, _invitation, created_at_ms)| {
            let invitee_display_name = state.username_registry.get_display_name(&invitee_did);
            OutboundInvitationInfo {
                invitee_did,
                invitee_display_name,
                created_at: created_at_ms,
                expires_at: created_at_ms + INVITE_TIMEOUT_MS,
            }
        })
        .collect();

    Ok(Json(results))
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

    // Frozen groups cannot accept new members.
    require_not_frozen(&state, &id).await?;

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
///
/// If the local user is the sole admin and other members remain, this returns
/// an error — the admin must transfer the role first, or use the abandon
/// endpoint to leave without succession (which freezes the group).
pub(super) async fn mls_leave_group(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>> {
    // Block sole-admin leave when other members still exist.
    if is_sole_admin(&state, &id).await && has_other_members(&state, &id).await {
        return Err(Error::BadRequest {
            message: "You are the only admin. Transfer the admin role to another member \
                      before leaving, or use 'Abandon group' to leave without succession \
                      (the group will be frozen for remaining members)."
                .to_string(),
        });
    }
    use variance_messaging::mls::MlsGroupHandler;
    use variance_messaging::storage::MessageStorage;

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

    // Purge all local state for this group.
    if let Err(e) = state.storage.delete_group_messages(&id).await {
        tracing::warn!("Failed to delete group messages on leave: {}", e);
    }
    if let Err(e) = state.storage.delete_group_metadata(&id).await {
        tracing::warn!("Failed to delete group metadata on leave: {}", e);
    }
    if let Err(e) = state
        .storage
        .delete_group_last_read_at(&state.local_did, &id)
        .await
    {
        tracing::warn!("Failed to delete group last_read_at on leave: {}", e);
    }
    if let Err(e) = state
        .storage
        .delete_all_outbound_invites_for_group(&id)
        .await
    {
        tracing::warn!("Failed to delete outbound invites on leave: {}", e);
    }

    // Generate a fresh KeyPackage so we can be reinvited.
    // The previous one was consumed when we originally joined via Welcome.
    match state.mls_groups.generate_key_package() {
        Ok(kp) => match MlsGroupHandler::serialize_message_bytes(&kp) {
            Ok(kp_bytes) => {
                if let Err(e) = state.node_handle.update_mls_key_package(kp_bytes).await {
                    tracing::warn!("Failed to republish MLS KeyPackage after leave: {}", e);
                }
                persist_mls_state(&state).await;
            }
            Err(e) => tracing::warn!(
                "Failed to serialize refreshed KeyPackage after leave: {}",
                e
            ),
        },
        Err(e) => tracing::warn!("Failed to generate refreshed KeyPackage after leave: {}", e),
    }

    Ok(Json(serde_json::json!({
        "success": true,
        "group_id": id,
    })))
}

/// Abandon a group without transferring the admin role.
///
/// Admin-only. Sends a special `admin_abandoned` control message before leaving
/// so remaining members know the group is now frozen (no admin = no invites,
/// kicks, or role changes). The leaving admin's local state is fully purged,
/// identical to a normal leave.
pub(super) async fn mls_abandon_group(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>> {
    use variance_messaging::mls::MlsGroupHandler;
    use variance_messaging::storage::MessageStorage;

    require_role(&state, &id, "admin").await?;

    // Broadcast an `admin_abandoned` control message before leaving so
    // remaining peers can mark the group as frozen.
    let mut metadata = HashMap::new();
    metadata.insert("type".to_string(), "admin_abandoned".to_string());
    metadata.insert("admin_did".to_string(), state.local_did.clone());

    let content = MessageContent {
        text: String::new(),
        attachments: vec![],
        mentions: vec![],
        reply_to: None,
        metadata,
    };

    if let Err(e) = send_group_content(&state, &id, content, false).await {
        tracing::warn!("Failed to broadcast admin_abandoned to group: {}", e);
    }

    // Now perform the same leave sequence as mls_leave_group.
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
        tracing::warn!("Failed to publish MLS leave proposal (abandon): {}", e);
    }

    if let Err(e) = state.node_handle.unsubscribe_from_topic(topic).await {
        tracing::warn!("Failed to unsubscribe from group topic (abandon): {}", e);
    }

    state.mls_groups.remove_group(&id);
    persist_mls_state(&state).await;

    // Purge all local state for this group.
    if let Err(e) = state.storage.delete_group_messages(&id).await {
        tracing::warn!("Failed to delete group messages on abandon: {}", e);
    }
    if let Err(e) = state.storage.delete_group_metadata(&id).await {
        tracing::warn!("Failed to delete group metadata on abandon: {}", e);
    }
    if let Err(e) = state
        .storage
        .delete_group_last_read_at(&state.local_did, &id)
        .await
    {
        tracing::warn!("Failed to delete group last_read_at on abandon: {}", e);
    }
    if let Err(e) = state
        .storage
        .delete_all_outbound_invites_for_group(&id)
        .await
    {
        tracing::warn!("Failed to delete outbound invites on abandon: {}", e);
    }

    // Generate a fresh KeyPackage so we can be reinvited.
    match state.mls_groups.generate_key_package() {
        Ok(kp) => match MlsGroupHandler::serialize_message_bytes(&kp) {
            Ok(kp_bytes) => {
                if let Err(e) = state.node_handle.update_mls_key_package(kp_bytes).await {
                    tracing::warn!("Failed to republish MLS KeyPackage after abandon: {}", e);
                }
                persist_mls_state(&state).await;
            }
            Err(e) => tracing::warn!(
                "Failed to serialize refreshed KeyPackage after abandon: {}",
                e
            ),
        },
        Err(e) => tracing::warn!(
            "Failed to generate refreshed KeyPackage after abandon: {}",
            e
        ),
    }

    Ok(Json(serde_json::json!({
        "success": true,
        "group_id": id,
        "abandoned": true,
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
    if let Err(e) = state
        .storage
        .delete_group_last_read_at(&state.local_did, &id)
        .await
    {
        tracing::warn!("Failed to delete group last_read_at on delete: {}", e);
    }
    if let Err(e) = state
        .storage
        .delete_all_outbound_invites_for_group(&id)
        .await
    {
        tracing::warn!("Failed to delete outbound invites on delete: {}", e);
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

    // Frozen groups cannot have members removed.
    require_not_frozen(&state, &id).await?;

    // Moderators and admins can kick members.
    require_role(&state, &id, "moderator").await?;

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

/// Change a member's role within an MLS group.
///
/// Only admins can promote/demote. The target must be outranked by the actor,
/// and the new role must also be at or below the actor's own role.
/// Admins can promote others to admin (for ownership transfer).
pub(super) async fn mls_change_member_role(
    State(state): State<AppState>,
    Path((id, member_did)): Path<(String, String)>,
    Json(req): Json<ChangeRoleRequest>,
) -> Result<Json<serde_json::Value>> {
    // Frozen groups cannot have roles changed.
    require_not_frozen(&state, &id).await?;

    // Only admins can change roles.
    require_role(&state, &id, "admin").await?;

    // Validate the requested new role.
    let new_role_rank = role_rank(&req.new_role);
    if new_role_rank == 0 {
        return Err(Error::BadRequest {
            message: format!(
                "Invalid target role '{}'. Must be 'admin', 'moderator', or 'member'.",
                req.new_role,
            ),
        });
    }

    // Can't change the role of someone you don't outrank (unless promoting to admin).
    let meta = state.storage.fetch_group_metadata(&id).await.ok().flatten();
    let my_role = member_role_from_metadata(meta.as_ref(), &state.local_did);
    let target_role = member_role_from_metadata(meta.as_ref(), &member_did);
    if !outranks(my_role, target_role) && req.new_role != "admin" {
        return Err(Error::Forbidden {
            message: format!(
                "Cannot change role of {} (role: {}) — your role ({}) does not outrank theirs",
                member_did, target_role, my_role,
            ),
        });
    }

    // Map role string to protobuf i32.
    let new_role_i32 = match req.new_role.as_str() {
        "admin" => variance_proto::messaging_proto::GroupRole::Admin as i32,
        "moderator" => variance_proto::messaging_proto::GroupRole::Moderator as i32,
        _ => variance_proto::messaging_proto::GroupRole::Member as i32,
    };

    // Update local storage.
    let updated = state
        .storage
        .update_member_role(&id, &member_did, new_role_i32)
        .await
        .map_err(|e| Error::App {
            message: format!("Failed to update member role: {}", e),
        })?;

    if !updated {
        return Err(Error::NotFound {
            message: format!("Member {} not found in group {}", member_did, id),
        });
    }

    // Broadcast role change as an MLS-encrypted metadata message so all
    // peers learn about the change.
    let mut metadata = HashMap::new();
    metadata.insert("type".to_string(), "role_change".to_string());
    metadata.insert("target_did".to_string(), member_did.clone());
    metadata.insert("new_role".to_string(), req.new_role.clone());

    let content = MessageContent {
        text: String::new(),
        attachments: vec![],
        mentions: vec![],
        reply_to: None,
        metadata,
    };

    if let Err(e) = send_group_content(&state, &id, content, false).await {
        tracing::warn!("Failed to broadcast role change to group: {}", e);
    }

    Ok(Json(serde_json::json!({
        "success": true,
        "group_id": id,
        "member_did": member_did,
        "new_role": req.new_role,
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

    // Frozen groups cannot accept new messages.
    require_not_frozen(&state, &req.group_id).await?;

    let content = MessageContent {
        text: req.text,
        attachments: vec![],
        mentions: vec![],
        reply_to: req.reply_to,
        metadata: req.metadata,
    };

    let message_id = send_group_content(&state, &req.group_id, content, true).await?;

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
/// Encrypt and broadcast group content over GossipSub.
///
/// When `store` is `true` the message is persisted locally and a
/// `MessageSent` event is emitted (used for regular messages and
/// reactions).  Control messages like role changes set `store` to
/// `false` — they are ephemeral signals that peers process but never
/// display in chat history.
///
/// All content is wrapped in a `GroupPayload` envelope before MLS encryption,
/// enabling receipts and future payload types to share the same GossipSub topic.
async fn send_group_content(
    state: &AppState,
    group_id: &str,
    content: MessageContent,
    store: bool,
) -> Result<String> {
    use variance_messaging::mls::MlsGroupHandler;
    use variance_proto::messaging_proto::{group_payload, GroupPayload};

    let payload = GroupPayload {
        payload: Some(group_payload::Payload::Message(content.clone())),
    };
    let plaintext = prost::Message::encode_to_vec(&payload);

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

    if store {
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
    }

    persist_mls_state(state).await;

    if store {
        state
            .event_channels
            .send_group_message(GroupMessageEvent::MessageSent {
                message_id: message_id.clone(),
                group_id: group_id.to_string(),
            });
    }

    Ok(message_id)
}

/// Encrypt a `GroupReadReceipt` inside a `GroupPayload` and publish to GossipSub.
///
/// Receipts are ephemeral — they are NOT stored as `GroupMessage` rows. Only
/// the receipt table (via `ReceiptHandler`) tracks them. This keeps the chat
/// history clean while still providing delivery/read feedback.
pub(crate) async fn publish_group_receipt(
    state: &AppState,
    group_id: &str,
    receipt: variance_proto::messaging_proto::GroupReadReceipt,
) -> std::result::Result<(), String> {
    use variance_messaging::mls::MlsGroupHandler;
    use variance_proto::messaging_proto::{group_payload, GroupPayload};

    let payload = GroupPayload {
        payload: Some(group_payload::Payload::Receipt(receipt.clone())),
    };
    let plaintext = prost::Message::encode_to_vec(&payload);

    let mls_msg = state
        .mls_groups
        .encrypt_message(group_id, &plaintext)
        .map_err(|e| format!("MLS encrypt receipt: {}", e))?;

    let mls_bytes = MlsGroupHandler::serialize_message(&mls_msg)
        .map_err(|e| format!("MLS serialize receipt: {}", e))?;

    // Wrap in a GroupMessage shell for GossipSub transport.
    // The id/sender_did/timestamp are for wire framing only — the
    // actual receipt data is inside the MLS ciphertext.
    let wire_msg = variance_proto::messaging_proto::GroupMessage {
        id: ulid::Ulid::new().to_string(),
        sender_did: state.local_did.clone(),
        group_id: group_id.to_string(),
        timestamp: chrono::Utc::now().timestamp_millis(),
        r#type: variance_proto::messaging_proto::MessageType::Unspecified.into(),
        reply_to: None,
        mls_ciphertext: mls_bytes,
    };

    let topic = format!("/variance/group/{}", group_id);
    state
        .node_handle
        .publish_group_message(topic, wire_msg)
        .await
        .map_err(|e| format!("GossipSub publish receipt: {}", e))?;

    // Persist MLS state — encryption advanced the ratchet.
    persist_mls_state(state).await;

    Ok(())
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

    let id = send_group_content(&state, &req.group_id, content, true).await?;

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

    let id = send_group_content(&state, &req.group_id, content, true).await?;

    Ok(Json(MessageResponse {
        message_id: id,
        success: true,
        message: "Reaction removed".to_string(),
    }))
}

// ===== Group recovery =====

/// Reinitialize a desynced MLS group.
///
/// Admin-only. Creates a fresh group under a new ID and re-invites all current
/// members whose KeyPackages can be fetched. The frontend receives a
/// `GroupReinitialized` WsMessage with the old and new group IDs.
///
/// The caller can optionally provide `key_packages` — a map of `member_did`
/// to hex-encoded TLS-serialized `KeyPackage`. Members whose packages are not
/// provided will be resolved via P2P identity queries (best-effort).
///
/// `POST /mls/groups/{id}/reinitialize`
pub(super) async fn mls_reinitialize_group(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<ReinitializeGroupRequest>,
) -> Result<Json<serde_json::Value>> {
    use variance_messaging::mls::MlsGroupHandler;

    require_role(&state, &id, "admin").await?;

    // Verify the group exists in MLS state.
    if !state.mls_groups.is_member(&id) {
        return Err(Error::App {
            message: format!("Not a member of group {id}"),
        });
    }

    // Collect KeyPackages: use provided ones first, then try P2P resolution.
    let members = state.mls_groups.list_members(&id).map_err(|e| Error::App {
        message: format!("Failed to list members: {e}"),
    })?;

    let mut key_packages = HashMap::new();

    for member_did in &members {
        if member_did == &state.local_did {
            continue;
        }

        // Check if the caller provided a KeyPackage for this member.
        if let Some(kp_hex) = req.key_packages.as_ref().and_then(|m| m.get(member_did)) {
            let kp_bytes = hex::decode(kp_hex).map_err(|e| Error::App {
                message: format!("Invalid hex KeyPackage for {member_did}: {e}"),
            })?;
            let kp_in =
                MlsGroupHandler::deserialize_key_package(&kp_bytes).map_err(|e| Error::App {
                    message: format!("Invalid KeyPackage for {member_did}: {e}"),
                })?;
            let kp = state
                .mls_groups
                .validate_key_package(kp_in)
                .map_err(|e| Error::App {
                    message: format!("KeyPackage validation failed for {member_did}: {e}"),
                })?;
            key_packages.insert(member_did.clone(), kp);
            continue;
        }

        // Try P2P resolution to get the member's KeyPackage.
        match state
            .node_handle
            .resolve_identity_by_did(member_did.clone())
            .await
        {
            Ok(identity) => {
                if let Some(kp_bytes) = identity.mls_key_package {
                    match MlsGroupHandler::deserialize_key_package(&kp_bytes) {
                        Ok(kp_in) => match state.mls_groups.validate_key_package(kp_in) {
                            Ok(kp) => {
                                key_packages.insert(member_did.clone(), kp);
                            }
                            Err(e) => {
                                tracing::warn!(
                                    "reinitialize: KeyPackage validation failed for {}: {}",
                                    member_did,
                                    e
                                );
                            }
                        },
                        Err(e) => {
                            tracing::warn!(
                                "reinitialize: Failed to deserialize KeyPackage for {}: {}",
                                member_did,
                                e
                            );
                        }
                    }
                }
            }
            Err(e) => {
                tracing::warn!(
                    "reinitialize: Failed to resolve identity for {}: {}",
                    member_did,
                    e
                );
            }
        }
    }

    // Perform the reinitialize.
    let result = state
        .mls_groups
        .reinitialize_group(&id, key_packages)
        .map_err(|e| Error::App {
            message: format!("Failed to reinitialize group: {e}"),
        })?;

    // Subscribe to the new group's GossipSub topic.
    let new_topic = format!("/variance/group/{}", result.new_group_id);
    if let Err(e) = state.node_handle.subscribe_to_topic(new_topic).await {
        tracing::warn!("reinitialize: Failed to subscribe to new topic: {}", e);
    }

    // Copy group metadata from the old group to the new one (name, etc.).
    if let Ok(Some(mut old_meta)) = state.storage.fetch_group_metadata(&id).await {
        old_meta.id = result.new_group_id.clone();
        if let Err(e) = state.storage.store_group_metadata(&old_meta).await {
            tracing::warn!("reinitialize: Failed to copy group metadata: {}", e);
        }
    }

    // Send Welcome messages to each re-invited member via Olm-encrypted DM,
    // following the same pattern as mls_invite_to_group.
    use super::helpers::ensure_olm_session;

    let group_meta = state.storage.fetch_group_metadata(&id).await.ok().flatten();
    let group_name = group_meta
        .as_ref()
        .map(|g| g.name.clone())
        .unwrap_or_default();
    let members_list = group_meta
        .as_ref()
        .map(|g| g.members.clone())
        .unwrap_or_default();

    let mut reinvited = Vec::new();
    for (member_did, welcome) in &result.welcomes {
        let welcome_bytes =
            MlsGroupHandler::serialize_message(welcome).map_err(|e| Error::App {
                message: format!("Failed to serialize Welcome for {member_did}: {e}"),
            })?;

        let invitation = variance_proto::messaging_proto::GroupInvitation {
            group_id: result.new_group_id.clone(),
            group_name: group_name.clone(),
            inviter_did: state.local_did.clone(),
            invitee_did: member_did.clone(),
            timestamp: chrono::Utc::now().timestamp_millis(),
            members: members_list.clone(),
            mls_welcome: welcome_bytes,
            mls_commit: vec![],
        };

        let invitation_hex = hex::encode(prost::Message::encode_to_vec(&invitation));

        if let Err(e) = ensure_olm_session(&state, member_did).await {
            tracing::warn!(
                "reinitialize: Could not establish Olm session with {} — \
                 Welcome must be delivered manually: {}",
                member_did,
                e
            );
            continue;
        }

        let mut metadata = HashMap::new();
        metadata.insert("type".to_string(), "mls_group_invitation".to_string());
        metadata.insert("group_id".to_string(), result.new_group_id.clone());
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
            .send_message(member_did.clone(), content)
            .await
        {
            Ok(_) => {
                reinvited.push(member_did.clone());
            }
            Err(e) => {
                tracing::warn!(
                    "reinitialize: Failed to send Welcome to {}: {}",
                    member_did,
                    e
                );
            }
        }
    }

    // Persist the new MLS state.
    persist_mls_state(&state).await;

    // Notify all connected WebSocket clients.
    state
        .ws_manager
        .broadcast(crate::websocket::WsMessage::GroupReinitialized {
            old_group_id: id.clone(),
            new_group_id: result.new_group_id.clone(),
        });

    Ok(Json(serde_json::json!({
        "success": true,
        "old_group_id": id,
        "new_group_id": result.new_group_id,
        "reinvited_members": reinvited,
        "total_members": result.members.len(),
    })))
}

#[derive(Debug, Deserialize)]
pub struct ReinitializeGroupRequest {
    /// Optional pre-provided KeyPackages by member DID (hex-encoded TLS-serialized).
    /// If not provided, the server attempts P2P resolution.
    #[serde(default)]
    pub key_packages: Option<HashMap<String, String>>,
}

// ===== Group receipt handlers =====

/// POST /groups/{group_id}/receipts/read
///
/// Send READ receipts for specific message IDs (or all recent messages) in a
/// group. Each receipt is MLS-encrypted and published to the group's GossipSub
/// topic so all members see it.
pub(super) async fn send_group_read_receipts(
    State(state): State<AppState>,
    Path(group_id): Path<String>,
    axum::Json(req): axum::Json<super::types::SendGroupReadReceiptRequest>,
) -> Result<Json<super::types::GroupReceiptsResponse>> {
    let message_ids = req.message_ids.unwrap_or_default();
    if message_ids.is_empty() {
        return Err(Error::BadRequest {
            message: "message_ids must not be empty".to_string(),
        });
    }

    let mut responses = Vec::with_capacity(message_ids.len());

    for message_id in &message_ids {
        let receipt = state
            .receipts
            .send_group_read(&group_id, message_id)
            .await
            .map_err(|e| Error::App {
                message: format!("Failed to create group read receipt: {}", e),
            })?;

        // Best-effort publish to GossipSub. If it fails we still stored the
        // receipt locally, which is fine — receipts are best-effort.
        if let Err(e) = publish_group_receipt(&state, &group_id, receipt.clone()).await {
            tracing::debug!(
                "Failed to publish group READ receipt for {} in {}: {}",
                message_id,
                group_id,
                e
            );
        }

        responses.push(super::types::GroupReceiptResponse {
            message_id: receipt.message_id,
            reader_did: receipt.reader_did,
            status: super::helpers::receipt_status_to_string(receipt.status),
            timestamp: receipt.timestamp,
        });
    }

    Ok(Json(super::types::GroupReceiptsResponse {
        receipts: responses,
    }))
}

/// GET /groups/{group_id}/messages/{message_id}/receipts
///
/// Fetch the per-member receipt breakdown for a specific group message.
pub(super) async fn get_group_message_receipts(
    State(state): State<AppState>,
    Path((group_id, message_id)): Path<(String, String)>,
) -> Result<Json<super::types::GroupReceiptsResponse>> {
    let receipts = state
        .receipts
        .get_group_receipts(&group_id, &message_id)
        .await
        .map_err(|e| Error::App {
            message: format!("Failed to fetch group receipts: {}", e),
        })?;

    let responses: Vec<super::types::GroupReceiptResponse> = receipts
        .into_iter()
        .map(|r| super::types::GroupReceiptResponse {
            message_id: r.message_id,
            reader_did: r.reader_did,
            status: super::helpers::receipt_status_to_string(r.status),
            timestamp: r.timestamp,
        })
        .collect();

    Ok(Json(super::types::GroupReceiptsResponse {
        receipts: responses,
    }))
}
