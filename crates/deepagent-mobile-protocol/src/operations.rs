use serde::{Deserialize, Serialize};

/// A structured mobile operation request.
///
/// Every operation carries an `operation_id` for cancellation, auditing and
/// event correlation. Operations are **never** free-form shell commands.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MobileOperation {
    pub operation_id: String,
    pub device_id: String,
    pub deadline_ms: u64,
    pub kind: MobileOperationKind,
}

/// The kind of mobile operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MobileOperationKind {
    ListDevices,
    DeviceInfo,
    Screenshot,
    UiSnapshot,
    Install(InstallRequest),
    Uninstall(AppTarget),
    Launch(LaunchRequest),
    Terminate(AppTarget),
    Input(InputRequest),
    ReadLogs(LogRequest),
    ListAvds,
    StartEmulator(StartEmulatorRequest),
    StopEmulator(StopEmulatorRequest),
    FindNodes(FindNodeRequest),
    StartNetworkCapture { device_id: String },
    StopNetworkCapture { device_id: String },
    GetNetworkRecords { device_id: String },
}

/// Target an installed application by package name (Android) or bundle ID
/// (iOS).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppTarget {
    pub device_id: String,
    pub package: String,
}

/// Request to install an application.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallRequest {
    pub device_id: String,
    pub artifact_path: String,
}

/// Request to launch an application.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaunchRequest {
    pub device_id: String,
    pub package: String,
    pub activity: Option<String>,
}

/// Structured input action. No free-form shell.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputAction {
    Tap {
        x: u32,
        y: u32,
    },
    LongPress {
        x: u32,
        y: u32,
        duration_ms: u64,
    },
    Swipe {
        x1: u32,
        y1: u32,
        x2: u32,
        y2: u32,
        duration_ms: u64,
    },
    InputText {
        text: String,
    },
    PressBack,
}

/// Input request wrapping an action with snapshot correlation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InputRequest {
    pub device_id: String,
    pub snapshot_id: Option<String>,
    pub action: InputAction,
}

/// Result of an input operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InputResult {
    pub accepted: bool,
}

/// Request to read device logs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogRequest {
    pub device_id: String,
    pub max_lines: u32,
    pub since_ms: Option<u64>,
}

/// A single log record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogRecord {
    pub timestamp_ms: u64,
    pub level: String,
    pub tag: Option<String>,
    pub message: String,
}

/// A page of log records.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogPage {
    pub device_id: String,
    pub records: Vec<LogRecord>,
    pub truncated: bool,
}

/// Information about an Android Virtual Device (AVD).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AvdInfo {
    /// AVD name (used with `emulator -avd <name>`).
    pub name: String,
    /// AVD directory path.
    pub path: Option<String>,
    /// Target API level or Android version.
    pub target: Option<String>,
    /// Whether the AVD is currently running.
    pub running: bool,
    /// ADB serial if running (e.g., "emulator-5554").
    pub serial: Option<String>,
}

/// Request to start an Android Emulator.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartEmulatorRequest {
    /// AVD name to start.
    pub avd_name: String,
    /// Additional emulator arguments (e.g., "-no-snapshot", "-port 5554").
    pub args: Vec<String>,
    /// Maximum time to wait for the emulator to boot (milliseconds).
    pub boot_timeout_ms: u64,
}

/// Request to stop a running Android Emulator.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StopEmulatorRequest {
    /// ADB serial of the running emulator (e.g., "emulator-5554").
    pub serial: String,
}

/// Filter criteria for finding nodes in a UI snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeFilter {
    /// Match nodes containing this text (case-insensitive substring).
    pub text: Option<String>,
    /// Match nodes with this resource-id (exact match).
    pub resource_id: Option<String>,
    /// Match nodes with this content-desc (case-insensitive substring).
    pub content_desc: Option<String>,
    /// Match nodes with this role.
    pub role: Option<super::UiRole>,
}

/// Request to find nodes in a UI snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FindNodeRequest {
    pub device_id: String,
    pub snapshot_id: String,
    pub filter: NodeFilter,
}

/// Result of a find-node operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FindNodeResult {
    pub snapshot_id: String,
    pub matches: Vec<super::UiNode>,
    pub total_nodes_searched: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operation_kind_round_trip() {
        let op = MobileOperation {
            operation_id: "op-1".into(),
            device_id: "dev-1".into(),
            deadline_ms: 30_000,
            kind: MobileOperationKind::Screenshot,
        };
        let json = serde_json::to_string(&op).unwrap();
        let back: MobileOperation = serde_json::from_str(&json).unwrap();
        assert_eq!(op, back);
    }

    #[test]
    fn input_action_tap_serde() {
        let action = InputAction::Tap { x: 100, y: 200 };
        let json = serde_json::to_string(&action).unwrap();
        assert!(json.contains("\"tap\""));
        let back: InputAction = serde_json::from_str(&json).unwrap();
        assert_eq!(action, back);
    }

    #[test]
    fn launch_request_serde() {
        let req = LaunchRequest {
            device_id: "dev-1".into(),
            package: "com.example.app".into(),
            activity: Some(".MainActivity".into()),
        };
        let json = serde_json::to_string(&req).unwrap();
        let back: LaunchRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(req, back);
    }
}
