use crate::error::*;
use crate::storage::MessageStorage;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use std::sync::Arc;
use variance_proto::messaging_proto::{GroupReadReceipt, ReadReceipt, ReceiptStatus};

/// Read receipt handler
///
/// Manages delivery and read confirmations for messages.
/// Receipts are signed to prevent spoofing.
pub struct ReceiptHandler {
    /// Local DID
    local_did: String,

    /// Signing key for receipt authentication
    signing_key: SigningKey,

    /// Message storage backend
    storage: Arc<dyn MessageStorage>,
}

impl ReceiptHandler {
    /// Create a new receipt handler
    pub fn new(
        local_did: String,
        signing_key: SigningKey,
        storage: Arc<dyn MessageStorage>,
    ) -> Self {
        Self {
            local_did,
            signing_key,
            storage,
        }
    }

    /// Send a delivery receipt
    ///
    /// Called when a message is received successfully.
    pub async fn send_delivered(&self, message_id: String) -> Result<ReadReceipt> {
        self.send_receipt(message_id, ReceiptStatus::Delivered)
            .await
    }

    /// Send a read receipt
    ///
    /// Called when a message is displayed to the user.
    pub async fn send_read(&self, message_id: String) -> Result<ReadReceipt> {
        self.send_receipt(message_id, ReceiptStatus::Read).await
    }

    /// Internal: Send a receipt with given status
    async fn send_receipt(&self, message_id: String, status: ReceiptStatus) -> Result<ReadReceipt> {
        let timestamp = chrono::Utc::now().timestamp_millis();

        let mut receipt = ReadReceipt {
            message_id,
            reader_did: self.local_did.clone(),
            status: status.into(),
            timestamp,
            signature: vec![],
        };

        // Sign receipt
        receipt.signature = self.sign_receipt(&receipt)?;

        // Store receipt
        self.storage.store_receipt(&receipt).await?;

        Ok(receipt)
    }

    /// Receive and verify a receipt
    ///
    /// NOTE: Caller must verify receipt signature using verify_receipt_with_key()
    /// before calling this, passing the reader's public key from their DID document.
    pub async fn receive_receipt(&self, receipt: ReadReceipt) -> Result<()> {
        // Store receipt
        self.storage.store_receipt(&receipt).await?;

        Ok(())
    }

    /// Get all receipts for a message
    pub async fn get_receipts(&self, message_id: &str) -> Result<Vec<ReadReceipt>> {
        self.storage.fetch_receipts(message_id).await
    }

    /// Get latest receipt status for a specific reader
    pub async fn get_receipt_status(
        &self,
        message_id: &str,
        reader_did: &str,
    ) -> Result<Option<ReadReceipt>> {
        self.storage
            .fetch_receipt_status(message_id, reader_did)
            .await
    }

    /// Sign a receipt
    fn sign_receipt(&self, receipt: &ReadReceipt) -> Result<Vec<u8>> {
        let mut data = Vec::new();
        data.extend_from_slice(receipt.message_id.as_bytes());
        data.extend_from_slice(receipt.reader_did.as_bytes());
        data.extend_from_slice(&receipt.status.to_le_bytes());
        data.extend_from_slice(&receipt.timestamp.to_le_bytes());

        let signature = self.signing_key.sign(&data);
        Ok(signature.to_bytes().to_vec())
    }

    /// Verify a receipt signature
    ///
    /// NOTE: This requires the reader's public key which must be fetched from their
    /// DID document via the identity system. Currently verification is deferred to
    /// the caller who must provide the reader's public key.
    pub fn verify_receipt_with_key(
        &self,
        receipt: &ReadReceipt,
        reader_public_key: &VerifyingKey,
    ) -> Result<()> {
        let mut data = Vec::new();
        data.extend_from_slice(receipt.message_id.as_bytes());
        data.extend_from_slice(receipt.reader_did.as_bytes());
        data.extend_from_slice(&receipt.status.to_le_bytes());
        data.extend_from_slice(&receipt.timestamp.to_le_bytes());

        let signature =
            Signature::from_bytes(receipt.signature.as_slice().try_into().map_err(|_| {
                Error::InvalidSignature {
                    message_id: receipt.message_id.clone(),
                }
            })?);

        reader_public_key
            .verify(&data, &signature)
            .map_err(|_| Error::InvalidSignature {
                message_id: receipt.message_id.clone(),
            })?;

        Ok(())
    }

    // ===== Group receipt methods =====

    /// Send a group delivery receipt.
    ///
    /// Called when a group message is received and successfully decrypted.
    pub async fn send_group_delivered(
        &self,
        group_id: &str,
        message_id: &str,
    ) -> Result<GroupReadReceipt> {
        self.send_group_receipt(group_id, message_id, ReceiptStatus::Delivered)
            .await
    }

    /// Send a group read receipt.
    ///
    /// Called when a group message is displayed to the user.
    pub async fn send_group_read(
        &self,
        group_id: &str,
        message_id: &str,
    ) -> Result<GroupReadReceipt> {
        self.send_group_receipt(group_id, message_id, ReceiptStatus::Read)
            .await
    }

    /// Internal: create, sign, and store a group receipt.
    async fn send_group_receipt(
        &self,
        group_id: &str,
        message_id: &str,
        status: ReceiptStatus,
    ) -> Result<GroupReadReceipt> {
        let timestamp = chrono::Utc::now().timestamp_millis();

        let mut receipt = GroupReadReceipt {
            message_id: message_id.to_string(),
            group_id: group_id.to_string(),
            reader_did: self.local_did.clone(),
            status: status.into(),
            timestamp,
            signature: vec![],
        };

        receipt.signature = self.sign_group_receipt(&receipt)?;

        self.storage.store_group_receipt(&receipt).await?;

        Ok(receipt)
    }

    /// Receive and store an inbound group receipt from a peer.
    ///
    /// The receipt arrives MLS-encrypted on the group's GossipSub topic,
    /// so authenticity is guaranteed by MLS. Signature verification is an
    /// additional layer for non-repudiation.
    pub async fn receive_group_receipt(&self, receipt: GroupReadReceipt) -> Result<()> {
        self.storage.store_group_receipt(&receipt).await?;
        Ok(())
    }

    /// Get all receipts for a specific group message (all members).
    pub async fn get_group_receipts(
        &self,
        group_id: &str,
        message_id: &str,
    ) -> Result<Vec<GroupReadReceipt>> {
        self.storage
            .fetch_group_receipts(group_id, message_id)
            .await
    }

    /// Compute the aggregate receipt status for a group message.
    ///
    /// Returns the minimum status across all provided members (Signal-style):
    /// - If any member has no receipt → "sent"
    /// - If all members have at least DELIVERED → "delivered"
    /// - If all members have READ → "read"
    ///
    /// The caller should pass only the *other* members (excluding the sender).
    pub async fn get_group_aggregate_status(
        &self,
        group_id: &str,
        message_id: &str,
        member_dids: &[String],
    ) -> Result<String> {
        if member_dids.is_empty() {
            return Ok("sent".to_string());
        }

        let receipts = self
            .storage
            .fetch_group_receipts(group_id, message_id)
            .await?;

        // Build a map of reader_did → highest status
        let mut status_map: std::collections::HashMap<&str, i32> = std::collections::HashMap::new();

        for r in &receipts {
            let entry = status_map.entry(&r.reader_did).or_insert(0);
            if r.status > *entry {
                *entry = r.status;
            }
        }

        // Find the minimum status across all members.
        // Members with no receipt at all count as 0 (unspecified → "sent").
        let min_status = member_dids
            .iter()
            .map(|did| status_map.get(did.as_str()).copied().unwrap_or(0))
            .min()
            .unwrap_or(0);

        let status_str = if min_status >= ReceiptStatus::Read as i32 {
            "read"
        } else if min_status >= ReceiptStatus::Delivered as i32 {
            "delivered"
        } else {
            "sent"
        };

        Ok(status_str.to_string())
    }

    /// Sign a group receipt.
    fn sign_group_receipt(&self, receipt: &GroupReadReceipt) -> Result<Vec<u8>> {
        let mut data = Vec::new();
        data.extend_from_slice(receipt.message_id.as_bytes());
        data.extend_from_slice(receipt.group_id.as_bytes());
        data.extend_from_slice(receipt.reader_did.as_bytes());
        data.extend_from_slice(&receipt.status.to_le_bytes());
        data.extend_from_slice(&receipt.timestamp.to_le_bytes());

        let signature = self.signing_key.sign(&data);
        Ok(signature.to_bytes().to_vec())
    }

    /// Verify a group receipt signature.
    pub fn verify_group_receipt_with_key(
        &self,
        receipt: &GroupReadReceipt,
        reader_public_key: &VerifyingKey,
    ) -> Result<()> {
        let mut data = Vec::new();
        data.extend_from_slice(receipt.message_id.as_bytes());
        data.extend_from_slice(receipt.group_id.as_bytes());
        data.extend_from_slice(receipt.reader_did.as_bytes());
        data.extend_from_slice(&receipt.status.to_le_bytes());
        data.extend_from_slice(&receipt.timestamp.to_le_bytes());

        let signature =
            Signature::from_bytes(receipt.signature.as_slice().try_into().map_err(|_| {
                Error::InvalidSignature {
                    message_id: receipt.message_id.clone(),
                }
            })?);

        reader_public_key
            .verify(&data, &signature)
            .map_err(|_| Error::InvalidSignature {
                message_id: receipt.message_id.clone(),
            })?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::LocalMessageStorage;
    use rand_core::OsRng;
    use tempfile::tempdir;
    use ulid::Ulid;

    #[tokio::test]
    async fn test_create_handler() {
        let dir = tempdir().unwrap();
        let storage = Arc::new(LocalMessageStorage::new(dir.path()).unwrap());

        let signing_key = SigningKey::generate(&mut OsRng);
        let handler = ReceiptHandler::new("did:variance:alice".to_string(), signing_key, storage);

        assert_eq!(handler.local_did, "did:variance:alice");
    }

    #[tokio::test]
    async fn test_send_delivered() {
        let dir = tempdir().unwrap();
        let storage = Arc::new(LocalMessageStorage::new(dir.path()).unwrap());

        let signing_key = SigningKey::generate(&mut OsRng);
        let handler = ReceiptHandler::new("did:variance:alice".to_string(), signing_key, storage);

        let message_id = Ulid::new().to_string();
        let receipt = handler.send_delivered(message_id.clone()).await.unwrap();

        assert_eq!(receipt.message_id, message_id);
        assert_eq!(receipt.reader_did, "did:variance:alice");
        assert_eq!(receipt.status, ReceiptStatus::Delivered as i32);
        assert!(!receipt.signature.is_empty());
    }

    #[tokio::test]
    async fn test_send_read() {
        let dir = tempdir().unwrap();
        let storage = Arc::new(LocalMessageStorage::new(dir.path()).unwrap());

        let signing_key = SigningKey::generate(&mut OsRng);
        let handler = ReceiptHandler::new("did:variance:alice".to_string(), signing_key, storage);

        let message_id = Ulid::new().to_string();
        let receipt = handler.send_read(message_id.clone()).await.unwrap();

        assert_eq!(receipt.message_id, message_id);
        assert_eq!(receipt.status, ReceiptStatus::Read as i32);
    }

    #[tokio::test]
    async fn test_receipt_verification_success() {
        let dir = tempdir().unwrap();
        let storage = Arc::new(LocalMessageStorage::new(dir.path()).unwrap());

        let signing_key = SigningKey::generate(&mut OsRng);
        let verifying_key = signing_key.verifying_key();

        let handler = ReceiptHandler::new("did:variance:alice".to_string(), signing_key, storage);

        let message_id = Ulid::new().to_string();
        let receipt = handler.send_delivered(message_id).await.unwrap();

        // Verify with correct key
        assert!(handler
            .verify_receipt_with_key(&receipt, &verifying_key)
            .is_ok());
    }

    #[tokio::test]
    async fn test_receipt_verification_failure() {
        let dir = tempdir().unwrap();
        let storage = Arc::new(LocalMessageStorage::new(dir.path()).unwrap());

        let signing_key = SigningKey::generate(&mut OsRng);
        let wrong_key = SigningKey::generate(&mut OsRng).verifying_key();

        let handler = ReceiptHandler::new("did:variance:alice".to_string(), signing_key, storage);

        let message_id = Ulid::new().to_string();
        let receipt = handler.send_delivered(message_id).await.unwrap();

        // Verify with wrong key should fail
        assert!(handler
            .verify_receipt_with_key(&receipt, &wrong_key)
            .is_err());
    }

    #[tokio::test]
    async fn test_get_receipts() {
        let dir = tempdir().unwrap();
        let storage = Arc::new(LocalMessageStorage::new(dir.path()).unwrap());

        let signing_key = SigningKey::generate(&mut OsRng);
        let handler = ReceiptHandler::new("did:variance:alice".to_string(), signing_key, storage);

        let message_id = Ulid::new().to_string();

        // Send delivered receipt
        handler.send_delivered(message_id.clone()).await.unwrap();

        // Wait a moment to ensure different timestamps
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

        // Send read receipt
        handler.send_read(message_id.clone()).await.unwrap();

        // Fetch all receipts for message
        let receipts = handler.get_receipts(&message_id).await.unwrap();

        assert_eq!(receipts.len(), 2);
    }

    #[tokio::test]
    async fn test_get_receipt_status() {
        let dir = tempdir().unwrap();
        let storage = Arc::new(LocalMessageStorage::new(dir.path()).unwrap());

        let signing_key = SigningKey::generate(&mut OsRng);
        let handler = ReceiptHandler::new("did:variance:alice".to_string(), signing_key, storage);

        let message_id = Ulid::new().to_string();

        // Send delivered receipt
        handler.send_delivered(message_id.clone()).await.unwrap();

        // Wait a moment
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

        // Send read receipt
        handler.send_read(message_id.clone()).await.unwrap();

        // Fetch latest status for alice
        let status = handler
            .get_receipt_status(&message_id, "did:variance:alice")
            .await
            .unwrap();

        assert!(status.is_some());
        let receipt = status.unwrap();
        assert_eq!(receipt.status, ReceiptStatus::Read as i32);
    }

    #[tokio::test]
    async fn test_receive_receipt() {
        let dir = tempdir().unwrap();
        let storage = Arc::new(LocalMessageStorage::new(dir.path()).unwrap());

        let signing_key = SigningKey::generate(&mut OsRng);
        let handler = ReceiptHandler::new(
            "did:variance:alice".to_string(),
            signing_key,
            storage.clone(),
        );

        let message_id = Ulid::new().to_string();

        // Create receipt from another user
        let receipt = ReadReceipt {
            message_id: message_id.clone(),
            reader_did: "did:variance:bob".to_string(),
            status: ReceiptStatus::Read as i32,
            timestamp: chrono::Utc::now().timestamp_millis(),
            signature: vec![1, 2, 3],
        };

        // Receive receipt
        handler.receive_receipt(receipt.clone()).await.unwrap();

        // Verify it was stored
        let receipts = storage.fetch_receipts(&message_id).await.unwrap();
        assert_eq!(receipts.len(), 1);
        assert_eq!(receipts[0].reader_did, "did:variance:bob");
    }

    // ===== Group receipt handler tests =====

    #[tokio::test]
    async fn test_send_group_delivered() {
        let dir = tempdir().unwrap();
        let storage = Arc::new(LocalMessageStorage::new(dir.path()).unwrap());

        let signing_key = SigningKey::generate(&mut OsRng);
        let handler = ReceiptHandler::new("did:variance:alice".to_string(), signing_key, storage);

        let receipt = handler
            .send_group_delivered("group-1", "msg-001")
            .await
            .unwrap();

        assert_eq!(receipt.message_id, "msg-001");
        assert_eq!(receipt.group_id, "group-1");
        assert_eq!(receipt.reader_did, "did:variance:alice");
        assert_eq!(receipt.status, ReceiptStatus::Delivered as i32);
        assert!(!receipt.signature.is_empty());
    }

    #[tokio::test]
    async fn test_send_group_read() {
        let dir = tempdir().unwrap();
        let storage = Arc::new(LocalMessageStorage::new(dir.path()).unwrap());

        let signing_key = SigningKey::generate(&mut OsRng);
        let handler = ReceiptHandler::new("did:variance:alice".to_string(), signing_key, storage);

        let receipt = handler.send_group_read("group-1", "msg-001").await.unwrap();

        assert_eq!(receipt.status, ReceiptStatus::Read as i32);
    }

    #[tokio::test]
    async fn test_group_receipt_verification() {
        let dir = tempdir().unwrap();
        let storage = Arc::new(LocalMessageStorage::new(dir.path()).unwrap());

        let signing_key = SigningKey::generate(&mut OsRng);
        let verifying_key = signing_key.verifying_key();

        let handler = ReceiptHandler::new("did:variance:alice".to_string(), signing_key, storage);

        let receipt = handler
            .send_group_delivered("group-1", "msg-001")
            .await
            .unwrap();

        // Correct key succeeds
        assert!(handler
            .verify_group_receipt_with_key(&receipt, &verifying_key)
            .is_ok());

        // Wrong key fails
        let wrong_key = SigningKey::generate(&mut OsRng).verifying_key();
        assert!(handler
            .verify_group_receipt_with_key(&receipt, &wrong_key)
            .is_err());
    }

    #[tokio::test]
    async fn test_receive_group_receipt() {
        let dir = tempdir().unwrap();
        let storage = Arc::new(LocalMessageStorage::new(dir.path()).unwrap());

        let signing_key = SigningKey::generate(&mut OsRng);
        let handler = ReceiptHandler::new(
            "did:variance:alice".to_string(),
            signing_key,
            storage.clone(),
        );

        let receipt = GroupReadReceipt {
            message_id: "msg-001".to_string(),
            group_id: "group-1".to_string(),
            reader_did: "did:variance:bob".to_string(),
            status: ReceiptStatus::Delivered as i32,
            timestamp: chrono::Utc::now().timestamp_millis(),
            signature: vec![1, 2, 3],
        };

        handler.receive_group_receipt(receipt).await.unwrap();

        let stored = storage
            .fetch_group_receipts("group-1", "msg-001")
            .await
            .unwrap();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].reader_did, "did:variance:bob");
    }

    #[tokio::test]
    async fn test_group_aggregate_status_all_read() {
        let dir = tempdir().unwrap();
        let storage = Arc::new(LocalMessageStorage::new(dir.path()).unwrap());

        let signing_key = SigningKey::generate(&mut OsRng);
        let handler = ReceiptHandler::new("did:variance:alice".to_string(), signing_key, storage);

        // Both bob and charlie have read
        handler
            .receive_group_receipt(GroupReadReceipt {
                message_id: "msg-001".to_string(),
                group_id: "group-1".to_string(),
                reader_did: "did:variance:bob".to_string(),
                status: ReceiptStatus::Read as i32,
                timestamp: 2000,
                signature: vec![],
            })
            .await
            .unwrap();
        handler
            .receive_group_receipt(GroupReadReceipt {
                message_id: "msg-001".to_string(),
                group_id: "group-1".to_string(),
                reader_did: "did:variance:charlie".to_string(),
                status: ReceiptStatus::Read as i32,
                timestamp: 2001,
                signature: vec![],
            })
            .await
            .unwrap();

        let members = vec![
            "did:variance:bob".to_string(),
            "did:variance:charlie".to_string(),
        ];
        let status = handler
            .get_group_aggregate_status("group-1", "msg-001", &members)
            .await
            .unwrap();
        assert_eq!(status, "read");
    }

    #[tokio::test]
    async fn test_group_aggregate_status_partial_delivery() {
        let dir = tempdir().unwrap();
        let storage = Arc::new(LocalMessageStorage::new(dir.path()).unwrap());

        let signing_key = SigningKey::generate(&mut OsRng);
        let handler = ReceiptHandler::new("did:variance:alice".to_string(), signing_key, storage);

        // Bob delivered, charlie has no receipt yet
        handler
            .receive_group_receipt(GroupReadReceipt {
                message_id: "msg-002".to_string(),
                group_id: "group-1".to_string(),
                reader_did: "did:variance:bob".to_string(),
                status: ReceiptStatus::Delivered as i32,
                timestamp: 1000,
                signature: vec![],
            })
            .await
            .unwrap();

        let members = vec![
            "did:variance:bob".to_string(),
            "did:variance:charlie".to_string(),
        ];
        let status = handler
            .get_group_aggregate_status("group-1", "msg-002", &members)
            .await
            .unwrap();
        // Charlie has no receipt → minimum is "sent"
        assert_eq!(status, "sent");
    }

    #[tokio::test]
    async fn test_group_aggregate_status_all_delivered() {
        let dir = tempdir().unwrap();
        let storage = Arc::new(LocalMessageStorage::new(dir.path()).unwrap());

        let signing_key = SigningKey::generate(&mut OsRng);
        let handler = ReceiptHandler::new("did:variance:alice".to_string(), signing_key, storage);

        // Both delivered, bob also read — minimum is still "delivered"
        handler
            .receive_group_receipt(GroupReadReceipt {
                message_id: "msg-003".to_string(),
                group_id: "group-1".to_string(),
                reader_did: "did:variance:bob".to_string(),
                status: ReceiptStatus::Read as i32,
                timestamp: 2000,
                signature: vec![],
            })
            .await
            .unwrap();
        handler
            .receive_group_receipt(GroupReadReceipt {
                message_id: "msg-003".to_string(),
                group_id: "group-1".to_string(),
                reader_did: "did:variance:charlie".to_string(),
                status: ReceiptStatus::Delivered as i32,
                timestamp: 1500,
                signature: vec![],
            })
            .await
            .unwrap();

        let members = vec![
            "did:variance:bob".to_string(),
            "did:variance:charlie".to_string(),
        ];
        let status = handler
            .get_group_aggregate_status("group-1", "msg-003", &members)
            .await
            .unwrap();
        assert_eq!(status, "delivered");
    }

    #[tokio::test]
    async fn test_group_aggregate_status_empty_members() {
        let dir = tempdir().unwrap();
        let storage = Arc::new(LocalMessageStorage::new(dir.path()).unwrap());

        let signing_key = SigningKey::generate(&mut OsRng);
        let handler = ReceiptHandler::new("did:variance:alice".to_string(), signing_key, storage);

        let status = handler
            .get_group_aggregate_status("group-1", "msg-004", &[])
            .await
            .unwrap();
        assert_eq!(status, "sent");
    }

    // ===== GroupPayload encode/decode + legacy fallback tests =====

    #[test]
    fn test_group_payload_message_roundtrip() {
        use prost::Message;
        use variance_proto::messaging_proto::{group_payload, GroupPayload, MessageContent};

        let content = MessageContent {
            text: "hello group".to_string(),
            reply_to: Some("msg-parent".to_string()),
            ..Default::default()
        };

        let payload = GroupPayload {
            payload: Some(group_payload::Payload::Message(content.clone())),
        };

        let bytes = payload.encode_to_vec();
        let decoded = GroupPayload::decode(bytes.as_slice()).unwrap();

        match decoded.payload {
            Some(group_payload::Payload::Message(msg)) => {
                assert_eq!(msg.text, "hello group");
                assert_eq!(msg.reply_to, Some("msg-parent".to_string()));
            }
            other => panic!("Expected Payload::Message, got {:?}", other),
        }
    }

    #[test]
    fn test_group_payload_receipt_roundtrip() {
        use prost::Message;
        use variance_proto::messaging_proto::{group_payload, GroupPayload};

        let receipt = GroupReadReceipt {
            message_id: "msg-200".to_string(),
            group_id: "group-42".to_string(),
            reader_did: "did:variance:bob".to_string(),
            status: ReceiptStatus::Read as i32,
            timestamp: 9999999999,
            signature: vec![0xDE, 0xAD, 0xBE, 0xEF],
        };

        let payload = GroupPayload {
            payload: Some(group_payload::Payload::Receipt(receipt)),
        };

        let bytes = payload.encode_to_vec();
        let decoded = GroupPayload::decode(bytes.as_slice()).unwrap();

        match decoded.payload {
            Some(group_payload::Payload::Receipt(r)) => {
                assert_eq!(r.message_id, "msg-200");
                assert_eq!(r.group_id, "group-42");
                assert_eq!(r.reader_did, "did:variance:bob");
                assert_eq!(r.status, ReceiptStatus::Read as i32);
                assert_eq!(r.signature, vec![0xDE, 0xAD, 0xBE, 0xEF]);
            }
            other => panic!("Expected Payload::Receipt, got {:?}", other),
        }
    }

    #[test]
    fn test_legacy_message_content_fallback() {
        use prost::Message;
        use variance_proto::messaging_proto::{GroupPayload, MessageContent};

        // Simulate a legacy message: bare MessageContent (not wrapped in GroupPayload).
        let content = MessageContent {
            text: "legacy message".to_string(),
            reply_to: Some("old-ref".to_string()),
            ..Default::default()
        };
        let legacy_bytes = content.encode_to_vec();

        // Try GroupPayload first — this may succeed with garbage data due to
        // protobuf's permissive decoding, but the oneof payload will be None
        // or contain unexpected data. The correct pattern: try GroupPayload,
        // check if payload is valid, fall back to bare MessageContent.
        let gp_attempt = GroupPayload::decode(legacy_bytes.as_slice());

        // Either decode fails or payload is None/unexpected — fall back.
        let result = match gp_attempt.ok().and_then(|p| p.payload) {
            Some(variance_proto::messaging_proto::group_payload::Payload::Message(msg)) => msg,
            Some(variance_proto::messaging_proto::group_payload::Payload::Receipt(_)) => {
                // Unexpected receipt from legacy data — fall back
                MessageContent::decode(legacy_bytes.as_slice()).unwrap()
            }
            None => {
                // No payload — decode as bare MessageContent (legacy)
                MessageContent::decode(legacy_bytes.as_slice()).unwrap()
            }
        };

        assert_eq!(result.text, "legacy message");
        assert_eq!(result.reply_to, Some("old-ref".to_string()));
    }

    #[test]
    fn test_group_payload_distinguishes_message_from_receipt() {
        use prost::Message;
        use variance_proto::messaging_proto::{group_payload, GroupPayload, MessageContent};

        // Encode a message payload
        let msg_payload = GroupPayload {
            payload: Some(group_payload::Payload::Message(MessageContent {
                text: "a message".to_string(),
                ..Default::default()
            })),
        };
        let msg_bytes = msg_payload.encode_to_vec();

        // Encode a receipt payload
        let rcpt_payload = GroupPayload {
            payload: Some(group_payload::Payload::Receipt(GroupReadReceipt {
                message_id: "msg-1".to_string(),
                group_id: "g-1".to_string(),
                reader_did: "did:bob".to_string(),
                status: ReceiptStatus::Delivered as i32,
                timestamp: 12345,
                signature: vec![],
            })),
        };
        let rcpt_bytes = rcpt_payload.encode_to_vec();

        // Decode and verify they're distinguished correctly
        let decoded_msg = GroupPayload::decode(msg_bytes.as_slice()).unwrap();
        assert!(matches!(
            decoded_msg.payload,
            Some(group_payload::Payload::Message(_))
        ));

        let decoded_rcpt = GroupPayload::decode(rcpt_bytes.as_slice()).unwrap();
        assert!(matches!(
            decoded_rcpt.payload,
            Some(group_payload::Payload::Receipt(_))
        ));

        // They must not be the same variant
        assert_ne!(msg_bytes, rcpt_bytes);
    }
}
