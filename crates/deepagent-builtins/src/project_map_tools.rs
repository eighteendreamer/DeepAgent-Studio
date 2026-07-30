//! `code_map_*` read-only tools for querying the active project's code map.

use async_trait::async_trait;

use deepagent_core::error::Result;
use deepagent_tools::permission::{PermissionSet, RiskLevel};
use deepagent_tools::{Tool, ToolDescriptor, ToolOutput};

/// Backend adapter implemented by the host over its project-map service.
#[async_trait]
pub trait ProjectMapBackend: Send + Sync {
    /// Project map overview.
    async fn overview(&self) -> Result<serde_json::Value>;
    /// Search map nodes.
    async fn search(&self, query: &str, limit: usize) -> Result<serde_json::Value>;
    /// Query graph neighbors for a node id.
    async fn neighbors(&self, node_id: &str) -> Result<serde_json::Value>;
    /// Query likely dependents impacted by changing a file/node.
    async fn impact(&self, target: &str) -> Result<serde_json::Value>;
    /// (Re)build the project's code index (code graph + map). Used by the
    /// `code_map_refresh` tool so the model can build/refresh the map itself
    /// instead of being stuck when it is missing or stale.
    async fn refresh(&self) -> Result<serde_json::Value>;
}

/// Tool name for overview.
pub const CODE_MAP_OVERVIEW_TOOL_NAME: &str = "code_map_overview";
/// Tool name for search.
pub const CODE_MAP_SEARCH_TOOL_NAME: &str = "code_map_search";
/// Tool name for neighbor lookup.
pub const CODE_MAP_NEIGHBORS_TOOL_NAME: &str = "code_map_neighbors";
/// Tool name for impact lookup.
pub const CODE_MAP_IMPACT_TOOL_NAME: &str = "code_map_impact";
/// Tool name for refresh/index build.
pub const CODE_MAP_REFRESH_TOOL_NAME: &str = "code_map_refresh";

const DEFAULT_LIMIT: usize = 8;
const MAX_LIMIT: usize = 30;

/// `code_map_overview` — compact active project map summary.
pub struct CodeMapOverviewTool<B: ProjectMapBackend> {
    backend: B,
}

impl<B: ProjectMapBackend> CodeMapOverviewTool<B> {
    /// Build the tool.
    pub fn new(backend: B) -> Self {
        Self { backend }
    }
}

#[async_trait]
impl<B: ProjectMapBackend> Tool for CodeMapOverviewTool<B> {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: CODE_MAP_OVERVIEW_TOOL_NAME.into(),
            description: "Inspect the active project's code map before broad file reads. \
                Returns project-map status, counts, languages, frameworks, and complex nodes."
                .into(),
            parameters: serde_json::json!({ "type": "object", "properties": {} }),
            risk: RiskLevel::Safe,
            required_permissions: PermissionSet::read_only(),
        }
    }

    async fn invoke(&self, _args: serde_json::Value) -> Result<ToolOutput> {
        let value = self.backend.overview().await?;
        Ok(ToolOutput::success(value))
    }
}

/// `code_map_search` — find relevant files/functions/classes in the project map.
pub struct CodeMapSearchTool<B: ProjectMapBackend> {
    backend: B,
}

impl<B: ProjectMapBackend> CodeMapSearchTool<B> {
    /// Build the tool.
    pub fn new(backend: B) -> Self {
        Self { backend }
    }
}

#[async_trait]
impl<B: ProjectMapBackend> Tool for CodeMapSearchTool<B> {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: CODE_MAP_SEARCH_TOOL_NAME.into(),
            description: "Search the active project's code map for relevant files, functions, \
                classes, modules, tags, and summaries. Use this before glob/grep/read_file when \
                locating code in a project."
                .into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string" },
                    "limit": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 30,
                        "description": "Max hits to return (default 8)."
                    }
                },
                "required": ["query"]
            }),
            risk: RiskLevel::Safe,
            required_permissions: PermissionSet::read_only(),
        }
    }

    async fn invoke(&self, args: serde_json::Value) -> Result<ToolOutput> {
        let Some(query) = args.get("query").and_then(|v| v.as_str()) else {
            return Ok(ToolOutput::failure("missing 'query'"));
        };
        if query.trim().is_empty() {
            return Ok(ToolOutput::failure("'query' must not be empty"));
        }
        let limit = args
            .get("limit")
            .and_then(|v| v.as_u64())
            .map(|n| (n as usize).clamp(1, MAX_LIMIT))
            .unwrap_or(DEFAULT_LIMIT);
        let value = self.backend.search(query, limit).await?;
        Ok(ToolOutput::success(value))
    }
}

/// `code_map_neighbors` — inspect upstream/downstream relationships.
pub struct CodeMapNeighborsTool<B: ProjectMapBackend> {
    backend: B,
}

impl<B: ProjectMapBackend> CodeMapNeighborsTool<B> {
    /// Build the tool.
    pub fn new(backend: B) -> Self {
        Self { backend }
    }
}

#[async_trait]
impl<B: ProjectMapBackend> Tool for CodeMapNeighborsTool<B> {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: CODE_MAP_NEIGHBORS_TOOL_NAME.into(),
            description: "Given a project-map node id, return imports/imported_by/calls/called_by \
                relationships. Use this to understand dependencies before editing."
                .into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "node_id": { "type": "string", "description": "Project-map node id, e.g. file:src/App.tsx." }
                },
                "required": ["node_id"]
            }),
            risk: RiskLevel::Safe,
            required_permissions: PermissionSet::read_only(),
        }
    }

    async fn invoke(&self, args: serde_json::Value) -> Result<ToolOutput> {
        let Some(node_id) = args.get("node_id").and_then(|v| v.as_str()) else {
            return Ok(ToolOutput::failure("missing 'node_id'"));
        };
        let value = self.backend.neighbors(node_id).await?;
        Ok(ToolOutput::success(value))
    }
}

/// `code_map_impact` — inspect likely dependents affected by a change.
pub struct CodeMapImpactTool<B: ProjectMapBackend> {
    backend: B,
}

impl<B: ProjectMapBackend> CodeMapImpactTool<B> {
    /// Build the tool.
    pub fn new(backend: B) -> Self {
        Self { backend }
    }
}

#[async_trait]
impl<B: ProjectMapBackend> Tool for CodeMapImpactTool<B> {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: CODE_MAP_IMPACT_TOOL_NAME.into(),
            description: "Given a file path or project-map node id, return likely direct and \
                indirect dependents. Use this before changing shared or complex files."
                .into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "target": { "type": "string", "description": "File path or node id." }
                },
                "required": ["target"]
            }),
            risk: RiskLevel::Safe,
            required_permissions: PermissionSet::read_only(),
        }
    }

    async fn invoke(&self, args: serde_json::Value) -> Result<ToolOutput> {
        let Some(target) = args.get("target").and_then(|v| v.as_str()) else {
            return Ok(ToolOutput::failure("missing 'target'"));
        };
        let value = self.backend.impact(target).await?;
        Ok(ToolOutput::success(value))
    }
}

/// `code_map_refresh` — (re)build the project's code index/map on demand.
///
/// The index is also built lazily on first use, but this lets the model force a
/// rebuild after large edits or when a tool reports the map is missing/stale.
pub struct CodeMapRefreshTool<B: ProjectMapBackend> {
    backend: B,
}

impl<B: ProjectMapBackend> CodeMapRefreshTool<B> {
    /// Build the tool.
    pub fn new(backend: B) -> Self {
        Self { backend }
    }
}

#[async_trait]
impl<B: ProjectMapBackend> Tool for CodeMapRefreshTool<B> {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: CODE_MAP_REFRESH_TOOL_NAME.into(),
            description: "(Re)build the active project's code index and map (tree-sitter symbol \
                graph). Call this once if code_map_*/codegraph_* reported the map is missing, or \
                after large edits to refresh it. Incremental after the first build; safe and \
                read-only with respect to your source files."
                .into(),
            parameters: serde_json::json!({ "type": "object", "properties": {} }),
            risk: RiskLevel::Safe,
            required_permissions: PermissionSet::read_only(),
        }
    }

    async fn invoke(&self, _args: serde_json::Value) -> Result<ToolOutput> {
        let value = self.backend.refresh().await?;
        Ok(ToolOutput::success(value))
    }
}
