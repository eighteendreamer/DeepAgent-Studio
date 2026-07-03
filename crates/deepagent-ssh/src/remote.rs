use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum RemoteVerifyMode {
    None,
    Size,
    #[default]
    Sha256,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RemoteProbeResult {
    pub os: Option<String>,
    pub distro: Option<String>,
    pub distro_version: Option<String>,
    pub arch: Option<String>,
    pub shell: Option<String>,
    pub user: Option<String>,
    pub cwd: Option<String>,
    pub path: Option<String>,
    #[serde(default)]
    pub package_managers: Vec<String>,
    #[serde(default)]
    pub commands: HashMap<String, bool>,
    #[serde(default)]
    pub runtimes: HashMap<String, String>,
    pub probed_at_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemotePushFileRequest {
    pub local_path: String,
    pub remote_path: String,
    #[serde(default = "default_true")]
    pub create_parent: bool,
    #[serde(default)]
    pub overwrite: bool,
    #[serde(default)]
    pub verify_mode: RemoteVerifyMode,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemotePushFileResult {
    pub ok: bool,
    pub remote_path: String,
    pub bytes: u64,
    pub local_sha256: Option<String>,
    pub remote_sha256: Option<String>,
    pub integrity_verified: bool,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteBundleRequest {
    pub local_path: String,
    pub remote_path: String,
    #[serde(default = "default_true")]
    pub create_parent: bool,
    #[serde(default)]
    pub overwrite: bool,
    #[serde(default)]
    pub verify_mode: RemoteVerifyMode,
    #[serde(default = "default_true")]
    pub remove_archive_after_extract: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteBundleResult {
    pub ok: bool,
    pub remote_path: String,
    pub remote_archive_path: String,
    pub remote_manifest_path: String,
    pub files: u64,
    pub bytes: u64,
    pub local_archive_sha256: Option<String>,
    pub remote_archive_sha256: Option<String>,
    pub integrity_verified: bool,
    pub extract_verified: bool,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteManifestEntry {
    pub path: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteBundleManifest {
    pub version: u32,
    pub root_name: String,
    #[serde(default)]
    pub entries: Vec<RemoteManifestEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteRuntimeRequirement {
    pub name: String,
    #[serde(default)]
    pub version: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RemoteRequireRequest {
    #[serde(default)]
    pub commands: Vec<String>,
    #[serde(default)]
    pub runtimes: Vec<RemoteRuntimeRequirement>,
    #[serde(default)]
    pub archives: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteRequireResult {
    pub package_manager: Option<String>,
    #[serde(default)]
    pub package_managers: Vec<String>,
    #[serde(default)]
    pub missing_commands: Vec<String>,
    #[serde(default)]
    pub missing_runtimes: Vec<String>,
    #[serde(default)]
    pub missing_archive_tools: Vec<String>,
    #[serde(default)]
    pub install_commands: Vec<String>,
    pub can_install: bool,
    pub probe: RemoteProbeResult,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RemoteInstallRequest {
    #[serde(default)]
    pub package_manager: Option<String>,
    #[serde(default)]
    pub commands: Vec<String>,
    #[serde(default)]
    pub runtimes: Vec<RemoteRuntimeRequirement>,
    #[serde(default)]
    pub packages: Vec<String>,
    #[serde(default = "default_true")]
    pub update_index: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteInstallResult {
    pub ok: bool,
    pub package_manager: Option<String>,
    #[serde(default)]
    pub commands_run: Vec<String>,
    pub stdout: String,
    pub stderr: String,
    #[serde(default)]
    pub installed_packages: Vec<String>,
    pub probe: Option<RemoteProbeResult>,
}

const fn default_true() -> bool {
    true
}
