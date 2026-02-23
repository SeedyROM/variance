use libp2p::{gossipsub, identify, kad, mdns, ping, request_response, swarm::NetworkBehaviour};

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

    /// Custom protocol: Identity resolution
    pub identity: request_response::Behaviour<crate::protocols::identity::IdentityCodec>,

    /// Custom protocol: Offline message relay
    pub offline_messages:
        request_response::Behaviour<crate::protocols::messaging::OfflineMessageCodec>,

    /// Custom protocol: WebRTC signaling
    pub signaling: request_response::Behaviour<crate::protocols::media::SignalingCodec>,

    /// Custom protocol: Direct messages (Double Ratchet encrypted)
    pub direct_messages:
        request_response::Behaviour<crate::protocols::messaging::DirectMessageCodec>,

    /// Custom protocol: Typing indicators (fire-and-forget)
    pub typing_indicators:
        request_response::Behaviour<crate::protocols::messaging::TypingIndicatorCodec>,
}
