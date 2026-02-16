use crate::error::*;
use std::sync::Arc;
use tracing::{debug, error};
use variance_messaging::offline::OfflineRelayHandler;
use variance_messaging::storage::{LocalMessageStorage, MessageStorage};
use variance_proto::messaging_proto::{OfflineMessageRequest, OfflineMessageResponse};

/// Offline message protocol handler
///
/// Wraps the OfflineRelayHandler from variance-messaging and provides
/// request/response handling for the libp2p protocol.
pub struct OfflineMessageHandler {
    relay_handler: OfflineRelayHandler,
}

impl OfflineMessageHandler {
    /// Create a new offline message handler
    ///
    /// # Arguments
    /// * `peer_id` - Local peer ID (used as relay peer ID)
    /// * `storage` - Message storage backend
    pub fn new(peer_id: String, storage: Arc<dyn MessageStorage>) -> Self {
        Self {
            relay_handler: OfflineRelayHandler::new(peer_id, storage),
        }
    }

    /// Create with local storage at a specific path
    pub fn with_local_storage(peer_id: String, storage_path: &std::path::Path) -> Result<Self> {
        let storage =
            Arc::new(
                LocalMessageStorage::new(storage_path).map_err(|e| Error::Transport {
                    source: Box::new(std::io::Error::other(e.to_string())),
                })?,
            );

        Ok(Self::new(peer_id, storage))
    }

    /// Handle an offline message fetch request
    pub async fn handle_request(
        &self,
        request: OfflineMessageRequest,
    ) -> Result<OfflineMessageResponse> {
        debug!(
            "Handling offline message request for DID: {} (limit: {}, since: {:?})",
            request.did, request.limit, request.since_timestamp
        );

        match self.relay_handler.fetch_messages(request).await {
            Ok(response) => {
                debug!(
                    "Returning {} offline messages (has_more: {})",
                    response.messages.len(),
                    response.has_more
                );
                Ok(response)
            }
            Err(e) => {
                error!("Failed to fetch offline messages: {}", e);
                // TODO: Return proper error response instead of empty response
                // Options:
                // 1. Add error variant to OfflineMessageResponse protobuf
                // 2. Use request/response error handling at protocol level
                // 3. Define error codes in response metadata
                // Current behavior silently returns empty, which is ambiguous
                // (can't distinguish "no messages" from "storage error")
                Ok(OfflineMessageResponse {
                    messages: vec![],
                    has_more: false,
                    next_cursor: None,
                })
            }
        }
    }

    /// Get the underlying relay handler
    ///
    /// Useful for storing messages when acting as a relay
    pub fn relay_handler(&self) -> &OfflineRelayHandler {
        &self.relay_handler
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    use ulid::Ulid;
    use variance_proto::messaging_proto::{DirectMessage, MessageType};

    #[tokio::test]
    async fn test_create_handler() {
        let dir = tempdir().unwrap();
        let handler =
            OfflineMessageHandler::with_local_storage("relay1".to_string(), dir.path()).unwrap();

        assert_eq!(handler.relay_handler().ttl_ms(), 30 * 24 * 60 * 60 * 1000);
    }

    #[tokio::test]
    async fn test_fetch_empty() {
        let dir = tempdir().unwrap();
        let handler =
            OfflineMessageHandler::with_local_storage("relay1".to_string(), dir.path()).unwrap();

        let request = OfflineMessageRequest {
            did: "did:variance:bob".to_string(),
            since_timestamp: None,
            limit: 10,
        };

        let response = handler.handle_request(request).await.unwrap();

        assert_eq!(response.messages.len(), 0);
        assert!(!response.has_more);
    }

    #[tokio::test]
    async fn test_store_and_fetch() {
        let dir = tempdir().unwrap();
        let handler =
            OfflineMessageHandler::with_local_storage("relay1".to_string(), dir.path()).unwrap();

        // Store a message via the relay handler
        let direct_msg = DirectMessage {
            id: Ulid::new().to_string(),
            sender_did: "did:variance:alice".to_string(),
            recipient_did: "did:variance:bob".to_string(),
            ciphertext: vec![1, 2, 3],
            nonce: vec![4, 5, 6],
            signature: vec![],
            timestamp: chrono::Utc::now().timestamp_millis(),
            r#type: MessageType::Text.into(),
            reply_to: None,
        };

        let envelope = handler.relay_handler().create_envelope(
            "did:variance:bob".to_string(),
            variance_proto::messaging_proto::offline_message_envelope::Message::Direct(direct_msg),
        );

        handler
            .relay_handler()
            .store_message(envelope)
            .await
            .unwrap();

        // Fetch via the handler
        let request = OfflineMessageRequest {
            did: "did:variance:bob".to_string(),
            since_timestamp: None,
            limit: 10,
        };

        let response = handler.handle_request(request).await.unwrap();

        assert_eq!(response.messages.len(), 1);
        assert_eq!(response.messages[0].recipient_did, "did:variance:bob");
        assert!(!response.has_more);
    }

    #[tokio::test]
    async fn test_pagination() {
        let dir = tempdir().unwrap();
        let handler =
            OfflineMessageHandler::with_local_storage("relay1".to_string(), dir.path()).unwrap();

        // Store 3 messages
        for i in 0..3 {
            let direct_msg = DirectMessage {
                id: Ulid::new().to_string(),
                sender_did: format!("did:variance:sender{}", i),
                recipient_did: "did:variance:bob".to_string(),
                ciphertext: vec![i as u8],
                nonce: vec![],
                signature: vec![],
                timestamp: chrono::Utc::now().timestamp_millis() + i as i64,
                r#type: MessageType::Text.into(),
                reply_to: None,
            };

            let envelope = handler.relay_handler().create_envelope(
                "did:variance:bob".to_string(),
                variance_proto::messaging_proto::offline_message_envelope::Message::Direct(
                    direct_msg,
                ),
            );

            handler
                .relay_handler()
                .store_message(envelope)
                .await
                .unwrap();
        }

        // Fetch with limit=2
        let request = OfflineMessageRequest {
            did: "did:variance:bob".to_string(),
            since_timestamp: None,
            limit: 2,
        };

        let response = handler.handle_request(request).await.unwrap();

        assert_eq!(response.messages.len(), 2);
        assert!(response.has_more);
        assert!(response.next_cursor.is_some());
    }
}
