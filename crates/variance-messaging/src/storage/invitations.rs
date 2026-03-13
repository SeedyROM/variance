use prost::Message;
use variance_proto::messaging_proto::GroupInvitation;

use crate::error::*;

use super::LocalMessageStorage;

/// Serde-serializable envelope for outbound invites.
///
/// Wraps the `GroupInvitation` proto with the serialized MLS commit bytes
/// that must be broadcast to existing group members after the invitee accepts.
/// The commit is already captured inside `GroupInvitation.mls_commit`, so
/// this struct exists primarily to track the invite creation timestamp
/// for timeout expiration.
#[derive(serde::Serialize, serde::Deserialize)]
pub(crate) struct OutboundInvite {
    /// Protobuf-encoded `GroupInvitation`.
    pub invitation_bytes: Vec<u8>,
    /// Unix timestamp (ms) when the invite was created — used for 5-minute timeout.
    pub created_at_ms: i64,
}

impl LocalMessageStorage {
    // ===== Tree accessors =====

    /// Pending invitations tree (invitee side).
    ///
    /// Key: `{group_id}` — only one pending invite per group makes sense.
    /// Value: protobuf-encoded `GroupInvitation`.
    pub(crate) fn pending_invitations_tree(&self) -> Result<sled::Tree> {
        self.db
            .open_tree("pending_invitations")
            .map_err(|e| Error::Storage { source: e })
    }

    /// Outbound invites tree (sender/admin side).
    ///
    /// Key: `{group_id}:{invitee_did}` — one outbound invite per (group, invitee) pair.
    /// Value: JSON-encoded `OutboundInvite`.
    pub(crate) fn outbound_invites_tree(&self) -> Result<sled::Tree> {
        self.db
            .open_tree("outbound_invites")
            .map_err(|e| Error::Storage { source: e })
    }

    // ===== Invitee side: pending invitations =====

    /// Store a pending group invitation received from another user.
    pub(crate) async fn impl_store_pending_invitation(
        &self,
        invitation: &GroupInvitation,
    ) -> Result<()> {
        let tree = self.pending_invitations_tree()?;
        let bytes = Message::encode_to_vec(invitation);
        tree.insert(invitation.group_id.as_bytes(), bytes.as_slice())
            .map_err(|e| Error::Storage { source: e })?;
        Ok(())
    }

    /// Fetch all pending invitations (for displaying in the Invitations tab).
    pub(crate) async fn impl_fetch_pending_invitations(&self) -> Result<Vec<GroupInvitation>> {
        let tree = self.pending_invitations_tree()?;
        let mut invitations = Vec::new();
        for entry in tree.iter() {
            let (_, value) = entry.map_err(|e| Error::Storage { source: e })?;
            let invitation = GroupInvitation::decode(value.as_ref())
                .map_err(|e| Error::Protocol { source: e })?;
            invitations.push(invitation);
        }
        Ok(invitations)
    }

    /// Fetch a single pending invitation by group ID.
    pub(crate) async fn impl_fetch_pending_invitation(
        &self,
        group_id: &str,
    ) -> Result<Option<GroupInvitation>> {
        let tree = self.pending_invitations_tree()?;
        tree.get(group_id.as_bytes())
            .map_err(|e| Error::Storage { source: e })?
            .map(|v| GroupInvitation::decode(v.as_ref()))
            .transpose()
            .map_err(|e| Error::Protocol { source: e })
    }

    /// Delete a pending invitation (after accepting or declining).
    pub(crate) async fn impl_delete_pending_invitation(&self, group_id: &str) -> Result<()> {
        let tree = self.pending_invitations_tree()?;
        tree.remove(group_id.as_bytes())
            .map_err(|e| Error::Storage { source: e })?;
        Ok(())
    }

    // ===== Sender side: outbound invites =====

    /// Store an outbound invite (admin side) so we can confirm/cancel later.
    pub(crate) async fn impl_store_outbound_invite(
        &self,
        group_id: &str,
        invitee_did: &str,
        invitation: &GroupInvitation,
        created_at_ms: i64,
    ) -> Result<()> {
        let tree = self.outbound_invites_tree()?;
        let key = format!("{group_id}:{invitee_did}");
        let envelope = OutboundInvite {
            invitation_bytes: Message::encode_to_vec(invitation),
            created_at_ms,
        };
        let json = serde_json::to_vec(&envelope).map_err(|e| Error::Serialization { source: e })?;
        tree.insert(key.as_bytes(), json.as_slice())
            .map_err(|e| Error::Storage { source: e })?;
        Ok(())
    }

    /// Fetch an outbound invite by (group_id, invitee_did).
    pub(crate) async fn impl_fetch_outbound_invite(
        &self,
        group_id: &str,
        invitee_did: &str,
    ) -> Result<Option<(GroupInvitation, i64)>> {
        let tree = self.outbound_invites_tree()?;
        let key = format!("{group_id}:{invitee_did}");
        let Some(value) = tree
            .get(key.as_bytes())
            .map_err(|e| Error::Storage { source: e })?
        else {
            return Ok(None);
        };
        let envelope: OutboundInvite =
            serde_json::from_slice(&value).map_err(|e| Error::Serialization { source: e })?;
        let invitation = GroupInvitation::decode(envelope.invitation_bytes.as_slice())
            .map_err(|e| Error::Protocol { source: e })?;
        Ok(Some((invitation, envelope.created_at_ms)))
    }

    /// Delete an outbound invite (after the invitee accepted/declined or timeout).
    pub(crate) async fn impl_delete_outbound_invite(
        &self,
        group_id: &str,
        invitee_did: &str,
    ) -> Result<()> {
        let tree = self.outbound_invites_tree()?;
        let key = format!("{group_id}:{invitee_did}");
        tree.remove(key.as_bytes())
            .map_err(|e| Error::Storage { source: e })?;
        Ok(())
    }

    /// Delete all outbound invites for a group (used during group leave/kick cleanup).
    pub(crate) async fn impl_delete_all_outbound_invites_for_group(
        &self,
        group_id: &str,
    ) -> Result<()> {
        let tree = self.outbound_invites_tree()?;
        let prefix = format!("{group_id}:");
        let keys: Vec<sled::IVec> = tree
            .scan_prefix(prefix.as_bytes())
            .filter_map(|r| r.ok().map(|(k, _)| k))
            .collect();
        for key in keys {
            tree.remove(&key)
                .map_err(|e| Error::Storage { source: e })?;
        }
        Ok(())
    }

    /// Fetch all outbound invites for a specific group.
    ///
    /// Uses `scan_prefix` on the `{group_id}:` key prefix.
    /// Returns `(invitee_did, GroupInvitation, created_at_ms)` triples.
    pub(crate) async fn impl_fetch_outbound_invites_for_group(
        &self,
        group_id: &str,
    ) -> Result<Vec<(String, GroupInvitation, i64)>> {
        let tree = self.outbound_invites_tree()?;
        let prefix = format!("{group_id}:");
        let mut results = Vec::new();

        for entry in tree.scan_prefix(prefix.as_bytes()) {
            let (key, value) = entry.map_err(|e| Error::Storage { source: e })?;
            let envelope: OutboundInvite = match serde_json::from_slice(&value) {
                Ok(e) => e,
                Err(_) => continue,
            };

            let key_str = String::from_utf8_lossy(&key);
            // Key format: {group_id}:{invitee_did}
            // The prefix already matched group_id, so everything after the first
            // `:did:` boundary is the invitee DID.
            if let Some(split_pos) = key_str.find(":did:") {
                let invitee_did = key_str[split_pos + 1..].to_string();
                let invitation = match GroupInvitation::decode(envelope.invitation_bytes.as_slice())
                {
                    Ok(inv) => inv,
                    Err(_) => continue,
                };
                results.push((invitee_did, invitation, envelope.created_at_ms));
            }
        }

        Ok(results)
    }

    /// Fetch all outbound invites that have expired (created_at_ms + timeout < now).
    ///
    /// Returns `(group_id, invitee_did, GroupInvitation)` triples.
    pub(crate) async fn impl_fetch_expired_outbound_invites(
        &self,
        timeout_ms: i64,
        now_ms: i64,
    ) -> Result<Vec<(String, String, GroupInvitation)>> {
        let tree = self.outbound_invites_tree()?;
        let mut expired = Vec::new();

        for entry in tree.iter() {
            let (key, value) = entry.map_err(|e| Error::Storage { source: e })?;
            let envelope: OutboundInvite = match serde_json::from_slice(&value) {
                Ok(e) => e,
                Err(_) => continue, // skip corrupted entries
            };

            if envelope.created_at_ms + timeout_ms < now_ms {
                let key_str = String::from_utf8_lossy(&key);
                // Key format: {group_id}:{invitee_did}
                // Both group_id and DID may contain colons (e.g. "did:key:..."),
                // so we split on the first colon-separated DID-like segment.
                // Since group IDs don't start with "did:", find the first ":did:" boundary.
                if let Some(split_pos) = key_str.find(":did:") {
                    let group_id = key_str[..split_pos].to_string();
                    let invitee_did = key_str[split_pos + 1..].to_string();

                    let invitation =
                        match GroupInvitation::decode(envelope.invitation_bytes.as_slice()) {
                            Ok(inv) => inv,
                            Err(_) => continue,
                        };
                    expired.push((group_id, invitee_did, invitation));
                }
            }
        }

        Ok(expired)
    }
}

#[cfg(test)]
mod tests {
    use crate::storage::{LocalMessageStorage, MessageStorage};
    use tempfile::tempdir;
    use variance_proto::messaging_proto::GroupInvitation;

    fn test_invitation(group_id: &str, inviter: &str, invitee: &str) -> GroupInvitation {
        GroupInvitation {
            group_id: group_id.to_string(),
            group_name: format!("Test Group {group_id}"),
            inviter_did: inviter.to_string(),
            invitee_did: invitee.to_string(),
            timestamp: chrono::Utc::now().timestamp_millis(),
            mls_welcome: vec![1, 2, 3],
            mls_commit: vec![4, 5, 6],
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn pending_invitation_store_fetch_delete() {
        let dir = tempdir().unwrap();
        let storage = LocalMessageStorage::new(dir.path()).unwrap();

        let inv = test_invitation("group-1", "did:key:alice", "did:key:bob");
        storage.store_pending_invitation(&inv).await.unwrap();

        // Fetch all
        let all = storage.fetch_pending_invitations().await.unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].group_id, "group-1");

        // Fetch by group_id
        let single = storage.fetch_pending_invitation("group-1").await.unwrap();
        assert!(single.is_some());
        assert_eq!(single.unwrap().inviter_did, "did:key:alice");

        // Delete
        storage.delete_pending_invitation("group-1").await.unwrap();
        let after = storage.fetch_pending_invitations().await.unwrap();
        assert!(after.is_empty());
    }

    #[tokio::test]
    async fn pending_invitation_overwrites_same_group() {
        let dir = tempdir().unwrap();
        let storage = LocalMessageStorage::new(dir.path()).unwrap();

        let inv1 = test_invitation("group-x", "did:key:alice", "did:key:bob");
        let mut inv2 = test_invitation("group-x", "did:key:charlie", "did:key:bob");
        inv2.group_name = "Updated Group".to_string();

        storage.store_pending_invitation(&inv1).await.unwrap();
        storage.store_pending_invitation(&inv2).await.unwrap();

        let all = storage.fetch_pending_invitations().await.unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].inviter_did, "did:key:charlie");
    }

    #[tokio::test]
    async fn outbound_invite_store_fetch_delete() {
        let dir = tempdir().unwrap();
        let storage = LocalMessageStorage::new(dir.path()).unwrap();

        let inv = test_invitation("group-2", "did:key:alice", "did:key:bob");
        let now = chrono::Utc::now().timestamp_millis();

        storage
            .store_outbound_invite("group-2", "did:key:bob", &inv, now)
            .await
            .unwrap();

        let fetched = storage
            .fetch_outbound_invite("group-2", "did:key:bob")
            .await
            .unwrap();
        assert!(fetched.is_some());
        let (fetched_inv, created_at) = fetched.unwrap();
        assert_eq!(fetched_inv.group_id, "group-2");
        assert_eq!(created_at, now);

        // Delete
        storage
            .delete_outbound_invite("group-2", "did:key:bob")
            .await
            .unwrap();
        let after = storage
            .fetch_outbound_invite("group-2", "did:key:bob")
            .await
            .unwrap();
        assert!(after.is_none());
    }

    #[tokio::test]
    async fn expired_outbound_invites() {
        let dir = tempdir().unwrap();
        let storage = LocalMessageStorage::new(dir.path()).unwrap();

        let now = chrono::Utc::now().timestamp_millis();
        let timeout_ms = 5 * 60 * 1000; // 5 minutes

        // Old invite: 10 minutes ago (expired)
        let old_inv = test_invitation("group-old", "did:key:alice", "did:key:bob");
        storage
            .store_outbound_invite("group-old", "did:key:bob", &old_inv, now - 10 * 60 * 1000)
            .await
            .unwrap();

        // Fresh invite: 1 minute ago (not expired)
        let fresh_inv = test_invitation("group-fresh", "did:key:alice", "did:key:charlie");
        storage
            .store_outbound_invite(
                "group-fresh",
                "did:key:charlie",
                &fresh_inv,
                now - 60 * 1000,
            )
            .await
            .unwrap();

        let expired = storage
            .fetch_expired_outbound_invites(timeout_ms, now)
            .await
            .unwrap();
        assert_eq!(expired.len(), 1);
        assert_eq!(expired[0].0, "group-old");
        assert_eq!(expired[0].1, "did:key:bob");
    }

    #[tokio::test]
    async fn fetch_nonexistent_returns_none() {
        let dir = tempdir().unwrap();
        let storage = LocalMessageStorage::new(dir.path()).unwrap();

        assert!(storage
            .fetch_pending_invitation("nope")
            .await
            .unwrap()
            .is_none());
        assert!(storage
            .fetch_outbound_invite("nope", "did:key:nope")
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn fetch_outbound_invites_for_group() {
        let dir = tempdir().unwrap();
        let storage = LocalMessageStorage::new(dir.path()).unwrap();

        let now = chrono::Utc::now().timestamp_millis();

        // Two invites in group-a, one in group-b
        let inv1 = test_invitation("group-a", "did:key:alice", "did:key:bob");
        let inv2 = test_invitation("group-a", "did:key:alice", "did:key:charlie");
        let inv3 = test_invitation("group-b", "did:key:alice", "did:key:dave");

        storage
            .store_outbound_invite("group-a", "did:key:bob", &inv1, now)
            .await
            .unwrap();
        storage
            .store_outbound_invite("group-a", "did:key:charlie", &inv2, now + 1000)
            .await
            .unwrap();
        storage
            .store_outbound_invite("group-b", "did:key:dave", &inv3, now + 2000)
            .await
            .unwrap();

        // Fetch only group-a invites
        let results = storage
            .fetch_outbound_invites_for_group("group-a")
            .await
            .unwrap();
        assert_eq!(results.len(), 2);

        let dids: Vec<&str> = results.iter().map(|(did, _, _)| did.as_str()).collect();
        assert!(dids.contains(&"did:key:bob"));
        assert!(dids.contains(&"did:key:charlie"));

        // Verify created_at values
        for (did, _, ts) in &results {
            if did == "did:key:bob" {
                assert_eq!(*ts, now);
            } else {
                assert_eq!(*ts, now + 1000);
            }
        }

        // Fetch group-b — should only have one
        let results_b = storage
            .fetch_outbound_invites_for_group("group-b")
            .await
            .unwrap();
        assert_eq!(results_b.len(), 1);
        assert_eq!(results_b[0].0, "did:key:dave");

        // Fetch nonexistent group — empty
        let results_c = storage
            .fetch_outbound_invites_for_group("group-nope")
            .await
            .unwrap();
        assert!(results_c.is_empty());
    }
}
