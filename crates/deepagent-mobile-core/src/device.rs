use serde::{Deserialize, Serialize};

/// Target mobile operating system.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MobilePlatform {
    Android,
    Ios,
}

/// Physical device vs emulator/simulator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceKind {
    Physical,
    Emulator,
    Simulator,
}

/// How the host reaches the device.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceConnection {
    Usb,
    Local,
    Remote { host_id: String },
}

/// Lifecycle state of a device as seen by the runtime.
///
/// State transitions are centralized in `deepagent-mobile-runtime`; no other
/// crate may infer state from raw tool output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceState {
    Disconnected,
    Connecting,
    Ready,
    Booting,
    Busy,
    Unauthorized,
    Offline,
    Error,
}

/// Declared capabilities of a device. Determines which operations the runtime
/// will accept for this device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceCapabilities {
    pub screenshot: bool,
    pub ui_tree: bool,
    pub input: bool,
    pub logs: bool,
    pub install: bool,
    pub network_inspection: bool,
}

/// A discovered mobile device.
///
/// The `id` field is always generated or validated by the backend. Raw serial
/// numbers, UDIDs and remote host IDs must **not** be used directly as `id`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MobileDevice {
    pub id: String,
    pub name: String,
    pub platform: MobilePlatform,
    pub kind: DeviceKind,
    pub connection: DeviceConnection,
    pub state: DeviceState,
    pub os_version: Option<String>,
    pub capabilities: DeviceCapabilities,
}

/// Probe result for a mobile backend (Android SDK, Xcode, etc.).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackendStatus {
    pub platform: MobilePlatform,
    pub available: bool,
    pub toolchain_version: Option<String>,
    pub tool_paths: Vec<ToolPath>,
    pub diagnostics: Vec<String>,
}

/// A resolved external tool path with optional version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolPath {
    pub name: String,
    pub path: String,
    pub version: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_round_trip_serde() {
        let device = MobileDevice {
            id: "android-usb-ABC123".into(),
            name: "Pixel 7".into(),
            platform: MobilePlatform::Android,
            kind: DeviceKind::Physical,
            connection: DeviceConnection::Usb,
            state: DeviceState::Ready,
            os_version: Some("14".into()),
            capabilities: DeviceCapabilities {
                screenshot: true,
                ui_tree: true,
                input: true,
                logs: true,
                install: true,
                network_inspection: false,
            },
        };
        let json = serde_json::to_string(&device).unwrap();
        let back: MobileDevice = serde_json::from_str(&json).unwrap();
        assert_eq!(device, back);
    }

    #[test]
    fn backend_status_unavailable() {
        let status = BackendStatus {
            platform: MobilePlatform::Ios,
            available: false,
            toolchain_version: None,
            tool_paths: vec![],
            diagnostics: vec!["Xcode not found".into()],
        };
        let json = serde_json::to_string(&status).unwrap();
        assert!(json.contains("Xcode not found"));
    }

    #[test]
    fn device_state_all_variants_serialize() {
        let states = [
            DeviceState::Disconnected,
            DeviceState::Connecting,
            DeviceState::Ready,
            DeviceState::Booting,
            DeviceState::Busy,
            DeviceState::Unauthorized,
            DeviceState::Offline,
            DeviceState::Error,
        ];
        for s in states {
            let json = serde_json::to_string(&s).unwrap();
            let back: DeviceState = serde_json::from_str(&json).unwrap();
            assert_eq!(s, back);
        }
    }
}
