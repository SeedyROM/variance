use libp2p::{
    gossipsub, identify, kad, mdns, ping,
    swarm::NetworkBehaviour,
};

/// Combined network behaviour for Variance P2P
#[derive(NetworkBehaviour)]
pub struct VarianceBehaviour {
    /// Kademlia DHT for peer/content routing
    pub kad: kad::Behaviour<kad::store::MemoryStore>,

    /// GossipSub for pub/sub messaging
    pub gossipsub: gossipsub::Behaviour,

    /// mDNS for local peer discovery
    pub mdns: mdns::tokio::Behaviour,

    /// Identify protocol for peer information exchange
    pub identify: identify::Behaviour,

    /// Ping for connection keep-alive
    pub ping: ping::Behaviour,
}
