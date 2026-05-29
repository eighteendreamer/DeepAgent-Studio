//! The MCP registry: manages connected servers and their namespaced tools.
//!
//! For each configured server the registry holds an [`McpClient`] and the tools
//! it discovered. Tools are exposed under a namespaced name —
//! `mcp__<server>__<tool>` — mirroring Claude Code's `mcp__plugin_<plugin>_<server>__<tool>`
//! convention, so remote tools never collide with built-in ones and the source
//! server is always recoverable from the name.

use std::collections::BTreeMap;
use std::sync::Arc;

use deepagent_core::error::{CoreError, Result};

use crate::client::McpClient;
use crate::protocol::{McpToolDef, ToolCallResult};

/// A discovered remote tool, namespaced and tagged with its server.
#[derive(Debug, Clone, PartialEq)]
pub struct RemoteTool {
    /// Namespaced name: `mcp__<server>__<tool>`.
    pub namespaced_name: String,
    /// The server this tool belongs to.
    pub server: String,
    /// The server-local tool name used on the wire.
    pub local_name: String,
    /// Description.
    pub description: String,
    /// JSON Schema for arguments.
    pub input_schema: serde_json::Value,
}

/// Build the namespaced tool name for a server + local tool.
pub fn namespaced_name(server: &str, tool: &str) -> String {
    format!("mcp__{server}__{tool}")
}

/// Split a namespaced name back into `(server, tool)`, if it is one.
pub fn split_namespaced(name: &str) -> Option<(&str, &str)> {
    let rest = name.strip_prefix("mcp__")?;
    let sep = rest.find("__")?;
    Some((&rest[..sep], &rest[sep + 2..]))
}

struct ConnectedServer {
    client: Arc<McpClient>,
    tools: Vec<RemoteTool>,
}

/// Registry of connected MCP servers and their tools.
#[derive(Default)]
pub struct McpRegistry {
    servers: BTreeMap<String, ConnectedServer>,
}

impl McpRegistry {
    /// New empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register an already-connected client under `server_name`, discovering and
    /// namespacing its tools via `tools/list`.
    pub async fn register(&mut self, server_name: &str, client: Arc<McpClient>) -> Result<usize> {
        if self.servers.contains_key(server_name) {
            return Err(CoreError::invalid(format!(
                "MCP server '{server_name}' is already registered"
            )));
        }
        let defs = client.list_tools().await?;
        let tools: Vec<RemoteTool> = defs
            .into_iter()
            .map(|d: McpToolDef| RemoteTool {
                namespaced_name: namespaced_name(server_name, &d.name),
                server: server_name.to_string(),
                local_name: d.name,
                description: d.description,
                input_schema: d.input_schema,
            })
            .collect();
        let count = tools.len();
        self.servers
            .insert(server_name.to_string(), ConnectedServer { client, tools });
        Ok(count)
    }

    /// Number of connected servers.
    pub fn server_count(&self) -> usize {
        self.servers.len()
    }

    /// All namespaced tools across all servers (for advertising to the model).
    pub fn all_tools(&self) -> Vec<RemoteTool> {
        self.servers
            .values()
            .flat_map(|s| s.tools.iter().cloned())
            .collect()
    }

    /// Look up a tool by its namespaced name.
    pub fn find_tool(&self, namespaced: &str) -> Option<&RemoteTool> {
        let (server, _) = split_namespaced(namespaced)?;
        self.servers
            .get(server)?
            .tools
            .iter()
            .find(|t| t.namespaced_name == namespaced)
    }

    /// Invoke a namespaced tool, routing to the owning server's client.
    pub async fn invoke(
        &self,
        namespaced: &str,
        arguments: serde_json::Value,
    ) -> Result<ToolCallResult> {
        let (server, local) = split_namespaced(namespaced)
            .ok_or_else(|| CoreError::invalid(format!("not an MCP tool name: {namespaced}")))?;
        let connected = self
            .servers
            .get(server)
            .ok_or_else(|| CoreError::not_found(format!("MCP server '{server}'")))?;
        // Confirm the tool actually exists on that server.
        if !connected.tools.iter().any(|t| t.local_name == local) {
            return Err(CoreError::not_found(format!(
                "tool '{local}' on MCP server '{server}'"
            )));
        }
        connected.client.call_tool(local, arguments).await
    }

    /// Close all server connections.
    pub async fn close_all(&self) -> Result<()> {
        for s in self.servers.values() {
            let _ = s.client.close().await;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::MockTransport;

    fn client(transport: MockTransport) -> Arc<McpClient> {
        Arc::new(McpClient::new(Arc::new(transport)))
    }

    fn tools_transport() -> MockTransport {
        MockTransport::new()
            .with_result(
                "tools/list",
                serde_json::json!({"tools":[
                    {"name":"search","description":"search docs","inputSchema":{"type":"object"}},
                    {"name":"create_task","description":"create a task","inputSchema":{"type":"object"}}
                ]}),
            )
            .with_result(
                "tools/call",
                serde_json::json!({"content":[{"type":"text","text":"ok"}],"isError":false}),
            )
    }

    #[test]
    fn namespacing_roundtrip() {
        let n = namespaced_name("asana", "create_task");
        assert_eq!(n, "mcp__asana__create_task");
        assert_eq!(split_namespaced(&n), Some(("asana", "create_task")));
        assert_eq!(split_namespaced("read_file"), None);
    }

    #[tokio::test]
    async fn registers_and_namespaces_tools() {
        let mut reg = McpRegistry::new();
        let n = reg.register("asana", client(tools_transport())).await.unwrap();
        assert_eq!(n, 2);
        assert_eq!(reg.server_count(), 1);
        let tools = reg.all_tools();
        assert!(tools.iter().any(|t| t.namespaced_name == "mcp__asana__search"));
        assert!(tools
            .iter()
            .any(|t| t.namespaced_name == "mcp__asana__create_task"));
    }

    #[tokio::test]
    async fn invokes_routed_tool() {
        let mut reg = McpRegistry::new();
        reg.register("asana", client(tools_transport())).await.unwrap();
        let res = reg
            .invoke("mcp__asana__search", serde_json::json!({"q": "x"}))
            .await
            .unwrap();
        assert_eq!(res.text(), "ok");
    }

    #[tokio::test]
    async fn invoke_unknown_server_errors() {
        let reg = McpRegistry::new();
        let err = reg
            .invoke("mcp__ghost__tool", serde_json::json!({}))
            .await
            .unwrap_err();
        assert!(matches!(err, CoreError::NotFound(_)));
    }

    #[tokio::test]
    async fn invoke_unknown_tool_on_known_server_errors() {
        let mut reg = McpRegistry::new();
        reg.register("asana", client(tools_transport())).await.unwrap();
        let err = reg
            .invoke("mcp__asana__nonexistent", serde_json::json!({}))
            .await
            .unwrap_err();
        assert!(matches!(err, CoreError::NotFound(_)));
    }

    #[tokio::test]
    async fn duplicate_server_registration_fails() {
        let mut reg = McpRegistry::new();
        reg.register("asana", client(tools_transport())).await.unwrap();
        assert!(reg.register("asana", client(tools_transport())).await.is_err());
    }
}
