//! Social-related event listeners: typing, receipts, rename, identity/presence.

use crate::websocket::{WebSocketManager, WsMessage};
use std::sync::Arc;
use tracing::{debug, warn};
use variance_identity::cache::MultiLayerCache;
use variance_identity::username::UsernameRegistry;
use variance_media::{CallManager, SignalingHandler};
use variance_messaging::{
    direct::DirectMessageHandler,
    mls::MlsGroupHandler,
    receipts::ReceiptHandler,
    storage::{LocalMessageStorage, MessageStorage},
    typing::TypingHandler,
};
use variance_p2p::{
    EventChannels, IdentityEvent, NodeHandle, ReceiptEvent, RenameEvent, TypingEvent,
};

/// Dependencies for all social-related event listeners.
pub(super) struct SocialDeps {
    pub ws_manager: WebSocketManager,
    pub typing: Arc<TypingHandler>,
    pub receipts: Arc<ReceiptHandler>,
    pub username_registry: Arc<UsernameRegistry>,
    pub storage: Arc<LocalMessageStorage>,
    pub identity_cache: Arc<MultiLayerCache>,
    pub direct_messaging: Arc<DirectMessageHandler>,
    pub mls_groups: Arc<MlsGroupHandler>,
    pub node_handle: NodeHandle,
    pub call_manager: Arc<CallManager>,
    pub signaling: Arc<SignalingHandler>,
}

/// Spawn all social-related event listeners (typing, receipts, rename, identity/presence).
pub(super) fn spawn_social_listeners(deps: SocialDeps, events: EventChannels) {
    spawn_typing_listener(
        deps.ws_manager.clone(),
        deps.typing,
        deps.mls_groups.clone(),
        events.clone(),
    );
    spawn_receipt_listener(deps.ws_manager.clone(), deps.receipts, events.clone());
    spawn_rename_listener(
        deps.ws_manager.clone(),
        deps.username_registry.clone(),
        deps.storage.clone(),
        events.clone(),
    );
    spawn_identity_listener(
        IdentityListenerDeps {
            ws_manager: deps.ws_manager,
            username_registry: deps.username_registry,
            storage: deps.storage,
            identity_cache: deps.identity_cache,
            direct_messaging: deps.direct_messaging,
            mls_groups: deps.mls_groups,
            node_handle: deps.node_handle,
            call_manager: deps.call_manager,
            signaling: deps.signaling,
        },
        events,
    );
}

fn spawn_typing_listener(
    ws_manager: WebSocketManager,
    typing: Arc<TypingHandler>,
    mls_groups: Arc<MlsGroupHandler>,
    events: EventChannels,
) {
    tokio::spawn(async move {
        let mut rx = events.subscribe_typing();
        debug!("EventRouter: Started typing event listener");

        while let Ok(TypingEvent::IndicatorReceived {
            sender_did,
            recipient,
            is_typing,
        }) = rx.recv().await
        {
            // For group typing indicators, verify the sender is still a group
            // member. Kicked members may still attempt to send indicators; drop
            // them silently.
            if let Some(group_id) = recipient.strip_prefix("group:") {
                let is_member = mls_groups
                    .list_members(group_id)
                    .map(|members| members.contains(&sender_did))
                    .unwrap_or(false);
                if !is_member {
                    debug!(
                        "EventRouter: Dropping typing indicator from non-member {} for group {}",
                        sender_did, group_id
                    );
                    continue;
                }
            }

            // Update the local typing state so the polling endpoint also works
            use variance_proto::messaging_proto::{typing_indicator::Recipient, TypingIndicator};
            let indicator = TypingIndicator {
                sender_did: sender_did.clone(),
                recipient: Some(if let Some(group_id) = recipient.strip_prefix("group:") {
                    Recipient::GroupId(group_id.to_string())
                } else {
                    Recipient::RecipientDid(recipient.clone())
                }),
                is_typing,
                timestamp: chrono::Utc::now().timestamp_millis(),
            };
            typing.receive_indicator(indicator);

            // Push to WebSocket clients for immediate UI update
            let msg = if is_typing {
                WsMessage::TypingStarted {
                    from: sender_did,
                    recipient,
                }
            } else {
                WsMessage::TypingStopped {
                    from: sender_did,
                    recipient,
                }
            };
            ws_manager.broadcast(msg);
        }

        warn!("EventRouter: Typing event listener ended");
    });
}

fn spawn_receipt_listener(
    ws_manager: WebSocketManager,
    receipts: Arc<ReceiptHandler>,
    events: EventChannels,
) {
    tokio::spawn(async move {
        let mut rx = events.subscribe_receipts();
        debug!("EventRouter: Started receipt event listener");

        while let Ok(ReceiptEvent::Received { receipt }) = rx.recv().await {
            let message_id = receipt.message_id.clone();
            let status = receipt.status;

            if let Err(e) = receipts.receive_receipt(receipt).await {
                warn!(
                    "EventRouter: Failed to store receipt for {}: {}",
                    message_id, e
                );
                continue;
            }

            use variance_proto::messaging_proto::ReceiptStatus;
            let msg = if status == ReceiptStatus::Read as i32 {
                WsMessage::ReceiptRead { message_id }
            } else {
                WsMessage::ReceiptDelivered { message_id }
            };
            ws_manager.broadcast(msg);
        }

        warn!("EventRouter: Receipt event listener ended");
    });
}

fn spawn_rename_listener(
    ws_manager: WebSocketManager,
    username_registry: Arc<UsernameRegistry>,
    storage: Arc<LocalMessageStorage>,
    events: EventChannels,
) {
    tokio::spawn(async move {
        let mut rx = events.subscribe_rename();
        debug!("EventRouter: Started rename event listener");

        while let Ok(RenameEvent::PeerRenamed {
            did,
            username,
            discriminator,
        }) = rx.recv().await
        {
            username_registry.cache_mapping(username.clone(), discriminator, did.clone());
            if let Err(e) = storage
                .store_peer_name(&did, &username, discriminator)
                .await
            {
                warn!("Failed to persist peer name for {}: {}", did, e);
            }
            let display_name =
                UsernameRegistry::format_username(&username.to_lowercase(), discriminator);
            debug!("EventRouter: Peer {} renamed to {}", did, display_name);
            ws_manager.broadcast(WsMessage::PeerRenamed { did, display_name });
        }

        warn!("EventRouter: Rename event listener ended");
    });
}

/// Dependencies for the identity/presence listener, grouped to avoid too-many-arguments.
struct IdentityListenerDeps {
    ws_manager: WebSocketManager,
    username_registry: Arc<UsernameRegistry>,
    storage: Arc<LocalMessageStorage>,
    identity_cache: Arc<MultiLayerCache>,
    direct_messaging: Arc<DirectMessageHandler>,
    mls_groups: Arc<MlsGroupHandler>,
    node_handle: NodeHandle,
    call_manager: Arc<CallManager>,
    signaling: Arc<SignalingHandler>,
}

fn spawn_identity_listener(deps: IdentityListenerDeps, events: EventChannels) {
    tokio::spawn(async move {
        let mut rx = events.subscribe_identity();
        debug!("EventRouter: Started identity event listener");

        while let Ok(event) = rx.recv().await {
            debug!("EventRouter: Received identity event: {:?}", event);

            match event {
                // When we receive a full identity response, extract and cache
                // the peer's username so it's available for conversation lists.
                IdentityEvent::ResponseReceived { response, .. } => {
                    if let Some(variance_proto::identity_proto::identity_response::Result::Found(
                        ref found,
                    )) = response.result
                    {
                        if let Some(ref doc) = found.did_document {
                            // Prefer the dedicated username/discriminator proto
                            // fields; fall back to parsing display_name "name#0042".
                            let cached = match (&found.username, found.discriminator) {
                                (Some(name), Some(disc)) if !name.is_empty() && disc > 0 => {
                                    Some((name.clone(), disc))
                                }
                                _ => doc.display_name.as_ref().and_then(|dn| {
                                    let (name, disc_str) = dn.rsplit_once('#')?;
                                    let disc = disc_str.parse::<u32>().ok()?;
                                    Some((name.to_string(), disc))
                                }),
                            };

                            if let Some((name, disc)) = cached {
                                debug!(
                                    "EventRouter: Caching username {}#{:04} for {}",
                                    name, disc, doc.id
                                );
                                deps.username_registry.cache_mapping(
                                    name.clone(),
                                    disc,
                                    doc.id.clone(),
                                );
                                if let Err(e) =
                                    deps.storage.store_peer_name(&doc.id, &name, disc).await
                                {
                                    warn!("Failed to persist peer name for {}: {}", doc.id, e);
                                }
                            }
                        }
                    }
                }
                IdentityEvent::PeerOffline { did } => {
                    // Evict stale identity so a reconnecting peer with new
                    // keys (e.g. after reinstall) gets a fresh resolution.
                    deps.identity_cache.remove(&did);

                    // Purge signaling nonces for any call involving this peer.
                    // Without this, nonce sets for abandoned calls (peer dropped
                    // connection without sending hangup) would leak indefinitely.
                    for call in deps.call_manager.list_active_calls() {
                        if deps.call_manager.get_remote_peer(&call.id).as_deref()
                            == Some(did.as_str())
                        {
                            deps.signaling.purge_call_nonces(&call.id);
                        }
                    }

                    let display_name = deps.username_registry.get_display_name(&did);
                    deps.ws_manager.broadcast(WsMessage::PresenceUpdated {
                        did,
                        online: false,
                        display_name,
                    });
                }
                IdentityEvent::DidCached { did } => {
                    handle_peer_connected(&deps, &did).await;
                }
                _ => {}
            }
        }

        warn!("EventRouter: Identity event listener ended");
    });
}

/// Handle a newly-connected peer: broadcast presence, flush pending messages
/// and receipts, then trigger group sync for shared MLS groups.
async fn handle_peer_connected(deps: &IdentityListenerDeps, did: &str) {
    // Broadcast presence update (include cached display_name if available)
    let display_name = deps.username_registry.get_display_name(did);
    let msg = WsMessage::PresenceUpdated {
        did: did.to_string(),
        online: true,
        display_name,
    };
    deps.ws_manager.broadcast(msg);

    // Flush pending messages for this peer
    debug!(
        "Flushing pending messages for newly connected peer: {}",
        did
    );
    match deps.direct_messaging.get_pending_messages(did).await {
        Ok(messages) => {
            debug!("Found {} pending messages for {}", messages.len(), did);
            for message in messages {
                let message_id = message.id.clone();
                match deps
                    .node_handle
                    .send_direct_message(did.to_string(), message)
                    .await
                {
                    Ok(_) => {
                        debug!(
                            "Successfully sent pending message {} to {}",
                            message_id, did
                        );
                        if let Err(e) = deps.direct_messaging.mark_pending_sent(&message_id).await {
                            warn!("Failed to mark message {} as sent: {}", message_id, e);
                        }
                    }
                    Err(e) => {
                        warn!(
                            "Failed to send pending message {} to {}: {}",
                            message_id, did, e
                        );
                        // Keep in queue for next connection attempt
                    }
                }
            }
        }
        Err(e) => {
            warn!("Failed to fetch pending messages for {}: {}", did, e);
        }
    }

    // Flush pending receipts: receipts that couldn't be delivered
    // when the peer was offline are retried on reconnect.
    match deps.storage.drain_pending_receipts(did).await {
        Ok(pending) if !pending.is_empty() => {
            debug!("Flushing {} pending receipt(s) to {}", pending.len(), did);
            for receipt in pending {
                if let Err(e) = deps
                    .node_handle
                    .send_receipt(did.to_string(), receipt)
                    .await
                {
                    debug!("Failed to flush pending receipt to {}: {}", did, e);
                }
            }
        }
        Err(e) => warn!("Failed to drain pending receipts for {}: {}", did, e),
        _ => {}
    }

    // Trigger group sync: for every MLS group we share with
    // this peer, ask them for messages we may have missed.
    let shared_groups: Vec<String> = deps
        .mls_groups
        .group_ids()
        .into_iter()
        .filter(|gid| {
            deps.mls_groups
                .list_members(gid)
                .map(|members| members.contains(&did.to_string()))
                .unwrap_or(false)
        })
        .collect();

    if !shared_groups.is_empty() {
        debug!(
            "Triggering group sync with {} for {} shared group(s)",
            did,
            shared_groups.len()
        );
    }

    for group_id in shared_groups {
        let since = deps
            .storage
            .latest_group_timestamp(&group_id)
            .await
            .unwrap_or(None)
            .unwrap_or(0);

        if let Err(e) = deps
            .node_handle
            .request_group_sync(did.to_string(), group_id.clone(), since, 100)
            .await
        {
            debug!(
                "Failed to request group sync for {} from {}: {}",
                group_id, did, e
            );
        }
    }
}
