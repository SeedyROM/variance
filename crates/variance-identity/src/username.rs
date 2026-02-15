use crate::error::*;
use dashmap::DashMap;

/// Username registry using provider records (NOT DHT values)
///
/// Architecture:
/// - DHT provider records map username -> peer providing that username
/// - Custom libp2p protocol queries peers directly for DID
/// - Local cache prevents repeated network lookups
pub struct UsernameRegistry {
    /// Local username -> DID mapping cache
    local_cache: DashMap<String, String>,
    /// Reverse mapping: DID -> username
    reverse_cache: DashMap<String, String>,
}

impl UsernameRegistry {
    pub fn new() -> Self {
        Self {
            local_cache: DashMap::new(),
            reverse_cache: DashMap::new(),
        }
    }

    /// Register a username locally (provider record published separately via DHT)
    pub fn register_local(&self, username: String, did: String) -> Result<()> {
        // Check if username already taken
        if self.local_cache.contains_key(&username) {
            return Err(Error::UsernameTaken {
                username: username.clone(),
            });
        }

        self.local_cache.insert(username.clone(), did.clone());
        self.reverse_cache.insert(did, username);
        Ok(())
    }

    /// Lookup username in local cache
    pub fn lookup_local(&self, username: &str) -> Option<String> {
        self.local_cache.get(username).map(|v| v.clone())
    }

    /// Get username for a DID
    pub fn get_username(&self, did: &str) -> Option<String> {
        self.reverse_cache.get(did).map(|v| v.clone())
    }

    /// Cache a username -> DID mapping from network lookup
    pub fn cache_mapping(&self, username: String, did: String) {
        self.local_cache.insert(username.clone(), did.clone());
        self.reverse_cache.insert(did, username);
    }

    /// Validate username format
    pub fn validate_username(username: &str) -> Result<()> {
        if username.is_empty() {
            return Err(Error::InvalidDid {
                did: "Username cannot be empty".to_string(),
            });
        }

        if username.len() > 32 {
            return Err(Error::InvalidDid {
                did: "Username too long (max 32 chars)".to_string(),
            });
        }

        // Must be alphanumeric with underscores/hyphens
        if !username
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
        {
            return Err(Error::InvalidDid {
                did: "Username must be alphanumeric with underscores/hyphens".to_string(),
            });
        }

        // Must start with alphanumeric
        if !username.chars().next().unwrap().is_alphanumeric() {
            return Err(Error::InvalidDid {
                did: "Username must start with alphanumeric character".to_string(),
            });
        }

        Ok(())
    }
}

impl Default for UsernameRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_username_registration() {
        let registry = UsernameRegistry::new();
        let did = "did:peer:12D3KooWtest".to_string();

        registry
            .register_local("alice".to_string(), did.clone())
            .unwrap();

        assert_eq!(registry.lookup_local("alice"), Some(did.clone()));
        assert_eq!(registry.get_username(&did), Some("alice".to_string()));
    }

    #[test]
    fn test_duplicate_username() {
        let registry = UsernameRegistry::new();
        let did1 = "did:peer:12D3KooWtest1".to_string();
        let did2 = "did:peer:12D3KooWtest2".to_string();

        registry.register_local("alice".to_string(), did1).unwrap();

        let result = registry.register_local("alice".to_string(), did2);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), Error::UsernameTaken { .. }));
    }

    #[test]
    fn test_username_validation() {
        assert!(UsernameRegistry::validate_username("alice").is_ok());
        assert!(UsernameRegistry::validate_username("alice_123").is_ok());
        assert!(UsernameRegistry::validate_username("alice-bob").is_ok());

        assert!(UsernameRegistry::validate_username("").is_err());
        assert!(UsernameRegistry::validate_username("a".repeat(33).as_str()).is_err());
        assert!(UsernameRegistry::validate_username("alice@bob").is_err());
        assert!(UsernameRegistry::validate_username("_alice").is_err());
        assert!(UsernameRegistry::validate_username("-alice").is_err());
    }

    #[test]
    fn test_cache_mapping() {
        let registry = UsernameRegistry::new();
        let did = "did:peer:12D3KooWtest".to_string();

        registry.cache_mapping("bob".to_string(), did.clone());

        assert_eq!(registry.lookup_local("bob"), Some(did.clone()));
        assert_eq!(registry.get_username(&did), Some("bob".to_string()));
    }
}
