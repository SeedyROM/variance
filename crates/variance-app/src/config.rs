use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

/// Resolve the platform-specific data directory for this Variance instance.
///
/// Precedence:
/// 1. `VARIANCE_DATA_DIR` environment variable (for multi-instance testing)
/// 2. Debug:   `<local_data_dir>/variance-dev`
/// 3. Release: `<local_data_dir>/variance`
pub fn variance_data_dir() -> PathBuf {
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

/// Application configuration persisted to `config.toml`.
///
/// Only user-editable settings belong here; storage paths are derived at
/// runtime from `StorageConfig::base_dir` and never serialized to the file.
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

    /// Storage settings (only `group_message_max_age_days` is persisted).
    ///
    /// Machine-specific paths (`base_dir`, `identity_path`, etc.) are derived
    /// at runtime and excluded from serialization so the config file stays
    /// portable across machines and instances.
    #[serde(default)]
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
    /// Base directory for all storage.
    /// Always derived at runtime; never persisted to config.toml.
    #[serde(skip)]
    pub base_dir: PathBuf,

    /// Local identity file (DID + signing keys).
    /// Always derived at runtime; never persisted to config.toml.
    #[serde(skip)]
    pub identity_path: PathBuf,

    /// Identity cache directory.
    /// Always derived at runtime; never persisted to config.toml.
    #[serde(skip)]
    pub identity_cache_dir: PathBuf,

    /// Message database path.
    /// Always derived at runtime; never persisted to config.toml.
    #[serde(skip)]
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

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            base_dir: PathBuf::new(),
            identity_path: PathBuf::new(),
            identity_cache_dir: PathBuf::new(),
            message_db_path: PathBuf::new(),
            group_message_max_age_days: DEFAULT_GROUP_MESSAGE_MAX_AGE_DAYS,
        }
    }
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
            storage: StorageConfig::for_base_dir(variance_data_dir()),
        }
    }
}

impl StorageConfig {
    /// Derive all storage paths from a base directory.
    pub fn for_base_dir(base_dir: PathBuf) -> Self {
        Self {
            identity_path: base_dir.join("identity.json"),
            identity_cache_dir: base_dir.join("identity_cache"),
            message_db_path: base_dir.join("messages.db"),
            base_dir,
            group_message_max_age_days: DEFAULT_GROUP_MESSAGE_MAX_AGE_DAYS,
        }
    }
}

impl AppConfig {
    /// Load configuration from TOML file, deriving storage paths from `base_dir`.
    ///
    /// Storage paths are never read from the TOML — they are always computed
    /// from `base_dir` so the file stays portable.
    pub fn from_file(path: &str, base_dir: PathBuf) -> anyhow::Result<Self> {
        let contents = fs::read_to_string(path)?;
        let mut config: AppConfig = toml::from_str(&contents)?;
        let max_age = config.storage.group_message_max_age_days;
        config.storage = StorageConfig::for_base_dir(base_dir);
        config.storage.group_message_max_age_days = max_age;
        Ok(config)
    }

    /// Save configuration to TOML file.
    pub fn to_file(&self, path: &str) -> anyhow::Result<()> {
        let contents = toml::to_string_pretty(self)?;
        fs::write(path, contents)?;
        Ok(())
    }

    /// Try to load `base_dir/config.toml`; fall back to [`AppConfig::default()`]
    /// if absent or unparseable.
    ///
    /// Storage paths are derived from `base_dir` regardless of what the file
    /// contains (they are `#[serde(skip)]` and never persisted).
    pub fn load_or_default(base_dir: &Path) -> Self {
        let config_path = base_dir.join("config.toml");
        let mut config = match fs::read_to_string(&config_path) {
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
        };
        let max_age = config.storage.group_message_max_age_days;
        config.storage = StorageConfig::for_base_dir(base_dir.to_path_buf());
        config.storage.group_message_max_age_days = max_age;
        config
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
        let dir = tempfile::tempdir().unwrap();
        let base_dir = dir.path();
        let mut config = AppConfig::default();
        config.storage = StorageConfig::for_base_dir(base_dir.to_path_buf());

        // Save and reload
        config.save(base_dir).unwrap();
        let parsed = AppConfig::load_or_default(base_dir);

        assert_eq!(config.server.port, parsed.server.port);
        assert_eq!(config.identity.ipfs_api, parsed.identity.ipfs_api);
        // Storage paths are derived from base_dir, not from the file
        assert_eq!(parsed.storage.base_dir, base_dir);
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

    #[test]
    fn test_relay_save_load_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let base_dir = dir.path();

        // Save defaults (no relays)
        let mut default_cfg = AppConfig::default();
        default_cfg.storage = StorageConfig::for_base_dir(base_dir.to_path_buf());
        default_cfg.save(base_dir).unwrap();

        // Load, add relay, save (simulates API handler)
        let mut config = AppConfig::load_or_default(base_dir);
        config.p2p.relay_peers.push(RelayPeerConfig {
            peer_id: "12D3KooWTest".to_string(),
            multiaddr: "/ip4/1.2.3.4/tcp/4001".to_string(),
        });
        config.save(base_dir).unwrap();

        // Reload and verify relay persisted
        let reloaded = AppConfig::load_or_default(base_dir);
        assert_eq!(reloaded.p2p.relay_peers.len(), 1);
        assert_eq!(reloaded.p2p.relay_peers[0].peer_id, "12D3KooWTest");
        assert_eq!(
            reloaded.p2p.relay_peers[0].multiaddr,
            "/ip4/1.2.3.4/tcp/4001"
        );
    }

    #[test]
    fn test_storage_paths_not_serialized() {
        let dir = tempfile::tempdir().unwrap();
        let base_dir = dir.path();

        let mut config = AppConfig::default();
        config.storage = StorageConfig::for_base_dir(base_dir.to_path_buf());
        config.save(base_dir).unwrap();

        let contents = fs::read_to_string(base_dir.join("config.toml")).unwrap();
        // Machine-specific paths should never appear in the TOML
        assert!(
            !contents.contains("base_dir"),
            "base_dir should not be in config.toml"
        );
        assert!(
            !contents.contains("identity_path"),
            "identity_path should not be in config.toml"
        );
        assert!(
            !contents.contains("identity_cache_dir"),
            "identity_cache_dir should not be in config.toml"
        );
        assert!(
            !contents.contains("message_db_path"),
            "message_db_path should not be in config.toml"
        );
        // But user-editable settings should be there
        assert!(contents.contains("group_message_max_age_days"));
    }

    #[test]
    fn test_storage_paths_derived_from_base_dir() {
        let dir = tempfile::tempdir().unwrap();
        let base_dir = dir.path();

        // Write a config with no storage section at all
        let toml = r#"
[server]
host = "127.0.0.1"
port = 3000

[p2p]
listen_addrs = ["/ip4/0.0.0.0/tcp/0"]
bootstrap_peers = []

[[p2p.relay_peers]]
peer_id = "12D3KooWRelay"
multiaddr = "/ip4/10.0.0.1/tcp/4001"

[identity]
ipfs_api = "http://127.0.0.1:5001"
cache_ttl_secs = 3600

[media]
stun_servers = []
turn_servers = []
"#;
        fs::write(base_dir.join("config.toml"), toml).unwrap();

        let config = AppConfig::load_or_default(base_dir);
        assert_eq!(config.storage.base_dir, base_dir);
        assert_eq!(config.storage.identity_path, base_dir.join("identity.json"));
        assert_eq!(
            config.storage.identity_cache_dir,
            base_dir.join("identity_cache")
        );
        assert_eq!(config.storage.message_db_path, base_dir.join("messages.db"));
        // Relay from file should be loaded
        assert_eq!(config.p2p.relay_peers.len(), 1);
        assert_eq!(config.p2p.relay_peers[0].peer_id, "12D3KooWRelay");
    }

    #[test]
    fn test_retention_persisted_in_storage_section() {
        let dir = tempfile::tempdir().unwrap();
        let base_dir = dir.path();

        // Save config with custom retention
        let mut config = AppConfig::default();
        config.storage = StorageConfig::for_base_dir(base_dir.to_path_buf());
        config.storage.group_message_max_age_days = 90;
        config.save(base_dir).unwrap();

        // Reload and verify
        let reloaded = AppConfig::load_or_default(base_dir);
        assert_eq!(reloaded.storage.group_message_max_age_days, 90);
    }

    #[test]
    fn test_load_legacy_config_with_storage_paths() {
        // Legacy config.toml files may have storage paths serialized.
        // They should be ignored in favor of the runtime base_dir.
        let dir = tempfile::tempdir().unwrap();
        let base_dir = dir.path();

        let toml = r#"
[server]
host = "127.0.0.1"
port = 3000

[p2p]
listen_addrs = ["/ip4/0.0.0.0/tcp/0"]
bootstrap_peers = []
relay_peers = []

[identity]
ipfs_api = "http://127.0.0.1:5001"
cache_ttl_secs = 3600

[media]
stun_servers = []
turn_servers = []

[storage]
base_dir = "/some/old/machine/path"
identity_path = "/some/old/machine/path/identity.json"
identity_cache_dir = "/some/old/machine/path/identity_cache"
message_db_path = "/some/old/machine/path/messages.db"
group_message_max_age_days = 14
"#;
        fs::write(base_dir.join("config.toml"), toml).unwrap();

        let config = AppConfig::load_or_default(base_dir);
        // Paths should come from base_dir, NOT from the file
        assert_eq!(config.storage.base_dir, base_dir);
        assert_eq!(config.storage.identity_path, base_dir.join("identity.json"));
        // But retention should be from the file
        assert_eq!(config.storage.group_message_max_age_days, 14);
    }
}
