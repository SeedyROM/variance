use crate::error::*;
use async_trait::async_trait;
use prost::Message;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use variance_proto::messaging_proto::{
    DirectMessage, Group, GroupMessage, OfflineMessageEnvelope, ReadReceipt,
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

    /// Fetch the most recent `limit` direct messages for a conversation.
    ///
    /// Results are returned in chronological order (oldest first).
    /// `before` is an exclusive upper bound on `message.timestamp` (ms) for
    /// cursor-based backwards pagination — pass the oldest timestamp from the
    /// previous page to load the page before it.
    async fn fetch_direct(
        &self,
        sender_did: &str,
        recipient_did: &str,
        limit: usize,
        before: Option<i64>,
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

    /// Fetch the most recent group message (for last-activity timestamps).
    async fn fetch_group_latest(&self, group_id: &str) -> Result<Option<GroupMessage>>;

    /// Fetch group messages with `timestamp > since_timestamp`, oldest first.
    ///
    /// Used by the group-sync protocol to serve missed messages to a
    /// reconnecting peer.
    async fn fetch_group_since(
        &self,
        group_id: &str,
        since_timestamp: i64,
        limit: usize,
    ) -> Result<Vec<GroupMessage>>;

    /// Return the timestamp (ms) of the newest stored message for a group,
    /// or `None` if the group has no messages.
    async fn latest_group_timestamp(&self, group_id: &str) -> Result<Option<i64>>;

    /// Check whether a group message is already stored (by group_id + message_id).
    ///
    /// Used during sync to skip duplicates without reading the full message.
    async fn has_group_message(&self, group_id: &str, message_id: &str) -> bool;

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

    /// Clean up expired offline messages (TTL enforcement).
    ///
    /// Called hourly by the background cleanup task in `variance-app`.
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
    /// Returns `(peer_did, latest_timestamp, latest_peer_timestamp)` triples sorted by
    /// `latest_timestamp` descending. `latest_peer_timestamp` is the timestamp of the most
    /// recent message sent **by the peer** (i.e. `sender_did != local_did`), or `None` if the
    /// local user has only sent messages and received none.
    async fn list_direct_conversations(
        &self,
        local_did: &str,
    ) -> Result<Vec<(String, i64, Option<i64>)>>;

    /// Record that `our_did` has read all messages with `peer_did` up to `timestamp` (ms,
    /// inclusive). Stored as little-endian i64 bytes.
    async fn store_last_read_at(&self, our_did: &str, peer_did: &str, timestamp: i64)
        -> Result<()>;

    /// Fetch the last-read timestamp for a conversation. Returns `None` if never read.
    async fn fetch_last_read_at(&self, our_did: &str, peer_did: &str) -> Result<Option<i64>>;

    /// Record that `our_did` has read all messages in `group_id` up to `timestamp` (ms).
    async fn store_group_last_read_at(
        &self,
        our_did: &str,
        group_id: &str,
        timestamp: i64,
    ) -> Result<()>;

    /// Fetch the last-read timestamp for a group. Returns `None` if never read.
    async fn fetch_group_last_read_at(&self, our_did: &str, group_id: &str) -> Result<Option<i64>>;

    /// Delete all messages in a direct conversation.
    async fn delete_direct_conversation(&self, did1: &str, did2: &str) -> Result<()>;

    /// Delete a single direct message by its composite key fields.
    ///
    /// Used to remove control messages (e.g. MLS Welcome) that were decrypted
    /// and stored but should not appear in the conversation.
    async fn delete_direct_by_id(
        &self,
        sender_did: &str,
        recipient_did: &str,
        timestamp: i64,
        message_id: &str,
    ) -> Result<()>;

    /// Persist encrypted plaintext for a message so it can be read after restart.
    ///
    /// `encrypted` is `nonce (12 bytes) || AES-256-GCM ciphertext` produced by
    /// `DirectMessageHandler`. The storage layer treats it as opaque bytes.
    async fn store_plaintext(&self, message_id: &str, encrypted: &[u8]) -> Result<()>;

    /// Retrieve previously stored encrypted plaintext for a message, or `None`
    /// if the message has not been decrypted in this or a prior session.
    async fn fetch_plaintext(&self, message_id: &str) -> Result<Option<Vec<u8>>>;

    /// Store a pickled Olm session for a peer DID.
    ///
    /// Sessions must be persisted to survive restarts. vodozemac `Session::pickle()`
    /// produces JSON that can be restored via `Session::from_pickle()`.
    async fn store_session_pickle(&self, peer_did: &str, pickle_json: &str) -> Result<()>;

    /// Fetch a pickled Olm session for a peer DID.
    async fn fetch_session_pickle(&self, peer_did: &str) -> Result<Option<String>>;

    /// Load all stored session pickles (for restoring sessions on startup).
    async fn load_all_session_pickles(&self) -> Result<Vec<(String, String)>>;

    /// Store a pending message that couldn't be sent due to peer being offline.
    ///
    /// Messages are queued with their fully encrypted DirectMessage proto,
    /// ready to send when the peer reconnects.
    async fn store_pending_message(&self, peer_did: &str, message: &DirectMessage) -> Result<()>;

    /// Fetch all pending messages for a specific peer.
    async fn fetch_pending_messages(&self, peer_did: &str) -> Result<Vec<DirectMessage>>;

    /// Delete a pending message after successful transmission.
    async fn delete_pending_message(&self, message_id: &str) -> Result<()>;

    /// Check if a specific message is in the pending queue.
    async fn is_message_pending(&self, message_id: &str) -> Result<bool>;

    /// List all peer DIDs that have pending messages.
    async fn list_peers_with_pending_messages(&self) -> Result<Vec<String>>;

    // ===== Group metadata persistence =====

    /// Persist group membership/metadata (without the raw key — that goes in store_group_key_encrypted).
    async fn store_group_metadata(&self, group: &Group) -> Result<()>;

    /// Fetch all stored group metadata records (used at startup to restore in-memory state).
    async fn fetch_all_group_metadata(&self) -> Result<Vec<Group>>;

    /// Delete all stored messages for a group.
    async fn delete_group_messages(&self, group_id: &str) -> Result<()>;

    /// Delete the metadata record for a group.
    async fn delete_group_metadata(&self, group_id: &str) -> Result<()>;

    // ===== MLS provider state persistence =====

    /// Persist the full MLS provider state for a local identity.
    ///
    /// `state` is the output of `MlsGroupHandler::export_state()`. It contains the
    /// complete openmls `MemoryStorage` key-value map (ratchet trees, epoch secrets,
    /// leaf nodes, signature keypair) plus the list of active group IDs. This single
    /// blob is sufficient to fully reconstruct `MlsGroupHandler` on restart via
    /// `restore_in_place()`.
    async fn store_mls_state(&self, local_did: &str, state: &[u8]) -> Result<()>;

    /// Fetch the persisted MLS provider state for a local identity.
    ///
    /// Returns `None` on first run or if no groups have been created yet.
    async fn fetch_mls_state(&self, local_did: &str) -> Result<Option<Vec<u8>>>;

    // ===== Peer display name persistence =====

    /// Persist a peer's display name (username + discriminator) keyed by DID.
    ///
    /// Stored as `"username#discriminator"` so the registry can be seeded on
    /// restart without requiring another P2P identity resolution.
    async fn store_peer_name(&self, did: &str, username: &str, discriminator: u32) -> Result<()>;

    /// Load all stored peer display names.
    ///
    /// Returns `(did, username, discriminator)` triples for seeding the
    /// `UsernameRegistry` at startup.
    async fn load_all_peer_names(&self) -> Result<Vec<(String, String, u32)>>;
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

    /// Encrypted plaintext cache tree (message_id → nonce || ciphertext)
    fn plaintext_tree(&self) -> Result<sled::Tree> {
        self.db
            .open_tree("plaintext_cache")
            .map_err(|e| Error::Storage { source: e })
    }

    /// Olm session pickles tree (peer_did → JSON pickle)
    fn session_pickles_tree(&self) -> Result<sled::Tree> {
        self.db
            .open_tree("session_pickles")
            .map_err(|e| Error::Storage { source: e })
    }

    /// Pending messages tree (peer_did:message_id → DirectMessage)
    fn pending_messages_tree(&self) -> Result<sled::Tree> {
        self.db
            .open_tree("pending_messages")
            .map_err(|e| Error::Storage { source: e })
    }

    /// Group metadata tree (group_id → serialized Group proto, key cleared)
    fn group_metadata_tree(&self) -> Result<sled::Tree> {
        self.db
            .open_tree("group_metadata")
            .map_err(|e| Error::Storage { source: e })
    }

    /// MLS provider state tree (local_did → serialized MlsStateSnapshot JSON bytes)
    fn mls_state_tree(&self) -> Result<sled::Tree> {
        self.db
            .open_tree("mls_provider_state")
            .map_err(|e| Error::Storage { source: e })
    }

    /// Last-read-at tree — key: `"{our_did}::{peer_did}"`, value: i64 ms timestamp (LE bytes)
    fn last_read_at_tree(&self) -> Result<sled::Tree> {
        self.db
            .open_tree("last_read_at")
            .map_err(|e| Error::Storage { source: e })
    }

    /// Peer display names tree (did → "username#discriminator")
    fn peer_names_tree(&self) -> Result<sled::Tree> {
        self.db
            .open_tree("peer_names")
            .map_err(|e| Error::Storage { source: e })
    }

    /// Reverse index tree (idx:{message_id} → tree_name:full_key)
    ///
    /// Enables O(1) lookup by message ID for offline and pending messages,
    /// replacing the previous O(n) full-tree scans.
    fn message_index_tree(&self) -> Result<sled::Tree> {
        self.db
            .open_tree("message_index")
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

    async fn fetch_group_latest(&self, group_id: &str) -> Result<Option<GroupMessage>> {
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

    async fn fetch_group_since(
        &self,
        group_id: &str,
        since_timestamp: i64,
        limit: usize,
    ) -> Result<Vec<GroupMessage>> {
        let tree = self.group_tree()?;
        // Keys are "{group_id}:{timestamp:020}:{id}" — lexicographic scan
        // starting just after the given timestamp.
        let start = format!("{}:{:020}:", group_id, since_timestamp + 1);
        let prefix = format!("{}:", group_id);

        let mut messages = Vec::new();
        for entry in tree.range(start.as_bytes()..) {
            let (key, value) = entry.map_err(|e| Error::Storage { source: e })?;

            // Stop once we leave this group's prefix.
            let key_str = String::from_utf8_lossy(&key);
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

    async fn latest_group_timestamp(&self, group_id: &str) -> Result<Option<i64>> {
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

    async fn has_group_message(&self, group_id: &str, message_id: &str) -> bool {
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

    async fn store_offline(&self, envelope: &OfflineMessageEnvelope) -> Result<()> {
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

        let key = Self::offline_key(&envelope.recipient_did, timestamp, id);

        let bytes = prost::Message::encode_to_vec(envelope);
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

    async fn list_direct_conversations(
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

    async fn store_plaintext(&self, message_id: &str, encrypted: &[u8]) -> Result<()> {
        let tree = self.plaintext_tree()?;
        tree.insert(message_id.as_bytes(), encrypted)
            .map_err(|e| Error::Storage { source: e })?;
        Ok(())
    }

    async fn fetch_plaintext(&self, message_id: &str) -> Result<Option<Vec<u8>>> {
        let tree = self.plaintext_tree()?;
        Ok(tree
            .get(message_id.as_bytes())
            .map_err(|e| Error::Storage { source: e })?
            .map(|v| v.to_vec()))
    }

    async fn delete_direct_conversation(&self, did1: &str, did2: &str) -> Result<()> {
        let tree = self.direct_tree()?;
        let conv_id = Self::conversation_id(did1, did2);
        let prefix = format!("{conv_id}:");

        let keys_to_delete: Vec<sled::IVec> = tree
            .scan_prefix(prefix.as_bytes())
            .keys()
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| Error::Storage { source: e })?;

        for key in keys_to_delete {
            tree.remove(key).map_err(|e| Error::Storage { source: e })?;
        }

        Ok(())
    }

    async fn delete_direct_by_id(
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

        // Also remove the stored plaintext cache entry
        if let Ok(pt_tree) = self.plaintext_tree() {
            let _ = pt_tree.remove(message_id.as_bytes());
        }

        Ok(())
    }

    async fn store_session_pickle(&self, peer_did: &str, pickle_json: &str) -> Result<()> {
        let tree = self.session_pickles_tree()?;
        tree.insert(peer_did.as_bytes(), pickle_json.as_bytes())
            .map_err(|e| Error::Storage { source: e })?;
        Ok(())
    }

    async fn fetch_session_pickle(&self, peer_did: &str) -> Result<Option<String>> {
        let tree = self.session_pickles_tree()?;
        Ok(tree
            .get(peer_did.as_bytes())
            .map_err(|e| Error::Storage { source: e })?
            .map(|v| String::from_utf8_lossy(&v).to_string()))
    }

    async fn load_all_session_pickles(&self) -> Result<Vec<(String, String)>> {
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

    async fn store_pending_message(&self, peer_did: &str, message: &DirectMessage) -> Result<()> {
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

    async fn fetch_pending_messages(&self, peer_did: &str) -> Result<Vec<DirectMessage>> {
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

    async fn delete_pending_message(&self, message_id: &str) -> Result<()> {
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

    async fn is_message_pending(&self, message_id: &str) -> Result<bool> {
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

    async fn list_peers_with_pending_messages(&self) -> Result<Vec<String>> {
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

    async fn store_group_metadata(&self, group: &Group) -> Result<()> {
        let tree = self.group_metadata_tree()?;
        let bytes = prost::Message::encode_to_vec(group);
        tree.insert(group.id.as_bytes(), bytes.as_slice())
            .map_err(|e| Error::Storage { source: e })?;
        Ok(())
    }

    async fn fetch_all_group_metadata(&self) -> Result<Vec<Group>> {
        let tree = self.group_metadata_tree()?;
        let mut groups = Vec::new();
        for entry in tree.iter() {
            let (_, value) = entry.map_err(|e| Error::Storage { source: e })?;
            let group = Group::decode(value.as_ref()).map_err(|e| Error::Protocol { source: e })?;
            groups.push(group);
        }
        Ok(groups)
    }

    async fn delete_group_messages(&self, group_id: &str) -> Result<()> {
        let tree = self.group_tree()?;
        let prefix = format!("{}:", group_id);
        let keys_to_delete: Vec<sled::IVec> = tree
            .scan_prefix(prefix.as_bytes())
            .filter_map(|r| r.ok().map(|(k, _)| k))
            .collect();
        for key in keys_to_delete {
            tree.remove(&key)
                .map_err(|e| Error::Storage { source: e })?;
        }
        Ok(())
    }

    async fn delete_group_metadata(&self, group_id: &str) -> Result<()> {
        let tree = self.group_metadata_tree()?;
        tree.remove(group_id.as_bytes())
            .map_err(|e| Error::Storage { source: e })?;
        Ok(())
    }

    async fn store_last_read_at(
        &self,
        our_did: &str,
        peer_did: &str,
        timestamp: i64,
    ) -> Result<()> {
        let tree = self.last_read_at_tree()?;
        let key = format!("{}::{}", our_did, peer_did);
        tree.insert(key.as_bytes(), &timestamp.to_le_bytes())
            .map_err(|e| Error::Storage { source: e })?;
        Ok(())
    }

    async fn fetch_last_read_at(&self, our_did: &str, peer_did: &str) -> Result<Option<i64>> {
        let tree = self.last_read_at_tree()?;
        let key = format!("{}::{}", our_did, peer_did);
        Ok(tree
            .get(key.as_bytes())
            .map_err(|e| Error::Storage { source: e })?
            .map(|v| {
                let bytes: [u8; 8] = v.as_ref().try_into().unwrap_or([0u8; 8]);
                i64::from_le_bytes(bytes)
            }))
    }

    async fn store_group_last_read_at(
        &self,
        our_did: &str,
        group_id: &str,
        timestamp: i64,
    ) -> Result<()> {
        let tree = self.last_read_at_tree()?;
        let key = format!("{}::group::{}", our_did, group_id);
        tree.insert(key.as_bytes(), &timestamp.to_le_bytes())
            .map_err(|e| Error::Storage { source: e })?;
        Ok(())
    }

    async fn fetch_group_last_read_at(&self, our_did: &str, group_id: &str) -> Result<Option<i64>> {
        let tree = self.last_read_at_tree()?;
        let key = format!("{}::group::{}", our_did, group_id);
        Ok(tree
            .get(key.as_bytes())
            .map_err(|e| Error::Storage { source: e })?
            .map(|v| {
                let bytes: [u8; 8] = v.as_ref().try_into().unwrap_or([0u8; 8]);
                i64::from_le_bytes(bytes)
            }))
    }

    async fn store_mls_state(&self, local_did: &str, state: &[u8]) -> Result<()> {
        let tree = self.mls_state_tree()?;
        tree.insert(local_did.as_bytes(), state)
            .map_err(|e| Error::Storage { source: e })?;
        Ok(())
    }

    async fn fetch_mls_state(&self, local_did: &str) -> Result<Option<Vec<u8>>> {
        let tree = self.mls_state_tree()?;
        Ok(tree
            .get(local_did.as_bytes())
            .map_err(|e| Error::Storage { source: e })?
            .map(|v| v.to_vec()))
    }

    async fn store_peer_name(&self, did: &str, username: &str, discriminator: u32) -> Result<()> {
        let tree = self.peer_names_tree()?;
        let value = format!("{username}#{discriminator:04}");
        tree.insert(did.as_bytes(), value.as_bytes())
            .map_err(|e| Error::Storage { source: e })?;
        Ok(())
    }

    async fn load_all_peer_names(&self) -> Result<Vec<(String, String, u32)>> {
        let tree = self.peer_names_tree()?;
        let mut result = Vec::new();
        for item in tree.iter() {
            let (k, v) = item.map_err(|e| Error::Storage { source: e })?;
            let did = String::from_utf8_lossy(&k).into_owned();
            let formatted = String::from_utf8_lossy(&v).into_owned();
            if let Some((username, disc_str)) = formatted.rsplit_once('#') {
                if let Ok(disc) = disc_str.parse::<u32>() {
                    result.push((did, username.to_string(), disc));
                }
            }
        }
        Ok(result)
    }
}

impl LocalMessageStorage {
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

    /// Extract the timestamp (ms) from a group message key.
    ///
    /// Key format: `{group_id}:{timestamp:020}:{message_id}`.
    /// Neither the 20-digit timestamp nor the ULID message ID contain colons,
    /// so we can split from the right unambiguously regardless of the group ID.
    fn parse_group_key_timestamp(key: &str) -> Option<i64> {
        let last = key.rfind(':')?;
        let before_id = &key[..last];
        let ts_start = before_id.rfind(':')?;
        before_id[ts_start + 1..].parse::<i64>().ok()
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
            recipient_did: "did:variance:bob".to_string(),
            message: Some(
                variance_proto::messaging_proto::offline_message_envelope::Message::Direct(direct),
            ),
            stored_at: 3000,
            expires_at: i64::MAX,
        };

        // Store creates index entry
        storage.store_offline(&envelope).await.unwrap();

        let messages = storage
            .fetch_offline("did:variance:bob", None, 10)
            .await
            .unwrap();
        assert_eq!(messages.len(), 1);

        // O(1) delete via index
        storage.delete_offline("01OFFLINE_MSG_ID").await.unwrap();

        let messages = storage
            .fetch_offline("did:variance:bob", None, 10)
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

    #[tokio::test]
    async fn test_last_read_at_round_trip() {
        let dir = tempdir().unwrap();
        let storage = LocalMessageStorage::new(dir.path()).unwrap();

        // Initially absent
        let result = storage
            .fetch_last_read_at("did:variance:alice", "did:variance:bob")
            .await
            .unwrap();
        assert!(result.is_none());

        // Store a timestamp
        storage
            .store_last_read_at("did:variance:alice", "did:variance:bob", 123_456_789)
            .await
            .unwrap();

        let result = storage
            .fetch_last_read_at("did:variance:alice", "did:variance:bob")
            .await
            .unwrap();
        assert_eq!(result, Some(123_456_789));
    }

    #[tokio::test]
    async fn test_last_read_at_overwrite() {
        let dir = tempdir().unwrap();
        let storage = LocalMessageStorage::new(dir.path()).unwrap();

        storage
            .store_last_read_at("did:variance:alice", "did:variance:bob", 1000)
            .await
            .unwrap();
        storage
            .store_last_read_at("did:variance:alice", "did:variance:bob", 9999)
            .await
            .unwrap();

        let result = storage
            .fetch_last_read_at("did:variance:alice", "did:variance:bob")
            .await
            .unwrap();
        assert_eq!(result, Some(9999));
    }

    #[tokio::test]
    async fn test_last_read_at_per_conversation() {
        let dir = tempdir().unwrap();
        let storage = LocalMessageStorage::new(dir.path()).unwrap();

        // Different conversations are stored independently
        storage
            .store_last_read_at("did:variance:alice", "did:variance:bob", 1000)
            .await
            .unwrap();
        storage
            .store_last_read_at("did:variance:alice", "did:variance:charlie", 2000)
            .await
            .unwrap();

        let bob_ts = storage
            .fetch_last_read_at("did:variance:alice", "did:variance:bob")
            .await
            .unwrap();
        let charlie_ts = storage
            .fetch_last_read_at("did:variance:alice", "did:variance:charlie")
            .await
            .unwrap();

        assert_eq!(bob_ts, Some(1000));
        assert_eq!(charlie_ts, Some(2000));
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
}
