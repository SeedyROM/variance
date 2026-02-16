use async_trait::async_trait;
use futures::prelude::*;
use libp2p::request_response::{self, ProtocolSupport};
use libp2p::StreamProtocol;
use std::io;
use variance_proto::identity_proto::{IdentityRequest, IdentityResponse};

/// Protocol name for identity resolution
pub const IDENTITY_PROTOCOL: &str = "/variance/identity/1.0.0";

/// Identity resolution codec using protobuf
#[derive(Debug, Clone, Default)]
pub struct IdentityCodec;

#[async_trait]
impl request_response::Codec for IdentityCodec {
    type Protocol = StreamProtocol;
    type Request = IdentityRequest;
    type Response = IdentityResponse;

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
        prost::Message::decode(&buf[..]).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
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
        prost::Message::decode(&buf[..]).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
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
pub type IdentityEvent = request_response::Event<IdentityRequest, IdentityResponse>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_protocol_name() {
        assert_eq!(IDENTITY_PROTOCOL, "/variance/identity/1.0.0");
    }

    #[test]
    fn test_create_behaviour() {
        let _behaviour = create_identity_behaviour();
    }
}
