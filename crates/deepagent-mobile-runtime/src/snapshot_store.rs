//! Snapshot version tracking and stale detection.
//!
//! Each device maintains a monotonically increasing snapshot version. When a
//! new UI snapshot is captured, the previous snapshot for that device is
//! invalidated. Input operations that reference a stale snapshot are rejected
//! with `MobileError::StaleUiNode`.

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

/// Tracks active UI snapshots per device and detects stale references.
pub struct SnapshotStore {
    inner: Arc<Mutex<StoreInner>>,
}

struct StoreInner {
    /// Current snapshot version per device.
    versions: HashMap<String, u64>,
    /// Monotonic global counter for generating unique snapshot IDs.
    next_version: u64,
}

impl SnapshotStore {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(StoreInner {
                versions: HashMap::new(),
                next_version: 1,
            })),
        }
    }

    /// Register a new snapshot for a device. Returns the snapshot ID.
    ///
    /// Any previous snapshot for this device becomes stale.
    pub async fn register(&self, device_id: &str) -> String {
        let mut inner = self.inner.lock().await;
        let version = inner.next_version;
        inner.next_version += 1;
        inner.versions.insert(device_id.to_string(), version);
        format!("snap-{device_id}-{version}")
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

        let expected = format!(
            "snap-{device_id}-{}",
            inner.versions.get(device_id).unwrap_or(&0)
        );

        if snapshot_id == expected {
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
        let mut inner = self.inner.lock().await;
        inner.versions.remove(device_id);
    }

    /// Get the current snapshot ID for a device, if any.
    pub async fn current(&self, device_id: &str) -> Option<String> {
        let inner = self.inner.lock().await;
        inner
            .versions
            .get(device_id)
            .map(|v| format!("snap-{device_id}-{v}"))
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
    async fn current_returns_none_for_unknown_device() {
        let store = SnapshotStore::new();
        assert_eq!(store.current("dev-unknown").await, None);
    }
}
