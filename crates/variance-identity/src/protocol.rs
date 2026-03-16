//! Helper functions for constructing identity protocol request/response messages.
//!
//! The actual protocol codec (`IdentityCodec`) and libp2p behaviour types live
//! in `variance-p2p::protocols::identity` — this module only provides
//! convenience constructors so callers don't have to build protobuf structs
//! by hand.

use crate::did::Did;
use chrono::Utc;
use variance_proto::identity_proto;

/// Helper to create an identity request for a username
pub fn create_username_request(
    username: &str,
    requester_did: Option<String>,
) -> identity_proto::IdentityRequest {
    identity_proto::IdentityRequest {
        query: Some(identity_proto::identity_request::Query::Username(
            identity_proto::UsernameQuery {
                username: username.to_string(),
                discriminator: None,
                subnet_id: None,
            },
        )),
        requester_did,
        timestamp: Utc::now().timestamp(),
    }
}

/// Helper to create an identity request for a DID
pub fn create_did_request(
    did: &str,
    requester_did: Option<String>,
) -> identity_proto::IdentityRequest {
    identity_proto::IdentityRequest {
        query: Some(identity_proto::identity_request::Query::Did(
            did.to_string(),
        )),
        requester_did,
        timestamp: Utc::now().timestamp(),
    }
}

/// Helper to create an identity request for a peer ID
pub fn create_peer_id_request(
    peer_id: &str,
    requester_did: Option<String>,
) -> identity_proto::IdentityRequest {
    identity_proto::IdentityRequest {
        query: Some(identity_proto::identity_request::Query::PeerId(
            peer_id.to_string(),
        )),
        requester_did,
        timestamp: Utc::now().timestamp(),
    }
}

/// Helper to create a success response.
/// The `document_signature` field is left empty here; callers that need
/// authenticated responses (e.g., the identity protocol handler) must populate
/// it with the owner's Ed25519 signature.
pub fn create_success_response(did: &Did) -> identity_proto::IdentityResponse {
    identity_proto::IdentityResponse {
        result: Some(identity_proto::identity_response::Result::Found(
            identity_proto::IdentityFound {
                did_document: Some(did.to_proto()),
                ipns_key: None,
                multiaddrs: vec![],
                discriminator: None,
                olm_identity_key: vec![],
                one_time_keys: vec![],
                mls_key_package: None,
                username: None,
                mailbox_token: vec![],
                document_signature: did.document_signature.clone().unwrap_or_default(),
            },
        )),
        timestamp: Utc::now().timestamp(),
    }
}

/// Helper to create a not found response
pub fn create_not_found_response(query: &str, reason: &str) -> identity_proto::IdentityResponse {
    identity_proto::IdentityResponse {
        result: Some(identity_proto::identity_response::Result::NotFound(
            identity_proto::IdentityNotFound {
                query: query.to_string(),
                reason: reason.to_string(),
            },
        )),
        timestamp: Utc::now().timestamp(),
    }
}

/// Helper to create an error response
pub fn create_error_response(error: &str, details: &str) -> identity_proto::IdentityResponse {
    identity_proto::IdentityResponse {
        result: Some(identity_proto::identity_response::Result::Error(
            identity_proto::IdentityError {
                error: error.to_string(),
                details: details.to_string(),
            },
        )),
        timestamp: Utc::now().timestamp(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use libp2p::PeerId;

    #[test]
    fn test_create_username_request() {
        let request = create_username_request("alice", None);
        assert!(matches!(
            request.query,
            Some(identity_proto::identity_request::Query::Username(_))
        ));
    }

    #[test]
    fn test_create_did_request() {
        let did = "did:peer:12D3KooW...";
        let request = create_did_request(did, None);
        assert!(matches!(
            request.query,
            Some(identity_proto::identity_request::Query::Did(_))
        ));
    }

    #[test]
    fn test_create_success_response() {
        let peer_id = PeerId::random();
        let did = Did::new(&peer_id).unwrap();
        let response = create_success_response(&did);
        assert!(matches!(
            response.result,
            Some(identity_proto::identity_response::Result::Found(_))
        ));
    }

    #[test]
    fn test_create_not_found_response() {
        let response = create_not_found_response("alice", "User not found");
        assert!(matches!(
            response.result,
            Some(identity_proto::identity_response::Result::NotFound(_))
        ));
    }

    #[test]
    fn test_create_error_response() {
        let response = create_error_response("Internal error", "Database unavailable");
        assert!(matches!(
            response.result,
            Some(identity_proto::identity_response::Result::Error(_))
        ));
    }
}
