use std::sync::Arc;

use deepagent_core::error::Result;
use deepagent_tools::ToolRegistry;
use sha2::{Digest, Sha256};

use crate::mcp_service::McpService;
use crate::plugin_runtime::PluginRuntimeProjection;

#[derive(Default)]
pub(crate) struct McpRuntimeTools {
    pub(crate) hook_registry: Option<Arc<deepagent_mcp::McpRegistry>>,
    pub(crate) lifecycle: Vec<McpLifecycleRecord>,
    /// Aggregated `instructions` from every connected server (MCP `initialize`),
    /// surfaced to the model as `ContextSourceKind::McpCatalog`. `None` when no
    /// server declared any.
    pub(crate) instructions_block: Option<String>,
    /// Unrendered `(server_name, instructions)` pairs from every connected
    /// server that declared instructions. Kept so the run bound can render a
    /// per-session **delta** (only servers not yet announced) instead of
    /// re-sending the full block every turn; [`McpRuntimeTools::instructions_block`]
    /// remains the full-block fallback when delta mode is off.
    pub(crate) instructions_entries: Vec<(String, String)>,
}

#[derive(Debug, Clone)]
pub(crate) struct McpLifecycleRecord {
    pub(crate) server_id: String,
    pub(crate) status: String,
    pub(crate) transport: Option<String>,
    pub(crate) config_hash: Option<String>,
    pub(crate) tool_schema_hash: Option<String>,
    pub(crate) startup_attempt: u32,
    pub(crate) degradation_code: Option<String>,
    pub(crate) reason: Option<String>,
    pub(crate) tool_count: usize,
}

/// Read ready host snapshots only; no configuration I/O or startup awaits.
pub(crate) async fn attach_mcp_tools(
    registry: &mut ToolRegistry,
    service: Option<&McpService>,
    plugin_projection: Option<&PluginRuntimeProjection>,
    plugin_registry: Option<Arc<deepagent_mcp::McpRegistry>>,
) -> Result<McpRuntimeTools> {
    let Some(mcp) = service else {
        return Ok(McpRuntimeTools::default());
    };

    let mut ready = (*mcp.ready_registry()).clone();
    if let Some(plugins) = plugin_registry {
        let mut plugins = (*plugins).clone();
        let user_names = ready.server_names();
        plugins.retain_servers(|name| !user_names.iter().any(|user| user == name));
        ready.extend_ready(&plugins)?;
    }
    let mcp_registry = Arc::new(ready);
    for adapter in deepagent_mcp::adapters_for(mcp_registry.clone()) {
        registry.register(adapter)?;
    }
    let lifecycle = mcp_registry
        .server_names()
        .into_iter()
        .map(|server_id| McpLifecycleRecord {
            config_hash: plugin_projection
                .and_then(|projection| projection.mcp_config.servers.get(&server_id))
                .map(config_hash),
            tool_schema_hash: Some(tool_schema_hash(&mcp_registry, &server_id)),
            tool_count: mcp_registry
                .all_tools()
                .iter()
                .filter(|tool| tool.server == server_id)
                .count(),
            server_id,
            status: "ready".into(),
            transport: None,
            startup_attempt: 0,
            degradation_code: None,
            reason: None,
        })
        .collect();
    Ok(McpRuntimeTools {
        hook_registry: Some(mcp_registry.clone()),
        instructions_block: render_mcp_instructions(&mcp_registry),
        instructions_entries: mcp_registry.server_instructions(),
        lifecycle,
    })
}

/// Render the aggregated MCP `instructions` from every connected server into a
/// single context block, or `None` when no server declared any.
fn render_mcp_instructions(registry: &deepagent_mcp::McpRegistry) -> Option<String> {
    render_mcp_instruction_entries(&registry.server_instructions())
}

/// Render [`McpRuntimeTools::instructions_entries`] into a single context
/// block, or `None` when `entries` is empty. Shared by the full-block path
/// ([`render_mcp_instructions`]) and the per-session delta path (which passes
/// only the not-yet-announced servers) so both render identically.
pub(crate) fn render_mcp_instruction_entries(entries: &[(String, String)]) -> Option<String> {
    if entries.is_empty() {
        return None;
    }
    let mut out = String::from("# MCP server instructions\n");
    for (server, text) in entries {
        out.push_str(&format!("\n## {server}\n{}\n", text.trim()));
    }
    Some(out.trim_end().to_string())
}

fn config_hash(config: &deepagent_mcp::config::McpServerConfig) -> String {
    let value = serde_json::json!({
        "transport": config.transport,
        "command": config.command,
        "args": config.args,
        "envKeys": config.env.keys().collect::<Vec<_>>(),
        "cwd": config.cwd,
        "url": config.url,
        "headerKeys": config.headers.keys().collect::<Vec<_>>(),
    });
    let mut hasher = Sha256::new();
    hasher.update(serde_json::to_vec(&value).unwrap_or_default());
    format!("{:x}", hasher.finalize())
}

fn tool_schema_hash(registry: &deepagent_mcp::McpRegistry, server_id: &str) -> String {
    let mut tools = registry
        .all_tools()
        .into_iter()
        .filter(|tool| tool.server == server_id)
        .map(|tool| (tool.local_name, tool.description, tool.input_schema))
        .collect::<Vec<_>>();
    tools.sort_by(|a, b| a.0.cmp(&b.0));
    let mut hasher = Sha256::new();
    hasher.update(serde_json::to_vec(&tools).unwrap_or_default());
    format!("{:x}", hasher.finalize())
}
