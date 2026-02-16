use crate::error::*;
use crate::storage::MessageStorage;
use std::sync::Arc;
use variance_proto::messaging_proto::{
    OfflineMessageEnvelope, OfflineMessageRequest, OfflineMessageResponse,
};

/// Offline message relay handler
///
/// Implements store-and-forward protocol for offline users.
/// Messages are stored with a 30-day TTL and delivered when recipient comes online.
pub struct OfflineRelayHandler {
    /// Local peer ID
    peer_id: String,

    /// Message storage backend
    storage: Arc<dyn MessageStorage>,

    /// TTL for offline messages in milliseconds (default: 30 days)
    ttl_ms: i64,
}

impl OfflineRelayHandler {
    /// Create a new offline relay handler
    pub fn new(peer_id: String, storage: Arc<dyn MessageStorage>) -> Self {
        Self {
            peer_id,
            storage,
            ttl_ms: 30 * 24 * 60 * 60 * 1000, // 30 days in milliseconds
        }
    }

    /// Create a new offline relay handler with custom TTL
    pub fn with_ttl(peer_id: String, storage: Arc<dyn MessageStorage>, ttl_ms: i64) -> Self {
        Self {
            peer_id,
            storage,
            ttl_ms,
        }
    }

    /// Store a message for an offline recipient
    ///
    /// Returns an error if storage is full or TTL is invalid.
    pub async fn store_message(&self, envelope: OfflineMessageEnvelope) -> Result<()> {
        // Validate envelope
        if envelope.message.is_none() {
            return Err(Error::InvalidFormat {
                message: "Envelope must contain a message".to_string(),
            });
        }

        // Check if expired (shouldn't happen, but validate)
        let now = chrono::Utc::now().timestamp_millis();
        if envelope.expires_at < now {
            return Err(Error::MessageExpired {
                message_id: "envelope".to_string(),
            });
        }

        // Store envelope
        self.storage.store_offline(&envelope).await?;

        Ok(())
    }

    /// Fetch offline messages for a recipient
    ///
    /// Returns messages stored since the given timestamp, up to the limit.
    /// Returns pagination info if there are more messages available.
    pub async fn fetch_messages(
        &self,
        request: OfflineMessageRequest,
    ) -> Result<OfflineMessageResponse> {
        // Validate request
        if request.limit == 0 {
            return Err(Error::InvalidFormat {
                message: "Limit must be greater than 0".to_string(),
            });
        }

        // Fetch limit+1 messages to check if there are more
        let messages = self
            .storage
            .fetch_offline(
                &request.did,
                request.since_timestamp,
                (request.limit + 1) as usize,
            )
            .await?;

        // Check if there are more messages
        let has_more = messages.len() > request.limit as usize;
        let actual_messages = if has_more {
            messages[..request.limit as usize].to_vec()
        } else {
            messages
        };

        // Get next cursor (timestamp of last message)
        let next_cursor = actual_messages.last().map(|env| env.stored_at);

        Ok(OfflineMessageResponse {
            messages: actual_messages,
            has_more,
            next_cursor,
        })
    }

    /// Delete a message after successful delivery
    ///
    /// Should be called after the recipient acknowledges receipt.
    pub async fn delete_message(&self, message_id: &str) -> Result<()> {
        self.storage.delete_offline(message_id).await
    }

    /// Clean up expired messages
    ///
    /// Returns the number of messages cleaned up.
    /// Should be called periodically (e.g., daily).
    pub async fn cleanup_expired(&self) -> Result<usize> {
        self.storage.cleanup_expired().await
    }

    /// Create an envelope for a message
    ///
    /// Helper to wrap a DirectMessage or GroupMessage in an OfflineMessageEnvelope.
    pub fn create_envelope(
        &self,
        recipient_did: String,
        message: variance_proto::messaging_proto::offline_message_envelope::Message,
    ) -> OfflineMessageEnvelope {
        let now = chrono::Utc::now().timestamp_millis();

        OfflineMessageEnvelope {
            recipient_did,
            message: Some(message),
            relay_peer_id: self.peer_id.clone(),
            stored_at: now,
            expires_at: now + self.ttl_ms,
        }
    }

    /// Get TTL in milliseconds
    pub fn ttl_ms(&self) -> i64 {
        self.ttl_ms
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::LocalMessageStorage;
    use tempfile::tempdir;
    use ulid::Ulid;
    use variance_proto::messaging_proto::{DirectMessage, MessageType};

    #[tokio::test]
    async fn test_create_handler() {
        let dir = tempdir().unwrap();
        let storage = Arc::new(LocalMessageStorage::new(dir.path()).unwrap());

        let handler = OfflineRelayHandler::new("relay1".to_string(), storage);

        assert_eq!(handler.peer_id, "relay1");
        assert_eq!(handler.ttl_ms, 30 * 24 * 60 * 60 * 1000);
    }

    #[tokio::test]
    async fn test_custom_ttl() {
        let dir = tempdir().unwrap();
        let storage = Arc::new(LocalMessageStorage::new(dir.path()).unwrap());

        let custom_ttl = 7 * 24 * 60 * 60 * 1000; // 7 days
        let handler = OfflineRelayHandler::with_ttl("relay1".to_string(), storage, custom_ttl);

        assert_eq!(handler.ttl_ms(), custom_ttl);
    }

    #[tokio::test]
    async fn test_create_envelope() {
        let dir = tempdir().unwrap();
        let storage = Arc::new(LocalMessageStorage::new(dir.path()).unwrap());

        let handler = OfflineRelayHandler::new("relay1".to_string(), storage);

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

        let envelope = handler.create_envelope(
            "did:variance:bob".to_string(),
            variance_proto::messaging_proto::offline_message_envelope::Message::Direct(
                direct_msg.clone(),
            ),
        );

        assert_eq!(envelope.recipient_did, "did:variance:bob");
        assert_eq!(envelope.relay_peer_id, "relay1");
        assert!(envelope.stored_at > 0);
        assert!(envelope.expires_at > envelope.stored_at);
        assert!(envelope.message.is_some());
    }

    #[tokio::test]
    async fn test_store_and_fetch() {
        let dir = tempdir().unwrap();
        let storage = Arc::new(LocalMessageStorage::new(dir.path()).unwrap());

        let handler = OfflineRelayHandler::new("relay1".to_string(), storage);

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

        let envelope = handler.create_envelope(
            "did:variance:bob".to_string(),
            variance_proto::messaging_proto::offline_message_envelope::Message::Direct(
                direct_msg.clone(),
            ),
        );

        // Store message
        handler.store_message(envelope.clone()).await.unwrap();

        // Fetch messages
        let request = OfflineMessageRequest {
            did: "did:variance:bob".to_string(),
            since_timestamp: None,
            limit: 10,
        };

        let response = handler.fetch_messages(request).await.unwrap();

        assert_eq!(response.messages.len(), 1);
        assert_eq!(response.messages[0].recipient_did, "did:variance:bob");
        assert!(!response.has_more);
    }

    #[tokio::test]
    async fn test_fetch_with_pagination() {
        let dir = tempdir().unwrap();
        let storage = Arc::new(LocalMessageStorage::new(dir.path()).unwrap());

        let handler = OfflineRelayHandler::new("relay1".to_string(), storage);

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

            let envelope = handler.create_envelope(
                "did:variance:bob".to_string(),
                variance_proto::messaging_proto::offline_message_envelope::Message::Direct(
                    direct_msg,
                ),
            );

            handler.store_message(envelope).await.unwrap();
        }

        // Fetch with limit=2
        let request = OfflineMessageRequest {
            did: "did:variance:bob".to_string(),
            since_timestamp: None,
            limit: 2,
        };

        let response = handler.fetch_messages(request).await.unwrap();

        assert_eq!(response.messages.len(), 2);
        assert!(response.has_more);
        assert!(response.next_cursor.is_some());
    }

    #[tokio::test]
    async fn test_fetch_since_timestamp() {
        let dir = tempdir().unwrap();
        let storage = Arc::new(LocalMessageStorage::new(dir.path()).unwrap());

        let handler = OfflineRelayHandler::new("relay1".to_string(), storage);

        // Use well-separated timestamps
        let base_time = 1000000000000i64; // Fixed timestamp
        let old_time = base_time - 10000;
        let new_time = base_time + 10000;

        // Store old message
        let direct_msg1 = DirectMessage {
            id: "msg1".to_string(),
            sender_did: "did:variance:alice".to_string(),
            recipient_did: "did:variance:bob".to_string(),
            ciphertext: vec![1],
            nonce: vec![],
            signature: vec![],
            timestamp: old_time,
            r#type: MessageType::Text.into(),
            reply_to: None,
        };

        let envelope1 = handler.create_envelope(
            "did:variance:bob".to_string(),
            variance_proto::messaging_proto::offline_message_envelope::Message::Direct(direct_msg1),
        );

        handler.store_message(envelope1).await.unwrap();

        // Store new message
        let direct_msg2 = DirectMessage {
            id: "msg2".to_string(),
            sender_did: "did:variance:alice".to_string(),
            recipient_did: "did:variance:bob".to_string(),
            ciphertext: vec![2],
            nonce: vec![],
            signature: vec![],
            timestamp: new_time,
            r#type: MessageType::Text.into(),
            reply_to: None,
        };

        let envelope2 = handler.create_envelope(
            "did:variance:bob".to_string(),
            variance_proto::messaging_proto::offline_message_envelope::Message::Direct(direct_msg2),
        );

        handler.store_message(envelope2).await.unwrap();

        // Fetch only messages after base_time
        let request = OfflineMessageRequest {
            did: "did:variance:bob".to_string(),
            since_timestamp: Some(base_time),
            limit: 10,
        };

        let response = handler.fetch_messages(request).await.unwrap();

        assert_eq!(response.messages.len(), 1);

        // Verify it's the later message by checking the inner message timestamp
        match &response.messages[0].message {
            Some(variance_proto::messaging_proto::offline_message_envelope::Message::Direct(
                msg,
            )) => {
                assert_eq!(msg.timestamp, new_time);
                assert_eq!(msg.id, "msg2");
            }
            _ => panic!("Expected direct message"),
        }
    }

    #[tokio::test]
    async fn test_delete_message() {
        let dir = tempdir().unwrap();
        let storage = Arc::new(LocalMessageStorage::new(dir.path()).unwrap());

        let handler = OfflineRelayHandler::new("relay1".to_string(), storage);

        let direct_msg = DirectMessage {
            id: Ulid::new().to_string(),
            sender_did: "did:variance:alice".to_string(),
            recipient_did: "did:variance:bob".to_string(),
            ciphertext: vec![1, 2, 3],
            nonce: vec![],
            signature: vec![],
            timestamp: chrono::Utc::now().timestamp_millis(),
            r#type: MessageType::Text.into(),
            reply_to: None,
        };

        let message_id = direct_msg.id.clone();

        let envelope = handler.create_envelope(
            "did:variance:bob".to_string(),
            variance_proto::messaging_proto::offline_message_envelope::Message::Direct(direct_msg),
        );

        // Store message
        handler.store_message(envelope).await.unwrap();

        // Fetch to verify
        let request = OfflineMessageRequest {
            did: "did:variance:bob".to_string(),
            since_timestamp: None,
            limit: 10,
        };

        let response = handler.fetch_messages(request.clone()).await.unwrap();
        assert_eq!(response.messages.len(), 1);

        // Delete message
        handler.delete_message(&message_id).await.unwrap();

        // Fetch again - should be empty
        let response = handler.fetch_messages(request).await.unwrap();
        assert_eq!(response.messages.len(), 0);
    }

    #[tokio::test]
    async fn test_cleanup_expired() {
        let dir = tempdir().unwrap();
        let storage = Arc::new(LocalMessageStorage::new(dir.path()).unwrap());

        // Use short TTL for testing
        let handler = OfflineRelayHandler::with_ttl("relay1".to_string(), storage, 0);

        let direct_msg = DirectMessage {
            id: Ulid::new().to_string(),
            sender_did: "did:variance:alice".to_string(),
            recipient_did: "did:variance:bob".to_string(),
            ciphertext: vec![1, 2, 3],
            nonce: vec![],
            signature: vec![],
            timestamp: chrono::Utc::now().timestamp_millis(),
            r#type: MessageType::Text.into(),
            reply_to: None,
        };

        // Create envelope with TTL=0 (expires immediately)
        let envelope = handler.create_envelope(
            "did:variance:bob".to_string(),
            variance_proto::messaging_proto::offline_message_envelope::Message::Direct(direct_msg),
        );

        handler.store_message(envelope).await.unwrap();

        // Wait a moment to ensure expiration
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

        // Cleanup expired
        let cleaned = handler.cleanup_expired().await.unwrap();

        assert_eq!(cleaned, 1);

        // Verify no messages remain
        let request = OfflineMessageRequest {
            did: "did:variance:bob".to_string(),
            since_timestamp: None,
            limit: 10,
        };

        let response = handler.fetch_messages(request).await.unwrap();
        assert_eq!(response.messages.len(), 0);
    }

    #[tokio::test]
    async fn test_invalid_envelope() {
        let dir = tempdir().unwrap();
        let storage = Arc::new(LocalMessageStorage::new(dir.path()).unwrap());

        let handler = OfflineRelayHandler::new("relay1".to_string(), storage);

        let envelope = OfflineMessageEnvelope {
            recipient_did: "did:variance:bob".to_string(),
            message: None, // Invalid - no message
            relay_peer_id: "relay1".to_string(),
            stored_at: chrono::Utc::now().timestamp_millis(),
            expires_at: chrono::Utc::now().timestamp_millis() + 1000,
        };

        let result = handler.store_message(envelope).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), Error::InvalidFormat { .. }));
    }

    #[tokio::test]
    async fn test_invalid_request_limit() {
        let dir = tempdir().unwrap();
        let storage = Arc::new(LocalMessageStorage::new(dir.path()).unwrap());

        let handler = OfflineRelayHandler::new("relay1".to_string(), storage);

        let request = OfflineMessageRequest {
            did: "did:variance:bob".to_string(),
            since_timestamp: None,
            limit: 0, // Invalid - must be > 0
        };

        let result = handler.fetch_messages(request).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), Error::InvalidFormat { .. }));
    }
}
