//! Group invitation handlers: list, accept, and decline pending invitations.

use crate::{state::AppState, Error, Result};
use axum::{
    extract::{Path, State},
    response::Json,
};
use std::collections::HashMap;
use variance_messaging::storage::MessageStorage;
use variance_proto::messaging_proto::MessageContent;

// ===== Response types =====

#[derive(Debug, serde::Serialize)]
pub struct PendingInvitationResponse {
    pub group_id: String,
    pub group_name: String,
    pub inviter_did: String,
    pub inviter_display_name: Option<String>,
    pub timestamp: i64,
    pub member_count: usize,
}

// ===== Handlers =====

/// List all pending group invitations for the local user.
pub(super) async fn list_invitations(
    State(state): State<AppState>,
) -> Result<Json<Vec<PendingInvitationResponse>>> {
    let invitations = state
        .storage
        .fetch_pending_invitations()
        .await
        .map_err(|e| Error::App {
            message: format!("Failed to fetch pending invitations: {}", e),
        })?;

    let responses: Vec<PendingInvitationResponse> = invitations
        .into_iter()
        .map(|inv| PendingInvitationResponse {
            group_id: inv.group_id.clone(),
            group_name: inv.group_name.clone(),
            inviter_did: inv.inviter_did.clone(),
            inviter_display_name: state.username_registry.get_display_name(&inv.inviter_did),
            timestamp: inv.timestamp,
            member_count: inv.members.len(),
        })
        .collect();

    Ok(Json(responses))
}

/// Accept a pending group invitation.
///
/// 1. Processes the MLS Welcome to join the group.
/// 2. Sends an `mls_invite_accepted` DM back to the inviter so they can
///    merge the pending commit and broadcast to the group.
/// 3. Deletes the pending invitation from storage.
pub(super) async fn accept_invitation(
    State(state): State<AppState>,
    Path(group_id): Path<String>,
) -> Result<Json<serde_json::Value>> {
    use super::helpers::{ensure_olm_session, send_dm_to_peer};
    use variance_messaging::mls::MlsGroupHandler;

    let invitation = state
        .storage
        .fetch_pending_invitation(&group_id)
        .await
        .map_err(|e| Error::App {
            message: format!("Failed to fetch invitation: {}", e),
        })?
        .ok_or_else(|| Error::NotFound {
            message: format!("No pending invitation for group {}", group_id),
        })?;

    // Process the MLS Welcome.
    let welcome_msg =
        MlsGroupHandler::deserialize_message(&invitation.mls_welcome).map_err(|e| Error::App {
            message: format!("Failed to deserialize MLS Welcome: {}", e),
        })?;

    let joined_group_id = state
        .mls_groups
        .join_group_from_welcome(welcome_msg)
        .map_err(|e| Error::App {
            message: format!("Failed to join group from MLS Welcome: {}", e),
        })?;

    // Persist MLS state.
    super::groups::persist_mls_state(&state).await;

    // Store group metadata with self as MEMBER + any info from the invitation.
    let group_meta = variance_proto::messaging_proto::Group {
        id: joined_group_id.clone(),
        name: invitation.group_name.clone(),
        admin_did: invitation
            .members
            .iter()
            .find(|m| m.role == i32::from(variance_proto::messaging_proto::GroupRole::Admin))
            .map(|m| m.did.clone())
            .unwrap_or_default(),
        members: {
            let mut members = invitation.members.clone();
            // Ensure we're in the member list.
            if !members.iter().any(|m| m.did == state.local_did) {
                members.push(variance_proto::messaging_proto::GroupMember {
                    did: state.local_did.clone(),
                    role: variance_proto::messaging_proto::GroupRole::Member.into(),
                    joined_at: chrono::Utc::now().timestamp_millis(),
                    nickname: None,
                });
            }
            members
        },
        created_at: invitation.timestamp,
        ..Default::default()
    };
    if let Err(e) = state.storage.store_group_metadata(&group_meta).await {
        tracing::warn!(
            "Failed to persist group metadata on invitation accept: {}",
            e
        );
    }

    // Subscribe to the group's GossipSub topic.
    let topic = format!("/variance/group/{}", joined_group_id);
    if let Err(e) = state.node_handle.subscribe_to_topic(topic).await {
        tracing::warn!("Failed to subscribe to MLS group topic: {}", e);
    }

    // Generate a fresh KeyPackage (the Welcome consumed our advertised one).
    match state.mls_groups.generate_key_package() {
        Ok(kp) => match MlsGroupHandler::serialize_message_bytes(&kp) {
            Ok(kp_bytes) => {
                if let Err(e) = state.node_handle.update_mls_key_package(kp_bytes).await {
                    tracing::warn!("Failed to republish MLS KeyPackage after join: {}", e);
                }
            }
            Err(e) => tracing::warn!("Failed to serialize refreshed KeyPackage: {}", e),
        },
        Err(e) => tracing::warn!("Failed to generate refreshed KeyPackage: {}", e),
    }

    // Send acceptance DM to the inviter.
    let inviter_did = invitation.inviter_did.clone();
    if let Err(e) = ensure_olm_session(&state, &inviter_did).await {
        tracing::warn!(
            "Could not establish Olm session with inviter {}: {}",
            inviter_did,
            e
        );
    } else {
        let mut metadata = HashMap::new();
        metadata.insert("type".to_string(), "mls_invite_accepted".to_string());
        metadata.insert("group_id".to_string(), joined_group_id.clone());

        let content = MessageContent {
            text: String::new(),
            attachments: vec![],
            mentions: vec![],
            reply_to: None,
            metadata,
        };

        match state
            .direct_messaging
            .send_message(inviter_did.clone(), content)
            .await
        {
            Ok(message) => {
                send_dm_to_peer(&state, &inviter_did, &message).await;
                // Delete control message from local history.
                let _ = state
                    .storage
                    .delete_direct_by_id(
                        &message.sender_did,
                        &message.recipient_did,
                        message.timestamp,
                        &message.id,
                    )
                    .await;
            }
            Err(e) => {
                tracing::warn!(
                    "Failed to send invite-accepted DM to {}: {}",
                    inviter_did,
                    e
                );
            }
        }
    }

    // Clean up the pending invitation.
    let _ = state.storage.delete_pending_invitation(&group_id).await;

    Ok(Json(serde_json::json!({
        "success": true,
        "group_id": joined_group_id,
    })))
}

/// Decline a pending group invitation.
///
/// Sends an `mls_invite_declined` DM to the inviter so they can rollback the
/// pending MLS commit, then deletes the invitation from local storage.
pub(super) async fn decline_invitation(
    State(state): State<AppState>,
    Path(group_id): Path<String>,
) -> Result<Json<serde_json::Value>> {
    use super::helpers::{ensure_olm_session, send_dm_to_peer};

    let invitation = state
        .storage
        .fetch_pending_invitation(&group_id)
        .await
        .map_err(|e| Error::App {
            message: format!("Failed to fetch invitation: {}", e),
        })?
        .ok_or_else(|| Error::NotFound {
            message: format!("No pending invitation for group {}", group_id),
        })?;

    // Send decline DM to the inviter.
    let inviter_did = invitation.inviter_did.clone();
    if let Err(e) = ensure_olm_session(&state, &inviter_did).await {
        tracing::warn!(
            "Could not establish Olm session with inviter {}: {}",
            inviter_did,
            e
        );
    } else {
        let mut metadata = HashMap::new();
        metadata.insert("type".to_string(), "mls_invite_declined".to_string());
        metadata.insert("group_id".to_string(), group_id.clone());

        let content = MessageContent {
            text: String::new(),
            attachments: vec![],
            mentions: vec![],
            reply_to: None,
            metadata,
        };

        match state
            .direct_messaging
            .send_message(inviter_did.clone(), content)
            .await
        {
            Ok(message) => {
                send_dm_to_peer(&state, &inviter_did, &message).await;
                // Delete control message from local history.
                let _ = state
                    .storage
                    .delete_direct_by_id(
                        &message.sender_did,
                        &message.recipient_did,
                        message.timestamp,
                        &message.id,
                    )
                    .await;
            }
            Err(e) => {
                tracing::warn!(
                    "Failed to send invite-declined DM to {}: {}",
                    inviter_did,
                    e
                );
            }
        }
    }

    // Clean up the pending invitation.
    let _ = state.storage.delete_pending_invitation(&group_id).await;

    Ok(Json(serde_json::json!({
        "success": true,
        "group_id": group_id,
    })))
}
