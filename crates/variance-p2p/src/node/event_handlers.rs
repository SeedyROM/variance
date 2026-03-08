use crate::behaviour::VarianceBehaviourEvent;
use crate::error::*;
use crate::events::{
    DirectMessageEvent, GroupMessageEvent, GroupSyncEvent, IdentityEvent, OfflineMessageEvent,
    RenameEvent, SignalingEvent, TypingEvent,
};
use crate::rate_limiter::protocol as rl;

use super::Node;

use libp2p::dcutr::Event as DcutrEvent;
use libp2p::swarm::SwarmEvent;
use libp2p::{gossipsub, identify, kad, mdns, ping, relay, request_response, PeerId};
use prost::Message;
use tracing::{debug, info, warn};
use variance_proto::identity_proto::{IdentityRequest, IdentityResponse, UsernameChanged};
use variance_proto::media_proto::SignalingMessage;
use variance_proto::messaging_proto::{
    DirectMessage, DirectMessageAck, GroupMessage, GroupSyncRequest, GroupSyncResponse,
    OfflineMessageRequest, OfflineMessageResponse, TypingIndicator,
};

impl Node {
    /// Top-level swarm event dispatcher. Delegates to focused per-protocol handlers.
    pub(crate) async fn handle_event(&mut self, event: SwarmEvent<VarianceBehaviourEvent>) {
        match event {
            SwarmEvent::NewListenAddr { address, .. } => {
                info!("Listening on {}", address);
            }
            SwarmEvent::Behaviour(behaviour_event) => {
                self.handle_behaviour_event(behaviour_event).await;
            }
            SwarmEvent::ConnectionEstablished {
                peer_id, endpoint, ..
            } => {
                self.handle_connection_established(peer_id, endpoint).await;
            }
            SwarmEvent::ConnectionClosed { peer_id, cause, .. } => {
                self.handle_connection_closed(peer_id, cause).await;
            }
            SwarmEvent::IncomingConnection { send_back_addr, .. } => {
                debug!("Incoming connection from {}", send_back_addr);
            }
            SwarmEvent::OutgoingConnectionError { peer_id, error, .. } => {
                self.handle_outgoing_connection_error(peer_id, error);
            }
            SwarmEvent::IncomingConnectionError {
                send_back_addr,
                error,
                ..
            } => {
                debug!(
                    "Incoming connection from {} failed: {}",
                    send_back_addr, error
                );
            }
            _ => {}
        }
    }

    async fn handle_behaviour_event(&mut self, event: VarianceBehaviourEvent) {
        match event {
            VarianceBehaviourEvent::RelayClient(e) => self.handle_relay_client_event(e),
            VarianceBehaviourEvent::Dcutr(e) => self.handle_dcutr_event(e),
            VarianceBehaviourEvent::Kad(e) => self.handle_kad_event(e),
            VarianceBehaviourEvent::Gossipsub(e) => self.handle_gossipsub_event(e),
            VarianceBehaviourEvent::Mdns(e) => self.handle_mdns_event(e),
            VarianceBehaviourEvent::Identify(e) => self.handle_identify_event(e),
            VarianceBehaviourEvent::Ping(e) => self.handle_ping_event(e),
            VarianceBehaviourEvent::Identity(e) => self.handle_identity_protocol_event(e).await,
            VarianceBehaviourEvent::OfflineMessages(e) => {
                self.handle_offline_protocol_event(e).await;
            }
            VarianceBehaviourEvent::Signaling(e) => {
                self.handle_signaling_protocol_event(e).await;
            }
            VarianceBehaviourEvent::DirectMessages(e) => {
                self.handle_direct_message_event(e).await;
            }
            VarianceBehaviourEvent::TypingIndicators(e) => {
                self.handle_typing_indicator_event(e).await;
            }
            VarianceBehaviourEvent::Rename(e) => {
                self.handle_rename_event(e).await;
            }
            VarianceBehaviourEvent::GroupSync(e) => {
                self.handle_group_sync_event(e).await;
            }
        }
    }

    // ── Relay Client ──────────────────────────────────────────────────

    fn handle_relay_client_event(&mut self, event: relay::client::Event) {
        match event {
            relay::client::Event::ReservationReqAccepted { relay_peer_id, .. } => {
                info!(
                    "Relay reservation accepted: reachable via {}",
                    relay_peer_id
                );
            }
            relay::client::Event::OutboundCircuitEstablished { relay_peer_id, .. } => {
                debug!("Outbound circuit established via relay {}", relay_peer_id);
            }
            _ => {}
        }
    }

    // ── DCUTR (hole punching) ─────────────────────────────────────────

    fn handle_dcutr_event(&mut self, event: DcutrEvent) {
        // dcutr::Event is a struct with remote_peer_id + result (not an enum)
        match event.result {
            Ok(_) => {
                info!(
                    "Direct connection established with {} (hole punch succeeded)",
                    event.remote_peer_id
                );
            }
            Err(e) => {
                debug!(
                    "Hole punch failed with {}, staying on relay: {}",
                    event.remote_peer_id, e
                );
            }
        }
    }

    // ── Kademlia ──────────────────────────────────────────────────────

    fn handle_kad_event(&mut self, event: kad::Event) {
        match event {
            kad::Event::RoutingUpdated {
                peer, addresses, ..
            } => {
                debug!("Routing updated for {}: {:?}", peer, addresses);
            }
            kad::Event::InboundRequest { request } => {
                debug!("Inbound DHT request: {:?}", request);
            }
            kad::Event::OutboundQueryProgressed { id, result, .. } => match result {
                kad::QueryResult::GetProviders(Ok(kad::GetProvidersOk::FoundProviders {
                    providers,
                    ..
                })) => {
                    // Relay discovery: dial any new relay providers we found.
                    if self.pending_relay_query == Some(id) {
                        for peer_id in &providers {
                            if self.relay_peer_ids.contains(peer_id) {
                                continue; // already connected to this relay
                            }
                            info!("Discovered relay via DHT: {}", peer_id);
                            self.relay_peer_ids.insert(*peer_id);
                            // Dial by PeerId; addresses come from the peer store
                            // populated by the DHT routing table during get_providers.
                            if let Err(e) = self
                                .swarm
                                .dial(libp2p::swarm::dial_opts::DialOpts::peer_id(*peer_id).build())
                            {
                                debug!("Failed to dial DHT-discovered relay {}: {}", peer_id, e);
                            }
                        }
                    }

                    // Username provider queries (application-layer oneshot).
                    if let Some((peers, _)) = self.pending_provider_queries.get_mut(&id) {
                        peers.extend(providers);
                    }
                }
                kad::QueryResult::GetProviders(Ok(
                    kad::GetProvidersOk::FinishedWithNoAdditionalRecord { .. },
                )) => {
                    // Clear relay query on completion.
                    if self.pending_relay_query == Some(id) {
                        debug!("Relay DHT discovery query finished");
                        self.pending_relay_query = None;
                    }

                    if let Some((peers, tx)) = self.pending_provider_queries.remove(&id) {
                        let _ = tx.send(Ok(peers));
                    }
                }
                kad::QueryResult::GetProviders(Err(e)) => {
                    if self.pending_relay_query == Some(id) {
                        warn!("Relay DHT discovery failed: {:?}", e);
                        self.pending_relay_query = None;
                    }

                    if let Some((_, tx)) = self.pending_provider_queries.remove(&id) {
                        let _ = tx.send(Err(Error::Kad {
                            message: format!("get_providers failed: {:?}", e),
                        }));
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }

    // ── GossipSub ─────────────────────────────────────────────────────

    fn handle_gossipsub_event(&mut self, event: gossipsub::Event) {
        if let gossipsub::Event::Message {
            propagation_source,
            message_id,
            message,
        } = event
        {
            // GossipSub uses the propagation_source (immediate forwarder), not
            // necessarily the original author. This is still useful as a
            // per-peer ingress limit: a misbehaving forwarder gets throttled.
            if !self
                .rate_limiter
                .check(&propagation_source, rl::DIRECT_MESSAGES)
                .is_allowed()
            {
                warn!(
                    "Rate-limited GossipSub message {} from {}",
                    message_id, propagation_source
                );
                return;
            }

            debug!(
                "Got message {} from {} on topic {:?}",
                message_id, propagation_source, message.topic
            );

            match GroupMessage::decode(message.data.as_slice()) {
                Ok(group_msg) => {
                    // Map sender_did → propagation_source so typing indicators can reach
                    // this peer before an explicit identity resolution completes.
                    // In a direct-mesh topology the propagation_source IS the sender.
                    if !group_msg.sender_did.is_empty() {
                        let did = group_msg.sender_did.clone();
                        let peer = propagation_source;
                        let did_to_peer = self.did_to_peer.clone();
                        let peer_store = self.peer_store.clone();
                        tokio::spawn(async move {
                            did_to_peer.write().await.insert(did.clone(), peer);
                            peer_store.insert(&did, &peer);
                        });
                    }

                    self.events
                        .send_group_message(GroupMessageEvent::MessageReceived {
                            message: group_msg,
                        });
                }
                Err(e) => {
                    warn!("Failed to decode GossipSub message as GroupMessage: {}", e);
                }
            }
        }
    }

    // ── mDNS ──────────────────────────────────────────────────────────

    fn handle_mdns_event(&mut self, event: mdns::Event) {
        match event {
            mdns::Event::Discovered(peers) => {
                for (peer_id, multiaddr) in peers {
                    info!("Discovered peer via mDNS: {} at {}", peer_id, multiaddr);

                    // Track address for periodic reconnection
                    let known = self.known_peers.entry(peer_id).or_default();
                    if !known.contains(&multiaddr) {
                        known.push(multiaddr.clone());
                    }

                    self.swarm
                        .behaviour_mut()
                        .kad
                        .add_address(&peer_id, multiaddr.clone());

                    if let Err(e) = self.swarm.dial(
                        libp2p::swarm::dial_opts::DialOpts::peer_id(peer_id)
                            .addresses(vec![multiaddr.clone()])
                            .build(),
                    ) {
                        debug!("Failed to dial mDNS peer {}: {}", peer_id, e);
                    }
                }
            }
            mdns::Event::Expired(peers) => {
                for (peer_id, multiaddr) in peers {
                    debug!("mDNS peer expired: {} at {}", peer_id, multiaddr);
                }
            }
        }
    }

    // ── Identify ──────────────────────────────────────────────────────

    fn handle_identify_event(&mut self, event: identify::Event) {
        if let identify::Event::Received { peer_id, info, .. } = event {
            debug!("Identified peer {}: {:?}", peer_id, info);
            for addr in &info.listen_addrs {
                self.swarm
                    .behaviour_mut()
                    .kad
                    .add_address(&peer_id, addr.clone());
            }
            // Track addresses for periodic reconnection
            let known = self.known_peers.entry(peer_id).or_default();
            for addr in info.listen_addrs {
                if !known.contains(&addr) {
                    known.push(addr);
                }
            }
        }
    }

    // ── Ping ──────────────────────────────────────────────────────────

    fn handle_ping_event(&self, event: ping::Event) {
        match event {
            ping::Event {
                peer,
                result: Ok(rtt),
                ..
            } => {
                debug!("Ping to {} succeeded: {:?}", peer, rtt);
            }
            ping::Event {
                peer,
                result: Err(e),
                ..
            } => {
                warn!("Ping to {} failed: {}", peer, e);
            }
        }
    }

    // ── Identity Protocol (custom request-response) ───────────────────

    async fn handle_identity_protocol_event(
        &mut self,
        event: request_response::Event<IdentityRequest, IdentityResponse>,
    ) {
        use request_response::{Event, Message};

        match event {
            Event::Message { peer, message, .. } => match message {
                Message::Request {
                    request_id,
                    request,
                    channel,
                } => {
                    if !self.rate_limiter.check(&peer, rl::IDENTITY).is_allowed() {
                        warn!(
                            "Rate-limited identity request {:?} from {}",
                            request_id, peer
                        );
                        let nack = IdentityResponse {
                            result: Some(
                                variance_proto::identity_proto::identity_response::Result::Error(
                                    variance_proto::identity_proto::IdentityError {
                                        error: "rate_limited".into(),
                                        details: "Too many requests, retry later".into(),
                                    },
                                ),
                            ),
                            timestamp: chrono::Utc::now().timestamp(),
                        };
                        let _ = self
                            .swarm
                            .behaviour_mut()
                            .identity
                            .send_response(channel, nack);
                        return;
                    }

                    debug!(
                        "Received identity request {:?} from {}: {:?}",
                        request_id, peer, request
                    );

                    self.events.send_identity(IdentityEvent::RequestReceived {
                        peer,
                        request: request.clone(),
                    });

                    // .await the handler instead of block_on — see module docs
                    let handler = self.identity_handler.clone();
                    match handler.handle_request(request).await {
                        Ok(response) => {
                            debug!("Sending identity response for {:?}", request_id);
                            let _ = self
                                .swarm
                                .behaviour_mut()
                                .identity
                                .send_response(channel, response);
                        }
                        Err(e) => {
                            warn!("Failed to handle identity request: {}", e);
                        }
                    }
                }
                Message::Response {
                    request_id,
                    response,
                } => {
                    self.handle_identity_response(peer, request_id, response)
                        .await;
                }
            },
            Event::OutboundFailure {
                peer,
                request_id,
                error,
                ..
            } => {
                warn!(
                    "Identity request {:?} to {} failed: {}",
                    request_id, peer, error
                );

                if let Some(tx) = self.pending_identity_requests.remove(&request_id) {
                    let _ = tx.send(Err(Error::Protocol {
                        message: format!("Identity request to {} failed: {}", peer, error),
                    }));
                }

                // Decrement broadcast counter on outbound failure
                if let Some(did) = self.pending_resolve_requests.remove(&request_id) {
                    if let Some((remaining, _)) = self.pending_did_broadcasts.get_mut(&did) {
                        *remaining -= 1;
                        if *remaining == 0 {
                            if let Some((_, tx)) = self.pending_did_broadcasts.remove(&did) {
                                let _ = tx.send(Err(Error::Protocol {
                                    message: format!(
                                        "DID not found on any connected peer: {}",
                                        did
                                    ),
                                }));
                            }
                        }
                    }
                }
            }
            Event::InboundFailure {
                peer,
                request_id,
                error,
                ..
            } => {
                warn!(
                    "Identity request {:?} from {} failed: {}",
                    request_id, peer, error
                );
            }
            Event::ResponseSent {
                peer, request_id, ..
            } => {
                debug!("Identity response {:?} sent to {}", request_id, peer);
            }
        }
    }

    /// Process an identity response — handles point-to-point, auto-discovery,
    /// and broadcast DID resolution bookkeeping.
    async fn handle_identity_response(
        &mut self,
        peer: PeerId,
        request_id: libp2p::request_response::OutboundRequestId,
        response: IdentityResponse,
    ) {
        debug!(
            "Received identity response {:?} from {}: {:?}",
            request_id, peer, response
        );

        // Complete point-to-point pending request if exists
        if let Some(tx) = self.pending_identity_requests.remove(&request_id) {
            let _ = tx.send(Ok(response.clone()));
        }

        // Handle auto-discovery cleanup
        let is_auto_discovery = self.pending_auto_discovery.remove(&request_id).is_some();
        if is_auto_discovery {
            match &response.result {
                Some(variance_proto::identity_proto::identity_response::Result::Found(_)) => {
                    debug!("Auto-discovery succeeded for peer {}", peer);
                }
                _ => {
                    debug!("Auto-discovery failed for peer {}: no identity found", peer);
                }
            }
        }

        // Handle broadcast DID resolution if this request was part of one
        if let Some(did) = self.pending_resolve_requests.remove(&request_id) {
            match &response.result {
                Some(variance_proto::identity_proto::identity_response::Result::Found(found)) => {
                    if let Some((_, tx)) = self.pending_did_broadcasts.remove(&did) {
                        let _ = tx.send(Ok(found.clone()));
                    }
                    self.pending_resolve_requests.retain(|_, v| v != &did);
                }
                _ => {
                    if let Some((remaining, _)) = self.pending_did_broadcasts.get_mut(&did) {
                        *remaining -= 1;
                        if *remaining == 0 {
                            if let Some((_, tx)) = self.pending_did_broadcasts.remove(&did) {
                                let _ = tx.send(Err(Error::Protocol {
                                    message: format!(
                                        "DID not found on any connected peer: {}",
                                        did
                                    ),
                                }));
                            }
                        }
                    }
                }
            }
        }

        // Send event
        self.events.send_identity(IdentityEvent::ResponseReceived {
            peer,
            response: response.clone(),
        });

        // Cache the DID and update DID->PeerId mapping
        if let Some(variance_proto::identity_proto::identity_response::Result::Found(found)) =
            response.result
        {
            if let Some(did_doc) = found.did_document {
                let handler = self.identity_handler.clone();
                let events = self.events.clone();
                let did_id = did_doc.id.clone();
                let did_to_peer = self.did_to_peer.clone();
                let peer_store = self.peer_store.clone();
                let peer_id = peer;
                tokio::spawn(async move {
                    if let Ok(did) = variance_identity::did::Did::from_proto(did_doc) {
                        did_to_peer.write().await.insert(did_id.clone(), peer_id);
                        peer_store.insert(&did_id, &peer_id);

                        if handler.cache_did(did).await.is_ok() {
                            events.send_identity(IdentityEvent::DidCached { did: did_id });
                        }
                    }
                });
            }
        }
    }

    // ── Offline Message Protocol ──────────────────────────────────────

    async fn handle_offline_protocol_event(
        &mut self,
        event: request_response::Event<OfflineMessageRequest, OfflineMessageResponse>,
    ) {
        use request_response::{Event, Message};

        match event {
            Event::Message { peer, message, .. } => match message {
                Message::Request {
                    request_id,
                    request,
                    channel,
                } => {
                    if !self
                        .rate_limiter
                        .check(&peer, rl::OFFLINE_MESSAGES)
                        .is_allowed()
                    {
                        warn!(
                            "Rate-limited offline message request {:?} from {}",
                            request_id, peer
                        );
                        let nack = OfflineMessageResponse {
                            messages: vec![],
                            has_more: false,
                            next_cursor: None,
                            error_code: Some("rate_limited".into()),
                            error_message: Some("Too many requests, retry later".into()),
                        };
                        let _ = self
                            .swarm
                            .behaviour_mut()
                            .offline_messages
                            .send_response(channel, nack);
                        return;
                    }

                    debug!(
                        "Received offline message request {:?} from {}: {} messages since {:?}",
                        request_id, peer, request.limit, request.since_timestamp
                    );

                    self.events
                        .send_offline_message(OfflineMessageEvent::FetchRequested {
                            peer,
                            mailbox_token: request.mailbox_token.clone(),
                            limit: request.limit,
                        });

                    // .await the handler instead of block_on — see module docs
                    let handler = self.offline_handler.clone();
                    match handler.handle_request(request).await {
                        Ok(response) => {
                            debug!(
                                "Sending {} offline messages in response {:?}{}",
                                response.messages.len(),
                                request_id,
                                if response.error_code.is_some() {
                                    " (error)"
                                } else {
                                    ""
                                }
                            );
                            let _ = self
                                .swarm
                                .behaviour_mut()
                                .offline_messages
                                .send_response(channel, response);
                        }
                        Err(e) => {
                            warn!("Failed to handle offline message request: {}", e);
                        }
                    }
                }
                Message::Response {
                    request_id,
                    response,
                } => {
                    if let Some(ref error_code) = response.error_code {
                        warn!(
                            "Received error in offline message response {:?} from {}: {} - {}",
                            request_id,
                            peer,
                            error_code,
                            response.error_message.as_deref().unwrap_or("no details")
                        );
                    } else {
                        debug!(
                            "Received {} offline messages in response {:?} from {}",
                            response.messages.len(),
                            request_id,
                            peer
                        );
                    }

                    self.events
                        .send_offline_message(OfflineMessageEvent::MessagesReceived {
                            peer,
                            messages: response.messages,
                            has_more: response.has_more,
                        });
                }
            },
            Event::OutboundFailure {
                peer,
                request_id,
                error,
                ..
            } => {
                warn!(
                    "Offline message request {:?} to {} failed: {}",
                    request_id, peer, error
                );
            }
            Event::InboundFailure {
                peer,
                request_id,
                error,
                ..
            } => {
                warn!(
                    "Offline message request {:?} from {} failed: {}",
                    request_id, peer, error
                );
            }
            Event::ResponseSent {
                peer, request_id, ..
            } => {
                debug!("Offline message response {:?} sent to {}", request_id, peer);
            }
        }
    }

    // ── Signaling Protocol (WebRTC) ───────────────────────────────────

    async fn handle_signaling_protocol_event(
        &mut self,
        event: request_response::Event<SignalingMessage, SignalingMessage>,
    ) {
        use request_response::{Event, Message};

        match event {
            Event::Message { peer, message, .. } => match message {
                Message::Request {
                    request_id,
                    request,
                    channel,
                } => {
                    if !self.rate_limiter.check(&peer, rl::SIGNALING).is_allowed() {
                        warn!(
                            "Rate-limited signaling request {:?} from {}",
                            request_id, peer
                        );
                        return;
                    }

                    debug!(
                        "Received WebRTC signaling request {:?} from {} for call {}",
                        request_id, peer, request.call_id
                    );

                    self.emit_signaling_event(&request, peer);

                    // .await the handler instead of block_on — see module docs
                    let handler = self.signaling_handler.clone();
                    let peer_did = format!("did:peer:{}", peer);
                    match handler.handle_message(peer_did, request).await {
                        Ok(response) => {
                            debug!("Sending WebRTC signaling response for {:?}", request_id);
                            let _ = self
                                .swarm
                                .behaviour_mut()
                                .signaling
                                .send_response(channel, response);
                        }
                        Err(e) => {
                            warn!("Failed to handle signaling request: {}", e);
                        }
                    }
                }
                Message::Response {
                    request_id,
                    response,
                } => {
                    debug!(
                        "Received WebRTC signaling response {:?} from {} for call {}",
                        request_id, peer, response.call_id
                    );
                    self.emit_signaling_event(&response, peer);
                }
            },
            Event::OutboundFailure {
                peer,
                request_id,
                error,
                ..
            } => {
                warn!(
                    "WebRTC signaling request {:?} to {} failed: {}",
                    request_id, peer, error
                );
            }
            Event::InboundFailure {
                peer,
                request_id,
                error,
                ..
            } => {
                warn!(
                    "WebRTC signaling request {:?} from {} failed: {}",
                    request_id, peer, error
                );
            }
            Event::ResponseSent {
                peer, request_id, ..
            } => {
                debug!(
                    "WebRTC signaling response {:?} sent to {}",
                    request_id, peer
                );
            }
        }
    }

    /// Map a signaling message to the appropriate event channel variant.
    fn emit_signaling_event(&self, message: &SignalingMessage, peer: PeerId) {
        use variance_proto::media_proto::signaling_message;

        match &message.message {
            Some(signaling_message::Message::Offer(_)) => {
                self.events.send_signaling(SignalingEvent::OfferReceived {
                    peer,
                    call_id: message.call_id.clone(),
                    message: message.clone(),
                });
            }
            Some(signaling_message::Message::Answer(_)) => {
                self.events.send_signaling(SignalingEvent::AnswerReceived {
                    peer,
                    call_id: message.call_id.clone(),
                    message: message.clone(),
                });
            }
            Some(signaling_message::Message::IceCandidate(_)) => {
                self.events
                    .send_signaling(SignalingEvent::IceCandidateReceived {
                        peer,
                        call_id: message.call_id.clone(),
                        message: message.clone(),
                    });
            }
            Some(signaling_message::Message::Control(_)) => {
                self.events.send_signaling(SignalingEvent::ControlReceived {
                    peer,
                    call_id: message.call_id.clone(),
                    message: message.clone(),
                });
            }
            None => {}
        }
    }

    // ── Direct Messages ───────────────────────────────────────────────

    async fn handle_direct_message_event(
        &mut self,
        event: request_response::Event<DirectMessage, DirectMessageAck>,
    ) {
        use request_response::{Event, Message};

        match event {
            Event::Message { peer, message, .. } => match message {
                Message::Request {
                    request, channel, ..
                } => {
                    if !self
                        .rate_limiter
                        .check(&peer, rl::DIRECT_MESSAGES)
                        .is_allowed()
                    {
                        warn!("Rate-limited direct message {} from {}", request.id, peer);
                        let nack = DirectMessageAck {
                            message_id: request.id.clone(),
                            error: Some("rate_limited".into()),
                        };
                        let _ = self
                            .swarm
                            .behaviour_mut()
                            .direct_messages
                            .send_response(channel, nack);
                        return;
                    }

                    debug!("Received direct message {} from {}", request.id, peer);

                    // Learn the sender's DID → PeerId mapping for future sends
                    {
                        let mut did_to_peer = self.did_to_peer.write().await;
                        did_to_peer.insert(request.sender_did.clone(), peer);
                    }
                    self.peer_store.insert(&request.sender_did, &peer);

                    self.events
                        .send_direct_message(DirectMessageEvent::MessageReceived {
                            peer,
                            message: request.clone(),
                        });

                    let ack = DirectMessageAck {
                        message_id: request.id.clone(),
                        error: None,
                    };
                    let _ = self
                        .swarm
                        .behaviour_mut()
                        .direct_messages
                        .send_response(channel, ack);
                }
                Message::Response {
                    response,
                    request_id,
                    ..
                } => {
                    // Clean up tracking for this request
                    self.pending_dm_sends.remove(&request_id);

                    if let Some(ref err) = response.error {
                        warn!(
                            "Direct message {} NACK'd by {}: {}",
                            response.message_id, peer, err
                        );
                        self.events
                            .send_direct_message(DirectMessageEvent::DeliveryNack {
                                peer,
                                message_id: response.message_id.clone(),
                                error: err.clone(),
                            });
                    } else {
                        debug!("Direct message ACK received: {}", response.message_id);
                    }
                }
            },
            Event::OutboundFailure {
                peer,
                request_id,
                error,
                ..
            } => {
                warn!(
                    "Direct message {:?} to {} failed: {}",
                    request_id, peer, error
                );

                // Look up the message that failed and notify the app layer
                if let Some((message_id, recipient)) = self.pending_dm_sends.remove(&request_id) {
                    warn!(
                        "Message {} to {} failed delivery, emitting DeliveryFailed",
                        message_id, recipient
                    );
                    self.events
                        .send_direct_message(DirectMessageEvent::DeliveryFailed {
                            message_id,
                            recipient,
                        });
                }
            }
            Event::InboundFailure {
                peer,
                request_id,
                error,
                ..
            } => {
                warn!(
                    "Inbound direct message {:?} from {} failed: {}",
                    request_id, peer, error
                );
            }
            Event::ResponseSent {
                peer, request_id, ..
            } => {
                debug!("Direct message ACK {:?} sent to {}", request_id, peer);
            }
        }
    }

    // ── Typing Indicators ─────────────────────────────────────────────

    async fn handle_typing_indicator_event(
        &mut self,
        event: request_response::Event<TypingIndicator, TypingIndicator>,
    ) {
        use request_response::{Event, Message};
        use variance_proto::messaging_proto::typing_indicator::Recipient;

        match event {
            Event::Message {
                peer,
                message:
                    Message::Request {
                        request, channel, ..
                    },
                ..
            } => {
                if !self
                    .rate_limiter
                    .check(&peer, rl::TYPING_INDICATORS)
                    .is_allowed()
                {
                    debug!("Rate-limited typing indicator from {}", peer);
                    return;
                }

                let sender_did = request.sender_did.clone();
                let is_typing = request.is_typing;
                let recipient = match &request.recipient {
                    Some(Recipient::RecipientDid(did)) => did.clone(),
                    Some(Recipient::GroupId(id)) => format!("group:{}", id),
                    None => sender_did.clone(),
                };

                debug!(
                    "Received typing indicator from {}: is_typing={}",
                    sender_did, is_typing
                );

                self.events.send_typing(TypingEvent::IndicatorReceived {
                    sender_did,
                    recipient,
                    is_typing,
                });

                let _ = self
                    .swarm
                    .behaviour_mut()
                    .typing_indicators
                    .send_response(channel, TypingIndicator::default());
            }
            Event::OutboundFailure {
                peer,
                request_id,
                error,
                ..
            } => {
                debug!(
                    "Typing indicator {:?} to {} failed (best-effort): {}",
                    request_id, peer, error
                );
            }
            _ => {}
        }
    }

    // ── Rename Notifications ──────────────────────────────────────────

    async fn handle_rename_event(
        &mut self,
        event: request_response::Event<UsernameChanged, UsernameChanged>,
    ) {
        use request_response::{Event, Message};

        match event {
            Event::Message {
                peer,
                message:
                    Message::Request {
                        request, channel, ..
                    },
                ..
            } => {
                if !self.rate_limiter.check(&peer, rl::RENAME).is_allowed() {
                    debug!("Rate-limited rename notification from {}", peer);
                    return;
                }

                debug!(
                    "Received username rename from {}: {}#{:04}",
                    peer, request.username, request.discriminator
                );

                self.events.send_rename(RenameEvent::PeerRenamed {
                    did: request.did,
                    username: request.username,
                    discriminator: request.discriminator,
                });

                let _ = self
                    .swarm
                    .behaviour_mut()
                    .rename
                    .send_response(channel, UsernameChanged::default());
            }
            Event::OutboundFailure {
                peer,
                request_id,
                error,
                ..
            } => {
                debug!(
                    "Rename notification {:?} to {} failed (best-effort): {}",
                    request_id, peer, error
                );
            }
            Event::InboundFailure {
                peer,
                request_id,
                error,
                ..
            } => {
                debug!(
                    "Rename notification {:?} from {} failed (best-effort): {}",
                    request_id, peer, error
                );
            }
            _ => {}
        }
    }

    // ── Group Sync ────────────────────────────────────────────────────

    async fn handle_group_sync_event(
        &mut self,
        event: request_response::Event<GroupSyncRequest, GroupSyncResponse>,
    ) {
        use request_response::{Event, Message};

        match event {
            Event::Message {
                peer,
                message:
                    Message::Request {
                        request,
                        channel,
                        request_id,
                    },
                ..
            } => {
                if !self.rate_limiter.check(&peer, rl::RENAME).is_allowed() {
                    debug!("Rate-limited group sync request from {}", peer);
                    let _ = self.swarm.behaviour_mut().group_sync.send_response(
                        channel,
                        GroupSyncResponse {
                            messages: vec![],
                            has_more: false,
                            error_code: Some("429".to_string()),
                            error_message: Some("Rate limited".to_string()),
                        },
                    );
                    return;
                }

                debug!(
                    "Received group sync request from {} for group {} since {}",
                    peer, request.group_id, request.since_timestamp
                );

                // Store the response channel so the app layer can respond after
                // querying storage via RespondGroupSync command.
                let group_id = request.group_id.clone();
                self.pending_sync_responses
                    .insert(request_id, (channel, group_id));

                // Emit event so the app layer (which owns storage) can serve the response
                self.events.send_group_sync(GroupSyncEvent::SyncRequested {
                    peer,
                    group_id: request.group_id,
                    requester_did: request.requester_did,
                    since_timestamp: request.since_timestamp,
                    limit: request.limit,
                    request_id,
                });
            }
            Event::Message {
                message: Message::Response { response, .. },
                ..
            } => {
                debug!(
                    "Received group sync response: {} messages, has_more={}",
                    response.messages.len(),
                    response.has_more
                );

                if response.error_code.is_some() {
                    self.events.send_group_sync(GroupSyncEvent::SyncFailed {
                        group_id: String::new(),
                        error: response.error_message.unwrap_or_default(),
                    });
                    return;
                }

                // Determine group_id from the first message, if any
                let group_id = response
                    .messages
                    .first()
                    .map(|m| m.group_id.clone())
                    .unwrap_or_default();

                self.events.send_group_sync(GroupSyncEvent::SyncReceived {
                    group_id,
                    messages: response.messages,
                    has_more: response.has_more,
                });
            }
            Event::OutboundFailure {
                peer,
                request_id,
                error,
                ..
            } => {
                warn!(
                    "Group sync request {:?} to {} failed: {}",
                    request_id, peer, error
                );
                self.events.send_group_sync(GroupSyncEvent::SyncFailed {
                    group_id: String::new(),
                    error: format!("Outbound failure: {}", error),
                });
            }
            Event::InboundFailure {
                peer,
                request_id,
                error,
                ..
            } => {
                debug!(
                    "Group sync inbound {:?} from {} failed: {}",
                    request_id, peer, error
                );
                // Clean up any pending response channel
                self.pending_sync_responses.remove(&request_id);
            }
            _ => {}
        }
    }

    // ── Connection lifecycle ──────────────────────────────────────────

    async fn handle_connection_established(
        &mut self,
        peer_id: PeerId,
        endpoint: libp2p::core::ConnectedPoint,
    ) {
        info!(
            "Connection established to {} via {}",
            peer_id,
            endpoint.get_remote_address()
        );

        // Automatically query the peer for their identity to build DID → PeerId mapping.
        debug!("Querying {} for their identity", peer_id);
        let request =
            variance_identity::protocol::create_peer_id_request(&peer_id.to_string(), None);
        let request_id = self
            .swarm
            .behaviour_mut()
            .identity
            .send_request(&peer_id, request);

        self.pending_auto_discovery.insert(request_id, peer_id);

        // If this is a configured relay peer, reserve a circuit slot so we're
        // reachable via it. Circuit listen only works after the connection is up.
        // The full relay address must be used — libp2p rejects bare /p2p/{id}/p2p-circuit
        // addresses without a transport prefix (returns MultiaddrNotSupported).
        if self.relay_peer_ids.contains(&peer_id) {
            let relay_transport_addr = endpoint.get_remote_address().clone();
            let circuit_addr = relay_transport_addr
                .with(libp2p::multiaddr::Protocol::P2p(peer_id))
                .with(libp2p::multiaddr::Protocol::P2pCircuit);
            if let Err(e) = self.swarm.listen_on(circuit_addr) {
                // Expected on the first ConnectionEstablished (fires before relay protocol
                // is negotiated). The second event succeeds — no action needed.
                debug!("Relay circuit listen deferred for {}: {}", peer_id, e);
            }
        }
    }

    async fn handle_connection_closed(
        &mut self,
        peer_id: PeerId,
        cause: Option<libp2p::swarm::ConnectionError>,
    ) {
        debug!("Connection to {} closed: {:?}", peer_id, cause);

        // Free rate-limiter state for this peer
        self.rate_limiter.remove_peer(&peer_id);

        // Remove stale DID→PeerId mappings so future sends are correctly
        // detected as offline and queued rather than silently dropped.
        let mut did_to_peer = self.did_to_peer.write().await;
        let offline_dids: Vec<String> = did_to_peer
            .iter()
            .filter(|(_, &v)| v == peer_id)
            .map(|(k, _)| k.clone())
            .collect();
        for did in &offline_dids {
            did_to_peer.remove(did);
        }
        drop(did_to_peer);

        for did in offline_dids {
            debug!("Peer {} (DID {}) went offline", peer_id, did);
            self.events
                .send_identity(IdentityEvent::PeerOffline { did });
        }
    }

    fn handle_outgoing_connection_error(
        &self,
        peer_id: Option<PeerId>,
        error: libp2p::swarm::DialError,
    ) {
        let error_str = error.to_string();
        let is_transient = error_str.contains("Handshake failed")
            || error_str.contains("Address already in use")
            || error_str.contains("Connection refused")
            || error_str.contains("Transport error");

        if !is_transient {
            if let Some(peer) = peer_id {
                warn!("Outgoing connection to {} failed: {}", peer, error);
            } else {
                warn!("Outgoing connection failed: {}", error);
            }
        } else if let Some(peer) = peer_id {
            debug!(
                "Outgoing connection to {} failed (transient): {}",
                peer, error
            );
        } else {
            debug!("Outgoing connection failed (transient): {}", error);
        }
    }
}
