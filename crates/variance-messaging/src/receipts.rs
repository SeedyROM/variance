use crate::error::*;
use crate::storage::MessageStorage;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use std::sync::Arc;
use variance_proto::messaging_proto::{ReadReceipt, ReceiptStatus};

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
    async fn send_receipt(
        &self,
        message_id: String,
        status: ReceiptStatus,
    ) -> Result<ReadReceipt> {
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

        let signature = Signature::from_bytes(
            receipt
                .signature
                .as_slice()
                .try_into()
                .map_err(|_| Error::InvalidSignature {
                    message_id: receipt.message_id.clone(),
                })?,
        );

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
    use rand::rngs::OsRng;
    use tempfile::tempdir;
    use ulid::Ulid;

    #[tokio::test]
    async fn test_create_handler() {
        let dir = tempdir().unwrap();
        let storage = Arc::new(LocalMessageStorage::new(dir.path()).unwrap());

        let signing_key = SigningKey::generate(&mut OsRng);
        let handler = ReceiptHandler::new(
            "did:variance:alice".to_string(),
            signing_key,
            storage,
        );

        assert_eq!(handler.local_did, "did:variance:alice");
    }

    #[tokio::test]
    async fn test_send_delivered() {
        let dir = tempdir().unwrap();
        let storage = Arc::new(LocalMessageStorage::new(dir.path()).unwrap());

        let signing_key = SigningKey::generate(&mut OsRng);
        let handler = ReceiptHandler::new(
            "did:variance:alice".to_string(),
            signing_key,
            storage,
        );

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
        let handler = ReceiptHandler::new(
            "did:variance:alice".to_string(),
            signing_key,
            storage,
        );

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

        let handler = ReceiptHandler::new(
            "did:variance:alice".to_string(),
            signing_key,
            storage,
        );

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

        let handler = ReceiptHandler::new(
            "did:variance:alice".to_string(),
            signing_key,
            storage,
        );

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
        let handler = ReceiptHandler::new(
            "did:variance:alice".to_string(),
            signing_key,
            storage,
        );

        let message_id = Ulid::new().to_string();

        // Send delivered receipt
        handler
            .send_delivered(message_id.clone())
            .await
            .unwrap();

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
        let handler = ReceiptHandler::new(
            "did:variance:alice".to_string(),
            signing_key,
            storage,
        );

        let message_id = Ulid::new().to_string();

        // Send delivered receipt
        handler
            .send_delivered(message_id.clone())
            .await
            .unwrap();

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
}
