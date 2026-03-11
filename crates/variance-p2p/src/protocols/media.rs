use async_trait::async_trait;
use futures::prelude::*;
use libp2p::request_response::{self, ProtocolSupport};
use libp2p::StreamProtocol;
use std::io;
use variance_proto::media_proto::SignalingMessage;

/// Maximum message size for signaling protocol (64 KiB).
const MAX_MESSAGE_SIZE: u64 = 64 * 1024;

/// Protocol name for WebRTC signaling
pub const SIGNALING_PROTOCOL: &str = "/variance/webrtc-signaling/1.0.0";

/// WebRTC signaling codec using protobuf
#[derive(Debug, Clone, Default)]
pub struct SignalingCodec;

#[async_trait]
impl request_response::Codec for SignalingCodec {
    type Protocol = StreamProtocol;
    type Request = SignalingMessage;
    type Response = SignalingMessage;

    async fn read_request<T>(
        &mut self,
        _protocol: &Self::Protocol,
        io: &mut T,
    ) -> io::Result<Self::Request>
    where
        T: AsyncRead + Unpin + Send,
    {
        let mut buf = Vec::new();
        io.take(MAX_MESSAGE_SIZE).read_to_end(&mut buf).await?;
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
        io.take(MAX_MESSAGE_SIZE).read_to_end(&mut buf).await?;
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

/// WebRTC signaling protocol behaviour
pub type SignalingBehaviour = request_response::Behaviour<SignalingCodec>;

/// Create signaling protocol configuration
pub fn create_signaling_behaviour() -> SignalingBehaviour {
    let protocol = StreamProtocol::new(SIGNALING_PROTOCOL);
    request_response::Behaviour::new(
        [(protocol, ProtocolSupport::Full)],
        request_response::Config::default(),
    )
}

/// Events from the signaling protocol
pub type SignalingEvent = request_response::Event<SignalingMessage, SignalingMessage>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_protocol_name() {
        assert_eq!(SIGNALING_PROTOCOL, "/variance/webrtc-signaling/1.0.0");
    }

    #[test]
    fn test_create_behaviour() {
        let _behaviour = create_signaling_behaviour();
    }
}
