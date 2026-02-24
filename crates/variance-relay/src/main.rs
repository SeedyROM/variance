use anyhow::{Context, Result};
use clap::Parser;
use futures::StreamExt;
use libp2p::{
    identify,
    identity::Keypair,
    noise, ping, relay,
    swarm::{NetworkBehaviour, SwarmEvent},
    tcp, yamux, Multiaddr, SwarmBuilder,
};
use std::{
    fs::read_to_string,
    path::{Path, PathBuf},
};
use tracing::{debug, info, warn};

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
    std::fs::create_dir_all(&data_dir)
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
            Ok(RelayBehaviour {
                relay: relay::Behaviour::new(keypair.public().to_peer_id(), Default::default()),
                identify: identify::Behaviour::new(identify::Config::new(
                    "/variance-relay/1.0.0".to_string(),
                    keypair.public(),
                )),
                ping: ping::Behaviour::new(ping::Config::new()),
            })
        })
        .context("Failed to build relay behaviour")?
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
                    Some(SwarmEvent::ConnectionClosed { peer_id: remote, cause, .. }) => {
                        debug!("Connection closed with {}: {:?}", remote, cause);
                    }
                    Some(SwarmEvent::Behaviour(RelayBehaviourEvent::Relay(event))) => {
                        handle_relay_event(event);
                    }
                    Some(SwarmEvent::Behaviour(RelayBehaviourEvent::Identify(
                        identify::Event::Received { peer_id: remote, info, .. },
                    ))) => {
                        debug!("Identified {}: agent={}", remote, info.agent_version);
                    }
                    Some(SwarmEvent::OutgoingConnectionError { peer_id, error, .. }) => {
                        warn!("Outgoing connection error to {:?}: {}", peer_id, error);
                    }
                    Some(SwarmEvent::IncomingConnectionError { error, .. }) => {
                        debug!("Incoming connection error: {}", error);
                    }
                    _ => {}
                }
            }
            _ = tokio::signal::ctrl_c() => {
                info!("Shutting down relay node");
                break;
            }
        }
    }

    Ok(())
}

fn handle_relay_event(event: relay::Event) {
    match event {
        relay::Event::ReservationReqAccepted { src_peer_id, .. } => {
            info!("Relay reservation accepted for {}", src_peer_id);
        }
        relay::Event::ReservationReqDenied { src_peer_id } => {
            warn!("Relay reservation denied for {}", src_peer_id);
        }
        relay::Event::CircuitReqAccepted {
            src_peer_id,
            dst_peer_id,
        } => {
            info!(
                "Circuit request accepted: {} → {}",
                src_peer_id, dst_peer_id
            );
        }
        relay::Event::CircuitReqDenied {
            src_peer_id,
            dst_peer_id,
        } => {
            debug!("Circuit request denied: {} → {}", src_peer_id, dst_peer_id);
        }
        relay::Event::CircuitClosed {
            src_peer_id,
            dst_peer_id,
            error,
        } => {
            if let Some(e) = error {
                debug!("Circuit closed: {} → {}: {}", src_peer_id, dst_peer_id, e);
            } else {
                debug!("Circuit closed normally: {} → {}", src_peer_id, dst_peer_id);
            }
        }
        _ => {}
    }
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
        std::fs::write(&path, &json)
            .with_context(|| format!("Failed to write keypair to {}", path.display()))?;
        info!("Generated and persisted new keypair to {}", path.display());
        Ok(keypair)
    }
}
