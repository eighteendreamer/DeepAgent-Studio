//! Remote SSH interactive tools for the remote AI assistant.
//!
//! These tools let the agent execute commands, read files, and list directories
//! on the remote server via an active SSH connection.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use deepagent_core::error::Result;
use deepagent_tools::permission::{PermissionSet, RiskLevel};
use deepagent_tools::{Tool, ToolDescriptor, ToolOutput};

/// Tool name for remote bash execution.
pub const REMOTE_BASH_TOOL_NAME: &str = "remote_bash";
/// Tool name for remote file reading.
pub const REMOTE_READ_FILE_TOOL_NAME: &str = "remote_read_file";
/// Tool name for remote directory listing.
pub const REMOTE_LIST_DIR_TOOL_NAME: &str = "remote_list_dir";

/// Arguments for the remote bash tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteBashArgs {
    /// Shell command to execute on the remote server.
    pub command: String,
}

/// Arguments for the remote read-file tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteReadFileArgs {
    /// Path to the file on the remote server.
    pub path: String,
}

/// Arguments for the remote list-directory tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteListDirArgs {
    /// Path to the directory on the remote server.
    pub path: String,
}

/// Backend trait for remote interactive operations (bash, read_file, list_dir).
///
/// Implementations bridge to the actual transport (e.g. SSH). The desktop app
/// provides an [`SshRemoteInteractiveBackend`] that delegates to `SshService`.
#[async_trait]
pub trait RemoteInteractiveBackend: Send + Sync {
    /// Execute a shell command on the remote server.
    async fn bash(&self, args: RemoteBashArgs) -> Result<serde_json::Value>;
    /// Read a file from the remote server.
    async fn read_file(&self, args: RemoteReadFileArgs) -> Result<serde_json::Value>;
    /// List a directory on the remote server.
    async fn list_dir(&self, args: RemoteListDirArgs) -> Result<serde_json::Value>;
}

/// Tool that executes a shell command on the remote SSH server.
pub struct RemoteBashTool {
    backend: Arc<dyn RemoteInteractiveBackend>,
}

impl RemoteBashTool {
    /// Create a new remote bash tool backed by the given backend.
    pub fn new(backend: Arc<dyn RemoteInteractiveBackend>) -> Self {
        Self { backend }
    }
}

#[async_trait]
impl Tool for RemoteBashTool {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: REMOTE_BASH_TOOL_NAME.into(),
            description: "Execute a shell command on the remote SSH server and return stdout, stderr, and exit code.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string", "description": "The shell command to execute on the remote server." }
                },
                "required": ["command"]
            }),
            risk: RiskLevel::Medium,
            required_permissions: PermissionSet::developer(),
        }
    }

    async fn invoke(&self, arguments: serde_json::Value) -> Result<ToolOutput> {
        let args: RemoteBashArgs = match serde_json::from_value(arguments) {
            Ok(value) => value,
            Err(err) => return Ok(ToolOutput::failure(format!("invalid arguments: {err}"))),
        };
        if args.command.trim().is_empty() {
            return Ok(ToolOutput::failure("command is required"));
        }
        match self.backend.bash(args).await {
            Ok(value) => Ok(ToolOutput::success(value)),
            Err(err) => Ok(ToolOutput::failure(err.to_string())),
        }
    }
}

/// Tool that reads a file from the remote SSH server.
pub struct RemoteReadFileTool {
    backend: Arc<dyn RemoteInteractiveBackend>,
}

impl RemoteReadFileTool {
    /// Create a new remote read-file tool backed by the given backend.
    pub fn new(backend: Arc<dyn RemoteInteractiveBackend>) -> Self {
        Self { backend }
    }
}

#[async_trait]
impl Tool for RemoteReadFileTool {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: REMOTE_READ_FILE_TOOL_NAME.into(),
            description: "Read the contents of a file on the remote SSH server.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "The path to the file on the remote server." }
                },
                "required": ["path"]
            }),
            risk: RiskLevel::Safe,
            required_permissions: PermissionSet::read_only(),
        }
    }

    async fn invoke(&self, arguments: serde_json::Value) -> Result<ToolOutput> {
        let args: RemoteReadFileArgs = match serde_json::from_value(arguments) {
            Ok(value) => value,
            Err(err) => return Ok(ToolOutput::failure(format!("invalid arguments: {err}"))),
        };
        if args.path.trim().is_empty() {
            return Ok(ToolOutput::failure("path is required"));
        }
        match self.backend.read_file(args).await {
            Ok(value) => Ok(ToolOutput::success(value)),
            Err(err) => Ok(ToolOutput::failure(err.to_string())),
        }
    }
}

/// Tool that lists a directory on the remote SSH server.
pub struct RemoteListDirTool {
    backend: Arc<dyn RemoteInteractiveBackend>,
}

impl RemoteListDirTool {
    /// Create a new remote list-directory tool backed by the given backend.
    pub fn new(backend: Arc<dyn RemoteInteractiveBackend>) -> Self {
        Self { backend }
    }
}

#[async_trait]
impl Tool for RemoteListDirTool {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: REMOTE_LIST_DIR_TOOL_NAME.into(),
            description: "List the contents of a directory on the remote SSH server.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "The path to the directory on the remote server." }
                },
                "required": ["path"]
            }),
            risk: RiskLevel::Safe,
            required_permissions: PermissionSet::read_only(),
        }
    }

    async fn invoke(&self, arguments: serde_json::Value) -> Result<ToolOutput> {
        let args: RemoteListDirArgs = match serde_json::from_value(arguments) {
            Ok(value) => value,
            Err(err) => return Ok(ToolOutput::failure(format!("invalid arguments: {err}"))),
        };
        if args.path.trim().is_empty() {
            return Ok(ToolOutput::failure("path is required"));
        }
        match self.backend.list_dir(args).await {
            Ok(value) => Ok(ToolOutput::success(value)),
            Err(err) => Ok(ToolOutput::failure(err.to_string())),
        }
    }
}
