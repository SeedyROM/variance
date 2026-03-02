mod event_handlers;

use std::collections::{HashMap, HashSet};
use std::num::{NonZeroU8, NonZeroUsize};
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use libp2p::{
    dcutr,
    gossipsub::{self, IdentTopic},
    identify, kad, mdns, noise, ping,
    request_response::OutboundRequestId,
    swarm::dial_opts::DialOpts,
    tcp, yamux, Multiaddr, PeerId, Swarm, SwarmBuilder,
};
use prost::Message;
use tokio::sync::oneshot;
use tokio::{select, sync};
use tracing::{debug, info, warn};

use variance_identity::protocol;
use variance_proto::identity_proto::{IdentityFound, IdentityResponse};

use crate::{
    behaviour::VarianceBehaviour,
    commands::*,
    config::Config,
    error::*,
    events::{DirectMessageEvent, EventChannels},
    handlers::{identity, offline, signaling},
    peer_store::PeerStore,
    rate_limiter::PeerRateLimiter,
};

/// Response channel for a single identity request to a peer.
type IdentityRequestOneshot = oneshot::Sender<Result<IdentityResponse>>;
/// Type alias for the response channel of a get_providers query: a oneshot sender that takes a Result with either a Vec of PeerIds or an Error.
type ProviderQueryOneshot = oneshot::Sender<Result<Vec<PeerId>>>;
/// Type alias for the pending state of a get_providers query: the list of PeerIds accumulated so far and the oneshot sender to respond to when the query completes.
type ProviderQueryResult = (Vec<PeerId>, ProviderQueryOneshot);
/// Response channel for a broadcast DID resolve: a oneshot sender that takes a Result with either an IdentityFound or an Error.
type BroadcastDidResolveOneshot = oneshot::Sender<Result<IdentityFound>>;
/// Tracks a broadcast DID resolve: how many individual requests are still pending
/// and the oneshot channel to fire when the first Found response arrives.
type BroadcastResolve = (usize, BroadcastDidResolveOneshot);

pub struct Node {
    swarm: Swarm<VarianceBehaviour>,
    peer_id: PeerId,
    /// Local DID, set via SetLocalIdentity command after node initialization
    local_did: Arc<sync::RwLock<Option<String>>>,
    identity_handler: Arc<identity::IdentityHandler>,
    offline_handler: Arc<offline::OfflineMessageHandler>,
    signaling_handler: Arc<signaling::SignalingHandler>,
    events: EventChannels,
    command_rx: sync::mpsc::Receiver<NodeCommand>,
    /// Pending identity requests awaiting responses
    pending_identity_requests: HashMap<OutboundRequestId, IdentityRequestOneshot>,
    /// Pending get_providers queries: query_id → (accumulated peers, response sender)
    pending_provider_queries: HashMap<kad::QueryId, ProviderQueryResult>,
    /// DID to PeerId mapping for routing signaling messages (connected peers only)
    did_to_peer: Arc<sync::RwLock<HashMap<String, PeerId>>>,
    /// Persistent DID→PeerId store backed by sled (survives restarts)
    peer_store: PeerStore,
    /// Broadcast DID resolution: DID → (remaining request count, response sender).
    /// Fires on the first Found response; if all requests fail, fires an error.
    pending_did_broadcasts: HashMap<String, BroadcastResolve>,
    /// Maps individual identity request_id → DID being resolved (for broadcast lookups).
    pending_resolve_requests: HashMap<OutboundRequestId, String>,
    /// Auto-discovery requests sent when peers connect: request_id → peer_id
    pending_auto_discovery: HashMap<OutboundRequestId, libp2p::PeerId>,
    /// Per-peer, per-protocol inbound rate limiter
    rate_limiter: PeerRateLimiter,
    /// Tracks in-flight direct message sends: OutboundRequestId → (message_id, recipient_did).
    /// Used to correlate OutboundFailure/ACK events back to specific messages.
    pending_dm_sends: HashMap<OutboundRequestId, (String, String)>,
    /// Peers discovered via mDNS or Identify, keyed by PeerId with their known addresses.
    /// Used by the periodic reconnect loop to redial peers that dropped.
    known_peers: HashMap<PeerId, Vec<Multiaddr>>,
    /// Peer IDs of configured relay nodes, used to trigger circuit listen after connection.
    relay_peer_ids: HashSet<PeerId>,
    /// In-flight DHT get_providers query for relay discovery (at most one at a time).
    pending_relay_query: Option<kad::QueryId>,
    /// How often to re-query the DHT for relay providers (seconds, 0 = disabled).
    relay_discovery_interval_secs: u64,
}

impl Node {
    /// Create a new P2P node and return both the node and a handle for sending commands
    pub fn new(config: Config) -> Result<(Self, NodeHandle)> {
        let keypair = libp2p::identity::Keypair::generate_ed25519();
        let peer_id = keypair.public().to_peer_id();

        info!("Creating P2P node with peer ID: {}", peer_id);

        // Create command channel for application layer to send commands to the node
        let (command_tx, command_rx) = sync::mpsc::channel(100);

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
                let rename = crate::protocols::identity::create_rename_behaviour();

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
                    rename,
                })
            })
            .map_err(|e| Error::Transport {
                source: Box::new(e),
            })?
            .with_swarm_config(|c| {
                c.with_idle_connection_timeout(Duration::from_secs(300))
                    // Limit concurrent dials to prevent connection storms
                    .with_dial_concurrency_factor(NonZeroU8::new(8).expect("8 is non-zero"))
                    // Only keep one connection per peer
                    .with_max_negotiating_inbound_streams(128)
            })
            .build();

        // Initialize protocol handlers
        let identity_handler = Arc::new(identity::IdentityHandler::new(peer_id));

        let offline_handler = Arc::new(offline::OfflineMessageHandler::with_local_storage(
            peer_id.to_string(),
            &config.storage_path.join("messages"),
        )?);

        let signaling_handler =
            Arc::new(signaling::SignalingHandler::new(identity_handler.clone()));

        let events = EventChannels::default();

        let relay_peer_ids = config
            .relay_peers
            .iter()
            .filter_map(|r| r.peer_id.parse().ok())
            .collect();

        let relay_discovery_interval_secs = config.relay_discovery_interval_secs;

        let peer_store = PeerStore::open(&config.storage_path)?;

        let node = Node {
            swarm,
            peer_id,
            local_did: Arc::new(sync::RwLock::new(None)),
            identity_handler,
            offline_handler,
            signaling_handler,
            events,
            command_rx,
            pending_identity_requests: HashMap::new(),
            pending_provider_queries: HashMap::new(),
            did_to_peer: Arc::new(sync::RwLock::new(HashMap::new())),
            peer_store,
            pending_did_broadcasts: HashMap::new(),
            pending_resolve_requests: HashMap::new(),
            pending_auto_discovery: HashMap::new(),
            rate_limiter: PeerRateLimiter::new(),
            pending_dm_sends: HashMap::new(),
            known_peers: HashMap::new(),
            relay_peer_ids,
            pending_relay_query: None,
            relay_discovery_interval_secs,
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

    pub async fn run(&mut self, mut shutdown: sync::mpsc::Receiver<()>) -> Result<()> {
        // Delay first ticks so initial connections have time to establish.
        let start = tokio::time::Instant::now() + Duration::from_secs(30);
        let mut reconnect_interval = tokio::time::interval_at(start, Duration::from_secs(30));
        reconnect_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        // Relay discovery: first tick at 30s, then every relay_discovery_interval_secs.
        // Disabled when interval is 0.
        let relay_interval_secs = self.relay_discovery_interval_secs;
        let relay_interval = if relay_interval_secs > 0 {
            let period = Duration::from_secs(relay_interval_secs);
            let mut iv = tokio::time::interval_at(start, period);
            iv.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            Some(iv)
        } else {
            None
        };
        tokio::pin!(relay_interval);

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
                _ = async {
                    match relay_interval.as_mut().as_pin_mut() {
                        Some(mut iv) => iv.tick().await,
                        None => std::future::pending().await,
                    }
                } => {
                    self.trigger_relay_discovery();
                }
                _ = shutdown.recv() => {
                    info!("Shutdown signal received");
                    break;
                }
            }
        }

        Ok(())
    }

    /// Issue a DHT `get_providers` for the relay provider key.
    /// Skipped if a query is already in flight.
    fn trigger_relay_discovery(&mut self) {
        if self.pending_relay_query.is_some() {
            debug!("Relay discovery already in progress, skipping");
            return;
        }
        let key = kad::RecordKey::new(&crate::protocols::RELAY_PROVIDER_KEY);
        info!("Querying DHT for relay providers");
        let query_id = self.swarm.behaviour_mut().kad.get_providers(key);
        self.pending_relay_query = Some(query_id);
    }

    /// Periodically re-dial known peers that are not currently connected.
    /// Handles missed mDNS announcements at startup and connections that
    /// dropped due to idle timeout.
    fn reconnect_known_peers(&mut self) {
        let connected: HashSet<PeerId> = self.swarm.connected_peers().cloned().collect();

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
            if let Err(e) = self
                .swarm
                .dial(DialOpts::peer_id(peer_id).addresses(addrs).build())
            {
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
                // Look up peer ID from DID (in-memory, then persisted store)
                let did_to_peer = self.did_to_peer.read().await;
                let peer = did_to_peer
                    .get(&peer_did)
                    .copied()
                    .or_else(|| self.peer_store.get(&peer_did));
                if let Some(peer) = peer {
                    debug!("Sending signaling message to {} ({})", peer_did, peer);
                    self.swarm
                        .behaviour_mut()
                        .signaling
                        .send_request(&peer, message);
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

                let topic_hash = IdentTopic::new(&topic);
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

                let topic_hash = IdentTopic::new(&topic);

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

                let topic_hash = IdentTopic::new(&topic);

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
                mls_key_package,
            } => {
                // Store local DID for self-messaging support
                *self.local_did.write().await = Some(did.clone());

                let handler = self.identity_handler.clone();
                tokio::spawn(async move {
                    handler
                        .set_local_identity(did, olm_identity_key, one_time_keys, mls_key_package)
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

                let request = protocol::create_did_request(&did, None);
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
                let peer = did_to_peer
                    .get(&peer_did)
                    .copied()
                    .or_else(|| self.peer_store.get(&peer_did));
                if let Some(peer) = peer {
                    if !self.swarm.is_connected(&peer) {
                        warn!(
                            "Peer {} ({}) is known but not connected, cannot send",
                            peer_did, peer
                        );
                        let _ = response_tx.send(Err(Error::Protocol {
                            message: format!("Peer not connected: {}", peer_did),
                        }));
                    } else {
                        debug!("Sending direct message to {} ({})", peer_did, peer);
                        let msg_id = message.id.clone();
                        let request_id = self
                            .swarm
                            .behaviour_mut()
                            .direct_messages
                            .send_request(&peer, message);
                        self.pending_dm_sends
                            .insert(request_id, (msg_id, peer_did.clone()));
                        let _ = response_tx.send(Ok(()));
                    }
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
                let peer = did_to_peer
                    .get(&peer_did)
                    .copied()
                    .or_else(|| self.peer_store.get(&peer_did));
                if let Some(peer) = peer {
                    self.swarm
                        .behaviour_mut()
                        .typing_indicators
                        .send_request(&peer, indicator);
                } else {
                    debug!(
                        "Cannot send typing indicator: unknown peer DID {}",
                        peer_did
                    );
                }
            }
            NodeCommand::BroadcastUsernameChange {
                did,
                username,
                discriminator,
            } => {
                let notification = variance_proto::identity_proto::UsernameChanged {
                    did,
                    username,
                    discriminator,
                };
                let peers: Vec<PeerId> = self.swarm.connected_peers().cloned().collect();
                debug!(
                    "Broadcasting username change to {} connected peer(s)",
                    peers.len()
                );
                for peer in peers {
                    self.swarm
                        .behaviour_mut()
                        .rename
                        .send_request(&peer, notification.clone());
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
