use async_trait::async_trait;
use deepagent_mobile_core::*;
use deepagent_mobile_protocol::*;
use deepagent_mobile_runtime::{MobileBackend, OperationContext};
use std::sync::Arc;
use tokio::sync::Mutex;

/// Fake Android backend for testing without real devices or ADB.
///
/// Returns configurable device lists and accepts all operations without
/// executing real commands. Used in unit tests and CI where no Android SDK
/// is available.
pub struct FakeAndroidBackend {
    devices: Arc<Mutex<Vec<MobileDevice>>>,
    probe_result: Arc<Mutex<BackendStatus>>,
}

impl FakeAndroidBackend {
    pub fn new() -> Self {
        Self {
            devices: Arc::new(Mutex::new(Vec::new())),
            probe_result: Arc::new(Mutex::new(BackendStatus {
                platform: MobilePlatform::Android,
                available: true,
                toolchain_version: Some("fake-1.0.41".into()),
                tool_paths: vec![ToolPath {
                    name: "adb".into(),
                    path: "/fake/adb".into(),
                    version: Some("1.0.41".into()),
                }],
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

    pub fn fake_device(id: &str, state: DeviceState) -> MobileDevice {
        MobileDevice {
            id: id.into(),
            name: format!("Fake {id}"),
            platform: MobilePlatform::Android,
            kind: DeviceKind::Emulator,
            connection: DeviceConnection::Local,
            state,
            os_version: Some("14".into()),
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

impl Default for FakeAndroidBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl MobileBackend for FakeAndroidBackend {
    async fn probe(&self) -> MobileResult<BackendStatus> {
        Ok(self.probe_result.lock().await.clone())
    }

    async fn list_devices(&self, ctx: &OperationContext) -> MobileResult<Vec<MobileDevice>> {
        if ctx.is_cancelled() {
            return Err(MobileError::Cancelled {
                operation_id: ctx.operation_id.clone(),
            });
        }
        Ok(self.devices.lock().await.clone())
    }

    async fn device_info(
        &self,
        device_id: &str,
        ctx: &OperationContext,
    ) -> MobileResult<MobileDevice> {
        if ctx.is_cancelled() {
            return Err(MobileError::Cancelled {
                operation_id: ctx.operation_id.clone(),
            });
        }
        let devices = self.devices.lock().await;
        devices
            .iter()
            .find(|d| d.id == device_id)
            .cloned()
            .ok_or_else(|| MobileError::DeviceNotFound {
                device_id: device_id.to_string(),
            })
    }

    async fn screenshot(
        &self,
        device_id: &str,
        ctx: &OperationContext,
    ) -> MobileResult<ArtifactRef> {
        if ctx.is_cancelled() {
            return Err(MobileError::Cancelled {
                operation_id: ctx.operation_id.clone(),
            });
        }
        Ok(ArtifactRef {
            artifact_id: format!("fake-screenshot-{device_id}"),
            mime: "image/png".into(),
            size_bytes: 1024,
            sha256: Some("fake-sha256".into()),
            storage_path: format!("/fake/artifacts/{device_id}/screenshot.png"),
        })
    }

    async fn ui_snapshot(
        &self,
        device_id: &str,
        ctx: &OperationContext,
    ) -> MobileResult<UiSnapshot> {
        if ctx.is_cancelled() {
            return Err(MobileError::Cancelled {
                operation_id: ctx.operation_id.clone(),
            });
        }
        Ok(UiSnapshot {
            snapshot_id: format!("fake-snap-{device_id}"),
            device_id: device_id.to_string(),
            root_node_id: "root".into(),
            captured_at_ms: 0,
            nodes: vec![UiNode {
                node_id: "root".into(),
                parent_id: None,
                role: UiRole::Page,
                text: None,
                label: None,
                accessibility_id: None,
                bounds: Bounds {
                    x: 0,
                    y: 0,
                    width: 1080,
                    height: 1920,
                },
                visible: true,
                enabled: true,
                clickable: false,
                editable: false,
                children: vec![],
                source: UiNodeSource::AndroidUiAutomator,
            }],
        })
    }

    async fn install(&self, _request: &InstallRequest, ctx: &OperationContext) -> MobileResult<()> {
        if ctx.is_cancelled() {
            return Err(MobileError::Cancelled {
                operation_id: ctx.operation_id.clone(),
            });
        }
        Ok(())
    }

    async fn launch(&self, _request: &LaunchRequest, ctx: &OperationContext) -> MobileResult<()> {
        if ctx.is_cancelled() {
            return Err(MobileError::Cancelled {
                operation_id: ctx.operation_id.clone(),
            });
        }
        Ok(())
    }

    async fn terminate(&self, _target: &AppTarget, ctx: &OperationContext) -> MobileResult<()> {
        if ctx.is_cancelled() {
            return Err(MobileError::Cancelled {
                operation_id: ctx.operation_id.clone(),
            });
        }
        Ok(())
    }

    async fn input(
        &self,
        _request: &InputRequest,
        ctx: &OperationContext,
    ) -> MobileResult<InputResult> {
        if ctx.is_cancelled() {
            return Err(MobileError::Cancelled {
                operation_id: ctx.operation_id.clone(),
            });
        }
        Ok(InputResult { accepted: true })
    }

    async fn read_logs(
        &self,
        request: &LogRequest,
        ctx: &OperationContext,
    ) -> MobileResult<LogPage> {
        if ctx.is_cancelled() {
            return Err(MobileError::Cancelled {
                operation_id: ctx.operation_id.clone(),
            });
        }
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

    fn ctx() -> OperationContext {
        OperationContext::new("op-test".into(), "dev-1".into(), Duration::from_secs(30))
    }

    #[tokio::test]
    async fn fake_probe_reports_available() {
        let backend = FakeAndroidBackend::new();
        let status = backend.probe().await.unwrap();
        assert!(status.available);
        assert_eq!(status.platform, MobilePlatform::Android);
    }

    #[tokio::test]
    async fn fake_list_empty_by_default() {
        let backend = FakeAndroidBackend::new();
        let devices = backend.list_devices(&ctx()).await.unwrap();
        assert!(devices.is_empty());
    }

    #[tokio::test]
    async fn fake_set_and_list_devices() {
        let backend = FakeAndroidBackend::new();
        backend
            .set_devices(vec![FakeAndroidBackend::fake_device(
                "emulator-5554",
                DeviceState::Ready,
            )])
            .await;
        let devices = backend.list_devices(&ctx()).await.unwrap();
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].id, "emulator-5554");
    }

    #[tokio::test]
    async fn fake_device_info_not_found() {
        let backend = FakeAndroidBackend::new();
        let err = backend
            .device_info("nonexistent", &ctx())
            .await
            .unwrap_err();
        assert!(matches!(err, MobileError::DeviceNotFound { .. }));
    }

    #[tokio::test]
    async fn fake_cancelled_operation() {
        let backend = FakeAndroidBackend::new();
        let context = ctx();
        context.cancel();
        let err = backend.list_devices(&context).await.unwrap_err();
        assert!(matches!(err, MobileError::Cancelled { .. }));
    }

    #[tokio::test]
    async fn fake_screenshot_returns_artifact() {
        let backend = FakeAndroidBackend::new();
        let artifact = backend.screenshot("dev-1", &ctx()).await.unwrap();
        assert_eq!(artifact.mime, "image/png");
        assert!(artifact.storage_path.contains("dev-1"));
    }

    #[tokio::test]
    async fn fake_ui_snapshot_returns_tree() {
        let backend = FakeAndroidBackend::new();
        let snap = backend.ui_snapshot("dev-1", &ctx()).await.unwrap();
        assert_eq!(snap.node_count(), 1);
        assert!(!snap.has_duplicate_ids());
        assert!(!snap.has_dangling_children());
    }

    #[tokio::test]
    async fn fake_input_accepted() {
        let backend = FakeAndroidBackend::new();
        let result = backend
            .input(
                &InputRequest {
                    device_id: "dev-1".into(),
                    snapshot_id: None,
                    action: InputAction::Tap { x: 100, y: 200 },
                },
                &ctx(),
            )
            .await
            .unwrap();
        assert!(result.accepted);
    }
}
