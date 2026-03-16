use crate::did::Did;
use crate::error::*;
use async_trait::async_trait;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::Cursor;
use std::path::Path;
use std::sync::Arc;

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

/// IPFS storage implementation
///
/// Integrates with a real IPFS daemon for production use.
/// Requires:
/// - IPFS daemon running locally or remotely
/// - IPNS key management for mutable pointers
///
/// Features:
/// - Store DID documents in IPFS (get real CIDs)
/// - Publish IPNS records for mutable updates
/// - Resolve IPNS names to current CIDs
pub struct IpfsStorage {
    client: ipfs_api_backend_hyper::IpfsClient,
    /// Cache of username → IPNS key name for publishing
    ipns_keys: Arc<tokio::sync::RwLock<HashMap<String, String>>>,
}

impl IpfsStorage {
    /// Create a new IPFS storage instance
    ///
    /// # Arguments
    /// * `api_url` - IPFS API endpoint (e.g., "http://127.0.0.1:5001")
    ///
    /// Supports both HTTP URLs and multiaddr format:
    /// - HTTP: "http://127.0.0.1:5001"
    /// - Multiaddr: "/ip4/127.0.0.1/tcp/5001"
    pub fn new(api_url: &str) -> Result<Self> {
        use ipfs_api_backend_hyper::TryFromUri;

        let client = if api_url.starts_with('/') {
            // Multiaddr format
            ipfs_api_backend_hyper::IpfsClient::from_multiaddr_str(api_url).map_err(|e| {
                Error::Crypto {
                    message: format!("Invalid multiaddr: {}", e),
                }
            })?
        } else if api_url.starts_with("http://") || api_url.starts_with("https://") {
            // HTTP URL format - convert to multiaddr
            let url_str = api_url
                .strip_prefix("http://")
                .or_else(|| api_url.strip_prefix("https://"))
                .unwrap_or(api_url);

            let parts: Vec<&str> = url_str.split(':').collect();
            let host = parts.first().copied().unwrap_or("127.0.0.1");
            let port = parts
                .get(1)
                .and_then(|p| p.parse::<u16>().ok())
                .unwrap_or(5001);

            let multiaddr = format!("/ip4/{}/tcp/{}", host, port);
            ipfs_api_backend_hyper::IpfsClient::from_multiaddr_str(&multiaddr).map_err(|e| {
                Error::Crypto {
                    message: format!("Failed to create IPFS client: {}", e),
                }
            })?
        } else {
            // Default to localhost:5001
            ipfs_api_backend_hyper::IpfsClient::default()
        };

        Ok(Self {
            client,
            ipns_keys: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
        })
    }
}

#[async_trait]
impl IdentityStorage for IpfsStorage {
    async fn store(&self, did: &Did) -> Result<String> {
        use ipfs_api_backend_hyper::IpfsApi;

        // Serialize DID to JSON
        let json = serde_json::to_vec(did).map_err(|e| Error::Serialization { source: e })?;

        // Clone client for moving into spawn
        let client = self.client.clone();

        // Spawn task to make future Send (ipfs-api-backend-hyper futures are not Send)
        let handle = tokio::task::spawn(async move {
            let cursor = Cursor::new(json);
            client.add(cursor).await
        });

        let response = handle
            .await
            .map_err(|e| Error::Crypto {
                message: format!("Task join error: {}", e),
            })?
            .map_err(|e| Error::Crypto {
                message: format!("Failed to add DID to IPFS: {}", e),
            })?;

        // Return IPFS CID
        Ok(response.hash)
    }

    async fn fetch(&self, cid: &str) -> Result<Option<Did>> {
        use futures::TryStreamExt;
        use ipfs_api_backend_hyper::IpfsApi;

        let client = self.client.clone();
        let cid = cid.to_string();

        // Spawn task to make future Send
        let handle = tokio::task::spawn(async move {
            client
                .cat(&cid)
                .map_ok(|chunk| chunk.to_vec())
                .try_concat()
                .await
        });

        // Fetch from IPFS
        let data = match handle.await.map_err(|e| Error::Crypto {
            message: format!("Task join error: {}", e),
        })? {
            Ok(bytes) => bytes,
            Err(e) => {
                let err_str = e.to_string();
                if err_str.contains("not found") || err_str.contains("no such file") {
                    return Ok(None); // CID doesn't exist
                }
                return Err(Error::Crypto {
                    message: format!("Failed to fetch from IPFS: {}", e),
                });
            }
        };

        // Parse JSON to DID
        let did = serde_json::from_slice(&data).map_err(|e| Error::Serialization { source: e })?;

        Ok(Some(did))
    }

    async fn publish(&self, name: &str, content_id: &str) -> Result<()> {
        use ipfs_api_backend_hyper::IpfsApi;

        let key_name = format!("variance-{}", name);
        let client = self.client.clone();
        let content_id = content_id.to_string();
        let key_name_check = key_name.clone();

        // Check if key exists and create if needed
        let handle = tokio::task::spawn(async move {
            use ipfs_api_backend_hyper::request::KeyType;

            let keys = client.key_list().await?;

            if !keys.keys.iter().any(|k| k.name == key_name_check) {
                // Create new IPNS key with Ed25519
                client
                    .key_gen(&key_name_check, KeyType::Ed25519, 2048)
                    .await?;
            }

            // Publish CID to IPNS
            client
                .name_publish(&content_id, false, None, None, Some(&key_name_check))
                .await
        });

        handle
            .await
            .map_err(|e| Error::Crypto {
                message: format!("Task join error: {}", e),
            })?
            .map_err(|e| Error::Crypto {
                message: format!("Failed to publish to IPNS: {}", e),
            })?;

        // Cache key name
        self.ipns_keys
            .write()
            .await
            .insert(name.to_string(), key_name);

        Ok(())
    }

    async fn resolve(&self, name: &str) -> Result<Option<String>> {
        use ipfs_api_backend_hyper::IpfsApi;

        let key_name = format!("variance-{}", name);
        let client = self.client.clone();

        // Spawn task to make future Send
        let handle =
            tokio::task::spawn(
                async move { client.name_resolve(Some(&key_name), false, false).await },
            );

        // Resolve IPNS name to CID
        match handle.await.map_err(|e| Error::Crypto {
            message: format!("Task join error: {}", e),
        })? {
            Ok(resolved) => {
                // Extract CID from /ipfs/<cid> path
                let cid = resolved
                    .path
                    .strip_prefix("/ipfs/")
                    .unwrap_or(&resolved.path)
                    .to_string();
                Ok(Some(cid))
            }
            Err(e) => {
                let err_str = e.to_string();
                if err_str.contains("not found") || err_str.contains("no such name") {
                    return Ok(None);
                }
                Err(Error::Crypto {
                    message: format!("Failed to resolve IPNS name: {}", e),
                })
            }
        }
    }
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
        let did = Did::new("did:variance:store_test", &peer_id).unwrap();

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
        let did = Did::new("did:variance:name_test", &peer_id).unwrap();

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
        let did1 = Did::new("did:variance:update_test1", &peer_id1).unwrap();
        let mut did2 = Did::new("did:variance:update_test2", &peer_id2).unwrap();
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
        let did1 = Did::new("did:variance:dedup1", &peer_id).unwrap();
        let did2 = Did::new("did:variance:dedup2", &peer_id).unwrap();

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

    // IPFS storage tests (require running IPFS daemon)
    // Run with: cargo test --package variance-identity -- --ignored

    #[tokio::test]
    #[ignore] // Requires running IPFS daemon on localhost:5001
    async fn test_ipfs_store_and_fetch() {
        let storage = IpfsStorage::new("http://127.0.0.1:5001").unwrap();

        let peer_id = PeerId::random();
        let did = Did::new("did:variance:ipfs_test", &peer_id).unwrap();

        // Store in IPFS
        let cid = storage.store(&did).await.unwrap();
        assert!(cid.starts_with("Qm") || cid.starts_with("bafy"));

        // Fetch back
        let fetched = storage.fetch(&cid).await.unwrap().unwrap();
        assert_eq!(fetched.id, did.id);
    }

    #[tokio::test]
    #[ignore] // Requires running IPFS daemon on localhost:5001
    async fn test_ipfs_missing_content() {
        let storage = IpfsStorage::new("http://127.0.0.1:5001").unwrap();

        // Try to fetch non-existent CID
        let result = storage
            .fetch("QmNotRealCIDxxxxxxxxxxxxxxxxxxxxxxxxxxx")
            .await
            .unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    #[ignore] // Requires running IPFS daemon on localhost:5001
    async fn test_ipns_publish_and_resolve() {
        let storage = IpfsStorage::new("http://127.0.0.1:5001").unwrap();

        let peer_id = PeerId::random();
        let did = Did::new("did:variance:ipns_test", &peer_id).unwrap();

        // Use a unique name to avoid conflicts with other tests
        let username = format!("test-{}", chrono::Utc::now().timestamp_nanos_opt().unwrap());

        // Store and publish
        let cid = storage.store(&did).await.unwrap();
        storage.publish(&username, &cid).await.unwrap();

        // Resolve username
        let resolved_cid = storage.resolve(&username).await.unwrap().unwrap();
        assert_eq!(resolved_cid, cid);

        // Fetch DID via resolved CID
        let fetched = storage.fetch(&resolved_cid).await.unwrap().unwrap();
        assert_eq!(fetched.id, did.id);
    }

    #[tokio::test]
    #[ignore] // Requires running IPFS daemon on localhost:5001
    async fn test_ipns_update() {
        let storage = IpfsStorage::new("http://127.0.0.1:5001").unwrap();

        let peer_id = PeerId::random();
        let did1 = Did::new("did:variance:ipns_upd1", &peer_id).unwrap();
        let mut did2 = Did::new("did:variance:ipns_upd2", &peer_id).unwrap();

        // Use a unique name to avoid conflicts with other tests
        let username = format!(
            "test-update-{}",
            chrono::Utc::now().timestamp_nanos_opt().unwrap()
        );

        // Store first version
        let cid1 = storage.store(&did1).await.unwrap();
        storage.publish(&username, &cid1).await.unwrap();

        // Update profile
        did2.update_profile(
            Some("Alice Updated".to_string()),
            None,
            Some("New bio".to_string()),
        );

        // Store second version
        let cid2 = storage.store(&did2).await.unwrap();
        storage.publish(&username, &cid2).await.unwrap();

        // Resolve should return latest
        let resolved = storage.resolve(&username).await.unwrap().unwrap();
        assert_eq!(resolved, cid2);
        assert_ne!(cid1, cid2);

        // Both versions should still be fetchable
        let v1 = storage.fetch(&cid1).await.unwrap().unwrap();
        let v2 = storage.fetch(&cid2).await.unwrap().unwrap();
        assert!(v1.document.display_name.is_none());
        assert_eq!(v2.document.display_name, Some("Alice Updated".to_string()));
    }

    #[tokio::test]
    #[ignore] // Requires running IPFS daemon on localhost:5001
    async fn test_ipns_missing_name() {
        let storage = IpfsStorage::new("http://127.0.0.1:5001").unwrap();

        // Try to resolve non-existent name
        let result = storage
            .resolve(&format!(
                "nonexistent-{}",
                chrono::Utc::now().timestamp_nanos_opt().unwrap()
            ))
            .await
            .unwrap();
        assert!(result.is_none());
    }
}
