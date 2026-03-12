use async_trait::async_trait;
use variance_proto::messaging_proto::{
    DirectMessage, Group, GroupInvitation, GroupMessage, OfflineMessageEnvelope, ReadReceipt,
};

use crate::error::Result;

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

    /// Fetch offline messages for a recipient by their mailbox token.
    async fn fetch_offline(
        &self,
        mailbox_token: &[u8],
        since: Option<i64>,
        limit: usize,
    ) -> Result<Vec<OfflineMessageEnvelope>>;

    /// Count queued offline messages for a mailbox token (used to enforce per-mailbox limits).
    async fn count_offline_for_mailbox(&self, mailbox_token: &[u8]) -> Result<usize>;

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

    /// Fetch metadata for a single group by ID.
    async fn fetch_group_metadata(&self, group_id: &str) -> Result<Option<Group>>;

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

    /// Persist an outgoing receipt that could not be delivered because the target peer
    /// was offline. Keyed by `target_did` so all pending receipts for a peer can be
    /// drained at once when they reconnect.
    async fn store_pending_receipt(&self, target_did: &str, receipt: &ReadReceipt) -> Result<()>;

    /// Drain all pending receipts for `target_did`, deleting them from storage.
    ///
    /// Called when the peer reconnects so we can attempt delivery immediately.
    async fn drain_pending_receipts(&self, target_did: &str) -> Result<Vec<ReadReceipt>>;

    // ===== Group invitation persistence =====

    /// Store a pending group invitation (invitee side).
    ///
    /// Keyed by group_id — only one pending invite per group. A newer invite
    /// for the same group overwrites the previous one.
    async fn store_pending_invitation(&self, invitation: &GroupInvitation) -> Result<()>;

    /// Fetch all pending invitations.
    async fn fetch_pending_invitations(&self) -> Result<Vec<GroupInvitation>>;

    /// Fetch a single pending invitation by group ID.
    async fn fetch_pending_invitation(&self, group_id: &str) -> Result<Option<GroupInvitation>>;

    /// Delete a pending invitation (after accepting or declining).
    async fn delete_pending_invitation(&self, group_id: &str) -> Result<()>;

    /// Store an outbound invite (sender/admin side).
    ///
    /// `created_at_ms` is used for the 5-minute timeout.
    async fn store_outbound_invite(
        &self,
        group_id: &str,
        invitee_did: &str,
        invitation: &GroupInvitation,
        created_at_ms: i64,
    ) -> Result<()>;

    /// Fetch an outbound invite by (group_id, invitee_did).
    ///
    /// Returns `(invitation, created_at_ms)` if found.
    async fn fetch_outbound_invite(
        &self,
        group_id: &str,
        invitee_did: &str,
    ) -> Result<Option<(GroupInvitation, i64)>>;

    /// Delete an outbound invite (after accept/decline/timeout).
    async fn delete_outbound_invite(&self, group_id: &str, invitee_did: &str) -> Result<()>;

    /// Fetch all outbound invites that have expired.
    ///
    /// An invite is expired when `created_at_ms + timeout_ms < now_ms`.
    /// Returns `(group_id, invitee_did, GroupInvitation)` triples.
    async fn fetch_expired_outbound_invites(
        &self,
        timeout_ms: i64,
        now_ms: i64,
    ) -> Result<Vec<(String, String, GroupInvitation)>>;
}
