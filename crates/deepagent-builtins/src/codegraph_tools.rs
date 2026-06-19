//! `codegraph_*` read-only AI tools over the native code graph.

use async_trait::async_trait;

use deepagent_codegraph::error_locator::{parse as parse_error_frames, Frame};
use deepagent_core::error::Result;
use deepagent_tools::permission::{PermissionSet, RiskLevel};
use deepagent_tools::{Tool, ToolDescriptor, ToolOutput};

/// Backend adapter implemented by the host over `deepagent-codegraph`.
#[async_trait]
pub trait CodeGraphBackend: Send + Sync {
    /// Full-text/symbol search.
    async fn search(
        &self,
        query: &str,
        kind: Option<String>,
        limit: usize,
    ) -> Result<serde_json::Value>;
    /// Symbol exploration.
    async fn explore(
        &self,
        symbols: &[String],
        budget: serde_json::Value,
    ) -> Result<serde_json::Value>;
    /// Direct callers.
    async fn callers(&self, symbol: &str, limit: usize) -> Result<serde_json::Value>;
    /// Direct callees.
    async fn callees(&self, symbol: &str, limit: usize) -> Result<serde_json::Value>;
    /// Change impact.
    async fn impact(&self, symbol: &str, depth: usize) -> Result<serde_json::Value>;
    /// Node detail.
    async fn node(&self, target: &str) -> Result<serde_json::Value>;
    /// Symbol at a file/line.
    async fn node_at_location(&self, file: &str, line: u32) -> Result<serde_json::Value>;
    /// Locate frames from raw text. Backends may enrich this with source,
    /// callers/callees, imports, and project/external classification.
    async fn locate(&self, text: &str) -> Result<serde_json::Value> {
        let frames = parse_error_frames(text);
        let mut located = Vec::new();
        for frame in &frames {
            let node = self.node_at_location(&frame.file, frame.line).await?;
            located.push(serde_json::json!({ "frame": frame_to_json(frame), "node": node }));
        }
        Ok(
            serde_json::json!({ "frames": frames.iter().map(frame_to_json).collect::<Vec<_>>(), "located": located }),
        )
    }
}

/// Tool name.
pub const CODEGRAPH_SEARCH_TOOL_NAME: &str = "codegraph_search";
/// Tool name.
pub const CODEGRAPH_EXPLORE_TOOL_NAME: &str = "codegraph_explore";
/// Tool name.
pub const CODEGRAPH_CALLERS_TOOL_NAME: &str = "codegraph_callers";
/// Tool name.
pub const CODEGRAPH_CALLEES_TOOL_NAME: &str = "codegraph_callees";
/// Tool name.
pub const CODEGRAPH_IMPACT_TOOL_NAME: &str = "codegraph_impact";
/// Tool name.
pub const CODEGRAPH_NODE_TOOL_NAME: &str = "codegraph_node";
/// Tool name.
pub const CODEGRAPH_LOCATE_TOOL_NAME: &str = "codegraph_locate";

const DEFAULT_LIMIT: usize = 8;
const MAX_LIMIT: usize = 50;
const DEFAULT_DEPTH: usize = 3;
const MAX_DEPTH: usize = 8;

/// Search code graph symbols.
pub struct CodeGraphSearchTool<B: CodeGraphBackend> {
    backend: B,
}

impl<B: CodeGraphBackend> CodeGraphSearchTool<B> {
    /// Build the tool.
    pub fn new(backend: B) -> Self {
        Self { backend }
    }
}

#[async_trait]
impl<B: CodeGraphBackend> Tool for CodeGraphSearchTool<B> {
    fn descriptor(&self) -> ToolDescriptor {
        descriptor(
            CODEGRAPH_SEARCH_TOOL_NAME,
            "Search indexed code symbols using the native code graph. Prefer this over grep when locating functions, classes, methods, or modules.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string" },
                    "kind": { "type": "string", "description": "Optional node kind such as function, class, method, file." },
                    "limit": { "type": "integer", "minimum": 1, "maximum": MAX_LIMIT }
                },
                "required": ["query"]
            }),
        )
    }

    async fn invoke(&self, args: serde_json::Value) -> Result<ToolOutput> {
        let Some(query) = required_str(&args, "query") else {
            return Ok(ToolOutput::failure("missing 'query'"));
        };
        if query.trim().is_empty() {
            return Ok(ToolOutput::failure("'query' must not be empty"));
        }
        let value = self
            .backend
            .search(query, optional_string(&args, "kind"), limit_arg(&args))
            .await?;
        Ok(ToolOutput::success(value))
    }
}

/// Explore symbols and call flow.
pub struct CodeGraphExploreTool<B: CodeGraphBackend> {
    backend: B,
}

impl<B: CodeGraphBackend> CodeGraphExploreTool<B> {
    /// Build the tool.
    pub fn new(backend: B) -> Self {
        Self { backend }
    }
}

#[async_trait]
impl<B: CodeGraphBackend> Tool for CodeGraphExploreTool<B> {
    fn descriptor(&self) -> ToolDescriptor {
        descriptor(
            CODEGRAPH_EXPLORE_TOOL_NAME,
            "Explore how one or more symbols work by returning call-flow hops and grouped source snippets. Prefer this before read_file for code understanding.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "symbols": { "type": "array", "items": { "type": "string" }, "minItems": 1 },
                    "budget": { "type": "object", "description": "Optional ExploreBudget override." }
                },
                "required": ["symbols"]
            }),
        )
    }

    async fn invoke(&self, args: serde_json::Value) -> Result<ToolOutput> {
        let Some(symbols) = args.get("symbols").and_then(|v| v.as_array()) else {
            return Ok(ToolOutput::failure("missing 'symbols' array"));
        };
        let symbols: Vec<String> = symbols
            .iter()
            .filter_map(|v| v.as_str().map(str::trim))
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect();
        if symbols.is_empty() {
            return Ok(ToolOutput::failure(
                "'symbols' must contain at least one string",
            ));
        }
        let budget = args
            .get("budget")
            .cloned()
            .unwrap_or_else(|| serde_json::json!({}));
        Ok(ToolOutput::success(
            self.backend.explore(&symbols, budget).await?,
        ))
    }
}

/// Inspect direct callers.
pub struct CodeGraphCallersTool<B: CodeGraphBackend> {
    backend: B,
}

impl<B: CodeGraphBackend> CodeGraphCallersTool<B> {
    /// Build the tool.
    pub fn new(backend: B) -> Self {
        Self { backend }
    }
}

#[async_trait]
impl<B: CodeGraphBackend> Tool for CodeGraphCallersTool<B> {
    fn descriptor(&self) -> ToolDescriptor {
        descriptor(
            CODEGRAPH_CALLERS_TOOL_NAME,
            "Find direct callers of a symbol or node id using calls edges.",
            symbol_limit_schema("symbol"),
        )
    }

    async fn invoke(&self, args: serde_json::Value) -> Result<ToolOutput> {
        let Some(symbol) = required_str(&args, "symbol") else {
            return Ok(ToolOutput::failure("missing 'symbol'"));
        };
        Ok(ToolOutput::success(
            self.backend.callers(symbol, limit_arg(&args)).await?,
        ))
    }
}

/// Inspect direct callees.
pub struct CodeGraphCalleesTool<B: CodeGraphBackend> {
    backend: B,
}

impl<B: CodeGraphBackend> CodeGraphCalleesTool<B> {
    /// Build the tool.
    pub fn new(backend: B) -> Self {
        Self { backend }
    }
}

#[async_trait]
impl<B: CodeGraphBackend> Tool for CodeGraphCalleesTool<B> {
    fn descriptor(&self) -> ToolDescriptor {
        descriptor(
            CODEGRAPH_CALLEES_TOOL_NAME,
            "Find direct callees of a symbol or node id using calls edges.",
            symbol_limit_schema("symbol"),
        )
    }

    async fn invoke(&self, args: serde_json::Value) -> Result<ToolOutput> {
        let Some(symbol) = required_str(&args, "symbol") else {
            return Ok(ToolOutput::failure("missing 'symbol'"));
        };
        Ok(ToolOutput::success(
            self.backend.callees(symbol, limit_arg(&args)).await?,
        ))
    }
}

/// Inspect change impact.
pub struct CodeGraphImpactTool<B: CodeGraphBackend> {
    backend: B,
}

impl<B: CodeGraphBackend> CodeGraphImpactTool<B> {
    /// Build the tool.
    pub fn new(backend: B) -> Self {
        Self { backend }
    }
}

#[async_trait]
impl<B: CodeGraphBackend> Tool for CodeGraphImpactTool<B> {
    fn descriptor(&self) -> ToolDescriptor {
        descriptor(
            CODEGRAPH_IMPACT_TOOL_NAME,
            "Find direct and indirect callers impacted by changing a symbol.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "symbol": { "type": "string" },
                    "depth": { "type": "integer", "minimum": 1, "maximum": MAX_DEPTH }
                },
                "required": ["symbol"]
            }),
        )
    }

    async fn invoke(&self, args: serde_json::Value) -> Result<ToolOutput> {
        let Some(symbol) = required_str(&args, "symbol") else {
            return Ok(ToolOutput::failure("missing 'symbol'"));
        };
        let depth = args
            .get("depth")
            .and_then(|v| v.as_u64())
            .map(|n| (n as usize).clamp(1, MAX_DEPTH))
            .unwrap_or(DEFAULT_DEPTH);
        Ok(ToolOutput::success(
            self.backend.impact(symbol, depth).await?,
        ))
    }
}

/// Inspect one node.
pub struct CodeGraphNodeTool<B: CodeGraphBackend> {
    backend: B,
}

impl<B: CodeGraphBackend> CodeGraphNodeTool<B> {
    /// Build the tool.
    pub fn new(backend: B) -> Self {
        Self { backend }
    }
}

#[async_trait]
impl<B: CodeGraphBackend> Tool for CodeGraphNodeTool<B> {
    fn descriptor(&self) -> ToolDescriptor {
        descriptor(
            CODEGRAPH_NODE_TOOL_NAME,
            "Return node details, callers, callees, and imports for a symbol or node id.",
            serde_json::json!({
                "type": "object",
                "properties": { "target": { "type": "string" } },
                "required": ["target"]
            }),
        )
    }

    async fn invoke(&self, args: serde_json::Value) -> Result<ToolOutput> {
        let Some(target) = required_str(&args, "target") else {
            return Ok(ToolOutput::failure("missing 'target'"));
        };
        Ok(ToolOutput::success(self.backend.node(target).await?))
    }
}

/// Locate error stack frames in the code graph.
pub struct CodeGraphLocateTool<B: CodeGraphBackend> {
    backend: B,
}

impl<B: CodeGraphBackend> CodeGraphLocateTool<B> {
    /// Build the tool.
    pub fn new(backend: B) -> Self {
        Self { backend }
    }
}

#[async_trait]
impl<B: CodeGraphBackend> Tool for CodeGraphLocateTool<B> {
    fn descriptor(&self) -> ToolDescriptor {
        descriptor(
            CODEGRAPH_LOCATE_TOOL_NAME,
            "Locate pasted error text or a file:line reference in the code graph, returning matching symbols and call context. Use this first for stack traces or screenshots with errors.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "text": { "type": "string", "description": "Raw error text, stack trace, or file:line reference." }
                },
                "required": ["text"]
            }),
        )
    }

    async fn invoke(&self, args: serde_json::Value) -> Result<ToolOutput> {
        let Some(text) = required_str(&args, "text") else {
            return Ok(ToolOutput::failure("missing 'text'"));
        };
        if text.trim().is_empty() {
            return Ok(ToolOutput::failure("'text' must not be empty"));
        }
        Ok(ToolOutput::success(self.backend.locate(text).await?))
    }
}

fn descriptor(name: &str, description: &str, parameters: serde_json::Value) -> ToolDescriptor {
    ToolDescriptor {
        name: name.to_string(),
        description: description.to_string(),
        parameters,
        risk: RiskLevel::Safe,
        required_permissions: PermissionSet::read_only(),
    }
}

fn symbol_limit_schema(field: &str) -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            field: { "type": "string" },
            "limit": { "type": "integer", "minimum": 1, "maximum": MAX_LIMIT }
        },
        "required": [field]
    })
}

fn required_str<'a>(args: &'a serde_json::Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(|v| v.as_str())
}

fn optional_string(args: &serde_json::Value, key: &str) -> Option<String> {
    args.get(key)
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn limit_arg(args: &serde_json::Value) -> usize {
    args.get("limit")
        .and_then(|v| v.as_u64())
        .map(|n| (n as usize).clamp(1, MAX_LIMIT))
        .unwrap_or(DEFAULT_LIMIT)
}

fn frame_to_json(frame: &Frame) -> serde_json::Value {
    serde_json::json!({
        "file": frame.file,
        "line": frame.line,
        "col": frame.col,
        "symbol": frame.symbol,
        "errorCode": frame.error_code,
        "isProject": frame.is_project,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[derive(Default, Clone)]
    struct StubBackend {
        calls: Arc<Mutex<Vec<String>>>,
    }

    #[async_trait]
    impl CodeGraphBackend for StubBackend {
        async fn search(
            &self,
            query: &str,
            _kind: Option<String>,
            limit: usize,
        ) -> Result<serde_json::Value> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("search:{query}:{limit}"));
            Ok(serde_json::json!({ "hits": [query] }))
        }

        async fn explore(
            &self,
            symbols: &[String],
            _budget: serde_json::Value,
        ) -> Result<serde_json::Value> {
            Ok(serde_json::json!({ "symbols": symbols }))
        }

        async fn callers(&self, symbol: &str, _limit: usize) -> Result<serde_json::Value> {
            Ok(serde_json::json!({ "callers": [symbol] }))
        }

        async fn callees(&self, symbol: &str, _limit: usize) -> Result<serde_json::Value> {
            Ok(serde_json::json!({ "callees": [symbol] }))
        }

        async fn impact(&self, symbol: &str, depth: usize) -> Result<serde_json::Value> {
            Ok(serde_json::json!({ "symbol": symbol, "depth": depth }))
        }

        async fn node(&self, target: &str) -> Result<serde_json::Value> {
            Ok(serde_json::json!({ "node": target }))
        }

        async fn node_at_location(&self, file: &str, line: u32) -> Result<serde_json::Value> {
            Ok(serde_json::json!({ "file": file, "line": line }))
        }
    }

    #[test]
    fn descriptors_are_safe_and_read_only() {
        let tools: Vec<Box<dyn Tool>> = vec![
            Box::new(CodeGraphSearchTool::new(StubBackend::default())),
            Box::new(CodeGraphExploreTool::new(StubBackend::default())),
            Box::new(CodeGraphCallersTool::new(StubBackend::default())),
            Box::new(CodeGraphCalleesTool::new(StubBackend::default())),
            Box::new(CodeGraphImpactTool::new(StubBackend::default())),
            Box::new(CodeGraphNodeTool::new(StubBackend::default())),
            Box::new(CodeGraphLocateTool::new(StubBackend::default())),
        ];
        for tool in tools {
            let desc = tool.descriptor();
            assert_eq!(desc.risk, RiskLevel::Safe);
            assert_eq!(desc.required_permissions, PermissionSet::read_only());
        }
    }

    #[tokio::test]
    async fn search_invokes_backend() {
        let backend = StubBackend::default();
        let calls = backend.calls.clone();
        let out = CodeGraphSearchTool::new(backend)
            .invoke(serde_json::json!({ "query": "handler", "limit": 2 }))
            .await
            .unwrap();
        assert!(out.ok);
        assert_eq!(calls.lock().unwrap()[0], "search:handler:2");
    }

    #[tokio::test]
    async fn locate_parses_and_resolves_frames() {
        let out = CodeGraphLocateTool::new(StubBackend::default())
            .invoke(serde_json::json!({ "text": "at handler (src/app.ts:12:7)" }))
            .await
            .unwrap();
        assert!(out.ok);
        assert_eq!(out.value["located"][0]["node"]["file"], "src/app.ts");
    }

    #[tokio::test]
    async fn invalid_args_are_failures() {
        let out = CodeGraphExploreTool::new(StubBackend::default())
            .invoke(serde_json::json!({ "symbols": [] }))
            .await
            .unwrap();
        assert!(!out.ok);
    }
}
