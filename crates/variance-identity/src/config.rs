use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Identity system configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdentityConfig {
    /// Storage backend to use
    pub storage: StorageConfig,

    /// Cache configuration
    pub cache: CacheConfig,
}

/// Storage backend configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "backend", rename_all = "lowercase")]
pub enum StorageConfig {
    /// Local storage using sled (for development/testing)
    Local {
        /// Path to sled database
        db_path: PathBuf,
    },

    /// IPFS storage (for production)
    #[allow(dead_code)]
    Ipfs {
        /// IPFS API endpoint
        api_url: String,
        /// IPFS gateway URL (optional)
        gateway_url: Option<String>,
    },
}

/// Cache configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheConfig {
    /// Path to disk cache (L3)
    pub disk_cache_path: PathBuf,

    /// L1 cache TTL in seconds (default: 5 minutes)
    #[serde(default)]
    pub l1_ttl_seconds: u64,

    /// L2 cache TTL in seconds (default: 1 hour)
    #[serde(default)]
    pub l2_ttl_seconds: u64,

    /// L3 cache TTL in seconds (default: 24 hours)
    #[serde(default)]
    pub l3_ttl_seconds: u64,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            disk_cache_path: PathBuf::from("~/.variance/identity/cache"),
            l1_ttl_seconds: 5 * 60,       // 5 minutes
            l2_ttl_seconds: 60 * 60,      // 1 hour
            l3_ttl_seconds: 24 * 60 * 60, // 24 hours
        }
    }
}

impl Default for IdentityConfig {
    fn default() -> Self {
        Self {
            storage: StorageConfig::Local {
                db_path: PathBuf::from("~/.variance/identity/storage"),
            },
            cache: CacheConfig::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = IdentityConfig::default();
        assert!(matches!(config.storage, StorageConfig::Local { .. }));
        assert_eq!(config.cache.l1_ttl_seconds, 300);
        assert_eq!(config.cache.l2_ttl_seconds, 3600);
        assert_eq!(config.cache.l3_ttl_seconds, 86400);
    }

    #[test]
    fn test_serialize_local_config() {
        let config = IdentityConfig {
            storage: StorageConfig::Local {
                db_path: PathBuf::from("/tmp/test"),
            },
            cache: CacheConfig {
                disk_cache_path: PathBuf::from("/tmp/cache"),
                l1_ttl_seconds: 300,
                l2_ttl_seconds: 3600,
                l3_ttl_seconds: 86400,
            },
        };

        let json = serde_json::to_string_pretty(&config).unwrap();
        assert!(json.contains("\"backend\": \"local\""));
        assert!(json.contains("/tmp/test"));
    }

    #[test]
    fn test_deserialize_local_config() {
        let json = r#"
        {
            "storage": {
                "backend": "local",
                "db_path": "/tmp/test"
            },
            "cache": {
                "disk_cache_path": "/tmp/cache",
                "l1_ttl_seconds": 300,
                "l2_ttl_seconds": 3600,
                "l3_ttl_seconds": 86400
            }
        }
        "#;

        let config: IdentityConfig = serde_json::from_str(json).unwrap();
        assert!(matches!(config.storage, StorageConfig::Local { .. }));
    }
}
