//! `mobile_list_devices` / `mobile_screenshot` / `mobile_ui_snapshot` / etc.
//! — mobile device debugging tools for the agent.
//!
//! These tools let the agent interact with Android/iOS devices and emulators
//! through the mobile runtime. All operations go through a pluggable
//! [`MobileBackend`] trait, so the desktop app can wire up the real
//! `AppMobileService` while tests use a stub.
//!
//! ## Tool inventory
//!
//! | built-in                  | risk   | permission    |
//! |---------------------------|--------|---------------|
//! | `mobile_list_devices`     | Safe   | read-only     |
//! | `mobile_device_info`      | Safe   | read-only     |
//! | `mobile_screenshot`       | Low    | ReadOnly      |
//! | `mobile_ui_snapshot`      | Safe   | read-only     |
//!
//! ## Safety model
//!
//! - Read-only operations (list, info, ui_snapshot) are safe and need no
//!   approval.
//! - Screenshot is low risk (writes an artifact).
//! - The agent never sees ADB/simctl commands — only structured results.

use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use deepagent_core::error::{CoreError, Result};
use deepagent_mobile_core::{ArtifactRef, MobileDevice};
use deepagent_mobile_protocol::{
    AppTarget, InputAction, InputRequest, InputResult, InstallRequest, LaunchRequest, UiSnapshot,
};
use deepagent_tools::permission::{PermissionSet, RiskLevel};
use deepagent_tools::{Tool, ToolDescriptor, ToolOutput};

/// Tool names for mobile operations.
pub const MOBILE_LIST_DEVICES_TOOL_NAME: &str = "mobile_list_devices";
pub const MOBILE_DEVICE_INFO_TOOL_NAME: &str = "mobile_device_info";
pub const MOBILE_SCREENSHOT_TOOL_NAME: &str = "mobile_screenshot";
pub const MOBILE_UI_SNAPSHOT_TOOL_NAME: &str = "mobile_ui_snapshot";
pub const MOBILE_INSTALL_TOOL_NAME: &str = "mobile_install";
pub const MOBILE_LAUNCH_TOOL_NAME: &str = "mobile_launch";
pub const MOBILE_TERMINATE_TOOL_NAME: &str = "mobile_terminate";
pub const MOBILE_INPUT_TOOL_NAME: &str = "mobile_input";
pub const MOBILE_READ_LOGS_TOOL_NAME: &str = "mobile_read_logs";

/// Serializable device info for tool output.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MobileDeviceDto {
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

impl From<MobileDevice> for MobileDeviceDto {
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

/// Serializable artifact reference for tool output.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactRefDto {
    pub artifact_id: String,
    pub mime: String,
    pub size_bytes: u64,
    pub sha256: Option<String>,
    pub storage_path: String,
}

impl From<ArtifactRef> for ArtifactRefDto {
    fn from(a: ArtifactRef) -> Self {
        Self {
            artifact_id: a.artifact_id,
            mime: a.mime,
            size_bytes: a.size_bytes,
            sha256: a.sha256,
            storage_path: a.storage_path,
        }
    }
}

/// Bridges mobile tools to the host's mobile service.
#[async_trait]
pub trait MobileBackend: Send + Sync {
    /// List all known devices.
    async fn list_devices(&self) -> Result<Vec<MobileDeviceDto>>;

    /// Get device info by ID.
    async fn device_info(&self, device_id: &str) -> Result<MobileDeviceDto>;

    /// Capture a screenshot.
    async fn screenshot(&self, device_id: &str) -> Result<ArtifactRefDto>;

    /// Capture a UI snapshot.
    async fn ui_snapshot(&self, device_id: &str) -> Result<UiSnapshot>;

    /// Install an application.
    async fn install(&self, request: InstallRequest) -> Result<()>;

    /// Launch an application.
    async fn launch(&self, request: LaunchRequest) -> Result<()>;

    /// Terminate a running application.
    async fn terminate(&self, target: AppTarget) -> Result<()>;

    /// Perform an input action.
    async fn input(&self, request: InputRequest) -> Result<InputResult>;
}

/// A backend that reports mobile is unavailable (headless default).
#[derive(Debug, Default, Clone, Copy)]
pub struct UnavailableMobileBackend;

#[async_trait]
impl MobileBackend for UnavailableMobileBackend {
    async fn list_devices(&self) -> Result<Vec<MobileDeviceDto>> {
        Ok(vec![])
    }

    async fn device_info(&self, device_id: &str) -> Result<MobileDeviceDto> {
        Err(CoreError::invalid(format!(
            "mobile backend unavailable, cannot query device {device_id}"
        )))
    }

    async fn screenshot(&self, device_id: &str) -> Result<ArtifactRefDto> {
        Err(CoreError::invalid(format!(
            "mobile backend unavailable, cannot screenshot {device_id}"
        )))
    }

    async fn ui_snapshot(&self, device_id: &str) -> Result<UiSnapshot> {
        Err(CoreError::invalid(format!(
            "mobile backend unavailable, cannot snapshot {device_id}"
        )))
    }

    async fn install(&self, _request: InstallRequest) -> Result<()> {
        Err(CoreError::invalid(
            "mobile backend unavailable, cannot install",
        ))
    }

    async fn launch(&self, _request: LaunchRequest) -> Result<()> {
        Err(CoreError::invalid(
            "mobile backend unavailable, cannot launch",
        ))
    }

    async fn terminate(&self, _target: AppTarget) -> Result<()> {
        Err(CoreError::invalid(
            "mobile backend unavailable, cannot terminate",
        ))
    }

    async fn input(&self, _request: InputRequest) -> Result<InputResult> {
        Err(CoreError::invalid(
            "mobile backend unavailable, cannot input",
        ))
    }
}

// ---------------------------------------------------------------------------
// Tool implementations
// ---------------------------------------------------------------------------

/// `mobile_list_devices` — list all known devices (safe, read-only).
pub struct MobileListDevicesTool {
    backend: Arc<dyn MobileBackend>,
}

impl MobileListDevicesTool {
    pub fn new(backend: impl MobileBackend + 'static) -> Self {
        Self {
            backend: Arc::new(backend),
        }
    }
}

#[async_trait]
impl Tool for MobileListDevicesTool {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: MOBILE_LIST_DEVICES_TOOL_NAME.into(),
            description: "List all known mobile devices (Android/iOS, local/remote). Returns device IDs, platforms, states, and capabilities. Safe read-only operation.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {},
            }),
            required_permissions: PermissionSet::read_only(),
            risk: RiskLevel::Safe,
        }
    }

    async fn invoke(&self, _input: serde_json::Value) -> Result<ToolOutput> {
        match self.backend.list_devices().await {
            Ok(devices) => Ok(ToolOutput::success(serde_json::json!({
                "count": devices.len(),
                "devices": devices,
            }))),
            Err(e) => Ok(ToolOutput::failure(format!("failed to list devices: {e}"))),
        }
    }
}

/// `mobile_device_info` — get detailed info for a device (safe, read-only).
pub struct MobileDeviceInfoTool {
    backend: Arc<dyn MobileBackend>,
}

impl MobileDeviceInfoTool {
    pub fn new(backend: impl MobileBackend + 'static) -> Self {
        Self {
            backend: Arc::new(backend),
        }
    }
}

#[async_trait]
impl Tool for MobileDeviceInfoTool {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: MOBILE_DEVICE_INFO_TOOL_NAME.into(),
            description: "Get detailed information for a specific mobile device by ID. Returns platform, state, capabilities, and OS version. Safe read-only operation.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "device_id": {
                        "type": "string",
                        "description": "The device ID (from mobile_list_devices)"
                    }
                },
                "required": ["device_id"]
            }),
            required_permissions: PermissionSet::read_only(),
            risk: RiskLevel::Safe,
        }
    }

    async fn invoke(&self, input: serde_json::Value) -> Result<ToolOutput> {
        let Some(device_id) = input.get("device_id").and_then(|v| v.as_str()) else {
            return Ok(ToolOutput::failure("missing 'device_id'"));
        };
        match self.backend.device_info(device_id).await {
            Ok(device) => Ok(ToolOutput::success(serde_json::to_value(&device)?)),
            Err(e) => Ok(ToolOutput::failure(format!(
                "failed to get device info: {e}"
            ))),
        }
    }
}

/// `mobile_screenshot` — capture a screenshot (low risk).
pub struct MobileScreenshotTool {
    backend: Arc<dyn MobileBackend>,
}

impl MobileScreenshotTool {
    pub fn new(backend: impl MobileBackend + 'static) -> Self {
        Self {
            backend: Arc::new(backend),
        }
    }
}

#[async_trait]
impl Tool for MobileScreenshotTool {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: MOBILE_SCREENSHOT_TOOL_NAME.into(),
            description: "Capture a screenshot from a mobile device. Returns an artifact reference with the image path. Requires device_id.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "device_id": {
                        "type": "string",
                        "description": "The device ID"
                    }
                },
                "required": ["device_id"]
            }),
            required_permissions: PermissionSet::read_only(),
            risk: RiskLevel::Low,
        }
    }

    async fn invoke(&self, input: serde_json::Value) -> Result<ToolOutput> {
        let Some(device_id) = input.get("device_id").and_then(|v| v.as_str()) else {
            return Ok(ToolOutput::failure("missing 'device_id'"));
        };
        match self.backend.screenshot(device_id).await {
            Ok(artifact) => Ok(ToolOutput::success(serde_json::to_value(&artifact)?)),
            Err(e) => Ok(ToolOutput::failure(format!(
                "failed to capture screenshot: {e}"
            ))),
        }
    }
}

/// `mobile_ui_snapshot` — capture UI hierarchy (safe, read-only).
pub struct MobileUiSnapshotTool {
    backend: Arc<dyn MobileBackend>,
}

impl MobileUiSnapshotTool {
    pub fn new(backend: impl MobileBackend + 'static) -> Self {
        Self {
            backend: Arc::new(backend),
        }
    }
}

#[async_trait]
impl Tool for MobileUiSnapshotTool {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: MOBILE_UI_SNAPSHOT_TOOL_NAME.into(),
            description: "Capture the full UI hierarchy from a mobile device. Returns node tree with IDs for subsequent input operations. Safe read-only.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "device_id": {
                        "type": "string",
                        "description": "The device ID"
                    }
                },
                "required": ["device_id"]
            }),
            required_permissions: PermissionSet::read_only(),
            risk: RiskLevel::Safe,
        }
    }

    async fn invoke(&self, input: serde_json::Value) -> Result<ToolOutput> {
        let Some(device_id) = input.get("device_id").and_then(|v| v.as_str()) else {
            return Ok(ToolOutput::failure("missing 'device_id'"));
        };
        match self.backend.ui_snapshot(device_id).await {
            Ok(snapshot) => Ok(ToolOutput::success(serde_json::to_value(&snapshot)?)),
            Err(e) => Ok(ToolOutput::failure(format!(
                "failed to capture UI snapshot: {e}"
            ))),
        }
    }
}

/// `mobile_install` — install an app on a device (medium risk, write).
pub struct MobileInstallTool {
    backend: Arc<dyn MobileBackend>,
}

impl MobileInstallTool {
    pub fn new(backend: impl MobileBackend + 'static) -> Self {
        Self {
            backend: Arc::new(backend),
        }
    }
}

#[async_trait]
impl Tool for MobileInstallTool {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: MOBILE_INSTALL_TOOL_NAME.into(),
            description: "Install an application package on a mobile device. Requires device_id and package_path. Medium risk — writes to device.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "device_id": {
                        "type": "string",
                        "description": "The device ID"
                    },
                    "package_path": {
                        "type": "string",
                        "description": "Local path to the package (.apk / .ipa)"
                    }
                },
                "required": ["device_id", "package_path"]
            }),
            required_permissions: PermissionSet::read_only(),
            risk: RiskLevel::Medium,
        }
    }

    async fn invoke(&self, input: serde_json::Value) -> Result<ToolOutput> {
        let device_id = input
            .get("device_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| CoreError::invalid("missing 'device_id'"))?;
        let package_path = input
            .get("package_path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| CoreError::invalid("missing 'package_path'"))?;
        let req = InstallRequest {
            device_id: device_id.to_string(),
            artifact_path: package_path.to_string(),
        };
        match self.backend.install(req).await {
            Ok(()) => Ok(ToolOutput::success(serde_json::json!({
                "status": "installed",
                "device_id": device_id,
                "package_path": package_path,
            }))),
            Err(e) => Ok(ToolOutput::failure(format!("install failed: {e}"))),
        }
    }
}

/// `mobile_launch` — launch an app on a device (medium risk, write).
pub struct MobileLaunchTool {
    backend: Arc<dyn MobileBackend>,
}

impl MobileLaunchTool {
    pub fn new(backend: impl MobileBackend + 'static) -> Self {
        Self {
            backend: Arc::new(backend),
        }
    }
}

#[async_trait]
impl Tool for MobileLaunchTool {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: MOBILE_LAUNCH_TOOL_NAME.into(),
            description: "Launch an application on a mobile device. Requires device_id and package_name. Medium risk.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "device_id": {
                        "type": "string",
                        "description": "The device ID"
                    },
                    "package_name": {
                        "type": "string",
                        "description": "The package / bundle identifier"
                    },
                    "activity": {
                        "type": "string",
                        "description": "Optional activity name (Android)"
                    }
                },
                "required": ["device_id", "package_name"]
            }),
            required_permissions: PermissionSet::read_only(),
            risk: RiskLevel::Medium,
        }
    }

    async fn invoke(&self, input: serde_json::Value) -> Result<ToolOutput> {
        let device_id = input
            .get("device_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| CoreError::invalid("missing 'device_id'"))?;
        let package_name = input
            .get("package_name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| CoreError::invalid("missing 'package_name'"))?;
        let activity = input
            .get("activity")
            .and_then(|v| v.as_str())
            .map(String::from);
        let req = LaunchRequest {
            device_id: device_id.to_string(),
            package: package_name.to_string(),
            activity,
        };
        match self.backend.launch(req).await {
            Ok(()) => Ok(ToolOutput::success(serde_json::json!({
                "status": "launched",
                "device_id": device_id,
                "package_name": package_name,
            }))),
            Err(e) => Ok(ToolOutput::failure(format!("launch failed: {e}"))),
        }
    }
}

/// `mobile_terminate` — stop a running app (medium risk, write).
pub struct MobileTerminateTool {
    backend: Arc<dyn MobileBackend>,
}

impl MobileTerminateTool {
    pub fn new(backend: impl MobileBackend + 'static) -> Self {
        Self {
            backend: Arc::new(backend),
        }
    }
}

#[async_trait]
impl Tool for MobileTerminateTool {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: MOBILE_TERMINATE_TOOL_NAME.into(),
            description: "Terminate a running application on a mobile device. Requires device_id and package_name. Medium risk.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "device_id": {
                        "type": "string",
                        "description": "The device ID"
                    },
                    "package_name": {
                        "type": "string",
                        "description": "The package / bundle identifier"
                    }
                },
                "required": ["device_id", "package_name"]
            }),
            required_permissions: PermissionSet::read_only(),
            risk: RiskLevel::Medium,
        }
    }

    async fn invoke(&self, input: serde_json::Value) -> Result<ToolOutput> {
        let device_id = input
            .get("device_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| CoreError::invalid("missing 'device_id'"))?;
        let package_name = input
            .get("package_name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| CoreError::invalid("missing 'package_name'"))?;
        let target = AppTarget {
            device_id: device_id.to_string(),
            package: package_name.to_string(),
        };
        match self.backend.terminate(target).await {
            Ok(()) => Ok(ToolOutput::success(serde_json::json!({
                "status": "terminated",
                "device_id": device_id,
                "package_name": package_name,
            }))),
            Err(e) => Ok(ToolOutput::failure(format!("terminate failed: {e}"))),
        }
    }
}

/// `mobile_input` — perform an input action on a device (medium risk, write).
pub struct MobileInputTool {
    backend: Arc<dyn MobileBackend>,
}

impl MobileInputTool {
    pub fn new(backend: impl MobileBackend + 'static) -> Self {
        Self {
            backend: Arc::new(backend),
        }
    }
}

#[async_trait]
impl Tool for MobileInputTool {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: MOBILE_INPUT_TOOL_NAME.into(),
            description: "Perform an input action (tap, swipe, text) on a mobile device. Requires device_id and action. Medium risk.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "device_id": {
                        "type": "string",
                        "description": "The device ID"
                    },
                    "action": {
                        "type": "string",
                        "enum": ["tap", "swipe", "text", "key"],
                        "description": "The input action type"
                    },
                    "x": { "type": "integer" },
                    "y": { "type": "integer" },
                    "end_x": { "type": "integer" },
                    "end_y": { "type": "integer" },
                    "text": { "type": "string" },
                    "key": { "type": "string" }
                },
                "required": ["device_id", "action"]
            }),
            required_permissions: PermissionSet::read_only(),
            risk: RiskLevel::Medium,
        }
    }

    async fn invoke(&self, input: serde_json::Value) -> Result<ToolOutput> {
        let device_id = input
            .get("device_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| CoreError::invalid("missing 'device_id'"))?;
        let action_str = input
            .get("action")
            .and_then(|v| v.as_str())
            .ok_or_else(|| CoreError::invalid("missing 'action'"))?;
        let action = match action_str {
            "tap" => {
                let x = input
                    .get("x")
                    .and_then(|v| v.as_u64())
                    .ok_or_else(|| CoreError::invalid("tap requires 'x'"))?
                    as u32;
                let y = input
                    .get("y")
                    .and_then(|v| v.as_u64())
                    .ok_or_else(|| CoreError::invalid("tap requires 'y'"))?
                    as u32;
                InputAction::Tap { x, y }
            }
            "swipe" => {
                let x1 = input
                    .get("x")
                    .and_then(|v| v.as_u64())
                    .ok_or_else(|| CoreError::invalid("swipe requires 'x'"))?
                    as u32;
                let y1 = input
                    .get("y")
                    .and_then(|v| v.as_u64())
                    .ok_or_else(|| CoreError::invalid("swipe requires 'y'"))?
                    as u32;
                let x2 = input
                    .get("end_x")
                    .and_then(|v| v.as_u64())
                    .ok_or_else(|| CoreError::invalid("swipe requires 'end_x'"))?
                    as u32;
                let y2 = input
                    .get("end_y")
                    .and_then(|v| v.as_u64())
                    .ok_or_else(|| CoreError::invalid("swipe requires 'end_y'"))?
                    as u32;
                InputAction::Swipe {
                    x1,
                    y1,
                    x2,
                    y2,
                    duration_ms: 300,
                }
            }
            "text" => {
                let text = input
                    .get("text")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| CoreError::invalid("text action requires 'text'"))?;
                InputAction::InputText {
                    text: text.to_string(),
                }
            }
            "key" => {
                let key = input
                    .get("key")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| CoreError::invalid("key action requires 'key'"))?;
                if key == "back" {
                    InputAction::PressBack
                } else {
                    return Ok(ToolOutput::failure(format!("unsupported key: {key}")));
                }
            }
            other => {
                return Ok(ToolOutput::failure(format!("unknown action: {other}")));
            }
        };
        let snapshot_id = input
            .get("snapshot_id")
            .and_then(|v| v.as_str())
            .map(String::from);
        let req = InputRequest {
            device_id: device_id.to_string(),
            snapshot_id,
            action,
        };
        match self.backend.input(req).await {
            Ok(result) => Ok(ToolOutput::success(serde_json::to_value(&result)?)),
            Err(e) => Ok(ToolOutput::failure(format!("input failed: {e}"))),
        }
    }
}

/// Create all mobile tools with the given backend.
pub fn mobile_tools(backend: impl MobileBackend + 'static) -> Vec<Arc<dyn Tool>> {
    let b: Arc<dyn MobileBackend> = Arc::new(backend);
    vec![
        Arc::new(MobileListDevicesTool { backend: b.clone() }),
        Arc::new(MobileDeviceInfoTool { backend: b.clone() }),
        Arc::new(MobileScreenshotTool { backend: b.clone() }),
        Arc::new(MobileUiSnapshotTool { backend: b.clone() }),
        Arc::new(MobileInstallTool { backend: b.clone() }),
        Arc::new(MobileLaunchTool { backend: b.clone() }),
        Arc::new(MobileTerminateTool { backend: b.clone() }),
        Arc::new(MobileInputTool { backend: b }),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn unavailable_backend_list_returns_empty() {
        let backend = UnavailableMobileBackend;
        let devices = backend.list_devices().await.unwrap();
        assert!(devices.is_empty());
    }

    #[tokio::test]
    async fn unavailable_backend_device_info_fails() {
        let backend = UnavailableMobileBackend;
        let result = backend.device_info("dev-1").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn unavailable_backend_write_ops_fail() {
        let backend = UnavailableMobileBackend;
        assert!(backend
            .install(InstallRequest {
                device_id: "d".into(),
                artifact_path: "/tmp/a.apk".into(),
            })
            .await
            .is_err());
        assert!(backend
            .launch(LaunchRequest {
                device_id: "d".into(),
                package: "com.example".into(),
                activity: None,
            })
            .await
            .is_err());
        assert!(backend
            .terminate(AppTarget {
                device_id: "d".into(),
                package: "com.example".into(),
            })
            .await
            .is_err());
        assert!(backend
            .input(InputRequest {
                device_id: "d".into(),
                snapshot_id: None,
                action: InputAction::Tap { x: 100, y: 200 },
            })
            .await
            .is_err());
    }

    #[test]
    fn tool_descriptors_have_correct_names() {
        let backend = UnavailableMobileBackend;
        let list = MobileListDevicesTool::new(backend);
        assert_eq!(list.descriptor().name, MOBILE_LIST_DEVICES_TOOL_NAME);
    }

    #[test]
    fn write_tool_descriptors_have_correct_names() {
        let b = UnavailableMobileBackend;
        assert_eq!(
            MobileInstallTool::new(b).descriptor().name,
            MOBILE_INSTALL_TOOL_NAME
        );
        assert_eq!(
            MobileLaunchTool::new(b).descriptor().name,
            MOBILE_LAUNCH_TOOL_NAME
        );
        assert_eq!(
            MobileTerminateTool::new(b).descriptor().name,
            MOBILE_TERMINATE_TOOL_NAME
        );
        assert_eq!(
            MobileInputTool::new(b).descriptor().name,
            MOBILE_INPUT_TOOL_NAME
        );
    }

    #[test]
    fn mobile_tools_creates_eight_tools() {
        let tools = mobile_tools(UnavailableMobileBackend);
        assert_eq!(tools.len(), 8);
    }
}
