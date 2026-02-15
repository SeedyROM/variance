use crate::did::Did;
use async_trait::async_trait;
use chrono::Utc;
use futures::prelude::*;
use libp2p::request_response::{self, ProtocolSupport};
use libp2p::StreamProtocol;
use std::io;
use variance_proto::identity_proto;

/// Protocol name for identity resolution
pub const IDENTITY_PROTOCOL: &str = "/variance/identity/1.0.0";

/// Identity resolution codec using protobuf
#[derive(Debug, Clone, Default)]
pub struct IdentityCodec;

#[async_trait]
impl request_response::Codec for IdentityCodec {
    type Protocol = StreamProtocol;
    type Request = identity_proto::IdentityRequest;
    type Response = identity_proto::IdentityResponse;

    async fn read_request<T>(
        &mut self,
        _protocol: &Self::Protocol,
        io: &mut T,
    ) -> io::Result<Self::Request>
    where
        T: AsyncRead + Unpin + Send,
    {
        let mut buf = Vec::new();
        io.read_to_end(&mut buf).await?;
        prost::Message::decode(&buf[..])
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }

    async fn read_response<T>(
        &mut self,
        _protocol: &Self::Protocol,
        io: &mut T,
    ) -> io::Result<Self::Response>
    where
        T: AsyncRead + Unpin + Send,
    {
        let mut buf = Vec::new();
        io.read_to_end(&mut buf).await?;
        prost::Message::decode(&buf[..])
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }

    async fn write_request<T>(
        &mut self,
        _protocol: &Self::Protocol,
        io: &mut T,
        req: Self::Request,
    ) -> io::Result<()>
    where
        T: AsyncWrite + Unpin + Send,
    {
        let mut buf = Vec::new();
        prost::Message::encode(&req, &mut buf)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        io.write_all(&buf).await?;
        io.close().await
    }

    async fn write_response<T>(
        &mut self,
        _protocol: &Self::Protocol,
        io: &mut T,
        res: Self::Response,
    ) -> io::Result<()>
    where
        T: AsyncWrite + Unpin + Send,
    {
        let mut buf = Vec::new();
        prost::Message::encode(&res, &mut buf)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        io.write_all(&buf).await?;
        io.close().await
    }
}

/// Identity protocol behaviour
pub type IdentityBehaviour = request_response::Behaviour<IdentityCodec>;

/// Create identity protocol configuration
pub fn create_identity_behaviour() -> IdentityBehaviour {
    let protocol = StreamProtocol::new(IDENTITY_PROTOCOL);
    request_response::Behaviour::new(
        [(protocol, ProtocolSupport::Full)],
        request_response::Config::default(),
    )
}

/// Events from the identity protocol
pub type IdentityEvent = request_response::Event<
    identity_proto::IdentityRequest,
    identity_proto::IdentityResponse,
>;

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

/// Helper to create a success response
pub fn create_success_response(did: &Did) -> identity_proto::IdentityResponse {
    identity_proto::IdentityResponse {
        result: Some(identity_proto::identity_response::Result::Found(
            identity_proto::IdentityFound {
                did_document: Some(did.to_proto()),
                ipns_key: None,
                multiaddrs: vec![],
                discriminator: None,
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
