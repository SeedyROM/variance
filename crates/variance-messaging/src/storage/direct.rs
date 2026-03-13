use std::collections::{HashMap, HashSet};

use prost::Message;
use tracing::warn;
use variance_proto::messaging_proto::DirectMessage;

use crate::error::*;

use super::LocalMessageStorage;

impl LocalMessageStorage {
    pub(crate) async fn impl_store_direct(&self, message: &DirectMessage) -> Result<()> {
        let tree = self.direct_tree()?;
        let key = Self::direct_key(
            &message.sender_did,
            &message.recipient_did,
            message.timestamp,
            &message.id,
        );

        let bytes = Message::encode_to_vec(message);
        tree.insert(key.as_bytes(), bytes.as_slice())
            .map_err(|e| Error::Storage { source: e })?;

        Ok(())
    }

    pub(crate) async fn impl_fetch_direct(
        &self,
        sender_did: &str,
        recipient_did: &str,
        limit: usize,
        before: Option<i64>,
    ) -> Result<Vec<DirectMessage>> {
        let tree = self.direct_tree()?;
        let conv_id = Self::conversation_id(sender_did, recipient_did);
        let prefix = format!("{conv_id}:");

        // Scan newest-first so `limit` gives the most recent N messages.
        // Keys are `{conv_id}:{timestamp:020}:{id}` — lexicographic == chronological.
        // .rev() on sled's DoubleEndedIterator walks from the last key in the prefix.
        let mut messages: Vec<DirectMessage> = tree
            .scan_prefix(prefix.as_bytes())
            .rev()
            .filter_map(|entry| DirectMessage::decode(entry.ok()?.1.as_ref()).ok())
            .filter(|msg| before.is_none_or(|ts| msg.timestamp < ts))
            .take(limit)
            .collect();

        // Restore chronological order for the caller.
        messages.reverse();
        Ok(messages)
    }

    pub(crate) async fn impl_list_direct_conversations(
        &self,
        local_did: &str,
    ) -> Result<Vec<(String, i64, Option<i64>)>> {
        let tree = self.direct_tree()?;
        // (peer_did -> (latest_any_ts, latest_peer_ts))
        let mut conversations: HashMap<String, (i64, Option<i64>)> = HashMap::new();

        for entry in tree.iter() {
            let (key, value) = entry.map_err(|e| Error::Storage { source: e })?;
            let key_str = String::from_utf8_lossy(&key);

            if let Some((conv_id, timestamp)) = Self::parse_direct_key(&key_str) {
                if let Some(peer_did) = Self::peer_did_from_conv_id(conv_id, local_did) {
                    let entry = conversations.entry(peer_did).or_insert((i64::MIN, None));
                    if timestamp > entry.0 {
                        entry.0 = timestamp;
                    }
                    if let Ok(msg) = DirectMessage::decode(value.as_ref()) {
                        if msg.sender_did != local_did {
                            let peer_ts = entry.1.get_or_insert(i64::MIN);
                            if timestamp > *peer_ts {
                                *peer_ts = timestamp;
                            }
                        }
                    }
                }
            }
        }

        let mut result: Vec<(String, i64, Option<i64>)> = conversations
            .into_iter()
            .map(|(peer_did, (latest, peer_latest))| (peer_did, latest, peer_latest))
            .collect();
        result.sort_by(|a, b| b.1.cmp(&a.1));
        Ok(result)
    }

    pub(crate) async fn impl_delete_direct_conversation(
        &self,
        did1: &str,
        did2: &str,
    ) -> Result<()> {
        let tree = self.direct_tree()?;
        let conv_id = Self::conversation_id(did1, did2);
        let prefix = format!("{conv_id}:");

        // Collect keys and extract message_ids for plaintext cache cleanup.
        // Key format: {conv_id}:{timestamp:020}:{message_id}
        let mut keys_to_delete: Vec<sled::IVec> = Vec::new();
        let mut message_ids: Vec<String> = Vec::new();

        for entry in tree.scan_prefix(prefix.as_bytes()) {
            let (key, _) = entry.map_err(|e| Error::Storage { source: e })?;
            if let Ok(key_str) = std::str::from_utf8(&key) {
                if let Some(last_colon) = key_str.rfind(':') {
                    message_ids.push(key_str[last_colon + 1..].to_string());
                }
            }
            keys_to_delete.push(key);
        }

        for key in &keys_to_delete {
            tree.remove(key).map_err(|e| Error::Storage { source: e })?;
        }

        // Also clean the plaintext cache for these messages.
        if !message_ids.is_empty() {
            match self.plaintext_tree() {
                Ok(pt_tree) => {
                    for msg_id in &message_ids {
                        if let Err(e) = pt_tree.remove(msg_id.as_bytes()) {
                            warn!("Failed to remove plaintext cache for {}: {}", msg_id, e);
                        }
                    }
                }
                Err(e) => {
                    warn!(
                        "Failed to open plaintext tree while deleting conversation: {}",
                        e
                    );
                }
            }
        }

        Ok(())
    }

    pub(crate) async fn impl_delete_direct_by_id(
        &self,
        sender_did: &str,
        recipient_did: &str,
        timestamp: i64,
        message_id: &str,
    ) -> Result<()> {
        let tree = self.direct_tree()?;
        let key = Self::direct_key(sender_did, recipient_did, timestamp, message_id);
        tree.remove(key.as_bytes())
            .map_err(|e| Error::Storage { source: e })?;

        // Also remove the stored plaintext cache entry; log on failure so leaked
        // entries are visible in logs rather than silently accumulating.
        match self.plaintext_tree() {
            Ok(pt_tree) => {
                if let Err(e) = pt_tree.remove(message_id.as_bytes()) {
                    warn!("Failed to remove plaintext cache for {}: {}", message_id, e);
                }
            }
            Err(e) => {
                warn!(
                    "Failed to open plaintext tree while deleting {}: {}",
                    message_id, e
                );
            }
        }

        Ok(())
    }

    pub(crate) async fn impl_store_plaintext(
        &self,
        message_id: &str,
        encrypted: &[u8],
    ) -> Result<()> {
        let tree = self.plaintext_tree()?;
        tree.insert(message_id.as_bytes(), encrypted)
            .map_err(|e| Error::Storage { source: e })?;
        Ok(())
    }

    pub(crate) async fn impl_fetch_plaintext(&self, message_id: &str) -> Result<Option<Vec<u8>>> {
        let tree = self.plaintext_tree()?;
        Ok(tree
            .get(message_id.as_bytes())
            .map_err(|e| Error::Storage { source: e })?
            .map(|v| v.to_vec()))
    }

    pub(crate) async fn impl_store_session_pickle(
        &self,
        peer_did: &str,
        pickle_json: &str,
    ) -> Result<()> {
        let tree = self.session_pickles_tree()?;
        tree.insert(peer_did.as_bytes(), pickle_json.as_bytes())
            .map_err(|e| Error::Storage { source: e })?;
        Ok(())
    }

    pub(crate) async fn impl_fetch_session_pickle(&self, peer_did: &str) -> Result<Option<String>> {
        let tree = self.session_pickles_tree()?;
        Ok(tree
            .get(peer_did.as_bytes())
            .map_err(|e| Error::Storage { source: e })?
            .map(|v| String::from_utf8_lossy(&v).to_string()))
    }

    pub(crate) async fn impl_load_all_session_pickles(&self) -> Result<Vec<(String, String)>> {
        let tree = self.session_pickles_tree()?;
        let mut sessions = Vec::new();

        for item in tree.iter() {
            let (key, value) = item.map_err(|e| Error::Storage { source: e })?;
            let peer_did = String::from_utf8_lossy(&key).to_string();
            let pickle_json = String::from_utf8_lossy(&value).to_string();
            sessions.push((peer_did, pickle_json));
        }

        Ok(sessions)
    }

    pub(crate) async fn impl_store_pending_message(
        &self,
        peer_did: &str,
        message: &DirectMessage,
    ) -> Result<()> {
        let tree = self.pending_messages_tree()?;
        let index = self.message_index_tree()?;
        let key = format!("{}:{}", peer_did, message.id);
        let value = message.encode_to_vec();
        tree.insert(key.as_bytes(), value.as_slice())
            .map_err(|e| Error::Storage { source: e })?;

        // Insert reverse index: idx:{message_id} → pending:{full_key}
        let idx_key = format!("idx:{}", message.id);
        let idx_val = format!("pending:{key}");
        index
            .insert(idx_key.as_bytes(), idx_val.as_bytes())
            .map_err(|e| Error::Storage { source: e })?;

        Ok(())
    }

    pub(crate) async fn impl_fetch_pending_messages(
        &self,
        peer_did: &str,
    ) -> Result<Vec<DirectMessage>> {
        let tree = self.pending_messages_tree()?;
        let prefix = format!("{}:", peer_did);
        let mut messages = Vec::new();

        for item in tree.scan_prefix(prefix.as_bytes()) {
            let (_, value) = item.map_err(|e| Error::Storage { source: e })?;
            let message =
                DirectMessage::decode(value.as_ref()).map_err(|e| Error::Protocol { source: e })?;
            messages.push(message);
        }

        Ok(messages)
    }

    pub(crate) async fn impl_delete_pending_message(&self, message_id: &str) -> Result<()> {
        let tree = self.pending_messages_tree()?;
        let index = self.message_index_tree()?;
        let idx_key = format!("idx:{message_id}");

        // Try O(1) reverse-index lookup first
        if let Some(idx_val) = index
            .get(idx_key.as_bytes())
            .map_err(|e| Error::Storage { source: e })?
        {
            let val_str = String::from_utf8_lossy(&idx_val);
            if let Some(full_key) = val_str.strip_prefix("pending:") {
                tree.remove(full_key.as_bytes())
                    .map_err(|e| Error::Storage { source: e })?;
                index
                    .remove(idx_key.as_bytes())
                    .map_err(|e| Error::Storage { source: e })?;
                return Ok(());
            }
        }

        // Fallback: scan for backward compatibility with pre-index data
        for item in tree.iter() {
            let (key, _) = item.map_err(|e| Error::Storage { source: e })?;
            let key_str = String::from_utf8_lossy(&key);
            if key_str.ends_with(&format!(":{}", message_id)) {
                tree.remove(key).map_err(|e| Error::Storage { source: e })?;
                break;
            }
        }

        Ok(())
    }

    pub(crate) async fn impl_is_message_pending(&self, message_id: &str) -> Result<bool> {
        let index = self.message_index_tree()?;
        let idx_key = format!("idx:{message_id}");

        // Try O(1) reverse-index lookup first
        if let Some(idx_val) = index
            .get(idx_key.as_bytes())
            .map_err(|e| Error::Storage { source: e })?
        {
            let val_str = String::from_utf8_lossy(&idx_val);
            if val_str.starts_with("pending:") {
                return Ok(true);
            }
        }

        // Fallback: scan for backward compatibility with pre-index data
        let tree = self.pending_messages_tree()?;
        for item in tree.iter() {
            let (key, _) = item.map_err(|e| Error::Storage { source: e })?;
            let key_str = String::from_utf8_lossy(&key);
            if key_str.ends_with(&format!(":{}", message_id)) {
                return Ok(true);
            }
        }

        Ok(false)
    }

    pub(crate) async fn impl_list_peers_with_pending_messages(&self) -> Result<Vec<String>> {
        let tree = self.pending_messages_tree()?;
        let mut peers = HashSet::new();

        for item in tree.iter() {
            let (key, _) = item.map_err(|e| Error::Storage { source: e })?;
            let key_str = String::from_utf8_lossy(&key);
            // Key format: "{peer_did}:{message_ulid}" where peer_did contains colons
            // (e.g. "did:variance:abc123:01ARZMSGID"). rsplit_once peels off the
            // ULID suffix leaving the full DID as the peer identifier.
            if let Some((peer_did, _)) = key_str.rsplit_once(':') {
                peers.insert(peer_did.to_string());
            }
        }

        Ok(peers.into_iter().collect())
    }

    /// Delete direct messages older than `max_age`.
    ///
    /// Iterates the direct message tree and removes entries whose embedded
    /// timestamp predates `now - max_age`. Returns the number of messages deleted.
    pub async fn cleanup_old_direct_messages(&self, max_age: std::time::Duration) -> Result<usize> {
        let tree = self.direct_tree()?;
        let cutoff_ms = chrono::Utc::now().timestamp_millis() - max_age.as_millis() as i64;

        let mut to_delete: Vec<Vec<u8>> = Vec::new();

        for entry in tree.iter() {
            let (key, _) = entry.map_err(|e| Error::Storage { source: e })?;
            let key_str = String::from_utf8_lossy(&key);
            if let Some((_, ts)) = Self::parse_direct_key(&key_str) {
                if ts < cutoff_ms {
                    to_delete.push(key.to_vec());
                }
            }
        }

        let deleted = to_delete.len();
        for key in to_delete {
            tree.remove(key).map_err(|e| Error::Storage { source: e })?;
        }

        Ok(deleted)
    }
}

#[cfg(test)]
mod tests {
    use crate::storage::{LocalMessageStorage, MessageStorage};
    use tempfile::tempdir;
    use variance_proto::messaging_proto::{DirectMessage, MessageType};

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
        // carol→alice: peer is carol, so latest_peer_timestamp = Some(2000)
        assert_eq!(convs[0].2, Some(2000));
        assert_eq!(convs[1].0, "did:variance:bob");
        assert_eq!(convs[1].1, 1000);
        // alice→bob: peer is bob, bob never sent, so latest_peer_timestamp = None
        assert_eq!(convs[1].2, None);
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
    async fn test_pending_message_reverse_index() {
        let dir = tempdir().unwrap();
        let storage = LocalMessageStorage::new(dir.path()).unwrap();

        let message = DirectMessage {
            id: "01PENDING_MSG_ID".to_string(),
            sender_did: "did:variance:alice".to_string(),
            recipient_did: "did:variance:bob".to_string(),
            ciphertext: vec![1, 2, 3],
            olm_message_type: 0,
            signature: vec![],
            timestamp: 5000,
            r#type: MessageType::Text.into(),
            reply_to: None,
            sender_identity_key: None,
        };

        // Store pending message (creates reverse index entry)
        storage
            .store_pending_message("did:variance:bob", &message)
            .await
            .unwrap();

        // O(1) is_message_pending via index
        assert!(storage
            .is_message_pending("01PENDING_MSG_ID")
            .await
            .unwrap());
        assert!(!storage.is_message_pending("nonexistent").await.unwrap());

        // O(1) delete via index
        storage
            .delete_pending_message("01PENDING_MSG_ID")
            .await
            .unwrap();

        assert!(!storage
            .is_message_pending("01PENDING_MSG_ID")
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn test_delete_direct_by_id() {
        let dir = tempdir().unwrap();
        let storage = LocalMessageStorage::new(dir.path()).unwrap();

        let msg = DirectMessage {
            id: "01ARZ3NDEKTSV4RRFFQ69G5FAV".to_string(),
            sender_did: "did:key:alice".to_string(),
            recipient_did: "did:key:bob".to_string(),
            ciphertext: vec![1, 2, 3],
            olm_message_type: 0,
            signature: vec![],
            timestamp: 5000,
            r#type: MessageType::Text.into(),
            reply_to: None,
            sender_identity_key: None,
        };

        storage.store_direct(&msg).await.unwrap();

        // Verify the message exists.
        let before = storage
            .fetch_direct("did:key:alice", "did:key:bob", 10, None)
            .await
            .unwrap();
        assert_eq!(before.len(), 1);

        // Delete it by ID.
        storage
            .delete_direct_by_id(
                "did:key:alice",
                "did:key:bob",
                5000,
                "01ARZ3NDEKTSV4RRFFQ69G5FAV",
            )
            .await
            .unwrap();

        // Verify it's gone.
        let after = storage
            .fetch_direct("did:key:alice", "did:key:bob", 10, None)
            .await
            .unwrap();
        assert!(after.is_empty());
    }

    #[tokio::test]
    async fn test_delete_direct_by_id_preserves_other_messages() {
        let dir = tempdir().unwrap();
        let storage = LocalMessageStorage::new(dir.path()).unwrap();

        let msg1 = DirectMessage {
            id: "msg-keep".to_string(),
            sender_did: "did:key:alice".to_string(),
            recipient_did: "did:key:bob".to_string(),
            ciphertext: vec![1],
            olm_message_type: 0,
            signature: vec![],
            timestamp: 1000,
            r#type: MessageType::Text.into(),
            reply_to: None,
            sender_identity_key: None,
        };
        let msg2 = DirectMessage {
            id: "msg-delete".to_string(),
            sender_did: "did:key:alice".to_string(),
            recipient_did: "did:key:bob".to_string(),
            ciphertext: vec![2],
            olm_message_type: 0,
            signature: vec![],
            timestamp: 2000,
            r#type: MessageType::Text.into(),
            reply_to: None,
            sender_identity_key: None,
        };

        storage.store_direct(&msg1).await.unwrap();
        storage.store_direct(&msg2).await.unwrap();

        // Delete only msg2.
        storage
            .delete_direct_by_id("did:key:alice", "did:key:bob", 2000, "msg-delete")
            .await
            .unwrap();

        let remaining = storage
            .fetch_direct("did:key:alice", "did:key:bob", 10, None)
            .await
            .unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].id, "msg-keep");
    }

    #[tokio::test]
    async fn test_cleanup_old_direct_messages() {
        let dir = tempdir().unwrap();
        let storage = LocalMessageStorage::new(dir.path()).unwrap();

        let now_ms = chrono::Utc::now().timestamp_millis();

        // Old message: 40 days ago (deleted with 30-day cutoff)
        let old_ts = now_ms - 40 * 86_400_000i64;
        let old_msg = DirectMessage {
            id: "01ARZ3NDEKTSV4RRFFQ69G5OLD".to_string(),
            sender_did: "did:key:alice".to_string(),
            recipient_did: "did:key:bob".to_string(),
            ciphertext: vec![1],
            olm_message_type: 0,
            signature: vec![],
            timestamp: old_ts,
            r#type: MessageType::Text.into(),
            reply_to: None,
            sender_identity_key: None,
        };

        // Recent message: 1 day ago (survives)
        let recent_ts = now_ms - 86_400_000i64;
        let recent_msg = DirectMessage {
            id: "01ARZ3NDEKTSV4RRFFQ69G5NEW".to_string(),
            sender_did: "did:key:alice".to_string(),
            recipient_did: "did:key:bob".to_string(),
            ciphertext: vec![2],
            olm_message_type: 0,
            signature: vec![],
            timestamp: recent_ts,
            r#type: MessageType::Text.into(),
            reply_to: None,
            sender_identity_key: None,
        };

        storage.store_direct(&old_msg).await.unwrap();
        storage.store_direct(&recent_msg).await.unwrap();

        let before = storage
            .fetch_direct("did:key:alice", "did:key:bob", 10, None)
            .await
            .unwrap();
        assert_eq!(before.len(), 2);

        let deleted = storage
            .cleanup_old_direct_messages(std::time::Duration::from_secs(30 * 86400))
            .await
            .unwrap();
        assert_eq!(deleted, 1, "only the 40-day-old message should be deleted");

        let after = storage
            .fetch_direct("did:key:alice", "did:key:bob", 10, None)
            .await
            .unwrap();
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].id, "01ARZ3NDEKTSV4RRFFQ69G5NEW");
    }
}
