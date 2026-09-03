use std::sync::Arc;

use deepagent_core::error::Result;
use deepagent_tools::ToolRegistry;

use crate::mcp_service::McpService;
use crate::plugin_runtime::PluginRuntimeProjection;

#[derive(Default)]
pub(crate) struct McpRuntimeTools {
    pub(crate) hook_registry: Option<Arc<deepagent_mcp::McpRegistry>>,
    pub(crate) lifecycle: Vec<McpLifecycleRecord>,
}

#[derive(Debug, Clone)]
pub(crate) struct McpLifecycleRecord {
    pub(crate) server_id: String,
    pub(crate) status: String,
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
                lifecycle: vec![McpLifecycleRecord {
                    server_id: "mcp".into(),
                    status: "degraded".into(),
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
            server_id,
            status: "ready".into(),
            degradation_code: None,
            reason: None,
        })
        .collect::<Vec<_>>();
    if !failures.is_empty() {
        tracing::warn!(count = failures.len(), "some MCP servers failed to connect");
        lifecycle.extend(
            failures
                .into_iter()
                .map(|(server_id, reason)| McpLifecycleRecord {
                    server_id,
                    status: "degraded".into(),
                    degradation_code: Some("connect_failed".into()),
                    reason: Some(reason),
                    tool_count: 0,
                }),
        );
    }
    for adapter in deepagent_mcp::adapters_for(mcp_registry.clone()) {
        if let Err(error) = registry.register(adapter) {
            tracing::warn!(%error, "failed to register MCP tool adapter");
        }
    }

    Ok(McpRuntimeTools {
        hook_registry: Some(mcp_registry),
        lifecycle,
    })
}
