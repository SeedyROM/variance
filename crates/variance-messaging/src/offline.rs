use crate::error::*;
use crate::storage::MessageStorage;
use std::sync::Arc;
use variance_proto::messaging_proto::{
    OfflineMessageEnvelope, OfflineMessageRequest, OfflineMessageResponse,
};

/// Maximum number of messages queued per mailbox token.
/// Prevents a single mailbox from exhausting relay storage.
const MAX_QUEUED_PER_MAILBOX: usize = 512;

/// Offline message relay handler
///
/// Implements store-and-forward protocol for offline users.
/// Messages are stored with a 14-day TTL and delivered when recipient comes online.
pub struct OfflineRelayHandler {
    /// Message storage backend
    storage: Arc<dyn MessageStorage>,

    /// TTL for offline messages in milliseconds (default: 14 days)
    ttl_ms: i64,
}

impl OfflineRelayHandler {
    /// Create a new offline relay handler
    pub fn new(storage: Arc<dyn MessageStorage>) -> Self {
        Self {
            storage,
            ttl_ms: 14 * 24 * 60 * 60 * 1000, // 14 days in milliseconds
        }
    }

    /// Create a new offline relay handler with custom TTL
    pub fn with_ttl(storage: Arc<dyn MessageStorage>, ttl_ms: i64) -> Self {
        Self { storage, ttl_ms }
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

        // Enforce per-mailbox queue limit before storing
        let queued = self
            .storage
            .count_offline_for_mailbox(&envelope.mailbox_token)
            .await?;
        if queued >= MAX_QUEUED_PER_MAILBOX {
            return Err(Error::RelayStorageFull);
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
                &request.mailbox_token,
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
            error_code: None,
            error_message: None,
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

    /// Create an envelope for a message.
    ///
    /// `mailbox_token` is the recipient's opaque relay address — 32 bytes derived
    /// from their signing key. The relay stores and routes by this token without
    /// ever seeing the recipient's DID.
    pub fn create_envelope(
        &self,
        mailbox_token: Vec<u8>,
        message: variance_proto::messaging_proto::offline_message_envelope::Message,
    ) -> OfflineMessageEnvelope {
        let now = chrono::Utc::now().timestamp_millis();

        OfflineMessageEnvelope {
            mailbox_token,
            message: Some(message),
            stored_at: now,
            expires_at: now + self.ttl_ms,
        }
    }

    /// Get TTL in milliseconds
    pub fn ttl_ms(&self) -> i64 {
        self.ttl_ms
    }
}

/// Create error response
pub fn create_error_response(error_code: &str, error_message: &str) -> OfflineMessageResponse {
    OfflineMessageResponse {
        messages: vec![],
        has_more: false,
        next_cursor: None,
        error_code: Some(error_code.to_string()),
        error_message: Some(error_message.to_string()),
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

        let handler = OfflineRelayHandler::new(storage);

        assert_eq!(handler.ttl_ms, 14 * 24 * 60 * 60 * 1000);
    }

    #[tokio::test]
    async fn test_custom_ttl() {
        let dir = tempdir().unwrap();
        let storage = Arc::new(LocalMessageStorage::new(dir.path()).unwrap());

        let custom_ttl = 7 * 24 * 60 * 60 * 1000; // 7 days
        let handler = OfflineRelayHandler::with_ttl(storage, custom_ttl);

        assert_eq!(handler.ttl_ms(), custom_ttl);
    }

    #[tokio::test]
    async fn test_create_envelope() {
        let dir = tempdir().unwrap();
        let storage = Arc::new(LocalMessageStorage::new(dir.path()).unwrap());

        let handler = OfflineRelayHandler::new(storage);

        let direct_msg = DirectMessage {
            id: Ulid::new().to_string(),
            sender_did: "did:variance:alice".to_string(),
            recipient_did: "did:variance:bob".to_string(),
            ciphertext: vec![1, 2, 3],
            olm_message_type: 0,
            signature: vec![],
            timestamp: chrono::Utc::now().timestamp_millis(),
            r#type: MessageType::Text.into(),
            reply_to: None,
            sender_identity_key: None,
        };

        let envelope = handler.create_envelope(
            vec![0xb0u8; 32],
            variance_proto::messaging_proto::offline_message_envelope::Message::Direct(
                direct_msg.clone(),
            ),
        );

        assert_eq!(envelope.mailbox_token, vec![0xb0u8; 32]);
        assert!(envelope.stored_at > 0);
        assert!(envelope.expires_at > envelope.stored_at);
        assert!(envelope.message.is_some());
    }

    #[tokio::test]
    async fn test_store_and_fetch() {
        let dir = tempdir().unwrap();
        let storage = Arc::new(LocalMessageStorage::new(dir.path()).unwrap());

        let handler = OfflineRelayHandler::new(storage);

        let direct_msg = DirectMessage {
            id: Ulid::new().to_string(),
            sender_did: "did:variance:alice".to_string(),
            recipient_did: "did:variance:bob".to_string(),
            ciphertext: vec![1, 2, 3],
            olm_message_type: 0,
            signature: vec![],
            timestamp: chrono::Utc::now().timestamp_millis(),
            r#type: MessageType::Text.into(),
            reply_to: None,
            sender_identity_key: None,
        };

        let envelope = handler.create_envelope(
            vec![0xb0u8; 32],
            variance_proto::messaging_proto::offline_message_envelope::Message::Direct(
                direct_msg.clone(),
            ),
        );

        // Store message
        handler.store_message(envelope.clone()).await.unwrap();

        // Fetch messages
        let request = OfflineMessageRequest {
            mailbox_token: vec![0xb0u8; 32],
            since_timestamp: None,
            limit: 10,
            ..Default::default()
        };

        let response = handler.fetch_messages(request).await.unwrap();

        assert!(response.error_code.is_none());
        assert_eq!(response.messages.len(), 1);
        assert_eq!(response.messages[0].mailbox_token, vec![0xb0u8; 32]);
        assert!(!response.has_more);
    }

    #[tokio::test]
    async fn test_fetch_with_pagination() {
        let dir = tempdir().unwrap();
        let storage = Arc::new(LocalMessageStorage::new(dir.path()).unwrap());

        let handler = OfflineRelayHandler::new(storage);

        // Store 3 messages
        for i in 0..3 {
            let direct_msg = DirectMessage {
                id: Ulid::new().to_string(),
                sender_did: format!("did:variance:sender{}", i),
                recipient_did: "did:variance:bob".to_string(),
                ciphertext: vec![i as u8],
                olm_message_type: 0,
                signature: vec![],
                timestamp: chrono::Utc::now().timestamp_millis() + i as i64,
                r#type: MessageType::Text.into(),
                reply_to: None,
                sender_identity_key: None,
            };

            let envelope = handler.create_envelope(
                vec![0xb0u8; 32],
                variance_proto::messaging_proto::offline_message_envelope::Message::Direct(
                    direct_msg,
                ),
            );

            handler.store_message(envelope).await.unwrap();
        }

        // Fetch with limit=2
        let request = OfflineMessageRequest {
            mailbox_token: vec![0xb0u8; 32],
            since_timestamp: None,
            limit: 2,
            ..Default::default()
        };

        let response = handler.fetch_messages(request).await.unwrap();

        assert!(response.error_code.is_none());
        assert_eq!(response.messages.len(), 2);
        assert!(response.has_more);
        assert!(response.next_cursor.is_some());
    }

    #[tokio::test]
    async fn test_fetch_since_timestamp() {
        let dir = tempdir().unwrap();
        let storage = Arc::new(LocalMessageStorage::new(dir.path()).unwrap());

        let handler = OfflineRelayHandler::new(storage);

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
            olm_message_type: 0,
            signature: vec![],
            timestamp: old_time,
            r#type: MessageType::Text.into(),
            reply_to: None,
            sender_identity_key: None,
        };

        let envelope1 = handler.create_envelope(
            vec![0xb0u8; 32],
            variance_proto::messaging_proto::offline_message_envelope::Message::Direct(direct_msg1),
        );

        handler.store_message(envelope1).await.unwrap();

        // Store new message
        let direct_msg2 = DirectMessage {
            id: "msg2".to_string(),
            sender_did: "did:variance:alice".to_string(),
            recipient_did: "did:variance:bob".to_string(),
            ciphertext: vec![2],
            olm_message_type: 0,
            signature: vec![],
            timestamp: new_time,
            r#type: MessageType::Text.into(),
            reply_to: None,
            sender_identity_key: None,
        };

        let envelope2 = handler.create_envelope(
            vec![0xb0u8; 32],
            variance_proto::messaging_proto::offline_message_envelope::Message::Direct(direct_msg2),
        );

        handler.store_message(envelope2).await.unwrap();

        // Fetch only messages after base_time
        let request = OfflineMessageRequest {
            mailbox_token: vec![0xb0u8; 32],
            since_timestamp: Some(base_time),
            limit: 10,
            ..Default::default()
        };

        let response = handler.fetch_messages(request).await.unwrap();

        assert!(response.error_code.is_none());
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

        let handler = OfflineRelayHandler::new(storage);

        let direct_msg = DirectMessage {
            id: Ulid::new().to_string(),
            sender_did: "did:variance:alice".to_string(),
            recipient_did: "did:variance:bob".to_string(),
            ciphertext: vec![1, 2, 3],
            olm_message_type: 0,
            signature: vec![],
            timestamp: chrono::Utc::now().timestamp_millis(),
            r#type: MessageType::Text.into(),
            reply_to: None,
            sender_identity_key: None,
        };

        let message_id = direct_msg.id.clone();

        let envelope = handler.create_envelope(
            vec![0xb0u8; 32],
            variance_proto::messaging_proto::offline_message_envelope::Message::Direct(direct_msg),
        );

        // Store message
        handler.store_message(envelope).await.unwrap();

        // Fetch to verify
        let request = OfflineMessageRequest {
            mailbox_token: vec![0xb0u8; 32],
            since_timestamp: None,
            limit: 10,
            ..Default::default()
        };

        let response = handler.fetch_messages(request.clone()).await.unwrap();
        assert!(response.error_code.is_none());
        assert_eq!(response.messages.len(), 1);

        // Delete message
        handler.delete_message(&message_id).await.unwrap();

        // Fetch again - should be empty
        let response = handler.fetch_messages(request).await.unwrap();
        assert!(response.error_code.is_none());
        assert_eq!(response.messages.len(), 0);
    }

    #[tokio::test]
    async fn test_cleanup_expired() {
        let dir = tempdir().unwrap();
        let storage = Arc::new(LocalMessageStorage::new(dir.path()).unwrap());

        // Use short TTL for testing
        let handler = OfflineRelayHandler::with_ttl(storage, 0);

        let direct_msg = DirectMessage {
            id: Ulid::new().to_string(),
            sender_did: "did:variance:alice".to_string(),
            recipient_did: "did:variance:bob".to_string(),
            ciphertext: vec![1, 2, 3],
            olm_message_type: 0,
            signature: vec![],
            timestamp: chrono::Utc::now().timestamp_millis(),
            r#type: MessageType::Text.into(),
            reply_to: None,
            sender_identity_key: None,
        };

        // Create envelope with TTL=0 (expires immediately)
        let envelope = handler.create_envelope(
            vec![0xb0u8; 32],
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
            mailbox_token: vec![0xb0u8; 32],
            since_timestamp: None,
            limit: 10,
            ..Default::default()
        };

        let response = handler.fetch_messages(request).await.unwrap();
        assert!(response.error_code.is_none());
        assert_eq!(response.messages.len(), 0);
    }

    #[tokio::test]
    async fn test_invalid_envelope() {
        let dir = tempdir().unwrap();
        let storage = Arc::new(LocalMessageStorage::new(dir.path()).unwrap());

        let handler = OfflineRelayHandler::new(storage);

        let envelope = OfflineMessageEnvelope {
            mailbox_token: vec![0xb0u8; 32],
            message: None, // Invalid - no message
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

        let handler = OfflineRelayHandler::new(storage);

        let request = OfflineMessageRequest {
            mailbox_token: vec![0xb0u8; 32],
            since_timestamp: None,
            limit: 0, // Invalid - must be > 0
            ..Default::default()
        };

        let result = handler.fetch_messages(request).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), Error::InvalidFormat { .. }));
    }
}
