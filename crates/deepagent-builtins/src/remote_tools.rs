//! Remote SSH-backed environment probe / transfer / install tools.
#![allow(missing_docs)]

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use deepagent_core::error::Result;
use deepagent_tools::permission::{PermissionSet, RiskLevel};
use deepagent_tools::{Tool, ToolDescriptor, ToolOutput};

pub const REMOTE_PROBE_TOOL_NAME: &str = "remote_probe";
pub const REMOTE_PUSH_FILE_TOOL_NAME: &str = "remote_push_file";
pub const REMOTE_PUSH_BUNDLE_TOOL_NAME: &str = "remote_push_bundle";
pub const REMOTE_REQUIRE_TOOL_NAME: &str = "remote_require";
pub const REMOTE_INSTALL_TOOL_NAME: &str = "remote_install";

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RemoteProbeArgs {
    #[serde(default)]
    pub force_refresh: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RemotePushFileArgs {
    pub local_path: String,
    pub remote_path: String,
    #[serde(default = "default_true")]
    pub create_parent: bool,
    #[serde(default)]
    pub overwrite: bool,
    #[serde(default = "default_verify_mode")]
    pub verify_mode: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RemotePushBundleArgs {
    pub local_path: String,
    pub remote_path: String,
    #[serde(default = "default_true")]
    pub create_parent: bool,
    #[serde(default)]
    pub overwrite: bool,
    #[serde(default = "default_verify_mode")]
    pub verify_mode: String,
    #[serde(default = "default_true")]
    pub remove_archive_after_extract: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RemoteRuntimeRequirement {
    pub name: String,
    #[serde(default)]
    pub version: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RemoteRequireArgs {
    #[serde(default)]
    pub commands: Vec<String>,
    #[serde(default)]
    pub runtimes: Vec<RemoteRuntimeRequirement>,
    #[serde(default)]
    pub archives: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RemoteInstallArgs {
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

#[async_trait]
pub trait RemoteOpsBackend: Send + Sync {
    async fn probe(&self, args: RemoteProbeArgs) -> Result<serde_json::Value>;
    async fn push_file(&self, args: RemotePushFileArgs) -> Result<serde_json::Value>;
    async fn push_bundle(&self, args: RemotePushBundleArgs) -> Result<serde_json::Value>;
    async fn require(&self, args: RemoteRequireArgs) -> Result<serde_json::Value>;
    async fn install(&self, args: RemoteInstallArgs) -> Result<serde_json::Value>;
}

#[async_trait]
impl<T> RemoteOpsBackend for Arc<T>
where
    T: RemoteOpsBackend + ?Sized,
{
    async fn probe(&self, args: RemoteProbeArgs) -> Result<serde_json::Value> {
        (**self).probe(args).await
    }

    async fn push_file(&self, args: RemotePushFileArgs) -> Result<serde_json::Value> {
        (**self).push_file(args).await
    }

    async fn push_bundle(&self, args: RemotePushBundleArgs) -> Result<serde_json::Value> {
        (**self).push_bundle(args).await
    }

    async fn require(&self, args: RemoteRequireArgs) -> Result<serde_json::Value> {
        (**self).require(args).await
    }

    async fn install(&self, args: RemoteInstallArgs) -> Result<serde_json::Value> {
        (**self).install(args).await
    }
}

pub struct RemoteProbeTool<B: RemoteOpsBackend> {
    backend: B,
}

impl<B: RemoteOpsBackend> RemoteProbeTool<B> {
    pub fn new(backend: B) -> Self {
        Self { backend }
    }
}

#[async_trait]
impl<B: RemoteOpsBackend> Tool for RemoteProbeTool<B> {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: REMOTE_PROBE_TOOL_NAME.into(),
            description: "Probe the active remote SSH host and return its OS, package managers, command availability, and runtime versions. Use this before assuming any remote command or environment exists.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "force_refresh": { "type": "boolean", "description": "Ignore cached probe data and re-query the remote host." }
                }
            }),
            risk: RiskLevel::Safe,
            required_permissions: PermissionSet::read_only(),
        }
    }

    async fn invoke(&self, arguments: serde_json::Value) -> Result<ToolOutput> {
        let args: RemoteProbeArgs =
            serde_json::from_value(arguments).unwrap_or_else(|_| RemoteProbeArgs::default());
        Ok(ToolOutput::success(self.backend.probe(args).await?))
    }
}

pub struct RemotePushFileTool<B: RemoteOpsBackend> {
    backend: B,
}

impl<B: RemoteOpsBackend> RemotePushFileTool<B> {
    pub fn new(backend: B) -> Self {
        Self { backend }
    }
}

#[async_trait]
impl<B: RemoteOpsBackend> Tool for RemotePushFileTool<B> {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: REMOTE_PUSH_FILE_TOOL_NAME.into(),
            description: "Upload one local file to the active remote SSH host over SFTP, then verify transfer integrity with size or sha256 checks.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "local_path": { "type": "string" },
                    "remote_path": { "type": "string" },
                    "create_parent": { "type": "boolean" },
                    "overwrite": { "type": "boolean" },
                    "verify_mode": { "type": "string", "enum": ["none", "size", "sha256"] }
                },
                "required": ["local_path", "remote_path"]
            }),
            risk: RiskLevel::High,
            required_permissions: PermissionSet::developer(),
        }
    }

    async fn invoke(&self, arguments: serde_json::Value) -> Result<ToolOutput> {
        let args: RemotePushFileArgs = match serde_json::from_value(arguments) {
            Ok(value) => value,
            Err(err) => return Ok(ToolOutput::failure(format!("invalid arguments: {err}"))),
        };
        if args.local_path.trim().is_empty() || args.remote_path.trim().is_empty() {
            return Ok(ToolOutput::failure(
                "local_path and remote_path are required",
            ));
        }
        Ok(ToolOutput::success(self.backend.push_file(args).await?))
    }
}

pub struct RemotePushBundleTool<B: RemoteOpsBackend> {
    backend: B,
}

impl<B: RemoteOpsBackend> RemotePushBundleTool<B> {
    pub fn new(backend: B) -> Self {
        Self { backend }
    }
}

#[async_trait]
impl<B: RemoteOpsBackend> Tool for RemotePushBundleTool<B> {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: REMOTE_PUSH_BUNDLE_TOOL_NAME.into(),
            description: "Package a local directory as a tar.gz bundle, upload it to the remote SSH host, extract it remotely, and verify the extracted files against a manifest.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "local_path": { "type": "string" },
                    "remote_path": { "type": "string", "description": "Remote destination directory." },
                    "create_parent": { "type": "boolean" },
                    "overwrite": { "type": "boolean" },
                    "verify_mode": { "type": "string", "enum": ["none", "size", "sha256"] },
                    "remove_archive_after_extract": { "type": "boolean" }
                },
                "required": ["local_path", "remote_path"]
            }),
            risk: RiskLevel::High,
            required_permissions: PermissionSet::developer(),
        }
    }

    async fn invoke(&self, arguments: serde_json::Value) -> Result<ToolOutput> {
        let args: RemotePushBundleArgs = match serde_json::from_value(arguments) {
            Ok(value) => value,
            Err(err) => return Ok(ToolOutput::failure(format!("invalid arguments: {err}"))),
        };
        if args.local_path.trim().is_empty() || args.remote_path.trim().is_empty() {
            return Ok(ToolOutput::failure(
                "local_path and remote_path are required",
            ));
        }
        Ok(ToolOutput::success(self.backend.push_bundle(args).await?))
    }
}

pub struct RemoteRequireTool<B: RemoteOpsBackend> {
    backend: B,
}

impl<B: RemoteOpsBackend> RemoteRequireTool<B> {
    pub fn new(backend: B) -> Self {
        Self { backend }
    }
}

#[async_trait]
impl<B: RemoteOpsBackend> Tool for RemoteRequireTool<B> {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: REMOTE_REQUIRE_TOOL_NAME.into(),
            description: "Given the commands, runtimes, or archive formats needed for a remote task, compare them against the probed remote host and return what is missing plus an installation strategy.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "commands": { "type": "array", "items": { "type": "string" } },
                    "archives": { "type": "array", "items": { "type": "string" } },
                    "runtimes": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "name": { "type": "string" },
                                "version": { "type": "string" }
                            },
                            "required": ["name"]
                        }
                    }
                }
            }),
            risk: RiskLevel::Safe,
            required_permissions: PermissionSet::read_only(),
        }
    }

    async fn invoke(&self, arguments: serde_json::Value) -> Result<ToolOutput> {
        let args: RemoteRequireArgs = match serde_json::from_value(arguments) {
            Ok(value) => value,
            Err(err) => return Ok(ToolOutput::failure(format!("invalid arguments: {err}"))),
        };
        Ok(ToolOutput::success(self.backend.require(args).await?))
    }
}

pub struct RemoteInstallTool<B: RemoteOpsBackend> {
    backend: B,
}

impl<B: RemoteOpsBackend> RemoteInstallTool<B> {
    pub fn new(backend: B) -> Self {
        Self { backend }
    }
}

#[async_trait]
impl<B: RemoteOpsBackend> Tool for RemoteInstallTool<B> {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: REMOTE_INSTALL_TOOL_NAME.into(),
            description: "Install missing remote commands or runtimes on the active SSH host using the remote host's detected package manager. Use remote_require first instead of guessing.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "package_manager": { "type": "string" },
                    "update_index": { "type": "boolean" },
                    "packages": { "type": "array", "items": { "type": "string" } },
                    "commands": { "type": "array", "items": { "type": "string" } },
                    "runtimes": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "name": { "type": "string" },
                                "version": { "type": "string" }
                            },
                            "required": ["name"]
                        }
                    }
                }
            }),
            risk: RiskLevel::High,
            required_permissions: PermissionSet::developer(),
        }
    }

    async fn invoke(&self, arguments: serde_json::Value) -> Result<ToolOutput> {
        let args: RemoteInstallArgs = match serde_json::from_value(arguments) {
            Ok(value) => value,
            Err(err) => return Ok(ToolOutput::failure(format!("invalid arguments: {err}"))),
        };
        Ok(ToolOutput::success(self.backend.install(args).await?))
    }
}

const fn default_true() -> bool {
    true
}

fn default_verify_mode() -> String {
    "sha256".to_string()
}
