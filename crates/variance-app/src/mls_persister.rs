//! Debounced MLS state persistence.
//!
//! MLS operations (encrypt, decrypt, commit, etc.) mutate the provider's
//! in-memory state and require serialization to sled for crash recovery.
//! Without debouncing, every single message send/receive triggers a full
//! snapshot write (potentially 1+ MB for active groups).
//!
//! `MlsPersister` coalesces rapid-fire mutations into a single write by
//! waiting for a configurable debounce window after the last `schedule()`
//! call before actually flushing to disk.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Notify;
use tracing::warn;
use variance_messaging::mls::MlsGroupHandler;
use variance_messaging::storage::{LocalMessageStorage, MessageStorage};

/// Default debounce window: 500ms after the last mutation before persisting.
const DEFAULT_DEBOUNCE_MS: u64 = 500;

/// Debounced MLS state persister.
///
/// Call `schedule()` after every MLS-mutating operation. The actual write
/// happens after no new `schedule()` calls arrive for the debounce window.
///
/// Call `flush()` for an immediate synchronous persist (e.g. on shutdown).
#[derive(Clone)]
pub struct MlsPersister {
    notify: Arc<Notify>,
    mls_groups: Arc<MlsGroupHandler>,
    storage: Arc<LocalMessageStorage>,
    local_did: String,
    running: Arc<AtomicBool>,
}

impl MlsPersister {
    /// Create a new persister and spawn the background debounce task.
    pub fn new(
        mls_groups: Arc<MlsGroupHandler>,
        storage: Arc<LocalMessageStorage>,
        local_did: String,
    ) -> Self {
        Self::with_debounce(mls_groups, storage, local_did, DEFAULT_DEBOUNCE_MS)
    }

    /// Create with a custom debounce duration (useful for testing).
    pub fn with_debounce(
        mls_groups: Arc<MlsGroupHandler>,
        storage: Arc<LocalMessageStorage>,
        local_did: String,
        debounce_ms: u64,
    ) -> Self {
        let notify = Arc::new(Notify::new());
        let running = Arc::new(AtomicBool::new(true));

        let persister = Self {
            notify: notify.clone(),
            mls_groups: mls_groups.clone(),
            storage: storage.clone(),
            local_did: local_did.clone(),
            running: running.clone(),
        };

        // Spawn the background debounce loop.
        let debounce = Duration::from_millis(debounce_ms);
        tokio::spawn(async move {
            while running.load(Ordering::Acquire) {
                // Wait for at least one schedule() call.
                notify.notified().await;

                // Even if stop() was called, complete the debounce cycle so any
                // pending scheduled persist isn't silently dropped on shutdown.
                while let Ok(()) = tokio::time::timeout(debounce, notify.notified()).await {
                    // Another schedule() arrived, reset the timer.
                }

                // Perform the actual persist.
                do_persist(&mls_groups, &storage, &local_did).await;
            }
        });

        persister
    }

    /// Schedule an MLS state persist. Returns immediately; the actual write
    /// happens after the debounce window elapses.
    pub fn schedule(&self) {
        self.notify.notify_one();
    }

    /// Immediately persist MLS state, bypassing the debounce. Use on shutdown.
    pub async fn flush(&self) {
        do_persist(&self.mls_groups, &self.storage, &self.local_did).await;
    }

    /// Stop the background task. Call before dropping if you need clean shutdown.
    pub fn stop(&self) {
        self.running.store(false, Ordering::Release);
        // notify_waiters() wakes the task even when a schedule() permit is already
        // stored in the Notify — notify_one() would be a no-op in that case.
        self.notify.notify_waiters();
    }
}

async fn do_persist(mls_groups: &MlsGroupHandler, storage: &LocalMessageStorage, local_did: &str) {
    match mls_groups.export_state() {
        Ok(bytes) => {
            if let Err(e) = storage.store_mls_state(local_did, &bytes).await {
                warn!("MlsPersister: Failed to write MLS state to storage: {}", e);
            }
        }
        Err(e) => warn!("MlsPersister: Failed to export MLS state: {}", e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use variance_messaging::storage::MessageStorage;

    #[tokio::test]
    async fn test_debounced_persist() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let storage = Arc::new(LocalMessageStorage::new(db_path.to_str().unwrap()).unwrap());
        let signing_key = ed25519_dalek::SigningKey::generate(&mut rand_core::OsRng);
        let mls_groups =
            Arc::new(MlsGroupHandler::new("did:variance:test".to_string(), &signing_key).unwrap());

        // Use a short debounce for testing.
        let persister = MlsPersister::with_debounce(
            mls_groups.clone(),
            storage.clone(),
            "did:variance:test".to_string(),
            50,
        );

        // Schedule multiple persists rapidly.
        for _ in 0..10 {
            persister.schedule();
            tokio::time::sleep(Duration::from_millis(5)).await;
        }

        // Wait for debounce to complete.
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Verify state was persisted.
        let state = storage.fetch_mls_state("did:variance:test").await.unwrap();
        assert!(
            state.is_some(),
            "MLS state should be persisted after debounce"
        );

        persister.stop();
    }

    #[tokio::test]
    async fn test_flush_bypasses_debounce() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let storage = Arc::new(LocalMessageStorage::new(db_path.to_str().unwrap()).unwrap());
        let signing_key = ed25519_dalek::SigningKey::generate(&mut rand_core::OsRng);
        let mls_groups =
            Arc::new(MlsGroupHandler::new("did:variance:test".to_string(), &signing_key).unwrap());

        // Use a very long debounce — flush should bypass it.
        let persister = MlsPersister::with_debounce(
            mls_groups.clone(),
            storage.clone(),
            "did:variance:test".to_string(),
            60_000,
        );

        persister.flush().await;

        let state = storage.fetch_mls_state("did:variance:test").await.unwrap();
        assert!(state.is_some(), "flush() should persist immediately");

        persister.stop();
    }
}
