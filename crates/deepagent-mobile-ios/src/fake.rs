use async_trait::async_trait;
use deepagent_mobile_core::*;
use deepagent_mobile_protocol::*;
use deepagent_mobile_runtime::{MobileBackend, OperationContext};
use std::sync::Arc;
use tokio::sync::Mutex;

/// Fake iOS backend for testing without real devices or Xcode.
///
/// Returns configurable device lists and accepts all operations without
/// executing real commands. Used in unit tests and CI where no macOS/Xcode
/// is available.
pub struct FakeIosBackend {
    devices: Arc<Mutex<Vec<MobileDevice>>>,
    probe_result: Arc<Mutex<BackendStatus>>,
}

impl FakeIosBackend {
    pub fn new() -> Self {
        Self {
            devices: Arc::new(Mutex::new(Vec::new())),
            probe_result: Arc::new(Mutex::new(BackendStatus {
                platform: MobilePlatform::Ios,
                available: true,
                toolchain_version: Some("fake-xcode-15.0".into()),
                tool_paths: vec![
                    ToolPath {
                        name: "simctl".into(),
                        path: "/fake/simctl".into(),
                        version: Some("15.0".into()),
                    },
                    ToolPath {
                        name: "devicectl".into(),
                        path: "/fake/devicectl".into(),
                        version: Some("15.0".into()),
                    },
                ],
                diagnostics: vec![],
            })),
        }
    }

    pub async fn set_devices(&self, devices: Vec<MobileDevice>) {
        *self.devices.lock().await = devices;
    }

    pub async fn set_probe_result(&self, status: BackendStatus) {
        *self.probe_result.lock().await = status;
    }

    pub fn fake_simulator(udid: &str, state: DeviceState) -> MobileDevice {
        MobileDevice {
            id: format!("ios-sim-{udid}"),
            name: format!("iPhone 15 ({udid})"),
            platform: MobilePlatform::Ios,
            kind: DeviceKind::Simulator,
            connection: DeviceConnection::Local,
            state,
            os_version: Some("17.0".into()),
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

    pub fn fake_physical(udid: &str, state: DeviceState) -> MobileDevice {
        MobileDevice {
            id: format!("ios-usb-{udid}"),
            name: format!("iPhone ({udid})"),
            platform: MobilePlatform::Ios,
            kind: DeviceKind::Physical,
            connection: DeviceConnection::Usb,
            state,
            os_version: Some("17.0".into()),
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
}

impl Default for FakeIosBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl MobileBackend for FakeIosBackend {
    async fn probe(&self) -> MobileResult<BackendStatus> {
        Ok(self.probe_result.lock().await.clone())
    }

    async fn list_devices(&self, _ctx: &OperationContext) -> MobileResult<Vec<MobileDevice>> {
        Ok(self.devices.lock().await.clone())
    }

    async fn device_info(
        &self,
        device_id: &str,
        _ctx: &OperationContext,
    ) -> MobileResult<MobileDevice> {
        let devices = self.devices.lock().await;
        devices
            .iter()
            .find(|d| d.id == device_id)
            .cloned()
            .ok_or_else(|| MobileError::ToolNotFound {
                tool_name: format!("device {device_id}"),
            })
    }

    async fn screenshot(
        &self,
        _device_id: &str,
        _ctx: &OperationContext,
    ) -> MobileResult<ArtifactRef> {
        Ok(ArtifactRef {
            artifact_id: "fake-ios-screenshot".into(),
            mime: "image/png".into(),
            size_bytes: 0,
            sha256: None,
            storage_path: "/fake/ios-screenshot.png".into(),
        })
    }

    async fn ui_snapshot(
        &self,
        _device_id: &str,
        _ctx: &OperationContext,
    ) -> MobileResult<UiSnapshot> {
        Ok(UiSnapshot {
            snapshot_id: "fake-ios-snap".into(),
            device_id: "fake".into(),
            root_node_id: "root".into(),
            nodes: vec![UiNode {
                node_id: "root".into(),
                parent_id: None,
                role: UiRole::Page,
                text: Some("Fake iOS Screen".into()),
                label: None,
                accessibility_id: None,
                bounds: Bounds {
                    x: 0,
                    y: 0,
                    width: 393,
                    height: 852,
                },
                visible: true,
                enabled: true,
                clickable: false,
                editable: false,
                children: vec![],
                source: UiNodeSource::IosXctest,
            }],
            captured_at_ms: 0,
        })
    }

    async fn install(
        &self,
        _request: &InstallRequest,
        _ctx: &OperationContext,
    ) -> MobileResult<()> {
        Ok(())
    }

    async fn launch(&self, _request: &LaunchRequest, _ctx: &OperationContext) -> MobileResult<()> {
        Ok(())
    }

    async fn terminate(&self, _target: &AppTarget, _ctx: &OperationContext) -> MobileResult<()> {
        Ok(())
    }

    async fn input(
        &self,
        _request: &InputRequest,
        _ctx: &OperationContext,
    ) -> MobileResult<InputResult> {
        Ok(InputResult { accepted: true })
    }

    async fn read_logs(
        &self,
        request: &LogRequest,
        _ctx: &OperationContext,
    ) -> MobileResult<LogPage> {
        Ok(LogPage {
            device_id: request.device_id.clone(),
            records: vec![],
            truncated: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn test_ctx() -> OperationContext {
        OperationContext::new("op-1".into(), "dev-1".into(), Duration::from_secs(30))
    }

    #[tokio::test]
    async fn fake_probe_returns_ios_platform() {
        let backend = FakeIosBackend::new();
        let status = backend.probe().await.unwrap();
        assert_eq!(status.platform, MobilePlatform::Ios);
        assert!(status.available);
    }

    #[tokio::test]
    async fn fake_device_lifecycle() {
        let backend = FakeIosBackend::new();
        let ctx = test_ctx();
        let sim = FakeIosBackend::fake_simulator("ABCD", DeviceState::Ready);
        backend.set_devices(vec![sim.clone()]).await;

        let devices = backend.list_devices(&ctx).await.unwrap();
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].id, "ios-sim-ABCD");
        assert_eq!(devices[0].platform, MobilePlatform::Ios);
        assert_eq!(devices[0].kind, DeviceKind::Simulator);

        let info = backend.device_info("ios-sim-ABCD", &ctx).await.unwrap();
        assert_eq!(info.name, "iPhone 15 (ABCD)");
    }

    #[tokio::test]
    async fn fake_device_not_found() {
        let backend = FakeIosBackend::new();
        let ctx = test_ctx();
        let result = backend.device_info("nonexistent", &ctx).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn fake_screenshot_returns_artifact() {
        let backend = FakeIosBackend::new();
        let ctx = test_ctx();
        let artifact = backend.screenshot("any-device", &ctx).await.unwrap();
        assert_eq!(artifact.mime, "image/png");
    }

    #[tokio::test]
    async fn fake_ui_snapshot_has_ios_source() {
        let backend = FakeIosBackend::new();
        let ctx = test_ctx();
        let snap = backend.ui_snapshot("any-device", &ctx).await.unwrap();
        assert_eq!(snap.nodes[0].source, UiNodeSource::IosXctest);
        assert_eq!(snap.root_node_id, "root");
    }

    #[tokio::test]
    async fn fake_input_accepted() {
        let backend = FakeIosBackend::new();
        let ctx = test_ctx();
        let result = backend
            .input(
                &InputRequest {
                    device_id: "dev".into(),
                    snapshot_id: None,
                    action: InputAction::Tap { x: 100, y: 200 },
                },
                &ctx,
            )
            .await
            .unwrap();
        assert!(result.accepted);
    }

    #[tokio::test]
    async fn fake_physical_device() {
        let phys = FakeIosBackend::fake_physical("UDID-123", DeviceState::Ready);
        assert_eq!(phys.kind, DeviceKind::Physical);
        assert_eq!(phys.connection, DeviceConnection::Usb);
        assert!(phys.id.starts_with("ios-usb-"));
    }
}
