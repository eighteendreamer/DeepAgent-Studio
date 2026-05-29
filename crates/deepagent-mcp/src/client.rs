//! The MCP client: drives the JSON-RPC lifecycle over a [`McpTransport`].
//!
//! Lifecycle (per the MCP spec / Claude Code): `initialize` → `tools/list`
//! (discovery) → `tools/call` (invocation). The client assigns monotonic
//! request ids and unwraps JSON-RPC errors into [`CoreError`].

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use deepagent_core::error::{CoreError, Result};

use crate::protocol::{
    JsonRpcRequest, McpToolDef, ToolCallResult, ToolsListResult, MCP_PROTOCOL_VERSION,
};
use crate::transport::McpTransport;

/// An MCP client bound to one server via a transport.
pub struct McpClient {
    transport: Arc<dyn McpTransport>,
    next_id: AtomicU64,
}

impl McpClient {
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

    async fn call(&self, method: &str, params: Option<serde_json::Value>) -> Result<serde_json::Value> {
        let req = JsonRpcRequest::new(self.next_id(), method, params);
        let resp = self.transport.send(&req).await?;
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
    /// client info. Returns the server's `initialize` result.
    pub async fn initialize(&self, client_name: &str) -> Result<serde_json::Value> {
        let params = serde_json::json!({
            "protocolVersion": MCP_PROTOCOL_VERSION,
            "capabilities": { "tools": {} },
            "clientInfo": { "name": client_name, "version": env!("CARGO_PKG_VERSION") }
        });
        self.call("initialize", Some(params)).await
    }

    /// Discover the tools the server provides (`tools/list`).
    pub async fn list_tools(&self) -> Result<Vec<McpToolDef>> {
        let result = self.call("tools/list", None).await?;
        let parsed: ToolsListResult = serde_json::from_value(result)?;
        Ok(parsed.tools)
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

    /// Close the underlying transport.
    pub async fn close(&self) -> Result<()> {
        self.transport.close().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::MockTransport;

    fn client_with(transport: MockTransport) -> McpClient {
        McpClient::new(Arc::new(transport))
    }

    #[tokio::test]
    async fn initialize_handshake() {
        let t = MockTransport::new().with_result(
            "initialize",
            serde_json::json!({"protocolVersion": MCP_PROTOCOL_VERSION}),
        );
        let client = client_with(t);
        let res = client.initialize("deepagent").await.unwrap();
        assert_eq!(res["protocolVersion"], MCP_PROTOCOL_VERSION);
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
