//! The MCP registry: manages connected servers and their namespaced tools.
//!
//! For each configured server the registry holds an [`McpClient`] and the tools
//! it discovered. Tools are exposed under a namespaced name —
//! `mcp__<server>__<tool>` for unambiguous short names, otherwise a stable bounded
//! alias. Raw ownership lives in [`RemoteTool`], never in parsed alias text.

use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::sync::Arc;
use tracing::Instrument;

use deepagent_core::error::{CoreError, Result};
use deepagent_tools::{
    validate_tool_arguments, validate_tool_name, validate_tool_schema, ToolExecutionContext,
    MAX_TOOL_NAME_BYTES,
};

use crate::catalog::{CatalogSnapshot, ToolCatalog};
use crate::client::McpClient;
use crate::liveness::{LivenessConfig, LivenessProbe};
use crate::protocol::{McpToolDef, ToolCallResult};

/// A discovered remote tool, namespaced and tagged with its server.
#[derive(Debug, Clone, PartialEq)]
pub struct RemoteTool {
    /// Stable model-visible name; not an encoding that callers may reverse.
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

/// Project a raw identity to the shared function-name wire contract.
///
/// Version 1 is independent of catalog membership/order, so adding a server or
/// rebuilding a snapshot cannot invalidate approvals, discoveries or history.
/// The reserved alias namespace cannot be produced by the legacy path. Full
/// SHA-256 of length-delimited UTF-8 identities prevents delimiter/sanitization
/// ambiguity; registration still rejects any actual collision, without retrying
/// or choosing an arbitrary owner. Ordinary legacy names remain unchanged.
pub fn namespaced_name(server: &str, tool: &str) -> String {
    const ALIAS_SERVER: &str = "alias_v1";
    const PREFIX: &str = "mcp__alias_v1__";
    let legacy = format!("mcp__{server}__{tool}");
    let unambiguous = |part: &str| {
        !part.is_empty() && !part.contains("__") && !part.starts_with('_') && !part.ends_with('_')
    };
    if server != ALIAS_SERVER
        && unambiguous(server)
        && unambiguous(tool)
        && validate_tool_name(&legacy).is_ok()
    {
        return legacy;
    }
    let mut hash = Sha256::new();
    hash.update(b"deepagent-mcp-tool-name-v1");
    for part in [server, tool] {
        hash.update((part.len() as u64).to_le_bytes());
        hash.update(part.as_bytes());
    }
    let digest = format!("{:x}", hash.finalize());
    let hint: String = legacy[5..]
        .chars()
        .take(MAX_TOOL_NAME_BYTES - PREFIX.len() - 2 - digest.len())
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect();
    format!("{PREFIX}{hint}__{digest}")
}

/// Historical convenience for legacy names only, not a routing API. Hashed
/// aliases are deliberately not reversible. Always look up [`RemoteTool`].
pub fn split_namespaced(name: &str) -> Option<(&str, &str)> {
    let rest = name.strip_prefix("mcp__")?;
    let sep = rest.find("__")?;
    let server = &rest[..sep];
    (server != "alias_v1").then_some((server, &rest[sep + 2..]))
}

#[derive(Clone)]
struct ConnectedServer {
    client: Arc<McpClient>,
    catalog: Arc<ToolCatalog>,
    /// A request/host-reader view, not a second mutable catalog.
    frozen_catalog: Option<Arc<CatalogSnapshot>>,
    /// Run-bound discovery projection, never a second dispatch/lifecycle owner.
    /// Live registries leave this empty; a request snapshots it once for all
    /// descriptors, instructions and lifecycle records of the same identity.
    discovery_status: Option<crate::McpConnectionStatus>,
    /// Background liveness pinger (long-lived registries only); aborts with
    /// the entry, so probing stops exactly when the connection is dropped.
    probe: Option<Arc<LivenessProbe>>,
    /// Server `instructions` from the MCP `initialize` result, surfaced to the
    /// model via `<mcp-instructions>` context. `None` when the server omits it.
    instructions: Option<String>,
}

impl ConnectedServer {
    fn catalog(&self) -> Arc<CatalogSnapshot> {
        self.frozen_catalog
            .clone()
            .unwrap_or_else(|| self.catalog.snapshot())
    }

    fn discovery_status(&self) -> crate::McpConnectionStatus {
        self.discovery_status
            .clone()
            .unwrap_or_else(|| self.client.connection_status())
    }
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
        let defs = client
            .list_tools()
            .instrument(tracing::info_span!("mcp_tools_list", server = server_name))
            .await?;
        let tools = validated_tools(server_name, defs)?;
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
                catalog: Arc::new(ToolCatalog::new(
                    server_name.to_owned(),
                    client.clone(),
                    tools,
                )),
                frozen_catalog: None,
                client,
                discovery_status: None,
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
        let mut tool_names = self
            .all_tools()
            .into_iter()
            .map(|tool| tool.namespaced_name)
            .collect::<std::collections::BTreeSet<_>>();
        for tool in other.all_tools() {
            if !tool_names.insert(tool.namespaced_name.clone()) {
                return Err(CoreError::invalid(format!(
                    "MCP tool '{}' is already registered",
                    tool.namespaced_name
                )));
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

    /// Read the same live identity state used by transport dispatch.
    pub fn server_status(&self, name: &str) -> Option<crate::McpConnectionStatus> {
        self.servers
            .get(name)
            .map(ConnectedServer::discovery_status)
    }

    /// Capture one discovery view at a request boundary. Dispatch still checks
    /// each live client, so a previously advertised tool cannot revive it.
    pub fn discovery_snapshot(&self) -> Self {
        let mut snapshot = self.catalog_snapshot();
        for server in snapshot.servers.values_mut() {
            server.discovery_status = Some(server.client.connection_status());
        }
        snapshot
    }

    /// Freeze definitions and routes without performing discovery I/O. Live
    /// entries (including plugin entries) share one connection-owned updater.
    pub fn catalog_snapshot(&self) -> Self {
        let mut snapshot = self.clone();
        for server in snapshot.servers.values_mut() {
            server.frozen_catalog = Some(server.catalog());
        }
        snapshot
    }

    pub fn catalog_generation(&self) -> String {
        let mut digest = Sha256::new();
        for (name, server) in &self.servers {
            digest.update((name.len() as u64).to_le_bytes());
            digest.update(name.as_bytes());
            digest.update(server.catalog().generation.as_bytes());
        }
        format!("{:x}", digest.finalize())
    }

    pub fn server_catalog_error(&self, name: &str) -> Option<String> {
        self.servers
            .get(name)
            .and_then(|server| server.catalog().error.clone())
    }

    /// Retiring a host identity stops only refresh work; borrowed run snapshots
    /// retain their original definitions/client and existing dispatch policy.
    pub fn stop_catalog_refresh(&self) {
        for server in self.servers.values() {
            server.catalog.stop();
        }
    }

    /// All namespaced tools across all servers (for advertising to the model).
    /// Server-provided `instructions` (from the `initialize` handshake), paired
    /// with the server name, for every server that declared any. Empty when no
    /// server provided instructions.
    pub fn server_instructions(&self) -> Vec<(String, String)> {
        self.servers
            .iter()
            .filter(|(_, server)| {
                server.discovery_status().is_discoverable() && server.catalog().error.is_none()
            })
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
            .filter(|server| server.discovery_status().is_discoverable())
            .flat_map(|s| s.catalog().tools.clone())
            .collect()
    }

    /// Look up a tool by its namespaced name.
    pub fn find_tool(&self, namespaced: &str) -> Option<RemoteTool> {
        self.lookup_tool(namespaced, true)
    }

    fn lookup_tool(&self, namespaced: &str, discoverable: bool) -> Option<RemoteTool> {
        let mut tools = self
            .servers
            .values()
            .filter(|server| !discoverable || server.discovery_status().is_discoverable())
            .filter_map(|server| {
                server
                    .catalog()
                    .tools
                    .iter()
                    .find(|tool| tool.namespaced_name == namespaced)
                    .cloned()
            });
        let first = tools.next()?;
        // No ambiguous first-owner routing, even if an actual alias collision
        // were ever admitted by independently refreshed server catalogs.
        tools.next().is_none().then_some(first)
    }

    // Retain metadata for previously borrowed adapters and historical calls;
    // dispatch still goes through the live lifecycle gate, yielding a typed
    // not-dispatched error rather than reviving an unavailable connection.
    fn registered_tool(&self, namespaced: &str) -> Option<RemoteTool> {
        self.lookup_tool(namespaced, false)
    }

    /// Invoke a namespaced tool, routing to the owning server's client.
    pub async fn invoke(
        &self,
        namespaced: &str,
        arguments: serde_json::Value,
    ) -> Result<ToolCallResult> {
        if !namespaced.starts_with("mcp__") {
            return Err(CoreError::invalid(format!(
                "not an MCP tool name: {namespaced}"
            )));
        }
        let tool = self
            .registered_tool(namespaced)
            .ok_or_else(|| CoreError::not_found(format!("MCP tool '{namespaced}'")))?;
        validate_tool_arguments(namespaced, &tool.input_schema, &arguments)?;
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
        if !namespaced.starts_with("mcp__") {
            return Err(CoreError::invalid(format!(
                "not an MCP tool name: {namespaced}"
            )));
        }
        let tool = self
            .registered_tool(namespaced)
            .ok_or_else(|| CoreError::not_found(format!("MCP tool '{namespaced}'")))?;
        validate_tool_arguments(namespaced, &tool.input_schema, &arguments)?;
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
        self.stop_catalog_refresh();
        for s in self.servers.values() {
            let _ = s.client.close().await;
        }
        Ok(())
    }
}

pub(crate) fn validated_tools(server_name: &str, defs: Vec<McpToolDef>) -> Result<Vec<RemoteTool>> {
    let mut tools = Vec::new();
    let mut names = std::collections::BTreeSet::new();
    for definition in defs {
        let name = namespaced_name(server_name, &definition.name);
        if let Err(error) = validate_tool_schema(&name, &definition.input_schema) {
            tracing::warn!(server = server_name, tool = definition.name, %error, "ignoring MCP tool with invalid input schema");
            continue;
        }
        if !names.insert(name.clone()) {
            return Err(CoreError::invalid(
                "MCP catalog contains a duplicate tool identity",
            ));
        }
        tools.push(RemoteTool {
            namespaced_name: name,
            server: server_name.to_owned(),
            local_name: definition.name,
            description: definition.description,
            input_schema: definition.input_schema,
        });
    }
    Ok(tools)
}

#[cfg(test)]
#[path = "../tests/support/node_runtime.rs"]
#[allow(dead_code)] // Shared support also has helpers for isolated environment hosts.
mod node_runtime;

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

    fn single_tool_transport(name: &str) -> MockTransport {
        MockTransport::new()
            .with_result(
                "tools/list",
                serde_json::json!({"tools":[
                    {"name":name,"description":"collision fixture","inputSchema":{"type":"object"}}
                ]}),
            )
            .with_result(
                "tools/call",
                serde_json::json!({"content":[{"type":"text","text":"ok"}],"isError":false}),
            )
    }

    fn strict_tools_transport() -> MockTransport {
        MockTransport::new()
            .with_result(
                "tools/list",
                serde_json::json!({
                    "tools": [{
                        "name": "check",
                        "description": "check policy",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "decision": {"type": "string", "enum": ["allow", "deny"]},
                                "hook_input": {"type": "object"}
                            },
                            "required": ["decision"],
                            "additionalProperties": false
                        }
                    }]
                }),
            )
            .with_result(
                "tools/call",
                serde_json::json!({
                    "content": [{"type": "text", "text": "ok"}],
                    "isError": false
                }),
            )
    }

    #[tokio::test]
    async fn audit_exhausted_liveness_retires_tools_and_stops_request_reconnects() {
        let node = super::node_runtime::resolve_node_runtime().await;
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
            response_limit_bytes: None,
            transport: None,
            command: Some(node.to_string_lossy().into_owned()),
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
        let after_probe_status =
            format!("{:?}", registry.servers["audit"].client.connection_status());
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
                "status_after_probe": after_probe_status,
                "starts_after_calls": starts_after_calls,
                "failed_calls": failed_calls,
                "discoverable_tools": registry.all_tools().len(),
                "adapter_count": adapters.len(),
                "instruction_entries": registry.server_instructions().len(),
            })
        );
        assert!(
            registry.all_tools().is_empty()
                && adapters.is_empty()
                && registry.server_instructions().is_empty()
                && starts_after_calls.iter().all(|count| *count == after_probe),
            "a parked server must not remain discoverable and spend a new recovery budget per call"
        );
        assert_eq!(
            after_probe, 4,
            "one initial process plus one shared three-attempt budget"
        );
    }

    #[test]
    fn namespacing_roundtrip() {
        let n = namespaced_name("asana", "create_task");
        assert_eq!(n, "mcp__asana__create_task");
        assert_eq!(split_namespaced(&n), Some(("asana", "create_task")));
        assert_eq!(split_namespaced("read_file"), None);
    }

    #[test]
    fn h147_names_are_valid_stable_and_preserve_distinct_raw_identities() {
        let long = "x".repeat(200);
        let pairs = [
            ("simple", "read"),
            ("simple", "Read"),
            ("a", "b__echo"),
            ("a__b", "echo"),
            ("a.b", "read.file"),
            ("a_b", "read_file"),
            ("", ""),
            ("服务", "读取"),
            ("s", long.as_str()),
            ("alias_v1", "echo"),
            ("s", "a\0b"),
            ("s\0a", "b"),
            ("a_", "b"),
            ("a", "_b"),
        ];
        let mut names = std::collections::BTreeSet::new();
        for (server, tool) in pairs {
            let name = namespaced_name(server, tool);
            assert!(!name.is_empty() && name.len() <= 128, "{name}");
            assert!(
                name.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-'),
                "{name}"
            );
            assert!(
                names.insert(name.clone()),
                "raw identities must not collapse: {name}"
            );
            assert_eq!(name, namespaced_name(server, tool));
        }
        assert_eq!(namespaced_name("simple", "read"), "mcp__simple__read");
        assert_eq!(namespaced_name("a.b", "read.file"), "mcp__alias_v1__a_b__read_file__9afc3be941908bce5ded982f0dab85bafa79ee135a82038a550d33d00b1b1b25");
        assert!(split_namespaced(&namespaced_name("a.b", "read.file")).is_none());
        let projected = namespaced_name("a.b", "read.file");
        let forged_raw = projected.strip_prefix("mcp__alias_v1__").unwrap();
        assert_ne!(namespaced_name("alias_v1", forged_raw), projected);
    }

    #[tokio::test]
    async fn h147_registration_order_and_snapshot_merge_do_not_rename_tools() {
        let mut first = McpRegistry::new();
        first
            .register("a", client(single_tool_transport("b__echo")))
            .await
            .unwrap();
        let before = first.all_tools()[0].namespaced_name.clone();
        let mut second = McpRegistry::new();
        second
            .register("a__b", client(single_tool_transport("echo")))
            .await
            .unwrap();
        let other = second.all_tools()[0].namespaced_name.clone();
        first.extend_ready(&second).unwrap();
        assert_ne!(before, other);
        assert!(first.find_tool(&before).is_some());
        assert!(first.find_tool(&other).is_some());
        let mut reversed = McpRegistry::new();
        reversed
            .register("a__b", client(single_tool_transport("echo")))
            .await
            .unwrap();
        reversed
            .register("a", client(single_tool_transport("b__echo")))
            .await
            .unwrap();
        assert_eq!(first.all_tools(), reversed.all_tools());
    }

    #[tokio::test]
    async fn extend_ready_rejects_colliding_namespaced_tools_transactionally() {
        let mut first = McpRegistry::new();
        first
            .register("a", client(single_tool_transport("b__echo")))
            .await
            .unwrap();
        let mut second = McpRegistry::new();
        second
            .register("a__b", client(single_tool_transport("echo")))
            .await
            .unwrap();

        // Force a genuine projected-name collision, independently of the now
        // disambiguated delimiter fixture, to exercise hash-collision safety.
        let collision = first.all_tools()[0].namespaced_name.clone();
        let server = second.servers.get_mut("a__b").unwrap();
        let catalog = server.catalog();
        let mut tools = catalog.tools.clone();
        tools[0].namespaced_name = collision.clone();
        server.frozen_catalog = Some(Arc::new(CatalogSnapshot {
            tools,
            generation: catalog.generation.clone(),
            error: None,
        }));
        let error = first.extend_ready(&second).unwrap_err().to_string();
        assert!(error.contains(&collision), "{error}");
        assert_eq!(first.server_names(), vec!["a"]);
        assert_eq!(first.all_tools().len(), 1);
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
    async fn invalid_tool_schema_isolated_while_valid_tools_register() {
        let transport = MockTransport::new().with_result(
            "tools/list",
            serde_json::json!({
                "tools": [
                    {"name":"broken", "inputSchema":"not-a-schema"},
                    {
                        "name":"search",
                        "description":"search docs",
                        "inputSchema": {
                            "type":"object",
                            "properties":{"query":{"type":"string"}},
                            "required":["query"]
                        }
                    }
                ]
            }),
        );
        let mut registry = McpRegistry::new();
        let count = registry
            .register("schema-audit", client(transport))
            .await
            .unwrap();

        assert_eq!(count, 1);
        assert!(registry.find_tool("mcp__schema-audit__broken").is_none());
        assert!(registry.find_tool("mcp__schema-audit__search").is_some());
    }

    #[tokio::test]
    async fn server_with_only_invalid_tool_schemas_publishes_no_tools() {
        let transport = MockTransport::new().with_result(
            "tools/list",
            serde_json::json!({
                "tools": [
                    {"name":"missing_schema"},
                    {"name":"array_schema", "inputSchema":[]}
                ]
            }),
        );
        let mut registry = McpRegistry::new();
        let count = registry
            .register_with_instructions(
                "schema-audit",
                client(transport),
                Some("still connected".into()),
            )
            .await
            .unwrap();

        assert_eq!(count, 0);
        assert_eq!(registry.server_count(), 1);
        assert!(registry.all_tools().is_empty());
        assert_eq!(
            registry.server_instructions(),
            vec![("schema-audit".into(), "still connected".into())]
        );
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
    async fn invocation_validates_discovered_arguments_before_wire() {
        let transport = Arc::new(strict_tools_transport());
        let mut reg = McpRegistry::new();
        reg.register("policy", Arc::new(McpClient::new(transport.clone())))
            .await
            .unwrap();

        let error = reg
            .invoke(
                "mcp__policy__check",
                serde_json::json!({"decision": "unknown"}),
            )
            .await
            .expect_err("invalid arguments must be rejected before tools/call");
        assert!(error.to_string().contains("input validation failed"));
        assert_eq!(transport.sent_methods(), vec!["tools/list"]);

        let error = reg
            .invoke_with_context(
                "mcp__policy__check",
                serde_json::json!({"decision": "allow", "extra": true}),
                ToolExecutionContext::default(),
            )
            .await
            .expect_err("context-aware invocation must share the schema gate");
        assert!(error.to_string().contains("input validation failed"));
        assert_eq!(transport.sent_methods(), vec!["tools/list"]);

        let result = reg
            .invoke(
                "mcp__policy__check",
                serde_json::json!({"decision": "allow"}),
            )
            .await
            .unwrap();
        assert_eq!(result.text(), "ok");
        assert_eq!(transport.sent_methods(), vec!["tools/list", "tools/call"]);
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
        let name = namespaced_name("plugin__rea__rea", "search");
        let tool = reg.find_tool(&name).expect("plugin tool lookup");
        assert_eq!(tool.server, "plugin__rea__rea");
        assert_eq!(
            reg.invoke(&name, serde_json::json!({"q":"x"}))
                .await
                .unwrap()
                .text(),
            "ok"
        );
        assert_eq!(transport.sent_methods(), vec!["tools/list", "tools/call"]);
    }

    #[tokio::test]
    async fn ambiguous_raw_names_are_disambiguated_without_changing_ownership() {
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
        reg.register("plugin", client(ambiguous)).await.unwrap();
        assert_eq!(reg.server_count(), 2);
        for (server, raw) in [("plugin", "rea__search"), ("plugin__rea", "search")] {
            let tool = reg.find_tool(&namespaced_name(server, raw)).unwrap();
            assert_eq!(tool.server, server);
            assert_eq!(tool.local_name, raw);
        }
    }

    #[tokio::test]
    async fn h147_duplicate_raw_identity_cannot_be_hidden_by_schema_filtering() {
        for invalid in [
            serde_json::json!({"name":"same"}),
            serde_json::json!({"name":"same","inputSchema":false}),
        ] {
            let transport = MockTransport::new().with_result(
                "tools/list",
                serde_json::json!({"tools":[
                    invalid, {"name":"same","inputSchema":{"type":"object"}}
                ]}),
            );
            let mut registry = McpRegistry::new();
            registry
                .register("healthy", client(tools_transport()))
                .await
                .unwrap();
            let before = registry.all_tools();
            let error = registry
                .register_with_instructions(
                    "ambiguous",
                    client(transport),
                    Some("must not publish".into()),
                )
                .await
                .unwrap_err();
            assert!(
                error.to_string().contains("duplicate raw tool identity"),
                "{error}"
            );
            assert_eq!(registry.all_tools(), before);
            assert!(registry.server_instructions().is_empty());
        }
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
