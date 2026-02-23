use async_trait::async_trait;
use futures::prelude::*;
use libp2p::request_response::{self, ProtocolSupport};
use libp2p::StreamProtocol;
use std::io;
use variance_proto::messaging_proto::{
    DirectMessage, DirectMessageAck, OfflineMessageRequest, OfflineMessageResponse, TypingIndicator,
};

/// Protocol name for offline message relay
pub const OFFLINE_MESSAGE_PROTOCOL: &str = "/variance/offline-messages/1.0.0";

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

/// Protocol name for direct messages
pub const DIRECT_MESSAGE_PROTOCOL: &str = "/variance/direct-messages/1.0.0";

/// Direct message codec: sends a DirectMessage, receives a DirectMessageAck
#[derive(Debug, Clone, Default)]
pub struct DirectMessageCodec;

#[async_trait]
impl request_response::Codec for DirectMessageCodec {
    type Protocol = StreamProtocol;
    type Request = DirectMessage;
    type Response = DirectMessageAck;

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

/// Direct message protocol behaviour
pub type DirectMessageBehaviour = request_response::Behaviour<DirectMessageCodec>;

/// Create direct message protocol configuration
pub fn create_direct_message_behaviour() -> DirectMessageBehaviour {
    let protocol = StreamProtocol::new(DIRECT_MESSAGE_PROTOCOL);
    request_response::Behaviour::new(
        [(protocol, ProtocolSupport::Full)],
        request_response::Config::default(),
    )
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

/// Protocol name for typing indicators
pub const TYPING_INDICATOR_PROTOCOL: &str = "/variance/typing-indicators/1.0.0";

/// Typing indicator codec: sends a TypingIndicator, receives a TypingIndicator ack.
///
/// The response is an empty TypingIndicator (default) — the sender ignores it.
/// Fire-and-forget semantics; the request_response layer handles the ack internally.
#[derive(Debug, Clone, Default)]
pub struct TypingIndicatorCodec;

#[async_trait]
impl request_response::Codec for TypingIndicatorCodec {
    type Protocol = StreamProtocol;
    type Request = TypingIndicator;
    type Response = TypingIndicator;

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

/// Typing indicator protocol behaviour
pub type TypingIndicatorBehaviour = request_response::Behaviour<TypingIndicatorCodec>;

/// Create typing indicator protocol configuration
pub fn create_typing_indicator_behaviour() -> TypingIndicatorBehaviour {
    let protocol = StreamProtocol::new(TYPING_INDICATOR_PROTOCOL);
    request_response::Behaviour::new(
        [(protocol, ProtocolSupport::Full)],
        request_response::Config::default(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_protocol_names() {
        assert_eq!(OFFLINE_MESSAGE_PROTOCOL, "/variance/offline-messages/1.0.0");
        assert_eq!(DIRECT_MESSAGE_PROTOCOL, "/variance/direct-messages/1.0.0");
        assert_eq!(
            TYPING_INDICATOR_PROTOCOL,
            "/variance/typing-indicators/1.0.0"
        );
    }

    #[test]
    fn test_create_behaviours() {
        let _offline = create_offline_message_behaviour();
        let _direct = create_direct_message_behaviour();
        let _typing = create_typing_indicator_behaviour();
    }
}
