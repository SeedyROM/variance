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
}

/// Spawn all messaging-related event listeners (DM, group, group sync).
pub(super) fn spawn_messaging_listeners(deps: MessagingDeps, events: EventChannels) {
    spawn_direct_message_listener(
        deps.ws_manager.clone(),
        deps.direct_messaging,
        deps.mls_groups.clone(),
        deps.node_handle.clone(),
        deps.storage.clone(),
        deps.local_did.clone(),
        deps.receipts,
        events.clone(),
    );
    spawn_group_message_listener(
        deps.ws_manager.clone(),
        deps.mls_groups.clone(),
        deps.storage.clone(),
        deps.local_did.clone(),
        events.clone(),
    );
    spawn_group_sync_listener(
        deps.ws_manager,
        deps.mls_groups,
        deps.storage,
        deps.local_did,
        deps.node_handle,
        events,
    );
}

/// Process an incoming MLS Welcome that was delivered via encrypted DM.
///
/// Deserializes the Welcome, calls `join_group_from_welcome`, persists
/// MLS state, and subscribes to the group's GossipSub topic.
/// Returns the group ID on success.
async fn handle_mls_welcome_dm(
    mls_groups: &MlsGroupHandler,
    storage: &LocalMessageStorage,
    local_did: &str,
    node_handle: &NodeHandle,
    welcome_hex: &str,
    group_id_hint: Option<&String>,
    group_name: Option<&str>,
) -> std::result::Result<String, String> {
    use variance_messaging::mls::MlsGroupHandler;

    let welcome_bytes =
        hex::decode(welcome_hex).map_err(|e| format!("Invalid Welcome hex: {}", e))?;

    let welcome_msg = MlsGroupHandler::deserialize_message(&welcome_bytes)
        .map_err(|e| format!("Failed to deserialize MLS Welcome: {}", e))?;

    let group_id = mls_groups
        .join_group_from_welcome(welcome_msg)
        .map_err(|e| format!("Failed to join group from Welcome: {}", e))?;

    persist_mls_state_async(mls_groups, storage, local_did).await;

    // Persist group name metadata so the invitee sees the human-readable name.
    if let Some(name) = group_name {
        let group_meta = variance_proto::messaging_proto::Group {
            id: group_id.clone(),
            name: name.to_string(),
            ..Default::default()
        };
        if let Err(e) = storage.store_group_metadata(&group_meta).await {
            warn!(
                "EventRouter: Failed to persist group metadata for joined group: {}",
                e
            );
        }
    }

    // Subscribe to the group's GossipSub topic
    let topic = format!("/variance/group/{}", group_id);
    if let Err(e) = node_handle.subscribe_to_topic(topic).await {
        warn!(
            "EventRouter: Failed to subscribe to group topic after Welcome join: {}",
            e
        );
    }

    // Log if the group_id from the metadata hint differs (shouldn't happen)
    if let Some(hint) = group_id_hint {
        if *hint != group_id {
            warn!(
                "EventRouter: Welcome produced group_id {} but metadata hinted {}",
                group_id, hint
            );
        }
    }

    Ok(group_id)
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
                            // Check for MLS Welcome delivered via DM
                            if content.metadata.get("type").map(|s| s.as_str())
                                == Some("mls_welcome")
                            {
                                if let Some(welcome_hex) = content.metadata.get("mls_welcome") {
                                    let group_name = content.metadata.get("group_name").cloned();
                                    match handle_mls_welcome_dm(
                                        &mls_groups,
                                        &storage,
                                        &local_did,
                                        &node_handle,
                                        welcome_hex,
                                        content.metadata.get("group_id"),
                                        group_name.as_deref(),
                                    )
                                    .await
                                    {
                                        Ok(group_id) => {
                                            debug!(
                                                "EventRouter: Auto-joined MLS group {} \
                                                 from Welcome sent by {}",
                                                group_id, from
                                            );
                                            ws_manager.broadcast(WsMessage::MlsGroupJoined {
                                                group_id,
                                                group_name,
                                                inviter: from.clone(),
                                            });

                                            // The Welcome consumed our advertised KeyPackage.
                                            // Generate a fresh one so future invites work.
                                            use variance_messaging::mls::MlsGroupHandler;
                                            match mls_groups.generate_key_package() {
                                                Ok(kp) => {
                                                    match MlsGroupHandler::serialize_message_bytes(
                                                        &kp,
                                                    ) {
                                                        Ok(kp_bytes) => {
                                                            if let Err(e) = node_handle
                                                                .update_mls_key_package(kp_bytes)
                                                                .await
                                                            {
                                                                warn!(
                                                                    "Failed to republish MLS \
                                                                 KeyPackage after join: {}",
                                                                    e
                                                                );
                                                            }
                                                        }
                                                        Err(e) => warn!(
                                                            "Failed to serialize refreshed \
                                                         KeyPackage: {}",
                                                            e
                                                        ),
                                                    }
                                                }
                                                Err(e) => warn!(
                                                    "Failed to generate refreshed \
                                                     KeyPackage: {}",
                                                    e
                                                ),
                                            }
                                        }
                                        Err(e) => {
                                            warn!(
                                                "EventRouter: Failed to auto-join MLS \
                                                 group from Welcome DM: {}",
                                                e
                                            );
                                        }
                                    }
                                }
                                // Don't broadcast mls_welcome as a normal DM.
                                // Remove the stored message from the direct messages tree
                                // so it doesn't appear as an empty bubble in the conversation.
                                if let Err(e) = storage
                                    .delete_direct_by_id(&from, &local_did, timestamp, &message_id)
                                    .await
                                {
                                    warn!(
                                        "Failed to delete MLS Welcome DM {} from storage: {}",
                                        message_id, e
                                    );
                                }
                            } else {
                                let msg = WsMessage::DirectMessageReceived {
                                    from: from.clone(),
                                    message_id: message_id.clone(),
                                    text: content.text,
                                    timestamp,
                                    reply_to,
                                };
                                ws_manager.broadcast(msg);

                                // Acknowledge delivery to the sender (best-effort).
                                // Store as pending in case the sender is currently offline.
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
                                        if let Err(e) =
                                            node_handle.send_receipt(from, receipt).await
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

                            // If this was a PreKey message, it consumed an OTK from the
                            // published pool. Generate a fresh replacement batch, mark it
                            // published, then update what we advertise to peers.
                            // NOTE: one_time_keys() only returns *unpublished* keys — calling
                            // it before generate_one_time_keys would yield an empty set
                            // (all were marked published at startup). Always generate first.
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
                    // The is_connected check in the P2P node prevents most false
                    // positives. This path fires only in the rare race where the peer
                    // disconnects between the check and send_request. Notify the
                    // frontend so the UI can update the status indicator.
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

fn spawn_group_message_listener(
    ws_manager: WebSocketManager,
    mls_groups: Arc<MlsGroupHandler>,
    storage: Arc<LocalMessageStorage>,
    local_did: String,
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
                    match variance_messaging::mls::MlsGroupHandler::deserialize_message(
                        &message.mls_ciphertext,
                    ) {
                        Ok(mls_msg) => match mls_groups.process_message(&group_id, mls_msg) {
                            Ok(Some(decrypted)) => {
                                // Store the raw message so history fetches include it.
                                if let Err(e) = storage.store_group(&message).await {
                                    warn!(
                                        "EventRouter: Failed to store group message {}: {}",
                                        message_id, e
                                    );
                                }

                                // Cache the plaintext (at-rest encrypted) for history.
                                // MLS forward secrecy means we can't re-decrypt later.
                                #[allow(unused_imports)]
                                use prost::Message as _;
                                if let Ok(content) =
                                    variance_proto::messaging_proto::MessageContent::decode(
                                        decrypted.plaintext.as_slice(),
                                    )
                                {
                                    if let Err(e) = mls_groups
                                        .persist_group_plaintext(&storage, &message_id, &content)
                                        .await
                                    {
                                        warn!(
                                            "EventRouter: Failed to cache group plaintext {}: {}",
                                            message_id, e
                                        );
                                    }
                                }

                                let msg = WsMessage::GroupMessageReceived {
                                    group_id: group_id.clone(),
                                    from,
                                    message_id,
                                    timestamp,
                                };
                                ws_manager.broadcast(msg);

                                // Decrypt advanced the ratchet — persist the new state.
                                persist_mls_state_async(&mls_groups, &storage, &local_did).await;
                            }
                            Ok(None) => {
                                // Commit or proposal processed — epoch or tree changed.
                                persist_mls_state_async(&mls_groups, &storage, &local_did).await;
                            }
                            Err(e) => {
                                warn!("EventRouter: MLS decrypt failed for {}: {}", message_id, e);
                            }
                        },
                        Err(e) => {
                            warn!(
                                "EventRouter: Failed to deserialize MLS message {}: {}",
                                message_id, e
                            );
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
                                        // Commit/proposal processed
                                    }
                                    Err(e) => {
                                        warn!(
                                            "EventRouter: MLS decrypt failed for synced {}: {}",
                                            message_id, e
                                        );
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
