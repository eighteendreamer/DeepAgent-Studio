//! H-50 contract tests. The tool bridge is test-only: production registration
//! of mobile tools does not exist yet, and is not introduced by this repair.
use super::*;
use async_trait::async_trait;
use deepagent_builtins::mobile_tools::{
    mobile_tools, ArtifactRefDto, MobileBackend as ToolBackend, MobileDeviceDto,
};
use deepagent_core::error::{CoreError, Result};
use deepagent_mobile_android::FakeAndroidBackend;
use deepagent_mobile_core::{DeviceState, MobileError, MobilePlatform};
use deepagent_tools::{PermissionSet, ToolRegistry};
use serde_json::json;

struct ToolBridge(Arc<AppMobileService>);

fn tool_error(error: MobileError) -> CoreError {
    CoreError::invalid(error.to_string())
}

#[async_trait]
impl ToolBackend for ToolBridge {
    async fn list_devices(&self) -> Result<Vec<MobileDeviceDto>> {
        Ok(self
            .0
            .inner
            .list_devices()
            .await
            .into_iter()
            .map(Into::into)
            .collect())
    }
    async fn device_info(&self, id: &str) -> Result<MobileDeviceDto> {
        self.0
            .inner
            .device_info(id)
            .await
            .map(Into::into)
            .map_err(tool_error)
    }
    async fn screenshot(&self, id: &str) -> Result<ArtifactRefDto> {
        let context = self.0.make_context(id, "screenshot");
        self.0
            .inner
            .screenshot(id, &context)
            .await
            .map(Into::into)
            .map_err(tool_error)
    }
    async fn ui_snapshot(&self, id: &str) -> Result<UiSnapshot> {
        self.0.ui_snapshot(id).await.map_err(tool_error)
    }
    async fn install(&self, request: InstallRequest) -> Result<()> {
        self.0.install(&request).await.map_err(tool_error)
    }
    async fn launch(&self, request: LaunchRequest) -> Result<()> {
        self.0.launch(&request).await.map_err(tool_error)
    }
    async fn terminate(&self, target: AppTarget) -> Result<()> {
        self.0.terminate(&target).await.map_err(tool_error)
    }
    async fn input(&self, request: InputRequest) -> Result<InputResult> {
        self.0.input(&request).await.map_err(tool_error)
    }
}

async fn fixture() -> Arc<AppMobileService> {
    let service = Arc::new(AppMobileService::new());
    service
        .inner
        .registry()
        .upsert(FakeAndroidBackend::fake_device(
            "offline-android",
            DeviceState::Ready,
        ))
        .await;
    service
        .inner
        .register_backend(MobilePlatform::Android, Arc::new(FakeAndroidBackend::new()))
        .await;
    service
}

#[tokio::test]
async fn tool_schema_and_invoke_preserve_explicit_snapshot_fence() {
    let service = fixture().await;
    let mut registry = ToolRegistry::new();
    for tool in mobile_tools(ToolBridge(service.clone())) {
        registry.register(tool).unwrap();
    }
    let descriptor = &registry.get("mobile_input").unwrap().descriptor;
    assert_eq!(
        descriptor.parameters["properties"]["snapshot_id"]["type"],
        "string"
    );
    let permissions = PermissionSet::read_only();
    let snapshot = registry
        .invoke(
            "mobile_ui_snapshot",
            json!({"device_id": "offline-android"}),
            &permissions,
            false,
        )
        .await
        .unwrap();
    assert!(snapshot.ok);
    let id = snapshot.value["snapshot_id"].as_str().unwrap();
    // Only the service-issued identity is valid, including after serialization.
    assert_eq!(
        service
            .snapshot_store()
            .current("offline-android")
            .await
            .as_deref(),
        Some(id)
    );
    let mut request =
        json!({"device_id": "offline-android", "action": "tap", "x": 1, "y": 1, "snapshot_id": id});
    let accepted = registry
        .invoke("mobile_input", request.clone(), &permissions, false)
        .await
        .unwrap();
    assert!(accepted.ok);
    assert_eq!(accepted.value["accepted"], true);
    let stale = registry
        .invoke("mobile_input", request.clone(), &permissions, false)
        .await
        .unwrap();
    assert!(
        !stale.ok,
        "dropping snapshot_id here would dispatch manual input"
    );
    request["snapshot_id"] = json!("unknown");
    let unknown = registry
        .invoke("mobile_input", request.clone(), &permissions, false)
        .await
        .unwrap();
    assert!(!unknown.ok);
    // A malformed reference cannot silently turn into uncorrelated input.
    request["snapshot_id"] = json!(false);
    assert!(registry
        .validate_invocation("mobile_input", request.clone())
        .is_err());
    request.as_object_mut().unwrap().remove("snapshot_id");
    assert!(
        registry
            .invoke("mobile_input", request, &permissions, false)
            .await
            .unwrap()
            .ok
    );
}

#[tokio::test]
async fn desktop_snapshot_views_keep_service_identity_and_revoke_previous_view() {
    let service = fixture().await;
    let summary = service
        .ui_snapshot_summary("offline-android")
        .await
        .unwrap();
    assert_eq!(
        service.snapshot_store().current("offline-android").await,
        Some(summary.snapshot_id.clone())
    );
    let full = service.ui_snapshot_full("offline-android").await.unwrap();
    assert_ne!(summary.snapshot_id, full.snapshot_id);
    let request = |id: String| InputRequest {
        device_id: "offline-android".into(),
        snapshot_id: Some(id),
        action: deepagent_mobile_protocol::InputAction::Tap { x: 1, y: 1 },
    };
    // Same serde DTO accepted by mobile_ui_action; no UI or real device is used.
    let wire = serde_json::to_value(request(summary.snapshot_id)).unwrap();
    let decoded = serde_json::from_value::<InputRequest>(wire).unwrap();
    assert!(matches!(
        service.input(&decoded).await,
        Err(MobileError::StaleUiNode { .. })
    ));
    assert!(
        service
            .input(&request(full.snapshot_id))
            .await
            .unwrap()
            .accepted
    );
}
