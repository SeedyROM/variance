//! Messaging-related event listeners: direct messages, group messages, group sync.

use super::persist_mls_state_async;
use crate::websocket::{WebSocketManager, WsMessage};
use std::sync::Arc;
use tracing::{debug, warn};
use variance_messaging::{
    direct::DirectMessageHandler,
    mls::MlsGroupHandler,
    receipts::ReceiptHandler,
    storage::{LocalMessageStorage, MessageStorage},
};
use variance_p2p::{EventChannels, NodeHandle};

/// Dependencies for all messaging-related event listeners.
pub(super) struct MessagingDeps {
    pub ws_manager: WebSocketManager,
    pub direct_messaging: Arc<DirectMessageHandler>,
    pub mls_groups: Arc<MlsGroupHandler>,
    pub node_handle: NodeHandle,
    pub storage: Arc<LocalMessageStorage>,
    pub local_did: String,
    pub receipts: Arc<ReceiptHandler>,
    pub username_registry: Arc<variance_identity::username::UsernameRegistry>,
}

/// Spawn all messaging-related event listeners (DM, group, group sync, invite timeout).
pub(super) fn spawn_messaging_listeners(deps: MessagingDeps, events: EventChannels) {
    spawn_direct_message_listener(
        deps.ws_manager.clone(),
        deps.direct_messaging,
        deps.mls_groups.clone(),
        deps.node_handle.clone(),
        deps.storage.clone(),
        deps.local_did.clone(),
        deps.receipts.clone(),
        deps.username_registry,
        events.clone(),
    );
    spawn_group_message_listener(
        deps.ws_manager.clone(),
        deps.mls_groups.clone(),
        deps.storage.clone(),
        deps.local_did.clone(),
        deps.node_handle.clone(),
        deps.receipts.clone(),
        events.clone(),
    );
    spawn_group_sync_listener(
        deps.ws_manager.clone(),
        deps.mls_groups.clone(),
        deps.storage.clone(),
        deps.local_did.clone(),
        deps.node_handle,
        events,
    );
    spawn_invite_timeout_sweep(
        deps.ws_manager,
        deps.mls_groups,
        deps.storage,
        deps.local_did,
    );
}

/// Process an incoming `mls_group_invitation` DM: store as pending invitation
/// and notify the frontend. The user must explicitly accept or decline.
async fn handle_group_invitation_dm(
    storage: &LocalMessageStorage,
    ws_manager: &WebSocketManager,
    invitation_hex: &str,
    from: &str,
    username_registry: &variance_identity::username::UsernameRegistry,
) -> std::result::Result<(), String> {
    let invitation_bytes =
        hex::decode(invitation_hex).map_err(|e| format!("Invalid invitation hex: {}", e))?;

    let invitation = <variance_proto::messaging_proto::GroupInvitation as prost::Message>::decode(
        invitation_bytes.as_slice(),
    )
    .map_err(|e| format!("Failed to decode GroupInvitation proto: {}", e))?;

    let group_id = invitation.group_id.clone();
    let group_name = invitation.group_name.clone();

    // Store as pending invitation (invitee side).
    storage
        .store_pending_invitation(&invitation)
        .await
        .map_err(|e| format!("Failed to store pending invitation: {}", e))?;

    // Notify frontend so the Invitations tab updates.
    let inviter_display_name = username_registry.get_display_name(from);
    ws_manager.broadcast(WsMessage::GroupInvitationReceived {
        group_id,
        group_name,
        inviter_did: from.to_string(),
        inviter_display_name,
    });

    Ok(())
}

/// Process an `mls_invite_accepted` DM (sender/admin side):
/// merge the pending commit, broadcast it to the group, update metadata.
#[allow(clippy::too_many_arguments)]
async fn handle_invite_accepted_dm(
    mls_groups: &MlsGroupHandler,
    storage: &LocalMessageStorage,
    local_did: &str,
    node_handle: &NodeHandle,
    ws_manager: &WebSocketManager,
    group_id: &str,
    invitee_did: &str,
    username_registry: &variance_identity::username::UsernameRegistry,
) {
    // Confirm the pending MLS commit (merge it).
    if let Err(e) = mls_groups.confirm_add_member(group_id) {
        warn!(
            "EventRouter: Failed to confirm MLS add for group {}: {}",
            group_id, e
        );
        return;
    }

    persist_mls_state_async(mls_groups, storage, local_did).await;

    // Broadcast the stored commit to existing group members via GossipSub.
    // The commit bytes are stored in the outbound invite.
    if let Ok(Some((invitation, _))) = storage.fetch_outbound_invite(group_id, invitee_did).await {
        if !invitation.mls_commit.is_empty() {
            let topic = format!("/variance/group/{}", group_id);
            let commit_proto = variance_proto::messaging_proto::GroupMessage {
                id: ulid::Ulid::new().to_string(),
                sender_did: local_did.to_string(),
                group_id: group_id.to_string(),
                timestamp: chrono::Utc::now().timestamp_millis(),
                r#type: 0,
                reply_to: None,
                mls_ciphertext: invitation.mls_commit,
            };
            if let Err(e) = node_handle.publish_group_message(topic, commit_proto).await {
                warn!(
                    "EventRouter: Failed to broadcast add-member commit for group {}: {}",
                    group_id, e
                );
            }
        }
    }

    // Update group metadata: add the new member.
    if let Ok(Some(mut group_meta)) = storage.fetch_group_metadata(group_id).await {
        if !group_meta.members.iter().any(|m| m.did == invitee_did) {
            group_meta
                .members
                .push(variance_proto::messaging_proto::GroupMember {
                    did: invitee_did.to_string(),
                    role: variance_proto::messaging_proto::GroupRole::Member.into(),
                    joined_at: chrono::Utc::now().timestamp_millis(),
                    nickname: None,
                });
            if let Err(e) = storage.store_group_metadata(&group_meta).await {
                warn!(
                    "EventRouter: Failed to update group metadata with new member: {}",
                    e
                );
            }
        }
    }

    // Clean up the outbound invite.
    let _ = storage.delete_outbound_invite(group_id, invitee_did).await;

    // Notify frontend.
    let invitee_display_name = username_registry.get_display_name(invitee_did);
    ws_manager.broadcast(WsMessage::GroupInvitationAccepted {
        group_id: group_id.to_string(),
        invitee_did: invitee_did.to_string(),
        invitee_display_name,
    });

    debug!(
        "EventRouter: Invite accepted — {} joined group {}",
        invitee_did, group_id
    );
}

/// Process an `mls_invite_declined` DM (sender/admin side):
/// roll back the pending commit and clean up.
async fn handle_invite_declined_dm(
    mls_groups: &MlsGroupHandler,
    storage: &LocalMessageStorage,
    local_did: &str,
    ws_manager: &WebSocketManager,
    group_id: &str,
    invitee_did: &str,
) {
    // Roll back the pending MLS commit.
    if let Err(e) = mls_groups.cancel_add_member(group_id) {
        warn!(
            "EventRouter: Failed to cancel MLS add for group {}: {}",
            group_id, e
        );
        // Even if cancel fails, still clean up the outbound invite.
    }

    persist_mls_state_async(mls_groups, storage, local_did).await;

    // Clean up the outbound invite.
    let _ = storage.delete_outbound_invite(group_id, invitee_did).await;

    // Notify frontend.
    ws_manager.broadcast(WsMessage::GroupInvitationDeclined {
        group_id: group_id.to_string(),
        invitee_did: invitee_did.to_string(),
    });

    debug!(
        "EventRouter: Invite declined — {} declined group {}",
        invitee_did, group_id
    );
}

#[allow(clippy::too_many_arguments)]
fn spawn_direct_message_listener(
    ws_manager: WebSocketManager,
    direct_messaging: Arc<DirectMessageHandler>,
    mls_groups: Arc<MlsGroupHandler>,
    node_handle: NodeHandle,
    storage: Arc<LocalMessageStorage>,
    local_did: String,
    receipts: Arc<ReceiptHandler>,
    username_registry: Arc<variance_identity::username::UsernameRegistry>,
    events: EventChannels,
) {
    tokio::spawn(async move {
        use variance_p2p::events::DirectMessageEvent;
        let mut rx = events.subscribe_direct_messages();
        debug!("EventRouter: Started direct message event listener");

        while let Ok(event) = rx.recv().await {
            debug!("EventRouter: Received direct message event: {:?}", event);

            match event {
                DirectMessageEvent::MessageReceived { peer: _, message } => {
                    let from = message.sender_did.clone();
                    let message_id = message.id.clone();
                    let timestamp = message.timestamp;
                    let reply_to = message.reply_to.clone();
                    let was_prekey = message.olm_message_type == 0;

                    match direct_messaging.receive_message(message).await {
                        Ok(content) => {
                            let dm_type = content.metadata.get("type").map(|s| s.as_str());

                            match dm_type {
                                // ── Group invitation (new deferred flow) ─────
                                Some("mls_group_invitation") => {
                                    if let Some(invitation_hex) = content.metadata.get("invitation")
                                    {
                                        match handle_group_invitation_dm(
                                            &storage,
                                            &ws_manager,
                                            invitation_hex,
                                            &from,
                                            &username_registry,
                                        )
                                        .await
                                        {
                                            Ok(()) => {
                                                debug!(
                                                    "EventRouter: Stored pending invitation \
                                                     from {} for group {}",
                                                    from,
                                                    content
                                                        .metadata
                                                        .get("group_id")
                                                        .unwrap_or(&String::new()),
                                                );
                                            }
                                            Err(e) => {
                                                warn!(
                                                    "EventRouter: Failed to handle group \
                                                     invitation DM: {}",
                                                    e
                                                );
                                            }
                                        }
                                    }
                                    // Delete control message from local DM history.
                                    let _ = storage
                                        .delete_direct_by_id(
                                            &from,
                                            &local_did,
                                            timestamp,
                                            &message_id,
                                        )
                                        .await;
                                }

                                // ── Invite accepted (sender/admin side) ──────
                                Some("mls_invite_accepted") => {
                                    if let Some(group_id) = content.metadata.get("group_id") {
                                        handle_invite_accepted_dm(
                                            &mls_groups,
                                            &storage,
                                            &local_did,
                                            &node_handle,
                                            &ws_manager,
                                            group_id,
                                            &from,
                                            &username_registry,
                                        )
                                        .await;
                                    }
                                    // Delete control message.
                                    let _ = storage
                                        .delete_direct_by_id(
                                            &from,
                                            &local_did,
                                            timestamp,
                                            &message_id,
                                        )
                                        .await;
                                }

                                // ── Invite declined (sender/admin side) ──────
                                Some("mls_invite_declined") => {
                                    if let Some(group_id) = content.metadata.get("group_id") {
                                        handle_invite_declined_dm(
                                            &mls_groups,
                                            &storage,
                                            &local_did,
                                            &ws_manager,
                                            group_id,
                                            &from,
                                        )
                                        .await;
                                    }
                                    // Delete control message.
                                    let _ = storage
                                        .delete_direct_by_id(
                                            &from,
                                            &local_did,
                                            timestamp,
                                            &message_id,
                                        )
                                        .await;
                                }

                                // ── Normal DM ────────────────────────────────
                                _ => {
                                    let msg = WsMessage::DirectMessageReceived {
                                        from: from.clone(),
                                        message_id: message_id.clone(),
                                        text: content.text,
                                        timestamp,
                                        reply_to,
                                    };
                                    ws_manager.broadcast(msg);

                                    // Acknowledge delivery to the sender (best-effort).
                                    match receipts.send_delivered(message_id.clone()).await {
                                        Ok(receipt) => {
                                            if let Err(e) =
                                                storage.store_pending_receipt(&from, &receipt).await
                                            {
                                                warn!(
                                                    "Failed to store pending delivered receipt \
                                                     for {}: {}",
                                                    message_id, e
                                                );
                                            }
                                            if let Err(e) = node_handle
                                                .send_receipt(from.clone(), receipt)
                                                .await
                                            {
                                                debug!(
                                                    "Failed to send delivered receipt for \
                                                     {} (best-effort): {}",
                                                    message_id, e
                                                );
                                            }
                                        }
                                        Err(e) => warn!(
                                            "Failed to create delivered receipt for {}: {}",
                                            message_id, e
                                        ),
                                    }
                                }
                            }

                            // If this was a PreKey message, it consumed an OTK from the
                            // published pool. Generate a fresh replacement batch, mark it
                            // published, then update what we advertise to peers.
                            if was_prekey {
                                debug!("PreKey message consumed an OTK, replenishing pool");
                                direct_messaging.generate_one_time_keys(10).await;
                                let fresh_otks = direct_messaging
                                    .one_time_keys()
                                    .await
                                    .values()
                                    .map(|k| k.to_bytes().to_vec())
                                    .collect();
                                direct_messaging.mark_one_time_keys_as_published().await;

                                if let Err(e) = node_handle.update_one_time_keys(fresh_otks).await {
                                    warn!("Failed to replenish OTK pool in P2P handler: {}", e);
                                }
                            }
                        }
                        Err(e) => {
                            warn!(
                                "EventRouter: Failed to decrypt direct message {}: {}",
                                message_id, e
                            );
                        }
                    }
                }
                DirectMessageEvent::MessageSent {
                    message_id: _,
                    recipient: _,
                } => {
                    // DirectMessageSent is now broadcast directly from the API layer
                    // with full message content, so we don't handle it here
                }
                DirectMessageEvent::DeliveryNack {
                    peer: _,
                    message_id,
                    error,
                } => {
                    warn!(
                        "EventRouter: Message {} NACK'd ({}), sender should retry",
                        message_id, error
                    );
                    let msg = WsMessage::DirectMessageNack { message_id, error };
                    ws_manager.broadcast(msg);
                }
                DirectMessageEvent::DeliveryFailed {
                    message_id,
                    recipient,
                } => {
                    warn!(
                        "EventRouter: Message {} delivery to {} failed (OutboundFailure), \
                         notifying frontend",
                        message_id, recipient
                    );
                    ws_manager.broadcast(WsMessage::DirectMessageStatusChanged {
                        message_id,
                        status: "pending".to_string(),
                    });
                }
            }
        }

        warn!("EventRouter: Direct message event listener ended");
    });
}

/// 5-minute timeout for outbound invites.
const INVITE_TIMEOUT_MS: i64 = 5 * 60 * 1000;
/// How often to sweep for expired invites (60 seconds).
const INVITE_SWEEP_INTERVAL: std::time::Duration = std::time::Duration::from_secs(60);

/// Background task that periodically cancels expired outbound invites.
///
/// While an MLS invite is pending, the group is blocked from other MLS operations.
/// This sweep ensures stale invites don't block the group indefinitely.
fn spawn_invite_timeout_sweep(
    ws_manager: WebSocketManager,
    mls_groups: Arc<MlsGroupHandler>,
    storage: Arc<LocalMessageStorage>,
    local_did: String,
) {
    tokio::spawn(async move {
        debug!("EventRouter: Started invite timeout sweep (interval={INVITE_SWEEP_INTERVAL:?})");

        loop {
            tokio::time::sleep(INVITE_SWEEP_INTERVAL).await;

            let now_ms = chrono::Utc::now().timestamp_millis();
            let expired = match storage
                .fetch_expired_outbound_invites(INVITE_TIMEOUT_MS, now_ms)
                .await
            {
                Ok(list) => list,
                Err(e) => {
                    warn!(
                        "EventRouter: Failed to fetch expired outbound invites: {}",
                        e
                    );
                    continue;
                }
            };

            for (group_id, invitee_did, _invitation) in &expired {
                // Roll back the pending MLS commit so the group is unblocked.
                if let Err(e) = mls_groups.cancel_add_member(group_id) {
                    warn!(
                        "EventRouter: Failed to cancel expired MLS add for group {}: {}",
                        group_id, e
                    );
                    // Continue cleanup even if cancel fails — the commit may have
                    // already been resolved (accepted/declined race).
                }

                // Delete the outbound invite record.
                let _ = storage.delete_outbound_invite(group_id, invitee_did).await;

                // Notify the frontend.
                ws_manager.broadcast(WsMessage::GroupInvitationExpired {
                    group_id: group_id.clone(),
                    invitee_did: invitee_did.clone(),
                });

                debug!(
                    "EventRouter: Expired invite — cancelled pending add of {} to group {}",
                    invitee_did, group_id
                );
            }

            // Persist MLS state once if any invites were cancelled.
            if !expired.is_empty() {
                persist_mls_state_async(&mls_groups, &storage, &local_did).await;
            }
        }
    });
}

/// Handle a decoded `MessageContent` from a group message (both GroupPayload-wrapped
/// and legacy bare format). Handles role changes, regular messages, reactions, and
/// sends auto-DELIVERED receipts for regular messages from other members.
#[allow(clippy::too_many_arguments)]
async fn handle_group_message_content(
    content: &variance_proto::messaging_proto::MessageContent,
    message: &variance_proto::messaging_proto::GroupMessage,
    group_id: &str,
    from: &str,
    message_id: &str,
    timestamp: i64,
    ws_manager: &WebSocketManager,
    mls_groups: &Arc<MlsGroupHandler>,
    storage: &Arc<LocalMessageStorage>,
    local_did: &str,
    receipts: &Arc<ReceiptHandler>,
    node_handle: &NodeHandle,
) {
    let is_role_change = content.metadata.get("type").map(String::as_str) == Some("role_change");
    let is_admin_abandoned =
        content.metadata.get("type").map(String::as_str) == Some("admin_abandoned");

    // Admin abandoned: the sole admin left without transferring the role.
    // Mark the group as frozen so the frontend can disable inputs.
    if is_admin_abandoned {
        if let Ok(Some(mut group_meta)) = storage.fetch_group_metadata(group_id).await {
            group_meta.frozen = true;
            // Remove the departed admin from the member list.
            if let Some(admin_did) = content.metadata.get("admin_did") {
                group_meta.members.retain(|m| m.did != *admin_did);
            }
            if let Err(e) = storage.store_group_metadata(&group_meta).await {
                warn!(
                    "EventRouter: Failed to mark group {} as frozen: {}",
                    group_id, e
                );
            }
        }

        ws_manager.broadcast(WsMessage::GroupFrozen {
            group_id: group_id.to_string(),
        });
    } else if is_role_change {
        if let (Some(target_did), Some(new_role)) = (
            content.metadata.get("target_did"),
            content.metadata.get("new_role"),
        ) {
            let new_role_i32 = match new_role.as_str() {
                "admin" => variance_proto::messaging_proto::GroupRole::Admin as i32,
                "moderator" => variance_proto::messaging_proto::GroupRole::Moderator as i32,
                _ => variance_proto::messaging_proto::GroupRole::Member as i32,
            };
            if let Err(e) = storage
                .update_member_role(group_id, target_did, new_role_i32)
                .await
            {
                warn!(
                    "EventRouter: Failed to apply role change for {} in {}: {}",
                    target_did, group_id, e
                );
            }

            let role_msg = WsMessage::RoleChanged {
                group_id: group_id.to_string(),
                target_did: target_did.clone(),
                new_role: new_role.clone(),
                changed_by: from.to_string(),
            };
            ws_manager.broadcast(role_msg);
        }
    } else {
        // Regular message or reaction — store for history.
        if let Err(e) = storage.store_group(message).await {
            warn!(
                "EventRouter: Failed to store group message {}: {}",
                message_id, e
            );
        }

        // Cache the plaintext (at-rest encrypted) for history.
        // MLS forward secrecy means we can't re-decrypt later.
        if let Err(e) = mls_groups
            .persist_group_plaintext(storage, message_id, content)
            .await
        {
            warn!(
                "EventRouter: Failed to cache group plaintext {}: {}",
                message_id, e
            );
        }

        let msg = WsMessage::GroupMessageReceived {
            group_id: group_id.to_string(),
            from: from.to_string(),
            message_id: message_id.to_string(),
            timestamp,
        };
        ws_manager.broadcast(msg);

        // Auto-DELIVERED: send a delivery receipt back to the group for
        // messages from other members (mirrors DM auto-delivered pattern).
        if from != local_did {
            match receipts.send_group_delivered(group_id, message_id).await {
                Ok(receipt) => {
                    if let Err(e) = publish_group_receipt_from_event_router(
                        mls_groups,
                        storage,
                        node_handle,
                        local_did,
                        group_id,
                        receipt,
                    )
                    .await
                    {
                        debug!(
                            "EventRouter: Failed to publish auto-DELIVERED for {} in {}: {}",
                            message_id, group_id, e
                        );
                    }
                }
                Err(e) => {
                    debug!(
                        "EventRouter: Failed to create DELIVERED receipt for {} in {}: {}",
                        message_id, group_id, e
                    );
                }
            }
        }
    }
}

/// Encrypt and publish a group receipt from the event router context.
///
/// This mirrors `crate::api::groups::publish_group_receipt` but operates on
/// individual `Arc` references rather than `AppState`, since the event router
/// doesn't have access to the full `AppState`.
async fn publish_group_receipt_from_event_router(
    mls_groups: &Arc<MlsGroupHandler>,
    storage: &Arc<LocalMessageStorage>,
    node_handle: &NodeHandle,
    local_did: &str,
    group_id: &str,
    receipt: variance_proto::messaging_proto::GroupReadReceipt,
) -> std::result::Result<(), String> {
    use variance_proto::messaging_proto::{group_payload, GroupPayload};

    let payload = GroupPayload {
        payload: Some(group_payload::Payload::Receipt(receipt)),
    };
    let plaintext = prost::Message::encode_to_vec(&payload);

    let mls_msg = mls_groups
        .encrypt_message(group_id, &plaintext)
        .map_err(|e| format!("MLS encrypt receipt: {}", e))?;

    let mls_bytes = MlsGroupHandler::serialize_message(&mls_msg)
        .map_err(|e| format!("MLS serialize receipt: {}", e))?;

    let wire_msg = variance_proto::messaging_proto::GroupMessage {
        id: ulid::Ulid::new().to_string(),
        sender_did: local_did.to_string(),
        group_id: group_id.to_string(),
        timestamp: chrono::Utc::now().timestamp_millis(),
        r#type: variance_proto::messaging_proto::MessageType::Unspecified.into(),
        reply_to: None,
        mls_ciphertext: mls_bytes,
    };

    let topic = format!("/variance/group/{}", group_id);
    node_handle
        .publish_group_message(topic, wire_msg)
        .await
        .map_err(|e| format!("GossipSub publish receipt: {}", e))?;

    // Persist MLS state — encryption advanced the ratchet.
    super::persist_mls_state_async(mls_groups, storage, local_did).await;

    Ok(())
}

fn spawn_group_message_listener(
    ws_manager: WebSocketManager,
    mls_groups: Arc<MlsGroupHandler>,
    storage: Arc<LocalMessageStorage>,
    local_did: String,
    node_handle: NodeHandle,
    receipts: Arc<ReceiptHandler>,
    events: EventChannels,
) {
    tokio::spawn(async move {
        use variance_p2p::events::GroupMessageEvent;
        let mut rx = events.subscribe_group_messages();
        debug!("EventRouter: Started group message event listener");

        while let Ok(event) = rx.recv().await {
            debug!("EventRouter: Received group message event: {:?}", event);

            if let GroupMessageEvent::MessageReceived { message } = event {
                let group_id = message.group_id.clone();
                let from = message.sender_did.clone();
                let message_id = message.id.clone();
                let timestamp = message.timestamp;

                if message.mls_ciphertext.is_empty() {
                    warn!(
                        "EventRouter: Group message {} has no mls_ciphertext, dropping",
                        message_id,
                    );
                } else {
                    // Snapshot member list before MLS processing so we can
                    // detect removals after a commit is applied.
                    let members_before: Vec<String> =
                        mls_groups.list_members(&group_id).unwrap_or_default();

                    match variance_messaging::mls::MlsGroupHandler::deserialize_message(
                        &message.mls_ciphertext,
                    ) {
                        Ok(mls_msg) => match mls_groups.process_message(&group_id, mls_msg) {
                            Ok(Some(decrypted)) => {
                                mls_groups.record_processing_success(&group_id);
                                // Decode the plaintext: try GroupPayload envelope
                                // first, fall back to bare MessageContent for
                                // backward compatibility with pre-envelope messages.
                                #[allow(unused_imports)]
                                use prost::Message as _;
                                use variance_proto::messaging_proto::{
                                    group_payload, GroupPayload,
                                };

                                let payload = GroupPayload::decode(decrypted.plaintext.as_slice())
                                    .ok()
                                    .and_then(|p| p.payload);

                                match payload {
                                    Some(group_payload::Payload::Receipt(receipt)) => {
                                        // Inbound group receipt — store and notify.
                                        // Ignore receipts from ourselves.
                                        if receipt.reader_did != local_did {
                                            if let Err(e) = receipts
                                                .receive_group_receipt(receipt.clone())
                                                .await
                                            {
                                                warn!(
                                                    "EventRouter: Failed to store group receipt for {} in {}: {}",
                                                    receipt.message_id, group_id, e
                                                );
                                            } else {
                                                use variance_proto::messaging_proto::ReceiptStatus;
                                                let ws_msg = if receipt.status
                                                    == ReceiptStatus::Read as i32
                                                {
                                                    WsMessage::GroupReceiptRead {
                                                        group_id: group_id.clone(),
                                                        message_id: receipt.message_id.clone(),
                                                        member_did: receipt.reader_did.clone(),
                                                    }
                                                } else {
                                                    WsMessage::GroupReceiptDelivered {
                                                        group_id: group_id.clone(),
                                                        message_id: receipt.message_id.clone(),
                                                        member_did: receipt.reader_did.clone(),
                                                    }
                                                };
                                                ws_manager.broadcast(ws_msg);
                                            }
                                        }
                                    }
                                    Some(group_payload::Payload::Message(ref content)) => {
                                        handle_group_message_content(
                                            content,
                                            &message,
                                            &group_id,
                                            &from,
                                            &message_id,
                                            timestamp,
                                            &ws_manager,
                                            &mls_groups,
                                            &storage,
                                            &local_did,
                                            &receipts,
                                            &node_handle,
                                        )
                                        .await;
                                    }
                                    None => {
                                        // Legacy fallback: bare MessageContent
                                        // (pre-GroupPayload messages).
                                        let content =
                                            variance_proto::messaging_proto::MessageContent::decode(
                                                decrypted.plaintext.as_slice(),
                                            )
                                            .ok();
                                        if let Some(ref content) = content {
                                            handle_group_message_content(
                                                content,
                                                &message,
                                                &group_id,
                                                &from,
                                                &message_id,
                                                timestamp,
                                                &ws_manager,
                                                &mls_groups,
                                                &storage,
                                                &local_did,
                                                &receipts,
                                                &node_handle,
                                            )
                                            .await;
                                        } else {
                                            warn!(
                                                "EventRouter: Failed to decode group plaintext for {} (neither GroupPayload nor MessageContent)",
                                                message_id,
                                            );
                                        }
                                    }
                                }

                                // Decrypt advanced the ratchet — persist the new state.
                                persist_mls_state_async(&mls_groups, &storage, &local_did).await;
                            }
                            Ok(None) => {
                                mls_groups.record_processing_success(&group_id);
                                // Commit or proposal processed — epoch or tree changed.
                                persist_mls_state_async(&mls_groups, &storage, &local_did).await;

                                let mut members_after: Vec<String> =
                                    mls_groups.list_members(&group_id).unwrap_or_default();

                                // If the member list didn't change, a proposal was
                                // stored (not a commit).  Auto-commit it so the
                                // departing member is actually removed from the tree.
                                if members_after == members_before {
                                    match mls_groups.commit_pending_proposals(&group_id) {
                                        Ok(Some(commit_msg)) => {
                                            persist_mls_state_async(
                                                &mls_groups,
                                                &storage,
                                                &local_did,
                                            )
                                            .await;

                                            // Broadcast the commit to other members.
                                            if let Ok(commit_bytes) =
                                                MlsGroupHandler::serialize_message(&commit_msg)
                                            {
                                                let topic = format!("/variance/group/{}", group_id);
                                                let commit_proto =
                                                    variance_proto::messaging_proto::GroupMessage {
                                                        id: ulid::Ulid::new().to_string(),
                                                        sender_did: local_did.to_string(),
                                                        group_id: group_id.clone(),
                                                        timestamp: chrono::Utc::now()
                                                            .timestamp_millis(),
                                                        r#type: 0,
                                                        reply_to: None,
                                                        mls_ciphertext: commit_bytes,
                                                    };
                                                if let Err(e) = node_handle
                                                    .publish_group_message(topic, commit_proto)
                                                    .await
                                                {
                                                    warn!(
                                                        "EventRouter: Failed to publish auto-commit for {}: {}",
                                                        group_id, e,
                                                    );
                                                }
                                            }

                                            // Re-snapshot after committing.
                                            members_after = mls_groups
                                                .list_members(&group_id)
                                                .unwrap_or_default();
                                        }
                                        Ok(None) => {
                                            // No pending proposals — nothing to do.
                                        }
                                        Err(e) => {
                                            warn!(
                                                "EventRouter: Failed to auto-commit pending proposals for {}: {}",
                                                group_id, e,
                                            );
                                        }
                                    }
                                }

                                // Detect members removed since the snapshot.
                                let removed: Vec<String> = members_before
                                    .iter()
                                    .filter(|m| !members_after.contains(m))
                                    .cloned()
                                    .collect();

                                // Check if the local user was among the removed.
                                let still_member = members_after.contains(&local_did);

                                if !still_member {
                                    debug!(
                                        "EventRouter: Local user was removed from group {}, cleaning up",
                                        group_id,
                                    );

                                    // Unsubscribe from GossipSub topic.
                                    let topic = format!("/variance/group/{}", group_id);
                                    if let Err(e) = node_handle.unsubscribe_from_topic(topic).await
                                    {
                                        warn!(
                                            "EventRouter: Failed to unsubscribe from group topic after removal: {}",
                                            e,
                                        );
                                    }

                                    // Remove the MLS group state so is_member() returns false.
                                    mls_groups.remove_group(&group_id);
                                    persist_mls_state_async(&mls_groups, &storage, &local_did)
                                        .await;

                                    // Purge all local state for this group.
                                    if let Err(e) = storage.delete_group_metadata(&group_id).await {
                                        warn!(
                                            "EventRouter: Failed to delete group metadata after removal: {}",
                                            e,
                                        );
                                    }
                                    if let Err(e) = storage.delete_group_messages(&group_id).await {
                                        warn!(
                                            "EventRouter: Failed to delete group messages after removal: {}",
                                            e,
                                        );
                                    }
                                    if let Err(e) = storage
                                        .delete_group_last_read_at(&local_did, &group_id)
                                        .await
                                    {
                                        warn!(
                                            "EventRouter: Failed to delete group last_read_at after removal: {}",
                                            e,
                                        );
                                    }
                                    if let Err(e) = storage
                                        .delete_all_outbound_invites_for_group(&group_id)
                                        .await
                                    {
                                        warn!(
                                            "EventRouter: Failed to delete outbound invites after removal: {}",
                                            e,
                                        );
                                    }

                                    // Notify frontend so it can remove the group.
                                    let msg = WsMessage::MlsGroupRemoved {
                                        group_id: group_id.clone(),
                                        reason: "kicked".to_string(),
                                    };
                                    ws_manager.broadcast(msg);

                                    // Generate a fresh KeyPackage so we can be reinvited.
                                    match mls_groups.generate_key_package() {
                                        Ok(kp) => {
                                            match MlsGroupHandler::serialize_message_bytes(&kp) {
                                                Ok(kp_bytes) => {
                                                    if let Err(e) = node_handle
                                                        .update_mls_key_package(kp_bytes)
                                                        .await
                                                    {
                                                        warn!(
                                                            "EventRouter: Failed to republish MLS KeyPackage after kick: {}",
                                                            e,
                                                        );
                                                    }
                                                    persist_mls_state_async(
                                                        &mls_groups,
                                                        &storage,
                                                        &local_did,
                                                    )
                                                    .await;
                                                }
                                                Err(e) => warn!(
                                                    "EventRouter: Failed to serialize refreshed KeyPackage after kick: {}",
                                                    e,
                                                ),
                                            }
                                        }
                                        Err(e) => warn!(
                                            "EventRouter: Failed to generate refreshed KeyPackage after kick: {}",
                                            e,
                                        ),
                                    }
                                } else {
                                    // We're still a member — notify frontend about
                                    // any other members that were removed so it can
                                    // refresh the member list / sidebar.
                                    for removed_did in &removed {
                                        ws_manager.broadcast(WsMessage::GroupMemberRemoved {
                                            group_id: group_id.clone(),
                                            member_did: removed_did.clone(),
                                        });
                                    }
                                }
                            }
                            Err(e) => {
                                warn!("EventRouter: MLS decrypt failed for {}: {}", message_id, e);
                                if let Some(info) = mls_groups.record_processing_failure(&group_id)
                                {
                                    warn!(
                                        "EventRouter: Group {} flagged as desynced after {} consecutive failures (local epoch {})",
                                        info.group_id, info.failed_count, info.local_epoch
                                    );
                                    ws_manager.broadcast(WsMessage::GroupDesyncDetected {
                                        group_id: info.group_id,
                                        failed_count: info.failed_count,
                                        local_epoch: info.local_epoch,
                                    });
                                }
                            }
                        },
                        Err(e) => {
                            warn!(
                                "EventRouter: Failed to deserialize MLS message {}: {}",
                                message_id, e
                            );
                            // Deserialization failures also count toward desync detection,
                            // since a corrupt wire format is often a symptom of epoch mismatch.
                            if let Some(info) = mls_groups.record_processing_failure(&group_id) {
                                warn!(
                                    "EventRouter: Group {} flagged as desynced after {} consecutive failures (local epoch {})",
                                    info.group_id, info.failed_count, info.local_epoch
                                );
                                ws_manager.broadcast(WsMessage::GroupDesyncDetected {
                                    group_id: info.group_id,
                                    failed_count: info.failed_count,
                                    local_epoch: info.local_epoch,
                                });
                            }
                        }
                    }
                }
            }
        }

        warn!("EventRouter: Group message event listener ended");
    });
}

fn spawn_group_sync_listener(
    ws_manager: WebSocketManager,
    mls_groups: Arc<MlsGroupHandler>,
    storage: Arc<LocalMessageStorage>,
    local_did: String,
    node_handle: NodeHandle,
    events: EventChannels,
) {
    tokio::spawn(async move {
        use variance_p2p::GroupSyncEvent;
        let mut rx = events.subscribe_group_sync();
        debug!("EventRouter: Started group sync event listener");

        while let Ok(event) = rx.recv().await {
            debug!("EventRouter: Received group sync event: {:?}", event);

            match event {
                GroupSyncEvent::SyncRequested {
                    group_id,
                    since_timestamp,
                    limit,
                    request_id,
                    ..
                } => {
                    // Serve the requesting peer from our local storage
                    let effective_limit = if limit == 0 { 100 } else { limit.min(500) };

                    match storage
                        .fetch_group_since(&group_id, since_timestamp, effective_limit as usize)
                        .await
                    {
                        Ok(messages) => {
                            let has_more = messages.len() == effective_limit as usize;
                            debug!(
                                "EventRouter: Serving {} messages for group {} sync",
                                messages.len(),
                                group_id
                            );
                            let response = variance_proto::messaging_proto::GroupSyncResponse {
                                messages,
                                has_more,
                                error_code: None,
                                error_message: None,
                            };
                            if let Err(e) =
                                node_handle.respond_group_sync(request_id, response).await
                            {
                                warn!("EventRouter: Failed to send group sync response: {}", e);
                            }
                        }
                        Err(e) => {
                            warn!(
                                "EventRouter: Failed to fetch group messages for sync: {}",
                                e
                            );
                            let response = variance_proto::messaging_proto::GroupSyncResponse {
                                messages: vec![],
                                has_more: false,
                                error_code: Some("500".to_string()),
                                error_message: Some(format!("Storage error: {}", e)),
                            };
                            let _ = node_handle.respond_group_sync(request_id, response).await;
                        }
                    }
                }
                GroupSyncEvent::SyncReceived {
                    group_id, messages, ..
                } => {
                    // Process received sync messages through MLS, same as live messages
                    debug!(
                        "EventRouter: Processing {} synced messages for group {}",
                        messages.len(),
                        group_id
                    );
                    let mut new_count = 0u32;

                    for message in messages {
                        let message_id = message.id.clone();
                        let from = message.sender_did.clone();
                        let timestamp = message.timestamp;

                        // Skip if we already have this message
                        if storage.has_group_message(&group_id, &message_id).await {
                            continue;
                        }

                        if message.mls_ciphertext.is_empty() {
                            continue;
                        }

                        match variance_messaging::mls::MlsGroupHandler::deserialize_message(
                            &message.mls_ciphertext,
                        ) {
                            Ok(mls_msg) => {
                                match mls_groups.process_message(&group_id, mls_msg) {
                                    Ok(Some(decrypted)) => {
                                        mls_groups.record_processing_success(&group_id);
                                        if let Err(e) = storage.store_group(&message).await {
                                            warn!(
                                                "EventRouter: Failed to store synced group message {}: {}",
                                                message_id, e
                                            );
                                        }

                                        #[allow(unused_imports)]
                                        use prost::Message as _;
                                        if let Ok(content) =
                                            variance_proto::messaging_proto::MessageContent::decode(
                                                decrypted.plaintext.as_slice(),
                                            )
                                        {
                                            if let Err(e) = mls_groups
                                                .persist_group_plaintext(
                                                    &storage,
                                                    &message_id,
                                                    &content,
                                                )
                                                .await
                                            {
                                                warn!(
                                                    "EventRouter: Failed to cache synced plaintext {}: {}",
                                                    message_id, e
                                                );
                                            }
                                        }

                                        new_count += 1;
                                        ws_manager.broadcast(WsMessage::GroupMessageReceived {
                                            group_id: group_id.clone(),
                                            from,
                                            message_id,
                                            timestamp,
                                        });
                                    }
                                    Ok(None) => {
                                        mls_groups.record_processing_success(&group_id);
                                        // Commit/proposal processed
                                    }
                                    Err(e) => {
                                        warn!(
                                            "EventRouter: MLS decrypt failed for synced {}: {}",
                                            message_id, e
                                        );
                                        // Don't flag desync during sync — these are
                                        // historical messages and failures are expected
                                        // for epochs we've already advanced past.
                                        let _ = mls_groups.record_processing_failure(&group_id);
                                    }
                                }
                            }
                            Err(e) => {
                                warn!(
                                    "EventRouter: Failed to deserialize synced MLS message {}: {}",
                                    message_id, e
                                );
                            }
                        }
                    }

                    if new_count > 0 {
                        persist_mls_state_async(&mls_groups, &storage, &local_did).await;
                        debug!(
                            "EventRouter: Synced {} new messages for group {}",
                            new_count, group_id
                        );
                    }
                }
                GroupSyncEvent::SyncFailed { group_id, error } => {
                    warn!("EventRouter: Group sync failed for {}: {}", group_id, error);
                }
            }
        }

        warn!("EventRouter: Group sync event listener ended");
    });
}
