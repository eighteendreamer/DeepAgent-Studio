//! Installation orchestration only; NSIS remains the package/update/uninstall authority.
#[cfg(windows)]
pub mod engine;
pub mod options;
pub mod payload;
#[cfg(windows)]
pub mod platform;
#[cfg(all(windows, feature = "gui"))]
pub mod ui;

use serde::{Deserialize, Serialize};

pub const PRODUCT: &str = "DeepAgent Studio";
pub const MAIN_EXE: &str = "deepagent-desktop.exe";
pub const CLI_EXE: &str = "deepagent.exe";
pub const OPTIONS_EXE: &str = "deepagent-install-options.exe";

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct InstallOptions {
    pub directory: String,
    pub desktop_shortcut: bool,
    pub start_menu_shortcut: bool,
    pub add_to_path: bool,
    pub autostart: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct Progress {
    pub phase: &'static str,
    pub percent: u8,
    pub written_bytes: u64,
    pub total_bytes: u64,
}

/// File sizes are an observation, not NSIS progress. Never report completion
/// from a timer or from old files left by a previous install.
pub fn observed_percent(written: u64, total: u64) -> u8 {
    if total == 0 {
        return 0;
    }
    ((u128::from(written) * 95 / u128::from(total)).min(95)) as u8
}

#[cfg(test)]
mod tests {
    #[test]
    fn bytes_never_claim_installation_success() {
        assert_eq!(super::observed_percent(0, 0), 0);
        assert_eq!(super::observed_percent(50, 100), 47);
        assert_eq!(super::observed_percent(u64::MAX, 1), 95);
        assert_eq!(super::observed_percent(100, 100), 95);
    }
}
