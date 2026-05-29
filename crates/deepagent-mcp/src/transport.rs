//! MCP transports.
//!
//! A [`McpTransport`] sends a JSON-RPC request and returns the response. The
//! stdio transport spawns the server process and speaks newline-delimited
//! JSON-RPC over stdin/stdout (the standard MCP stdio framing). Network
//! transports (SSE/HTTP/WS) are represented by their config and wired to a real
//! HTTP/WS client behind a feature in a later iteration; the trait keeps the
//! client code transport-agnostic. A [`MockTransport`] enables full offline
//! testing of the client + registry.

use std::collections::HashMap;
use std::sync::Mutex;

use async_trait::async_trait;

use deepagent_core::error::{CoreError, Result};

use crate::protocol::{JsonRpcRequest, JsonRpcResponse};

/// Sends JSON-RPC requests to an MCP server.
#[async_trait]
pub trait McpTransport: Send + Sync {
    /// Send `request` and await the correlated response.
    async fn send(&self, request: &JsonRpcRequest) -> Result<JsonRpcResponse>;

    /// Close the transport / terminate the server process.
    async fn close(&self) -> Result<()> {
        Ok(())
    }
}

/// A deterministic transport that replies to methods from a canned table.
/// Used for tests and offline development of the client/registry.
#[derive(Default)]
pub struct MockTransport {
    /// method name -> result JSON to return.
    results: HashMap<String, serde_json::Value>,
    /// Records of sent requests (for assertions).
    sent: Mutex<Vec<JsonRpcRequest>>,
}

impl MockTransport {
    /// New empty mock.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a canned result for a method (builder).
    pub fn with_result(mut self, method: impl Into<String>, result: serde_json::Value) -> Self {
        self.results.insert(method.into(), result);
        self
    }

    /// The requests sent so far.
    pub fn sent_methods(&self) -> Vec<String> {
        self.sent
            .lock()
            .expect("mock poisoned")
            .iter()
            .map(|r| r.method.clone())
            .collect()
    }
}

#[async_trait]
impl McpTransport for MockTransport {
    async fn send(&self, request: &JsonRpcRequest) -> Result<JsonRpcResponse> {
        self.sent
            .lock()
            .expect("mock poisoned")
            .push(request.clone());
        let result = self.results.get(&request.method).cloned();
        match result {
            Some(value) => Ok(JsonRpcResponse {
                jsonrpc: crate::protocol::JSONRPC_VERSION.to_string(),
                id: request.id,
                result: Some(value),
                error: None,
            }),
            None => Err(CoreError::other(format!(
                "mock has no result for method '{}'",
                request.method
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn mock_replies_to_known_method() {
        let t = MockTransport::new().with_result("ping", serde_json::json!({"ok": true}));
        let req = JsonRpcRequest::new(1, "ping", None);
        let res = t.send(&req).await.unwrap();
        assert_eq!(res.result.unwrap()["ok"], true);
        assert_eq!(t.sent_methods(), vec!["ping".to_string()]);
    }

    #[tokio::test]
    async fn mock_errors_on_unknown_method() {
        let t = MockTransport::new();
        let req = JsonRpcRequest::new(1, "unknown", None);
        assert!(t.send(&req).await.is_err());
    }
}
