use crate::did::Did;
use crate::error::*;
use async_trait::async_trait;
use sha2::{Digest, Sha256};
use std::path::Path;

/// Abstraction for identity storage backend
///
/// This trait allows swapping between local testing storage and production IPFS/IPNS.
/// For development, use `LocalStorage` which provides IPFS-like semantics without
/// requiring an IPFS daemon. For production, implement `IpfsStorage` using a real
/// IPFS client.
#[async_trait]
pub trait IdentityStorage: Send + Sync {
    /// Store a DID document, returns content ID (CID-like)
    async fn store(&self, did: &Did) -> Result<String>;

    /// Fetch a DID document by content ID
    async fn fetch(&self, id: &str) -> Result<Option<Did>>;

    /// Publish a mutable pointer (IPNS-like): name -> content ID
    async fn publish(&self, name: &str, content_id: &str) -> Result<()>;

    /// Resolve a mutable name to content ID
    async fn resolve(&self, name: &str) -> Result<Option<String>>;
}

/// Local storage implementation using sled
///
/// Simulates IPFS/IPNS for local development and testing:
/// - Content-addressed storage (like IPFS CIDs)
/// - Mutable name resolution (like IPNS)
/// - No external dependencies
///
/// Trade-offs:
/// - No global namespace (each node has its own DB)
/// - No content replication
/// - "CIDs" won't match real IPFS CIDs
///
/// These trade-offs are acceptable for local testing since the libp2p protocol
/// handles cross-node resolution.
pub struct LocalStorage {
    db: sled::Db,
}

impl LocalStorage {
    /// Create a new local storage instance
    pub fn new<P: AsRef<Path>>(path: P) -> Result<Self> {
        let db = sled::open(path).map_err(|e| Error::Storage { source: e })?;
        Ok(Self { db })
    }

    /// Generate a content ID from DID document (simulates IPFS CID)
    fn generate_cid(did: &Did) -> Result<String> {
        let json = serde_json::to_string(did).map_err(|e| Error::Serialization { source: e })?;
        let mut hasher = Sha256::new();
        hasher.update(json.as_bytes());
        let hash = hasher.finalize();
        Ok(format!("local:{}", hex::encode(hash)))
    }

    /// Content tree for storing DID documents
    fn content_tree(&self) -> Result<sled::Tree> {
        self.db
            .open_tree("content")
            .map_err(|e| Error::Storage { source: e })
    }

    /// Names tree for IPNS-like mutable pointers
    fn names_tree(&self) -> Result<sled::Tree> {
        self.db
            .open_tree("names")
            .map_err(|e| Error::Storage { source: e })
    }
}

#[async_trait]
impl IdentityStorage for LocalStorage {
    async fn store(&self, did: &Did) -> Result<String> {
        let cid = Self::generate_cid(did)?;
        let json = serde_json::to_string(did).map_err(|e| Error::Serialization { source: e })?;

        let content = self.content_tree()?;
        content
            .insert(cid.as_bytes(), json.as_bytes())
            .map_err(|e| Error::Storage { source: e })?;

        Ok(cid)
    }

    async fn fetch(&self, id: &str) -> Result<Option<Did>> {
        let content = self.content_tree()?;
        let data = content
            .get(id.as_bytes())
            .map_err(|e| Error::Storage { source: e })?;

        match data {
            Some(bytes) => {
                let did: Did = serde_json::from_slice(&bytes)
                    .map_err(|e| Error::Serialization { source: e })?;
                Ok(Some(did))
            }
            None => Ok(None),
        }
    }

    async fn publish(&self, name: &str, content_id: &str) -> Result<()> {
        let names = self.names_tree()?;
        names
            .insert(name.as_bytes(), content_id.as_bytes())
            .map_err(|e| Error::Storage { source: e })?;
        Ok(())
    }

    async fn resolve(&self, name: &str) -> Result<Option<String>> {
        let names = self.names_tree()?;
        let data = names
            .get(name.as_bytes())
            .map_err(|e| Error::Storage { source: e })?;

        match data {
            Some(bytes) => {
                let cid = String::from_utf8(bytes.to_vec()).map_err(|e| Error::Crypto {
                    message: format!("Invalid UTF-8 in stored CID: {}", e),
                })?;
                Ok(Some(cid))
            }
            None => Ok(None),
        }
    }
}

/// IPFS storage implementation (TODO)
///
/// This will integrate with a real IPFS daemon for production use.
/// Implementation requires:
/// - IPFS client library (e.g., rust-ipfs or ipfs-api)
/// - IPFS daemon running locally or remotely
/// - IPNS key management for mutable pointers
///
/// When implemented, this should:
/// - Store DID documents in IPFS (get real CIDs)
/// - Publish IPNS records for mutable updates
/// - Resolve IPNS names to current CIDs
/// - Handle IPFS pinning for important content
#[allow(dead_code)]
pub struct IpfsStorage {
    // TODO: Add IPFS client when ready
    // client: ipfs_api::IpfsClient,
}

#[cfg(test)]
mod tests {
    use super::*;
    use libp2p::PeerId;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_local_storage_store_and_fetch() {
        let dir = tempdir().unwrap();
        let storage = LocalStorage::new(dir.path()).unwrap();

        let peer_id = PeerId::random();
        let did = Did::new(&peer_id).unwrap();

        // Store DID
        let cid = storage.store(&did).await.unwrap();
        assert!(cid.starts_with("local:"));

        // Fetch DID
        let fetched = storage.fetch(&cid).await.unwrap().unwrap();
        assert_eq!(fetched.id, did.id);
    }

    #[tokio::test]
    async fn test_local_storage_publish_and_resolve() {
        let dir = tempdir().unwrap();
        let storage = LocalStorage::new(dir.path()).unwrap();

        let peer_id = PeerId::random();
        let did = Did::new(&peer_id).unwrap();

        // Store and get CID
        let cid = storage.store(&did).await.unwrap();

        // Publish name
        storage.publish("alice", &cid).await.unwrap();

        // Resolve name
        let resolved = storage.resolve("alice").await.unwrap().unwrap();
        assert_eq!(resolved, cid);

        // Fetch via resolved CID
        let fetched = storage.fetch(&resolved).await.unwrap().unwrap();
        assert_eq!(fetched.id, did.id);
    }

    #[tokio::test]
    async fn test_local_storage_update_name() {
        let dir = tempdir().unwrap();
        let storage = LocalStorage::new(dir.path()).unwrap();

        let peer_id1 = PeerId::random();
        let peer_id2 = PeerId::random();
        let did1 = Did::new(&peer_id1).unwrap();
        let mut did2 = Did::new(&peer_id2).unwrap();
        did2.id = did1.id.clone(); // Same DID, updated

        // Store first version
        let cid1 = storage.store(&did1).await.unwrap();
        storage.publish("alice", &cid1).await.unwrap();

        // Update profile
        did2.update_profile(
            Some("Alice Updated".to_string()),
            None,
            Some("New bio".to_string()),
        );

        // Store second version
        let cid2 = storage.store(&did2).await.unwrap();
        storage.publish("alice", &cid2).await.unwrap();

        // Resolve should return latest
        let resolved = storage.resolve("alice").await.unwrap().unwrap();
        assert_eq!(resolved, cid2);
        assert_ne!(cid1, cid2);

        // Both versions should still be fetchable
        let v1 = storage.fetch(&cid1).await.unwrap().unwrap();
        let v2 = storage.fetch(&cid2).await.unwrap().unwrap();
        assert!(v1.document.display_name.is_none());
        assert_eq!(v2.document.display_name, Some("Alice Updated".to_string()));
    }

    #[tokio::test]
    async fn test_local_storage_content_addressing() {
        let dir = tempdir().unwrap();
        let storage = LocalStorage::new(dir.path()).unwrap();

        let peer_id = PeerId::random();
        let did1 = Did::new(&peer_id).unwrap();
        let did2 = Did::new(&peer_id).unwrap();

        // Same content should produce same CID
        let cid1 = storage.store(&did1).await.unwrap();
        let _ = storage.store(&did2).await.unwrap();

        // Note: These will be different because Did::new() creates different timestamps
        // But storing the same Did instance twice should give same CID
        let cid1_again = storage.store(&did1).await.unwrap();
        assert_eq!(cid1, cid1_again);
    }

    #[tokio::test]
    async fn test_local_storage_missing_content() {
        let dir = tempdir().unwrap();
        let storage = LocalStorage::new(dir.path()).unwrap();

        let result = storage.fetch("local:nonexistent").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_local_storage_missing_name() {
        let dir = tempdir().unwrap();
        let storage = LocalStorage::new(dir.path()).unwrap();

        let result = storage.resolve("nonexistent").await.unwrap();
        assert!(result.is_none());
    }
}
