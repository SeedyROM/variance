use serde::{Deserialize, Serialize};
use variance_proto::media_proto::{IceServer, StunturnConfig};

/// STUN/TURN server configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaConfig {
    /// ICE servers for WebRTC
    pub ice_servers: Vec<IceServerConfig>,

    /// ICE transport policy (0 = all, 1 = relay only)
    pub ice_transport_policy: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IceServerConfig {
    /// Server URLs (e.g., "stun:stun.l.google.com:19302")
    pub urls: Vec<String>,

    /// Optional username for TURN servers
    pub username: Option<String>,

    /// Optional credential for TURN servers
    pub credential: Option<String>,
}

impl Default for MediaConfig {
    fn default() -> Self {
        Self {
            ice_servers: vec![
                // Google's public STUN servers
                IceServerConfig {
                    urls: vec!["stun:stun.l.google.com:19302".to_string()],
                    username: None,
                    credential: None,
                },
                IceServerConfig {
                    urls: vec!["stun:stun1.l.google.com:19302".to_string()],
                    username: None,
                    credential: None,
                },
            ],
            ice_transport_policy: 0, // Use all candidates
        }
    }
}

impl MediaConfig {
    /// Create a new media configuration
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a STUN server
    pub fn add_stun_server(&mut self, url: String) {
        self.ice_servers.push(IceServerConfig {
            urls: vec![url],
            username: None,
            credential: None,
        });
    }

    /// Add a TURN server with credentials
    pub fn add_turn_server(&mut self, url: String, username: String, credential: String) {
        self.ice_servers.push(IceServerConfig {
            urls: vec![url],
            username: Some(username),
            credential: Some(credential),
        });
    }

    /// Set relay-only mode (force TURN)
    pub fn set_relay_only(&mut self) {
        self.ice_transport_policy = 1;
    }

    /// Convert to protobuf format
    pub fn to_proto(&self) -> StunturnConfig {
        StunturnConfig {
            ice_servers: self
                .ice_servers
                .iter()
                .map(|server| IceServer {
                    urls: server.urls.clone(),
                    username: server.username.clone(),
                    credential: server.credential.clone(),
                })
                .collect(),
            ice_transport_policy: Some(self.ice_transport_policy),
        }
    }

    /// Create from protobuf format
    pub fn from_proto(proto: &StunturnConfig) -> Self {
        Self {
            ice_servers: proto
                .ice_servers
                .iter()
                .map(|server| IceServerConfig {
                    urls: server.urls.clone(),
                    username: server.username.clone(),
                    credential: server.credential.clone(),
                })
                .collect(),
            ice_transport_policy: proto.ice_transport_policy.unwrap_or(0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = MediaConfig::default();

        assert_eq!(config.ice_servers.len(), 2);
        assert_eq!(
            config.ice_servers[0].urls[0],
            "stun:stun.l.google.com:19302"
        );
        assert_eq!(config.ice_transport_policy, 0);
    }

    #[test]
    fn test_new_config() {
        let config = MediaConfig::new();

        assert_eq!(config.ice_servers.len(), 2);
    }

    #[test]
    fn test_add_stun_server() {
        let mut config = MediaConfig::new();
        config.add_stun_server("stun:stun.example.com:3478".to_string());

        assert_eq!(config.ice_servers.len(), 3);
        assert_eq!(
            config.ice_servers[2].urls[0],
            "stun:stun.example.com:3478"
        );
        assert!(config.ice_servers[2].username.is_none());
    }

    #[test]
    fn test_add_turn_server() {
        let mut config = MediaConfig::new();
        config.add_turn_server(
            "turn:turn.example.com:3478".to_string(),
            "user".to_string(),
            "pass".to_string(),
        );

        assert_eq!(config.ice_servers.len(), 3);
        assert_eq!(
            config.ice_servers[2].urls[0],
            "turn:turn.example.com:3478"
        );
        assert_eq!(config.ice_servers[2].username, Some("user".to_string()));
        assert_eq!(config.ice_servers[2].credential, Some("pass".to_string()));
    }

    #[test]
    fn test_set_relay_only() {
        let mut config = MediaConfig::new();
        config.set_relay_only();

        assert_eq!(config.ice_transport_policy, 1);
    }

    #[test]
    fn test_proto_conversion() {
        let config = MediaConfig::new();
        let proto = config.to_proto();

        assert_eq!(proto.ice_servers.len(), 2);
        assert_eq!(proto.ice_transport_policy, Some(0));

        let restored = MediaConfig::from_proto(&proto);
        assert_eq!(restored.ice_servers.len(), 2);
        assert_eq!(restored.ice_transport_policy, 0);
    }

    #[test]
    fn test_proto_roundtrip() {
        let mut config = MediaConfig::new();
        config.add_turn_server(
            "turn:example.com:3478".to_string(),
            "testuser".to_string(),
            "testpass".to_string(),
        );
        config.set_relay_only();

        let proto = config.to_proto();
        let restored = MediaConfig::from_proto(&proto);

        assert_eq!(config.ice_servers.len(), restored.ice_servers.len());
        assert_eq!(
            config.ice_transport_policy,
            restored.ice_transport_policy
        );
        assert_eq!(
            config.ice_servers[2].username,
            restored.ice_servers[2].username
        );
    }
}
