use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

fn variance_data_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("VARIANCE_DATA_DIR") {
        return PathBuf::from(dir);
    }
    let dir_name = if cfg!(debug_assertions) {
        "variance-dev"
    } else {
        "variance"
    };
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(dir_name)
}

/// Application configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    /// HTTP server configuration
    pub server: ServerConfig,

    /// P2P networking configuration
    pub p2p: P2PConfig,

    /// Identity system configuration
    pub identity: IdentityConfig,

    /// Media configuration
    pub media: MediaConfig,

    /// Storage paths
    pub storage: StorageConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    /// Host to bind to
    pub host: String,

    /// Port to listen on
    pub port: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct P2PConfig {
    /// Listen addresses for libp2p
    pub listen_addrs: Vec<String>,

    /// Bootstrap peers (format: "peer_id@multiaddr")
    pub bootstrap_peers: Vec<String>,

    /// Relay peers for NAT traversal
    pub relay_peers: Vec<RelayPeerConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelayPeerConfig {
    pub peer_id: String,
    pub multiaddr: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdentityConfig {
    /// IPFS API endpoint
    pub ipfs_api: String,

    /// Cache TTL in seconds
    pub cache_ttl_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaConfig {
    /// STUN server URLs
    pub stun_servers: Vec<String>,

    /// TURN server configuration
    pub turn_servers: Vec<TurnServer>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnServer {
    pub url: String,
    pub username: String,
    pub credential: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageConfig {
    /// Base directory for all storage
    pub base_dir: PathBuf,

    /// Local identity file (DID + signing keys)
    pub identity_path: PathBuf,

    /// Identity cache directory
    pub identity_cache_dir: PathBuf,

    /// Message database path
    pub message_db_path: PathBuf,

    /// Maximum age in days for messages (direct and group) before they are purged.
    /// Cleanup runs hourly alongside expired offline message cleanup.
    /// Set to 0 to keep messages forever (cleanup is skipped entirely).
    #[serde(default = "default_group_message_max_age_days")]
    pub group_message_max_age_days: u64,
}

const DEFAULT_GROUP_MESSAGE_MAX_AGE_DAYS: u64 = 30;

fn default_group_message_max_age_days() -> u64 {
    DEFAULT_GROUP_MESSAGE_MAX_AGE_DAYS
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            server: ServerConfig {
                host: "127.0.0.1".to_string(),
                port: 3000,
            },
            p2p: P2PConfig {
                listen_addrs: vec!["/ip4/0.0.0.0/tcp/0".to_string()],
                bootstrap_peers: vec![],
                relay_peers: vec![],
            },
            identity: IdentityConfig {
                ipfs_api: "http://127.0.0.1:5001".to_string(),
                cache_ttl_secs: 3600,
            },
            // Default STUN servers are Google's public infrastructure.
            // These reveal your external IP to Google during call setup.
            // Replace with self-hosted or alternative STUN servers if needed.
            media: MediaConfig {
                stun_servers: vec![
                    "stun:stun.l.google.com:19302".to_string(),
                    "stun:stun1.l.google.com:19302".to_string(),
                ],
                turn_servers: vec![],
            },
            storage: StorageConfig {
                base_dir: variance_data_dir(),
                identity_path: variance_data_dir().join("identity.json"),
                identity_cache_dir: variance_data_dir().join("identity_cache"),
                message_db_path: variance_data_dir().join("messages.db"),
                group_message_max_age_days: DEFAULT_GROUP_MESSAGE_MAX_AGE_DAYS, // 30 days
            },
        }
    }
}

impl AppConfig {
    /// Load configuration from TOML file
    pub fn from_file(path: &str) -> anyhow::Result<Self> {
        let contents = fs::read_to_string(path)?;
        let config: AppConfig = toml::from_str(&contents)?;
        Ok(config)
    }

    /// Save configuration to TOML file
    pub fn to_file(&self, path: &str) -> anyhow::Result<()> {
        let contents = toml::to_string_pretty(self)?;
        fs::write(path, contents)?;
        Ok(())
    }

    /// Try to load `base_dir/config.toml`; fall back to [`AppConfig::default()`] if absent or unparseable.
    pub fn load_or_default(base_dir: &Path) -> Self {
        let config_path = base_dir.join("config.toml");
        match fs::read_to_string(&config_path) {
            Ok(contents) => match toml::from_str(&contents) {
                Ok(config) => config,
                Err(e) => {
                    tracing::warn!(
                        "Failed to parse {}: {}; using defaults",
                        config_path.display(),
                        e
                    );
                    AppConfig::default()
                }
            },
            Err(_) => AppConfig::default(),
        }
    }

    /// Write this config to `base_dir/config.toml`.
    pub fn save(&self, base_dir: &Path) -> anyhow::Result<()> {
        let config_path = base_dir.join("config.toml");
        let contents = toml::to_string_pretty(self)?;
        fs::write(config_path, contents)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = AppConfig::default();

        assert_eq!(config.server.host, "127.0.0.1");
        assert_eq!(config.server.port, 3000);
        assert_eq!(config.p2p.listen_addrs.len(), 1);
        assert_eq!(config.identity.ipfs_api, "http://127.0.0.1:5001");
        assert_eq!(config.media.stun_servers.len(), 2);
    }

    #[test]
    fn test_config_roundtrip() {
        let config = AppConfig::default();

        let toml_str = toml::to_string(&config).unwrap();
        let parsed: AppConfig = toml::from_str(&toml_str).unwrap();

        assert_eq!(config.server.port, parsed.server.port);
        assert_eq!(config.identity.ipfs_api, parsed.identity.ipfs_api);
    }

    #[test]
    fn test_custom_config() {
        let mut config = AppConfig::default();
        config.server.port = 8080;
        config.media.turn_servers.push(TurnServer {
            url: "turn:example.com:3478".to_string(),
            username: "user".to_string(),
            credential: "pass".to_string(),
        });

        assert_eq!(config.server.port, 8080);
        assert_eq!(config.media.turn_servers.len(), 1);
    }
}
