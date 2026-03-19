mod direct;
mod group;
mod invitations;
mod offline;
mod peer;
mod receipts;
mod trait_def;

pub use trait_def::MessageStorage;

use crate::error::*;
use async_trait::async_trait;
use std::path::Path;
use variance_proto::messaging_proto::{
    DirectMessage, Group, GroupInvitation, GroupMessage, GroupReadReceipt, OfflineMessageEnvelope,
    ReadReceipt,
};

/// Local storage implementation using sled
///
/// Stores messages in embedded key-value database:
/// - Direct messages: indexed by conversation ID (sorted pair of DIDs)
/// - Group messages: indexed by group ID
/// - Offline messages: indexed by recipient DID with TTL
pub struct LocalMessageStorage {
    pub(crate) db: sled::Db,
}

impl LocalMessageStorage {
    /// Create a new local message storage instance
    pub fn new<P: AsRef<Path>>(path: P) -> Result<Self> {
        let db = sled::open(path).map_err(|e| Error::Storage { source: e })?;
        Ok(Self { db })
    }

    // ===== Tree accessors =====

    /// Direct messages tree
    pub(crate) fn direct_tree(&self) -> Result<sled::Tree> {
        self.db
            .open_tree("direct_messages")
            .map_err(|e| Error::Storage { source: e })
    }

    /// Group messages tree
    pub(crate) fn group_tree(&self) -> Result<sled::Tree> {
        self.db
            .open_tree("group_messages")
            .map_err(|e| Error::Storage { source: e })
    }

    /// Offline messages tree
    pub(crate) fn offline_tree(&self) -> Result<sled::Tree> {
        self.db
            .open_tree("offline_messages")
            .map_err(|e| Error::Storage { source: e })
    }

    /// Read receipts tree
    pub(crate) fn receipts_tree(&self) -> Result<sled::Tree> {
        self.db
            .open_tree("read_receipts")
            .map_err(|e| Error::Storage { source: e })
    }

    /// Encrypted plaintext cache tree (message_id → nonce || ciphertext)
    pub(crate) fn plaintext_tree(&self) -> Result<sled::Tree> {
        self.db
            .open_tree("plaintext_cache")
            .map_err(|e| Error::Storage { source: e })
    }

    /// Olm session pickles tree (peer_did → JSON pickle)
    pub(crate) fn session_pickles_tree(&self) -> Result<sled::Tree> {
        self.db
            .open_tree("session_pickles")
            .map_err(|e| Error::Storage { source: e })
    }

    /// Pending messages tree (peer_did:message_id → DirectMessage)
    pub(crate) fn pending_messages_tree(&self) -> Result<sled::Tree> {
        self.db
            .open_tree("pending_messages")
            .map_err(|e| Error::Storage { source: e })
    }

    /// Group metadata tree (group_id → serialized Group proto, key cleared)
    pub(crate) fn group_metadata_tree(&self) -> Result<sled::Tree> {
        self.db
            .open_tree("group_metadata")
            .map_err(|e| Error::Storage { source: e })
    }

    /// MLS provider state tree (local_did → serialized MlsStateSnapshot JSON bytes)
    pub(crate) fn mls_state_tree(&self) -> Result<sled::Tree> {
        self.db
            .open_tree("mls_provider_state")
            .map_err(|e| Error::Storage { source: e })
    }

    /// Last-read-at tree — key: `"{our_did}::{peer_did}"`, value: i64 ms timestamp (LE bytes)
    pub(crate) fn last_read_at_tree(&self) -> Result<sled::Tree> {
        self.db
            .open_tree("last_read_at")
            .map_err(|e| Error::Storage { source: e })
    }

    /// Peer display names tree (did → "username#discriminator")
    pub(crate) fn peer_names_tree(&self) -> Result<sled::Tree> {
        self.db
            .open_tree("peer_names")
            .map_err(|e| Error::Storage { source: e })
    }

    /// Pending outgoing receipts tree.
    ///
    /// Key: `{target_did}:{message_id}`, value: serialized `ReadReceipt` proto.
    /// Drained when the target peer reconnects.
    pub(crate) fn pending_receipts_tree(&self) -> Result<sled::Tree> {
        self.db
            .open_tree("pending_receipts")
            .map_err(|e| Error::Storage { source: e })
    }

    /// Group read receipts tree (per-member, per-message).
    ///
    /// Key: `{group_id}:{message_id}:{reader_did}:{timestamp:020}`,
    /// value: serialized `GroupReadReceipt` proto.
    pub(crate) fn group_receipts_tree(&self) -> Result<sled::Tree> {
        self.db
            .open_tree("group_receipts")
            .map_err(|e| Error::Storage { source: e })
    }

    /// Reverse index tree (idx:{message_id} → tree_name:full_key)
    ///
    /// Enables O(1) lookup by message ID for offline and pending messages,
    /// replacing the previous O(n) full-tree scans.
    pub(crate) fn message_index_tree(&self) -> Result<sled::Tree> {
        self.db
            .open_tree("message_index")
            .map_err(|e| Error::Storage { source: e })
    }

    // ===== Key generation / parsing helpers =====

    /// Generate conversation ID from two DIDs (sorted for consistency)
    pub(crate) fn conversation_id(did1: &str, did2: &str) -> String {
        let mut dids = [did1, did2];
        dids.sort();
        format!("{}:{}", dids[0], dids[1])
    }

    /// Generate storage key: conversation_id:timestamp:message_id
    pub(crate) fn direct_key(sender: &str, recipient: &str, timestamp: i64, id: &str) -> String {
        let conv_id = Self::conversation_id(sender, recipient);
        format!("{conv_id}:{timestamp:020}:{id}")
    }

    /// Generate group message key: group_id:timestamp:message_id
    pub(crate) fn group_key(group_id: &str, timestamp: i64, id: &str) -> String {
        format!("{group_id}:{timestamp:020}:{id}")
    }

    /// Generate offline message key: hex(mailbox_token):timestamp:message_id
    pub(crate) fn offline_key(mailbox_token: &[u8], timestamp: i64, id: &str) -> String {
        format!("{}:{timestamp:020}:{id}", hex::encode(mailbox_token))
    }

    /// Parse a direct message key to extract `(conv_id, timestamp)`.
    ///
    /// Key format: `{conv_id}:{timestamp:020}:{msg_id}`.
    /// Since neither the 20-digit timestamp nor the ULID message ID contain
    /// colons, we can reliably split from the right.
    pub(crate) fn parse_direct_key(key: &str) -> Option<(&str, i64)> {
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
    pub(crate) fn peer_did_from_conv_id(conv_id: &str, local_did: &str) -> Option<String> {
        if let Some(rest) = conv_id.strip_prefix(&format!("{local_did}:")) {
            Some(rest.to_string())
        } else {
            conv_id
                .strip_suffix(&format!(":{local_did}"))
                .map(|rest| rest.to_string())
        }
    }

    /// Extract the timestamp (ms) from a group message key.
    ///
    /// Key format: `{group_id}:{timestamp:020}:{message_id}`.
    /// Neither the 20-digit timestamp nor the ULID message ID contain colons,
    /// so we can split from the right unambiguously regardless of the group ID.
    pub(crate) fn parse_group_key_timestamp(key: &str) -> Option<i64> {
        let last = key.rfind(':')?;
        let before_id = &key[..last];
        let ts_start = before_id.rfind(':')?;
        before_id[ts_start + 1..].parse::<i64>().ok()
    }
}

// ===== Delegation: impl MessageStorage for LocalMessageStorage =====
//
// Each method delegates to an `impl_*` inherent method defined in the
// domain-specific submodule (direct.rs, group.rs, offline.rs, etc.).

#[async_trait]
impl MessageStorage for LocalMessageStorage {
    async fn store_direct(&self, message: &DirectMessage) -> Result<()> {
        self.impl_store_direct(message).await
    }

    async fn fetch_direct(
        &self,
        sender_did: &str,
        recipient_did: &str,
        limit: usize,
        before: Option<i64>,
    ) -> Result<Vec<DirectMessage>> {
        self.impl_fetch_direct(sender_did, recipient_did, limit, before)
            .await
    }

    async fn store_group(&self, message: &GroupMessage) -> Result<()> {
        self.impl_store_group(message).await
    }

    async fn fetch_group(
        &self,
        group_id: &str,
        limit: usize,
        before: Option<i64>,
    ) -> Result<Vec<GroupMessage>> {
        self.impl_fetch_group(group_id, limit, before).await
    }

    async fn fetch_group_latest(&self, group_id: &str) -> Result<Option<GroupMessage>> {
        self.impl_fetch_group_latest(group_id).await
    }

    async fn fetch_group_since(
        &self,
        group_id: &str,
        since_timestamp: i64,
        limit: usize,
    ) -> Result<Vec<GroupMessage>> {
        self.impl_fetch_group_since(group_id, since_timestamp, limit)
            .await
    }

    async fn latest_group_timestamp(&self, group_id: &str) -> Result<Option<i64>> {
        self.impl_latest_group_timestamp(group_id).await
    }

    async fn has_group_message(&self, group_id: &str, message_id: &str) -> bool {
        self.impl_has_group_message(group_id, message_id).await
    }

    async fn store_offline(&self, envelope: &OfflineMessageEnvelope) -> Result<()> {
        self.impl_store_offline(envelope).await
    }

    async fn fetch_offline(
        &self,
        mailbox_token: &[u8],
        since: Option<i64>,
        limit: usize,
    ) -> Result<Vec<OfflineMessageEnvelope>> {
        self.impl_fetch_offline(mailbox_token, since, limit).await
    }

    async fn count_offline_for_mailbox(&self, mailbox_token: &[u8]) -> Result<usize> {
        self.impl_count_offline_for_mailbox(mailbox_token).await
    }

    async fn delete_offline(&self, message_id: &str) -> Result<()> {
        self.impl_delete_offline(message_id).await
    }

    async fn cleanup_expired(&self) -> Result<usize> {
        self.impl_cleanup_expired().await
    }

    async fn store_receipt(&self, receipt: &ReadReceipt) -> Result<()> {
        self.impl_store_receipt(receipt).await
    }

    async fn fetch_receipts(&self, message_id: &str) -> Result<Vec<ReadReceipt>> {
        self.impl_fetch_receipts(message_id).await
    }

    async fn fetch_receipt_status(
        &self,
        message_id: &str,
        reader_did: &str,
    ) -> Result<Option<ReadReceipt>> {
        self.impl_fetch_receipt_status(message_id, reader_did).await
    }

    async fn store_group_receipt(&self, receipt: &GroupReadReceipt) -> Result<()> {
        self.impl_store_group_receipt(receipt).await
    }

    async fn fetch_group_receipts(
        &self,
        group_id: &str,
        message_id: &str,
    ) -> Result<Vec<GroupReadReceipt>> {
        self.impl_fetch_group_receipts(group_id, message_id).await
    }

    async fn fetch_group_receipt_status(
        &self,
        group_id: &str,
        message_id: &str,
        reader_did: &str,
    ) -> Result<Option<GroupReadReceipt>> {
        self.impl_fetch_group_receipt_status(group_id, message_id, reader_did)
            .await
    }

    async fn list_direct_conversations(
        &self,
        local_did: &str,
    ) -> Result<Vec<(String, i64, Option<i64>)>> {
        self.impl_list_direct_conversations(local_did).await
    }

    async fn store_last_read_at(
        &self,
        our_did: &str,
        peer_did: &str,
        timestamp: i64,
    ) -> Result<()> {
        self.impl_store_last_read_at(our_did, peer_did, timestamp)
            .await
    }

    async fn fetch_last_read_at(&self, our_did: &str, peer_did: &str) -> Result<Option<i64>> {
        self.impl_fetch_last_read_at(our_did, peer_did).await
    }

    async fn store_group_last_read_at(
        &self,
        our_did: &str,
        group_id: &str,
        timestamp: i64,
    ) -> Result<()> {
        self.impl_store_group_last_read_at(our_did, group_id, timestamp)
            .await
    }

    async fn fetch_group_last_read_at(&self, our_did: &str, group_id: &str) -> Result<Option<i64>> {
        self.impl_fetch_group_last_read_at(our_did, group_id).await
    }

    async fn delete_last_read_at(&self, our_did: &str, peer_did: &str) -> Result<()> {
        self.impl_delete_last_read_at(our_did, peer_did).await
    }

    async fn delete_group_last_read_at(&self, our_did: &str, group_id: &str) -> Result<()> {
        self.impl_delete_group_last_read_at(our_did, group_id).await
    }

    async fn delete_direct_conversation(&self, did1: &str, did2: &str) -> Result<()> {
        self.impl_delete_direct_conversation(did1, did2).await
    }

    async fn delete_direct_by_id(
        &self,
        sender_did: &str,
        recipient_did: &str,
        timestamp: i64,
        message_id: &str,
    ) -> Result<()> {
        self.impl_delete_direct_by_id(sender_did, recipient_did, timestamp, message_id)
            .await
    }

    async fn store_plaintext(&self, message_id: &str, encrypted: &[u8]) -> Result<()> {
        self.impl_store_plaintext(message_id, encrypted).await
    }

    async fn fetch_plaintext(&self, message_id: &str) -> Result<Option<Vec<u8>>> {
        self.impl_fetch_plaintext(message_id).await
    }

    async fn store_session_pickle(&self, peer_did: &str, pickle_json: &str) -> Result<()> {
        self.impl_store_session_pickle(peer_did, pickle_json).await
    }

    async fn fetch_session_pickle(&self, peer_did: &str) -> Result<Option<String>> {
        self.impl_fetch_session_pickle(peer_did).await
    }

    async fn load_all_session_pickles(&self) -> Result<Vec<(String, String)>> {
        self.impl_load_all_session_pickles().await
    }

    async fn store_pending_message(&self, peer_did: &str, message: &DirectMessage) -> Result<()> {
        self.impl_store_pending_message(peer_did, message).await
    }

    async fn fetch_pending_messages(&self, peer_did: &str) -> Result<Vec<DirectMessage>> {
        self.impl_fetch_pending_messages(peer_did).await
    }

    async fn delete_pending_message(&self, message_id: &str) -> Result<()> {
        self.impl_delete_pending_message(message_id).await
    }

    async fn is_message_pending(&self, message_id: &str) -> Result<bool> {
        self.impl_is_message_pending(message_id).await
    }

    async fn list_peers_with_pending_messages(&self) -> Result<Vec<String>> {
        self.impl_list_peers_with_pending_messages().await
    }

    async fn store_group_metadata(&self, group: &Group) -> Result<()> {
        self.impl_store_group_metadata(group).await
    }

    async fn fetch_group_metadata(&self, group_id: &str) -> Result<Option<Group>> {
        self.impl_fetch_group_metadata(group_id).await
    }

    async fn fetch_all_group_metadata(&self) -> Result<Vec<Group>> {
        self.impl_fetch_all_group_metadata().await
    }

    async fn delete_group_messages(&self, group_id: &str) -> Result<()> {
        self.impl_delete_group_messages(group_id).await
    }

    async fn delete_group_metadata(&self, group_id: &str) -> Result<()> {
        self.impl_delete_group_metadata(group_id).await
    }

    async fn store_mls_state(&self, local_did: &str, state: &[u8]) -> Result<()> {
        self.impl_store_mls_state(local_did, state).await
    }

    async fn fetch_mls_state(&self, local_did: &str) -> Result<Option<Vec<u8>>> {
        self.impl_fetch_mls_state(local_did).await
    }

    async fn store_peer_name(&self, did: &str, username: &str, discriminator: u32) -> Result<()> {
        self.impl_store_peer_name(did, username, discriminator)
            .await
    }

    async fn load_all_peer_names(&self) -> Result<Vec<(String, String, u32)>> {
        self.impl_load_all_peer_names().await
    }

    async fn store_pending_receipt(&self, target_did: &str, receipt: &ReadReceipt) -> Result<()> {
        self.impl_store_pending_receipt(target_did, receipt).await
    }

    async fn drain_pending_receipts(&self, target_did: &str) -> Result<Vec<ReadReceipt>> {
        self.impl_drain_pending_receipts(target_did).await
    }

    async fn store_pending_invitation(&self, invitation: &GroupInvitation) -> Result<()> {
        self.impl_store_pending_invitation(invitation).await
    }

    async fn fetch_pending_invitations(&self) -> Result<Vec<GroupInvitation>> {
        self.impl_fetch_pending_invitations().await
    }

    async fn fetch_pending_invitation(&self, group_id: &str) -> Result<Option<GroupInvitation>> {
        self.impl_fetch_pending_invitation(group_id).await
    }

    async fn delete_pending_invitation(&self, group_id: &str) -> Result<()> {
        self.impl_delete_pending_invitation(group_id).await
    }

    async fn store_outbound_invite(
        &self,
        group_id: &str,
        invitee_did: &str,
        invitation: &GroupInvitation,
        created_at_ms: i64,
    ) -> Result<()> {
        self.impl_store_outbound_invite(group_id, invitee_did, invitation, created_at_ms)
            .await
    }

    async fn fetch_outbound_invite(
        &self,
        group_id: &str,
        invitee_did: &str,
    ) -> Result<Option<(GroupInvitation, i64)>> {
        self.impl_fetch_outbound_invite(group_id, invitee_did).await
    }

    async fn delete_outbound_invite(&self, group_id: &str, invitee_did: &str) -> Result<()> {
        self.impl_delete_outbound_invite(group_id, invitee_did)
            .await
    }

    async fn fetch_outbound_invites_for_group(
        &self,
        group_id: &str,
    ) -> Result<Vec<(String, GroupInvitation, i64)>> {
        self.impl_fetch_outbound_invites_for_group(group_id).await
    }

    async fn fetch_expired_outbound_invites(
        &self,
        timeout_ms: i64,
        now_ms: i64,
    ) -> Result<Vec<(String, String, GroupInvitation)>> {
        self.impl_fetch_expired_outbound_invites(timeout_ms, now_ms)
            .await
    }

    async fn delete_all_outbound_invites_for_group(&self, group_id: &str) -> Result<()> {
        self.impl_delete_all_outbound_invites_for_group(group_id)
            .await
    }

    async fn update_member_role(
        &self,
        group_id: &str,
        member_did: &str,
        new_role: i32,
    ) -> Result<bool> {
        self.impl_update_member_role(group_id, member_did, new_role)
            .await
    }
}
