use prost::Message;
use tracing::warn;
use variance_proto::messaging_proto::{GroupReadReceipt, ReadReceipt};

use crate::error::*;

use super::LocalMessageStorage;

impl LocalMessageStorage {
    pub(crate) async fn impl_store_receipt(&self, receipt: &ReadReceipt) -> Result<()> {
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

    pub(crate) async fn impl_fetch_receipts(&self, message_id: &str) -> Result<Vec<ReadReceipt>> {
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

    pub(crate) async fn impl_fetch_receipt_status(
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

    pub(crate) async fn impl_store_last_read_at(
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

    pub(crate) async fn impl_fetch_last_read_at(
        &self,
        our_did: &str,
        peer_did: &str,
    ) -> Result<Option<i64>> {
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

    pub(crate) async fn impl_store_group_last_read_at(
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

    pub(crate) async fn impl_fetch_group_last_read_at(
        &self,
        our_did: &str,
        group_id: &str,
    ) -> Result<Option<i64>> {
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

    pub(crate) async fn impl_delete_last_read_at(
        &self,
        our_did: &str,
        peer_did: &str,
    ) -> Result<()> {
        let tree = self.last_read_at_tree()?;
        let key = format!("{}::{}", our_did, peer_did);
        tree.remove(key.as_bytes())
            .map_err(|e| Error::Storage { source: e })?;
        Ok(())
    }

    pub(crate) async fn impl_delete_group_last_read_at(
        &self,
        our_did: &str,
        group_id: &str,
    ) -> Result<()> {
        let tree = self.last_read_at_tree()?;
        let key = format!("{}::group::{}", our_did, group_id);
        tree.remove(key.as_bytes())
            .map_err(|e| Error::Storage { source: e })?;
        Ok(())
    }

    // ===== Group read receipts =====

    pub(crate) async fn impl_store_group_receipt(&self, receipt: &GroupReadReceipt) -> Result<()> {
        let tree = self.group_receipts_tree()?;

        // Key format: {group_id}:{message_id}:{reader_did}:{timestamp:020}
        let key = format!(
            "{}:{}:{}:{:020}",
            receipt.group_id, receipt.message_id, receipt.reader_did, receipt.timestamp
        );

        let value = receipt.encode_to_vec();
        tree.insert(key.as_bytes(), value)
            .map_err(|e| Error::Storage { source: e })?;

        Ok(())
    }

    pub(crate) async fn impl_fetch_group_receipts(
        &self,
        group_id: &str,
        message_id: &str,
    ) -> Result<Vec<GroupReadReceipt>> {
        let tree = self.group_receipts_tree()?;
        let prefix = format!("{group_id}:{message_id}:");

        let mut receipts = Vec::new();
        for entry in tree.scan_prefix(prefix.as_bytes()) {
            let (_key, value) = entry.map_err(|e| Error::Storage { source: e })?;
            let receipt = GroupReadReceipt::decode(value.as_ref())
                .map_err(|e| Error::Protocol { source: e })?;
            receipts.push(receipt);
        }

        Ok(receipts)
    }

    pub(crate) async fn impl_fetch_group_receipt_status(
        &self,
        group_id: &str,
        message_id: &str,
        reader_did: &str,
    ) -> Result<Option<GroupReadReceipt>> {
        let tree = self.group_receipts_tree()?;
        let prefix = format!("{group_id}:{message_id}:{reader_did}:");

        // Get the latest receipt (highest timestamp) for this triple.
        let mut latest: Option<GroupReadReceipt> = None;

        for entry in tree.scan_prefix(prefix.as_bytes()) {
            let (_key, value) = entry.map_err(|e| Error::Storage { source: e })?;
            let receipt = GroupReadReceipt::decode(value.as_ref())
                .map_err(|e| Error::Protocol { source: e })?;

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

    // ===== Pending receipts (direct messages) =====

    pub(crate) async fn impl_store_pending_receipt(
        &self,
        target_did: &str,
        receipt: &ReadReceipt,
    ) -> Result<()> {
        let tree = self.pending_receipts_tree()?;
        let key = format!("{}:{}", target_did, receipt.message_id);
        tree.insert(key.as_bytes(), receipt.encode_to_vec())
            .map_err(|e| Error::Storage { source: e })?;
        Ok(())
    }

    pub(crate) async fn impl_drain_pending_receipts(
        &self,
        target_did: &str,
    ) -> Result<Vec<ReadReceipt>> {
        let tree = self.pending_receipts_tree()?;
        let prefix = format!("{}:", target_did);
        let mut receipts = Vec::new();
        let entries: Vec<_> = tree
            .scan_prefix(prefix.as_bytes())
            .filter_map(|r| r.ok())
            .collect();
        for (k, v) in &entries {
            match ReadReceipt::decode(v.as_ref()) {
                Ok(receipt) => receipts.push(receipt),
                Err(e) => warn!("Failed to decode pending receipt {:?}: {}", k, e),
            }
            tree.remove(k).map_err(|e| Error::Storage { source: e })?;
        }
        Ok(receipts)
    }
}

#[cfg(test)]
mod tests {
    use crate::storage::{LocalMessageStorage, MessageStorage};
    use tempfile::tempdir;
    use variance_proto::messaging_proto::{GroupReadReceipt, ReceiptStatus};

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
    async fn test_store_and_fetch_group_receipts() {
        let dir = tempdir().unwrap();
        let storage = LocalMessageStorage::new(dir.path()).unwrap();

        let receipt = GroupReadReceipt {
            message_id: "msg-001".to_string(),
            group_id: "group-abc".to_string(),
            reader_did: "did:variance:bob".to_string(),
            status: ReceiptStatus::Delivered as i32,
            timestamp: 1000,
            signature: vec![1, 2, 3],
        };
        storage.store_group_receipt(&receipt).await.unwrap();

        let receipt2 = GroupReadReceipt {
            message_id: "msg-001".to_string(),
            group_id: "group-abc".to_string(),
            reader_did: "did:variance:charlie".to_string(),
            status: ReceiptStatus::Delivered as i32,
            timestamp: 1001,
            signature: vec![4, 5, 6],
        };
        storage.store_group_receipt(&receipt2).await.unwrap();

        let receipts = storage
            .fetch_group_receipts("group-abc", "msg-001")
            .await
            .unwrap();
        assert_eq!(receipts.len(), 2);
    }

    #[tokio::test]
    async fn test_group_receipt_status_returns_latest() {
        let dir = tempdir().unwrap();
        let storage = LocalMessageStorage::new(dir.path()).unwrap();

        // Bob sends DELIVERED then READ
        let delivered = GroupReadReceipt {
            message_id: "msg-002".to_string(),
            group_id: "group-abc".to_string(),
            reader_did: "did:variance:bob".to_string(),
            status: ReceiptStatus::Delivered as i32,
            timestamp: 1000,
            signature: vec![1],
        };
        storage.store_group_receipt(&delivered).await.unwrap();

        let read = GroupReadReceipt {
            message_id: "msg-002".to_string(),
            group_id: "group-abc".to_string(),
            reader_did: "did:variance:bob".to_string(),
            status: ReceiptStatus::Read as i32,
            timestamp: 2000,
            signature: vec![2],
        };
        storage.store_group_receipt(&read).await.unwrap();

        let latest = storage
            .fetch_group_receipt_status("group-abc", "msg-002", "did:variance:bob")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(latest.status, ReceiptStatus::Read as i32);
        assert_eq!(latest.timestamp, 2000);
    }

    #[tokio::test]
    async fn test_group_receipts_isolate_by_group() {
        let dir = tempdir().unwrap();
        let storage = LocalMessageStorage::new(dir.path()).unwrap();

        let r1 = GroupReadReceipt {
            message_id: "msg-001".to_string(),
            group_id: "group-a".to_string(),
            reader_did: "did:variance:bob".to_string(),
            status: ReceiptStatus::Delivered as i32,
            timestamp: 1000,
            signature: vec![],
        };
        let r2 = GroupReadReceipt {
            message_id: "msg-001".to_string(),
            group_id: "group-b".to_string(),
            reader_did: "did:variance:bob".to_string(),
            status: ReceiptStatus::Delivered as i32,
            timestamp: 1000,
            signature: vec![],
        };
        storage.store_group_receipt(&r1).await.unwrap();
        storage.store_group_receipt(&r2).await.unwrap();

        let receipts_a = storage
            .fetch_group_receipts("group-a", "msg-001")
            .await
            .unwrap();
        assert_eq!(receipts_a.len(), 1);
        assert_eq!(receipts_a[0].group_id, "group-a");

        let receipts_b = storage
            .fetch_group_receipts("group-b", "msg-001")
            .await
            .unwrap();
        assert_eq!(receipts_b.len(), 1);
        assert_eq!(receipts_b[0].group_id, "group-b");
    }
}
