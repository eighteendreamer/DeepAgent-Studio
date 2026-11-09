use std::sync::Arc;

use deepagent_core::error::Result;
use deepagent_tools::ToolRegistry;
use sha2::{Digest, Sha256};

use crate::mcp_service::McpService;
use crate::plugin_runtime::PluginRuntimeProjection;

#[derive(Default)]
pub(crate) struct McpRuntimeTools {
    pub(crate) generation: Option<String>,
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
pub(crate) fn attach_mcp_tools(
    registry: &mut ToolRegistry,
    service: Option<&McpService>,
    plugin_projection: Option<&PluginRuntimeProjection>,
    plugin_registry: Option<Arc<deepagent_mcp::McpRegistry>>,
    image_store: Option<Arc<dyn deepagent_mcp::adapter::McpImageStore>>,
) -> Result<McpRuntimeTools> {
    let Some(mcp) = service else {
        return Ok(McpRuntimeTools::default());
    };

    let (mcp_registry, configs, generation) = runtime_snapshot(mcp, plugin_registry)?;
    for tool in mcp_registry.all_tools() {
        let mut adapter = deepagent_mcp::McpToolAdapter::new(tool, mcp_registry.clone());
        if let Some(store) = image_store.as_ref() {
            adapter = adapter.with_image_store(store.clone());
        }
        registry.register(Arc::new(adapter))?;
    }
    let lifecycle = mcp_registry
        .server_names()
        .into_iter()
        .map(|server_id| {
            let (status, degradation_code, reason) = lifecycle_status(&mcp_registry, &server_id);
            McpLifecycleRecord {
                config_hash: configs
                    .get(&server_id)
                    .or_else(|| {
                        plugin_projection
                            .and_then(|projection| projection.mcp_config.servers.get(&server_id))
                    })
                    .map(config_hash),
                tool_schema_hash: Some(tool_schema_hash(&mcp_registry, &server_id)),
                tool_count: mcp_registry
                    .all_tools()
                    .iter()
                    .filter(|tool| tool.server == server_id)
                    .count(),
                server_id,
                status: status.into(),
                transport: None,
                startup_attempt: 0,
                degradation_code,
                reason,
            }
        })
        .collect();
    Ok(McpRuntimeTools {
        generation: Some(generation),
        hook_registry: Some(mcp_registry.clone()),
        instructions_block: render_mcp_instructions(&mcp_registry),
        instructions_entries: mcp_registry.server_instructions(),
        lifecycle,
    })
}

/// Config/read and run assembly must identify the exact same host/plugin
/// catalog projection. This helper only reads already-published revisions.
pub(crate) fn runtime_snapshot(
    mcp: &McpService,
    plugins: Option<Arc<deepagent_mcp::McpRegistry>>,
) -> Result<(
    Arc<deepagent_mcp::McpRegistry>,
    std::collections::BTreeMap<String, deepagent_mcp::McpServerConfig>,
    String,
)> {
    let (registry, configs, generation) = mcp.ready_runtime_snapshot()?;
    let mut ready = (*registry).clone();
    if let Some(plugins) = plugins {
        let mut plugins = (*plugins).clone();
        let user_names = ready.server_names();
        plugins.retain_servers(|name| !user_names.iter().any(|user| user == name));
        if let Err(error) = ready.extend_ready(&plugins) {
            tracing::warn!(%error, "MCP plugin snapshot conflict; excluding plugin capabilities from this run");
        }
    }
    let registry = Arc::new(ready.discovery_snapshot());
    let generation = format!("{generation}:{}", registry.catalog_generation());
    Ok((registry, configs, generation))
}

fn lifecycle_status(
    registry: &deepagent_mcp::McpRegistry,
    server: &str,
) -> (&'static str, Option<String>, Option<String>) {
    if registry.server_catalog_error(server).is_some() {
        return (
            "unavailable",
            Some("mcp_catalog_refresh_failed".into()),
            Some("MCP catalog refresh failed; capabilities withdrawn until a later change or explicit retry".into()),
        );
    }
    match registry.server_status(server) {
        Some(deepagent_mcp::McpConnectionStatus::Unavailable { recovery_attempts, .. }) => (
            "unavailable",
            Some("mcp_recovery_exhausted".into()),
            // Durable reason, never arbitrary peer/handshake diagnostics.
            Some(format!("MCP local connection unavailable after {recovery_attempts} recovery attempts; explicit lifecycle retry required")),
        ),
        Some(deepagent_mcp::McpConnectionStatus::Closed) => (
            "unavailable",
            Some("mcp_connection_closed".into()),
            Some("MCP transport is closed".into()),
        ),
        Some(deepagent_mcp::McpConnectionStatus::Recovering) => ("recovering", None, None),
        _ => ("ready", None, None),
    }
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
