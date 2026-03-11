use prost::Message;
use variance_proto::messaging_proto::OfflineMessageEnvelope;

use crate::error::*;

use super::LocalMessageStorage;

impl LocalMessageStorage {
    pub(crate) async fn impl_store_offline(&self, envelope: &OfflineMessageEnvelope) -> Result<()> {
        let tree = self.offline_tree()?;
        let index = self.message_index_tree()?;

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

        let key = Self::offline_key(&envelope.mailbox_token, timestamp, id);

        let bytes = Message::encode_to_vec(envelope);
        tree.insert(key.as_bytes(), bytes.as_slice())
            .map_err(|e| Error::Storage { source: e })?;

        // Insert reverse index: idx:{message_id} → offline:{full_key}
        let idx_key = format!("idx:{id}");
        let idx_val = format!("offline:{key}");
        index
            .insert(idx_key.as_bytes(), idx_val.as_bytes())
            .map_err(|e| Error::Storage { source: e })?;

        Ok(())
    }

    pub(crate) async fn impl_fetch_offline(
        &self,
        mailbox_token: &[u8],
        since: Option<i64>,
        limit: usize,
    ) -> Result<Vec<OfflineMessageEnvelope>> {
        let tree = self.offline_tree()?;
        // Key format: {64-char-hex}:{timestamp:020}:{id} — no colons in the hex prefix,
        // so splitting from the right is unambiguous.
        let prefix = format!("{}:", hex::encode(mailbox_token));

        let mut messages = Vec::new();
        let iter = tree.scan_prefix(prefix.as_bytes());

        for entry in iter {
            let (key, value) = entry.map_err(|e| Error::Storage { source: e })?;

            // Check since timestamp if specified
            if let Some(since_ts) = since {
                let key_str = String::from_utf8_lossy(&key);
                // Key format: {hex}:{timestamp:020}:{id} — split from right, take second segment
                let last = key_str.rfind(':').unwrap_or(0);
                let before_id = &key_str[..last];
                if let Some(ts_start) = before_id.rfind(':') {
                    if let Ok(ts) = before_id[ts_start + 1..].parse::<i64>() {
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

    pub(crate) async fn impl_count_offline_for_mailbox(
        &self,
        mailbox_token: &[u8],
    ) -> Result<usize> {
        let tree = self.offline_tree()?;
        let prefix = format!("{}:", hex::encode(mailbox_token));
        Ok(tree.scan_prefix(prefix.as_bytes()).count())
    }

    pub(crate) async fn impl_delete_offline(&self, message_id: &str) -> Result<()> {
        let tree = self.offline_tree()?;
        let index = self.message_index_tree()?;
        let idx_key = format!("idx:{message_id}");

        // Try O(1) reverse-index lookup first
        if let Some(idx_val) = index
            .get(idx_key.as_bytes())
            .map_err(|e| Error::Storage { source: e })?
        {
            let val_str = String::from_utf8_lossy(&idx_val);
            if let Some(full_key) = val_str.strip_prefix("offline:") {
                tree.remove(full_key.as_bytes())
                    .map_err(|e| Error::Storage { source: e })?;
                index
                    .remove(idx_key.as_bytes())
                    .map_err(|e| Error::Storage { source: e })?;
                return Ok(());
            }
        }

        // Fallback: scan for backward compatibility with data stored before the index
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
                tree.remove(&key)
                    .map_err(|e| Error::Storage { source: e })?;
                return Ok(());
            }
        }

        Err(Error::MessageNotFound {
            message_id: message_id.to_string(),
        })
    }

    pub(crate) async fn impl_cleanup_expired(&self) -> Result<usize> {
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
}

#[cfg(test)]
mod tests {
    use crate::storage::{LocalMessageStorage, MessageStorage};
    use tempfile::tempdir;
    use variance_proto::messaging_proto::{DirectMessage, MessageType, OfflineMessageEnvelope};

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
            mailbox_token: vec![0xb0u8; 32],
            message: Some(
                variance_proto::messaging_proto::offline_message_envelope::Message::Direct(
                    direct.clone(),
                ),
            ),
            stored_at: 1000,
            expires_at: 2000,
        };

        storage.store_offline(&envelope).await.unwrap();

        let messages = storage
            .fetch_offline(&[0xb0u8; 32], None, 10)
            .await
            .unwrap();

        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].mailbox_token, vec![0xb0u8; 32]);
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
            mailbox_token: vec![0xb0u8; 32],
            message: Some(
                variance_proto::messaging_proto::offline_message_envelope::Message::Direct(
                    direct.clone(),
                ),
            ),
            stored_at: 1000,
            expires_at: 2000,
        };

        storage.store_offline(&envelope).await.unwrap();
        storage
            .delete_offline("01ARZ3NDEKTSV4RRFFQ69G5FAV")
            .await
            .unwrap();

        let messages = storage
            .fetch_offline(&[0xb0u8; 32], None, 10)
            .await
            .unwrap();

        assert_eq!(messages.len(), 0);
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
            mailbox_token: vec![0xb0u8; 32],
            message: Some(
                variance_proto::messaging_proto::offline_message_envelope::Message::Direct(expired),
            ),
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
            mailbox_token: vec![0xb0u8; 32],
            message: Some(
                variance_proto::messaging_proto::offline_message_envelope::Message::Direct(valid),
            ),
            stored_at: 2000,
            expires_at: now + (86400 * 1000), // Expires in 1 day (milliseconds)
        };

        storage.store_offline(&envelope1).await.unwrap();
        storage.store_offline(&envelope2).await.unwrap();

        let deleted = storage.cleanup_expired().await.unwrap();
        assert_eq!(deleted, 1);

        let messages = storage
            .fetch_offline(&[0xb0u8; 32], None, 10)
            .await
            .unwrap();

        assert_eq!(messages.len(), 1);
    }

    #[tokio::test]
    async fn test_offline_message_reverse_index() {
        let dir = tempdir().unwrap();
        let storage = LocalMessageStorage::new(dir.path()).unwrap();

        let direct = DirectMessage {
            id: "01OFFLINE_MSG_ID".to_string(),
            sender_did: "did:variance:alice".to_string(),
            recipient_did: "did:variance:bob".to_string(),
            ciphertext: vec![],
            olm_message_type: 0,
            signature: vec![],
            timestamp: 3000,
            r#type: MessageType::Text.into(),
            reply_to: None,
            sender_identity_key: None,
        };

        let envelope = OfflineMessageEnvelope {
            mailbox_token: vec![0xb0u8; 32],
            message: Some(
                variance_proto::messaging_proto::offline_message_envelope::Message::Direct(direct),
            ),
            stored_at: 3000,
            expires_at: i64::MAX,
        };

        // Store creates index entry
        storage.store_offline(&envelope).await.unwrap();

        let messages = storage
            .fetch_offline(&[0xb0u8; 32], None, 10)
            .await
            .unwrap();
        assert_eq!(messages.len(), 1);

        // O(1) delete via index
        storage.delete_offline("01OFFLINE_MSG_ID").await.unwrap();

        let messages = storage
            .fetch_offline(&[0xb0u8; 32], None, 10)
            .await
            .unwrap();
        assert_eq!(messages.len(), 0);

        // Index entry should be cleaned up too
        let index = storage.message_index_tree().unwrap();
        assert!(index
            .get("idx:01OFFLINE_MSG_ID".as_bytes())
            .unwrap()
            .is_none());
    }
}
