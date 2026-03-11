use anyhow::{Context, Result};
use axum::{extract::State, response::Json, routing::get, Router};
use clap::Parser;
use futures::StreamExt;
use libp2p::{
    identify,
    identity::Keypair,
    kad, noise, ping, relay,
    swarm::{NetworkBehaviour, SwarmEvent},
    tcp, yamux, Multiaddr, PeerId, SwarmBuilder,
};
use serde::Serialize;
use std::{
    collections::HashMap,
    fs::{self, read_to_string},
    num::{NonZeroU32, NonZeroUsize},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};
use tracing::{debug, info, warn};

/// DHT provider key advertised by relay nodes.
/// MUST match `crates/variance-p2p/src/protocols.rs` constant.
const RELAY_PROVIDER_KEY: &[u8] = b"/variance/relay/v1";

#[derive(Parser)]
#[command(
    name = "variance-relay",
    about = "Variance P2P relay node for NAT traversal"
)]
struct Args {
    /// TCP/QUIC listen port
    #[arg(long, default_value = "4001")]
    port: u16,

    /// Directory for persistent state (keypair)
    #[arg(long, default_value_t = default_data_dir())]
    data_dir: String,

    /// Maximum concurrent relay reservations
    #[arg(long, default_value = "128")]
    max_reservations: usize,

    /// Maximum concurrent relay circuits
    #[arg(long, default_value = "256")]
    max_circuits: usize,

    /// Maximum circuit duration in seconds
    #[arg(long, default_value = "1200")]
    max_circuit_duration_secs: u64,

    /// Maximum bytes per circuit (0 = unlimited)
    #[arg(long, default_value = "0")]
    max_circuit_bytes: u64,

    /// How often to log relay stats (seconds, 0 = disabled)
    #[arg(long, default_value = "60")]
    stats_interval_secs: u64,

    /// Port for the HTTP stats endpoint (0 = disabled)
    #[arg(long, default_value = "9090")]
    stats_port: u16,

    /// Bootstrap peers for DHT-based relay auto-discovery.
    /// Format: PEER_ID@MULTIADDR (repeat for multiple peers).
    /// Example: --bootstrap-peers 12D3...@/ip4/1.2.3.4/tcp/4001
    #[arg(long, value_name = "PEERID@MULTIADDR")]
    bootstrap_peers: Vec<String>,
}

/// Snapshot of relay stats for the HTTP `/stats` endpoint.
#[derive(Serialize, Clone)]
struct RelayStatsSnapshot {
    active_reservations: usize,
    active_circuits: usize,
    total_reservations: usize,
    total_circuits: usize,
    denied_reservations: usize,
    denied_circuits: usize,
    tracked_peers: usize,
}

/// Tracks relay activity for operational monitoring.
///
/// Wrapped in `Arc<Mutex<…>>` so the HTTP stats endpoint can read it
/// concurrently with the swarm event loop.
struct RelayStats {
    active_reservations: usize,
    active_circuits: usize,
    total_reservations: usize,
    total_circuits: usize,
    denied_reservations: usize,
    denied_circuits: usize,
    /// Tracks how many active reservations each peer holds so we can clean up
    /// when the peer fully disconnects (all connections closed).
    peer_reservations: HashMap<PeerId, usize>,
}

impl RelayStats {
    fn new() -> Self {
        Self {
            active_reservations: 0,
            active_circuits: 0,
            total_reservations: 0,
            total_circuits: 0,
            denied_reservations: 0,
            denied_circuits: 0,
            peer_reservations: HashMap::new(),
        }
    }

    fn reservation_accepted(&mut self, peer: PeerId) {
        self.active_reservations += 1;
        self.total_reservations += 1;
        *self.peer_reservations.entry(peer).or_insert(0) += 1;
    }

    fn reservation_timed_out(&mut self, peer: &PeerId) {
        self.active_reservations = self.active_reservations.saturating_sub(1);
        if let Some(count) = self.peer_reservations.get_mut(peer) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                self.peer_reservations.remove(peer);
            }
        }
    }

    /// Called when a peer fully disconnects (num_established == 0).
    /// Removes all reservations attributed to this peer.
    fn peer_disconnected(&mut self, peer: &PeerId) {
        if let Some(count) = self.peer_reservations.remove(peer) {
            if count > 0 {
                info!(
                    peer = %peer,
                    orphaned_reservations = count,
                    "Cleaning up reservations for disconnected peer"
                );
                self.active_reservations = self.active_reservations.saturating_sub(count);
            }
        }
    }

    fn log_summary(&self) {
        info!(
            active_reservations = self.active_reservations,
            active_circuits = self.active_circuits,
            total_reservations = self.total_reservations,
            total_circuits = self.total_circuits,
            denied_reservations = self.denied_reservations,
            denied_circuits = self.denied_circuits,
            tracked_peers = self.peer_reservations.len(),
            "Relay stats"
        );
    }

    fn snapshot(&self) -> RelayStatsSnapshot {
        RelayStatsSnapshot {
            active_reservations: self.active_reservations,
            active_circuits: self.active_circuits,
            total_reservations: self.total_reservations,
            total_circuits: self.total_circuits,
            denied_reservations: self.denied_reservations,
            denied_circuits: self.denied_circuits,
            tracked_peers: self.peer_reservations.len(),
        }
    }
}

fn default_data_dir() -> String {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".variance-relay")
        .to_string_lossy()
        .into_owned()
}

#[derive(NetworkBehaviour)]
struct RelayBehaviour {
    relay: relay::Behaviour,
    identify: identify::Behaviour,
    ping: ping::Behaviour,
    kad: kad::Behaviour<kad::store::MemoryStore>,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let args = Args::parse();

    let data_dir = PathBuf::from(&args.data_dir);
    fs::create_dir_all(&data_dir)
        .with_context(|| format!("Failed to create data dir: {}", data_dir.display()))?;

    let keypair = load_or_generate_keypair(&data_dir)?;
    let peer_id = keypair.public().to_peer_id();

    info!("Relay peer ID: {}", peer_id);

    let tcp_addr: Multiaddr = format!("/ip4/0.0.0.0/tcp/{}", args.port)
        .parse()
        .context("invalid TCP multiaddr")?;
    let quic_addr: Multiaddr = format!("/ip4/0.0.0.0/udp/{}/quic-v1", args.port)
        .parse()
        .context("invalid QUIC multiaddr")?;

    let mut swarm = SwarmBuilder::with_existing_identity(keypair.clone())
        .with_tokio()
        .with_tcp(
            tcp::Config::default().nodelay(true),
            noise::Config::new,
            yamux::Config::default,
        )
        .context("Failed to build TCP transport")?
        .with_quic()
        .with_behaviour(|keypair| {
            let relay_config = relay::Config {
                max_reservations: args.max_reservations,
                max_reservations_per_peer: 4,
                reservation_duration: Duration::from_secs(3600),
                max_circuits: args.max_circuits,
                max_circuits_per_peer: 4,
                max_circuit_duration: Duration::from_secs(args.max_circuit_duration_secs),
                max_circuit_bytes: args.max_circuit_bytes,
                ..Default::default()
            };
            // Per-peer rate limits: max 10 reservations per 60s, 30 circuits per 60s
            let relay_config = relay_config
                .reservation_rate_per_peer(
                    NonZeroU32::new(10).expect("10 > 0"),
                    Duration::from_secs(60),
                )
                .circuit_src_per_peer(
                    NonZeroU32::new(30).expect("30 > 0"),
                    Duration::from_secs(60),
                );

            info!(?relay_config, "Relay behaviour configuration");

            // Kademlia for relay auto-discovery: operate in server mode so peers
            // can store and retrieve provider records through this node.
            let store = kad::store::MemoryStore::new(keypair.public().to_peer_id());
            let mut kad_config = kad::Config::default();
            kad_config.set_replication_factor(NonZeroUsize::new(20).expect("20 > 0"));
            kad_config.set_provider_record_ttl(Some(Duration::from_secs(24 * 60 * 60)));
            let mut kad =
                kad::Behaviour::with_config(keypair.public().to_peer_id(), store, kad_config);
            kad.set_mode(Some(kad::Mode::Server));

            Ok(RelayBehaviour {
                relay: relay::Behaviour::new(keypair.public().to_peer_id(), relay_config),
                identify: identify::Behaviour::new(identify::Config::new(
                    "/variance-relay/1.0.0".to_string(),
                    keypair.public(),
                )),
                ping: ping::Behaviour::new(ping::Config::new()),
                kad,
            })
        })
        .context("Failed to build relay behaviour")?
        .with_swarm_config(|c| c.with_idle_connection_timeout(Duration::from_secs(600)))
        .build();

    swarm
        .listen_on(tcp_addr.clone())
        .context("Failed to listen on TCP")?;
    swarm
        .listen_on(quic_addr.clone())
        .context("Failed to listen on QUIC")?;

    info!("Listening on:");
    info!("  {}/p2p/{}", tcp_addr, peer_id);
    info!("  {}/p2p/{}", quic_addr, peer_id);
    info!("");
    info!("Add to client config:");
    info!(
        "  relay_peers = [{{ peer_id = \"{}\", multiaddr = \"/ip4/<YOUR_IP>/tcp/{}\" }}]",
        peer_id, args.port
    );

    // Add bootstrap peers and start DHT bootstrap for relay auto-discovery.
    let mut has_bootstrap = false;
    for spec in &args.bootstrap_peers {
        match parse_bootstrap_peer(spec) {
            Ok((bp_peer_id, bp_addr)) => {
                swarm
                    .behaviour_mut()
                    .kad
                    .add_address(&bp_peer_id, bp_addr.clone());
                if let Err(e) = swarm.dial(bp_addr.clone()) {
                    warn!("Failed to dial bootstrap peer {}: {}", bp_peer_id, e);
                } else {
                    info!("Dialing bootstrap peer {} at {}", bp_peer_id, bp_addr);
                    has_bootstrap = true;
                }
            }
            Err(e) => warn!("Invalid bootstrap peer '{}': {}", spec, e),
        }
    }
    if has_bootstrap {
        if let Err(e) = swarm.behaviour_mut().kad.bootstrap() {
            warn!("DHT bootstrap failed: {:?}", e);
        }
    }

    // Register as a relay provider in the DHT so clients can discover us.
    // Kademlia will propagate this record to the network once bootstrapped.
    let relay_key = kad::RecordKey::new(&RELAY_PROVIDER_KEY);
    match swarm.behaviour_mut().kad.start_providing(relay_key) {
        Ok(_) => info!("Registered as relay provider in DHT"),
        Err(e) => warn!("Failed to register relay provider: {:?}", e),
    }

    let stats = Arc::new(Mutex::new(RelayStats::new()));

    // Spawn HTTP stats server if a port is configured.
    if args.stats_port > 0 {
        let stats_http = stats.clone();
        let addr = std::net::SocketAddr::from(([0, 0, 0, 0], args.stats_port));
        let app = Router::new()
            .route("/stats", get(stats_handler))
            .with_state(stats_http);
        tokio::spawn(async move {
            info!("Relay stats HTTP server listening on {}", addr);
            let listener = match tokio::net::TcpListener::bind(addr).await {
                Ok(l) => l,
                Err(e) => {
                    warn!("Failed to bind stats HTTP server on {}: {}", addr, e);
                    return;
                }
            };
            if let Err(e) = axum::serve(listener, app).await {
                warn!("Stats HTTP server error: {}", e);
            }
        });
    }

    // Optional periodic stats reporting
    let stats_interval = if args.stats_interval_secs > 0 {
        Some(tokio::time::interval(Duration::from_secs(
            args.stats_interval_secs,
        )))
    } else {
        None
    };
    // Pin the interval so we can poll it in the select loop
    tokio::pin!(stats_interval);

    loop {
        tokio::select! {
            event = swarm.next() => {
                match event {
                    Some(SwarmEvent::NewListenAddr { address, .. }) => {
                        info!("Now listening on {}/p2p/{}", address, peer_id);
                    }
                    Some(SwarmEvent::ConnectionEstablished { peer_id: remote, .. }) => {
                        debug!("Connection established with {}", remote);
                    }
                    Some(SwarmEvent::ConnectionClosed { peer_id: remote, num_established, cause, .. }) => {
                        debug!("Connection closed with {}: {:?}", remote, cause);
                        if num_established == 0 {
                            if let Ok(mut s) = stats.lock() {
                                s.peer_disconnected(&remote);
                            }
                        }
                    }
                    Some(SwarmEvent::Behaviour(RelayBehaviourEvent::Relay(event))) => {
                        if let Ok(mut s) = stats.lock() {
                            handle_relay_event(event, &mut s);
                        }
                    }
                    Some(SwarmEvent::Behaviour(RelayBehaviourEvent::Identify(
                        identify::Event::Received { peer_id: remote, info, .. },
                    ))) => {
                        debug!(
                            peer = %remote,
                            agent = %info.agent_version,
                            "Identified peer"
                        );
                        // Add the remote peer's listen addresses to Kademlia routing.
                        for addr in &info.listen_addrs {
                            swarm
                                .behaviour_mut()
                                .kad
                                .add_address(&remote, addr.clone());
                        }
                        // Use the address the remote peer observed us at to learn
                        // our own public address — NOT the remote's listen_addrs.
                        swarm.add_external_address(info.observed_addr);
                    }
                    Some(SwarmEvent::Behaviour(RelayBehaviourEvent::Kad(event))) => {
                        match event {
                            kad::Event::OutboundQueryProgressed { result, .. } => match result {
                                kad::QueryResult::StartProviding(Ok(ok)) => {
                                    info!(
                                        "DHT provider record published for key {:?}",
                                        ok.key
                                    );
                                }
                                kad::QueryResult::StartProviding(Err(e)) => {
                                    warn!("Failed to publish DHT provider record: {:?}", e);
                                }
                                _ => {}
                            },
                            kad::Event::RoutingUpdated { peer, .. } => {
                                debug!("DHT routing updated: {}", peer);
                            }
                            _ => {}
                        }
                    }
                    Some(SwarmEvent::OutgoingConnectionError { peer_id, error, .. }) => {
                        warn!("Outgoing connection error to {:?}: {}", peer_id, error);
                    }
                    Some(SwarmEvent::IncomingConnectionError { error, .. }) => {
                        debug!("Incoming connection error: {}", error);
                    }
                    Some(SwarmEvent::ExternalAddrConfirmed { address }) => {
                        info!("External address confirmed: {}", address);
                    }
                    _ => {}
                }
            }
            // Periodic stats logging (only enabled when stats_interval_secs > 0)
            _ = async {
                match stats_interval.as_mut().as_pin_mut() {
                    Some(mut interval) => interval.tick().await,
                    None => std::future::pending().await,
                }
            } => {
                if let Ok(s) = stats.lock() {
                    s.log_summary();
                }
            }
            _ = tokio::signal::ctrl_c() => {
                info!("Shutting down relay node");
                if let Ok(s) = stats.lock() {
                    s.log_summary();
                }
                break;
            }
        }
    }

    Ok(())
}

async fn stats_handler(State(stats): State<Arc<Mutex<RelayStats>>>) -> Json<RelayStatsSnapshot> {
    let snapshot = stats
        .lock()
        .map(|s| s.snapshot())
        .unwrap_or_else(|_| RelayStatsSnapshot {
            active_reservations: 0,
            active_circuits: 0,
            total_reservations: 0,
            total_circuits: 0,
            denied_reservations: 0,
            denied_circuits: 0,
            tracked_peers: 0,
        });
    Json(snapshot)
}

fn handle_relay_event(event: relay::Event, stats: &mut RelayStats) {
    match event {
        relay::Event::ReservationReqAccepted { src_peer_id, .. } => {
            stats.reservation_accepted(src_peer_id);
            info!(peer = %src_peer_id, "Relay reservation accepted");
        }
        relay::Event::ReservationReqDenied { src_peer_id, .. } => {
            stats.denied_reservations += 1;
            warn!(peer = %src_peer_id, "Relay reservation denied");
        }
        relay::Event::ReservationTimedOut { src_peer_id } => {
            stats.reservation_timed_out(&src_peer_id);
            debug!(peer = %src_peer_id, "Relay reservation timed out");
        }
        relay::Event::CircuitReqAccepted {
            src_peer_id,
            dst_peer_id,
        } => {
            stats.active_circuits += 1;
            stats.total_circuits += 1;
            info!(src = %src_peer_id, dst = %dst_peer_id, "Circuit accepted");
        }
        relay::Event::CircuitReqDenied {
            src_peer_id,
            dst_peer_id,
            ..
        } => {
            stats.denied_circuits += 1;
            debug!(src = %src_peer_id, dst = %dst_peer_id, "Circuit denied");
        }
        relay::Event::CircuitClosed {
            src_peer_id,
            dst_peer_id,
            error,
        } => {
            stats.active_circuits = stats.active_circuits.saturating_sub(1);
            if let Some(e) = error {
                debug!(src = %src_peer_id, dst = %dst_peer_id, error = %e, "Circuit closed with error");
            } else {
                debug!(src = %src_peer_id, dst = %dst_peer_id, "Circuit closed");
            }
        }
        _ => {}
    }
}

/// Parse a bootstrap peer spec in `PEERID@MULTIADDR` format.
fn parse_bootstrap_peer(spec: &str) -> Result<(PeerId, Multiaddr)> {
    let (peer_id_str, addr_str) = spec
        .split_once('@')
        .with_context(|| format!("expected PEERID@MULTIADDR, got '{}'", spec))?;
    let peer_id: PeerId = peer_id_str
        .parse()
        .with_context(|| format!("invalid PeerId: '{}'", peer_id_str))?;
    let addr: Multiaddr = addr_str
        .parse()
        .with_context(|| format!("invalid multiaddr: '{}'", addr_str))?;
    Ok((peer_id, addr))
}

/// Load keypair from `<data_dir>/keypair.json`, or generate and persist a new one.
fn load_or_generate_keypair(data_dir: &Path) -> Result<Keypair> {
    let path = data_dir.join("keypair.json");

    if path.exists() {
        let json = read_to_string(&path)
            .with_context(|| format!("Failed to read keypair from {}", path.display()))?;
        let hex: String = serde_json::from_str(&json).context("Failed to parse keypair JSON")?;
        let bytes = hex::decode(&hex).context("Failed to hex-decode keypair bytes")?;
        let keypair = Keypair::from_protobuf_encoding(&bytes)
            .context("Failed to decode keypair from protobuf")?;
        info!("Loaded existing keypair from {}", path.display());
        Ok(keypair)
    } else {
        let keypair = Keypair::generate_ed25519();
        let bytes = keypair
            .to_protobuf_encoding()
            .context("Failed to encode keypair")?;
        let hex = hex::encode(&bytes);
        let json = serde_json::to_string(&hex).context("Failed to serialize keypair JSON")?;
        fs::write(&path, &json)
            .with_context(|| format!("Failed to write keypair to {}", path.display()))?;
        info!("Generated and persisted new keypair to {}", path.display());
        Ok(keypair)
    }
}
