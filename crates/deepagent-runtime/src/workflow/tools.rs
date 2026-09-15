//! Tool execution abstraction for workflow nodes.
//!
//! The trait lives in `deepagent-runtime` so the workflow agent can execute
//! tools without depending on `deepagent-app-core`. The concrete implementation
//! wraps `ToolRegistry` on the app-core side.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use deepagent_core::error::Result;

/// The result of executing a tool.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolExecutionResult {
    /// Whether the tool execution succeeded.
    pub success: bool,
    /// The output value from the tool.
    pub output: Value,
    /// Error message if the tool failed.
    pub error: Option<String>,
}

/// Pluggable tool execution backend for workflow nodes.
#[async_trait]
pub trait ToolExecutor: Send + Sync {
    /// Execute a tool by name with the given arguments.
    async fn execute(&self, tool_name: &str, arguments: Value) -> Result<ToolExecutionResult>;

    /// List available tool names.
    async fn list_tools(&self) -> Result<Vec<String>>;
}
