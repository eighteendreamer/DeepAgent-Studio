//! Snapshot version tracking and stale detection.
//!
//! Each device maintains a monotonically increasing snapshot version. When a
//! new UI snapshot is captured, the previous snapshot for that device is
//! invalidated. Input operations that reference a stale snapshot are rejected
//! with `MobileError::StaleUiNode`.

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Mutex, OwnedMutexGuard};

/// Tracks active UI snapshots per device and detects stale references.
pub struct SnapshotStore {
    inner: Arc<Mutex<StoreInner>>,
    operation_locks: Arc<Mutex<HashMap<String, Arc<Mutex<()>>>>>,
}

struct StoreInner {
    /// Current generated version per device.
    versions: HashMap<String, u64>,
    /// The exact snapshot identity published by the backend.
    current_ids: HashMap<String, String>,
    /// Monotonic global counter for generating unique snapshot IDs.
    next_version: u64,
}

impl SnapshotStore {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(StoreInner {
                versions: HashMap::new(),
                current_ids: HashMap::new(),
                next_version: 1,
            })),
            operation_locks: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Serialize a capture/input/clear operation for one device.
    ///
    /// The guard deliberately spans the backend call: validating a snapshot
    /// and then executing the corresponding input must be one operation with
    /// respect to a concurrent capture that invalidates the snapshot.
    pub(crate) async fn acquire_device(&self, device_id: &str) -> OwnedMutexGuard<()> {
        let lock = {
            let mut locks = self.operation_locks.lock().await;
            locks
                .entry(device_id.to_string())
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .clone()
        };
        lock.lock_owned().await
    }

    /// Register a new snapshot for a device. Returns the snapshot ID.
    ///
    /// Any previous snapshot for this device becomes stale.
    pub async fn register(&self, device_id: &str) -> String {
        let mut inner = self.inner.lock().await;
        let version = inner.next_version;
        inner.next_version += 1;
        let snapshot_id = format!("snap-{device_id}-{version}");
        inner.versions.insert(device_id.to_string(), version);
        inner
            .current_ids
            .insert(device_id.to_string(), snapshot_id.clone());
        snapshot_id
    }

    /// Register the exact identity returned by a backend snapshot capture.
    ///
    /// The backend is the authority for the identity carried by `UiSnapshot`;
    /// generating a second ID here would make every returned snapshot stale.
    pub async fn register_id(&self, device_id: &str, snapshot_id: &str) {
        let mut inner = self.inner.lock().await;
        inner
            .current_ids
            .insert(device_id.to_string(), snapshot_id.to_string());
    }

    /// Check whether a snapshot ID is still the current one for its device.
    ///
    /// Returns `Ok(())` if the snapshot is current, or `Err(StaleUiNode)` if
    /// a newer snapshot has been taken for the same device.
    pub async fn validate(
        &self,
        device_id: &str,
        snapshot_id: &str,
        node_id: &str,
    ) -> Result<(), deepagent_mobile_core::MobileError> {
        let inner = self.inner.lock().await;

        let is_current = inner
            .current_ids
            .get(device_id)
            .is_some_and(|current| current == snapshot_id);

        if is_current {
            Ok(())
        } else {
            Err(deepagent_mobile_core::MobileError::StaleUiNode {
                snapshot_id: snapshot_id.to_string(),
                node_id: node_id.to_string(),
            })
        }
    }

    /// Remove all snapshot tracking for a device (e.g., on disconnect).
    pub async fn clear(&self, device_id: &str) {
        let _operation = self.acquire_device(device_id).await;
        let mut inner = self.inner.lock().await;
        inner.versions.remove(device_id);
        inner.current_ids.remove(device_id);
    }

    /// Get the current snapshot ID for a device, if any.
    pub async fn current(&self, device_id: &str) -> Option<String> {
        let inner = self.inner.lock().await;
        inner.current_ids.get(device_id).cloned()
    }
}

impl Default for SnapshotStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn register_returns_unique_ids() {
        let store = SnapshotStore::new();
        let s1 = store.register("dev-1").await;
        let s2 = store.register("dev-1").await;
        assert_ne!(s1, s2);
    }

    #[tokio::test]
    async fn validate_current_snapshot_succeeds() {
        let store = SnapshotStore::new();
        let snap = store.register("dev-1").await;
        store.validate("dev-1", &snap, "node-1").await.unwrap();
    }

    #[tokio::test]
    async fn validate_old_snapshot_returns_stale() {
        let store = SnapshotStore::new();
        let old_snap = store.register("dev-1").await;
        let _new_snap = store.register("dev-1").await;

        let err = store
            .validate("dev-1", &old_snap, "node-1")
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            deepagent_mobile_core::MobileError::StaleUiNode { .. }
        ));
    }

    #[tokio::test]
    async fn different_devices_independent() {
        let store = SnapshotStore::new();
        let s1 = store.register("dev-1").await;
        let _s2 = store.register("dev-2").await;

        // dev-1 snapshot should still be valid
        store.validate("dev-1", &s1, "node-1").await.unwrap();
    }

    #[tokio::test]
    async fn clear_removes_tracking() {
        let store = SnapshotStore::new();
        let snap = store.register("dev-1").await;
        store.clear("dev-1").await;

        // After clear, the snapshot should be stale (no current version)
        let err = store.validate("dev-1", &snap, "node-1").await.unwrap_err();
        assert!(matches!(
            err,
            deepagent_mobile_core::MobileError::StaleUiNode { .. }
        ));
    }

    #[tokio::test]
    async fn current_returns_latest() {
        let store = SnapshotStore::new();
        let _ = store.register("dev-1").await;
        let s2 = store.register("dev-1").await;
        assert_eq!(store.current("dev-1").await, Some(s2));
    }

    #[tokio::test]
    async fn register_id_tracks_backend_identity() {
        let store = SnapshotStore::new();
        store.register_id("dev-1", "backend-snapshot").await;
        assert_eq!(
            store.current("dev-1").await.as_deref(),
            Some("backend-snapshot")
        );
        store
            .validate("dev-1", "backend-snapshot", "input")
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn current_returns_none_for_unknown_device() {
        let store = SnapshotStore::new();
        assert_eq!(store.current("dev-unknown").await, None);
    }
}
