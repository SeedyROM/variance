use crate::error::*;
use crate::storage::MessageStorage;
use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Nonce,
};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use hkdf::Hkdf;
use prost::Message;
use rand::RngCore;
use sha2::Sha256;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};
use ulid::Ulid;
use variance_proto::messaging_proto::{DirectMessage, MessageContent, MessageType};
use vodozemac::olm::{Account, OlmMessage, Session, SessionConfig};
use vodozemac::Curve25519PublicKey;

/// Special olm_message_type value indicating unencrypted self-message
/// (avoids Olm session conflicts when messaging yourself)
const OLM_MESSAGE_TYPE_SELF: u32 = 999;

/// Direct message handler
///
/// Manages 1-on-1 encrypted conversations using the Olm Double Ratchet protocol
/// (vodozemac implementation, as used by Matrix/Element).
pub struct DirectMessageHandler {
    /// Local DID
    local_did: String,

    /// Ed25519 signing key for message authentication
    signing_key: SigningKey,

    /// Olm account — manages identity keys and one-time pre-keys.
    /// Wrapped in RwLock because create_inbound_session() requires &mut.
    account: Arc<RwLock<Account>>,

    /// Cached identity key (never changes after account creation).
    identity_key: Curve25519PublicKey,

    /// Olm sessions indexed by conversation partner DID
    sessions: Arc<RwLock<HashMap<String, Session>>>,

    /// Message storage backend
    storage: Arc<dyn MessageStorage>,

    /// Serializes session initialization to prevent TOCTOU races.
    ///
    /// Two concurrent sends to the same peer with no existing session would
    /// otherwise both see no session, both create one, and the second would
    /// overwrite the first — wasting the OTK and corrupting send ordering.
    session_init_lock: Mutex<()>,

    /// AES-256-GCM key for at-rest encryption of decrypted message plaintext.
    ///
    /// Derived deterministically from the Ed25519 signing key via HKDF-SHA256
    /// so it can be rederived on any restart without storing it separately.
    /// Protects the `plaintext_cache` sled tree: a stolen DB file is unreadable
    /// without the identity file.
    storage_key: [u8; 32],
}

impl DirectMessageHandler {
    /// Create a new direct message handler from a vodozemac `Account`.
    pub fn new(
        local_did: String,
        signing_key: SigningKey,
        account: Account,
        storage: Arc<dyn MessageStorage>,
    ) -> Self {
        let identity_key = account.curve25519_key();

        // Derive a 32-byte AES-256-GCM key from the signing key.
        // Using a labeled HKDF expansion means this key is distinct from the
        // signing key itself and can't be used to forge signatures.
        let hk = Hkdf::<Sha256>::new(None, signing_key.as_bytes());
        let mut storage_key = [0u8; 32];
        hk.expand(b"variance-plaintext-storage-v1", &mut storage_key)
            .expect("HKDF expand with 32-byte output always succeeds");

        Self {
            local_did,
            signing_key,
            account: Arc::new(RwLock::new(account)),
            identity_key,
            sessions: Arc::new(RwLock::new(HashMap::new())),
            storage,
            storage_key,
            session_init_lock: Mutex::new(()),
        }
    }

    /// Return this node's Olm identity key (Curve25519).
    ///
    /// This key should be published in the DID document so peers can use it
    /// to establish Olm sessions.
    pub fn identity_key(&self) -> Curve25519PublicKey {
        self.identity_key
    }

    /// Generate new one-time pre-keys and return them for publication.
    ///
    /// Call `mark_one_time_keys_as_published` after distributing the keys.
    pub async fn generate_one_time_keys(&self, count: usize) {
        self.account.write().await.generate_one_time_keys(count);
    }

    /// Return currently unpublished one-time pre-keys.
    pub async fn one_time_keys(&self) -> HashMap<vodozemac::KeyId, Curve25519PublicKey> {
        self.account.read().await.one_time_keys()
    }

    /// Mark all pending one-time keys as published.
    pub async fn mark_one_time_keys_as_published(&self) {
        self.account.write().await.mark_keys_as_published();
    }

    /// Serialize the current Olm account state to a JSON pickle string.
    ///
    /// Must be called after `mark_one_time_keys_as_published` and the result
    /// written back to the identity file.  Without this, OTKs generated at
    /// startup are in-memory only: after a restart the account reverts to the
    /// initial (zero-OTK) state, making any pending PreKey messages that
    /// reference those keys impossible to decrypt on the recipient side.
    pub async fn account_pickle(&self) -> Result<String> {
        let pickle = self.account.read().await.pickle();
        serde_json::to_string(&pickle).map_err(|e| Error::Crypto {
            message: format!("Failed to serialize account pickle: {}", e),
        })
    }

    /// Encrypt `content` with AES-256-GCM and persist it under `message_id`.
    ///
    /// Format: random 12-byte nonce || GCM ciphertext (plaintext + 16-byte tag).
    /// This is called after every successful send or receive so history is
    /// readable across restarts without re-doing Olm decryption.
    async fn persist_plaintext(&self, message_id: &str, content: &MessageContent) -> Result<()> {
        let key = aes_gcm::Key::<Aes256Gcm>::from_slice(&self.storage_key);
        let cipher = Aes256Gcm::new(key);

        let mut nonce_bytes = [0u8; 12];
        rand::thread_rng().fill_bytes(&mut nonce_bytes);
        let nonce = Nonce::from_slice(&nonce_bytes);

        let plaintext = content.encode_to_vec();
        let ciphertext =
            cipher
                .encrypt(nonce, plaintext.as_slice())
                .map_err(|_| Error::Crypto {
                    message: "At-rest encryption failed".to_string(),
                })?;

        let mut blob = nonce_bytes.to_vec();
        blob.extend_from_slice(&ciphertext);

        self.storage.store_plaintext(message_id, &blob).await
    }

    /// Decrypt a blob previously written by `persist_plaintext`.
    async fn load_plaintext(&self, message_id: &str) -> Result<Option<MessageContent>> {
        let Some(blob) = self.storage.fetch_plaintext(message_id).await? else {
            return Ok(None);
        };

        if blob.len() < 12 {
            return Err(Error::Crypto {
                message: "Stored plaintext blob is too short".to_string(),
            });
        }

        let (nonce_bytes, ciphertext) = blob.split_at(12);
        let key = aes_gcm::Key::<Aes256Gcm>::from_slice(&self.storage_key);
        let cipher = Aes256Gcm::new(key);
        let nonce = Nonce::from_slice(nonce_bytes);

        let plaintext = cipher
            .decrypt(nonce, ciphertext)
            .map_err(|_| Error::Crypto {
                message: "At-rest decryption failed (wrong key or corrupted data)".to_string(),
            })?;

        let content = MessageContent::decode(plaintext.as_slice())
            .map_err(|e| Error::Protocol { source: e })?;

        Ok(Some(content))
    }

    /// Initialize a session as initiator (Alice).
    ///
    /// `recipient_identity_key` is the Curve25519 key from the peer's DID document.
    /// `recipient_one_time_key` is a one-time pre-key fetched from the peer's DID document.
    pub async fn init_session_as_initiator(
        &self,
        recipient_did: String,
        recipient_identity_key: Curve25519PublicKey,
        recipient_one_time_key: Curve25519PublicKey,
    ) -> Result<()> {
        let session = self.account.read().await.create_outbound_session(
            SessionConfig::version_2(),
            recipient_identity_key,
            recipient_one_time_key,
        );

        self.sessions
            .write()
            .await
            .insert(recipient_did.clone(), session);
        self.persist_session(&recipient_did).await?;
        Ok(())
    }

    /// Check if an Olm session already exists for the given peer DID.
    pub async fn has_session(&self, peer_did: &str) -> bool {
        self.sessions.read().await.contains_key(peer_did)
    }

    /// Restore all sessions from disk (called on startup).
    ///
    /// Loads pickled sessions from storage and deserializes them. Should be called
    /// immediately after creating the DirectMessageHandler to restore session state.
    pub async fn restore_sessions(&self) -> Result<()> {
        let session_pickles = self.storage.load_all_session_pickles().await?;
        let mut sessions = self.sessions.write().await;

        for (peer_did, pickle_json) in session_pickles {
            match serde_json::from_str::<vodozemac::olm::SessionPickle>(&pickle_json) {
                Ok(pickle) => {
                    let session = vodozemac::olm::Session::from_pickle(pickle);
                    sessions.insert(peer_did.clone(), session);
                    tracing::debug!("Restored Olm session for {}", peer_did);
                }
                Err(e) => {
                    tracing::warn!(
                        "Failed to restore session for {}: {} (pickle may be corrupted)",
                        peer_did,
                        e
                    );
                }
            }
        }

        tracing::info!("Restored {} Olm sessions from storage", sessions.len());
        Ok(())
    }

    /// Persist a session to disk after creation or modification.
    async fn persist_session(&self, peer_did: &str) -> Result<()> {
        let sessions = self.sessions.read().await;
        if let Some(session) = sessions.get(peer_did) {
            let pickle = session.pickle();
            let pickle_json = serde_json::to_string(&pickle).map_err(|e| Error::Crypto {
                message: format!("Failed to serialize session pickle: {}", e),
            })?;
            self.storage
                .store_session_pickle(peer_did, &pickle_json)
                .await?;
        }
        Ok(())
    }

    /// Initialize a session as initiator only if one doesn't already exist for this peer.
    ///
    /// Idempotent — safe to call before every outbound message. The mutex ensures
    /// that concurrent calls for the same peer are serialized: the second caller
    /// re-checks under the lock and bails out if the first already created it.
    pub async fn init_session_if_needed(
        &self,
        recipient_did: &str,
        recipient_identity_key: Curve25519PublicKey,
        recipient_one_time_key: Curve25519PublicKey,
    ) -> Result<()> {
        let _guard = self.session_init_lock.lock().await;

        // Re-check under the lock: a concurrent caller may have created it.
        if self.sessions.read().await.contains_key(recipient_did) {
            return Ok(());
        }

        self.init_session_as_initiator(
            recipient_did.to_string(),
            recipient_identity_key,
            recipient_one_time_key,
        )
        .await
    }

    /// Queue a message for later delivery when the peer comes online.
    ///
    /// The message should be fully encrypted and signed. It will be stored
    /// locally and automatically sent when the peer connects.
    pub async fn queue_pending_message(
        &self,
        recipient_did: &str,
        message: DirectMessage,
    ) -> Result<()> {
        self.storage
            .store_pending_message(recipient_did, &message)
            .await?;
        tracing::debug!(
            "Queued message {} for {} (peer offline)",
            message.id,
            recipient_did
        );
        Ok(())
    }

    /// Fetch all pending messages for a peer.
    ///
    /// Returns messages that were queued while the peer was offline,
    /// ready to be transmitted now that they're connected.
    pub async fn get_pending_messages(&self, peer_did: &str) -> Result<Vec<DirectMessage>> {
        self.storage.fetch_pending_messages(peer_did).await
    }

    /// Mark a pending message as successfully sent (delete from queue).
    pub async fn mark_pending_sent(&self, message_id: &str) -> Result<()> {
        self.storage.delete_pending_message(message_id).await
    }

    /// Get list of all peers with pending messages.
    pub async fn peers_with_pending_messages(&self) -> Result<Vec<String>> {
        self.storage.list_peers_with_pending_messages().await
    }

    /// Check if a message is currently in the pending queue.
    pub async fn is_message_pending(&self, message_id: &str) -> Result<bool> {
        self.storage.is_message_pending(message_id).await
    }

    /// Return the number of active sessions (for testing).
    #[cfg(test)]
    async fn session_count(&self) -> usize {
        self.sessions.read().await.len()
    }

    /// Send a direct message.
    ///
    /// The session must be initialized first via `init_session_as_initiator` or
    /// `init_session_if_needed`. The first encrypted message automatically carries
    /// the Olm PreKey payload needed by the recipient to establish their session.
    ///
    /// **Self-messaging:** If `recipient_did` equals `local_did`, the message is
    /// stored unencrypted (no Olm session needed) to avoid session conflicts.
    pub async fn send_message(
        &self,
        recipient_did: String,
        content: MessageContent,
    ) -> Result<DirectMessage> {
        // Self-messaging: bypass Olm encryption to avoid session conflicts
        // (you can't maintain separate inbound/outbound sessions with yourself)
        if recipient_did == self.local_did {
            return self.send_self_message(content).await;
        }

        let plaintext = prost::Message::encode_to_vec(&content);

        // Encrypt inside a scoped block so the sessions write lock is released
        // before we call persist_session (which needs its own read lock).
        let (olm_message_type, ciphertext, sender_identity_key) = {
            let mut sessions = self.sessions.write().await;
            let session = sessions
                .get_mut(&recipient_did)
                .ok_or_else(|| Error::DoubleRatchet {
                    message: format!("No session with {}", recipient_did),
                })?;

            let olm_message = session.encrypt(&plaintext);

            // Include our identity key on PreKey messages so the recipient can call
            // create_inbound_session() without a separate key lookup.
            let sender_identity_key = match &olm_message {
                OlmMessage::PreKey(_) => Some(self.identity_key.to_vec()),
                OlmMessage::Normal(_) => None,
            };

            let (msg_type, bytes) = olm_message.to_parts();
            (msg_type, bytes, sender_identity_key)
        }; // sessions write lock released here

        let id = Ulid::new().to_string();
        let timestamp = chrono::Utc::now().timestamp_millis();

        let mut message = DirectMessage {
            id: id.clone(),
            sender_did: self.local_did.clone(),
            recipient_did: recipient_did.clone(),
            ciphertext,
            olm_message_type: olm_message_type as u32,
            signature: vec![],
            timestamp,
            r#type: Self::infer_message_type(&content),
            reply_to: content.reply_to.clone(),
            sender_identity_key,
        };

        message.signature = self.sign_message(&message)?;
        self.storage.store_direct(&message).await?;
        self.persist_plaintext(&id, &content).await?;

        // Persist the advanced ratchet state so decryption survives restarts.
        if let Err(e) = self.persist_session(&recipient_did).await {
            tracing::warn!(
                "Failed to persist session state for {}: {}",
                recipient_did,
                e
            );
        }

        Ok(message)
    }

    /// Send an unencrypted message to yourself.
    ///
    /// Self-messages are stored with `olm_message_type = OLM_MESSAGE_TYPE_SELF`
    /// and the `ciphertext` field contains the unencrypted MessageContent protobuf.
    async fn send_self_message(&self, content: MessageContent) -> Result<DirectMessage> {
        let plaintext = prost::Message::encode_to_vec(&content);

        let id = Ulid::new().to_string();
        let timestamp = chrono::Utc::now().timestamp_millis();

        let mut message = DirectMessage {
            id: id.clone(),
            sender_did: self.local_did.clone(),
            recipient_did: self.local_did.clone(),
            ciphertext: plaintext, // Unencrypted MessageContent bytes
            olm_message_type: OLM_MESSAGE_TYPE_SELF,
            signature: vec![],
            timestamp,
            r#type: Self::infer_message_type(&content),
            reply_to: content.reply_to.clone(),
            sender_identity_key: None,
        };

        message.signature = self.sign_message(&message)?;
        self.storage.store_direct(&message).await?;
        self.persist_plaintext(&id, &content).await?;

        Ok(message)
    }

    /// Receive and decrypt a direct message.
    ///
    /// If the message is an Olm PreKey message, the inbound session is created
    /// automatically from the `sender_identity_key` field and the PreKey payload.
    /// Subsequent Normal messages are decrypted using the existing session.
    ///
    /// **Self-messages:** Unencrypted (marked with `olm_message_type = OLM_MESSAGE_TYPE_SELF`)
    /// are decoded directly without Olm decryption.
    ///
    /// NOTE: Callers should verify the message signature with `verify_message_with_key`
    /// before calling this, using the sender's Ed25519 key from their DID document.
    pub async fn receive_message(&self, message: DirectMessage) -> Result<MessageContent> {
        // Self-messages are unencrypted
        if message.olm_message_type == OLM_MESSAGE_TYPE_SELF {
            let content = MessageContent::decode(message.ciphertext.as_slice())
                .map_err(|e| Error::Protocol { source: e })?;
            self.storage.store_direct(&message).await?;
            self.persist_plaintext(&message.id, &content).await?;
            return Ok(content);
        }

        let olm_message =
            OlmMessage::from_parts(message.olm_message_type as usize, &message.ciphertext)
                .map_err(|e| Error::Crypto {
                    message: format!("Failed to decode OlmMessage: {}", e),
                })?;

        let plaintext = match &olm_message {
            OlmMessage::PreKey(pre_key_msg) => {
                let sender_identity_key =
                    message
                        .sender_identity_key
                        .as_ref()
                        .ok_or_else(|| Error::Crypto {
                            message: "PreKey message missing sender_identity_key".to_string(),
                        })?;

                let key_array: [u8; 32] =
                    sender_identity_key
                        .as_slice()
                        .try_into()
                        .map_err(|_| Error::Crypto {
                            message: "sender_identity_key must be exactly 32 bytes".to_string(),
                        })?;
                let identity_key = Curve25519PublicKey::from_bytes(key_array);

                let result = self
                    .account
                    .write()
                    .await
                    .create_inbound_session(identity_key, pre_key_msg)
                    .map_err(|e| Error::Crypto {
                        message: format!("Failed to create inbound Olm session: {}", e),
                    })?;

                self.sessions
                    .write()
                    .await
                    .insert(message.sender_did.clone(), result.session);

                // Persist the newly created inbound session
                if let Err(e) = self.persist_session(&message.sender_did).await {
                    tracing::warn!(
                        "Failed to persist inbound session for {}: {}",
                        message.sender_did,
                        e
                    );
                }

                result.plaintext
            }
            OlmMessage::Normal(_) => {
                // Decrypt inside a scoped block so the write lock is released
                // before persist_session acquires its own read lock below.
                let plaintext = {
                    let mut sessions = self.sessions.write().await;
                    let session = sessions.get_mut(&message.sender_did).ok_or_else(|| {
                        Error::DoubleRatchet {
                            message: format!("No session with {}", message.sender_did),
                        }
                    })?;

                    session.decrypt(&olm_message).map_err(|e| Error::Crypto {
                        message: format!("Decryption failure: {}", e),
                    })?
                }; // sessions write lock released here

                // Persist the advanced ratchet state so subsequent messages survive restarts.
                if let Err(e) = self.persist_session(&message.sender_did).await {
                    tracing::warn!(
                        "Failed to persist session for {}: {}",
                        message.sender_did,
                        e
                    );
                }

                plaintext
            }
        };

        let content = MessageContent::decode(plaintext.as_slice())
            .map_err(|e| Error::Protocol { source: e })?;

        self.storage.store_direct(&message).await?;
        self.persist_plaintext(&message.id, &content).await?;

        Ok(content)
    }

    /// Get message content (from cache if sent by us, otherwise decrypt)
    ///
    /// This is the preferred method for retrieving message content when displaying
    /// a conversation, as it handles both sent and received messages correctly.
    /// Get message content for display.
    ///
    /// Checks the encrypted persistent plaintext store first (survives restarts).
    /// Falls through to Olm decryption only for messages not yet in the store,
    /// which also writes the result to the store for future reads.
    pub async fn get_message_content(&self, message: &DirectMessage) -> Result<MessageContent> {
        if let Some(content) = self.load_plaintext(&message.id).await? {
            return Ok(content);
        }

        // Not in the persistent store yet — decrypt via Olm (also persists).
        self.receive_message(message.clone()).await
    }

    /// Sign a message with the local Ed25519 signing key.
    fn sign_message(&self, message: &DirectMessage) -> Result<Vec<u8>> {
        let data = Self::signable_bytes(message);
        let signature = self.signing_key.sign(&data);
        Ok(signature.to_bytes().to_vec())
    }

    /// Verify a message signature.
    ///
    /// `sender_public_key` must be fetched from the sender's DID document.
    pub fn verify_message_with_key(
        &self,
        message: &DirectMessage,
        sender_public_key: &VerifyingKey,
    ) -> Result<()> {
        let data = Self::signable_bytes(message);
        let signature =
            Signature::from_bytes(message.signature.as_slice().try_into().map_err(|_| {
                Error::InvalidSignature {
                    message_id: message.id.clone(),
                }
            })?);

        sender_public_key
            .verify(&data, &signature)
            .map_err(|_| Error::InvalidSignature {
                message_id: message.id.clone(),
            })?;

        Ok(())
    }

    /// Bytes that are signed/verified: id + sender_did + recipient_did + ciphertext + olm_message_type + timestamp
    fn signable_bytes(message: &DirectMessage) -> Vec<u8> {
        let mut data = Vec::new();
        data.extend_from_slice(message.id.as_bytes());
        data.extend_from_slice(message.sender_did.as_bytes());
        data.extend_from_slice(message.recipient_did.as_bytes());
        data.extend_from_slice(&message.ciphertext);
        data.extend_from_slice(&message.olm_message_type.to_le_bytes());
        data.extend_from_slice(&message.timestamp.to_le_bytes());
        data
    }

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
        before: Option<i64>,
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
    use rand::rngs::OsRng;
    use tempfile::tempdir;

    fn make_handler(did: &str, account: Account) -> (DirectMessageHandler, tempfile::TempDir) {
        let dir = tempdir().unwrap();
        let storage = Arc::new(LocalMessageStorage::new(dir.path()).unwrap());
        let signing_key = SigningKey::generate(&mut OsRng);
        (
            DirectMessageHandler::new(did.to_string(), signing_key, account, storage),
            dir,
        )
    }

    #[tokio::test]
    async fn test_create_handler() {
        let (handler, _dir) = make_handler("did:variance:alice", Account::new());
        assert_eq!(handler.local_did, "did:variance:alice");
    }

    #[tokio::test]
    async fn test_identity_key_is_stable() {
        let (handler, _dir) = make_handler("did:variance:alice", Account::new());
        let key1 = handler.identity_key();
        let key2 = handler.identity_key();
        assert_eq!(key1, key2);
    }

    #[tokio::test]
    async fn test_message_signing() {
        let (handler, _dir) = make_handler("did:variance:alice", Account::new());

        let message = DirectMessage {
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
        let signing_key = SigningKey::generate(&mut OsRng);
        let verifying_key = signing_key.verifying_key();
        let dir = tempdir().unwrap();
        let storage = Arc::new(LocalMessageStorage::new(dir.path()).unwrap());

        let handler = DirectMessageHandler::new(
            "did:variance:alice".to_string(),
            signing_key,
            Account::new(),
            storage,
        );

        let mut message = DirectMessage {
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

        message.signature = handler.sign_message(&message).unwrap();
        assert!(handler
            .verify_message_with_key(&message, &verifying_key)
            .is_ok());
    }

    #[tokio::test]
    async fn test_message_verification_failure() {
        let signing_key = SigningKey::generate(&mut OsRng);
        let wrong_key = SigningKey::generate(&mut OsRng).verifying_key();
        let dir = tempdir().unwrap();
        let storage = Arc::new(LocalMessageStorage::new(dir.path()).unwrap());

        let handler = DirectMessageHandler::new(
            "did:variance:alice".to_string(),
            signing_key,
            Account::new(),
            storage,
        );

        let mut message = DirectMessage {
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

        message.signature = handler.sign_message(&message).unwrap();
        assert!(handler
            .verify_message_with_key(&message, &wrong_key)
            .is_err());
    }

    #[tokio::test]
    async fn test_message_verification_invalid_signature() {
        let signing_key = SigningKey::generate(&mut OsRng);
        let verifying_key = signing_key.verifying_key();
        let dir = tempdir().unwrap();
        let storage = Arc::new(LocalMessageStorage::new(dir.path()).unwrap());

        let handler = DirectMessageHandler::new(
            "did:variance:alice".to_string(),
            signing_key,
            Account::new(),
            storage,
        );

        let message = DirectMessage {
            id: Ulid::new().to_string(),
            sender_did: "did:variance:alice".to_string(),
            recipient_did: "did:variance:bob".to_string(),
            ciphertext: vec![1, 2, 3],
            olm_message_type: 0,
            signature: vec![0; 64],
            timestamp: chrono::Utc::now().timestamp_millis(),
            r#type: MessageType::Text.into(),
            reply_to: None,
            sender_identity_key: None,
        };

        let result = handler.verify_message_with_key(&message, &verifying_key);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            Error::InvalidSignature { .. }
        ));
    }

    #[tokio::test]
    async fn test_init_session_if_needed_idempotent() {
        let mut bob_account = Account::new();
        bob_account.generate_one_time_keys(2);
        let bob_identity = bob_account.curve25519_key();
        let otk1 = *bob_account.one_time_keys().values().next().unwrap();

        let (alice, _dir) = make_handler("did:variance:alice", Account::new());

        alice
            .init_session_if_needed("did:variance:bob", bob_identity, otk1)
            .await
            .unwrap();

        // Second call with the same DID is a no-op even with a different OTK.
        let otk2 = *bob_account.one_time_keys().values().nth(1).unwrap();
        alice
            .init_session_if_needed("did:variance:bob", bob_identity, otk2)
            .await
            .unwrap();

        assert_eq!(alice.session_count().await, 1);
    }

    /// Full round-trip: Alice creates an outbound Olm session using Bob's identity key
    /// and one-time key. Bob's Account auto-creates the inbound session from the first
    /// (PreKey) message and decrypts it.
    #[tokio::test]
    async fn test_full_send_receive_round_trip() {
        let dir = tempdir().unwrap();
        let storage = Arc::new(LocalMessageStorage::new(dir.path()).unwrap());

        let alice_signing = SigningKey::generate(&mut OsRng);
        let bob_signing = SigningKey::generate(&mut OsRng);

        // Bob generates a one-time key for Alice to use.
        let mut bob_account = Account::new();
        bob_account.generate_one_time_keys(1);
        let bob_identity_key = bob_account.curve25519_key();
        let bob_otk = *bob_account.one_time_keys().values().next().unwrap();

        let alice = DirectMessageHandler::new(
            "did:variance:alice".to_string(),
            alice_signing,
            Account::new(),
            storage.clone(),
        );
        let bob = DirectMessageHandler::new(
            "did:variance:bob".to_string(),
            bob_signing,
            bob_account,
            storage,
        );

        // Alice initializes an outbound session with Bob's identity key + OTK.
        alice
            .init_session_as_initiator("did:variance:bob".to_string(), bob_identity_key, bob_otk)
            .await
            .unwrap();

        let content = MessageContent {
            text: "Hello Bob!".to_string(),
            attachments: vec![],
            mentions: vec![],
            reply_to: None,
            metadata: HashMap::new(),
        };

        let wire_message = alice
            .send_message("did:variance:bob".to_string(), content)
            .await
            .unwrap();

        // First message must be a PreKey message (type 0 in Olm) carrying Alice's identity key.
        assert_eq!(
            wire_message.olm_message_type, 0,
            "Expected PreKey message type (0)"
        );
        assert!(
            wire_message.sender_identity_key.is_some(),
            "PreKey message must carry sender_identity_key"
        );

        // Bob receives the PreKey message, auto-creates his inbound session, and decrypts.
        let decrypted = bob.receive_message(wire_message).await.unwrap();
        assert_eq!(decrypted.text, "Hello Bob!");
    }
}
