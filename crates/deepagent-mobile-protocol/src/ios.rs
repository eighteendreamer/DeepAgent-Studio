use serde::{Deserialize, Serialize};

/// A Simulator device as reported by `xcrun simctl list devices --json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SimDevice {
    pub udid: String,
    pub name: String,
    pub state: SimDeviceState,
    pub is_available: bool,
    pub runtime_id: Option<String>,
    pub device_type_id: Option<String>,
}

/// Simulator device state from simctl.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SimDeviceState {
    Booted,
    Shutdown,
    Creating,
    Unknown,
}

impl SimDevice {
    /// Map simctl state to the unified `DeviceState`.
    pub fn to_unified_state(&self) -> deepagent_mobile_core::DeviceState {
        match self.state {
            SimDeviceState::Booted => deepagent_mobile_core::DeviceState::Ready,
            SimDeviceState::Shutdown => deepagent_mobile_core::DeviceState::Disconnected,
            SimDeviceState::Creating => deepagent_mobile_core::DeviceState::Booting,
            SimDeviceState::Unknown => deepagent_mobile_core::DeviceState::Error,
        }
    }
}

/// A Simulator runtime as reported by `xcrun simctl list runtimes --json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SimRuntime {
    pub identifier: String,
    pub name: String,
    pub version: String,
    pub is_available: bool,
    pub platform: Option<String>,
}

/// Container for `xcrun simctl list --json` output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SimctlListOutput {
    pub devices: Vec<SimDevice>,
    pub runtimes: Vec<SimRuntime>,
}

/// Classified error kinds for iOS toolchain operations.
///
/// Per §30: "无 Xcode、未授权设备和签名错误均能分类呈现".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IosErrorKind {
    /// Xcode or command-line tools not installed.
    XcodeNotInstalled,
    /// simctl binary not found in PATH.
    SimctlNotFound,
    /// devicectl binary not found in PATH.
    DevicectlNotFound,
    /// Target device not found by UDID or name.
    DeviceNotFound,
    /// Simulator failed to boot within timeout.
    BootFailed,
    /// App signing or provisioning profile error.
    SigningError,
    /// Device pairing failed (physical device).
    PairingFailed,
    /// Device is locked or trust not established.
    DeviceLocked,
    /// Operation not supported on this runtime or device.
    NotSupported,
    /// Remote Mac agent not reachable.
    RemoteMacUnavailable,
}

/// Structured error from devicectl or simctl operations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IosToolError {
    pub kind: IosErrorKind,
    pub message: String,
    pub suggestion: Option<String>,
}

impl IosToolError {
    pub fn new(kind: IosErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            suggestion: None,
        }
    }

    pub fn with_suggestion(mut self, suggestion: impl Into<String>) -> Self {
        self.suggestion = Some(suggestion.into());
        self
    }
}

impl std::fmt::Display for IosToolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}: {}", self.kind, self.message)?;
        if let Some(ref s) = self.suggestion {
            write!(f, " (suggestion: {s})")?;
        }
        Ok(())
    }
}

impl std::error::Error for IosToolError {}

/// Classify a raw error message from simctl/devicectl into a structured error.
pub fn classify_ios_error(message: &str) -> IosToolError {
    let lower = message.to_lowercase();

    if lower.contains("not installed") || lower.contains("xcode") && lower.contains("not found") {
        return IosToolError::new(IosErrorKind::XcodeNotInstalled, message)
            .with_suggestion("Install Xcode from the Mac App Store");
    }
    if lower.contains("simctl") && lower.contains("not found") || lower.contains("no such file") {
        return IosToolError::new(IosErrorKind::SimctlNotFound, message)
            .with_suggestion("Ensure Xcode command-line tools are installed");
    }
    if lower.contains("devicectl") && lower.contains("not found") {
        return IosToolError::new(IosErrorKind::DevicectlNotFound, message)
            .with_suggestion("Upgrade to Xcode 15+ for devicectl support");
    }
    if lower.contains("no devices found") || lower.contains("unable to find device") {
        return IosToolError::new(IosErrorKind::DeviceNotFound, message);
    }
    if lower.contains("failed to boot") || lower.contains("boot timeout") {
        return IosToolError::new(IosErrorKind::BootFailed, message)
            .with_suggestion("Try erasing the simulator: simctl erase <udid>");
    }
    if lower.contains("signing") || lower.contains("provisioning") || lower.contains("codesign") {
        return IosToolError::new(IosErrorKind::SigningError, message)
            .with_suggestion("Check provisioning profile and signing certificate");
    }
    if lower.contains("pairing") || lower.contains("pair") && lower.contains("failed") {
        return IosToolError::new(IosErrorKind::PairingFailed, message)
            .with_suggestion("Trust this computer on the device and retry");
    }
    if lower.contains("locked") || lower.contains("passcode") {
        return IosToolError::new(IosErrorKind::DeviceLocked, message)
            .with_suggestion("Unlock the device and try again");
    }

    IosToolError::new(IosErrorKind::NotSupported, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sim_device_serde() {
        let device = SimDevice {
            udid: "ABCD-1234".into(),
            name: "iPhone 15".into(),
            state: SimDeviceState::Booted,
            is_available: true,
            runtime_id: Some("com.apple.CoreSimulator.SimRuntime.iOS-17-0".into()),
            device_type_id: Some("com.apple.CoreSimulator.SimDeviceType.iPhone-15".into()),
        };
        let json = serde_json::to_string(&device).unwrap();
        let back: SimDevice = serde_json::from_str(&json).unwrap();
        assert_eq!(device, back);
    }

    #[test]
    fn sim_device_state_mapping() {
        assert_eq!(
            SimDevice {
                udid: "x".into(),
                name: "x".into(),
                state: SimDeviceState::Booted,
                is_available: true,
                runtime_id: None,
                device_type_id: None,
            }
            .to_unified_state(),
            deepagent_mobile_core::DeviceState::Ready
        );
        assert_eq!(
            SimDevice {
                udid: "x".into(),
                name: "x".into(),
                state: SimDeviceState::Shutdown,
                is_available: true,
                runtime_id: None,
                device_type_id: None,
            }
            .to_unified_state(),
            deepagent_mobile_core::DeviceState::Disconnected
        );
    }

    #[test]
    fn sim_runtime_serde() {
        let rt = SimRuntime {
            identifier: "com.apple.CoreSimulator.SimRuntime.iOS-17-0".into(),
            name: "iOS 17.0".into(),
            version: "17.0".into(),
            is_available: true,
            platform: Some("iOS".into()),
        };
        let json = serde_json::to_string(&rt).unwrap();
        let back: SimRuntime = serde_json::from_str(&json).unwrap();
        assert_eq!(rt, back);
    }

    #[test]
    fn simctl_list_output_serde() {
        let output = SimctlListOutput {
            devices: vec![SimDevice {
                udid: "abc".into(),
                name: "iPhone 15".into(),
                state: SimDeviceState::Shutdown,
                is_available: true,
                runtime_id: None,
                device_type_id: None,
            }],
            runtimes: vec![],
        };
        let json = serde_json::to_string(&output).unwrap();
        assert!(json.contains("iPhone 15"));
        let back: SimctlListOutput = serde_json::from_str(&json).unwrap();
        assert_eq!(output, back);
    }

    #[test]
    fn classify_xcode_not_installed() {
        let err = classify_ios_error("Xcode not installed");
        assert_eq!(err.kind, IosErrorKind::XcodeNotInstalled);
        assert!(err.suggestion.is_some());
    }

    #[test]
    fn classify_simctl_not_found() {
        let err = classify_ios_error("simctl: command not found");
        assert_eq!(err.kind, IosErrorKind::SimctlNotFound);
    }

    #[test]
    fn classify_device_not_found() {
        let err = classify_ios_error("unable to find device with UDID ABCD");
        assert_eq!(err.kind, IosErrorKind::DeviceNotFound);
    }

    #[test]
    fn classify_boot_failed() {
        let err = classify_ios_error("Failed to boot simulator ABCD");
        assert_eq!(err.kind, IosErrorKind::BootFailed);
        assert!(err.suggestion.is_some());
    }

    #[test]
    fn classify_signing_error() {
        let err = classify_ios_error("codesign failed: no signing certificate");
        assert_eq!(err.kind, IosErrorKind::SigningError);
    }

    #[test]
    fn classify_pairing_failed() {
        let err = classify_ios_error("device pairing failed");
        assert_eq!(err.kind, IosErrorKind::PairingFailed);
    }

    #[test]
    fn classify_device_locked() {
        let err = classify_ios_error("device is locked, enter passcode");
        assert_eq!(err.kind, IosErrorKind::DeviceLocked);
    }

    #[test]
    fn classify_unknown_error() {
        let err = classify_ios_error("something completely unexpected happened");
        assert_eq!(err.kind, IosErrorKind::NotSupported);
    }

    #[test]
    fn ios_tool_error_display() {
        let err = IosToolError::new(IosErrorKind::BootFailed, "timeout")
            .with_suggestion("erase simulator");
        let display = format!("{err}");
        assert!(display.contains("BootFailed"));
        assert!(display.contains("timeout"));
        assert!(display.contains("erase simulator"));
    }

    #[test]
    fn ios_error_kind_serde() {
        let kind = IosErrorKind::RemoteMacUnavailable;
        let json = serde_json::to_string(&kind).unwrap();
        assert_eq!(json, "\"remote_mac_unavailable\"");
        let back: IosErrorKind = serde_json::from_str(&json).unwrap();
        assert_eq!(kind, back);
    }
}
