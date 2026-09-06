use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use deepagent_mobile_core::{
    ArtifactRef, BackendStatus, MobileDevice, MobileError, MobilePlatform, MobileResult,
};
use deepagent_mobile_protocol::{
    AppTarget, ArtifactKind, AvdInfo, InputRequest, InputResult, InstallRequest, LaunchRequest,
    LogPage, LogRequest, NetworkRecord, StartEmulatorRequest, StopEmulatorRequest, UiSnapshot,
};
use tokio::sync::RwLock;

use crate::artifact_store::ArtifactStore;
use crate::backend::MobileBackend;
use crate::device_registry::DeviceRegistry;
use crate::operation::OperationContext;
use crate::remote_mac_manager::RemoteMacManager;
use crate::snapshot_store::SnapshotStore;
use crate::RegisterArtifactRequest;

/// High-level facade that ties together all mobile subsystems.
///
/// `MobileService` is the single entry point for Tauri commands, agent tools,
/// and CLI operations. It:
/// - Routes operations to the correct platform backend
/// - Manages device state through `DeviceRegistry`
/// - Tracks artifacts through `ArtifactStore`
/// - Validates snapshots through `SnapshotStore`
/// - Orchestrates remote Mac connections through `RemoteMacManager`
pub struct MobileService {
    inner: Arc<ServiceInner>,
}

struct ServiceInner {
    registry: DeviceRegistry,
    artifact_store: ArtifactStore,
    snapshot_store: SnapshotStore,
    remote_manager: RwLock<Option<RemoteMacManager>>,
    backends: RwLock<HashMap<MobilePlatform, Arc<dyn MobileBackend>>>,
}

impl MobileService {
    /// Create a new MobileService with the given subsystems.
    pub fn new(
        registry: DeviceRegistry,
        artifact_store: ArtifactStore,
        snapshot_store: SnapshotStore,
    ) -> Self {
        Self {
            inner: Arc::new(ServiceInner {
                registry,
                artifact_store,
                snapshot_store,
                remote_manager: RwLock::new(None),
                backends: RwLock::new(HashMap::new()),
            }),
        }
    }

    /// Attach a RemoteMacManager for remote device support.
    pub async fn set_remote_manager(&self, manager: RemoteMacManager) {
        *self.inner.remote_manager.write().await = Some(manager);
    }

    /// Register a platform backend.
    pub async fn register_backend(
        &self,
        platform: MobilePlatform,
        backend: Arc<dyn MobileBackend>,
    ) {
        self.inner.backends.write().await.insert(platform, backend);
    }

    /// Get a shared reference to the backend for a given platform.
    pub async fn backend_arc(
        &self,
        platform: MobilePlatform,
    ) -> MobileResult<Arc<dyn MobileBackend>> {
        self.inner
            .backends
            .read()
            .await
            .get(&platform)
            .cloned()
            .ok_or_else(|| MobileError::BackendUnavailable {
                reason: format!("{:?} backend not registered", platform),
            })
    }

    /// Probe all registered backends and return their status.
    pub async fn probe_all(&self) -> Vec<BackendStatus> {
        let backends = self.inner.backends.read().await;
        let mut results = Vec::new();
        for backend in backends.values() {
            match backend.probe().await {
                Ok(status) => results.push(status),
                Err(e) => {
                    tracing::warn!(error = %e, "backend probe failed");
                }
            }
        }
        results
    }

    /// List all known devices (local + remote).
    pub async fn list_devices(&self) -> Vec<MobileDevice> {
        self.inner.registry.list().await
    }

    /// Get a single device by ID.
    pub async fn device_info(&self, device_id: &str) -> MobileResult<MobileDevice> {
        self.inner.registry.get(device_id).await
    }

    /// Capture a screenshot and register it as an artifact.
    pub async fn screenshot(
        &self,
        device_id: &str,
        ctx: &OperationContext,
    ) -> MobileResult<ArtifactRef> {
        let device = self.inner.registry.get(device_id).await?;
        let backend = self.backend_arc(device.platform).await?;
        let artifact_ref = backend.screenshot(device_id, ctx).await?;

        let req = RegisterArtifactRequest {
            device_id,
            kind: ArtifactKind::Screenshot,
            mime: "image/png",
            size_bytes: 0,
            sha256: artifact_ref.sha256.as_deref(),
            storage_path: &artifact_ref.storage_path,
            ttl_ms: None,
        };
        let _ = self.inner.artifact_store.register(req);

        Ok(artifact_ref)
    }

    /// Capture a UI snapshot and track it in the snapshot store.
    pub async fn ui_snapshot(
        &self,
        device_id: &str,
        ctx: &OperationContext,
    ) -> MobileResult<UiSnapshot> {
        let device = self.inner.registry.get(device_id).await?;
        let backend = self.backend_arc(device.platform).await?;
        let snapshot = backend.ui_snapshot(device_id, ctx).await?;

        let _ = self.inner.snapshot_store.register(device_id).await;

        Ok(snapshot)
    }

    /// Install an application on a device.
    pub async fn install(
        &self,
        request: &InstallRequest,
        ctx: &OperationContext,
    ) -> MobileResult<()> {
        let device = self.inner.registry.get(&request.device_id).await?;
        let backend = self.backend_arc(device.platform).await?;
        backend.install(request, ctx).await
    }

    /// Launch an application on a device.
    pub async fn launch(
        &self,
        request: &LaunchRequest,
        ctx: &OperationContext,
    ) -> MobileResult<()> {
        let device = self.inner.registry.get(&request.device_id).await?;
        let backend = self.backend_arc(device.platform).await?;
        backend.launch(request, ctx).await
    }

    /// Terminate a running application.
    pub async fn terminate(&self, target: &AppTarget, ctx: &OperationContext) -> MobileResult<()> {
        let device = self.inner.registry.get(&target.device_id).await?;
        let backend = self.backend_arc(device.platform).await?;
        backend.terminate(target, ctx).await
    }

    /// Perform a structured input action.
    pub async fn input(
        &self,
        request: &InputRequest,
        ctx: &OperationContext,
    ) -> MobileResult<InputResult> {
        let device = self.inner.registry.get(&request.device_id).await?;
        let backend = self.backend_arc(device.platform).await?;
        backend.input(request, ctx).await
    }

    /// Read device logs.
    pub async fn read_logs(
        &self,
        request: &LogRequest,
        ctx: &OperationContext,
    ) -> MobileResult<LogPage> {
        let device = self.inner.registry.get(&request.device_id).await?;
        let backend = self.backend_arc(device.platform).await?;
        backend.read_logs(request, ctx).await
    }

    /// List available Android Virtual Devices.
    pub async fn list_avds(&self, ctx: &OperationContext) -> MobileResult<Vec<AvdInfo>> {
        let backend = self
            .backend_arc(deepagent_mobile_core::MobilePlatform::Android)
            .await?;
        backend.list_avds(ctx).await
    }

    /// Start an Android Emulator.
    pub async fn start_emulator(
        &self,
        request: &StartEmulatorRequest,
        ctx: &OperationContext,
    ) -> MobileResult<String> {
        let backend = self
            .backend_arc(deepagent_mobile_core::MobilePlatform::Android)
            .await?;
        backend.start_emulator(request, ctx).await
    }

    /// Stop an Android Emulator.
    pub async fn stop_emulator(
        &self,
        request: &StopEmulatorRequest,
        ctx: &OperationContext,
    ) -> MobileResult<()> {
        let backend = self
            .backend_arc(deepagent_mobile_core::MobilePlatform::Android)
            .await?;
        backend.stop_emulator(request, ctx).await
    }

    /// Start capturing network traffic for a device.
    pub async fn start_network_capture(
        &self,
        device_id: &str,
        ctx: &OperationContext,
    ) -> MobileResult<()> {
        let device = self.inner.registry.get(device_id).await?;
        let backend = self.backend_arc(device.platform).await?;
        backend.start_network_capture(device_id, ctx).await
    }

    /// Stop capturing network traffic for a device.
    pub async fn stop_network_capture(
        &self,
        device_id: &str,
        ctx: &OperationContext,
    ) -> MobileResult<()> {
        let device = self.inner.registry.get(device_id).await?;
        let backend = self.backend_arc(device.platform).await?;
        backend.stop_network_capture(device_id, ctx).await
    }

    /// Get captured network records for a device.
    pub async fn get_network_records(
        &self,
        device_id: &str,
        ctx: &OperationContext,
    ) -> MobileResult<Vec<NetworkRecord>> {
        let device = self.inner.registry.get(device_id).await?;
        let backend = self.backend_arc(device.platform).await?;
        backend.get_network_records(device_id, ctx).await
    }

    /// Get a reference to the device registry.
    pub fn registry(&self) -> &DeviceRegistry {
        &self.inner.registry
    }

    /// Get a reference to the artifact store.
    pub fn artifact_store(&self) -> &ArtifactStore {
        &self.inner.artifact_store
    }

    /// Get a reference to the snapshot store.
    pub fn snapshot_store(&self) -> &SnapshotStore {
        &self.inner.snapshot_store
    }

    /// Create an OperationContext for a device operation.
    pub fn operation_context(
        &self,
        device_id: &str,
        operation: &str,
        deadline: Duration,
    ) -> OperationContext {
        OperationContext::new(
            format!("op-{operation}-{device_id}"),
            device_id.to_string(),
            deadline,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use deepagent_mobile_core::*;
    use deepagent_mobile_protocol::MobileEvent;

    fn test_registry() -> (
        DeviceRegistry,
        tokio::sync::mpsc::UnboundedReceiver<MobileEvent>,
    ) {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        (DeviceRegistry::new(tx), rx)
    }

    fn test_device(id: &str, platform: MobilePlatform) -> MobileDevice {
        MobileDevice {
            id: id.into(),
            name: "Test".into(),
            platform,
            kind: DeviceKind::Physical,
            connection: DeviceConnection::Usb,
            state: DeviceState::Ready,
            os_version: None,
            capabilities: DeviceCapabilities {
                screenshot: true,
                ui_tree: true,
                input: true,
                logs: true,
                install: true,
                network_inspection: false,
            },
        }
    }

    #[tokio::test]
    async fn service_list_devices_empty() {
        let (registry, _rx) = test_registry();
        let service = MobileService::new(registry, ArtifactStore::new(), SnapshotStore::new());
        assert!(service.list_devices().await.is_empty());
    }

    #[tokio::test]
    async fn service_list_devices_after_upsert() {
        let (registry, _rx) = test_registry();
        let service =
            MobileService::new(registry.clone(), ArtifactStore::new(), SnapshotStore::new());
        registry
            .upsert(test_device("dev-1", MobilePlatform::Android))
            .await;
        let devices = service.list_devices().await;
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].id, "dev-1");
    }

    #[tokio::test]
    async fn service_device_info_found() {
        let (registry, _rx) = test_registry();
        let service =
            MobileService::new(registry.clone(), ArtifactStore::new(), SnapshotStore::new());
        registry
            .upsert(test_device("dev-1", MobilePlatform::Android))
            .await;
        let device = service.device_info("dev-1").await.unwrap();
        assert_eq!(device.platform, MobilePlatform::Android);
    }

    #[tokio::test]
    async fn service_device_info_not_found() {
        let (registry, _rx) = test_registry();
        let service = MobileService::new(registry, ArtifactStore::new(), SnapshotStore::new());
        let result = service.device_info("nonexistent").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn service_backend_arc_not_registered() {
        let (registry, _rx) = test_registry();
        let service =
            MobileService::new(registry.clone(), ArtifactStore::new(), SnapshotStore::new());
        let result = service.backend_arc(MobilePlatform::Android).await;
        assert!(matches!(
            result,
            Err(MobileError::BackendUnavailable { .. })
        ));
    }

    #[tokio::test]
    async fn service_operation_context_creation() {
        let (registry, _rx) = test_registry();
        let service = MobileService::new(registry, ArtifactStore::new(), SnapshotStore::new());
        let ctx = service.operation_context("dev-1", "screenshot", Duration::from_secs(30));
        assert!(!ctx.is_cancelled());
    }

    #[tokio::test]
    async fn service_subsystem_accessors() {
        let (registry, _rx) = test_registry();
        let artifact_store = ArtifactStore::new();
        let snapshot_store = SnapshotStore::new();
        let service = MobileService::new(registry, artifact_store, snapshot_store);
        assert_eq!(service.artifact_store().len(), 0);
    }

    #[tokio::test]
    async fn service_list_avds_no_backend() {
        let (registry, _rx) = test_registry();
        let service = MobileService::new(registry, ArtifactStore::new(), SnapshotStore::new());
        let ctx = OperationContext::new("op".into(), "dev".into(), Duration::from_secs(10));
        let result = service.list_avds(&ctx).await;
        assert!(matches!(
            result,
            Err(MobileError::BackendUnavailable { .. })
        ));
    }
}
