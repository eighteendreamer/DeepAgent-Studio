//! Snapshot identities and per-device UI operation ownership.
//!
//! Capture, input and explicit invalidation share one device record and lock.
//! References are opaque and scoped to the observed device connection.

use std::collections::HashMap;
use std::sync::Arc;

use deepagent_mobile_core::MobileError;
use tokio::sync::{Mutex, OwnedMutexGuard};

/// Tracks active UI snapshots per device and detects stale references.
pub struct SnapshotStore {
    devices: Mutex<HashMap<String, Arc<Mutex<DeviceSnapshot>>>>,
}

#[derive(Default)]
pub(crate) struct DeviceSnapshot {
    current: Option<SnapshotReference>,
}

struct SnapshotReference {
    id: String,
    connection: Option<SnapshotConnection>,
}

/// Both routing layers must still identify the source of the captured UI.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct SnapshotConnection {
    pub device: uuid::Uuid,
    pub backend: uuid::Uuid,
}

impl DeviceSnapshot {
    /// Publish a fresh, service-owned identity even if a backend reuses its ID.
    pub(crate) fn publish(&mut self, connection: Option<SnapshotConnection>) -> String {
        let id = format!("snap-{}", uuid::Uuid::new_v4());
        self.current = Some(SnapshotReference {
            id: id.clone(),
            connection,
        });
        id
    }

    pub(crate) fn validate(
        &self,
        snapshot_id: &str,
        node_id: &str,
        connection: Option<SnapshotConnection>,
    ) -> Result<(), MobileError> {
        if self
            .current
            .as_ref()
            .is_some_and(|current| current.id == snapshot_id && current.connection == connection)
        {
            Ok(())
        } else {
            Err(MobileError::StaleUiNode {
                snapshot_id: snapshot_id.to_string(),
                node_id: node_id.to_string(),
            })
        }
    }

    pub(crate) fn invalidate(&mut self) {
        self.current = None;
    }
}

impl SnapshotStore {
    pub fn new() -> Self {
        Self {
            devices: Mutex::new(HashMap::new()),
        }
    }

    /// Hold through capture or input validation + execution. Different devices
    /// remain independent; a cancelled future drops its guard automatically.
    pub(crate) async fn acquire_device(&self, device_id: &str) -> OwnedMutexGuard<DeviceSnapshot> {
        let lock = {
            let mut devices = self.devices.lock().await;
            devices
                .entry(device_id.to_string())
                .or_insert_with(|| Arc::new(Mutex::new(DeviceSnapshot::default())))
                .clone()
        };
        lock.lock_owned().await
    }

    /// Register a fresh opaque identity, invalidating the prior snapshot.
    pub async fn register(&self, device_id: &str) -> String {
        self.acquire_device(device_id).await.publish(None)
    }

    /// Validate the published identity for this device. The service additionally
    /// verifies its captured connection under the same operation guard.
    pub async fn validate(
        &self,
        device_id: &str,
        snapshot_id: &str,
        node_id: &str,
    ) -> Result<(), MobileError> {
        let record = self.acquire_device(device_id).await;
        let connection = record
            .current
            .as_ref()
            .and_then(|current| current.connection);
        record.validate(snapshot_id, node_id, connection)
    }

    /// Explicitly revoke snapshots, serialized with capture and input.
    pub async fn clear(&self, device_id: &str) {
        self.acquire_device(device_id).await.invalidate();
    }

    pub async fn current(&self, device_id: &str) -> Option<String> {
        self.acquire_device(device_id)
            .await
            .current
            .as_ref()
            .map(|current| current.id.clone())
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
