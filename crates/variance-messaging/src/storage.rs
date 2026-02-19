use crate::error::*;
use async_trait::async_trait;
use prost::Message;
use std::path::Path;
use variance_proto::messaging_proto::{
    DirectMessage, GroupMessage, OfflineMessageEnvelope, ReadReceipt,
};

/// Message storage backend abstraction
///
/// Provides persistence for:
/// - Direct message history
/// - Group message history
/// - Offline message relay queue
/// - Group metadata
#[async_trait]
pub trait MessageStorage: Send + Sync {
    /// Store a direct message
    async fn store_direct(&self, message: &DirectMessage) -> Result<()>;

    /// Fetch direct messages for a conversation
    async fn fetch_direct(
        &self,
        sender_did: &str,
        recipient_did: &str,
        limit: usize,
        before: Option<String>,
    ) -> Result<Vec<DirectMessage>>;

    /// Store a group message
    async fn store_group(&self, message: &GroupMessage) -> Result<()>;

    /// Fetch group messages
    async fn fetch_group(
        &self,
        group_id: &str,
        limit: usize,
        before: Option<String>,
    ) -> Result<Vec<GroupMessage>>;

    /// Store offline message for relay
    async fn store_offline(&self, envelope: &OfflineMessageEnvelope) -> Result<()>;

    /// Fetch offline messages for a recipient
    async fn fetch_offline(
        &self,
        recipient_did: &str,
        since: Option<i64>,
        limit: usize,
    ) -> Result<Vec<OfflineMessageEnvelope>>;

    /// Delete offline message (after delivery)
    async fn delete_offline(&self, message_id: &str) -> Result<()>;

    /// Clean up expired offline messages (TTL enforcement)
    ///
    /// TODO: Implement automatic cleanup scheduling
    /// This method exists but needs to be called periodically.
    /// Should add background task that runs cleanup every hour.
    async fn cleanup_expired(&self) -> Result<usize>;

    /// Store a read receipt
    async fn store_receipt(&self, receipt: &ReadReceipt) -> Result<()>;

    /// Fetch receipts for a specific message
    async fn fetch_receipts(&self, message_id: &str) -> Result<Vec<ReadReceipt>>;

    /// Fetch latest receipt status for a message from a specific reader
    async fn fetch_receipt_status(
        &self,
        message_id: &str,
        reader_did: &str,
    ) -> Result<Option<ReadReceipt>>;

    /// List all direct conversations for a local DID.
    ///
    /// Returns `(peer_did, latest_timestamp)` pairs sorted by timestamp descending.
    async fn list_direct_conversations(&self, local_did: &str) -> Result<Vec<(String, i64)>>;

    /// Delete all messages in a direct conversation.
    async fn delete_direct_conversation(&self, did1: &str, did2: &str) -> Result<()>;
}

/// Local storage implementation using sled
///
/// Stores messages in embedded key-value database:
/// - Direct messages: indexed by conversation ID (sorted pair of DIDs)
/// - Group messages: indexed by group ID
/// - Offline messages: indexed by recipient DID with TTL
pub struct LocalMessageStorage {
    db: sled::Db,
}

impl LocalMessageStorage {
    /// Create a new local message storage instance
    pub fn new<P: AsRef<Path>>(path: P) -> Result<Self> {
        let db = sled::open(path).map_err(|e| Error::Storage { source: e })?;
        Ok(Self { db })
    }

    /// Direct messages tree
    fn direct_tree(&self) -> Result<sled::Tree> {
        self.db
            .open_tree("direct_messages")
            .map_err(|e| Error::Storage { source: e })
    }

    /// Group messages tree
    fn group_tree(&self) -> Result<sled::Tree> {
        self.db
            .open_tree("group_messages")
            .map_err(|e| Error::Storage { source: e })
    }

    /// Offline messages tree
    fn offline_tree(&self) -> Result<sled::Tree> {
        self.db
            .open_tree("offline_messages")
            .map_err(|e| Error::Storage { source: e })
    }

    /// Read receipts tree
    fn receipts_tree(&self) -> Result<sled::Tree> {
        self.db
            .open_tree("read_receipts")
            .map_err(|e| Error::Storage { source: e })
    }

    /// Generate conversation ID from two DIDs (sorted for consistency)
    fn conversation_id(did1: &str, did2: &str) -> String {
        let mut dids = [did1, did2];
        dids.sort();
        format!("{}:{}", dids[0], dids[1])
    }

    /// Generate storage key: conversation_id:timestamp:message_id
    fn direct_key(sender: &str, recipient: &str, timestamp: i64, id: &str) -> String {
        let conv_id = Self::conversation_id(sender, recipient);
        format!("{conv_id}:{timestamp:020}:{id}")
    }

    /// Generate group message key: group_id:timestamp:message_id
    fn group_key(group_id: &str, timestamp: i64, id: &str) -> String {
        format!("{group_id}:{timestamp:020}:{id}")
    }

    /// Generate offline message key: recipient:timestamp:message_id
    fn offline_key(recipient: &str, timestamp: i64, id: &str) -> String {
        format!("{recipient}:{timestamp:020}:{id}")
    }

    /// Parse a direct message key to extract `(conv_id, timestamp)`.
    ///
    /// Key format: `{conv_id}:{timestamp:020}:{msg_id}`.
    /// Since neither the 20-digit timestamp nor the ULID message ID contain
    /// colons, we can reliably split from the right.
    fn parse_direct_key(key: &str) -> Option<(&str, i64)> {
        let last = key.rfind(':')?;
        let before_id = &key[..last];
        let ts_start = before_id.rfind(':')?;
        let conv_id = &before_id[..ts_start];
        let timestamp = before_id[ts_start + 1..].parse::<i64>().ok()?;
        Some((conv_id, timestamp))
    }

    /// Extract the peer DID from a conversation ID given the local DID.
    ///
    /// `conv_id` = `sorted_did_a:sorted_did_b`. Returns the other DID if
    /// `local_did` is one of the two participants, otherwise `None`.
    fn peer_did_from_conv_id(conv_id: &str, local_did: &str) -> Option<String> {
        if let Some(rest) = conv_id.strip_prefix(&format!("{local_did}:")) {
            Some(rest.to_string())
        } else {
            conv_id
                .strip_suffix(&format!(":{local_did}"))
                .map(|rest| rest.to_string())
        }
    }
}

#[async_trait]
impl MessageStorage for LocalMessageStorage {
    async fn store_direct(&self, message: &DirectMessage) -> Result<()> {
        let tree = self.direct_tree()?;
        let key = Self::direct_key(
            &message.sender_did,
            &message.recipient_did,
            message.timestamp,
            &message.id,
        );

        let bytes = prost::Message::encode_to_vec(message);
        tree.insert(key.as_bytes(), bytes.as_slice())
            .map_err(|e| Error::Storage { source: e })?;

        Ok(())
    }

    async fn fetch_direct(
        &self,
        sender_did: &str,
        recipient_did: &str,
        limit: usize,
        before: Option<String>,
    ) -> Result<Vec<DirectMessage>> {
        let tree = self.direct_tree()?;
        let conv_id = Self::conversation_id(sender_did, recipient_did);
        let prefix = format!("{conv_id}:");

        let mut messages = Vec::new();
        let iter = tree.scan_prefix(prefix.as_bytes());

        for entry in iter {
            let (key, value) = entry.map_err(|e| Error::Storage { source: e })?;

            // Check before cursor if specified
            if let Some(ref before_ts) = before {
                let key_str = String::from_utf8_lossy(&key);
                if key_str.as_ref() >= before_ts.as_str() {
                    continue;
                }
            }

            let message =
                DirectMessage::decode(value.as_ref()).map_err(|e| Error::Protocol { source: e })?;

            messages.push(message);

            if messages.len() >= limit {
                break;
            }
        }

        Ok(messages)
    }

    async fn store_group(&self, message: &GroupMessage) -> Result<()> {
        let tree = self.group_tree()?;
        let key = Self::group_key(&message.group_id, message.timestamp, &message.id);

        let bytes = prost::Message::encode_to_vec(message);
        tree.insert(key.as_bytes(), bytes.as_slice())
            .map_err(|e| Error::Storage { source: e })?;

        Ok(())
    }

    async fn fetch_group(
        &self,
        group_id: &str,
        limit: usize,
        before: Option<String>,
    ) -> Result<Vec<GroupMessage>> {
        let tree = self.group_tree()?;
        let prefix = format!("{group_id}:");

        let mut messages = Vec::new();
        let iter = tree.scan_prefix(prefix.as_bytes());

        for entry in iter {
            let (key, value) = entry.map_err(|e| Error::Storage { source: e })?;

            if let Some(ref before_ts) = before {
                let key_str = String::from_utf8_lossy(&key);
                if key_str.as_ref() >= before_ts.as_str() {
                    continue;
                }
            }

            let message =
                GroupMessage::decode(value.as_ref()).map_err(|e| Error::Protocol { source: e })?;

            messages.push(message);

            if messages.len() >= limit {
                break;
            }
        }

        Ok(messages)
    }

    async fn store_offline(&self, envelope: &OfflineMessageEnvelope) -> Result<()> {
        let tree = self.offline_tree()?;

        // Extract message ID and timestamp from envelope
        let (id, timestamp) = match &envelope.message {
            Some(variance_proto::messaging_proto::offline_message_envelope::Message::Direct(
                msg,
            )) => (&msg.id, msg.timestamp),
            Some(variance_proto::messaging_proto::offline_message_envelope::Message::Group(
                msg,
            )) => (&msg.id, msg.timestamp),
            None => {
                return Err(Error::InvalidFormat {
                    message: "Offline envelope has no message".to_string(),
                })
            }
        };

        let key = Self::offline_key(&envelope.recipient_did, timestamp, id);

        let bytes = prost::Message::encode_to_vec(envelope);
        tree.insert(key.as_bytes(), bytes.as_slice())
            .map_err(|e| Error::Storage { source: e })?;

        Ok(())
    }

    async fn fetch_offline(
        &self,
        recipient_did: &str,
        since: Option<i64>,
        limit: usize,
    ) -> Result<Vec<OfflineMessageEnvelope>> {
        let tree = self.offline_tree()?;
        let prefix = format!("{recipient_did}:");

        let mut messages = Vec::new();
        let iter = tree.scan_prefix(prefix.as_bytes());

        for entry in iter {
            let (key, value) = entry.map_err(|e| Error::Storage { source: e })?;

            // Check since timestamp if specified
            if let Some(since_ts) = since {
                let key_str = String::from_utf8_lossy(&key);
                let parts: Vec<&str> = key_str.split(':').collect();
                // Key format: {recipient_did}:{timestamp:020}:{id}
                // Since DID contains colons, timestamp is second-to-last part
                if parts.len() >= 2 {
                    if let Ok(ts) = parts[parts.len() - 2].parse::<i64>() {
                        if ts <= since_ts {
                            continue;
                        }
                    }
                }
            }

            let envelope = OfflineMessageEnvelope::decode(value.as_ref())
                .map_err(|e| Error::Protocol { source: e })?;

            messages.push(envelope);

            if messages.len() >= limit {
                break;
            }
        }

        Ok(messages)
    }

    async fn delete_offline(&self, message_id: &str) -> Result<()> {
        let tree = self.offline_tree()?;

        // Scan all keys to find matching message_id
        for entry in tree.iter() {
            let (key, value) = entry.map_err(|e| Error::Storage { source: e })?;

            let envelope = OfflineMessageEnvelope::decode(value.as_ref())
                .map_err(|e| Error::Protocol { source: e })?;

            let env_id = match &envelope.message {
                Some(
                    variance_proto::messaging_proto::offline_message_envelope::Message::Direct(msg),
                ) => &msg.id,
                Some(
                    variance_proto::messaging_proto::offline_message_envelope::Message::Group(msg),
                ) => &msg.id,
                None => continue,
            };

            if env_id == message_id {
                tree.remove(key).map_err(|e| Error::Storage { source: e })?;
                return Ok(());
            }
        }

        Err(Error::MessageNotFound {
            message_id: message_id.to_string(),
        })
    }

    async fn cleanup_expired(&self) -> Result<usize> {
        let tree = self.offline_tree()?;
        let now = chrono::Utc::now().timestamp_millis();
        let mut deleted = 0;

        let mut to_delete = Vec::new();

        for entry in tree.iter() {
            let (key, value) = entry.map_err(|e| Error::Storage { source: e })?;

            let envelope = OfflineMessageEnvelope::decode(value.as_ref())
                .map_err(|e| Error::Protocol { source: e })?;

            if envelope.expires_at <= now {
                to_delete.push(key.to_vec());
            }
        }

        for key in to_delete {
            tree.remove(key).map_err(|e| Error::Storage { source: e })?;
            deleted += 1;
        }

        Ok(deleted)
    }

    async fn store_receipt(&self, receipt: &ReadReceipt) -> Result<()> {
        let tree = self.receipts_tree()?;

        // Key format: {message_id}:{reader_did}:{timestamp:020}
        let key = format!(
            "{}:{}:{:020}",
            receipt.message_id, receipt.reader_did, receipt.timestamp
        );

        let value = receipt.encode_to_vec();
        tree.insert(key.as_bytes(), value)
            .map_err(|e| Error::Storage { source: e })?;

        Ok(())
    }

    async fn fetch_receipts(&self, message_id: &str) -> Result<Vec<ReadReceipt>> {
        let tree = self.receipts_tree()?;
        let prefix = format!("{message_id}:");

        let mut receipts = Vec::new();
        let iter = tree.scan_prefix(prefix.as_bytes());

        for entry in iter {
            let (_key, value) = entry.map_err(|e| Error::Storage { source: e })?;

            let receipt =
                ReadReceipt::decode(value.as_ref()).map_err(|e| Error::Protocol { source: e })?;

            receipts.push(receipt);
        }

        Ok(receipts)
    }

    async fn fetch_receipt_status(
        &self,
        message_id: &str,
        reader_did: &str,
    ) -> Result<Option<ReadReceipt>> {
        let tree = self.receipts_tree()?;
        let prefix = format!("{message_id}:{reader_did}:");

        // Get the latest receipt (highest timestamp) for this message+reader
        let mut latest: Option<ReadReceipt> = None;

        let iter = tree.scan_prefix(prefix.as_bytes());

        for entry in iter {
            let (_key, value) = entry.map_err(|e| Error::Storage { source: e })?;

            let receipt =
                ReadReceipt::decode(value.as_ref()).map_err(|e| Error::Protocol { source: e })?;

            if let Some(ref current) = latest {
                if receipt.timestamp > current.timestamp {
                    latest = Some(receipt);
                }
            } else {
                latest = Some(receipt);
            }
        }

        Ok(latest)
    }

    async fn list_direct_conversations(&self, local_did: &str) -> Result<Vec<(String, i64)>> {
        let tree = self.direct_tree()?;
        let mut conversations: std::collections::HashMap<String, i64> =
            std::collections::HashMap::new();

        for entry in tree.iter() {
            let (key, _) = entry.map_err(|e| Error::Storage { source: e })?;
            let key_str = String::from_utf8_lossy(&key);

            if let Some((conv_id, timestamp)) = Self::parse_direct_key(&key_str) {
                if let Some(peer_did) = Self::peer_did_from_conv_id(conv_id, local_did) {
                    let entry = conversations.entry(peer_did).or_insert(i64::MIN);
                    if timestamp > *entry {
                        *entry = timestamp;
                    }
                }
            }
        }

        let mut result: Vec<(String, i64)> = conversations.into_iter().collect();
        result.sort_by(|a, b| b.1.cmp(&a.1));
        Ok(result)
    }

    async fn delete_direct_conversation(&self, did1: &str, did2: &str) -> Result<()> {
        let tree = self.direct_tree()?;
        let conv_id = Self::conversation_id(did1, did2);
        let prefix = format!("{conv_id}:");

        let keys_to_delete: Vec<sled::IVec> = tree
            .scan_prefix(prefix.as_bytes())
            .keys()
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| Error::Storage { source: e })?;

        for key in keys_to_delete {
            tree.remove(key).map_err(|e| Error::Storage { source: e })?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    use variance_proto::messaging_proto::MessageType;

    #[tokio::test]
    async fn test_conversation_id_symmetry() {
        let id1 = LocalMessageStorage::conversation_id("alice", "bob");
        let id2 = LocalMessageStorage::conversation_id("bob", "alice");
        assert_eq!(id1, id2);
    }

    #[tokio::test]
    async fn test_store_and_fetch_direct() {
        let dir = tempdir().unwrap();
        let storage = LocalMessageStorage::new(dir.path()).unwrap();

        let message = DirectMessage {
            id: "01ARZ3NDEKTSV4RRFFQ69G5FAV".to_string(),
            sender_did: "did:variance:alice".to_string(),
            recipient_did: "did:variance:bob".to_string(),
            ciphertext: vec![1, 2, 3],
            olm_message_type: 0,
            signature: vec![7, 8, 9],
            timestamp: 1000,
            r#type: MessageType::Text.into(),
            reply_to: None,
            sender_identity_key: None,
        };

        storage.store_direct(&message).await.unwrap();

        let messages = storage
            .fetch_direct("did:variance:alice", "did:variance:bob", 10, None)
            .await
            .unwrap();

        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].id, message.id);
    }

    #[tokio::test]
    async fn test_fetch_direct_conversation_symmetry() {
        let dir = tempdir().unwrap();
        let storage = LocalMessageStorage::new(dir.path()).unwrap();

        let message = DirectMessage {
            id: "01ARZ3NDEKTSV4RRFFQ69G5FAV".to_string(),
            sender_did: "did:variance:alice".to_string(),
            recipient_did: "did:variance:bob".to_string(),
            ciphertext: vec![1, 2, 3],
            olm_message_type: 0,
            signature: vec![],
            timestamp: 1000,
            r#type: MessageType::Text.into(),
            reply_to: None,
            sender_identity_key: None,
        };

        storage.store_direct(&message).await.unwrap();

        // Fetch from both perspectives
        let from_alice = storage
            .fetch_direct("did:variance:alice", "did:variance:bob", 10, None)
            .await
            .unwrap();

        let from_bob = storage
            .fetch_direct("did:variance:bob", "did:variance:alice", 10, None)
            .await
            .unwrap();

        assert_eq!(from_alice.len(), 1);
        assert_eq!(from_bob.len(), 1);
        assert_eq!(from_alice[0].id, from_bob[0].id);
    }

    #[tokio::test]
    async fn test_store_and_fetch_group() {
        let dir = tempdir().unwrap();
        let storage = LocalMessageStorage::new(dir.path()).unwrap();

        let message = GroupMessage {
            id: "01ARZ3NDEKTSV4RRFFQ69G5FAV".to_string(),
            sender_did: "did:variance:alice".to_string(),
            group_id: "group123".to_string(),
            ciphertext: vec![1, 2, 3],
            nonce: vec![],
            signature: vec![],
            timestamp: 1000,
            r#type: MessageType::Text.into(),
            reply_to: None,
        };

        storage.store_group(&message).await.unwrap();

        let messages = storage.fetch_group("group123", 10, None).await.unwrap();

        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].id, message.id);
    }

    #[tokio::test]
    async fn test_store_and_fetch_offline() {
        let dir = tempdir().unwrap();
        let storage = LocalMessageStorage::new(dir.path()).unwrap();

        let direct = DirectMessage {
            id: "01ARZ3NDEKTSV4RRFFQ69G5FAV".to_string(),
            sender_did: "did:variance:alice".to_string(),
            recipient_did: "did:variance:bob".to_string(),
            ciphertext: vec![],
            olm_message_type: 0,
            signature: vec![],
            timestamp: 1000,
            r#type: MessageType::Text.into(),
            reply_to: None,
            sender_identity_key: None,
        };

        let envelope = OfflineMessageEnvelope {
            recipient_did: "did:variance:bob".to_string(),
            message: Some(
                variance_proto::messaging_proto::offline_message_envelope::Message::Direct(
                    direct.clone(),
                ),
            ),
            relay_peer_id: "peer123".to_string(),
            stored_at: 1000,
            expires_at: 2000,
        };

        storage.store_offline(&envelope).await.unwrap();

        let messages = storage
            .fetch_offline("did:variance:bob", None, 10)
            .await
            .unwrap();

        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].recipient_did, "did:variance:bob");
    }

    #[tokio::test]
    async fn test_delete_offline() {
        let dir = tempdir().unwrap();
        let storage = LocalMessageStorage::new(dir.path()).unwrap();

        let direct = DirectMessage {
            id: "01ARZ3NDEKTSV4RRFFQ69G5FAV".to_string(),
            sender_did: "did:variance:alice".to_string(),
            recipient_did: "did:variance:bob".to_string(),
            ciphertext: vec![],
            olm_message_type: 0,
            signature: vec![],
            timestamp: 1000,
            r#type: MessageType::Text.into(),
            reply_to: None,
            sender_identity_key: None,
        };

        let envelope = OfflineMessageEnvelope {
            recipient_did: "did:variance:bob".to_string(),
            message: Some(
                variance_proto::messaging_proto::offline_message_envelope::Message::Direct(
                    direct.clone(),
                ),
            ),
            relay_peer_id: "peer123".to_string(),
            stored_at: 1000,
            expires_at: 2000,
        };

        storage.store_offline(&envelope).await.unwrap();
        storage
            .delete_offline("01ARZ3NDEKTSV4RRFFQ69G5FAV")
            .await
            .unwrap();

        let messages = storage
            .fetch_offline("did:variance:bob", None, 10)
            .await
            .unwrap();

        assert_eq!(messages.len(), 0);
    }

    #[tokio::test]
    async fn test_list_direct_conversations_empty() {
        let dir = tempdir().unwrap();
        let storage = LocalMessageStorage::new(dir.path()).unwrap();

        let convs = storage
            .list_direct_conversations("did:variance:alice")
            .await
            .unwrap();

        assert!(convs.is_empty());
    }

    #[tokio::test]
    async fn test_list_direct_conversations_with_messages() {
        let dir = tempdir().unwrap();
        let storage = LocalMessageStorage::new(dir.path()).unwrap();

        let msg1 = DirectMessage {
            id: "01ARZ3NDEKTSV4RRFFQ69G5FA1".to_string(),
            sender_did: "did:variance:alice".to_string(),
            recipient_did: "did:variance:bob".to_string(),
            ciphertext: vec![],
            olm_message_type: 0,
            signature: vec![],
            timestamp: 1000,
            r#type: MessageType::Text.into(),
            reply_to: None,
            sender_identity_key: None,
        };
        let msg2 = DirectMessage {
            id: "01ARZ3NDEKTSV4RRFFQ69G5FA2".to_string(),
            sender_did: "did:variance:carol".to_string(),
            recipient_did: "did:variance:alice".to_string(),
            ciphertext: vec![],
            olm_message_type: 0,
            signature: vec![],
            timestamp: 2000,
            r#type: MessageType::Text.into(),
            reply_to: None,
            sender_identity_key: None,
        };

        storage.store_direct(&msg1).await.unwrap();
        storage.store_direct(&msg2).await.unwrap();

        let convs = storage
            .list_direct_conversations("did:variance:alice")
            .await
            .unwrap();

        assert_eq!(convs.len(), 2);
        // Sorted desc by timestamp: carol first (2000), bob second (1000)
        assert_eq!(convs[0].0, "did:variance:carol");
        assert_eq!(convs[0].1, 2000);
        assert_eq!(convs[1].0, "did:variance:bob");
        assert_eq!(convs[1].1, 1000);
    }

    #[tokio::test]
    async fn test_delete_direct_conversation() {
        let dir = tempdir().unwrap();
        let storage = LocalMessageStorage::new(dir.path()).unwrap();

        let message = DirectMessage {
            id: "01ARZ3NDEKTSV4RRFFQ69G5FA3".to_string(),
            sender_did: "did:variance:alice".to_string(),
            recipient_did: "did:variance:bob".to_string(),
            ciphertext: vec![1, 2, 3],
            olm_message_type: 0,
            signature: vec![],
            timestamp: 1000,
            r#type: MessageType::Text.into(),
            reply_to: None,
            sender_identity_key: None,
        };

        storage.store_direct(&message).await.unwrap();

        // Verify message exists
        let msgs = storage
            .fetch_direct("did:variance:alice", "did:variance:bob", 10, None)
            .await
            .unwrap();
        assert_eq!(msgs.len(), 1);

        // Delete conversation
        storage
            .delete_direct_conversation("did:variance:alice", "did:variance:bob")
            .await
            .unwrap();

        // Verify deleted
        let msgs = storage
            .fetch_direct("did:variance:alice", "did:variance:bob", 10, None)
            .await
            .unwrap();
        assert!(msgs.is_empty());
    }

    #[tokio::test]
    async fn test_cleanup_expired() {
        let dir = tempdir().unwrap();
        let storage = LocalMessageStorage::new(dir.path()).unwrap();

        let now = chrono::Utc::now().timestamp_millis();

        // Expired message
        let expired = DirectMessage {
            id: "expired".to_string(),
            sender_did: "did:variance:alice".to_string(),
            recipient_did: "did:variance:bob".to_string(),
            ciphertext: vec![],
            olm_message_type: 0,
            signature: vec![],
            timestamp: 1000,
            r#type: MessageType::Text.into(),
            reply_to: None,
            sender_identity_key: None,
        };

        let envelope1 = OfflineMessageEnvelope {
            recipient_did: "did:variance:bob".to_string(),
            message: Some(
                variance_proto::messaging_proto::offline_message_envelope::Message::Direct(expired),
            ),
            relay_peer_id: "peer123".to_string(),
            stored_at: 1000,
            expires_at: now - 1000, // Already expired
        };

        // Valid message
        let valid = DirectMessage {
            id: "valid".to_string(),
            sender_did: "did:variance:alice".to_string(),
            recipient_did: "did:variance:bob".to_string(),
            ciphertext: vec![],
            olm_message_type: 0,
            signature: vec![],
            timestamp: 2000,
            r#type: MessageType::Text.into(),
            reply_to: None,
            sender_identity_key: None,
        };

        let envelope2 = OfflineMessageEnvelope {
            recipient_did: "did:variance:bob".to_string(),
            message: Some(
                variance_proto::messaging_proto::offline_message_envelope::Message::Direct(valid),
            ),
            relay_peer_id: "peer123".to_string(),
            stored_at: 2000,
            expires_at: now + (86400 * 1000), // Expires in 1 day (milliseconds)
        };

        storage.store_offline(&envelope1).await.unwrap();
        storage.store_offline(&envelope2).await.unwrap();

        let deleted = storage.cleanup_expired().await.unwrap();
        assert_eq!(deleted, 1);

        let messages = storage
            .fetch_offline("did:variance:bob", None, 10)
            .await
            .unwrap();

        assert_eq!(messages.len(), 1);
    }
}
