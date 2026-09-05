//! Mobile subsystem integration for the application layer.
//!
//! This module bridges `deepagent-mobile-runtime` to the app-core service
//! layer. It provides DTO-returning operations that Tauri commands and agent
//! tools can consume without touching mobile internals.
//!
//! The chain is:
//! ```text
//! React / CLI / Agent
//!   -> AppMobileService (this module)
//!   -> deepagent_mobile_runtime::MobileService
//!   -> platform backend (ADB / simctl / ...)
//! ```

use std::sync::Arc;
use std::time::Duration;

use deepagent_mobile_core::{BackendStatus, MobileDevice, MobileResult};
use deepagent_mobile_protocol::MobileEvent;
use deepagent_mobile_protocol::{
    AppTarget, AvdInfo, InputRequest, InputResult, InstallRequest, LaunchRequest, LogPage,
    LogRequest, StartEmulatorRequest, StopEmulatorRequest, UiSnapshot,
};
use deepagent_mobile_runtime::{
    ArtifactStore, DeviceRegistry, MobileService, OperationContext, SnapshotStore,
};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

/// DTO for device information, suitable for frontend consumption.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceDto {
    pub id: String,
    pub name: String,
    pub platform: String,
    pub kind: String,
    pub connection: String,
    pub state: String,
    pub os_version: Option<String>,
    pub can_screenshot: bool,
    pub can_ui_tree: bool,
    pub can_input: bool,
    pub can_logs: bool,
    pub can_install: bool,
}

impl From<MobileDevice> for DeviceDto {
    fn from(d: MobileDevice) -> Self {
        Self {
            id: d.id,
            name: d.name,
            platform: format!("{:?}", d.platform),
            kind: format!("{:?}", d.kind),
            connection: format!("{:?}", d.connection),
            state: format!("{:?}", d.state),
            os_version: d.os_version,
            can_screenshot: d.capabilities.screenshot,
            can_ui_tree: d.capabilities.ui_tree,
            can_input: d.capabilities.input,
            can_logs: d.capabilities.logs,
            can_install: d.capabilities.install,
        }
    }
}

/// DTO for backend probe status.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackendStatusDto {
    pub platform: String,
    pub available: bool,
    pub toolchain_version: Option<String>,
    pub tool_paths: Vec<String>,
    pub diagnostics: Vec<String>,
}

impl From<BackendStatus> for BackendStatusDto {
    fn from(s: BackendStatus) -> Self {
        Self {
            platform: format!("{:?}", s.platform),
            available: s.available,
            toolchain_version: s.toolchain_version,
            tool_paths: s.tool_paths.iter().map(|tp| tp.path.clone()).collect(),
            diagnostics: s.diagnostics,
        }
    }
}

/// DTO for UI snapshot summary (not the full tree).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UiSnapshotSummaryDto {
    pub snapshot_id: String,
    pub device_id: String,
    pub node_count: usize,
    pub max_depth: u32,
    pub root_node_id: String,
    pub captured_at_ms: u64,
}

/// DTO for artifact reference (screenshot, recording, etc.).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactRefDto {
    pub artifact_id: String,
    pub mime: String,
    pub size_bytes: u64,
    pub sha256: Option<String>,
    pub storage_path: String,
}

impl From<deepagent_mobile_core::ArtifactRef> for ArtifactRefDto {
    fn from(a: deepagent_mobile_core::ArtifactRef) -> Self {
        Self {
            artifact_id: a.artifact_id,
            mime: a.mime,
            size_bytes: a.size_bytes,
            sha256: a.sha256,
            storage_path: a.storage_path,
        }
    }
}

/// Application-layer mobile service.
///
/// Wraps `deepagent_mobile_runtime::MobileService` and provides DTO-returning
/// operations for Tauri commands and agent tools.
pub struct AppMobileService {
    inner: Arc<MobileService>,
    default_timeout: Duration,
}

impl AppMobileService {
    /// Create a new AppMobileService with default settings.
    pub fn new() -> Self {
        let (event_tx, _event_rx) = mpsc::unbounded_channel::<MobileEvent>();
        let registry = DeviceRegistry::new(event_tx);
        let artifact_store = ArtifactStore::new();
        let snapshot_store = SnapshotStore::new();
        let inner = MobileService::new(registry, artifact_store, snapshot_store);

        Self {
            inner: Arc::new(inner),
            default_timeout: Duration::from_secs(30),
        }
    }

    /// Create from an existing MobileService (for testing or custom setup).
    pub fn from_service(service: MobileService) -> Self {
        Self {
            inner: Arc::new(service),
            default_timeout: Duration::from_secs(30),
        }
    }

    /// Set the default operation timeout.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.default_timeout = timeout;
        self
    }

    /// Get a reference to the underlying MobileService.
    pub fn inner(&self) -> &MobileService {
        &self.inner
    }

    /// Probe all registered backends.
    pub async fn probe_backends(&self) -> Vec<BackendStatusDto> {
        self.inner
            .probe_all()
            .await
            .into_iter()
            .map(BackendStatusDto::from)
            .collect()
    }

    /// List all known devices.
    pub async fn list_devices(&self) -> Vec<DeviceDto> {
        self.inner
            .list_devices()
            .await
            .into_iter()
            .map(DeviceDto::from)
            .collect()
    }

    /// Get device info by ID.
    pub async fn device_info(&self, device_id: &str) -> MobileResult<DeviceDto> {
        let device = self.inner.device_info(device_id).await?;
        Ok(DeviceDto::from(device))
    }

    /// Capture a screenshot and return the artifact reference DTO.
    pub async fn screenshot(&self, device_id: &str) -> MobileResult<ArtifactRefDto> {
        let ctx = self.make_context(device_id, "screenshot");
        let artifact = self.inner.screenshot(device_id, &ctx).await?;
        Ok(ArtifactRefDto::from(artifact))
    }

    /// Capture a UI snapshot and return it.
    pub async fn ui_snapshot(&self, device_id: &str) -> MobileResult<UiSnapshot> {
        let ctx = self.make_context(device_id, "ui_snapshot");
        self.inner.ui_snapshot(device_id, &ctx).await
    }

    /// Get a UI snapshot summary DTO (without the full tree).
    pub async fn ui_snapshot_summary(&self, device_id: &str) -> MobileResult<UiSnapshotSummaryDto> {
        let snapshot = self.ui_snapshot(device_id).await?;
        let max_depth = snapshot.max_depth();
        Ok(UiSnapshotSummaryDto {
            snapshot_id: snapshot.snapshot_id,
            device_id: snapshot.device_id,
            node_count: snapshot.nodes.len(),
            max_depth,
            root_node_id: snapshot.root_node_id,
            captured_at_ms: snapshot.captured_at_ms,
        })
    }

    /// Install an application.
    pub async fn install(&self, request: &InstallRequest) -> MobileResult<()> {
        let ctx = self.make_context(&request.device_id, "install");
        self.inner.install(request, &ctx).await
    }

    /// Launch an application.
    pub async fn launch(&self, request: &LaunchRequest) -> MobileResult<()> {
        let ctx = self.make_context(&request.device_id, "launch");
        self.inner.launch(request, &ctx).await
    }

    /// Terminate a running application.
    pub async fn terminate(&self, target: &AppTarget) -> MobileResult<()> {
        let ctx = self.make_context(&target.device_id, "terminate");
        self.inner.terminate(target, &ctx).await
    }

    /// Perform an input action.
    pub async fn input(&self, request: &InputRequest) -> MobileResult<InputResult> {
        let ctx = self.make_context(&request.device_id, "input");
        self.inner.input(request, &ctx).await
    }

    /// Read device logs.
    pub async fn read_logs(&self, request: &LogRequest) -> MobileResult<LogPage> {
        let ctx = self.make_context(&request.device_id, "read_logs");
        self.inner.read_logs(request, &ctx).await
    }

    /// List available Android Virtual Devices.
    pub async fn list_avds(&self) -> MobileResult<Vec<AvdInfo>> {
        let ctx = self.make_context("android", "list_avds");
        self.inner.list_avds(&ctx).await
    }

    /// Start an Android Emulator.
    pub async fn start_emulator(&self, request: &StartEmulatorRequest) -> MobileResult<String> {
        let ctx = self.make_context("emulator", "start_emulator");
        self.inner.start_emulator(request, &ctx).await
    }

    /// Stop an Android Emulator.
    pub async fn stop_emulator(&self, request: &StopEmulatorRequest) -> MobileResult<()> {
        let ctx = self.make_context("emulator", "stop_emulator");
        self.inner.stop_emulator(request, &ctx).await
    }

    /// Get the device registry for direct access if needed.
    pub fn registry(&self) -> &DeviceRegistry {
        self.inner.registry()
    }

    /// Get the artifact store for direct access if needed.
    pub fn artifact_store(&self) -> &ArtifactStore {
        self.inner.artifact_store()
    }

    /// Get the snapshot store for direct access if needed.
    pub fn snapshot_store(&self) -> &SnapshotStore {
        self.inner.snapshot_store()
    }

    /// Create an OperationContext with the default timeout.
    fn make_context(&self, device_id: &str, operation: &str) -> OperationContext {
        self.inner
            .operation_context(device_id, operation, self.default_timeout)
    }
}

impl Default for AppMobileService {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use deepagent_mobile_core::{MobileError, MobilePlatform};

    #[tokio::test]
    async fn app_mobile_service_creation() {
        let service = AppMobileService::new();
        assert!(service.list_devices().await.is_empty());
    }

    #[tokio::test]
    async fn app_mobile_service_probe_empty() {
        let service = AppMobileService::new();
        let statuses = service.probe_backends().await;
        assert!(statuses.is_empty());
    }

    #[tokio::test]
    async fn app_mobile_service_device_not_found() {
        let service = AppMobileService::new();
        let result = service.device_info("nonexistent").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn app_mobile_service_list_avds_no_backend() {
        let service = AppMobileService::new();
        let result = service.list_avds().await;
        assert!(matches!(
            result,
            Err(MobileError::BackendUnavailable { .. })
        ));
    }

    #[tokio::test]
    async fn app_mobile_service_custom_timeout() {
        let service = AppMobileService::new().with_timeout(Duration::from_secs(60));
        assert!(service.list_devices().await.is_empty());
    }

    #[test]
    fn device_dto_from_mobile_device() {
        let device = MobileDevice {
            id: "dev-1".into(),
            name: "Test Device".into(),
            platform: MobilePlatform::Android,
            kind: deepagent_mobile_core::DeviceKind::Physical,
            connection: deepagent_mobile_core::DeviceConnection::Usb,
            state: deepagent_mobile_core::DeviceState::Ready,
            os_version: Some("14".into()),
            capabilities: deepagent_mobile_core::DeviceCapabilities {
                screenshot: true,
                ui_tree: true,
                input: true,
                logs: true,
                install: true,
                network_inspection: false,
            },
        };
        let dto = DeviceDto::from(device);
        assert_eq!(dto.id, "dev-1");
        assert_eq!(dto.name, "Test Device");
        assert!(dto.can_screenshot);
        assert!(dto.can_ui_tree);
    }

    #[test]
    fn backend_status_dto_from_backend_status() {
        let status = BackendStatus {
            platform: MobilePlatform::Android,
            available: true,
            toolchain_version: Some("1.0".into()),
            tool_paths: vec![],
            diagnostics: vec!["OK".into()],
        };
        let dto = BackendStatusDto::from(status);
        assert_eq!(dto.platform, "Android");
        assert!(dto.available);
        assert_eq!(dto.toolchain_version, Some("1.0".into()));
        assert_eq!(dto.diagnostics, vec!["OK"]);
    }
}
