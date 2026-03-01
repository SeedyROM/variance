use crate::error::*;
use dashmap::DashMap;
use rand::Rng as _;

/// Maximum discriminator value (4-digit, 0001–9999)
pub const MAX_DISCRIMINATOR: u32 = 9999;
/// Minimum discriminator value
pub const MIN_DISCRIMINATOR: u32 = 1;
/// Minimum username length
pub const MIN_USERNAME_LENGTH: usize = 3;
/// Maximum username length
pub const MAX_USERNAME_LENGTH: usize = 32;
/// Maximum random attempts to find an available discriminator
const MAX_DISCRIMINATOR_ATTEMPTS: usize = 100;

/// Username registry using DHT provider records
///
/// Architecture:
/// - DHT provider records map username -> peer providing that username
/// - Custom libp2p protocol queries peers directly for DID
/// - Local cache prevents repeated network lookups
///
/// Usernames follow Discord-style `name#0001` format with 4-digit
/// zero-padded discriminators (0001–9999).
pub struct UsernameRegistry {
    /// Compound key `name#disc` → DID
    local_cache: DashMap<String, String>,
    /// DID → (username, discriminator)
    reverse_cache: DashMap<String, (String, u32)>,
}

impl UsernameRegistry {
    pub fn new() -> Self {
        Self {
            local_cache: DashMap::new(),
            reverse_cache: DashMap::new(),
        }
    }

    /// Format a username with its discriminator: `name#0001`
    pub fn format_username(name: &str, discriminator: u32) -> String {
        format!("{name}#{discriminator:04}")
    }

    /// Parse a formatted username string into (name, discriminator).
    ///
    /// Accepts `name#0001` or `@name#0001`.
    pub fn parse_username(formatted: &str) -> Option<(String, u32)> {
        let s = formatted.strip_prefix('@').unwrap_or(formatted);
        let (name, disc_str) = s.rsplit_once('#')?;
        let disc = disc_str.parse::<u32>().ok()?;
        if !(MIN_DISCRIMINATOR..=MAX_DISCRIMINATOR).contains(&disc) {
            return None;
        }
        Some((name.to_string(), disc))
    }

    /// Compound key for the local cache
    fn compound_key(name: &str, discriminator: u32) -> String {
        format!("{name}#{discriminator}")
    }

    /// Register a username locally, auto-assigning a random discriminator.
    ///
    /// Tries up to 100 random discriminators (1–9999) to find one that isn't
    /// already taken for this name. Returns `(name, discriminator)` on success.
    pub fn register_local(&self, username: String, did: String) -> Result<(String, u32)> {
        Self::validate_username(&username)?;
        let name = username.to_lowercase();

        // If this DID already has a username, remove it so we can replace it.
        if let Some((_, (old_name, old_disc))) = self.reverse_cache.remove(&did) {
            let old_key = Self::compound_key(&old_name, old_disc);
            self.local_cache.remove(&old_key);
        }

        let mut rng = rand::thread_rng();
        for _ in 0..MAX_DISCRIMINATOR_ATTEMPTS {
            let disc = rng.gen_range(MIN_DISCRIMINATOR..=MAX_DISCRIMINATOR);
            let key = Self::compound_key(&name, disc);
            if !self.local_cache.contains_key(&key) {
                self.local_cache.insert(key, did.clone());
                self.reverse_cache.insert(did, (name.clone(), disc));
                return Ok((name, disc));
            }
        }

        Err(Error::UsernameTaken {
            username: format!(
                "{name} (no available discriminators after {MAX_DISCRIMINATOR_ATTEMPTS} attempts)"
            ),
        })
    }

    /// Register with a specific discriminator (used when restoring from disk or
    /// caching a network result).
    pub fn register_with_discriminator(
        &self,
        username: String,
        discriminator: u32,
        did: String,
    ) -> Result<()> {
        let name = username.to_lowercase();
        let key = Self::compound_key(&name, discriminator);
        if self.local_cache.contains_key(&key) {
            return Err(Error::UsernameTaken { username: key });
        }
        self.local_cache.insert(key, did.clone());
        self.reverse_cache.insert(did, (name, discriminator));
        Ok(())
    }

    /// Lookup a specific name#discriminator pair → DID
    pub fn lookup_exact(&self, username: &str, discriminator: u32) -> Option<String> {
        let key = Self::compound_key(&username.to_lowercase(), discriminator);
        self.local_cache.get(&key).map(|v| v.clone())
    }

    /// Lookup all discriminators for a base username.
    ///
    /// Returns `Vec<(discriminator, did)>` for all known holders of this name.
    pub fn lookup_all(&self, username: &str) -> Vec<(u32, String)> {
        let name = username.to_lowercase();
        let prefix = format!("{name}#");
        self.local_cache
            .iter()
            .filter_map(|entry| {
                let key = entry.key();
                if let Some(disc_str) = key.strip_prefix(&prefix) {
                    let disc = disc_str.parse::<u32>().ok()?;
                    Some((disc, entry.value().clone()))
                } else {
                    None
                }
            })
            .collect()
    }

    /// Get the formatted display name for a DID, e.g. `alice#0042`
    pub fn get_display_name(&self, did: &str) -> Option<String> {
        self.reverse_cache
            .get(did)
            .map(|v| Self::format_username(&v.0, v.1))
    }

    /// Get the (username, discriminator) tuple for a DID
    pub fn get_username(&self, did: &str) -> Option<(String, u32)> {
        self.reverse_cache.get(did).map(|v| v.clone())
    }

    /// Cache a username → DID mapping from network lookup.
    ///
    /// Evicts any stale entry for the DID before inserting the new one so that
    /// both local and remote rename paths stay consistent.
    pub fn cache_mapping(&self, username: String, discriminator: u32, did: String) {
        let name = username.to_lowercase();
        // Evict old entry if this DID already has a cached name
        if let Some((_, (old_name, old_disc))) = self.reverse_cache.remove(&did) {
            self.local_cache
                .remove(&Self::compound_key(&old_name, old_disc));
        }
        let key = Self::compound_key(&name, discriminator);
        self.local_cache.insert(key, did.clone());
        self.reverse_cache.insert(did, (name, discriminator));
    }

    /// Validate username format
    pub fn validate_username(username: &str) -> Result<()> {
        if username.is_empty() {
            return Err(Error::InvalidDid {
                did: "Username cannot be empty".to_string(),
            });
        }

        if username.len() < MIN_USERNAME_LENGTH {
            return Err(Error::InvalidDid {
                did: format!("Username too short (min {} chars)", MIN_USERNAME_LENGTH),
            });
        }

        if username.len() > MAX_USERNAME_LENGTH {
            return Err(Error::InvalidDid {
                did: format!("Username too long (max {} chars)", MAX_USERNAME_LENGTH),
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
    fn test_username_registration_with_discriminator() {
        let registry = UsernameRegistry::new();
        let did = "did:peer:12D3KooWtest".to_string();

        let (name, disc) = registry
            .register_local("alice".to_string(), did.clone())
            .unwrap();

        assert_eq!(name, "alice");
        assert!((MIN_DISCRIMINATOR..=MAX_DISCRIMINATOR).contains(&disc));

        // Should be findable by exact lookup
        assert_eq!(registry.lookup_exact("alice", disc), Some(did.clone()));

        // Should be findable by DID
        let (found_name, found_disc) = registry.get_username(&did).unwrap();
        assert_eq!(found_name, "alice");
        assert_eq!(found_disc, disc);

        // Display name should be formatted
        let display = registry.get_display_name(&did).unwrap();
        assert_eq!(display, format!("alice#{disc:04}"));
    }

    #[test]
    fn test_did_can_change_username() {
        let registry = UsernameRegistry::new();
        let did = "did:peer:12D3KooWtest".to_string();

        let (_, disc1) = registry
            .register_local("alice".to_string(), did.clone())
            .unwrap();

        // Changing username: old entry should be evicted, new one registered.
        let (new_name, disc2) = registry
            .register_local("bob".to_string(), did.clone())
            .unwrap();
        assert_eq!(new_name, "bob");

        // Old username no longer maps to this DID.
        assert_eq!(registry.lookup_exact("alice", disc1), None);
        // New username resolves correctly.
        assert_eq!(registry.lookup_exact("bob", disc2), Some(did));
    }

    #[test]
    fn test_same_name_different_discriminators() {
        let registry = UsernameRegistry::new();
        let did1 = "did:peer:12D3KooWtest1".to_string();
        let did2 = "did:peer:12D3KooWtest2".to_string();

        let (_, disc1) = registry
            .register_local("alice".to_string(), did1.clone())
            .unwrap();
        let (_, disc2) = registry
            .register_local("alice".to_string(), did2.clone())
            .unwrap();

        // Different discriminators
        assert_ne!(disc1, disc2);

        // Both findable
        assert_eq!(registry.lookup_exact("alice", disc1), Some(did1));
        assert_eq!(registry.lookup_exact("alice", disc2), Some(did2));

        // lookup_all returns both
        let all = registry.lookup_all("alice");
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn test_register_with_specific_discriminator() {
        let registry = UsernameRegistry::new();
        let did = "did:peer:12D3KooWtest".to_string();

        registry
            .register_with_discriminator("alice".to_string(), 42, did.clone())
            .unwrap();

        assert_eq!(registry.lookup_exact("alice", 42), Some(did.clone()));
        assert_eq!(
            registry.get_display_name(&did),
            Some("alice#0042".to_string())
        );
    }

    #[test]
    fn test_format_and_parse_username() {
        assert_eq!(UsernameRegistry::format_username("alice", 1), "alice#0001");
        assert_eq!(
            UsernameRegistry::format_username("alice", 9999),
            "alice#9999"
        );
        assert_eq!(UsernameRegistry::format_username("alice", 42), "alice#0042");

        assert_eq!(
            UsernameRegistry::parse_username("alice#0001"),
            Some(("alice".to_string(), 1))
        );
        assert_eq!(
            UsernameRegistry::parse_username("@alice#0042"),
            Some(("alice".to_string(), 42))
        );
        assert_eq!(UsernameRegistry::parse_username("alice#0000"), None);
        assert_eq!(UsernameRegistry::parse_username("alice#10000"), None);
        assert_eq!(UsernameRegistry::parse_username("invalid"), None);
    }

    #[test]
    fn test_username_validation() {
        assert!(UsernameRegistry::validate_username("alice").is_ok());
        assert!(UsernameRegistry::validate_username("alice_123").is_ok());
        assert!(UsernameRegistry::validate_username("alice-bob").is_ok());
        assert!(UsernameRegistry::validate_username("abc").is_ok()); // min length 3

        assert!(UsernameRegistry::validate_username("").is_err());
        assert!(UsernameRegistry::validate_username("ab").is_err()); // too short
        assert!(UsernameRegistry::validate_username("a".repeat(33).as_str()).is_err());
        assert!(UsernameRegistry::validate_username("alice@bob").is_err());
        assert!(UsernameRegistry::validate_username("_alice").is_err());
        assert!(UsernameRegistry::validate_username("-alice").is_err());
    }

    #[test]
    fn test_cache_mapping() {
        let registry = UsernameRegistry::new();
        let did = "did:peer:12D3KooWtest".to_string();

        registry.cache_mapping("bob".to_string(), 1234, did.clone());

        assert_eq!(registry.lookup_exact("bob", 1234), Some(did.clone()));
        assert_eq!(
            registry.get_display_name(&did),
            Some("bob#1234".to_string())
        );
    }

    #[test]
    fn test_cache_mapping_evicts_stale_entry_on_rename() {
        let registry = UsernameRegistry::new();
        let did = "did:peer:12D3KooWtest".to_string();

        // Initial name
        registry.cache_mapping("alice".to_string(), 42, did.clone());
        assert_eq!(registry.lookup_exact("alice", 42), Some(did.clone()));

        // Peer renames — old entry must be evicted
        registry.cache_mapping("bob".to_string(), 99, did.clone());

        // Old mapping is gone
        assert_eq!(registry.lookup_exact("alice", 42), None);
        // New mapping is present
        assert_eq!(registry.lookup_exact("bob", 99), Some(did.clone()));
        assert_eq!(
            registry.get_display_name(&did),
            Some("bob#0099".to_string())
        );
    }

    #[test]
    fn test_case_insensitive_lookup() {
        let registry = UsernameRegistry::new();
        let did = "did:peer:12D3KooWtest".to_string();

        registry.cache_mapping("Alice".to_string(), 42, did.clone());

        // Lookup is case-insensitive because we normalize to lowercase
        assert_eq!(registry.lookup_exact("alice", 42), Some(did.clone()));
        assert_eq!(registry.lookup_exact("ALICE", 42), Some(did));
    }
}
