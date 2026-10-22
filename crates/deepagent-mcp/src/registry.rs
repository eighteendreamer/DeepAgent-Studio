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
use deepagent_tools::ToolExecutionContext;

use crate::client::McpClient;
use crate::liveness::{LivenessConfig, LivenessProbe};
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

/// Split at the first delimiter for simple server names. Server names may also
/// contain `__`; routing must use registered [`RemoteTool`] ownership instead.
pub fn split_namespaced(name: &str) -> Option<(&str, &str)> {
    let rest = name.strip_prefix("mcp__")?;
    let sep = rest.find("__")?;
    Some((&rest[..sep], &rest[sep + 2..]))
}

#[derive(Clone)]
struct ConnectedServer {
    client: Arc<McpClient>,
    tools: Vec<RemoteTool>,
    /// Background liveness pinger (long-lived registries only); aborts with
    /// the entry, so probing stops exactly when the connection is dropped.
    probe: Option<Arc<LivenessProbe>>,
    /// Server `instructions` from the MCP `initialize` result, surfaced to the
    /// model via `<mcp-instructions>` context. `None` when the server omits it.
    instructions: Option<String>,
}

/// Registry of connected MCP servers and their tools.
#[derive(Default, Clone)]
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
        self.register_with_instructions(server_name, client, None)
            .await
    }

    /// Like [`Self::register`], but also stores the server's `instructions`
    /// from the `initialize` handshake so they can be surfaced to the model.
    pub async fn register_with_instructions(
        &mut self,
        server_name: &str,
        client: Arc<McpClient>,
        instructions: Option<String>,
    ) -> Result<usize> {
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
        let mut names = std::collections::BTreeSet::new();
        for tool in &tools {
            if !names.insert(&tool.namespaced_name)
                || self.find_tool(&tool.namespaced_name).is_some()
            {
                return Err(CoreError::invalid(format!(
                    "MCP tool '{}' is already registered",
                    tool.namespaced_name
                )));
            }
        }
        self.servers.insert(
            server_name.to_string(),
            ConnectedServer {
                client,
                tools,
                probe: None,
                instructions,
            },
        );
        Ok(count)
    }

    /// Start a background liveness probe (MCP spec `ping` utility) for every
    /// registered server. Intended for long-lived registries (the cross-run
    /// connection cache): a dead server is detected between tool calls and
    /// recovered by the self-healing transport underneath the ping, instead of
    /// surprising the next tool invocation. Probes end with the registry.
    pub fn enable_liveness(&mut self, config: LivenessConfig) {
        for (name, server) in &mut self.servers {
            if server.probe.is_none() {
                server.probe = Some(Arc::new(LivenessProbe::spawn(
                    server.client.clone(),
                    name.clone(),
                    config.clone(),
                )));
            }
        }
    }

    /// Number of connected servers.
    pub fn server_count(&self) -> usize {
        self.servers.len()
    }

    /// Combine already discovered entries without performing network I/O.
    /// Connections and their liveness probes stay shared by immutable snapshots.
    pub fn extend_ready(&mut self, other: &Self) -> Result<()> {
        for name in other.servers.keys() {
            if self.servers.contains_key(name) {
                return Err(CoreError::invalid(format!("duplicate MCP server: {name}")));
            }
        }
        self.servers.extend(other.servers.clone());
        Ok(())
    }

    pub fn retain_servers(&mut self, mut keep: impl FnMut(&str) -> bool) {
        self.servers.retain(|name, _| keep(name));
    }

    /// Connected server identifiers, stable-sorted for deterministic lifecycle
    /// projections and capability snapshots.
    pub fn server_names(&self) -> Vec<String> {
        self.servers.keys().cloned().collect()
    }

    /// All namespaced tools across all servers (for advertising to the model).
    /// Server-provided `instructions` (from the `initialize` handshake), paired
    /// with the server name, for every server that declared any. Empty when no
    /// server provided instructions.
    pub fn server_instructions(&self) -> Vec<(String, String)> {
        self.servers
            .iter()
            .filter_map(|(name, server)| {
                server
                    .instructions
                    .as_ref()
                    .filter(|text| !text.trim().is_empty())
                    .map(|text| (name.clone(), text.clone()))
            })
            .collect()
    }

    pub fn all_tools(&self) -> Vec<RemoteTool> {
        self.servers
            .values()
            .flat_map(|s| s.tools.iter().cloned())
            .collect()
    }

    /// Look up a tool by its namespaced name.
    pub fn find_tool(&self, namespaced: &str) -> Option<&RemoteTool> {
        self.servers
            .values()
            .flat_map(|server| &server.tools)
            .find(|t| t.namespaced_name == namespaced)
    }

    /// Invoke a namespaced tool, routing to the owning server's client.
    pub async fn invoke(
        &self,
        namespaced: &str,
        arguments: serde_json::Value,
    ) -> Result<ToolCallResult> {
        if split_namespaced(namespaced).is_none() {
            return Err(CoreError::invalid(format!(
                "not an MCP tool name: {namespaced}"
            )));
        }
        let tool = self
            .find_tool(namespaced)
            .ok_or_else(|| CoreError::not_found(format!("MCP tool '{namespaced}'")))?;
        let connected = self
            .servers
            .get(&tool.server)
            .ok_or_else(|| CoreError::not_found(format!("MCP server '{}'", tool.server)))?;
        connected
            .client
            .call_tool(&tool.local_name, arguments)
            .await
    }

    /// Invoke a namespaced tool with run-scoped cancellation controls.
    pub async fn invoke_with_context(
        &self,
        namespaced: &str,
        arguments: serde_json::Value,
        context: ToolExecutionContext,
    ) -> Result<ToolCallResult> {
        if split_namespaced(namespaced).is_none() {
            return Err(CoreError::invalid(format!(
                "not an MCP tool name: {namespaced}"
            )));
        }
        let tool = self
            .find_tool(namespaced)
            .ok_or_else(|| CoreError::not_found(format!("MCP tool '{namespaced}'")))?;
        let connected = self
            .servers
            .get(&tool.server)
            .ok_or_else(|| CoreError::not_found(format!("MCP server '{}'", tool.server)))?;
        connected
            .client
            .call_tool_with_context(&tool.local_name, arguments, context)
            .await
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

    #[tokio::test]
    #[ignore = "audit: a parked stdio server must leave discovery and stop automatic respawns"]
    async fn audit_exhausted_liveness_retires_tools_and_stops_request_reconnects() {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("exhausted-server.cjs");
        let attempts = dir.path().join("process-starts");
        std::fs::write(
            &script,
            r#"
const fs = require('node:fs');
const attempts = process.argv[2];
const first = !fs.existsSync(attempts);
fs.appendFileSync(attempts, 'start\n');
if (!first) process.exit(19);
require('node:readline').createInterface({ input: process.stdin }).on('line', line => {
  const msg = JSON.parse(line);
  const reply = result => process.stdout.write(JSON.stringify({ jsonrpc: '2.0', id: msg.id, result }) + '\n');
  if (msg.method === 'initialize') {
    reply({ protocolVersion: msg.params.protocolVersion, capabilities: { tools: {} },
      serverInfo: { name: 'exhaustion-audit', version: '1' } });
  } else if (msg.method === 'tools/list') {
    reply({ tools: [{ name: 'echo', description: 'audit tool', inputSchema: { type: 'object' } }] });
  } else if (msg.method === 'ping') { process.exit(19); }
  else { reply({ content: [{ type: 'text', text: 'ok' }] }); }
});
"#,
        )
        .unwrap();
        let config = crate::McpServerConfig {
            transport: None,
            command: Some("node".into()),
            args: vec![
                script.to_string_lossy().into_owned(),
                attempts.to_string_lossy().into_owned(),
            ],
            env: Default::default(),
            cwd: None,
            url: None,
            headers: Default::default(),
        };
        let transport = crate::ReconnectingTransport::new(
            Arc::new(crate::StdioTransport::spawn(&config).unwrap()),
            Arc::new(crate::ConfigReconnectFactory::new(
                config,
                "exhaustion-audit",
            )),
        )
        .with_backoff(vec![std::time::Duration::from_millis(5); 3]);
        let client = Arc::new(McpClient::new(Arc::new(transport)));
        client.initialize("exhaustion-audit").await.unwrap();
        let mut registry = McpRegistry::new();
        registry
            .register_with_instructions("audit", client, Some("audit instructions".into()))
            .await
            .unwrap();
        registry.enable_liveness(LivenessConfig {
            interval: std::time::Duration::from_millis(5),
            timeout: std::time::Duration::from_secs(3),
            failure_limit: 3,
        });
        let probe = registry.servers["audit"].probe.as_ref().unwrap().clone();
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            while !probe.is_finished() {
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("real failed handshakes must exhaust the liveness budget");
        let starts = || std::fs::read_to_string(&attempts).unwrap().lines().count();
        let after_probe = starts();
        let registry = Arc::new(registry);
        let adapters = crate::adapters_for(registry.clone());
        let mut failed_calls = Vec::new();
        let mut starts_after_calls = Vec::new();
        for _ in 0..2 {
            let result = tokio::time::timeout(
                std::time::Duration::from_secs(3),
                registry.invoke("mcp__audit__echo", serde_json::json!({})),
            )
            .await
            .expect("fixture retries must finish");
            failed_calls.push(result.is_err());
            starts_after_calls.push(starts());
        }
        registry.close_all().await.unwrap();
        eprintln!(
            "{}",
            serde_json::json!({
                "probe_finished": probe.is_finished(),
                "starts_after_probe": after_probe,
                "starts_after_calls": starts_after_calls,
                "failed_calls": failed_calls,
                "discoverable_tools": registry.all_tools().len(),
                "adapter_count": adapters.len(),
                "instruction_entries": registry.server_instructions().len(),
            })
        );
        assert!(
            registry.all_tools().is_empty()
                && starts_after_calls.iter().all(|count| *count == after_probe),
            "a parked server must not remain discoverable and spend a new recovery budget per call"
        );
    }

    #[test]
    fn namespacing_roundtrip() {
        let n = namespaced_name("asana", "create_task");
        assert_eq!(n, "mcp__asana__create_task");
        assert_eq!(split_namespaced(&n), Some(("asana", "create_task")));
        assert_eq!(split_namespaced("read_file"), None);
    }

    #[tokio::test]
    async fn register_with_instructions_surfaces_server_instructions() {
        let mut registry = McpRegistry::new();
        registry
            .register_with_instructions(
                "demo",
                client(tools_transport()),
                Some("Use the demo tools.".to_string()),
            )
            .await
            .unwrap();
        assert_eq!(
            registry.server_instructions(),
            vec![("demo".to_string(), "Use the demo tools.".to_string())]
        );
    }

    #[tokio::test]
    async fn register_without_instructions_reports_none() {
        let mut registry = McpRegistry::new();
        registry
            .register("demo", client(tools_transport()))
            .await
            .unwrap();
        assert!(registry.server_instructions().is_empty());
    }

    #[tokio::test]
    async fn registers_and_namespaces_tools() {
        let mut reg = McpRegistry::new();
        let n = reg
            .register("asana", client(tools_transport()))
            .await
            .unwrap();
        assert_eq!(n, 2);
        assert_eq!(reg.server_count(), 1);
        let tools = reg.all_tools();
        assert!(tools
            .iter()
            .any(|t| t.namespaced_name == "mcp__asana__search"));
        assert!(tools
            .iter()
            .any(|t| t.namespaced_name == "mcp__asana__create_task"));
    }

    #[tokio::test]
    async fn invokes_routed_tool() {
        let mut reg = McpRegistry::new();
        reg.register("asana", client(tools_transport()))
            .await
            .unwrap();
        let res = reg
            .invoke("mcp__asana__search", serde_json::json!({"q": "x"}))
            .await
            .unwrap();
        assert_eq!(res.text(), "ok");
    }

    #[tokio::test]
    async fn plugin_server_names_route_using_registered_ownership() {
        let transport = Arc::new(tools_transport());
        let mut reg = McpRegistry::new();
        reg.register("plugin", client(tools_transport()))
            .await
            .unwrap();
        reg.register(
            "plugin__rea__rea",
            Arc::new(McpClient::new(transport.clone())),
        )
        .await
        .unwrap();
        let name = "mcp__plugin__rea__rea__search";
        let tool = reg.find_tool(name).expect("plugin tool lookup");
        assert_eq!(tool.server, "plugin__rea__rea");
        assert_eq!(
            reg.invoke(name, serde_json::json!({"q":"x"}))
                .await
                .unwrap()
                .text(),
            "ok"
        );
        assert_eq!(transport.sent_methods(), vec!["tools/list", "tools/call"]);
    }

    #[tokio::test]
    async fn ambiguous_namespaced_tool_registration_is_rejected() {
        let mut reg = McpRegistry::new();
        reg.register("plugin__rea", client(tools_transport()))
            .await
            .unwrap();
        let ambiguous = MockTransport::new().with_result(
            "tools/list",
            serde_json::json!({
                "tools":[{"name":"rea__search", "inputSchema":{}}]
            }),
        );
        assert!(reg.register("plugin", client(ambiguous)).await.is_err());
        assert_eq!(reg.server_count(), 1);
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
        reg.register("asana", client(tools_transport()))
            .await
            .unwrap();
        let err = reg
            .invoke("mcp__asana__nonexistent", serde_json::json!({}))
            .await
            .unwrap_err();
        assert!(matches!(err, CoreError::NotFound(_)));
    }

    #[tokio::test]
    async fn duplicate_server_registration_fails() {
        let mut reg = McpRegistry::new();
        reg.register("asana", client(tools_transport()))
            .await
            .unwrap();
        assert!(reg
            .register("asana", client(tools_transport()))
            .await
            .is_err());
    }

    #[tokio::test]
    async fn enable_liveness_probes_every_server_once() {
        use crate::liveness::LivenessConfig;
        use std::time::Duration;

        let mut reg = McpRegistry::new();
        reg.register(
            "asana",
            client(tools_transport().with_result("ping", serde_json::json!({}))),
        )
        .await
        .unwrap();
        let config = LivenessConfig {
            interval: Duration::from_millis(1),
            timeout: Duration::from_millis(200),
            failure_limit: 3,
        };
        reg.enable_liveness(config.clone());
        // Idempotent: a second call must not double-probe.
        reg.enable_liveness(config);
        tokio::time::sleep(Duration::from_millis(30)).await;
        // Registry stays fully usable while probed.
        let res = reg
            .invoke("mcp__asana__search", serde_json::json!({"q": "x"}))
            .await
            .unwrap();
        assert_eq!(res.text(), "ok");
    }
}
