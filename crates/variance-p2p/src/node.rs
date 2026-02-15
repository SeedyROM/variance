use crate::{behaviour::VarianceBehaviour, config::Config, error::*};
use futures::StreamExt;
use libp2p::{
    gossipsub, identify, kad, mdns, noise, ping, tcp, yamux, PeerId, Swarm, SwarmBuilder,
};
use libp2p::swarm::SwarmEvent;
use std::num::NonZeroUsize;
use std::time::Duration;
use tokio::select;
use tracing::{debug, info, warn};

pub struct Node {
    swarm: Swarm<VarianceBehaviour>,
    peer_id: PeerId,
}

impl Node {
    pub fn new(config: Config) -> Result<Self> {
        let keypair = libp2p::identity::Keypair::generate_ed25519();
        let peer_id = keypair.public().to_peer_id();

        info!("Creating P2P node with peer ID: {}", peer_id);

        // Build Kademlia DHT
        let mut kad_config = kad::Config::default();
        kad_config.set_replication_factor(
            NonZeroUsize::new(config.kad_config.replication_factor).unwrap_or(NonZeroUsize::new(20).unwrap()),
        );
        kad_config.set_provider_record_ttl(Some(Duration::from_secs(
            config.kad_config.provider_record_ttl,
        )));

        let store = kad::store::MemoryStore::new(peer_id);
        let kad = kad::Behaviour::with_config(peer_id, store, kad_config);

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

        // Combine into VarianceBehaviour
        let behaviour = VarianceBehaviour {
            kad,
            gossipsub,
            mdns,
            identify,
            ping,
        };

        // Build Swarm
        let swarm = SwarmBuilder::with_existing_identity(keypair)
            .with_tokio()
            .with_tcp(
                tcp::Config::default(),
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
            .with_swarm_config(|c| c.with_idle_connection_timeout(Duration::from_secs(60)))
            .build();

        Ok(Node { swarm, peer_id })
    }

    pub fn peer_id(&self) -> &PeerId {
        &self.peer_id
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

            info!("Added bootstrap peer: {} at {}", peer_id, bootstrap.multiaddr);
        }

        // Bootstrap the DHT
        if !config.bootstrap_peers.is_empty() {
            self.swarm.behaviour_mut().kad.bootstrap().map_err(|e| Error::Kad {
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
                _ = shutdown.recv() => {
                    info!("Shutdown signal received");
                    break;
                }
            }
        }

        Ok(())
    }

    async fn handle_event(&mut self, event: SwarmEvent<crate::behaviour::VarianceBehaviourEvent>) {
        match event {
            SwarmEvent::NewListenAddr { address, .. } => {
                info!("Listening on {}", address);
            }
            SwarmEvent::Behaviour(event) => match event {
                crate::behaviour::VarianceBehaviourEvent::Kad(kad_event) => match kad_event {
                    kad::Event::RoutingUpdated { peer, addresses, .. } => {
                        debug!("Routing updated for {}: {:?}", peer, addresses);
                    }
                    kad::Event::InboundRequest { request } => {
                        debug!("Inbound DHT request: {:?}", request);
                    }
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
                    }
                }
                crate::behaviour::VarianceBehaviourEvent::Mdns(mdns_event) => match mdns_event {
                    mdns::Event::Discovered(peers) => {
                        for (peer_id, multiaddr) in peers {
                            info!("Discovered peer via mDNS: {} at {}", peer_id, multiaddr);
                            self.swarm.behaviour_mut().kad.add_address(&peer_id, multiaddr);
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
            },
            SwarmEvent::ConnectionEstablished {
                peer_id, endpoint, ..
            } => {
                info!("Connection established to {} via {}", peer_id, endpoint.get_remote_address());
            }
            SwarmEvent::ConnectionClosed { peer_id, cause, .. } => {
                debug!("Connection to {} closed: {:?}", peer_id, cause);
            }
            SwarmEvent::IncomingConnection { send_back_addr, .. } => {
                debug!("Incoming connection from {}", send_back_addr);
            }
            SwarmEvent::OutgoingConnectionError { peer_id, error, .. } => {
                if let Some(peer) = peer_id {
                    warn!("Outgoing connection to {} failed: {}", peer, error);
                } else {
                    warn!("Outgoing connection failed: {}", error);
                }
            }
            SwarmEvent::IncomingConnectionError { send_back_addr, error, .. } => {
                warn!("Incoming connection from {} failed: {}", send_back_addr, error);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_node_creation() {
        let config = Config::default();
        let node = Node::new(config).unwrap();
        assert!(!node.peer_id().to_string().is_empty());
    }

    #[tokio::test]
    async fn test_node_listen() {
        let config = Config::default();
        let mut node = Node::new(config.clone()).unwrap();
        node.listen(&config).await.unwrap();
    }
}
