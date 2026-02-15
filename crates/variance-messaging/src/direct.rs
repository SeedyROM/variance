use crate::error::*;
use crate::storage::MessageStorage;
use double_ratchet_2::ratchet::Ratchet;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use prost::Message;
use rand::rngs::OsRng;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use ulid::Ulid;
use variance_proto::messaging_proto::{DirectMessage, MessageContent, MessageType};
use x25519_dalek::{PublicKey, StaticSecret};

/// Direct message handler
///
/// Manages 1-on-1 encrypted conversations using Double Ratchet protocol.
/// Each conversation has its own ratchet state for forward secrecy.
pub struct DirectMessageHandler {
    /// Local DID
    local_did: String,

    /// Signing key for message authentication
    signing_key: SigningKey,

    /// Ratchet sessions indexed by conversation partner DID
    sessions: Arc<RwLock<HashMap<String, Ratchet<StaticSecret>>>>,

    /// Message storage backend
    storage: Arc<dyn MessageStorage>,
}

impl DirectMessageHandler {
    /// Create a new direct message handler
    pub fn new(
        local_did: String,
        signing_key: SigningKey,
        storage: Arc<dyn MessageStorage>,
    ) -> Self {
        Self {
            local_did,
            signing_key,
            sessions: Arc::new(RwLock::new(HashMap::new())),
            storage,
        }
    }

    /// Initialize a new ratchet session as initiator (Alice)
    ///
    /// This is called when starting a new conversation.
    /// Alice knows Bob's public key and establishes the first session.
    /// Note: Alice must send the first message.
    pub async fn init_session_as_initiator(
        &self,
        recipient_did: String,
        recipient_public_key: PublicKey,
    ) -> Result<()> {
        let local_secret = StaticSecret::random_from_rng(OsRng);
        let shared_secret = local_secret.diffie_hellman(&recipient_public_key);

        let ratchet = Ratchet::init_alice(*shared_secret.as_bytes(), recipient_public_key);

        let mut sessions = self.sessions.write().await;
        sessions.insert(recipient_did, ratchet);

        Ok(())
    }

    /// Initialize a new ratchet session as responder (Bob)
    ///
    /// This is called when receiving the first message in a conversation.
    /// Returns Bob's public key which should be shared with Alice.
    pub async fn init_session_as_responder(
        &self,
        sender_did: String,
        sender_public_key: PublicKey,
    ) -> Result<PublicKey> {
        let local_secret = StaticSecret::random_from_rng(OsRng);
        let shared_secret = local_secret.diffie_hellman(&sender_public_key);

        let (ratchet, bob_public_key) = Ratchet::init_bob(*shared_secret.as_bytes());

        let mut sessions = self.sessions.write().await;
        sessions.insert(sender_did, ratchet);

        Ok(bob_public_key)
    }

    /// Send a direct message
    pub async fn send_message(
        &self,
        recipient_did: String,
        content: MessageContent,
    ) -> Result<DirectMessage> {
        // Get or create session
        let mut sessions = self.sessions.write().await;
        let ratchet = sessions.get_mut(&recipient_did).ok_or_else(|| {
            Error::DoubleRatchet {
                message: format!("No session with {}", recipient_did),
            }
        })?;

        // Serialize content using protobuf
        let plaintext = prost::Message::encode_to_vec(&content);

        // Encrypt with Double Ratchet (associated data is empty)
        let (header, ciphertext, nonce) = ratchet.ratchet_encrypt(&plaintext, b"");

        // Generate ULID for message ID
        let id = Ulid::new().to_string();
        let timestamp = chrono::Utc::now().timestamp_millis();

        // Combine header and nonce for storage (since protobuf only has one nonce field)
        let mut header_and_nonce = bincode::serialize(&header).map_err(|e| Error::Crypto {
            message: format!("Header serialization failed: {}", e),
        })?;
        header_and_nonce.extend_from_slice(&nonce);

        // Create message
        let mut message = DirectMessage {
            id: id.clone(),
            sender_did: self.local_did.clone(),
            recipient_did: recipient_did.clone(),
            ciphertext,
            nonce: header_and_nonce,
            signature: vec![],
            timestamp,
            r#type: Self::infer_message_type(&content),
            reply_to: content.reply_to.clone(),
        };

        // Sign message
        message.signature = self.sign_message(&message)?;

        // Store message
        self.storage.store_direct(&message).await?;

        Ok(message)
    }

    /// Receive and decrypt a direct message
    ///
    /// NOTE: Caller must verify message signature using verify_message_with_key()
    /// before calling this, passing the sender's public key from their DID document.
    pub async fn receive_message(&self, message: DirectMessage) -> Result<MessageContent> {
        // Get or create session
        let mut sessions = self.sessions.write().await;
        let ratchet = sessions.get_mut(&message.sender_did).ok_or_else(|| {
            Error::DoubleRatchet {
                message: format!("No session with {}", message.sender_did),
            }
        })?;

        // Split header and nonce from storage field
        // Header size for double-ratchet-2 is variable, nonce is 12 bytes
        if message.nonce.len() < 12 {
            return Err(Error::InvalidFormat {
                message: "Nonce field too short".to_string(),
            });
        }

        let nonce_start = message.nonce.len() - 12;
        let header_bytes = &message.nonce[..nonce_start];
        let nonce_slice = &message.nonce[nonce_start..];

        // Convert nonce slice to fixed-size array
        let nonce: &[u8; 12] = nonce_slice
            .try_into()
            .map_err(|_| Error::InvalidFormat {
                message: "Invalid nonce size".to_string(),
            })?;

        let header = bincode::deserialize(header_bytes).map_err(|e| Error::Crypto {
            message: format!("Header deserialization failed: {}", e),
        })?;

        // Decrypt with Double Ratchet (returns Vec<u8>, not Result)
        let plaintext = ratchet.ratchet_decrypt(&header, &message.ciphertext, nonce, b"");

        // Deserialize content using protobuf
        let content =
            MessageContent::decode(plaintext.as_slice()).map_err(|e| Error::Protocol { source: e })?;

        // Store message
        self.storage.store_direct(&message).await?;

        Ok(content)
    }

    /// Sign a message
    fn sign_message(&self, message: &DirectMessage) -> Result<Vec<u8>> {
        let mut data = Vec::new();
        data.extend_from_slice(message.id.as_bytes());
        data.extend_from_slice(message.sender_did.as_bytes());
        data.extend_from_slice(message.recipient_did.as_bytes());
        data.extend_from_slice(&message.ciphertext);
        data.extend_from_slice(&message.nonce);
        data.extend_from_slice(&message.timestamp.to_le_bytes());

        let signature = self.signing_key.sign(&data);
        Ok(signature.to_bytes().to_vec())
    }

    /// Verify a message signature
    ///
    /// NOTE: This requires the sender's public key which must be fetched from their
    /// DID document via the identity system. Currently verification is deferred to
    /// the caller who must provide the sender's public key.
    fn verify_message_with_key(
        &self,
        message: &DirectMessage,
        sender_public_key: &VerifyingKey,
    ) -> Result<()> {
        let mut data = Vec::new();
        data.extend_from_slice(message.id.as_bytes());
        data.extend_from_slice(message.sender_did.as_bytes());
        data.extend_from_slice(message.recipient_did.as_bytes());
        data.extend_from_slice(&message.ciphertext);
        data.extend_from_slice(&message.nonce);
        data.extend_from_slice(&message.timestamp.to_le_bytes());

        let signature = Signature::from_bytes(
            message
                .signature
                .as_slice()
                .try_into()
                .map_err(|_| Error::InvalidSignature {
                    message_id: message.id.clone(),
                })?,
        );

        sender_public_key
            .verify(&data, &signature)
            .map_err(|_| Error::InvalidSignature {
                message_id: message.id.clone(),
            })?;

        Ok(())
    }

    /// Infer message type from content
    fn infer_message_type(content: &MessageContent) -> i32 {
        if !content.attachments.is_empty() {
            let first = &content.attachments[0];
            match first.r#type {
                1 => MessageType::Image.into(),
                2 => MessageType::File.into(),
                3 => MessageType::Audio.into(),
                4 => MessageType::Video.into(),
                _ => MessageType::Text.into(),
            }
        } else {
            MessageType::Text.into()
        }
    }

    /// Fetch conversation history
    pub async fn get_conversation(
        &self,
        peer_did: &str,
        limit: usize,
        before: Option<String>,
    ) -> Result<Vec<DirectMessage>> {
        self.storage
            .fetch_direct(&self.local_did, peer_did, limit, before)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::LocalMessageStorage;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_create_handler() {
        let dir = tempdir().unwrap();
        let storage = Arc::new(LocalMessageStorage::new(dir.path()).unwrap());

        let signing_key = SigningKey::generate(&mut OsRng);
        let handler = DirectMessageHandler::new(
            "did:variance:alice".to_string(),
            signing_key,
            storage,
        );

        assert_eq!(handler.local_did, "did:variance:alice");
    }

    #[tokio::test]
    async fn test_message_signing() {
        let dir = tempdir().unwrap();
        let storage = Arc::new(LocalMessageStorage::new(dir.path()).unwrap());

        let signing_key = SigningKey::generate(&mut OsRng);
        let handler = DirectMessageHandler::new(
            "did:variance:alice".to_string(),
            signing_key,
            storage,
        );

        let message = DirectMessage {
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

        let signature = handler.sign_message(&message).unwrap();
        assert_eq!(signature.len(), 64); // Ed25519 signature size
    }

    #[tokio::test]
    async fn test_infer_message_type() {
        let content = MessageContent {
            text: "Hello".to_string(),
            attachments: vec![],
            mentions: vec![],
            reply_to: None,
            metadata: HashMap::new(),
        };

        let msg_type = DirectMessageHandler::infer_message_type(&content);
        assert_eq!(msg_type, MessageType::Text as i32);
    }

    #[tokio::test]
    async fn test_message_verification_success() {
        let dir = tempdir().unwrap();
        let storage = Arc::new(LocalMessageStorage::new(dir.path()).unwrap());

        let signing_key = SigningKey::generate(&mut OsRng);
        let verifying_key = signing_key.verifying_key();

        let handler = DirectMessageHandler::new(
            "did:variance:alice".to_string(),
            signing_key,
            storage,
        );

        let mut message = DirectMessage {
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

        // Sign message
        message.signature = handler.sign_message(&message).unwrap();

        // Verify with correct key
        assert!(handler
            .verify_message_with_key(&message, &verifying_key)
            .is_ok());
    }

    #[tokio::test]
    async fn test_message_verification_failure() {
        let dir = tempdir().unwrap();
        let storage = Arc::new(LocalMessageStorage::new(dir.path()).unwrap());

        let signing_key = SigningKey::generate(&mut OsRng);
        let wrong_key = SigningKey::generate(&mut OsRng).verifying_key();

        let handler = DirectMessageHandler::new(
            "did:variance:alice".to_string(),
            signing_key,
            storage,
        );

        let mut message = DirectMessage {
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

        // Sign message
        message.signature = handler.sign_message(&message).unwrap();

        // Verify with wrong key should fail
        assert!(handler
            .verify_message_with_key(&message, &wrong_key)
            .is_err());
    }

    #[tokio::test]
    async fn test_message_verification_invalid_signature() {
        let dir = tempdir().unwrap();
        let storage = Arc::new(LocalMessageStorage::new(dir.path()).unwrap());

        let signing_key = SigningKey::generate(&mut OsRng);
        let verifying_key = signing_key.verifying_key();

        let handler = DirectMessageHandler::new(
            "did:variance:alice".to_string(),
            signing_key,
            storage,
        );

        let message = DirectMessage {
            id: Ulid::new().to_string(),
            sender_did: "did:variance:alice".to_string(),
            recipient_did: "did:variance:bob".to_string(),
            ciphertext: vec![1, 2, 3],
            nonce: vec![4, 5, 6],
            signature: vec![0; 64], // Invalid signature
            timestamp: chrono::Utc::now().timestamp_millis(),
            r#type: MessageType::Text.into(),
            reply_to: None,
        };

        // Verify with invalid signature should fail
        let result = handler.verify_message_with_key(&message, &verifying_key);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), Error::InvalidSignature { .. }));
    }
}
