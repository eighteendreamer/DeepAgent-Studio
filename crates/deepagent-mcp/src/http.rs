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
use deepagent_tools::ToolExecutionContext;

use crate::config::McpServerConfig;
use crate::protocol::{JsonRpcRequest, JSONRPC_VERSION};
use crate::protocol::{JsonRpcResponse, SseFrames};
use crate::transport::{
    McpTransport, MCP_CANCELLED_BEFORE_DISPATCH, MCP_CANCELLED_UNKNOWN_OUTCOME,
};

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
        let response = self
            .post_json(request)
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

    async fn send_with_context(
        &self,
        request: &JsonRpcRequest,
        context: ToolExecutionContext,
    ) -> Result<JsonRpcResponse> {
        if context.is_cancelled() {
            return Err(CoreError::other(MCP_CANCELLED_BEFORE_DISPATCH));
        }

        tokio::select! {
            biased;
            _ = context.cancelled() => {
                self.notify_cancelled(request.id);
                Err(CoreError::other(MCP_CANCELLED_UNKNOWN_OUTCOME))
            }
            result = self.send(request) => result,
        }
    }
}

impl HttpTransport {
    fn post_json<T: serde::Serialize + ?Sized>(&self, body: &T) -> reqwest::RequestBuilder {
        let mut builder = self
            .client
            .post(&self.url)
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream");
        for (key, value) in &self.headers {
            builder = builder.header(key.as_str(), value.as_str());
        }
        builder.json(body)
    }

    /// Send a fire-and-forget MCP cancellation notification. It must not keep
    /// the cancelled tool call waiting on a slow or unavailable HTTP endpoint.
    fn notify_cancelled(&self, request_id: u64) {
        let client = self.client.clone();
        let url = self.url.clone();
        let headers = self.headers.clone();
        tokio::spawn(async move {
            let notification = serde_json::json!({
                "jsonrpc": JSONRPC_VERSION,
                "method": "notifications/cancelled",
                "params": {
                    "requestId": request_id,
                    "reason": "user requested cancellation"
                }
            });
            let mut builder = client
                .post(url)
                .header("content-type", "application/json")
                .header("accept", "application/json, text/event-stream");
            for (key, value) in &headers {
                builder = builder.header(key.as_str(), value.as_str());
            }

            match tokio::time::timeout(
                Duration::from_millis(100),
                builder.json(&notification).send(),
            )
            .await
            {
                Ok(Ok(response)) if response.status().is_success() => {}
                Ok(Ok(response)) => tracing::debug!(
                    request_id,
                    status = %response.status(),
                    "MCP HTTP cancellation notification was rejected"
                ),
                Ok(Err(error)) => tracing::debug!(
                    request_id,
                    %error,
                    "could not send MCP HTTP cancellation notification"
                ),
                Err(_) => {
                    tracing::debug!(request_id, "MCP HTTP cancellation notification timed out")
                }
            }
        });
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
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    async fn read_json_request(stream: &mut tokio::net::TcpStream) -> serde_json::Value {
        let mut bytes = Vec::new();
        let mut chunk = [0u8; 1024];
        let (header_end, body_length) = loop {
            let read = stream.read(&mut chunk).await.unwrap();
            assert_ne!(read, 0, "HTTP peer closed before sending request");
            bytes.extend_from_slice(&chunk[..read]);
            if let Some(header_end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
                let headers = String::from_utf8_lossy(&bytes[..header_end]);
                let body_length = headers
                    .lines()
                    .find_map(|line| {
                        let (key, value) = line.split_once(':')?;
                        key.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().unwrap())
                    })
                    .expect("reqwest JSON request should have a content length");
                if bytes.len() >= header_end + 4 + body_length {
                    break (header_end, body_length);
                }
            }
        };
        serde_json::from_slice(&bytes[header_end + 4..header_end + 4 + body_length]).unwrap()
    }

    #[tokio::test]
    async fn cancellation_interrupts_http_wait_and_notifies_server() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/mcp", listener.local_addr().unwrap());
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (cancelled_tx, cancelled_rx) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            let mut pending_requests = Vec::new();
            let mut started_tx = Some(started_tx);
            let mut cancelled_tx = Some(cancelled_tx);
            loop {
                let (mut stream, _) =
                    tokio::time::timeout(Duration::from_secs(2), listener.accept())
                        .await
                        .expect("HTTP cancellation request should arrive")
                        .unwrap();
                let body = read_json_request(&mut stream).await;
                match body["method"].as_str() {
                    Some("tools/call") => {
                        started_tx
                            .take()
                            .expect("only one active tool request")
                            .send(())
                            .unwrap();
                        pending_requests.push(stream);
                    }
                    Some("notifications/cancelled") => {
                        cancelled_tx
                            .take()
                            .expect("only one cancellation notification")
                            .send(body)
                            .unwrap();
                        stream
                            .write_all(
                                b"HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                            )
                            .await
                            .unwrap();
                        drop(pending_requests);
                        return;
                    }
                    method => panic!("unexpected MCP HTTP method: {method:?}"),
                }
            }
        });

        let transport = std::sync::Arc::new(HttpTransport::with_url(url, Vec::new()).unwrap());
        let context = ToolExecutionContext::default();
        let cancel = context.cancel_flag();
        let request =
            JsonRpcRequest::new(41, "tools/call", Some(serde_json::json!({"name":"hold"})));
        let call_transport = transport.clone();
        let call =
            tokio::spawn(async move { call_transport.send_with_context(&request, context).await });

        tokio::time::timeout(Duration::from_secs(1), started_rx)
            .await
            .expect("tool request should be issued")
            .unwrap();
        cancel.store(true, std::sync::atomic::Ordering::Release);

        let result = tokio::time::timeout(Duration::from_millis(200), call)
            .await
            .expect("HTTP tool wait must stop promptly")
            .unwrap();
        assert!(result.unwrap_err().to_string().contains("cancelled"));

        let notification = tokio::time::timeout(Duration::from_secs(1), cancelled_rx)
            .await
            .expect("server should receive the cancellation notification")
            .unwrap();
        assert_eq!(notification["method"], "notifications/cancelled");
        assert_eq!(notification["params"]["requestId"], 41);
        assert!(notification.get("id").is_none());
        server.await.unwrap();
    }

    #[test]
    fn connect_requires_url() {
        let cfg = McpServerConfig {
            transport: Some(crate::config::TransportType::Http),
            command: None,
            args: vec![],
            env: Default::default(),
            cwd: None,
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
            cwd: None,
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
