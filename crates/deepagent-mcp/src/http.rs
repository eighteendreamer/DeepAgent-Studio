//! The HTTP / SSE MCP transport (only compiled with `--features http`).
//!
//! Implements MCP's **Streamable HTTP** transport: each JSON-RPC request is
//! POSTed to the server URL with `Accept: application/json, text/event-stream`.
//! The server may answer with either:
//! - `application/json` — a single JSON-RPC response body, or
//! - `text/event-stream` — one or more SSE `data:` frames; the frame whose JSON
//!   carries the matching request `id` is the response (others are
//!   notifications/server events and are skipped).
//!
//! Configured request `headers` (e.g. `Authorization: Bearer …`) are applied to
//! every request, supporting token/OAuth-style auth. The same transport serves
//! both the `http` and `sse` [`TransportType`]s — they differ only in how the
//! server frames its reply, which this transport detects from the response
//! content type.

use std::time::Duration;

use async_trait::async_trait;

use deepagent_core::error::{CoreError, Result};

use crate::config::McpServerConfig;
use crate::protocol::{JsonRpcRequest, JSONRPC_VERSION};
use crate::protocol::{JsonRpcResponse, SseFrames};
use crate::transport::McpTransport;

/// HTTP/SSE transport backed by `reqwest`.
pub struct HttpTransport {
    client: reqwest::Client,
    url: String,
    headers: Vec<(String, String)>,
}

impl HttpTransport {
    /// Build a transport from a (validated, expanded) network server config.
    pub fn connect(config: &McpServerConfig) -> Result<Self> {
        config.validate()?;
        let url = config
            .url
            .clone()
            .ok_or_else(|| CoreError::invalid("network MCP transport requires 'url'"))?;

        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(60))
            .build()
            .map_err(|e| CoreError::other(format!("failed to build HTTP client: {e}")))?;

        let headers = config
            .headers
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();

        Ok(Self {
            client,
            url,
            headers,
        })
    }

    /// Build directly from a url + headers (e.g. for tests against a local
    /// server).
    pub fn with_url(url: impl Into<String>, headers: Vec<(String, String)>) -> Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(60))
            .build()
            .map_err(|e| CoreError::other(format!("failed to build HTTP client: {e}")))?;
        Ok(Self {
            client,
            url: url.into(),
            headers,
        })
    }
}

#[async_trait]
impl McpTransport for HttpTransport {
    async fn send(&self, request: &JsonRpcRequest) -> Result<JsonRpcResponse> {
        let mut builder = self
            .client
            .post(&self.url)
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream")
            .json(request);
        for (k, v) in &self.headers {
            builder = builder.header(k.as_str(), v.as_str());
        }

        let response = builder
            .send()
            .await
            .map_err(|e| CoreError::other(format!("MCP HTTP request failed: {e}")))?;

        if !response.status().is_success() {
            let status = response.status();
            let detail = response.text().await.unwrap_or_default();
            return Err(CoreError::other(format!(
                "MCP server returned {status}: {detail}"
            )));
        }

        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_lowercase();

        let body = response
            .text()
            .await
            .map_err(|e| CoreError::other(format!("reading MCP response body failed: {e}")))?;

        if content_type.contains("text/event-stream") {
            parse_sse_response(&body, request.id)
        } else {
            // Plain JSON-RPC response.
            let resp: JsonRpcResponse = serde_json::from_str(body.trim()).map_err(|e| {
                CoreError::other(format!("invalid JSON-RPC response: {e}; body={body}"))
            })?;
            Ok(resp)
        }
    }
}

/// Parse an SSE body, returning the JSON-RPC response whose id matches `id`.
fn parse_sse_response(body: &str, id: u64) -> Result<JsonRpcResponse> {
    for data in SseFrames::parse(body) {
        if let Ok(resp) = serde_json::from_str::<JsonRpcResponse>(&data) {
            if resp.id == id {
                return Ok(resp);
            }
        }
    }
    Err(CoreError::other(format!(
        "no JSON-RPC response with id {id} found in SSE stream (jsonrpc {JSONRPC_VERSION})"
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connect_requires_url() {
        let cfg = McpServerConfig {
            transport: Some(crate::config::TransportType::Http),
            command: None,
            args: vec![],
            env: Default::default(),
            url: None,
            headers: Default::default(),
        };
        assert!(HttpTransport::connect(&cfg).is_err());
    }

    #[test]
    fn connect_rejects_insecure_url() {
        let cfg = McpServerConfig {
            transport: Some(crate::config::TransportType::Http),
            command: None,
            args: vec![],
            env: Default::default(),
            url: Some("http://evil.example.com/mcp".into()),
            headers: Default::default(),
        };
        assert!(HttpTransport::connect(&cfg).is_err());
    }

    #[test]
    fn parse_sse_picks_matching_id() {
        let body =
            "event: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":7,\"result\":{\"ok\":true}}\n\n";
        let resp = parse_sse_response(body, 7).unwrap();
        assert_eq!(resp.id, 7);
        assert_eq!(resp.result.unwrap()["ok"], true);
    }

    #[test]
    fn parse_sse_skips_notifications() {
        let body = "data: {\"jsonrpc\":\"2.0\",\"method\":\"notify\"}\n\ndata: {\"jsonrpc\":\"2.0\",\"id\":3,\"result\":{}}\n\n";
        let resp = parse_sse_response(body, 3).unwrap();
        assert_eq!(resp.id, 3);
    }

    #[test]
    fn parse_sse_errors_when_absent() {
        let body = "data: {\"jsonrpc\":\"2.0\",\"id\":99,\"result\":{}}\n\n";
        assert!(parse_sse_response(body, 1).is_err());
    }
}
