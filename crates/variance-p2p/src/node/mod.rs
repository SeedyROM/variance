mod event_handlers;

use crate::{
    behaviour::VarianceBehaviour,
    commands::*,
    config::Config,
    error::*,
    events::{DirectMessageEvent, EventChannels},
    handlers,
    rate_limiter::PeerRateLimiter,
};
use futures::StreamExt;
use libp2p::{
    dcutr, gossipsub, identify, kad, mdns, noise, ping, tcp, yamux, Multiaddr, PeerId, Swarm,
    SwarmBuilder,
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
    /// Per-peer, per-protocol inbound rate limiter
    rate_limiter: PeerRateLimiter,
    /// Peers discovered via mDNS or Identify, keyed by PeerId with their known addresses.
    /// Used by the periodic reconnect loop to redial peers that dropped.
    known_peers: HashMap<PeerId, Vec<Multiaddr>>,
    /// Peer IDs of configured relay nodes, used to trigger circuit listen after connection.
    relay_peer_ids: std::collections::HashSet<PeerId>,
}

impl Node {
    /// Create a new P2P node and return both the node and a handle for sending commands
    pub fn new(config: Config) -> Result<(Self, NodeHandle)> {
        let keypair = libp2p::identity::Keypair::generate_ed25519();
        let peer_id = keypair.public().to_peer_id();

        info!("Creating P2P node with peer ID: {}", peer_id);

        // Create command channel for application layer to send commands to the node
        let (command_tx, command_rx) = tokio::sync::mpsc::channel(100);

        // Capture config values needed inside the SwarmBuilder closure (closures can't
        // borrow config across the builder chain).
        let kad_replication_factor = config.kad_config.replication_factor;
        let kad_provider_record_ttl = config.kad_config.provider_record_ttl;
        let gossipsub_heartbeat_interval = config.gossipsub_config.heartbeat_interval_secs;
        let gossipsub_history_length = config.gossipsub_config.history_length;
        let gossipsub_history_gossip = config.gossipsub_config.history_gossip;

        // Build Swarm — with_relay_client() must come before with_behaviour() and changes
        // the closure signature from |keypair| to |keypair, relay_client|.
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
            .with_relay_client(noise::Config::new, yamux::Config::default)
            .map_err(|e| Error::Transport {
                source: Box::new(e),
            })?
            .with_behaviour(|keypair, relay_client| {
                let peer_id = keypair.public().to_peer_id();

                // Build Kademlia DHT
                let mut kad_config = kad::Config::default();
                kad_config.set_replication_factor(
                    NonZeroUsize::new(kad_replication_factor)
                        .unwrap_or(NonZeroUsize::new(20).unwrap()),
                );
                kad_config
                    .set_provider_record_ttl(Some(Duration::from_secs(kad_provider_record_ttl)));
                let store = kad::store::MemoryStore::new(peer_id);
                let mut kad = kad::Behaviour::with_config(peer_id, store, kad_config);
                // libp2p 0.55+ defaults to client mode; nodes must explicitly opt into server
                // mode so they accept and serve incoming Kademlia requests (provider record
                // queries). Without this, /ipfs/kad/1.0.0 is rejected and get_providers can
                // never find peers that registered via start_providing.
                kad.set_mode(Some(kad::Mode::Server));

                // Build GossipSub
                let gossipsub_config = gossipsub::ConfigBuilder::default()
                    .heartbeat_interval(Duration::from_secs(gossipsub_heartbeat_interval))
                    .history_length(gossipsub_history_length)
                    .history_gossip(gossipsub_history_gossip)
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
                let mdns =
                    mdns::tokio::Behaviour::new(mdns::Config::default(), peer_id).map_err(|e| {
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
                let offline_messages =
                    crate::protocols::messaging::create_offline_message_behaviour();
                let signaling = crate::protocols::media::create_signaling_behaviour();
                let direct_messages =
                    crate::protocols::messaging::create_direct_message_behaviour();
                let typing_indicators =
                    crate::protocols::messaging::create_typing_indicator_behaviour();

                Ok(VarianceBehaviour {
                    relay_client,
                    dcutr: dcutr::Behaviour::new(peer_id),
                    kad,
                    gossipsub,
                    mdns,
                    identify,
                    ping,
                    identity,
                    offline_messages,
                    signaling,
                    direct_messages,
                    typing_indicators,
                })
            })
            .map_err(|e| Error::Transport {
                source: Box::new(e),
            })?
            .with_swarm_config(|c| {
                c.with_idle_connection_timeout(Duration::from_secs(300))
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

        let relay_peer_ids = config
            .relay_peers
            .iter()
            .filter_map(|r| r.peer_id.parse().ok())
            .collect();

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
            rate_limiter: PeerRateLimiter::new(),
            known_peers: HashMap::new(),
            relay_peer_ids,
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

        // Dial configured relay peers. Circuit listen is triggered in handle_connection_established
        // once the connection succeeds and we can reserve a slot.
        for relay in &config.relay_peers {
            let peer_id: PeerId = relay.peer_id.parse().map_err(|_| Error::InvalidPeerId {
                peer_id: relay.peer_id.clone(),
            })?;
            self.swarm
                .behaviour_mut()
                .kad
                .add_address(&peer_id, relay.multiaddr.clone());
            self.swarm
                .dial(relay.multiaddr.clone())
                .map_err(|e| Error::Transport {
                    source: Box::new(e),
                })?;
            info!("Dialing relay peer: {} at {}", peer_id, relay.multiaddr);
        }

        Ok(())
    }

    pub async fn run(&mut self, mut shutdown: tokio::sync::mpsc::Receiver<()>) -> Result<()> {
        // Delay the first tick so initial mDNS connections have time to establish.
        let start = tokio::time::Instant::now() + Duration::from_secs(30);
        let mut reconnect_interval = tokio::time::interval_at(start, Duration::from_secs(30));
        reconnect_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        loop {
            select! {
                event = self.swarm.select_next_some() => {
                    self.handle_event(event).await;
                }
                Some(command) = self.command_rx.recv() => {
                    self.handle_command(command).await;
                }
                _ = reconnect_interval.tick() => {
                    self.reconnect_known_peers();
                }
                _ = shutdown.recv() => {
                    info!("Shutdown signal received");
                    break;
                }
            }
        }

        Ok(())
    }

    /// Periodically re-dial known peers that are not currently connected.
    /// Handles missed mDNS announcements at startup and connections that
    /// dropped due to idle timeout.
    fn reconnect_known_peers(&mut self) {
        let connected: std::collections::HashSet<PeerId> =
            self.swarm.connected_peers().cloned().collect();

        let to_dial: Vec<(PeerId, Vec<Multiaddr>)> = self
            .known_peers
            .iter()
            .filter(|(peer_id, _)| !connected.contains(peer_id))
            .map(|(peer_id, addrs)| (*peer_id, addrs.clone()))
            .collect();

        if !to_dial.is_empty() {
            debug!(
                "Reconnect scan: {} known peer(s) not connected, redialing",
                to_dial.len()
            );
        }

        for (peer_id, addrs) in to_dial {
            if let Err(e) = self.swarm.dial(
                libp2p::swarm::dial_opts::DialOpts::peer_id(peer_id)
                    .addresses(addrs)
                    .build(),
            ) {
                debug!("Reconnect dial to {} failed: {}", peer_id, e);
            }
        }
    }

    /// Handle a command from the application layer.
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
            NodeCommand::SendTypingIndicator {
                peer_did,
                indicator,
            } => {
                let did_to_peer = self.did_to_peer.read().await;
                if let Some(peer) = did_to_peer.get(&peer_did) {
                    self.swarm
                        .behaviour_mut()
                        .typing_indicators
                        .send_request(peer, indicator);
                } else {
                    debug!(
                        "Cannot send typing indicator: unknown peer DID {}",
                        peer_did
                    );
                }
            }
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
