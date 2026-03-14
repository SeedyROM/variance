use crate::error::*;
use chrono::Utc;
use libp2p::PeerId;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::debug;
use variance_identity::did::Did;
use variance_identity::protocol::{
    create_error_response, create_not_found_response, create_success_response,
};
use variance_proto::identity_proto::{IdentityFound, IdentityRequest, IdentityResponse};

/// Local identity data needed to respond to identity requests about ourselves.
struct LocalIdentity {
    did: String,
    olm_identity_key: Vec<u8>,
    one_time_keys: Vec<Vec<u8>>,
    /// TLS-serialized MLS KeyPackage for group invitations.
    mls_key_package: Option<Vec<u8>>,
    /// Registered username (if any), e.g. "alice"
    username: Option<String>,
    /// 4-digit discriminator paired with username, e.g. 42
    discriminator: Option<u32>,
    /// Opaque relay mailbox token (SHA-256 of signing key).
    mailbox_token: Vec<u8>,
}

/// Identity resolution handler
///
/// Handles identity protocol requests by resolving DIDs from:
/// 1. Local identity (self-response with Olm keys)
/// 2. Local cache (memory, for previously resolved peers)
/// 3. IPFS/IPNS lookup (TODO: requires IPFS integration)
/// 4. DHT provider records (for discovery)
pub struct IdentityHandler {
    /// Local peer ID
    peer_id: PeerId,

    /// Local DID cache (in-memory for now)
    /// Key: DID or username, Value: DID document
    cache: Arc<RwLock<HashMap<String, Did>>>,

    /// Own identity + Olm keys, set via set_local_identity() after node startup.
    local_identity: Arc<RwLock<Option<LocalIdentity>>>,
}

impl IdentityHandler {
    /// Create a new identity handler
    pub fn new(peer_id: PeerId) -> Self {
        Self {
            peer_id,
            cache: Arc::new(RwLock::new(HashMap::new())),
            local_identity: Arc::new(RwLock::new(None)),
        }
    }

    /// Register this node's own identity so we can respond to requests about ourselves.
    ///
    /// Called after the Olm account is initialized and OTKs are generated.
    /// `one_time_keys` should be all available unpublished keys; the full list is
    /// returned in every response so the requester can pick one.
    /// `mls_key_package` is a TLS-serialized MLS KeyPackage for group invitations.
    pub async fn set_local_identity(
        &self,
        did: String,
        olm_identity_key: Vec<u8>,
        one_time_keys: Vec<Vec<u8>>,
        mls_key_package: Option<Vec<u8>>,
        mailbox_token: Vec<u8>,
    ) {
        *self.local_identity.write().await = Some(LocalIdentity {
            did,
            olm_identity_key,
            one_time_keys,
            mls_key_package,
            username: None,
            discriminator: None,
            mailbox_token,
        });
    }

    /// Set the local username and discriminator so they are included in identity
    /// responses. Call after registering or loading a persisted username.
    pub async fn set_local_username(&self, username: String, discriminator: u32) {
        if let Some(ref mut local_id) = *self.local_identity.write().await {
            local_id.username = Some(username);
            local_id.discriminator = Some(discriminator);
        }
    }

    /// Update just the one-time keys list (called after OTK consumption).
    ///
    /// When a PreKey message consumes an OTK, call this to refresh the advertised
    /// list so other peers don't try to use already-consumed keys.
    pub async fn update_one_time_keys(&self, one_time_keys: Vec<Vec<u8>>) {
        if let Some(ref mut local_id) = *self.local_identity.write().await {
            local_id.one_time_keys = one_time_keys;
        }
    }

    /// Replace the advertised MLS KeyPackage after the previous one was consumed by a Welcome.
    pub async fn update_mls_key_package(&self, key_package: Vec<u8>) {
        if let Some(ref mut local_id) = *self.local_identity.write().await {
            local_id.mls_key_package = Some(key_package);
        }
    }

    /// Handle an identity request
    pub async fn handle_request(&self, request: IdentityRequest) -> Result<IdentityResponse> {
        debug!("Handling identity request: {:?}", request);

        match request.query {
            Some(variance_proto::identity_proto::identity_request::Query::Did(did)) => {
                self.resolve_did(&did).await
            }
            Some(variance_proto::identity_proto::identity_request::Query::Username(
                username_query,
            )) => self.resolve_username(&username_query.username).await,
            Some(variance_proto::identity_proto::identity_request::Query::PeerId(peer_id)) => {
                // Check if query is for our own peer ID - if so, respond with our DID
                if peer_id == self.peer_id.to_string() {
                    let local = self.local_identity.read().await;
                    if let Some(ref local_id) = *local {
                        let did_str = local_id.did.clone();
                        drop(local);
                        return self.resolve_did(&did_str).await;
                    }
                    drop(local);
                }

                // Otherwise try to resolve via did:peer format
                let did = format!("did:peer:{}", peer_id);
                self.resolve_did(&did).await
            }
            None => Ok(create_error_response(
                "Invalid request",
                "Request must contain a query",
            )),
        }
    }

    /// Resolve a DID
    async fn resolve_did(&self, did: &str) -> Result<IdentityResponse> {
        debug!("Resolving DID: {}", did);

        // Self-resolution: if the query is for our own DID, respond with our Olm keys.
        // This is how peers learn our Curve25519 key + OTKs to establish an Olm session.
        let local = self.local_identity.read().await;
        if let Some(ref local_id) = *local {
            if local_id.did == did {
                debug!("Responding to self-DID query with Olm keys");

                // Build display_name from registered username+discriminator (e.g. "alice#0042")
                let display_name = match (&local_id.username, local_id.discriminator) {
                    (Some(name), Some(disc)) => Some(format!("{}#{:04}", name, disc)),
                    _ => None,
                };

                let did_doc = variance_proto::identity_proto::DidDocument {
                    id: local_id.did.clone(),
                    authentication: vec![],
                    key_agreement: vec![],
                    service: vec![],
                    created_at: 0,
                    updated_at: 0,
                    display_name,
                    avatar_cid: None,
                    bio: None,
                };

                let found = IdentityFound {
                    did_document: Some(did_doc),
                    olm_identity_key: local_id.olm_identity_key.clone(),
                    one_time_keys: local_id.one_time_keys.clone(),
                    discriminator: local_id.discriminator,
                    mls_key_package: local_id.mls_key_package.clone(),
                    username: local_id.username.clone(),
                    mailbox_token: local_id.mailbox_token.clone(),
                    ..Default::default()
                };
                return Ok(IdentityResponse {
                    result: Some(
                        variance_proto::identity_proto::identity_response::Result::Found(found),
                    ),
                    timestamp: Utc::now().timestamp(),
                });
            }
        }
        drop(local);

        // Check cache for previously resolved peer DIDs
        let cache = self.cache.read().await;
        if let Some(did_doc) = cache.get(did) {
            debug!("DID found in cache: {}", did);
            return Ok(create_success_response(did_doc));
        }
        drop(cache);

        // TODO: IPFS/IPNS resolution for persistent DID storage
        // - Query IPFS for DID document by content hash
        // - Resolve IPNS name to latest DID document version
        // - Cache result for future lookups
        // - Handle IPFS timeout/unavailability gracefully
        Ok(create_not_found_response(
            did,
            "DID not found in cache. IPFS/IPNS resolution not yet implemented.",
        ))
    }

    /// Resolve a username to a DID
    async fn resolve_username(&self, username: &str) -> Result<IdentityResponse> {
        debug!("Resolving username: {}", username);

        // Check cache by username
        let cache = self.cache.read().await;
        if let Some(did_doc) = cache.get(username) {
            debug!("Username found in cache: {}", username);
            return Ok(create_success_response(did_doc));
        }
        drop(cache);

        // TODO: Multi-step username resolution
        // 1. DHT provider record lookup for peers who have this username
        //    - Query Kademlia DHT for provider records with username as key
        //    - Get list of peers who claim to know this username
        // 2. Custom protocol query to those peers
        //    - Send identity request to each provider peer
        //    - Verify DID document username matches
        // 3. IPFS/IPNS resolution as fallback
        //    - Try resolving via IPNS if DHT lookup fails
        // 4. Cache successful result
        // If not in cache, we need:
        // 1. DHT provider record lookup for peers who have this username
        // 2. Custom protocol query to those peers
        // 3. IPFS/IPNS resolution
        Ok(create_not_found_response(
            username,
            "Username not found in cache. DHT provider lookup not yet implemented.",
        ))
    }

    /// Add a DID to the local cache
    ///
    /// This should be called when:
    /// - Local user creates their DID
    /// - A DID is resolved from IPFS/IPNS
    /// - A peer shares their DID via identity protocol
    pub async fn cache_did(&self, did: Did) -> Result<()> {
        let mut cache = self.cache.write().await;

        // Cache by DID
        cache.insert(did.id.clone(), did.clone());

        // Also cache by username if present
        if let Some(ref username) = did.document.display_name {
            cache.insert(username.clone(), did);
        }

        Ok(())
    }

    /// Get a DID from cache
    pub async fn get_cached_did(&self, did_or_username: &str) -> Option<Did> {
        let cache = self.cache.read().await;
        cache.get(did_or_username).cloned()
    }

    /// Get the local peer ID
    pub fn peer_id(&self) -> &PeerId {
        &self.peer_id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_create_handler() {
        let peer_id = PeerId::random();
        let handler = IdentityHandler::new(peer_id);
        assert_eq!(handler.peer_id(), &peer_id);
    }

    #[tokio::test]
    async fn test_resolve_did_not_found() {
        let peer_id = PeerId::random();
        let handler = IdentityHandler::new(peer_id);

        let request = IdentityRequest {
            query: Some(
                variance_proto::identity_proto::identity_request::Query::Did(
                    "did:peer:unknown".to_string(),
                ),
            ),
            requester_did: None,
            timestamp: 0,
        };

        let response = handler.handle_request(request).await.unwrap();

        assert!(matches!(
            response.result,
            Some(variance_proto::identity_proto::identity_response::Result::NotFound(_))
        ));
    }

    #[tokio::test]
    async fn test_cache_and_resolve_did() {
        let peer_id = PeerId::random();
        let handler = IdentityHandler::new(peer_id);

        // Create and cache a DID
        let did = Did::new(&peer_id).unwrap();
        let did_id = did.id.clone();
        handler.cache_did(did).await.unwrap();

        // Try to resolve it
        let request = IdentityRequest {
            query: Some(variance_proto::identity_proto::identity_request::Query::Did(did_id)),
            requester_did: None,
            timestamp: 0,
        };

        let response = handler.handle_request(request).await.unwrap();

        assert!(matches!(
            response.result,
            Some(variance_proto::identity_proto::identity_response::Result::Found(_))
        ));
    }

    #[tokio::test]
    async fn test_cache_and_resolve_username() {
        let peer_id = PeerId::random();
        let handler = IdentityHandler::new(peer_id);

        // Create a DID with a display name
        let mut did = Did::new(&peer_id).unwrap();
        did.update_profile(Some("alice".to_string()), None, None);
        handler.cache_did(did).await.unwrap();

        // Resolve by username
        let request = IdentityRequest {
            query: Some(
                variance_proto::identity_proto::identity_request::Query::Username(
                    variance_proto::identity_proto::UsernameQuery {
                        username: "alice".to_string(),
                        discriminator: None,
                        subnet_id: None,
                    },
                ),
            ),
            requester_did: None,
            timestamp: 0,
        };

        let response = handler.handle_request(request).await.unwrap();

        assert!(matches!(
            response.result,
            Some(variance_proto::identity_proto::identity_response::Result::Found(_))
        ));
    }

    #[tokio::test]
    async fn test_invalid_request() {
        let peer_id = PeerId::random();
        let handler = IdentityHandler::new(peer_id);

        let request = IdentityRequest {
            query: None, // Invalid - no query
            requester_did: None,
            timestamp: 0,
        };

        let response = handler.handle_request(request).await.unwrap();

        assert!(matches!(
            response.result,
            Some(variance_proto::identity_proto::identity_response::Result::Error(_))
        ));
    }

    // ── New tests for self-resolution and identity mutation methods ──

    #[tokio::test]
    async fn self_resolution_returns_olm_keys() {
        let peer_id = PeerId::random();
        let handler = IdentityHandler::new(peer_id);

        let my_did = "did:variance:myself".to_string();
        let olm_key = vec![10, 20, 30];
        let otks = vec![vec![1, 2], vec![3, 4]];
        let mailbox = vec![0xAA, 0xBB];

        handler
            .set_local_identity(
                my_did.clone(),
                olm_key.clone(),
                otks.clone(),
                None,
                mailbox.clone(),
            )
            .await;

        // Query our own DID
        let request = IdentityRequest {
            query: Some(
                variance_proto::identity_proto::identity_request::Query::Did(my_did.clone()),
            ),
            requester_did: None,
            timestamp: 0,
        };

        let response = handler.handle_request(request).await.unwrap();

        match response.result {
            Some(variance_proto::identity_proto::identity_response::Result::Found(found)) => {
                assert_eq!(found.olm_identity_key, olm_key);
                assert_eq!(found.one_time_keys, otks);
                assert_eq!(found.mailbox_token, mailbox);
                assert!(found.mls_key_package.is_none());
                // No username set yet
                assert!(found.username.is_none());
                assert!(found.discriminator.is_none());
                // DID document should be present with our DID
                let doc = found.did_document.unwrap();
                assert_eq!(doc.id, my_did);
                assert!(doc.display_name.is_none());
            }
            other => panic!("Expected Found, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn self_resolution_includes_mls_key_package() {
        let peer_id = PeerId::random();
        let handler = IdentityHandler::new(peer_id);

        let mls_pkg = vec![0xDE, 0xAD, 0xBE, 0xEF];
        handler
            .set_local_identity(
                "did:variance:me".to_string(),
                vec![1],
                vec![],
                Some(mls_pkg.clone()),
                vec![2],
            )
            .await;

        let request = IdentityRequest {
            query: Some(
                variance_proto::identity_proto::identity_request::Query::Did(
                    "did:variance:me".to_string(),
                ),
            ),
            requester_did: None,
            timestamp: 0,
        };

        let response = handler.handle_request(request).await.unwrap();
        match response.result {
            Some(variance_proto::identity_proto::identity_response::Result::Found(found)) => {
                assert_eq!(found.mls_key_package, Some(mls_pkg));
            }
            other => panic!("Expected Found, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn set_local_username_populates_display_name() {
        let peer_id = PeerId::random();
        let handler = IdentityHandler::new(peer_id);

        handler
            .set_local_identity(
                "did:variance:me".to_string(),
                vec![1],
                vec![],
                None,
                vec![2],
            )
            .await;

        handler.set_local_username("alice".to_string(), 42).await;

        let request = IdentityRequest {
            query: Some(
                variance_proto::identity_proto::identity_request::Query::Did(
                    "did:variance:me".to_string(),
                ),
            ),
            requester_did: None,
            timestamp: 0,
        };

        let response = handler.handle_request(request).await.unwrap();
        match response.result {
            Some(variance_proto::identity_proto::identity_response::Result::Found(found)) => {
                assert_eq!(found.username, Some("alice".to_string()));
                assert_eq!(found.discriminator, Some(42));
                // display_name should be formatted as "alice#0042"
                let doc = found.did_document.unwrap();
                assert_eq!(doc.display_name, Some("alice#0042".to_string()));
            }
            other => panic!("Expected Found, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn set_local_username_without_identity_is_noop() {
        let peer_id = PeerId::random();
        let handler = IdentityHandler::new(peer_id);

        // No identity set — set_local_username should not panic
        handler.set_local_username("alice".to_string(), 42).await;

        // Verify local_identity is still None
        let local = handler.local_identity.read().await;
        assert!(local.is_none());
    }

    #[tokio::test]
    async fn update_one_time_keys_replaces_keys() {
        let peer_id = PeerId::random();
        let handler = IdentityHandler::new(peer_id);

        handler
            .set_local_identity(
                "did:variance:me".to_string(),
                vec![1],
                vec![vec![10], vec![20]],
                None,
                vec![2],
            )
            .await;

        // Replace OTKs
        let new_otks = vec![vec![30], vec![40], vec![50]];
        handler.update_one_time_keys(new_otks.clone()).await;

        // Verify via self-resolution
        let request = IdentityRequest {
            query: Some(
                variance_proto::identity_proto::identity_request::Query::Did(
                    "did:variance:me".to_string(),
                ),
            ),
            requester_did: None,
            timestamp: 0,
        };

        let response = handler.handle_request(request).await.unwrap();
        match response.result {
            Some(variance_proto::identity_proto::identity_response::Result::Found(found)) => {
                assert_eq!(found.one_time_keys, new_otks);
            }
            other => panic!("Expected Found, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn update_one_time_keys_without_identity_is_noop() {
        let peer_id = PeerId::random();
        let handler = IdentityHandler::new(peer_id);
        handler.update_one_time_keys(vec![vec![99]]).await;
        let local = handler.local_identity.read().await;
        assert!(local.is_none());
    }

    #[tokio::test]
    async fn update_mls_key_package_replaces_package() {
        let peer_id = PeerId::random();
        let handler = IdentityHandler::new(peer_id);

        handler
            .set_local_identity(
                "did:variance:me".to_string(),
                vec![1],
                vec![],
                Some(vec![0xAA]),
                vec![2],
            )
            .await;

        let new_pkg = vec![0xBB, 0xCC];
        handler.update_mls_key_package(new_pkg.clone()).await;

        let request = IdentityRequest {
            query: Some(
                variance_proto::identity_proto::identity_request::Query::Did(
                    "did:variance:me".to_string(),
                ),
            ),
            requester_did: None,
            timestamp: 0,
        };

        let response = handler.handle_request(request).await.unwrap();
        match response.result {
            Some(variance_proto::identity_proto::identity_response::Result::Found(found)) => {
                assert_eq!(found.mls_key_package, Some(new_pkg));
            }
            other => panic!("Expected Found, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn update_mls_key_package_without_identity_is_noop() {
        let peer_id = PeerId::random();
        let handler = IdentityHandler::new(peer_id);
        handler.update_mls_key_package(vec![0xFF]).await;
        let local = handler.local_identity.read().await;
        assert!(local.is_none());
    }

    #[tokio::test]
    async fn peer_id_query_self_resolves_via_local_identity() {
        let peer_id = PeerId::random();
        let handler = IdentityHandler::new(peer_id);

        handler
            .set_local_identity(
                "did:variance:myself".to_string(),
                vec![1, 2, 3],
                vec![vec![4]],
                None,
                vec![5],
            )
            .await;

        // Query by our own PeerId
        let request = IdentityRequest {
            query: Some(
                variance_proto::identity_proto::identity_request::Query::PeerId(
                    peer_id.to_string(),
                ),
            ),
            requester_did: None,
            timestamp: 0,
        };

        let response = handler.handle_request(request).await.unwrap();
        match response.result {
            Some(variance_proto::identity_proto::identity_response::Result::Found(found)) => {
                assert_eq!(found.olm_identity_key, vec![1, 2, 3]);
                let doc = found.did_document.unwrap();
                assert_eq!(doc.id, "did:variance:myself");
            }
            other => panic!("Expected Found for self PeerId query, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn peer_id_query_without_local_identity_falls_through() {
        let peer_id = PeerId::random();
        let handler = IdentityHandler::new(peer_id);

        // No local identity set — query our own PeerId
        let request = IdentityRequest {
            query: Some(
                variance_proto::identity_proto::identity_request::Query::PeerId(
                    peer_id.to_string(),
                ),
            ),
            requester_did: None,
            timestamp: 0,
        };

        let response = handler.handle_request(request).await.unwrap();
        // Falls through to did:peer:{peer_id} lookup which is not cached → NotFound
        assert!(matches!(
            response.result,
            Some(variance_proto::identity_proto::identity_response::Result::NotFound(_))
        ));
    }

    #[tokio::test]
    async fn peer_id_query_foreign_peer_falls_to_did_peer() {
        let peer_id = PeerId::random();
        let handler = IdentityHandler::new(peer_id);

        let foreign_peer = PeerId::random();

        let request = IdentityRequest {
            query: Some(
                variance_proto::identity_proto::identity_request::Query::PeerId(
                    foreign_peer.to_string(),
                ),
            ),
            requester_did: None,
            timestamp: 0,
        };

        let response = handler.handle_request(request).await.unwrap();
        // Should try did:peer:{foreign_peer} and get NotFound
        assert!(matches!(
            response.result,
            Some(variance_proto::identity_proto::identity_response::Result::NotFound(_))
        ));
    }

    #[tokio::test]
    async fn self_resolution_takes_priority_over_cache() {
        let peer_id = PeerId::random();
        let handler = IdentityHandler::new(peer_id);

        let my_did = "did:variance:me".to_string();

        // Cache a DID with same ID but different data
        let mut cached = Did::new(&peer_id).unwrap();
        // Overwrite the DID id to match our local identity
        cached.id = my_did.clone();
        cached.update_profile(Some("cached_name".to_string()), None, None);
        handler.cache_did(cached).await.unwrap();

        // Set local identity with specific Olm keys
        handler
            .set_local_identity(
                my_did.clone(),
                vec![0xFF],
                vec![vec![0xAA]],
                None,
                vec![0xBB],
            )
            .await;

        let request = IdentityRequest {
            query: Some(variance_proto::identity_proto::identity_request::Query::Did(my_did)),
            requester_did: None,
            timestamp: 0,
        };

        let response = handler.handle_request(request).await.unwrap();
        match response.result {
            Some(variance_proto::identity_proto::identity_response::Result::Found(found)) => {
                // Should return our Olm keys (self-resolution), not the cached doc
                assert_eq!(found.olm_identity_key, vec![0xFF]);
                assert_eq!(found.one_time_keys, vec![vec![0xAA]]);
            }
            other => panic!("Expected Found, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn self_resolution_timestamp_is_recent() {
        let peer_id = PeerId::random();
        let handler = IdentityHandler::new(peer_id);

        handler
            .set_local_identity(
                "did:variance:me".to_string(),
                vec![1],
                vec![],
                None,
                vec![2],
            )
            .await;

        let before = chrono::Utc::now().timestamp();

        let request = IdentityRequest {
            query: Some(
                variance_proto::identity_proto::identity_request::Query::Did(
                    "did:variance:me".to_string(),
                ),
            ),
            requester_did: None,
            timestamp: 0,
        };

        let response = handler.handle_request(request).await.unwrap();
        let after = chrono::Utc::now().timestamp();

        assert!(
            response.timestamp >= before && response.timestamp <= after,
            "Response timestamp {} should be between {} and {}",
            response.timestamp,
            before,
            after,
        );
    }

    #[tokio::test]
    async fn get_cached_did_returns_none_for_unknown() {
        let peer_id = PeerId::random();
        let handler = IdentityHandler::new(peer_id);
        assert!(handler.get_cached_did("unknown").await.is_none());
    }

    #[tokio::test]
    async fn cache_did_indexes_by_both_id_and_display_name() {
        let peer_id = PeerId::random();
        let handler = IdentityHandler::new(peer_id);

        let mut did = Did::new(&peer_id).unwrap();
        let did_id = did.id.clone();
        did.update_profile(Some("bob".to_string()), None, None);
        handler.cache_did(did).await.unwrap();

        // Lookup by DID id
        assert!(handler.get_cached_did(&did_id).await.is_some());
        // Lookup by display name
        assert!(handler.get_cached_did("bob").await.is_some());
    }

    #[tokio::test]
    async fn resolve_username_not_found() {
        let peer_id = PeerId::random();
        let handler = IdentityHandler::new(peer_id);

        let request = IdentityRequest {
            query: Some(
                variance_proto::identity_proto::identity_request::Query::Username(
                    variance_proto::identity_proto::UsernameQuery {
                        username: "nonexistent".to_string(),
                        discriminator: None,
                        subnet_id: None,
                    },
                ),
            ),
            requester_did: None,
            timestamp: 0,
        };

        let response = handler.handle_request(request).await.unwrap();
        assert!(matches!(
            response.result,
            Some(variance_proto::identity_proto::identity_response::Result::NotFound(_))
        ));
    }

    #[tokio::test]
    async fn display_name_formatting_variants() {
        let peer_id = PeerId::random();
        let handler = IdentityHandler::new(peer_id);

        handler
            .set_local_identity(
                "did:variance:me".to_string(),
                vec![1],
                vec![],
                None,
                vec![2],
            )
            .await;

        // Discriminator with leading zeros: 7 → "0007"
        handler.set_local_username("zack".to_string(), 7).await;

        let request = IdentityRequest {
            query: Some(
                variance_proto::identity_proto::identity_request::Query::Did(
                    "did:variance:me".to_string(),
                ),
            ),
            requester_did: None,
            timestamp: 0,
        };

        let response = handler.handle_request(request).await.unwrap();
        match response.result {
            Some(variance_proto::identity_proto::identity_response::Result::Found(found)) => {
                let doc = found.did_document.unwrap();
                assert_eq!(doc.display_name, Some("zack#0007".to_string()));
            }
            other => panic!("Expected Found, got {:?}", other),
        }
    }
}
