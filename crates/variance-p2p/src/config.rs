use libp2p::Multiaddr;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Pre-derived libp2p keypair for a stable PeerId across restarts.
    ///
    /// Derive this from the identity file's Ed25519 signing key so the node
    /// always advertises the same PeerId. When `None`, a fresh keypair is
    /// generated each run and the PeerId is ephemeral.
    #[serde(skip)]
    pub keypair: Option<libp2p::identity::Keypair>,

    /// Local listen addresses
    pub listen_addresses: Vec<Multiaddr>,

    /// Bootstrap peers for initial DHT connection
    #[serde(default)]
    pub bootstrap_peers: Vec<BootstrapPeer>,

    /// Enable mDNS for local peer discovery
    pub enable_mdns: bool,

    /// Kademlia DHT configuration
    #[serde(default)]
    pub kad_config: KadConfig,

    /// GossipSub configuration
    #[serde(default)]
    pub gossipsub_config: GossipsubConfig,

    /// Relay nodes for NAT traversal. After connecting, the node will
    /// reserve a circuit slot and be reachable at the relay's circuit address.
    #[serde(default)]
    pub relay_peers: Vec<BootstrapPeer>,

    /// Storage path for local data
    #[serde(default = "default_storage_path")]
    pub storage_path: PathBuf,

    /// How often to query the DHT for relay providers (seconds).
    /// 0 disables auto-discovery. Default: 300 (5 minutes).
    #[serde(default = "default_relay_discovery_interval_secs")]
    pub relay_discovery_interval_secs: u64,
}

fn default_relay_discovery_interval_secs() -> u64 {
    300
}

fn default_storage_path() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("variance")
}

impl Default for Config {
    fn default() -> Self {
        Self {
            keypair: None,
            listen_addresses: vec![
                "/ip4/0.0.0.0/tcp/0".parse().unwrap(),
                "/ip4/0.0.0.0/udp/0/quic-v1".parse().unwrap(),
            ],
            bootstrap_peers: Vec::new(),
            relay_peers: Vec::new(),
            enable_mdns: true,
            kad_config: KadConfig::default(),
            gossipsub_config: GossipsubConfig::default(),
            storage_path: default_storage_path(),
            relay_discovery_interval_secs: 300,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BootstrapPeer {
    pub peer_id: String,
    pub multiaddr: Multiaddr,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KadConfig {
    /// DHT replication factor
    pub replication_factor: usize,

    /// Provider record TTL in seconds
    pub provider_record_ttl: u64,
}

impl Default for KadConfig {
    fn default() -> Self {
        Self {
            replication_factor: 20,
            provider_record_ttl: 24 * 60 * 60, // 24 hours
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GossipsubConfig {
    /// Heartbeat interval in seconds
    pub heartbeat_interval_secs: u64,

    /// Message history length
    pub history_length: usize,

    /// History gossip length
    pub history_gossip: usize,
}

impl Default for GossipsubConfig {
    fn default() -> Self {
        Self {
            heartbeat_interval_secs: 1,
            history_length: 5,
            history_gossip: 3,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = Config::default();
        assert_eq!(config.listen_addresses.len(), 2);
        assert!(config.enable_mdns);
        assert_eq!(config.kad_config.replication_factor, 20);
    }

    #[test]
    fn test_serialize_deserialize() {
        let config = Config::default();
        let json = serde_json::to_string(&config).unwrap();
        let deserialized: Config = serde_json::from_str(&json).unwrap();
        assert_eq!(config.enable_mdns, deserialized.enable_mdns);
    }
}
