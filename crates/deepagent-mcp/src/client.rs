//! The MCP client: drives the JSON-RPC lifecycle over a [`McpTransport`].
//!
//! Lifecycle (per the MCP spec / Claude Code): `initialize` → `tools/list`
//! (discovery) → `tools/call` (invocation). The client assigns monotonic
//! request ids and unwraps JSON-RPC errors into [`CoreError`].

use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use deepagent_core::error::{CoreError, Result};
use deepagent_tools::ToolExecutionContext;

use crate::protocol::{JsonRpcRequest, McpToolDef, ToolCallResult, MCP_PROTOCOL_VERSION};
use crate::transport::McpTransport;

const MAX_TOOLS_LIST_PAGES: usize = 100;
const MAX_TOOLS_LIST_ITEMS: usize = 2_048;
const MAX_TOOLS_LIST_CURSOR_BYTES: usize = 64 * 1024;

/// Decode the page envelope before individual definitions so a missing required
/// inputSchema can be isolated without dropping valid sibling tools. Count raw
/// entries before filtering to preserve the catalog limit across pages.
#[derive(serde::Deserialize)]
struct ToolsListPage {
    #[serde(default)]
    tools: Vec<serde_json::Value>,
    #[serde(rename = "nextCursor", default)]
    next_cursor: Option<String>,
}

/// The required business result for the MCP initialize handshake.
///
/// Keep this DTO private to the handshake seam: callers still receive the raw
/// result for optional fields such as `instructions`, while readiness is gated
/// by one strongly typed validation path.
#[derive(Debug, serde::Deserialize)]
struct InitializeResult {
    #[serde(rename = "protocolVersion")]
    protocol_version: String,
    capabilities: serde_json::Map<String, serde_json::Value>,
    #[serde(rename = "serverInfo")]
    server_info: InitializeServerInfo,
}

#[derive(Debug, serde::Deserialize)]
struct InitializeServerInfo {
    name: String,
    version: String,
}

fn validate_initialize_result(result: &serde_json::Value) -> Result<()> {
    let InitializeResult {
        protocol_version,
        capabilities,
        server_info: InitializeServerInfo { name, version },
    } = serde_json::from_value(result.clone()).map_err(|error| {
        CoreError::invalid(format!(
            "MCP initialize result has an invalid shape: {error}"
        ))
    })?;

    if protocol_version != MCP_PROTOCOL_VERSION {
        return Err(CoreError::invalid(format!(
            "MCP initialize protocol version mismatch: expected {MCP_PROTOCOL_VERSION}, got {protocol_version}"
        )));
    }

    // The field types above are the protocol gate. Consume all required
    // values so future DTO changes cannot accidentally leave a required field
    // unchecked.
    let _ = (capabilities, name, version);
    Ok(())
}

/// An MCP client bound to one server via a transport.
pub struct McpClient {
    transport: Arc<dyn McpTransport>,
    next_id: AtomicU64,
}

impl McpClient {
    pub fn connection_status(&self) -> crate::McpConnectionStatus {
        self.transport.connection_status()
    }

    pub(crate) async fn park_after_liveness_failure(&self, reason: &str) {
        self.transport.park_after_liveness_failure(reason).await;
    }

    /// Build a client over a transport.
    pub fn new(transport: Arc<dyn McpTransport>) -> Self {
        Self {
            transport,
            next_id: AtomicU64::new(1),
        }
    }

    fn next_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    async fn call(
        &self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> Result<serde_json::Value> {
        self.call_inner(method, params, None).await
    }

    async fn call_inner(
        &self,
        method: &str,
        params: Option<serde_json::Value>,
        context: Option<ToolExecutionContext>,
    ) -> Result<serde_json::Value> {
        let req = JsonRpcRequest::new(self.next_id(), method, params);
        let resp = match context {
            Some(context) => self.transport.send_with_context(&req, context).await?,
            None => self.transport.send(&req).await?,
        };
        if let Some(err) = resp.error {
            return Err(CoreError::other(format!(
                "MCP error {} on {method}: {}",
                err.code, err.message
            )));
        }
        resp.result
            .ok_or_else(|| CoreError::other(format!("MCP response for {method} had no result")))
    }

    /// Perform the `initialize` handshake, advertising our protocol version and
    /// client info. The required `notifications/initialized` notification is
    /// sent only after a valid initialize result arrives. Returns the server's
    /// `initialize` result.
    pub async fn initialize(&self, client_name: &str) -> Result<serde_json::Value> {
        let params = serde_json::json!({
            "protocolVersion": MCP_PROTOCOL_VERSION,
            "capabilities": { "tools": {} },
            "clientInfo": { "name": client_name, "version": env!("CARGO_PKG_VERSION") }
        });
        let result = match self.call("initialize", Some(params)).await {
            Ok(result) => result,
            Err(error) => {
                if let Err(close_error) = self.transport.close().await {
                    tracing::warn!(error = %close_error, "failed to close MCP transport after initialize error");
                }
                return Err(error);
            }
        };
        if let Err(error) = validate_initialize_result(&result) {
            if let Err(close_error) = self.transport.close().await {
                tracing::warn!(error = %close_error, "failed to close MCP transport after invalid initialize result");
            }
            return Err(error);
        }
        if let Err(error) = self
            .transport
            .notify("notifications/initialized", None)
            .await
        {
            if let Err(close_error) = self.transport.close().await {
                tracing::warn!(error = %close_error, "failed to close MCP transport after initialized notification error");
            }
            return Err(error);
        }
        Ok(result)
    }

    /// Discover the tools the server provides (`tools/list`).
    pub async fn list_tools(&self) -> Result<Vec<McpToolDef>> {
        let mut tools = Vec::new();
        let mut raw_names = HashSet::new();
        let mut cursor = None;
        let mut seen_cursors = HashSet::new();
        let mut item_count = 0;

        for _ in 0..MAX_TOOLS_LIST_PAGES {
            let params = cursor
                .as_ref()
                .map(|cursor| serde_json::json!({ "cursor": cursor }));
            let result = self.call("tools/list", params).await?;
            let parsed: ToolsListPage = serde_json::from_value(result)?;

            if parsed.tools.len() > MAX_TOOLS_LIST_ITEMS.saturating_sub(item_count) {
                return Err(CoreError::invalid(format!(
                    "MCP tools/list exceeded the catalog limit of {MAX_TOOLS_LIST_ITEMS} tools"
                )));
            }
            item_count += parsed.tools.len();
            for definition in parsed.tools {
                // Identity must be unambiguous across the entire paginated
                // response, even when a definition will be schema-filtered.
                if let Some(name) = definition.get("name").and_then(|name| name.as_str()) {
                    if !raw_names.insert(name.to_owned()) {
                        return Err(CoreError::invalid(
                            "MCP tools/list contains duplicate raw tool identity",
                        ));
                    }
                }
                if definition.is_object() && definition.get("inputSchema").is_none() {
                    tracing::warn!(
                        tool = definition.get("name").and_then(|name| name.as_str()),
                        "ignoring MCP tool: required inputSchema is missing"
                    );
                    continue;
                }
                tools.push(serde_json::from_value(definition)?);
            }

            let Some(next_cursor) = parsed.next_cursor else {
                return Ok(tools);
            };
            if next_cursor.len() > MAX_TOOLS_LIST_CURSOR_BYTES {
                return Err(CoreError::invalid(format!(
                    "MCP tools/list returned a cursor exceeding {MAX_TOOLS_LIST_CURSOR_BYTES} bytes"
                )));
            }
            if !seen_cursors.insert(next_cursor.clone()) {
                return Err(CoreError::invalid(
                    "MCP tools/list returned a repeated pagination cursor",
                ));
            }
            cursor = Some(next_cursor);
        }

        Err(CoreError::invalid(format!(
            "MCP tools/list exceeded the pagination limit of {MAX_TOOLS_LIST_PAGES} pages"
        )))
    }

    /// Invoke a tool by its server-local name (`tools/call`).
    pub async fn call_tool(
        &self,
        name: &str,
        arguments: serde_json::Value,
    ) -> Result<ToolCallResult> {
        let params = serde_json::json!({ "name": name, "arguments": arguments });
        let result = self.call("tools/call", Some(params)).await?;
        let parsed: ToolCallResult = serde_json::from_value(result)?;
        Ok(parsed)
    }

    /// Invoke a remote tool with the current run's cancellation controls.
    pub async fn call_tool_with_context(
        &self,
        name: &str,
        arguments: serde_json::Value,
        context: ToolExecutionContext,
    ) -> Result<ToolCallResult> {
        let params = serde_json::json!({ "name": name, "arguments": arguments });
        let result = self
            .call_inner("tools/call", Some(params), Some(context))
            .await?;
        let parsed: ToolCallResult = serde_json::from_value(result)?;
        Ok(parsed)
    }

    /// Liveness check via the MCP spec's `ping` utility
    /// (`basic/utilities/ping`): a no-param request the receiver MUST answer
    /// promptly with an empty result. Any successful reply means the
    /// connection is alive; a transport error (or a caller-imposed timeout)
    /// means it should be treated as stale.
    pub async fn ping(&self) -> Result<()> {
        self.call("ping", None).await.map(|_| ())
    }

    /// Close the underlying transport.
    pub async fn close(&self) -> Result<()> {
        self.transport.close().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::MockTransport;

    #[tokio::test]
    async fn h147_duplicate_raw_identity_on_later_page_rejects_the_entire_catalog() {
        use crate::protocol::JsonRpcResponse;
        use async_trait::async_trait;

        struct DuplicateServer;
        #[async_trait]
        impl McpTransport for DuplicateServer {
            async fn send(&self, req: &JsonRpcRequest) -> Result<JsonRpcResponse> {
                let result = if req.params.is_none() {
                    serde_json::json!({"tools":[{"name":"same","inputSchema":{"type":"object"}}],"nextCursor":"next"})
                } else {
                    serde_json::json!({"tools":[{"name":"same","inputSchema":{"type":"object"}}]})
                };
                Ok(JsonRpcResponse {
                    jsonrpc: "2.0".into(),
                    id: req.id,
                    result: Some(result),
                    error: None,
                })
            }
        }
        let error = McpClient::new(Arc::new(DuplicateServer))
            .list_tools()
            .await
            .unwrap_err();
        assert!(error.to_string().contains("duplicate raw tool identity"));
    }

    fn client_with(transport: MockTransport) -> McpClient {
        McpClient::new(Arc::new(transport))
    }

    #[tokio::test]
    async fn initialize_handshake() {
        let t = Arc::new(MockTransport::new().with_result(
            "initialize",
            serde_json::json!({
                "protocolVersion": MCP_PROTOCOL_VERSION,
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "mock-server", "version": "1"}
            }),
        ));
        let client = McpClient::new(t.clone());
        let res = client.initialize("deepagent").await.unwrap();
        assert_eq!(res["protocolVersion"], MCP_PROTOCOL_VERSION);
        assert_eq!(
            t.notifications(),
            vec![("notifications/initialized".into(), None)]
        );
    }

    #[tokio::test]
    async fn initialize_rejects_invalid_result_before_initialized_notification() {
        let invalid_results = [
            serde_json::json!({
                "protocolVersion": MCP_PROTOCOL_VERSION,
                "capabilities": [],
                "serverInfo": {"name": "mock-server", "version": "1"}
            }),
            serde_json::json!({
                "protocolVersion": MCP_PROTOCOL_VERSION,
                "capabilities": {},
                "serverInfo": {"name": 42, "version": "1"}
            }),
            serde_json::json!({
                "protocolVersion": "2099-01-01",
                "capabilities": {},
                "serverInfo": {"name": "mock-server", "version": "1"}
            }),
        ];

        for result in invalid_results {
            let transport = Arc::new(MockTransport::new().with_result("initialize", result));
            let client = McpClient::new(transport.clone());
            assert!(client.initialize("deepagent").await.is_err());
            assert!(transport.notifications().is_empty());
        }
    }

    #[tokio::test]
    async fn lists_tools() {
        let t = MockTransport::new().with_result(
            "tools/list",
            serde_json::json!({"tools":[
                {"name":"search","description":"search","inputSchema":{"type":"object"}},
                {"name":"create","description":"create","inputSchema":{"type":"object"}}
            ]}),
        );
        let client = client_with(t);
        let tools = client.list_tools().await.unwrap();
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0].name, "search");
    }

    #[tokio::test]
    async fn missing_schema_entries_still_count_toward_catalog_limit() {
        let definitions = (0..=MAX_TOOLS_LIST_ITEMS)
            .map(|index| serde_json::json!({"name":format!("missing-{index}")}))
            .collect::<Vec<_>>();
        let client = client_with(
            MockTransport::new()
                .with_result("tools/list", serde_json::json!({"tools":definitions})),
        );
        assert!(client
            .list_tools()
            .await
            .unwrap_err()
            .to_string()
            .contains("catalog limit"));
    }

    #[tokio::test]
    async fn audit_tool_discovery_reads_all_pages() {
        use crate::protocol::JsonRpcResponse;
        use async_trait::async_trait;
        use std::sync::Mutex;

        #[derive(Default)]
        struct PaginatedServer(Mutex<Vec<Option<serde_json::Value>>>);

        #[async_trait]
        impl McpTransport for PaginatedServer {
            async fn send(&self, req: &JsonRpcRequest) -> Result<JsonRpcResponse> {
                assert_eq!(req.method, "tools/list");
                self.0.lock().unwrap().push(req.params.clone());
                let cursor = req.params.as_ref().and_then(|p| p["cursor"].as_str());
                let result = match cursor {
                    None => serde_json::json!({
                        "tools": [{"name":"first", "inputSchema":{"type":"object"}}],
                        "nextCursor":"opaque-page-two"
                    }),
                    Some("opaque-page-two") => serde_json::json!({
                        "tools": [{"name":"second", "inputSchema":{"type":"object"}}]
                    }),
                    _ => panic!("client changed the opaque cursor"),
                };
                Ok(JsonRpcResponse {
                    jsonrpc: "2.0".into(),
                    id: req.id,
                    result: Some(result),
                    error: None,
                })
            }
        }

        let server = Arc::new(PaginatedServer::default());
        let client = McpClient::new(server.clone());
        let tools = client.list_tools().await.unwrap();
        let names: Vec<_> = tools.iter().map(|tool| tool.name.as_str()).collect();
        eprintln!(
            "audit MCP pagination: names={names:?}, requests={:?}",
            server.0.lock().unwrap()
        );
        assert_eq!(names, ["first", "second"], "later pages must not disappear");
    }

    #[tokio::test]
    async fn list_tools_follows_empty_pages_and_preserves_opaque_cursor() {
        use crate::protocol::JsonRpcResponse;
        use async_trait::async_trait;
        use std::sync::Mutex;

        struct EmptyPageServer(Mutex<Vec<Option<String>>>);

        #[async_trait]
        impl McpTransport for EmptyPageServer {
            async fn send(&self, req: &JsonRpcRequest) -> Result<JsonRpcResponse> {
                let cursor = req
                    .params
                    .as_ref()
                    .and_then(|params| params["cursor"].as_str())
                    .map(str::to_string);
                self.0.lock().unwrap().push(cursor.clone());
                let result = match cursor.as_deref() {
                    None => serde_json::json!({"tools":[],"nextCursor":"cursor/empty"}),
                    Some("cursor/empty") => serde_json::json!({
                        "tools":[{"name":"after-empty","inputSchema":{"type":"object"}}]
                    }),
                    _ => panic!("client changed the opaque cursor"),
                };
                Ok(JsonRpcResponse {
                    jsonrpc: "2.0".into(),
                    id: req.id,
                    result: Some(result),
                    error: None,
                })
            }
        }

        let server = Arc::new(EmptyPageServer(Mutex::new(Vec::new())));
        let tools = McpClient::new(server.clone()).list_tools().await.unwrap();
        assert_eq!(
            tools
                .iter()
                .map(|tool| tool.name.as_str())
                .collect::<Vec<_>>(),
            ["after-empty"]
        );
        assert_eq!(
            server.0.lock().unwrap().as_slice(),
            &[None, Some("cursor/empty".into())]
        );
    }

    #[tokio::test]
    async fn list_tools_rejects_repeated_cursor() {
        use crate::protocol::JsonRpcResponse;
        use async_trait::async_trait;

        struct RepeatingServer;

        #[async_trait]
        impl McpTransport for RepeatingServer {
            async fn send(&self, req: &JsonRpcRequest) -> Result<JsonRpcResponse> {
                Ok(JsonRpcResponse {
                    jsonrpc: "2.0".into(),
                    id: req.id,
                    result: Some(serde_json::json!({
                        "tools": [],
                        "nextCursor": "same"
                    })),
                    error: None,
                })
            }
        }

        let error = McpClient::new(Arc::new(RepeatingServer))
            .list_tools()
            .await
            .unwrap_err();
        assert!(error.to_string().contains("repeated pagination cursor"));
    }

    #[tokio::test]
    async fn list_tools_rejects_oversized_cursor() {
        use crate::protocol::JsonRpcResponse;
        use async_trait::async_trait;

        struct OversizedCursorServer;

        #[async_trait]
        impl McpTransport for OversizedCursorServer {
            async fn send(&self, req: &JsonRpcRequest) -> Result<JsonRpcResponse> {
                Ok(JsonRpcResponse {
                    jsonrpc: "2.0".into(),
                    id: req.id,
                    result: Some(serde_json::json!({
                        "tools": [],
                        "nextCursor": "x".repeat(MAX_TOOLS_LIST_CURSOR_BYTES + 1)
                    })),
                    error: None,
                })
            }
        }

        let error = McpClient::new(Arc::new(OversizedCursorServer))
            .list_tools()
            .await
            .unwrap_err();
        assert!(error.to_string().contains("cursor exceeding"));
    }

    #[tokio::test]
    async fn list_tools_rejects_unbounded_page_chain() {
        use crate::protocol::JsonRpcResponse;
        use async_trait::async_trait;
        use std::sync::atomic::{AtomicUsize, Ordering};

        struct EndlessServer(AtomicUsize);

        #[async_trait]
        impl McpTransport for EndlessServer {
            async fn send(&self, req: &JsonRpcRequest) -> Result<JsonRpcResponse> {
                let page = self.0.fetch_add(1, Ordering::Relaxed) + 1;
                assert!(req.params.is_none() || req.params.as_ref().unwrap()["cursor"].is_string());
                Ok(JsonRpcResponse {
                    jsonrpc: "2.0".into(),
                    id: req.id,
                    result: Some(serde_json::json!({
                        "tools": [],
                        "nextCursor": format!("page-{page}")
                    })),
                    error: None,
                })
            }
        }

        let server = Arc::new(EndlessServer(AtomicUsize::new(0)));
        let error = McpClient::new(server.clone())
            .list_tools()
            .await
            .unwrap_err();
        assert!(error.to_string().contains("pagination limit"));
        assert_eq!(server.0.load(Ordering::Relaxed), MAX_TOOLS_LIST_PAGES);
    }

    #[tokio::test]
    async fn list_tools_rejects_catalog_overflow_without_returning_partial_tools() {
        use crate::protocol::JsonRpcResponse;
        use async_trait::async_trait;

        struct OverflowServer;

        #[async_trait]
        impl McpTransport for OverflowServer {
            async fn send(&self, req: &JsonRpcRequest) -> Result<JsonRpcResponse> {
                let tools = (0..=MAX_TOOLS_LIST_ITEMS)
                    .map(|index| serde_json::json!({"name": format!("tool-{index}"), "inputSchema": {}}))
                    .collect::<Vec<_>>();
                Ok(JsonRpcResponse {
                    jsonrpc: "2.0".into(),
                    id: req.id,
                    result: Some(serde_json::json!({"tools": tools})),
                    error: None,
                })
            }
        }

        let error = McpClient::new(Arc::new(OverflowServer))
            .list_tools()
            .await
            .unwrap_err();
        assert!(error.to_string().contains("catalog limit"));
    }

    #[tokio::test]
    async fn calls_tool_and_reads_text() {
        let t = MockTransport::new().with_result(
            "tools/call",
            serde_json::json!({"content":[{"type":"text","text":"result body"}],"isError":false}),
        );
        let client = client_with(t);
        let res = client
            .call_tool("search", serde_json::json!({"q": "rust"}))
            .await
            .unwrap();
        assert_eq!(res.text(), "result body");
        assert!(!res.is_error);
    }

    #[tokio::test]
    async fn ping_round_trips_empty_result() {
        // Spec shape: `ping` -> `{ "result": {} }`.
        let t = MockTransport::new().with_result("ping", serde_json::json!({}));
        let client = client_with(t);
        client.ping().await.unwrap();
    }

    #[tokio::test]
    async fn propagates_jsonrpc_error() {
        // No result registered -> mock returns transport error; but also test an
        // explicit JSON-RPC error path via a transport that returns one.
        use crate::protocol::{JsonRpcError, JsonRpcResponse};
        use async_trait::async_trait;

        struct ErrTransport;
        #[async_trait]
        impl McpTransport for ErrTransport {
            async fn send(&self, req: &JsonRpcRequest) -> Result<JsonRpcResponse> {
                Ok(JsonRpcResponse {
                    jsonrpc: "2.0".into(),
                    id: req.id,
                    result: None,
                    error: Some(JsonRpcError {
                        code: -32601,
                        message: "method not found".into(),
                        data: None,
                    }),
                })
            }
        }
        let client = McpClient::new(Arc::new(ErrTransport));
        let err = client.list_tools().await.unwrap_err();
        assert!(err.to_string().contains("method not found"));
    }
}
