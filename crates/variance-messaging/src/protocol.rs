use async_trait::async_trait;
use futures::prelude::*;
use libp2p::request_response::{self, ProtocolSupport};
use libp2p::StreamProtocol;
use std::io;
use variance_proto::messaging_proto::{OfflineMessageRequest, OfflineMessageResponse};

/// Protocol name for offline message relay
pub const OFFLINE_MESSAGE_PROTOCOL: &str = "/variance/offline-messages/1.0.0";

/// Maximum size for a single offline message protocol read (1 MiB).
/// Consistent with other messaging codecs — prevents OOM from malicious peers.
const MAX_MESSAGE_SIZE: u64 = 1024 * 1024;

/// Offline message codec using protobuf
#[derive(Debug, Clone, Default)]
pub struct OfflineMessageCodec;

#[async_trait]
impl request_response::Codec for OfflineMessageCodec {
    type Protocol = StreamProtocol;
    type Request = OfflineMessageRequest;
    type Response = OfflineMessageResponse;

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

/// Offline message protocol behaviour
pub type OfflineMessageBehaviour = request_response::Behaviour<OfflineMessageCodec>;

/// Create offline message protocol configuration
pub fn create_offline_message_behaviour() -> OfflineMessageBehaviour {
    let protocol = StreamProtocol::new(OFFLINE_MESSAGE_PROTOCOL);
    request_response::Behaviour::new(
        [(protocol, ProtocolSupport::Full)],
        request_response::Config::default(),
    )
}

/// Events from the offline message protocol
pub type OfflineMessageEvent =
    request_response::Event<OfflineMessageRequest, OfflineMessageResponse>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_protocol_name() {
        assert_eq!(OFFLINE_MESSAGE_PROTOCOL, "/variance/offline-messages/1.0.0");
    }

    #[test]
    fn test_create_behaviour() {
        let _behaviour = create_offline_message_behaviour();
    }
}
