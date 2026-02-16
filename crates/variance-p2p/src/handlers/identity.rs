use crate::error::*;
use libp2p::PeerId;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::debug;
use variance_identity::did::Did;
use variance_identity::protocol::{
    create_error_response, create_not_found_response, create_success_response,
};
use variance_proto::identity_proto::{IdentityRequest, IdentityResponse};

/// Identity resolution handler
///
/// Handles identity protocol requests by resolving DIDs from:
/// 1. Local cache (memory)
/// 2. IPFS/IPNS lookup (TODO: requires IPFS integration)
/// 3. DHT provider records (for discovery)
pub struct IdentityHandler {
    /// Local peer ID
    peer_id: PeerId,

    /// Local DID cache (in-memory for now)
    /// Key: DID or username, Value: DID document
    cache: Arc<RwLock<std::collections::HashMap<String, Did>>>,
}

impl IdentityHandler {
    /// Create a new identity handler
    pub fn new(peer_id: PeerId) -> Self {
        Self {
            peer_id,
            cache: Arc::new(RwLock::new(std::collections::HashMap::new())),
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
                // Resolve DID by peer ID (convert peer_id to DID format)
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

        // Check cache first
        let cache = self.cache.read().await;
        if let Some(did_doc) = cache.get(did) {
            debug!("DID found in cache: {}", did);
            return Ok(create_success_response(did_doc));
        }
        drop(cache);

        // If not in cache, we need IPFS/IPNS resolution
        // For now, return not found with a message about IPFS integration
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
}
