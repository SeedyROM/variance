//! Persistent DID ↔ PeerId mapping backed by sled.
//!
//! The P2P node learns DID→PeerId associations at runtime (from identity
//! protocol responses, direct messages, etc.). This store persists those
//! mappings so they survive restarts — a reconnecting peer can be routed
//! to immediately without waiting for another identity exchange.

use std::collections::HashMap;
use std::path::Path;

use libp2p::PeerId;
use tracing::{debug, warn};

use crate::error::{Error, Result};

/// Thin wrapper around a sled tree (`did_peer_map`) that persists DID → PeerId
/// associations.  Thread-safe; all sled operations are internally atomic.
#[derive(Clone)]
pub struct PeerStore {
    tree: sled::Tree,
}

impl PeerStore {
    /// Open (or create) the store at `<base_path>/peer_store`.
    pub fn open(base_path: &Path) -> Result<Self> {
        let db = sled::open(base_path.join("peer_store")).map_err(|e| Error::Storage {
            message: format!("Failed to open peer store: {}", e),
        })?;
        let tree = db.open_tree("did_peer_map").map_err(|e| Error::Storage {
            message: format!("Failed to open did_peer_map tree: {}", e),
        })?;
        debug!("Peer store opened with {} persisted mappings", tree.len());
        Ok(Self { tree })
    }

    /// Persist a DID → PeerId mapping.
    pub fn insert(&self, did: &str, peer_id: &PeerId) {
        let peer_bytes = peer_id.to_bytes();
        if let Err(e) = self.tree.insert(did.as_bytes(), peer_bytes.as_slice()) {
            warn!("Failed to persist DID→PeerId mapping for {}: {}", did, e);
        }
    }

    /// Look up a persisted PeerId by DID. Returns `None` if not found.
    pub fn get(&self, did: &str) -> Option<PeerId> {
        match self.tree.get(did.as_bytes()) {
            Ok(Some(bytes)) => PeerId::from_bytes(&bytes).ok(),
            Ok(None) => None,
            Err(e) => {
                warn!("Failed to read peer store for {}: {}", did, e);
                None
            }
        }
    }

    /// Load all persisted DID → PeerId mappings into a `HashMap`.
    pub fn load_all(&self) -> HashMap<String, PeerId> {
        let mut map = HashMap::new();
        for entry in self.tree.iter() {
            match entry {
                Ok((key, value)) => {
                    if let (Ok(did), Ok(peer_id)) =
                        (String::from_utf8(key.to_vec()), PeerId::from_bytes(&value))
                    {
                        map.insert(did, peer_id);
                    }
                }
                Err(e) => {
                    warn!("Error iterating peer store: {}", e);
                }
            }
        }
        map
    }

    /// Remove a persisted mapping by DID.
    #[allow(dead_code)]
    pub fn remove(&self, did: &str) {
        if let Err(e) = self.tree.remove(did.as_bytes()) {
            warn!("Failed to remove DID→PeerId mapping for {}: {}", did, e);
        }
    }

    /// Number of persisted mappings.
    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.tree.len()
    }

    /// Whether the store is empty.
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.tree.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_insert_and_get() {
        let dir = tempfile::tempdir().unwrap();
        let store = PeerStore::open(dir.path()).unwrap();
        let peer_id = PeerId::random();

        store.insert("did:variance:alice", &peer_id);
        assert_eq!(store.get("did:variance:alice"), Some(peer_id));
        assert_eq!(store.get("did:variance:bob"), None);
    }

    #[test]
    fn test_load_all() {
        let dir = tempfile::tempdir().unwrap();
        let store = PeerStore::open(dir.path()).unwrap();
        let alice = PeerId::random();
        let bob = PeerId::random();

        store.insert("did:variance:alice", &alice);
        store.insert("did:variance:bob", &bob);

        let all = store.load_all();
        assert_eq!(all.len(), 2);
        assert_eq!(all["did:variance:alice"], alice);
        assert_eq!(all["did:variance:bob"], bob);
    }

    #[test]
    fn test_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        let store = PeerStore::open(dir.path()).unwrap();
        let old_peer = PeerId::random();
        let new_peer = PeerId::random();

        store.insert("did:variance:alice", &old_peer);
        store.insert("did:variance:alice", &new_peer);
        assert_eq!(store.get("did:variance:alice"), Some(new_peer));
    }

    #[test]
    fn test_remove() {
        let dir = tempfile::tempdir().unwrap();
        let store = PeerStore::open(dir.path()).unwrap();
        let peer = PeerId::random();

        store.insert("did:variance:alice", &peer);
        store.remove("did:variance:alice");
        assert_eq!(store.get("did:variance:alice"), None);
    }

    #[test]
    fn test_persistence_across_reopens() {
        let dir = tempfile::tempdir().unwrap();
        let peer = PeerId::random();

        {
            let store = PeerStore::open(dir.path()).unwrap();
            store.insert("did:variance:alice", &peer);
        }

        // Reopen the store — data should survive
        let store = PeerStore::open(dir.path()).unwrap();
        assert_eq!(store.get("did:variance:alice"), Some(peer));
    }
}
