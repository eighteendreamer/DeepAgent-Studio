use deepagent_mobile_core::{BackendStatus, MobilePlatform};
use deepagent_mobile_protocol::IosToolError;

/// Resolves iOS toolchain paths (simctl, devicectl, xcrun).
///
/// On non-macOS platforms, all tools are reported as unavailable.
/// On macOS, checks for tool existence without executing them.
#[derive(Debug, Clone)]
pub struct IosToolResolver {
    platform_override: Option<BackendStatus>,
}

impl Default for IosToolResolver {
    fn default() -> Self {
        Self::new()
    }
}

impl IosToolResolver {
    pub fn new() -> Self {
        Self {
            platform_override: None,
        }
    }

    /// Create a resolver with a forced status (for testing or when the
    /// platform is known to be unavailable).
    pub fn with_forced_status(status: BackendStatus) -> Self {
        Self {
            platform_override: Some(status),
        }
    }

    /// Probe the iOS toolchain and return a status report.
    pub fn probe(&self) -> BackendStatus {
        if let Some(ref override_status) = self.platform_override {
            return override_status.clone();
        }

        if !cfg!(target_os = "macos") {
            return BackendStatus {
                platform: MobilePlatform::Ios,
                available: false,
                toolchain_version: None,
                tool_paths: vec![],
                diagnostics: vec![
                    "iOS toolchain requires macOS".into(),
                    "Use Remote Mac runtime to access iOS devices from Windows/Linux".into(),
                ],
            };
        }

        self.probe_macos()
    }

    #[cfg(target_os = "macos")]
    fn probe_macos(&self) -> BackendStatus {
        let mut tool_paths = Vec::new();
        let mut diagnostics = Vec::new();
        let mut available = true;

        if let Some(path) = find_tool("xcrun") {
            tool_paths.push(ToolPath {
                name: "xcrun".into(),
                path,
                version: None,
            });
        } else {
            available = false;
            diagnostics.push("xcrun not found in PATH".into());
        }

        if let Some(path) = find_tool("simctl") {
            tool_paths.push(ToolPath {
                name: "simctl".into(),
                path,
                version: None,
            });
        } else {
            diagnostics.push("simctl not found (requires Xcode)".into());
        }

        if let Some(path) = find_tool("devicectl") {
            tool_paths.push(ToolPath {
                name: "devicectl".into(),
                path,
                version: None,
            });
        } else {
            diagnostics.push("devicectl not found (requires Xcode 15+)".into());
        }

        BackendStatus {
            platform: MobilePlatform::Ios,
            available,
            toolchain_version: None,
            tool_paths,
            diagnostics,
        }
    }

    #[cfg(not(target_os = "macos"))]
    fn probe_macos(&self) -> BackendStatus {
        BackendStatus {
            platform: MobilePlatform::Ios,
            available: false,
            toolchain_version: None,
            tool_paths: vec![],
            diagnostics: vec!["iOS toolchain requires macOS".into()],
        }
    }

    /// Classify a tool error from the iOS backend.
    pub fn classify_error(message: &str) -> IosToolError {
        deepagent_mobile_protocol::classify_ios_error(message)
    }
}

#[cfg(target_os = "macos")]
fn find_tool(name: &str) -> Option<String> {
    use std::process::Command;
    let output = Command::new("which").arg(name).output().ok()?;
    if output.status.success() {
        let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if path.is_empty() {
            None
        } else {
            Some(path)
        }
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use deepagent_mobile_core::ToolPath;
    use deepagent_mobile_protocol::IosErrorKind;

    #[test]
    fn non_macos_reports_unavailable() {
        let resolver = IosToolResolver::new();
        let status = resolver.probe();
        assert_eq!(status.platform, MobilePlatform::Ios);

        if !cfg!(target_os = "macos") {
            assert!(!status.available);
            assert!(status.diagnostics.iter().any(|d| d.contains("macOS")));
        }
    }

    #[test]
    fn forced_status_overrides_probe() {
        let forced = BackendStatus {
            platform: MobilePlatform::Ios,
            available: true,
            toolchain_version: Some("Xcode 15.0".into()),
            tool_paths: vec![ToolPath {
                name: "simctl".into(),
                path: "/usr/bin/simctl".into(),
                version: Some("15.0".into()),
            }],
            diagnostics: vec![],
        };
        let resolver = IosToolResolver::with_forced_status(forced.clone());
        let status = resolver.probe();
        assert_eq!(status, forced);
    }

    #[test]
    fn classify_error_delegates_to_protocol() {
        let err = IosToolResolver::classify_error("Xcode not installed");
        assert_eq!(err.kind, IosErrorKind::XcodeNotInstalled);
    }
}
