use crate::did::Did;
use crate::error::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::RwLock;
use std::time::{Duration, Instant};

/// Entry in a cache layer with expiration
#[derive(Clone)]
struct CacheEntry<T> {
    value: T,
    expires_at: Instant,
}

impl<T> CacheEntry<T> {
    fn new(value: T, ttl: Duration) -> Self {
        Self {
            value,
            expires_at: Instant::now() + ttl,
        }
    }

    fn is_expired(&self) -> bool {
        Instant::now() > self.expires_at
    }
}

/// L1: Hot cache (5 minutes)
struct L1Cache<K, V> {
    data: RwLock<HashMap<K, CacheEntry<V>>>,
    ttl: Duration,
}

impl<K: std::hash::Hash + Eq, V: Clone> L1Cache<K, V> {
    fn new(ttl: Duration) -> Self {
        Self {
            data: RwLock::new(HashMap::new()),
            ttl,
        }
    }

    fn get(&self, key: &K) -> Option<V> {
        let data = self.data.read().unwrap();
        if let Some(entry) = data.get(key) {
            if !entry.is_expired() {
                return Some(entry.value.clone());
            }
        }
        None
    }

    fn insert(&self, key: K, value: V) {
        let mut data = self.data.write().unwrap();
        data.insert(key, CacheEntry::new(value, self.ttl));
    }

    fn evict_expired(&self) {
        let mut data = self.data.write().unwrap();
        data.retain(|_, entry| !entry.is_expired());
    }
}

/// L2: Warm cache (1 hour)
struct L2Cache<K, V> {
    data: RwLock<HashMap<K, CacheEntry<V>>>,
    ttl: Duration,
}

impl<K: std::hash::Hash + Eq, V: Clone> L2Cache<K, V> {
    fn new(ttl: Duration) -> Self {
        Self {
            data: RwLock::new(HashMap::new()),
            ttl,
        }
    }

    fn get(&self, key: &K) -> Option<V> {
        let data = self.data.read().unwrap();
        if let Some(entry) = data.get(key) {
            if !entry.is_expired() {
                return Some(entry.value.clone());
            }
        }
        None
    }

    fn insert(&self, key: K, value: V) {
        let mut data = self.data.write().unwrap();
        data.insert(key, CacheEntry::new(value, self.ttl));
    }

    fn evict_expired(&self) {
        let mut data = self.data.write().unwrap();
        data.retain(|_, entry| !entry.is_expired());
    }
}

/// L3: Disk cache (24 hours) using sled
struct L3Cache {
    db: sled::Db,
    ttl: Duration,
}

#[derive(Serialize, Deserialize)]
struct DiskEntry {
    value: String,
    expires_at: u64,
}

impl L3Cache {
    fn new(path: &str, ttl: Duration) -> Result<Self> {
        let db = sled::open(path).map_err(|e| Error::Storage { source: e })?;
        Ok(Self { db, ttl })
    }

    fn get(&self, key: &str) -> Result<Option<Did>> {
        if let Some(data) = self
            .db
            .get(key.as_bytes())
            .map_err(|e| Error::Storage { source: e })?
        {
            let entry: DiskEntry =
                serde_json::from_slice(&data).map_err(|e| Error::Serialization { source: e })?;

            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs();

            if now < entry.expires_at {
                let did: Did = serde_json::from_str(&entry.value)
                    .map_err(|e| Error::Serialization { source: e })?;
                return Ok(Some(did));
            } else {
                // Expired, remove it
                self.db
                    .remove(key.as_bytes())
                    .map_err(|e| Error::Storage { source: e })?;
            }
        }
        Ok(None)
    }

    fn insert(&self, key: &str, value: &Did) -> Result<()> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let entry = DiskEntry {
            value: serde_json::to_string(value).map_err(|e| Error::Serialization { source: e })?,
            expires_at: now + self.ttl.as_secs(),
        };

        let data =
            serde_json::to_vec(&entry).map_err(|e| Error::Serialization { source: e })?;
        self.db
            .insert(key.as_bytes(), data)
            .map_err(|e| Error::Storage { source: e })?;

        Ok(())
    }
}

/// Multi-layer cache: L1 (hot) → L2 (warm) → L3 (disk)
pub struct MultiLayerCache {
    l1: L1Cache<String, Did>,
    l2: L2Cache<String, Did>,
    l3: L3Cache,
}

impl MultiLayerCache {
    pub fn new(cache_dir: &str) -> Result<Self> {
        Ok(Self {
            l1: L1Cache::new(Duration::from_secs(5 * 60)), // 5 minutes
            l2: L2Cache::new(Duration::from_secs(60 * 60)), // 1 hour
            l3: L3Cache::new(cache_dir, Duration::from_secs(24 * 60 * 60))?, // 24 hours
        })
    }

    /// Get a DID from cache, checking L1 → L2 → L3
    pub fn get(&self, key: &str) -> Result<Option<Did>> {
        // Try L1
        if let Some(did) = self.l1.get(&key.to_string()) {
            return Ok(Some(did));
        }

        // Try L2
        if let Some(did) = self.l2.get(&key.to_string()) {
            // Promote to L1
            self.l1.insert(key.to_string(), did.clone());
            return Ok(Some(did));
        }

        // Try L3
        if let Some(did) = self.l3.get(key)? {
            // Promote to L2 and L1
            self.l2.insert(key.to_string(), did.clone());
            self.l1.insert(key.to_string(), did.clone());
            return Ok(Some(did));
        }

        Ok(None)
    }

    /// Insert a DID into all cache layers
    pub fn insert(&self, key: &str, value: Did) -> Result<()> {
        self.l1.insert(key.to_string(), value.clone());
        self.l2.insert(key.to_string(), value.clone());
        self.l3.insert(key, &value)?;
        Ok(())
    }

    /// Evict expired entries from memory caches
    pub fn evict_expired(&self) {
        self.l1.evict_expired();
        self.l2.evict_expired();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use libp2p::PeerId;
    use tempfile::tempdir;

    #[test]
    fn test_cache_insertion_and_retrieval() {
        let dir = tempdir().unwrap();
        let cache = MultiLayerCache::new(dir.path().to_str().unwrap()).unwrap();

        let peer_id = PeerId::random();
        let did = Did::new(&peer_id).unwrap();
        let key = did.id.clone();

        cache.insert(&key, did.clone()).unwrap();

        let retrieved = cache.get(&key).unwrap().unwrap();
        assert_eq!(retrieved.id, did.id);
    }

    #[test]
    fn test_cache_promotion() {
        let dir = tempdir().unwrap();
        let cache = MultiLayerCache::new(dir.path().to_str().unwrap()).unwrap();

        let peer_id = PeerId::random();
        let did = Did::new(&peer_id).unwrap();
        let key = did.id.clone();

        // Insert into L3 only
        cache.l3.insert(&key, &did).unwrap();

        // Get should promote to L2 and L1
        let retrieved = cache.get(&key).unwrap().unwrap();
        assert_eq!(retrieved.id, did.id);

        // Verify it's now in L1
        assert!(cache.l1.get(&key).is_some());
    }
}
