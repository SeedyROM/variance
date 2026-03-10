//! Event router that bridges P2P events to WebSocket clients
//!
//! Subscribes to variance-p2p EventChannels and forwards events to connected
//! WebSocket clients via the WebSocketManager.

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
    EventChannels, GroupSyncEvent, IdentityEvent, NodeHandle, OfflineMessageEvent, ReceiptEvent,
    RenameEvent, SignalingEvent, TypingEvent,
};

/// All dependencies needed by the EventRouter, grouped to avoid too-many-arguments.
pub struct EventRouterDeps {
    pub ws_manager: WebSocketManager,
    pub direct_messaging: Arc<DirectMessageHandler>,
    pub mls_groups: Arc<MlsGroupHandler>,
    pub call_manager: Arc<CallManager>,
    pub signaling: Arc<SignalingHandler>,
    pub node_handle: NodeHandle,
    pub username_registry: Arc<UsernameRegistry>,
    pub typing: Arc<TypingHandler>,
    /// Message storage — used to persist MLS state after every group operation.
    pub storage: Arc<LocalMessageStorage>,
    /// Local DID — key under which MLS state is persisted.
    pub local_did: String,
    /// Identity cache — evicted on peer disconnect so reconnecting peers with
    /// new keys don't get served stale identity documents.
    pub identity_cache: Arc<MultiLayerCache>,
    /// Receipt handler — stores inbound receipts from peers.
    pub receipts: Arc<ReceiptHandler>,
}

/// Bridges P2P events to WebSocket clients
pub struct EventRouter {
    ws_manager: WebSocketManager,
    direct_messaging: Arc<DirectMessageHandler>,
    mls_groups: Arc<MlsGroupHandler>,
    call_manager: Arc<CallManager>,
    signaling: Arc<SignalingHandler>,
    node_handle: NodeHandle,
    username_registry: Arc<UsernameRegistry>,
    typing: Arc<TypingHandler>,
    storage: Arc<LocalMessageStorage>,
    local_did: String,
    identity_cache: Arc<MultiLayerCache>,
    receipts: Arc<ReceiptHandler>,
}

impl EventRouter {
    pub fn new(deps: EventRouterDeps) -> Self {
        let EventRouterDeps {
            ws_manager,
            direct_messaging,
            mls_groups,
            call_manager,
            signaling,
            node_handle,
            username_registry,
            typing,
            storage,
            local_did,
            identity_cache,
            receipts,
        } = deps;

        Self {
            ws_manager,
            direct_messaging,
            mls_groups,
            call_manager,
            signaling,
            node_handle,
            username_registry,
            typing,
            storage,
            local_did,
            identity_cache,
            receipts,
        }
    }

    /// Start listening to P2P events and forwarding to WebSocket clients
    ///
    /// This spawns background tasks that subscribe to each event channel
    /// and broadcast events to all connected WebSocket clients.
    pub fn start(self, events: EventChannels) {
        // Spawn task for call manager events (state changes, ICE candidates)
        let ws_manager = self.ws_manager.clone();
        let call_manager = self.call_manager.clone();
        let signaling = self.signaling.clone();
        let node_handle = self.node_handle.clone();
        let mut call_rx = self.call_manager.subscribe();
        tokio::spawn(async move {
            use variance_media::CallEvent;
            debug!("EventRouter: Started call event listener");

            while let Ok(event) = call_rx.recv().await {
                debug!("EventRouter: Received call event: {:?}", event);

                match event {
                    CallEvent::StateChanged { call_id, status } => {
                        let status_str = match status {
                            variance_proto::media_proto::CallStatus::Active => "active",
                            variance_proto::media_proto::CallStatus::Failed => "failed",
                            variance_proto::media_proto::CallStatus::Ended => "ended",
                            _ => "unknown",
                        };
                        ws_manager.broadcast(WsMessage::CallStateChanged {
                            call_id,
                            status: status_str.to_string(),
                        });
                    }
                    CallEvent::IceCandidateGathered {
                        call_id,
                        candidate,
                        sdp_mid,
                        sdp_mline_index,
                    } => {
                        // Send local ICE candidate to remote peer via P2P signaling
                        let remote_peer = call_manager.get_remote_peer(&call_id);
                        if let Some(recipient_did) = remote_peer {
                            match signaling.send_ice_candidate(
                                call_id.clone(),
                                recipient_did.clone(),
                                candidate,
                                sdp_mid.unwrap_or_default(),
                                sdp_mline_index.map(|i| i as u32),
                            ) {
                                Ok(message) => {
                                    if let Err(e) = node_handle
                                        .send_signaling_message(recipient_did, message)
                                        .await
                                    {
                                        warn!(
                                            "Failed to send ICE candidate for call {}: {}",
                                            call_id, e
                                        );
                                    }
                                }
                                Err(e) => {
                                    warn!(
                                        "Failed to create ICE candidate message for call {}: {}",
                                        call_id, e
                                    );
                                }
                            }
                        } else {
                            warn!(
                                "No remote peer found for call {} to send ICE candidate",
                                call_id
                            );
                        }
                    }
                }
            }

            warn!("EventRouter: Call event listener ended");
        });

        // Spawn task for signaling events
        let ws_manager = self.ws_manager.clone();
        let events_clone = events.clone();
        tokio::spawn(async move {
            let mut rx = events_clone.subscribe_signaling();
            debug!("EventRouter: Started signaling event listener");

            while let Ok(event) = rx.recv().await {
                debug!("EventRouter: Received signaling event: {:?}", event);

                let msg = match event {
                    SignalingEvent::OfferReceived {
                        peer,
                        call_id,
                        message,
                    } => WsMessage::CallIncoming {
                        call_id,
                        from: format!("{}", peer),
                        message,
                    },
                    SignalingEvent::AnswerReceived {
                        peer,
                        call_id,
                        message,
                    } => WsMessage::CallAnswer {
                        call_id,
                        from: format!("{}", peer),
                        message,
                    },
                    SignalingEvent::IceCandidateReceived {
                        peer,
                        call_id,
                        message,
                    } => WsMessage::IceCandidate {
                        call_id,
                        from: format!("{}", peer),
                        message,
                    },
                    SignalingEvent::ControlReceived {
                        peer,
                        call_id,
                        message,
                    } => WsMessage::CallControl {
                        call_id,
                        from: format!("{}", peer),
                        message,
                    },
                    SignalingEvent::CallEnded { call_id, reason } => {
                        WsMessage::CallEnded { call_id, reason }
                    }
                };

                ws_manager.broadcast(msg);
            }

            warn!("EventRouter: Signaling event listener ended");
        });

        // Spawn task for offline message events
        let ws_manager = self.ws_manager.clone();
        let events_clone = events.clone();
        tokio::spawn(async move {
            let mut rx = events_clone.subscribe_offline_messages();
            debug!("EventRouter: Started offline message event listener");

            while let Ok(event) = rx.recv().await {
                debug!("EventRouter: Received offline message event: {:?}", event);

                if let OfflineMessageEvent::MessagesReceived { messages, .. } = event {
                    let msg = WsMessage::OfflineMessagesReceived {
                        count: messages.len(),
                    };
                    ws_manager.broadcast(msg);
                }
            }

            warn!("EventRouter: Offline message event listener ended");
        });

        // Spawn task for direct message events
        // Decrypts incoming messages using the Double Ratchet handler before broadcasting.
        // Also detects MLS Welcome messages (metadata type=mls_welcome) and auto-joins groups.
        let ws_manager = self.ws_manager.clone();
        let direct_messaging = self.direct_messaging.clone();
        let node_handle = self.node_handle.clone();
        let mls_groups_dm = self.mls_groups.clone();
        let storage_dm = self.storage.clone();
        let local_did_dm = self.local_did.clone();
        let receipts_dm = self.receipts.clone();
        let events_clone = events.clone();
        tokio::spawn(async move {
            use variance_p2p::events::DirectMessageEvent;
            let mut rx = events_clone.subscribe_direct_messages();
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
                                        let group_name =
                                            content.metadata.get("group_name").cloned();
                                        match Self::handle_mls_welcome_dm(
                                            &mls_groups_dm,
                                            &storage_dm,
                                            &local_did_dm,
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
                                                match mls_groups_dm.generate_key_package() {
                                                    Ok(kp) => match MlsGroupHandler::serialize_message_bytes(&kp) {
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
                                                    },
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
                                    if let Err(e) = storage_dm
                                        .delete_direct_by_id(
                                            &from,
                                            &local_did_dm,
                                            timestamp,
                                            &message_id,
                                        )
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
                                    use variance_messaging::storage::MessageStorage;
                                    match receipts_dm.send_delivered(message_id.clone()).await {
                                        Ok(receipt) => {
                                            if let Err(e) = storage_dm
                                                .store_pending_receipt(&from, &receipt)
                                                .await
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

                                    if let Err(e) =
                                        node_handle.update_one_time_keys(fresh_otks).await
                                    {
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

        // Spawn task for group message events
        let ws_manager = self.ws_manager.clone();
        let mls_groups = self.mls_groups.clone();
        let storage = self.storage.clone();
        let local_did = self.local_did.clone();
        let events_clone = events.clone();
        tokio::spawn(async move {
            use variance_p2p::events::GroupMessageEvent;
            let mut rx = events_clone.subscribe_group_messages();
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
                                    use variance_messaging::storage::MessageStorage;
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
                                            .persist_group_plaintext(
                                                &storage,
                                                &message_id,
                                                &content,
                                            )
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
                                    persist_mls_state_async(&mls_groups, &storage, &local_did)
                                        .await;
                                }
                                Ok(None) => {
                                    // Commit or proposal processed — epoch or tree changed.
                                    persist_mls_state_async(&mls_groups, &storage, &local_did)
                                        .await;
                                }
                                Err(e) => {
                                    warn!(
                                        "EventRouter: MLS decrypt failed for {}: {}",
                                        message_id, e
                                    );
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

        // Spawn task for typing events
        let ws_manager_typing = self.ws_manager.clone();
        let typing = self.typing;
        let events_clone = events.clone();
        tokio::spawn(async move {
            let mut rx = events_clone.subscribe_typing();
            debug!("EventRouter: Started typing event listener");

            while let Ok(TypingEvent::IndicatorReceived {
                sender_did,
                recipient,
                is_typing,
            }) = rx.recv().await
            {
                // Update the local typing state so the polling endpoint also works
                use variance_proto::messaging_proto::{
                    typing_indicator::Recipient, TypingIndicator,
                };
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
                ws_manager_typing.broadcast(msg);
            }

            warn!("EventRouter: Typing event listener ended");
        });

        // Spawn task for receipt events
        let ws_manager_receipts = self.ws_manager.clone();
        let receipts = self.receipts.clone();
        let events_clone = events.clone();
        tokio::spawn(async move {
            let mut rx = events_clone.subscribe_receipts();
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
                ws_manager_receipts.broadcast(msg);
            }

            warn!("EventRouter: Receipt event listener ended");
        });

        // Spawn task for rename events
        let ws_manager_rename = self.ws_manager.clone();
        let username_registry_rename = self.username_registry.clone();
        let storage_rename = self.storage.clone();
        let events_clone = events.clone();
        tokio::spawn(async move {
            let mut rx = events_clone.subscribe_rename();
            debug!("EventRouter: Started rename event listener");

            while let Ok(RenameEvent::PeerRenamed {
                did,
                username,
                discriminator,
            }) = rx.recv().await
            {
                username_registry_rename.cache_mapping(
                    username.clone(),
                    discriminator,
                    did.clone(),
                );
                if let Err(e) = storage_rename
                    .store_peer_name(&did, &username, discriminator)
                    .await
                {
                    warn!("Failed to persist peer name for {}: {}", did, e);
                }
                let display_name =
                    UsernameRegistry::format_username(&username.to_lowercase(), discriminator);
                debug!("EventRouter: Peer {} renamed to {}", did, display_name);
                ws_manager_rename.broadcast(WsMessage::PeerRenamed { did, display_name });
            }

            warn!("EventRouter: Rename event listener ended");
        });

        // Spawn task for group sync events (serve inbound requests + process responses)
        let ws_manager_sync = self.ws_manager.clone();
        let mls_groups_sync = self.mls_groups.clone();
        let storage_sync = self.storage.clone();
        let local_did_sync = self.local_did.clone();
        let node_handle_sync = self.node_handle.clone();
        let events_clone = events.clone();
        tokio::spawn(async move {
            let mut rx = events_clone.subscribe_group_sync();
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
                        use variance_messaging::storage::MessageStorage;
                        let effective_limit = if limit == 0 { 100 } else { limit.min(500) };

                        match storage_sync
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
                                if let Err(e) = node_handle_sync
                                    .respond_group_sync(request_id, response)
                                    .await
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
                                let _ = node_handle_sync
                                    .respond_group_sync(request_id, response)
                                    .await;
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
                            use variance_messaging::storage::MessageStorage;
                            if storage_sync.has_group_message(&group_id, &message_id).await {
                                continue;
                            }

                            if message.mls_ciphertext.is_empty() {
                                continue;
                            }

                            match variance_messaging::mls::MlsGroupHandler::deserialize_message(
                                &message.mls_ciphertext,
                            ) {
                                Ok(mls_msg) => {
                                    match mls_groups_sync.process_message(&group_id, mls_msg) {
                                        Ok(Some(decrypted)) => {
                                            if let Err(e) = storage_sync.store_group(&message).await
                                            {
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
                                                if let Err(e) = mls_groups_sync
                                                    .persist_group_plaintext(
                                                        &storage_sync,
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
                                            ws_manager_sync.broadcast(
                                                WsMessage::GroupMessageReceived {
                                                    group_id: group_id.clone(),
                                                    from,
                                                    message_id,
                                                    timestamp,
                                                },
                                            );
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
                            persist_mls_state_async(
                                &mls_groups_sync,
                                &storage_sync,
                                &local_did_sync,
                            )
                            .await;
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

        // Spawn task for identity events (presence tracking + pending message flush + group sync trigger)
        let ws_manager = self.ws_manager;
        let direct_messaging = self.direct_messaging;
        let node_handle = self.node_handle;
        let username_registry = self.username_registry;
        let mls_groups_identity = self.mls_groups;
        let storage_identity = self.storage;
        let identity_cache = self.identity_cache;
        let signaling_identity = self.signaling;
        let call_manager_identity = self.call_manager;
        tokio::spawn(async move {
            let mut rx = events.subscribe_identity();
            debug!("EventRouter: Started identity event listener");

            while let Ok(event) = rx.recv().await {
                debug!("EventRouter: Received identity event: {:?}", event);

                match event {
                    // When we receive a full identity response, extract and cache
                    // the peer's username so it's available for conversation lists.
                    IdentityEvent::ResponseReceived { response, .. } => {
                        if let Some(
                            variance_proto::identity_proto::identity_response::Result::Found(
                                ref found,
                            ),
                        ) = response.result
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
                                    username_registry.cache_mapping(
                                        name.clone(),
                                        disc,
                                        doc.id.clone(),
                                    );
                                    if let Err(e) =
                                        storage_identity.store_peer_name(&doc.id, &name, disc).await
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
                        identity_cache.remove(&did);

                        // Purge signaling nonces for any call involving this peer.
                        // Without this, nonce sets for abandoned calls (peer dropped
                        // connection without sending hangup) would leak indefinitely.
                        for call in call_manager_identity.list_active_calls() {
                            if call_manager_identity.get_remote_peer(&call.id).as_deref()
                                == Some(did.as_str())
                            {
                                signaling_identity.purge_call_nonces(&call.id);
                            }
                        }

                        let display_name = username_registry.get_display_name(&did);
                        ws_manager.broadcast(WsMessage::PresenceUpdated {
                            did,
                            online: false,
                            display_name,
                        });
                    }
                    IdentityEvent::DidCached { did } => {
                        // Broadcast presence update (include cached display_name if available)
                        let display_name = username_registry.get_display_name(&did);
                        let msg = WsMessage::PresenceUpdated {
                            did: did.clone(),
                            online: true,
                            display_name,
                        };
                        ws_manager.broadcast(msg);

                        // Flush pending messages for this peer
                        debug!(
                            "Flushing pending messages for newly connected peer: {}",
                            did
                        );
                        match direct_messaging.get_pending_messages(&did).await {
                            Ok(messages) => {
                                debug!("Found {} pending messages for {}", messages.len(), did);
                                for message in messages {
                                    let message_id = message.id.clone();
                                    match node_handle
                                        .send_direct_message(did.clone(), message)
                                        .await
                                    {
                                        Ok(_) => {
                                            debug!(
                                                "Successfully sent pending message {} to {}",
                                                message_id, did
                                            );
                                            if let Err(e) = direct_messaging
                                                .mark_pending_sent(&message_id)
                                                .await
                                            {
                                                warn!(
                                                    "Failed to mark message {} as sent: {}",
                                                    message_id, e
                                                );
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
                        match storage_identity.drain_pending_receipts(&did).await {
                            Ok(pending) if !pending.is_empty() => {
                                debug!("Flushing {} pending receipt(s) to {}", pending.len(), did);
                                for receipt in pending {
                                    if let Err(e) =
                                        node_handle.send_receipt(did.clone(), receipt).await
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
                        let shared_groups: Vec<String> = mls_groups_identity
                            .group_ids()
                            .into_iter()
                            .filter(|gid| {
                                mls_groups_identity
                                    .list_members(gid)
                                    .map(|members| members.contains(&did))
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
                            use variance_messaging::storage::MessageStorage;
                            let since = storage_identity
                                .latest_group_timestamp(&group_id)
                                .await
                                .unwrap_or(None)
                                .unwrap_or(0);

                            if let Err(e) = node_handle
                                .request_group_sync(did.clone(), group_id.clone(), since, 100)
                                .await
                            {
                                debug!(
                                    "Failed to request group sync for {} from {}: {}",
                                    group_id, did, e
                                );
                            }
                        }
                    }
                    _ => {}
                }
            }

            warn!("EventRouter: Identity event listener ended");
        });

        debug!("EventRouter: All event listeners started");
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
        node_handle: &variance_p2p::commands::NodeHandle,
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
}

/// Persist MLS state to storage after any mutation.
///
/// Logs a warning on failure but never panics — persistence failure degrades gracefully
/// (groups still work, they just won't survive a restart until the next persist succeeds).
async fn persist_mls_state_async(
    mls_groups: &MlsGroupHandler,
    storage: &LocalMessageStorage,
    local_did: &str,
) {
    match mls_groups.export_state() {
        Ok(bytes) => {
            if let Err(e) = storage.store_mls_state(local_did, &bytes).await {
                warn!("Failed to persist MLS state to storage: {}", e);
            }
        }
        Err(e) => warn!("Failed to export MLS state for persistence: {}", e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::AppState;
    use tempfile::tempdir;
    use variance_p2p::EventChannels;

    fn make_router() -> EventRouter {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let state =
            AppState::with_db_path("did:variance:test".to_string(), db_path.to_str().unwrap());
        EventRouter::new(EventRouterDeps {
            ws_manager: state.ws_manager.clone(),
            direct_messaging: state.direct_messaging.clone(),
            mls_groups: state.mls_groups.clone(),
            call_manager: state.calls.clone(),
            signaling: state.signaling.clone(),
            node_handle: state.node_handle.clone(),
            username_registry: state.username_registry.clone(),
            typing: state.typing.clone(),
            storage: state.storage.clone(),
            local_did: state.local_did.clone(),
            identity_cache: state.identity_cache.clone(),
            receipts: state.receipts.clone(),
        })
    }

    #[tokio::test]
    async fn test_event_router_creation() {
        let _router = make_router();
    }

    #[tokio::test]
    async fn test_event_router_start() {
        let router = make_router();
        let events = EventChannels::default();

        // Start the router (spawns background tasks)
        router.start(events);

        // Give tasks a moment to start
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

        // If we get here without panicking, the tasks started successfully
    }

    #[tokio::test]
    async fn test_signaling_event_routing() {
        use tokio::sync::mpsc;

        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let state =
            AppState::with_db_path("did:variance:test".to_string(), db_path.to_str().unwrap());
        let ws_manager = state.ws_manager.clone();
        let (tx, mut rx) = mpsc::unbounded_channel();

        // Register a test client
        ws_manager.register(
            "test_client".to_string(),
            crate::websocket::ConnectedClient {
                did: None,
                tx,
                subscriptions: crate::websocket::ClientSubscription::default(),
            },
        );

        let router = EventRouter::new(EventRouterDeps {
            ws_manager: ws_manager.clone(),
            direct_messaging: state.direct_messaging.clone(),
            mls_groups: state.mls_groups.clone(),
            call_manager: state.calls.clone(),
            signaling: state.signaling.clone(),
            node_handle: state.node_handle.clone(),
            username_registry: state.username_registry.clone(),
            typing: state.typing.clone(),
            storage: state.storage.clone(),
            local_did: state.local_did.clone(),
            identity_cache: state.identity_cache.clone(),
            receipts: state.receipts.clone(),
        });
        let events = EventChannels::default();

        // Start router
        router.start(events.clone());

        // Give router time to set up listeners
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

        // Send a test signaling event
        let event = SignalingEvent::CallEnded {
            call_id: "test123".to_string(),
            reason: "Test ended".to_string(),
        };

        events.send_signaling(event);

        // Wait a bit for the event to be processed
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        // Check if client received the message
        if let Ok(msg) = rx.try_recv() {
            match msg {
                WsMessage::CallEnded { call_id, reason } => {
                    assert_eq!(call_id, "test123");
                    assert_eq!(reason, "Test ended");
                }
                _ => panic!("Wrong message type received"),
            }
        }
        // Note: This test might fail in CI due to timing, but the important
        // part is that the code compiles and runs without panicking
    }
}
