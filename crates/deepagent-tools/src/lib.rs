//! # deepagent-tools
//!
//! The tool runtime (开发计划.md Phase 4; 开发提示词.md §15–§16).
//!
//! Core ideas implemented here:
//! - [`Tool`] — the async trait every tool implements, exposing a JSON-schema
//!   description and an `invoke` method.
//! - [`permission`] — the [`permission::Permission`] scopes and
//!   [`permission::RiskLevel`] used to gate dangerous operations.
//! - [`registry`] — the Capability Registry / [`registry::ToolRegistry`] that
//!   stores tools, filters them by an agent's granted permissions, and routes
//!   invocations.
//! - [`sandbox`] — the engine-independent [`sandbox::SandboxPolicy`] (memory /
//!   fuel / timeout / capability limits) + [`sandbox::Sandbox`] trait and JSON
//!   ABI helpers. The real WebAssembly backend
//!   ([`wasm::WasmSandbox`](crate::wasm), behind `--features wasm`) executes
//!   untrusted tool modules with no ambient authority.
//!
//! Sandboxing uses WebAssembly (Wasmtime): the [`Tool`] abstraction is designed
//! so a sandboxed executor can wrap any tool transparently, and [`sandbox`]
//! holds the policy/ABI while [`wasm`](crate::wasm) holds the engine backend.

pub mod permission;
pub mod registry;
pub mod sandbox;
pub mod sandboxed_tool;
#[cfg(feature = "wasm")]
pub mod wasm;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use deepagent_core::error::Result;

pub use permission::{Permission, PermissionSet, RiskLevel};
pub use registry::{ToolRegistry, ToolSpec};
pub use sandbox::{Capabilities, Sandbox, SandboxPolicy, SandboxStats};
pub use sandboxed_tool::SandboxedTool;

#[cfg(feature = "wasm")]
pub use wasm::WasmSandbox;

/// A tool invocation request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolInvocation {
    /// The tool name to invoke.
    pub name: String,
    /// JSON arguments.
    pub arguments: serde_json::Value,
    /// Optional provider-assigned call id (from the model's `tool_calls[].id`).
    /// Carried through so the runtime can correlate a tool result back to the
    /// exact call when the model emits several tool calls in one turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
}

impl ToolInvocation {
    /// Build an invocation.
    pub fn new(name: impl Into<String>, arguments: serde_json::Value) -> Self {
        Self {
            name: name.into(),
            arguments,
            id: None,
        }
    }

    /// Build an invocation carrying the model's tool-call id (builder style).
    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }
}

/// The outcome of a tool invocation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolOutput {
    /// Whether the tool succeeded.
    pub ok: bool,
    /// JSON-encoded result (or error detail when `ok == false`).
    pub value: serde_json::Value,
}

impl ToolOutput {
    /// A successful result.
    pub fn success(value: serde_json::Value) -> Self {
        Self { ok: true, value }
    }

    /// A failure result with a message.
    pub fn failure(message: impl Into<String>) -> Self {
        Self {
            ok: false,
            value: serde_json::json!({ "error": message.into() }),
        }
    }
}

/// Metadata describing a tool for routing and for the model's tool schema.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolDescriptor {
    /// Unique tool name.
    pub name: String,
    /// Human / model-facing description.
    pub description: String,
    /// JSON Schema for the arguments.
    pub parameters: serde_json::Value,
    /// Risk classification used for permission routing.
    pub risk: RiskLevel,
    /// Permissions required to invoke this tool.
    pub required_permissions: PermissionSet,
}

/// The trait every tool implements.
#[async_trait]
pub trait Tool: Send + Sync {
    /// Static descriptor (name, schema, risk, permissions).
    fn descriptor(&self) -> ToolDescriptor;

    /// Execute the tool with the given JSON arguments.
    async fn invoke(&self, arguments: serde_json::Value) -> Result<ToolOutput>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_helpers() {
        let ok = ToolOutput::success(serde_json::json!({"a": 1}));
        assert!(ok.ok);
        let err = ToolOutput::failure("boom");
        assert!(!err.ok);
        assert_eq!(err.value["error"], "boom");
    }
}
