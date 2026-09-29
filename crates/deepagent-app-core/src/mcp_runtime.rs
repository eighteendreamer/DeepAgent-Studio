use std::collections::BTreeMap;
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

/// Connect enabled MCP servers and register their tool adapters into the run
/// registry. Connection failures are intentionally non-fatal: one broken MCP
/// server should not prevent the agent from using built-in tools.
pub(crate) async fn attach_mcp_tools(
    registry: &mut ToolRegistry,
    service: Option<&McpService>,
    plugin_projection: Option<&PluginRuntimeProjection>,
) -> Result<McpRuntimeTools> {
    let Some(mcp) = service else {
        return Ok(McpRuntimeTools::default());
    };

    let snapshot_config = match plugin_projection {
        Some(projection) if !projection.mcp_config.servers.is_empty() => mcp
            .enabled_config_with_plugin_overlay(
                projection.mcp_config.clone(),
                &projection.mcp_server_sources,
            )
            .ok(),
        _ => mcp.enabled_config().ok(),
    };
    let config_metadata = snapshot_config
        .as_ref()
        .map(|config| {
            config
                .servers
                .iter()
                .map(|(id, cfg)| {
                    let transport = cfg
                        .effective_type()
                        .ok()
                        .map(|kind| format!("{kind:?}").to_ascii_lowercase());
                    (id.clone(), (transport, config_hash(cfg)))
                })
                .collect::<BTreeMap<_, _>>()
        })
        .unwrap_or_default();

    let connect_result = match plugin_projection {
        Some(projection) if !projection.mcp_config.servers.is_empty() => {
            mcp.connected_registry_with_plugin_overlay(
                projection.mcp_config.clone(),
                &projection.mcp_server_sources,
            )
            .await
        }
        _ => mcp.connected_registry().await,
    };

    let (mcp_registry, failures) = match connect_result {
        Ok(result) => result,
        Err(error) => {
            tracing::warn!(%error, "MCP connected_registry failed; continuing without MCP tools");
            return Ok(McpRuntimeTools {
                hook_registry: None,
                instructions_block: None,
                instructions_entries: Vec::new(),
                lifecycle: vec![McpLifecycleRecord {
                    server_id: "mcp".into(),
                    status: "degraded".into(),
                    transport: None,
                    config_hash: None,
                    tool_schema_hash: None,
                    startup_attempt: 1,
                    degradation_code: Some("resolve_failed".into()),
                    reason: Some(error.to_string()),
                    tool_count: 0,
                }],
            });
        }
    };

    let mut lifecycle = mcp_registry
        .server_names()
        .into_iter()
        .map(|server_id| McpLifecycleRecord {
            tool_count: mcp_registry
                .all_tools()
                .iter()
                .filter(|tool| tool.server == server_id)
                .count(),
            server_id: server_id.clone(),
            status: "ready".into(),
            transport: config_metadata
                .get(&server_id)
                .and_then(|meta| meta.0.clone()),
            config_hash: config_metadata.get(&server_id).map(|meta| meta.1.clone()),
            tool_schema_hash: Some(tool_schema_hash(&mcp_registry, &server_id)),
            startup_attempt: 1,
            degradation_code: None,
            reason: None,
        })
        .collect::<Vec<_>>();
    if !failures.is_empty() {
        tracing::warn!(count = failures.len(), "some MCP servers failed to connect");
        lifecycle.extend(failures.into_iter().map(|(server_id, reason)| {
            McpLifecycleRecord {
                server_id: server_id.clone(),
                status: "degraded".into(),
                transport: config_metadata
                    .get(&server_id)
                    .and_then(|meta| meta.0.clone()),
                config_hash: config_metadata.get(&server_id).map(|meta| meta.1.clone()),
                tool_schema_hash: None,
                startup_attempt: 1,
                degradation_code: Some("connect_failed".into()),
                reason: Some(reason),
                tool_count: 0,
            }
        }));
    }
    for adapter in deepagent_mcp::adapters_for(mcp_registry.clone()) {
        if let Err(error) = registry.register(adapter) {
            tracing::warn!(%error, "failed to register MCP tool adapter");
        }
    }

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
