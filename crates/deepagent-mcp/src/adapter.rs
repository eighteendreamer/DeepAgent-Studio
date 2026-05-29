//! Bridge MCP remote tools into the runtime's tool system.
//!
//! [`McpToolAdapter`] wraps a namespaced [`RemoteTool`] plus a shared
//! [`McpRegistry`] as a [`deepagent_tools::Tool`], so MCP tools flow through the
//! exact same capability registry, permission gating, and runtime loop as
//! built-in tools. MCP tools call external services, so they are classified as
//! [`RiskLevel::Medium`] and require the [`Permission::Network`] capability.

use std::sync::Arc;

use async_trait::async_trait;

use deepagent_core::error::Result;
use deepagent_tools::permission::{Permission, PermissionSet, RiskLevel};
use deepagent_tools::{Tool, ToolDescriptor, ToolOutput};

use crate::registry::{McpRegistry, RemoteTool};

/// Adapts one MCP [`RemoteTool`] to the [`Tool`] trait.
pub struct McpToolAdapter {
    tool: RemoteTool,
    registry: Arc<McpRegistry>,
}

impl McpToolAdapter {
    /// Wrap a remote tool with the registry that can route its calls.
    pub fn new(tool: RemoteTool, registry: Arc<McpRegistry>) -> Self {
        Self { tool, registry }
    }
}

#[async_trait]
impl Tool for McpToolAdapter {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: self.tool.namespaced_name.clone(),
            description: self.tool.description.clone(),
            parameters: self.tool.input_schema.clone(),
            // External service calls: notable side effects + network access.
            risk: RiskLevel::Medium,
            required_permissions: PermissionSet::from_iter_perms([Permission::Network]),
        }
    }

    async fn invoke(&self, arguments: serde_json::Value) -> Result<ToolOutput> {
        let result = self
            .registry
            .invoke(&self.tool.namespaced_name, arguments)
            .await?;
        if result.is_error {
            Ok(ToolOutput::failure(result.text()))
        } else {
            Ok(ToolOutput::success(serde_json::json!({
                "text": result.text(),
                "content": result.content,
            })))
        }
    }
}

/// Build adapters for every tool currently in `registry`, sharing the registry
/// via `Arc`. These can be registered into a `deepagent_tools::ToolRegistry`.
pub fn adapters_for(registry: Arc<McpRegistry>) -> Vec<Arc<dyn Tool>> {
    registry
        .all_tools()
        .into_iter()
        .map(|t| Arc::new(McpToolAdapter::new(t, registry.clone())) as Arc<dyn Tool>)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::McpClient;
    use crate::transport::MockTransport;

    async fn registry_with_tools() -> Arc<McpRegistry> {
        let transport = MockTransport::new()
            .with_result(
                "tools/list",
                serde_json::json!({"tools":[
                    {"name":"search","description":"search docs","inputSchema":{"type":"object"}}
                ]}),
            )
            .with_result(
                "tools/call",
                serde_json::json!({"content":[{"type":"text","text":"found it"}],"isError":false}),
            );
        let client = Arc::new(McpClient::new(Arc::new(transport)));
        let mut reg = McpRegistry::new();
        reg.register("docs", client).await.unwrap();
        Arc::new(reg)
    }

    #[tokio::test]
    async fn adapter_descriptor_is_namespaced_and_network_gated() {
        let reg = registry_with_tools().await;
        let adapters = adapters_for(reg);
        assert_eq!(adapters.len(), 1);
        let d = adapters[0].descriptor();
        assert_eq!(d.name, "mcp__docs__search");
        assert_eq!(d.risk, RiskLevel::Medium);
        assert!(d.required_permissions.contains(Permission::Network));
    }

    #[tokio::test]
    async fn adapter_invokes_remote_tool() {
        let reg = registry_with_tools().await;
        let adapters = adapters_for(reg);
        let out = adapters[0]
            .invoke(serde_json::json!({"q": "rust"}))
            .await
            .unwrap();
        assert!(out.ok);
        assert_eq!(out.value["text"], "found it");
    }

    #[tokio::test]
    async fn adapter_surfaces_tool_error() {
        let transport = MockTransport::new()
            .with_result(
                "tools/list",
                serde_json::json!({"tools":[{"name":"boom","description":"","inputSchema":{}}]}),
            )
            .with_result(
                "tools/call",
                serde_json::json!({"content":[{"type":"text","text":"bad input"}],"isError":true}),
            );
        let client = Arc::new(McpClient::new(Arc::new(transport)));
        let mut reg = McpRegistry::new();
        reg.register("svc", client).await.unwrap();
        let adapters = adapters_for(Arc::new(reg));
        let out = adapters[0].invoke(serde_json::json!({})).await.unwrap();
        assert!(!out.ok);
        assert_eq!(out.value["error"], "bad input");
    }
}
