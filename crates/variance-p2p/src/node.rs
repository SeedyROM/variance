use crate::{
    behaviour::VarianceBehaviour, commands::*, config::Config, error::*, events::*, handlers,
};
use futures::StreamExt;
use libp2p::swarm::SwarmEvent;
use libp2p::{
    gossipsub, identify, kad, mdns, noise, ping, tcp, yamux, PeerId, Swarm, SwarmBuilder,
};
use prost::Message;
use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::time::Duration;
use tokio::select;
use tokio::sync::oneshot;
use tracing::{debug, info, warn};
use variance_proto::identity_proto::{IdentityFound, IdentityResponse};
use variance_proto::messaging_proto::{DirectMessageAck, GroupMessage};

type ProviderQueryResult = (Vec<PeerId>, oneshot::Sender<Result<Vec<PeerId>>>);

/// Tracks a broadcast DID resolve: how many individual requests are still pending
/// and the oneshot channel to fire when the first Found response arrives.
type BroadcastResolve = (usize, oneshot::Sender<Result<IdentityFound>>);

pub struct Node {
    swarm: Swarm<VarianceBehaviour>,
    peer_id: PeerId,
    /// Local DID, set via SetLocalIdentity command after node initialization
    local_did: Arc<tokio::sync::RwLock<Option<String>>>,
    identity_handler: Arc<handlers::identity::IdentityHandler>,
    offline_handler: Arc<handlers::offline::OfflineMessageHandler>,
    signaling_handler: Arc<handlers::signaling::SignalingHandler>,
    events: EventChannels,
    command_rx: tokio::sync::mpsc::Receiver<NodeCommand>,
    /// Pending identity requests awaiting responses
    pending_identity_requests: HashMap<
        libp2p::request_response::OutboundRequestId,
        oneshot::Sender<Result<IdentityResponse>>,
    >,
    /// Pending get_providers queries: query_id → (accumulated peers, response sender)
    pending_provider_queries: HashMap<kad::QueryId, ProviderQueryResult>,
    /// DID to PeerId mapping for routing signaling messages
    did_to_peer: Arc<tokio::sync::RwLock<HashMap<String, PeerId>>>,
    /// Broadcast DID resolution: DID → (remaining request count, response sender).
    /// Fires on the first Found response; if all requests fail, fires an error.
    pending_did_broadcasts: HashMap<String, BroadcastResolve>,
    /// Maps individual identity request_id → DID being resolved (for broadcast lookups).
    pending_resolve_requests: HashMap<libp2p::request_response::OutboundRequestId, String>,
    /// Auto-discovery requests sent when peers connect: request_id → peer_id
    pending_auto_discovery: HashMap<libp2p::request_response::OutboundRequestId, libp2p::PeerId>,
}

impl Node {
    /// Create a new P2P node and return both the node and a handle for sending commands
    pub fn new(config: Config) -> Result<(Self, NodeHandle)> {
        let keypair = libp2p::identity::Keypair::generate_ed25519();
        let peer_id = keypair.public().to_peer_id();

        info!("Creating P2P node with peer ID: {}", peer_id);

        // Create command channel for application layer to send commands to the node
        let (command_tx, command_rx) = tokio::sync::mpsc::channel(100);

        // Build Kademlia DHT
        let mut kad_config = kad::Config::default();
        kad_config.set_replication_factor(
            NonZeroUsize::new(config.kad_config.replication_factor)
                .unwrap_or(NonZeroUsize::new(20).unwrap()),
        );
        kad_config.set_provider_record_ttl(Some(Duration::from_secs(
            config.kad_config.provider_record_ttl,
        )));

        let store = kad::store::MemoryStore::new(peer_id);
        let mut kad = kad::Behaviour::with_config(peer_id, store, kad_config);
        // libp2p 0.55+ defaults to client mode; nodes must explicitly opt into server mode
        // so they accept and serve incoming Kademlia requests (provider record queries).
        // Without this, /ipfs/kad/1.0.0 is rejected and get_providers can never find peers
        // that registered via start_providing.
        kad.set_mode(Some(kad::Mode::Server));

        // Build GossipSub
        let gossipsub_config = gossipsub::ConfigBuilder::default()
            .heartbeat_interval(Duration::from_secs(
                config.gossipsub_config.heartbeat_interval_secs,
            ))
            .history_length(config.gossipsub_config.history_length)
            .history_gossip(config.gossipsub_config.history_gossip)
            .build()
            .map_err(|e| Error::Gossipsub {
                message: e.to_string(),
            })?;

        let gossipsub = gossipsub::Behaviour::new(
            gossipsub::MessageAuthenticity::Signed(keypair.clone()),
            gossipsub_config,
        )
        .map_err(|e| Error::Gossipsub {
            message: e.to_string(),
        })?;

        // Build mDNS
        let mdns = mdns::tokio::Behaviour::new(mdns::Config::default(), peer_id).map_err(|e| {
            Error::Transport {
                source: Box::new(e),
            }
        })?;

        // Build Identify
        let identify = identify::Behaviour::new(identify::Config::new(
            "/variance/1.0.0".to_string(),
            keypair.public(),
        ));

        // Build Ping
        let ping = ping::Behaviour::new(ping::Config::new());

        // Build custom protocols
        let identity = crate::protocols::identity::create_identity_behaviour();
        let offline_messages = crate::protocols::messaging::create_offline_message_behaviour();
        let signaling = crate::protocols::media::create_signaling_behaviour();
        let direct_messages = crate::protocols::messaging::create_direct_message_behaviour();

        // Combine into VarianceBehaviour
        let behaviour = VarianceBehaviour {
            kad,
            gossipsub,
            mdns,
            identify,
            ping,
            identity,
            offline_messages,
            signaling,
            direct_messages,
        };

        // Build Swarm
        let swarm = SwarmBuilder::with_existing_identity(keypair)
            .with_tokio()
            .with_tcp(
                tcp::Config::default().nodelay(true),
                noise::Config::new,
                yamux::Config::default,
            )
            .map_err(|e| Error::Transport {
                source: Box::new(e),
            })?
            .with_quic()
            .with_behaviour(|_| behaviour)
            .map_err(|e| Error::Transport {
                source: Box::new(e),
            })?
            .with_swarm_config(|c| {
                c.with_idle_connection_timeout(Duration::from_secs(60))
                    // Limit concurrent dials to prevent connection storms
                    .with_dial_concurrency_factor(
                        std::num::NonZeroU8::new(8).expect("8 is non-zero"),
                    )
                    // Only keep one connection per peer
                    .with_max_negotiating_inbound_streams(128)
            })
            .build();

        // Initialize protocol handlers
        let identity_handler = Arc::new(handlers::identity::IdentityHandler::new(peer_id));

        let offline_handler = Arc::new(
            handlers::offline::OfflineMessageHandler::with_local_storage(
                peer_id.to_string(),
                &config.storage_path.join("messages"),
            )?,
        );

        let signaling_handler = Arc::new(handlers::signaling::SignalingHandler::new(
            identity_handler.clone(),
        ));

        let events = EventChannels::default();

        let node = Node {
            swarm,
            peer_id,
            local_did: Arc::new(tokio::sync::RwLock::new(None)),
            identity_handler,
            offline_handler,
            signaling_handler,
            events,
            command_rx,
            pending_identity_requests: HashMap::new(),
            pending_provider_queries: std::collections::HashMap::new(),
            did_to_peer: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
            pending_did_broadcasts: HashMap::new(),
            pending_resolve_requests: HashMap::new(),
            pending_auto_discovery: HashMap::new(),
        };

        let handle = NodeHandle::new(command_tx);

        Ok((node, handle))
    }

    pub fn peer_id(&self) -> &PeerId {
        &self.peer_id
    }

    /// Get event channels for subscribing to protocol events
    pub fn events(&self) -> &EventChannels {
        &self.events
    }

    pub async fn listen(&mut self, config: &Config) -> Result<()> {
        for addr in &config.listen_addresses {
            self.swarm
                .listen_on(addr.clone())
                .map_err(|e| Error::Listen {
                    address: addr.to_string(),
                    source: Box::new(e),
                })?;
            info!("Listening on {}", addr);
        }

        // Bootstrap DHT with configured peers
        for bootstrap in &config.bootstrap_peers {
            let peer_id: PeerId = bootstrap
                .peer_id
                .parse()
                .map_err(|_| Error::InvalidPeerId {
                    peer_id: bootstrap.peer_id.clone(),
                })?;

            self.swarm
                .behaviour_mut()
                .kad
                .add_address(&peer_id, bootstrap.multiaddr.clone());

            info!(
                "Added bootstrap peer: {} at {}",
                peer_id, bootstrap.multiaddr
            );
        }

        // Bootstrap the DHT
        if !config.bootstrap_peers.is_empty() {
            self.swarm
                .behaviour_mut()
                .kad
                .bootstrap()
                .map_err(|e| Error::Kad {
                    message: format!("Bootstrap failed: {:?}", e),
                })?;
        }

        Ok(())
    }

    pub async fn run(&mut self, mut shutdown: tokio::sync::mpsc::Receiver<()>) -> Result<()> {
        loop {
            select! {
                event = self.swarm.select_next_some() => {
                    self.handle_event(event).await;
                }
                Some(command) = self.command_rx.recv() => {
                    self.handle_command(command).await;
                }
                _ = shutdown.recv() => {
                    info!("Shutdown signal received");
                    break;
                }
            }
        }

        Ok(())
    }

    /// Handle a command from the application layer
    async fn handle_command(&mut self, command: NodeCommand) {
        match command {
            NodeCommand::SendIdentityRequest {
                peer,
                request,
                response_tx,
            } => {
                debug!("Sending identity request to {}", peer);
                let request_id = self
                    .swarm
                    .behaviour_mut()
                    .identity
                    .send_request(&peer, request);
                self.pending_identity_requests
                    .insert(request_id, response_tx);
            }
            NodeCommand::SendSignalingMessage {
                peer_did,
                message,
                response_tx,
            } => {
                // Look up peer ID from DID
                let did_to_peer = self.did_to_peer.read().await;
                if let Some(peer) = did_to_peer.get(&peer_did) {
                    debug!("Sending signaling message to {} ({})", peer_did, peer);
                    self.swarm
                        .behaviour_mut()
                        .signaling
                        .send_request(peer, message);
                    let _ = response_tx.send(Ok(()));
                } else {
                    warn!(
                        "Cannot send signaling message: unknown peer DID {}",
                        peer_did
                    );
                    let _ = response_tx.send(Err(Error::Protocol {
                        message: format!("Unknown peer DID: {}", peer_did),
                    }));
                }
            }
            NodeCommand::PublishGroupMessage {
                topic,
                message,
                response_tx,
            } => {
                debug!("Publishing group message to topic {}", topic);

                let topic_hash = gossipsub::IdentTopic::new(&topic);
                let encoded = message.encode_to_vec();

                match self
                    .swarm
                    .behaviour_mut()
                    .gossipsub
                    .publish(topic_hash, encoded)
                {
                    Ok(_) => {
                        let _ = response_tx.send(Ok(()));
                    }
                    Err(e) => {
                        warn!("Failed to publish group message: {}", e);
                        let _ = response_tx.send(Err(Error::Gossipsub {
                            message: e.to_string(),
                        }));
                    }
                }
            }
            NodeCommand::SubscribeToTopic { topic, response_tx } => {
                debug!("Subscribing to topic {}", topic);

                let topic_hash = gossipsub::IdentTopic::new(&topic);

                match self.swarm.behaviour_mut().gossipsub.subscribe(&topic_hash) {
                    Ok(_) => {
                        let _ = response_tx.send(Ok(()));
                    }
                    Err(e) => {
                        warn!("Failed to subscribe to topic {}: {}", topic, e);
                        let _ = response_tx.send(Err(Error::Gossipsub {
                            message: e.to_string(),
                        }));
                    }
                }
            }
            NodeCommand::UnsubscribeFromTopic { topic, response_tx } => {
                debug!("Unsubscribing from topic {}", topic);

                let topic_hash = gossipsub::IdentTopic::new(&topic);

                if self
                    .swarm
                    .behaviour_mut()
                    .gossipsub
                    .unsubscribe(&topic_hash)
                {
                    let _ = response_tx.send(Ok(()));
                } else {
                    warn!("Failed to unsubscribe from topic {}: not subscribed", topic);
                    let _ = response_tx.send(Err(Error::Gossipsub {
                        message: format!("Not subscribed to topic: {}", topic),
                    }));
                }
            }
            NodeCommand::ProvideUsername { key, response_tx } => {
                match self.swarm.behaviour_mut().kad.start_providing(key) {
                    Ok(_) => {
                        let _ = response_tx.send(Ok(()));
                    }
                    Err(e) => {
                        let _ = response_tx.send(Err(Error::Kad {
                            message: format!("start_providing failed: {:?}", e),
                        }));
                    }
                }
            }
            NodeCommand::FindUsernameProviders { key, response_tx } => {
                let query_id = self.swarm.behaviour_mut().kad.get_providers(key);
                self.pending_provider_queries
                    .insert(query_id, (Vec::new(), response_tx));
            }
            NodeCommand::SetLocalIdentity {
                did,
                olm_identity_key,
                one_time_keys,
            } => {
                // Store local DID for self-messaging support
                *self.local_did.write().await = Some(did.clone());

                let handler = self.identity_handler.clone();
                tokio::spawn(async move {
                    handler
                        .set_local_identity(did, olm_identity_key, one_time_keys)
                        .await;
                });
            }
            NodeCommand::UpdateOneTimeKeys { one_time_keys } => {
                let handler = self.identity_handler.clone();
                tokio::spawn(async move {
                    handler.update_one_time_keys(one_time_keys).await;
                });
            }
            NodeCommand::SetLocalUsername {
                username,
                discriminator,
            } => {
                let handler = self.identity_handler.clone();
                tokio::spawn(async move {
                    handler.set_local_username(username, discriminator).await;
                });
            }
            NodeCommand::ResolveIdentityByDid { did, response_tx } => {
                // Collect currently connected peers
                let peers: Vec<PeerId> = self.swarm.connected_peers().cloned().collect();

                if peers.is_empty() {
                    let _ = response_tx.send(Err(Error::Protocol {
                        message: format!("Cannot resolve {}: no peers connected", did),
                    }));
                    return;
                }

                let peer_count = peers.len();
                self.pending_did_broadcasts
                    .insert(did.clone(), (peer_count, response_tx));

                let request = variance_identity::protocol::create_did_request(&did, None);
                for peer in peers {
                    debug!("Broadcasting DID resolve request for {} to {}", did, peer);
                    let request_id = self
                        .swarm
                        .behaviour_mut()
                        .identity
                        .send_request(&peer, request.clone());
                    self.pending_resolve_requests
                        .insert(request_id, did.clone());
                }
            }
            NodeCommand::SendDirectMessage {
                peer_did,
                message,
                response_tx,
            } => {
                // Check for self-messaging: if sending to our own DID, emit locally
                let local_did = self.local_did.read().await;
                if let Some(ref our_did) = *local_did {
                    if peer_did == *our_did {
                        debug!("Self-messaging detected: emitting message locally");

                        // Emit the message as received locally without network transmission
                        self.events
                            .send_direct_message(DirectMessageEvent::MessageReceived {
                                peer: self.peer_id,
                                message: message.clone(),
                            });

                        let _ = response_tx.send(Ok(()));
                        return;
                    }
                }
                drop(local_did);

                let did_to_peer = self.did_to_peer.read().await;
                if let Some(peer) = did_to_peer.get(&peer_did) {
                    debug!("Sending direct message to {} ({})", peer_did, peer);
                    self.swarm
                        .behaviour_mut()
                        .direct_messages
                        .send_request(peer, message);
                    let _ = response_tx.send(Ok(()));
                } else {
                    warn!("Cannot send direct message: unknown peer DID {}", peer_did);
                    let _ = response_tx.send(Err(Error::Protocol {
                        message: format!("Unknown peer DID: {}", peer_did),
                    }));
                }
            }
            NodeCommand::GetConnectedDids { response_tx } => {
                let did_to_peer = self.did_to_peer.read().await;
                let dids: Vec<String> = did_to_peer.keys().cloned().collect();
                let _ = response_tx.send(dids);
            }
        }
    }

    async fn handle_event(&mut self, event: SwarmEvent<crate::behaviour::VarianceBehaviourEvent>) {
        match event {
            SwarmEvent::NewListenAddr { address, .. } => {
                info!("Listening on {}", address);
            }
            SwarmEvent::Behaviour(event) => match event {
                crate::behaviour::VarianceBehaviourEvent::Kad(kad_event) => match kad_event {
                    kad::Event::RoutingUpdated {
                        peer, addresses, ..
                    } => {
                        debug!("Routing updated for {}: {:?}", peer, addresses);
                    }
                    kad::Event::InboundRequest { request } => {
                        debug!("Inbound DHT request: {:?}", request);
                    }
                    kad::Event::OutboundQueryProgressed { id, result, .. } => match result {
                        kad::QueryResult::GetProviders(Ok(
                            kad::GetProvidersOk::FoundProviders { providers, .. },
                        )) => {
                            if let Some((peers, _)) = self.pending_provider_queries.get_mut(&id) {
                                peers.extend(providers);
                            }
                        }
                        kad::QueryResult::GetProviders(Ok(
                            kad::GetProvidersOk::FinishedWithNoAdditionalRecord { .. },
                        )) => {
                            if let Some((peers, tx)) = self.pending_provider_queries.remove(&id) {
                                let _ = tx.send(Ok(peers));
                            }
                        }
                        kad::QueryResult::GetProviders(Err(e)) => {
                            if let Some((_, tx)) = self.pending_provider_queries.remove(&id) {
                                let _ = tx.send(Err(Error::Kad {
                                    message: format!("get_providers failed: {:?}", e),
                                }));
                            }
                        }
                        _ => {}
                    },
                    _ => {}
                },
                crate::behaviour::VarianceBehaviourEvent::Gossipsub(gossipsub_event) => {
                    if let gossipsub::Event::Message {
                        propagation_source,
                        message_id,
                        message,
                    } = gossipsub_event
                    {
                        debug!(
                            "Got message {} from {} on topic {:?}",
                            message_id, propagation_source, message.topic
                        );

                        match GroupMessage::decode(message.data.as_slice()) {
                            Ok(group_msg) => {
                                self.events.send_group_message(
                                    GroupMessageEvent::MessageReceived { message: group_msg },
                                );
                            }
                            Err(e) => {
                                warn!("Failed to decode GossipSub message as GroupMessage: {}", e);
                            }
                        }
                    }
                }
                crate::behaviour::VarianceBehaviourEvent::Mdns(mdns_event) => match mdns_event {
                    mdns::Event::Discovered(peers) => {
                        for (peer_id, multiaddr) in peers {
                            info!("Discovered peer via mDNS: {} at {}", peer_id, multiaddr);

                            // Add to Kademlia routing table
                            self.swarm
                                .behaviour_mut()
                                .kad
                                .add_address(&peer_id, multiaddr.clone());

                            // Dial the peer to establish connection (triggers auto-discovery)
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
                },
                crate::behaviour::VarianceBehaviourEvent::Identify(identify_event) => {
                    if let identify::Event::Received { peer_id, info, .. } = identify_event {
                        debug!("Identified peer {}: {:?}", peer_id, info);
                        for addr in info.listen_addrs {
                            self.swarm.behaviour_mut().kad.add_address(&peer_id, addr);
                        }
                    }
                }
                crate::behaviour::VarianceBehaviourEvent::Ping(ping_event) => match ping_event {
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
                },
                crate::behaviour::VarianceBehaviourEvent::Identity(identity_event) => {
                    use libp2p::request_response::{Event, Message};
                    match identity_event {
                        Event::Message { peer, message, .. } => match message {
                            Message::Request {
                                request_id,
                                request,
                                channel,
                            } => {
                                debug!(
                                    "Received identity request {:?} from {}: {:?}",
                                    request_id, peer, request
                                );

                                // Send event
                                self.events.send_identity(IdentityEvent::RequestReceived {
                                    peer,
                                    request: request.clone(),
                                });

                                // Handle request and send response
                                let handler = self.identity_handler.clone();
                                match futures::executor::block_on(handler.handle_request(request)) {
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
                                debug!(
                                    "Received identity response {:?} from {}: {:?}",
                                    request_id, peer, response
                                );

                                // Complete point-to-point pending request if exists
                                if let Some(tx) = self.pending_identity_requests.remove(&request_id)
                                {
                                    let _ = tx.send(Ok(response.clone()));
                                }

                                // Handle auto-discovery cleanup
                                let is_auto_discovery =
                                    self.pending_auto_discovery.remove(&request_id).is_some();
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
                                if let Some(did) = self.pending_resolve_requests.remove(&request_id)
                                {
                                    match &response.result {
                                        Some(variance_proto::identity_proto::identity_response::Result::Found(found)) => {
                                            // Got a match — fire the broadcast sender and remove the broadcast entry
                                            if let Some((_, tx)) = self.pending_did_broadcasts.remove(&did) {
                                                let _ = tx.send(Ok(found.clone()));
                                            }
                                            // Remove any remaining resolve requests for this DID
                                            self.pending_resolve_requests.retain(|_, v| v != &did);
                                        }
                                        _ => {
                                            // Not found or error from this peer — decrement counter
                                            if let Some((remaining, _)) = self.pending_did_broadcasts.get_mut(&did) {
                                                *remaining -= 1;
                                                if *remaining == 0 {
                                                    // All peers responded without a Found — report failure
                                                    if let Some((_, tx)) = self.pending_did_broadcasts.remove(&did) {
                                                        let _ = tx.send(Err(Error::Protocol {
                                                            message: format!("DID not found on any connected peer: {}", did),
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

                                // Process response (e.g., cache the DID and update DID->PeerId mapping)
                                if let Some(variance_proto::identity_proto::identity_response::Result::Found(
                                    found,
                                )) = response.result
                                {
                                    if let Some(did_doc) = found.did_document {
                                        let handler = self.identity_handler.clone();
                                        let events = self.events.clone();
                                        let did_id = did_doc.id.clone();
                                        let did_to_peer = self.did_to_peer.clone();
                                        let peer_id = peer;
                                        tokio::spawn(async move {
                                            if let Ok(did) = variance_identity::did::Did::from_proto(did_doc) {
                                                // Update DID->PeerId mapping
                                                did_to_peer.write().await.insert(did_id.clone(), peer_id);

                                                if handler.cache_did(did).await.is_ok() {
                                                    events.send_identity(IdentityEvent::DidCached { did: did_id });
                                                }
                                            }
                                        });
                                    }
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
                                "Identity request {:?} to {} failed: {}",
                                request_id, peer, error
                            );
                            // Resolve pending point-to-point requests with an error so callers
                            // don't wait forever when the peer is unreachable.
                            if let Some(tx) = self.pending_identity_requests.remove(&request_id) {
                                let _ = tx.send(Err(Error::Protocol {
                                    message: format!(
                                        "Identity request to {} failed: {}",
                                        peer, error
                                    ),
                                }));
                            }
                            // Decrement broadcast counter on outbound failure
                            if let Some(did) = self.pending_resolve_requests.remove(&request_id) {
                                if let Some((remaining, _)) =
                                    self.pending_did_broadcasts.get_mut(&did)
                                {
                                    *remaining -= 1;
                                    if *remaining == 0 {
                                        if let Some((_, tx)) =
                                            self.pending_did_broadcasts.remove(&did)
                                        {
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
                crate::behaviour::VarianceBehaviourEvent::OfflineMessages(offline_event) => {
                    use libp2p::request_response::{Event, Message};
                    match offline_event {
                        Event::Message { peer, message, .. } => match message {
                            Message::Request {
                                request_id,
                                request,
                                channel,
                            } => {
                                debug!(
                                    "Received offline message request {:?} from {}: {} messages since {:?}",
                                    request_id, peer, request.limit, request.since_timestamp
                                );

                                // Send event
                                self.events.send_offline_message(
                                    OfflineMessageEvent::FetchRequested {
                                        peer,
                                        did: request.did.clone(),
                                        limit: request.limit,
                                    },
                                );

                                // Handle request and send response
                                let handler = self.offline_handler.clone();
                                match futures::executor::block_on(handler.handle_request(request)) {
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

                                // Send event with all received messages (empty if error)
                                self.events.send_offline_message(
                                    OfflineMessageEvent::MessagesReceived {
                                        peer,
                                        messages: response.messages,
                                        has_more: response.has_more,
                                    },
                                );
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
                crate::behaviour::VarianceBehaviourEvent::Signaling(signaling_event) => {
                    use libp2p::request_response::{Event, Message};
                    use variance_proto::media_proto::signaling_message;

                    match signaling_event {
                        Event::Message { peer, message, .. } => match message {
                            Message::Request {
                                request_id,
                                request,
                                channel,
                            } => {
                                debug!(
                                    "Received WebRTC signaling request {:?} from {} for call {}",
                                    request_id, peer, request.call_id
                                );

                                // Send appropriate event based on message type
                                match &request.message {
                                    Some(signaling_message::Message::Offer(_)) => {
                                        self.events.send_signaling(SignalingEvent::OfferReceived {
                                            peer,
                                            call_id: request.call_id.clone(),
                                            message: request.clone(),
                                        });
                                    }
                                    Some(signaling_message::Message::Answer(_)) => {
                                        self.events.send_signaling(
                                            SignalingEvent::AnswerReceived {
                                                peer,
                                                call_id: request.call_id.clone(),
                                                message: request.clone(),
                                            },
                                        );
                                    }
                                    Some(signaling_message::Message::IceCandidate(_)) => {
                                        self.events.send_signaling(
                                            SignalingEvent::IceCandidateReceived {
                                                peer,
                                                call_id: request.call_id.clone(),
                                                message: request.clone(),
                                            },
                                        );
                                    }
                                    Some(signaling_message::Message::Control(_)) => {
                                        self.events.send_signaling(
                                            SignalingEvent::ControlReceived {
                                                peer,
                                                call_id: request.call_id.clone(),
                                                message: request.clone(),
                                            },
                                        );
                                    }
                                    None => {}
                                }

                                // Handle request and send response
                                let handler = self.signaling_handler.clone();
                                let peer_did = format!("did:peer:{}", peer); // Simplified - should look up actual DID
                                match futures::executor::block_on(
                                    handler.handle_message(peer_did, request),
                                ) {
                                    Ok(response) => {
                                        debug!(
                                            "Sending WebRTC signaling response for {:?}",
                                            request_id
                                        );
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

                                // Send appropriate event based on message type
                                match &response.message {
                                    Some(signaling_message::Message::Offer(_)) => {
                                        self.events.send_signaling(SignalingEvent::OfferReceived {
                                            peer,
                                            call_id: response.call_id.clone(),
                                            message: response,
                                        });
                                    }
                                    Some(signaling_message::Message::Answer(_)) => {
                                        self.events.send_signaling(
                                            SignalingEvent::AnswerReceived {
                                                peer,
                                                call_id: response.call_id.clone(),
                                                message: response,
                                            },
                                        );
                                    }
                                    Some(signaling_message::Message::IceCandidate(_)) => {
                                        self.events.send_signaling(
                                            SignalingEvent::IceCandidateReceived {
                                                peer,
                                                call_id: response.call_id.clone(),
                                                message: response,
                                            },
                                        );
                                    }
                                    Some(signaling_message::Message::Control(_)) => {
                                        self.events.send_signaling(
                                            SignalingEvent::ControlReceived {
                                                peer,
                                                call_id: response.call_id.clone(),
                                                message: response,
                                            },
                                        );
                                    }
                                    None => {}
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
                crate::behaviour::VarianceBehaviourEvent::DirectMessages(dm_event) => {
                    use libp2p::request_response::{Event, Message};

                    match dm_event {
                        Event::Message { peer, message, .. } => match message {
                            Message::Request {
                                request, channel, ..
                            } => {
                                debug!("Received direct message {} from {}", request.id, peer);

                                // Learn the sender's DID → PeerId mapping for future sends
                                {
                                    let mut did_to_peer = self.did_to_peer.write().await;
                                    did_to_peer.insert(request.sender_did.clone(), peer);
                                }

                                // Emit event for the app layer to decrypt and deliver
                                self.events.send_direct_message(
                                    DirectMessageEvent::MessageReceived {
                                        peer,
                                        message: request.clone(),
                                    },
                                );

                                // Send ACK
                                let ack = DirectMessageAck {
                                    message_id: request.id.clone(),
                                };
                                let _ = self
                                    .swarm
                                    .behaviour_mut()
                                    .direct_messages
                                    .send_response(channel, ack);
                            }
                            Message::Response { response, .. } => {
                                debug!("Direct message ACK received: {}", response.message_id);
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
            },
            SwarmEvent::ConnectionEstablished {
                peer_id, endpoint, ..
            } => {
                info!(
                    "Connection established to {} via {}",
                    peer_id,
                    endpoint.get_remote_address()
                );

                // Automatically query the peer for their identity to build DID → PeerId mapping.
                // This allows us to send messages to their DID without manual discovery.
                // We query by their peer_id, which will cause them to respond with their actual DID.
                debug!("Querying {} for their identity", peer_id);
                let request =
                    variance_identity::protocol::create_peer_id_request(&peer_id.to_string(), None);
                let request_id = self
                    .swarm
                    .behaviour_mut()
                    .identity
                    .send_request(&peer_id, request);

                // Track this as an auto-discovery request (we don't need to respond to anyone)
                self.pending_auto_discovery.insert(request_id, peer_id);
            }
            SwarmEvent::ConnectionClosed { peer_id, cause, .. } => {
                debug!("Connection to {} closed: {:?}", peer_id, cause);

                // Remove stale DID→PeerId mappings for this peer so future sends are
                // correctly detected as offline and queued rather than silently dropped.
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
            SwarmEvent::IncomingConnection { send_back_addr, .. } => {
                debug!("Incoming connection from {}", send_back_addr);
            }
            SwarmEvent::OutgoingConnectionError { peer_id, error, .. } => {
                // Common transient errors (handshake failures, simultaneous dials, etc.) are debug level
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
            SwarmEvent::IncomingConnectionError {
                send_back_addr,
                error,
                ..
            } => {
                // Most incoming connection errors are transient (failed handshakes, etc.)
                debug!(
                    "Incoming connection from {} failed: {}",
                    send_back_addr, error
                );
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_node_creation() {
        let dir = tempdir().unwrap();
        let config = Config {
            storage_path: dir.path().to_path_buf(),
            ..Default::default()
        };

        let (node, _handle) = Node::new(config).unwrap();
        assert!(!node.peer_id().to_string().is_empty());
    }

    #[tokio::test]
    async fn test_node_listen() {
        let dir = tempdir().unwrap();
        let config = Config {
            storage_path: dir.path().to_path_buf(),
            ..Default::default()
        };

        let (mut node, _handle) = Node::new(config.clone()).unwrap();
        node.listen(&config).await.unwrap();
    }
}
