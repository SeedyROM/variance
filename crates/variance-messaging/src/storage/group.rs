use prost::Message;
use tracing::warn;
use variance_proto::messaging_proto::{Group, GroupMessage};

use crate::error::*;

use super::LocalMessageStorage;

impl LocalMessageStorage {
    pub(crate) async fn impl_store_group(&self, message: &GroupMessage) -> Result<()> {
        let tree = self.group_tree()?;
        let key = Self::group_key(&message.group_id, message.timestamp, &message.id);

        let bytes = Message::encode_to_vec(message);
        tree.insert(key.as_bytes(), bytes.as_slice())
            .map_err(|e| Error::Storage { source: e })?;

        Ok(())
    }

    pub(crate) async fn impl_fetch_group(
        &self,
        group_id: &str,
        limit: usize,
        before: Option<i64>,
    ) -> Result<Vec<GroupMessage>> {
        let tree = self.group_tree()?;
        let prefix = format!("{group_id}:");
        // Upper bound for this group's key range (';' is one byte after ':').
        let prefix_end = format!("{group_id};");

        // Scan newest-first so `limit` returns the most recent N messages.
        // Keys are `{group_id}:{timestamp:020}:{id}` — lexicographic == chronological.
        let mut messages: Vec<GroupMessage> = tree
            .range(prefix.as_bytes()..prefix_end.as_bytes())
            .rev()
            .filter_map(|entry| {
                let (_key, value) = entry.ok()?;
                GroupMessage::decode(value.as_ref()).ok()
            })
            .filter(|msg| before.is_none_or(|ts| msg.timestamp < ts))
            .take(limit)
            .collect();

        // Restore chronological order for the caller.
        messages.reverse();
        Ok(messages)
    }

    pub(crate) async fn impl_fetch_group_latest(
        &self,
        group_id: &str,
    ) -> Result<Option<GroupMessage>> {
        let tree = self.group_tree()?;
        let prefix = format!("{group_id}:");
        // The upper-bound key for this group's prefix: increment the last byte so we
        // can use scan_prefix in reverse by ranging up to the first key of the next group.
        let prefix_end = {
            let mut end = prefix.as_bytes().to_vec();
            *end.last_mut().unwrap() += 1;
            end
        };
        // Scan backwards from the end of this group's key range.
        Ok(tree
            .range(prefix.as_bytes()..prefix_end.as_slice())
            .next_back()
            .transpose()
            .map_err(|e| Error::Storage { source: e })?
            .and_then(|(_, v)| GroupMessage::decode(v.as_ref()).ok()))
    }

    pub(crate) async fn impl_fetch_group_since(
        &self,
        group_id: &str,
        since_timestamp: i64,
        limit: usize,
    ) -> Result<Vec<GroupMessage>> {
        let tree = self.group_tree()?;
        // Keys are "{group_id}:{timestamp:020}:{id}" — lexicographic scan
        // starting just after the given timestamp.
        let start = format!("{}:{:020}:", group_id, since_timestamp.saturating_add(1));
        let prefix = format!("{}:", group_id);

        let mut messages = Vec::new();
        for entry in tree.range(start.as_bytes()..) {
            let (key, value) = entry.map_err(|e| Error::Storage { source: e })?;

            // Stop once we leave this group's prefix.
            let key_str = match std::str::from_utf8(&key) {
                Ok(s) => s,
                Err(_) => {
                    warn!("Skipping group message with non-UTF-8 key");
                    continue;
                }
            };
            if !key_str.starts_with(&prefix) {
                break;
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

    pub(crate) async fn impl_latest_group_timestamp(&self, group_id: &str) -> Result<Option<i64>> {
        let tree = self.group_tree()?;
        let prefix = format!("{}:", group_id);

        // Scan backwards from the end of this group's key range.
        // The last key with this prefix holds the newest timestamp.
        let end = format!("{};", group_id); // ';' is one byte after ':' in ASCII
        let mut iter = tree.range(prefix.as_bytes()..end.as_bytes());

        if let Some(entry) = iter.next_back() {
            let (_key, value) = entry.map_err(|e| Error::Storage { source: e })?;
            let message =
                GroupMessage::decode(value.as_ref()).map_err(|e| Error::Protocol { source: e })?;
            Ok(Some(message.timestamp))
        } else {
            Ok(None)
        }
    }

    pub(crate) async fn impl_has_group_message(&self, group_id: &str, message_id: &str) -> bool {
        let tree = match self.group_tree() {
            Ok(t) => t,
            Err(_) => return false,
        };
        let prefix = format!("{}:", group_id);
        // Scan the group's range looking for a key that ends with the message_id.
        // Keys are `{group_id}:{timestamp:020}:{id}`.
        let end = format!("{};", group_id);
        let suffix = format!(":{}", message_id);
        for (key, _) in tree.range(prefix.as_bytes()..end.as_bytes()).flatten() {
            if let Ok(k) = std::str::from_utf8(&key) {
                if k.ends_with(&suffix) {
                    return true;
                }
            }
        }
        false
    }

    pub(crate) async fn impl_delete_group_messages(&self, group_id: &str) -> Result<()> {
        let tree = self.group_tree()?;
        let prefix = format!("{}:", group_id);

        // Collect keys and extract message_ids for plaintext cache cleanup.
        // Key format: {group_id}:{timestamp:020}:{message_id}
        let mut keys_to_delete: Vec<sled::IVec> = Vec::new();
        let mut message_ids: Vec<String> = Vec::new();

        for (key, _) in tree.scan_prefix(prefix.as_bytes()).flatten() {
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

        // Also clean the group plaintext cache for these messages.
        if !message_ids.is_empty() {
            if let Ok(pt_tree) = self.db.open_tree("group_plaintext_cache") {
                for msg_id in &message_ids {
                    if let Err(e) = pt_tree.remove(msg_id.as_bytes()) {
                        warn!(
                            "Failed to remove group plaintext cache for {}: {}",
                            msg_id, e
                        );
                    }
                }
            }
        }

        Ok(())
    }

    pub(crate) async fn impl_delete_group_metadata(&self, group_id: &str) -> Result<()> {
        let tree = self.group_metadata_tree()?;
        tree.remove(group_id.as_bytes())
            .map_err(|e| Error::Storage { source: e })?;
        Ok(())
    }

    pub(crate) async fn impl_store_group_metadata(&self, group: &Group) -> Result<()> {
        let tree = self.group_metadata_tree()?;
        let bytes = Message::encode_to_vec(group);
        tree.insert(group.id.as_bytes(), bytes.as_slice())
            .map_err(|e| Error::Storage { source: e })?;
        Ok(())
    }

    pub(crate) async fn impl_fetch_group_metadata(&self, group_id: &str) -> Result<Option<Group>> {
        let tree = self.group_metadata_tree()?;
        Ok(tree
            .get(group_id.as_bytes())
            .map_err(|e| Error::Storage { source: e })?
            .and_then(|v| Group::decode(v.as_ref()).ok()))
    }

    pub(crate) async fn impl_fetch_all_group_metadata(&self) -> Result<Vec<Group>> {
        let tree = self.group_metadata_tree()?;
        let mut groups = Vec::new();
        for entry in tree.iter() {
            let (_, value) = entry.map_err(|e| Error::Storage { source: e })?;
            let group = Group::decode(value.as_ref()).map_err(|e| Error::Protocol { source: e })?;
            groups.push(group);
        }
        Ok(groups)
    }

    pub(crate) async fn impl_update_member_role(
        &self,
        group_id: &str,
        member_did: &str,
        new_role: i32,
    ) -> Result<bool> {
        let tree = self.group_metadata_tree()?;
        let Some(raw) = tree
            .get(group_id.as_bytes())
            .map_err(|e| Error::Storage { source: e })?
        else {
            return Ok(false);
        };

        let mut group = Group::decode(raw.as_ref()).map_err(|e| Error::Protocol { source: e })?;

        let Some(member) = group.members.iter_mut().find(|m| m.did == member_did) else {
            return Ok(false);
        };

        member.role = new_role;

        let bytes = Message::encode_to_vec(&group);
        tree.insert(group_id.as_bytes(), bytes.as_slice())
            .map_err(|e| Error::Storage { source: e })?;

        Ok(true)
    }

    pub(crate) async fn impl_store_mls_state(&self, local_did: &str, state: &[u8]) -> Result<()> {
        let tree = self.mls_state_tree()?;
        tree.insert(local_did.as_bytes(), state)
            .map_err(|e| Error::Storage { source: e })?;
        Ok(())
    }

    pub(crate) async fn impl_fetch_mls_state(&self, local_did: &str) -> Result<Option<Vec<u8>>> {
        let tree = self.mls_state_tree()?;
        Ok(tree
            .get(local_did.as_bytes())
            .map_err(|e| Error::Storage { source: e })?
            .map(|v| v.to_vec()))
    }

    /// Persist the at-rest-encrypted plaintext blob for a group message.
    ///
    /// `blob` is `nonce (12 bytes) || AES-256-GCM ciphertext` produced by
    /// `MlsGroupHandler::persist_group_plaintext`. Stored in a dedicated tree
    /// keyed by message_id to keep it separate from the DM plaintext cache.
    pub async fn store_group_plaintext(&self, message_id: &str, blob: &[u8]) -> Result<()> {
        let tree = self
            .db
            .open_tree("group_plaintext_cache")
            .map_err(|e| Error::Storage { source: e })?;
        tree.insert(message_id.as_bytes(), blob)
            .map_err(|e| Error::Storage { source: e })?;
        Ok(())
    }

    /// Retrieve a previously stored group message plaintext blob.
    pub async fn fetch_group_plaintext(&self, message_id: &str) -> Result<Option<Vec<u8>>> {
        let tree = self
            .db
            .open_tree("group_plaintext_cache")
            .map_err(|e| Error::Storage { source: e })?;
        Ok(tree
            .get(message_id.as_bytes())
            .map_err(|e| Error::Storage { source: e })?
            .map(|v| v.to_vec()))
    }

    /// Delete group messages older than `max_age`.
    ///
    /// Group message keys are `{group_id}:{timestamp:020}:{message_id}` where
    /// timestamp is milliseconds since Unix epoch. Entries whose timestamp
    /// predates `now - max_age` are removed.
    ///
    /// Returns the number of messages deleted.
    pub async fn cleanup_old_group_messages(&self, max_age: std::time::Duration) -> Result<usize> {
        let tree = self.group_tree()?;
        let cutoff_ms = chrono::Utc::now().timestamp_millis() - max_age.as_millis() as i64;

        let mut to_delete: Vec<Vec<u8>> = Vec::new();

        for entry in tree.iter() {
            let (key, _) = entry.map_err(|e| Error::Storage { source: e })?;
            let key_str = String::from_utf8_lossy(&key);

            // Key format: {group_id}:{timestamp:020}:{message_id}
            // Parse timestamp by splitting from the right (message_id and timestamp have no colons).
            if let Some(ts) = Self::parse_group_key_timestamp(&key_str) {
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
    use variance_proto::messaging_proto::{
        Group, GroupMember, GroupMessage, GroupRole, MessageType,
    };

    #[tokio::test]
    async fn test_store_and_fetch_group() {
        let dir = tempdir().unwrap();
        let storage = LocalMessageStorage::new(dir.path()).unwrap();

        let message = GroupMessage {
            id: "01ARZ3NDEKTSV4RRFFQ69G5FAV".to_string(),
            sender_did: "did:variance:alice".to_string(),
            group_id: "group123".to_string(),
            timestamp: 1000,
            r#type: MessageType::Text.into(),
            reply_to: None,
            mls_ciphertext: vec![1, 2, 3],
        };

        storage.store_group(&message).await.unwrap();

        let messages = storage.fetch_group("group123", 10, None).await.unwrap();

        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].id, message.id);
    }

    #[tokio::test]
    async fn test_cleanup_old_group_messages() {
        let dir = tempdir().unwrap();
        let storage = LocalMessageStorage::new(dir.path()).unwrap();

        let now_ms = chrono::Utc::now().timestamp_millis();

        // Old message: 100 days ago (should be deleted with a 90-day cutoff)
        let old_ts = now_ms - 100 * 86_400_000i64;
        let old_msg = GroupMessage {
            id: "01ARZ3NDEKTSV4RRFFQ69G5FAA".to_string(),
            group_id: "test-group".to_string(),
            sender_did: "did:key:alice".to_string(),
            mls_ciphertext: vec![1],
            timestamp: old_ts,
            ..Default::default()
        };

        // Recent message: 1 day ago (should survive)
        let recent_ts = now_ms - 86_400_000i64;
        let recent_msg = GroupMessage {
            id: "01ARZ3NDEKTSV4RRFFQ69G5FBB".to_string(),
            group_id: "test-group".to_string(),
            sender_did: "did:key:alice".to_string(),
            mls_ciphertext: vec![2],
            timestamp: recent_ts,
            ..Default::default()
        };

        storage.store_group(&old_msg).await.unwrap();
        storage.store_group(&recent_msg).await.unwrap();

        // Verify both stored before cleanup.
        let before = storage.fetch_group("test-group", 10, None).await.unwrap();
        assert_eq!(before.len(), 2);

        let deleted = storage
            .cleanup_old_group_messages(std::time::Duration::from_secs(90 * 86400))
            .await
            .unwrap();
        assert_eq!(deleted, 1, "only the 100-day-old message should be deleted");

        let after = storage.fetch_group("test-group", 10, None).await.unwrap();
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].id, "01ARZ3NDEKTSV4RRFFQ69G5FBB");
    }

    #[tokio::test]
    async fn test_fetch_group_since() {
        let dir = tempdir().unwrap();
        let storage = LocalMessageStorage::new(dir.path()).unwrap();

        let msgs: Vec<GroupMessage> = (1..=5)
            .map(|i| GroupMessage {
                id: format!("msg-{}", i),
                group_id: "group-a".to_string(),
                sender_did: "did:key:alice".to_string(),
                mls_ciphertext: vec![i as u8],
                timestamp: i * 1000,
                ..Default::default()
            })
            .collect();

        for m in &msgs {
            storage.store_group(m).await.unwrap();
        }

        // Fetch since timestamp 2000 → should get messages at 3000, 4000, 5000
        let result = storage
            .fetch_group_since("group-a", 2000, 100)
            .await
            .unwrap();
        assert_eq!(result.len(), 3);
        assert_eq!(result[0].timestamp, 3000);
        assert_eq!(result[1].timestamp, 4000);
        assert_eq!(result[2].timestamp, 5000);
    }

    #[tokio::test]
    async fn test_fetch_group_since_with_limit() {
        let dir = tempdir().unwrap();
        let storage = LocalMessageStorage::new(dir.path()).unwrap();

        for i in 1..=10 {
            let msg = GroupMessage {
                id: format!("msg-{}", i),
                group_id: "group-b".to_string(),
                sender_did: "did:key:alice".to_string(),
                mls_ciphertext: vec![i as u8],
                timestamp: i * 1000,
                ..Default::default()
            };
            storage.store_group(&msg).await.unwrap();
        }

        // Limit to 3
        let result = storage.fetch_group_since("group-b", 0, 3).await.unwrap();
        assert_eq!(result.len(), 3);
        assert_eq!(result[0].timestamp, 1000);
        assert_eq!(result[2].timestamp, 3000);
    }

    #[tokio::test]
    async fn test_fetch_group_since_empty() {
        let dir = tempdir().unwrap();
        let storage = LocalMessageStorage::new(dir.path()).unwrap();

        let result = storage
            .fetch_group_since("nonexistent", 0, 100)
            .await
            .unwrap();
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn test_fetch_group_since_isolates_groups() {
        let dir = tempdir().unwrap();
        let storage = LocalMessageStorage::new(dir.path()).unwrap();

        // Store messages in two different groups
        for gid in &["group-x", "group-y"] {
            for i in 1..=3 {
                let msg = GroupMessage {
                    id: format!("{}-msg-{}", gid, i),
                    group_id: gid.to_string(),
                    sender_did: "did:key:alice".to_string(),
                    mls_ciphertext: vec![i as u8],
                    timestamp: i * 1000,
                    ..Default::default()
                };
                storage.store_group(&msg).await.unwrap();
            }
        }

        // Fetch only group-x since 0
        let result = storage.fetch_group_since("group-x", 0, 100).await.unwrap();
        assert_eq!(result.len(), 3);
        assert!(result.iter().all(|m| m.group_id == "group-x"));
    }

    #[tokio::test]
    async fn test_latest_group_timestamp() {
        let dir = tempdir().unwrap();
        let storage = LocalMessageStorage::new(dir.path()).unwrap();

        // Empty group
        assert_eq!(
            storage.latest_group_timestamp("group-c").await.unwrap(),
            None
        );

        for i in 1..=3 {
            let msg = GroupMessage {
                id: format!("msg-{}", i),
                group_id: "group-c".to_string(),
                sender_did: "did:key:bob".to_string(),
                mls_ciphertext: vec![i as u8],
                timestamp: i * 1000,
                ..Default::default()
            };
            storage.store_group(&msg).await.unwrap();
        }

        assert_eq!(
            storage.latest_group_timestamp("group-c").await.unwrap(),
            Some(3000)
        );
    }

    #[tokio::test]
    async fn test_has_group_message() {
        let dir = tempdir().unwrap();
        let storage = LocalMessageStorage::new(dir.path()).unwrap();

        let msg = GroupMessage {
            id: "unique-msg-id".to_string(),
            group_id: "group-d".to_string(),
            sender_did: "did:key:alice".to_string(),
            mls_ciphertext: vec![1],
            timestamp: 5000,
            ..Default::default()
        };
        storage.store_group(&msg).await.unwrap();

        assert!(storage.has_group_message("group-d", "unique-msg-id").await);
        assert!(!storage.has_group_message("group-d", "nonexistent").await);
        assert!(
            !storage
                .has_group_message("other-group", "unique-msg-id")
                .await
        );
    }

    #[tokio::test]
    async fn test_update_member_role() {
        let dir = tempdir().unwrap();
        let storage = LocalMessageStorage::new(dir.path()).unwrap();

        let group = Group {
            id: "group-role-test".to_string(),
            name: "Role Test".to_string(),
            members: vec![
                GroupMember {
                    did: "did:key:admin".to_string(),
                    role: GroupRole::Admin as i32,
                    joined_at: 1000,
                    nickname: None,
                },
                GroupMember {
                    did: "did:key:bob".to_string(),
                    role: GroupRole::Member as i32,
                    joined_at: 2000,
                    nickname: None,
                },
            ],
            ..Default::default()
        };
        storage.store_group_metadata(&group).await.unwrap();

        // Promote bob to moderator.
        let updated = storage
            .update_member_role(
                "group-role-test",
                "did:key:bob",
                GroupRole::Moderator as i32,
            )
            .await
            .unwrap();
        assert!(updated);

        let meta = storage
            .fetch_group_metadata("group-role-test")
            .await
            .unwrap()
            .unwrap();
        let bob = meta
            .members
            .iter()
            .find(|m| m.did == "did:key:bob")
            .unwrap();
        assert_eq!(bob.role, GroupRole::Moderator as i32);

        // Demote bob back to member.
        let updated = storage
            .update_member_role("group-role-test", "did:key:bob", GroupRole::Member as i32)
            .await
            .unwrap();
        assert!(updated);

        let meta = storage
            .fetch_group_metadata("group-role-test")
            .await
            .unwrap()
            .unwrap();
        let bob = meta
            .members
            .iter()
            .find(|m| m.did == "did:key:bob")
            .unwrap();
        assert_eq!(bob.role, GroupRole::Member as i32);

        // Unknown member returns false.
        let updated = storage
            .update_member_role(
                "group-role-test",
                "did:key:unknown",
                GroupRole::Moderator as i32,
            )
            .await
            .unwrap();
        assert!(!updated);

        // Unknown group returns false.
        let updated = storage
            .update_member_role(
                "nonexistent-group",
                "did:key:bob",
                GroupRole::Moderator as i32,
            )
            .await
            .unwrap();
        assert!(!updated);
    }

    /// Proves the pagination bug: when a group has more messages than the
    /// default page limit (50), `fetch_group` should return the **newest**
    /// messages (like DM pagination does). Instead, the current forward-scan
    /// implementation returns the oldest N, so newly sent messages beyond
    /// the limit are silently dropped from the response and never rendered
    /// in the chat view.
    #[tokio::test]
    async fn test_fetch_group_returns_newest_messages_when_over_limit() {
        let dir = tempdir().unwrap();
        let storage = LocalMessageStorage::new(dir.path()).unwrap();

        let total = 60;
        let limit = 50;

        // Insert 60 messages with timestamps 1000..60000.
        for i in 1..=total {
            let msg = GroupMessage {
                id: format!("msg-{:04}", i),
                group_id: "pagination-group".to_string(),
                sender_did: "did:key:alice".to_string(),
                mls_ciphertext: vec![i as u8],
                timestamp: i as i64 * 1000,
                ..Default::default()
            };
            storage.store_group(&msg).await.unwrap();
        }

        let result = storage
            .fetch_group("pagination-group", limit, None)
            .await
            .unwrap();

        assert_eq!(result.len(), limit);

        // The result MUST contain the newest messages (timestamps 11000..60000),
        // not the oldest (1000..50000). A user opening a chat should see recent
        // messages, not messages from the beginning of time.
        let newest_msg = result.last().unwrap();
        assert_eq!(
            newest_msg.timestamp,
            total as i64 * 1000,
            "The last message returned must be the newest one (timestamp {}), \
             but got timestamp {}. fetch_group scans forward from the oldest \
             key instead of backward from the newest, so messages beyond the \
             page limit are never returned.",
            total as i64 * 1000,
            newest_msg.timestamp,
        );

        let oldest_msg = result.first().unwrap();
        assert_eq!(
            oldest_msg.timestamp,
            (total as i64 - limit as i64 + 1) * 1000,
            "The first message returned must be the oldest within the page \
             (timestamp {}), not timestamp {}.",
            (total as i64 - limit as i64 + 1) * 1000,
            oldest_msg.timestamp,
        );

        // Results must be in chronological order (oldest first within the page).
        for w in result.windows(2) {
            assert!(
                w[0].timestamp <= w[1].timestamp,
                "Messages must be in chronological order, but {} came before {}",
                w[0].timestamp,
                w[1].timestamp,
            );
        }
    }

    /// Proves that after fetch_group returns the newest page, sending one
    /// more message should make it appear in the next fetch (simulating the
    /// real-time chat flow where a WS event triggers a refetch).
    #[tokio::test]
    async fn test_new_message_appears_after_exceeding_page_size() {
        let dir = tempdir().unwrap();
        let storage = LocalMessageStorage::new(dir.path()).unwrap();

        let limit = 50;

        // Fill exactly at the limit.
        for i in 1..=limit {
            let msg = GroupMessage {
                id: format!("msg-{:04}", i),
                group_id: "realtime-group".to_string(),
                sender_did: "did:key:alice".to_string(),
                mls_ciphertext: vec![i as u8],
                timestamp: i as i64 * 1000,
                ..Default::default()
            };
            storage.store_group(&msg).await.unwrap();
        }

        // At exactly the limit, the newest message must be present.
        let page = storage
            .fetch_group("realtime-group", limit, None)
            .await
            .unwrap();
        assert_eq!(page.len(), limit);
        assert_eq!(page.last().unwrap().timestamp, limit as i64 * 1000);

        // Now a new message arrives (simulating a WS-triggered store + refetch).
        let new_msg = GroupMessage {
            id: "msg-new".to_string(),
            group_id: "realtime-group".to_string(),
            sender_did: "did:key:bob".to_string(),
            mls_ciphertext: vec![99],
            timestamp: (limit as i64 + 1) * 1000,
            ..Default::default()
        };
        storage.store_group(&new_msg).await.unwrap();

        // Refetch: the new message MUST appear in the result.
        let page = storage
            .fetch_group("realtime-group", limit, None)
            .await
            .unwrap();
        assert_eq!(page.len(), limit);
        assert_eq!(
            page.last().unwrap().id,
            "msg-new",
            "A newly sent message (id='msg-new') must appear in the fetch_group \
             result even when the group has more than {} total messages. \
             Currently it is missing because fetch_group scans forward and \
             the new message falls outside the first {} keys.",
            limit,
            limit,
        );
    }
}
