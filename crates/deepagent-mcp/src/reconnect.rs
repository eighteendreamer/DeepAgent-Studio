//! Self-healing MCP transport: bounded auto-reconnect on transport-closed
//! errors.
//!
//! # Source alignment
//!
//! Grok's stdio MCP auto-restart (`xai-grok-shell/src/session/mcp_restart.rs`)
//! recovers a dead server with a **bounded exponential backoff** of exactly
//! `[1s, 4s, 16s]` (3 attempts, ~21s window) before parking the server as
//! unavailable. This module adopts that backoff schedule and bounded-attempt
//! philosophy.
//!
//! # Documented divergence (architecture-driven)
//!
//! Grok detects death **proactively** via a per-client liveness poller
//! (`liveness.rs`, 500ms `is_transport_closed()` polling) that emits a
//! `TransportClosed` event to a session dispatcher, which then schedules the
//! restart. That design fits Grok's `rmcp`-backed `RunningService` with a
//! background service loop and an ACP event bus.
//!
//! This system's [`McpTransport`] is a **synchronous request/response** trait
//! with no background service loop and no event bus, so a proactive poller has
//! nothing to poll. Instead we reconnect **reactively**: when a `send` fails
//! with a transport-closed error, we rebuild the transport (bounded backoff)
//! and retry the request. The recovery outcome — a crashed MCP server is
//! transparently respawned so the run keeps moving — is the same; only the
//! trigger differs (send-failure vs. background poll).
//!
//! Non-transport errors (a JSON-RPC error from a healthy server, a bad
//! request) are returned unchanged: only genuine transport failures trigger a
//! reconnect, so we never mask a real tool error as a connectivity blip.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::Mutex;

use deepagent_core::error::Result;

use crate::client::McpClient;
use crate::config::McpServerConfig;
use crate::connect::connect_transport;
use crate::protocol::{JsonRpcRequest, JsonRpcResponse};
use crate::transport::McpTransport;

/// Grok's bounded stdio-restart backoff schedule (`mcp_restart.rs::BACKOFF`):
/// three attempts at +1s / +4s / +16s (~21s total window).
pub const DEFAULT_RECONNECT_BACKOFF: [Duration; 3] = [
    Duration::from_secs(1),
    Duration::from_secs(4),
    Duration::from_secs(16),
];

/// Whether an error looks like a transport-level disconnect (as opposed to a
/// JSON-RPC error from a healthy server, or a malformed request). Only these
/// warrant a reconnect. Matches the messages produced by [`crate::stdio`] and
/// [`crate::http`] transports plus common OS pipe/socket failures.
pub fn is_transport_closed_error(error: &deepagent_core::error::CoreError) -> bool {
    let msg = error.to_string().to_ascii_lowercase();
    [
        "closed the connection",
        "connection lost",
        "connection reset",
        "connection refused",
        "broken pipe",
        "pipe closed",
        "pipe is closed",
        "write to mcp server failed",
        "read from mcp server failed",
        "flush to mcp server failed",
        "mcp http request failed",
        "unexpected eof",
        "os error 232", // Windows: the pipe is being closed
    ]
    .iter()
    .any(|needle| msg.contains(needle))
}

/// Builds a fresh, ready-to-use (handshake-completed) transport when the live
/// one dies. Implementations must return a transport on which `initialize` has
/// already succeeded, so the caller can immediately retry its request.
#[async_trait]
pub trait ReconnectFactory: Send + Sync {
    /// Reconnect: build a new transport and complete the MCP `initialize`
    /// handshake on it. Returns the ready transport, or an error if the server
    /// could not be reached/handshaked this attempt.
    async fn reconnect(&self) -> Result<Arc<dyn McpTransport>>;
}

/// The standard factory: rebuild the transport from a server config and run the
/// `initialize` handshake. Works for any transport [`connect_transport`]
/// supports (stdio always; http/sse behind the `http` feature).
pub struct ConfigReconnectFactory {
    config: McpServerConfig,
    client_name: String,
}

impl ConfigReconnectFactory {
    /// Build a factory that respawns `config` and re-initializes as
    /// `client_name`.
    pub fn new(config: McpServerConfig, client_name: impl Into<String>) -> Self {
        Self {
            config,
            client_name: client_name.into(),
        }
    }
}

#[async_trait]
impl ReconnectFactory for ConfigReconnectFactory {
    async fn reconnect(&self) -> Result<Arc<dyn McpTransport>> {
        let transport = connect_transport(&self.config)?;
        // Re-run the handshake on the fresh transport before it is used. The
        // temporary client only borrows the Arc; the same transport is then
        // returned ready for `tools/call`.
        McpClient::new(transport.clone())
            .initialize(&self.client_name)
            .await?;
        Ok(transport)
    }
}

/// A [`McpTransport`] that transparently rebuilds its inner transport (bounded
/// backoff) when a request fails with a transport-closed error.
pub struct ReconnectingTransport {
    inner: Mutex<Arc<dyn McpTransport>>,
    factory: Arc<dyn ReconnectFactory>,
    backoff: Vec<Duration>,
}

impl ReconnectingTransport {
    /// Wrap an already-connected `inner` transport with self-healing reconnect
    /// using the default Grok-aligned backoff.
    pub fn new(inner: Arc<dyn McpTransport>, factory: Arc<dyn ReconnectFactory>) -> Self {
        Self {
            inner: Mutex::new(inner),
            factory,
            backoff: DEFAULT_RECONNECT_BACKOFF.to_vec(),
        }
    }

    /// Override the reconnect backoff schedule (tests use near-zero delays).
    pub fn with_backoff(mut self, backoff: Vec<Duration>) -> Self {
        self.backoff = backoff;
        self
    }
}

#[async_trait]
impl McpTransport for ReconnectingTransport {
    async fn send(&self, request: &JsonRpcRequest) -> Result<JsonRpcResponse> {
        // Fast path: try the live transport.
        let current = self.inner.lock().await.clone();
        match current.send(request).await {
            Ok(response) => return Ok(response),
            Err(error) if is_transport_closed_error(&error) => {
                tracing::warn!(
                    error = %error,
                    "MCP transport closed mid-request; attempting bounded reconnect"
                );
            }
            // A JSON-RPC error or malformed-request failure from a healthy
            // server is a real result, not a connectivity problem — surface it.
            Err(error) => return Err(error),
        }

        // Recovery path: bounded backoff respawn + retry (Grok mcp_restart).
        let mut last_error = deepagent_core::error::CoreError::other(
            "MCP transport closed; reconnect not attempted",
        );
        for (idx, wait) in self.backoff.iter().enumerate() {
            let attempt = idx + 1;
            tokio::time::sleep(*wait).await;
            match self.factory.reconnect().await {
                Ok(fresh) => {
                    tracing::info!(attempt, "MCP transport reconnected; retrying request");
                    *self.inner.lock().await = fresh.clone();
                    match fresh.send(request).await {
                        Ok(response) => return Ok(response),
                        Err(error) if is_transport_closed_error(&error) => {
                            // Reconnected but died again before/at the retry —
                            // keep trying within the budget.
                            last_error = error;
                            continue;
                        }
                        Err(error) => return Err(error),
                    }
                }
                Err(error) => {
                    tracing::warn!(attempt, error = %error, "MCP reconnect attempt failed");
                    last_error = error;
                    continue;
                }
            }
        }
        tracing::warn!(
            attempts = self.backoff.len(),
            "MCP reconnect budget exhausted; surfacing transport error"
        );
        Err(last_error)
    }

    async fn close(&self) -> Result<()> {
        self.inner.lock().await.close().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::MockTransport;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn req() -> JsonRpcRequest {
        JsonRpcRequest::new(1, "tools/call", None)
    }

    /// A factory that hands out a fresh healthy MockTransport each reconnect,
    /// counting how many times it was called; can be told to fail the first
    /// `fail_first` reconnect attempts before succeeding.
    struct CountingFactory {
        calls: AtomicUsize,
        fail_first: usize,
    }

    #[async_trait]
    impl ReconnectFactory for CountingFactory {
        async fn reconnect(&self) -> Result<Arc<dyn McpTransport>> {
            let n = self.calls.fetch_add(1, Ordering::SeqCst);
            if n < self.fail_first {
                return Err(deepagent_core::error::CoreError::other(
                    "MCP server closed the connection",
                ));
            }
            Ok(Arc::new(MockTransport::new().with_result(
                "tools/call",
                serde_json::json!({"content":[{"type":"text","text":"healed"}],"isError":false}),
            )))
        }
    }

    fn fast_backoff() -> Vec<Duration> {
        vec![Duration::from_millis(0); 3]
    }

    #[test]
    fn classifies_transport_closed_vs_other_errors() {
        use deepagent_core::error::CoreError;
        assert!(is_transport_closed_error(&CoreError::other(
            "MCP server closed the connection"
        )));
        assert!(is_transport_closed_error(&CoreError::other(
            "mcp server connection lost: pipe closed"
        )));
        // A JSON-RPC-style error is not a transport failure.
        assert!(!is_transport_closed_error(&CoreError::other(
            "MCP error -32601 on tools/call: method not found"
        )));
        assert!(!is_transport_closed_error(&CoreError::invalid(
            "not an MCP tool name"
        )));
    }

    #[tokio::test]
    async fn passes_through_when_healthy() {
        let inner = Arc::new(MockTransport::new().with_result(
            "tools/call",
            serde_json::json!({"content":[{"type":"text","text":"ok"}],"isError":false}),
        ));
        let factory = Arc::new(CountingFactory {
            calls: AtomicUsize::new(0),
            fail_first: 0,
        });
        let t = ReconnectingTransport::new(inner, factory.clone()).with_backoff(fast_backoff());
        let resp = t.send(&req()).await.unwrap();
        assert!(resp.result.is_some());
        // No reconnect attempted on the healthy path.
        assert_eq!(factory.calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn reconnects_and_retries_after_transport_death() {
        // Inner dies immediately (fail after 0 sends), factory heals on the
        // first reconnect.
        let inner = Arc::new(
            MockTransport::new().with_failure_after(0, "MCP server closed the connection"),
        );
        let factory = Arc::new(CountingFactory {
            calls: AtomicUsize::new(0),
            fail_first: 0,
        });
        let t = ReconnectingTransport::new(inner, factory.clone()).with_backoff(fast_backoff());
        let resp = t.send(&req()).await.unwrap();
        assert_eq!(resp.result.unwrap()["content"][0]["text"], "healed");
        assert_eq!(factory.calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn retries_reconnect_within_budget_then_succeeds() {
        let inner = Arc::new(
            MockTransport::new().with_failure_after(0, "MCP server closed the connection"),
        );
        // First two reconnects fail, third succeeds — within the 3-attempt budget.
        let factory = Arc::new(CountingFactory {
            calls: AtomicUsize::new(0),
            fail_first: 2,
        });
        let t = ReconnectingTransport::new(inner, factory.clone()).with_backoff(fast_backoff());
        let resp = t.send(&req()).await.unwrap();
        assert_eq!(resp.result.unwrap()["content"][0]["text"], "healed");
        assert_eq!(factory.calls.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn gives_up_after_exhausting_backoff() {
        let inner = Arc::new(
            MockTransport::new().with_failure_after(0, "MCP server closed the connection"),
        );
        // Every reconnect fails; budget is 3 → give up with a transport error.
        let factory = Arc::new(CountingFactory {
            calls: AtomicUsize::new(0),
            fail_first: 999,
        });
        let t = ReconnectingTransport::new(inner, factory.clone()).with_backoff(fast_backoff());
        let error = t.send(&req()).await.unwrap_err();
        assert!(is_transport_closed_error(&error));
        assert_eq!(factory.calls.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn non_transport_error_is_not_reconnected() {
        // Inner returns "no result for method" (a non-transport error) — must
        // surface unchanged without any reconnect attempt.
        let inner = Arc::new(MockTransport::new());
        let factory = Arc::new(CountingFactory {
            calls: AtomicUsize::new(0),
            fail_first: 0,
        });
        let t = ReconnectingTransport::new(inner, factory.clone()).with_backoff(fast_backoff());
        let error = t.send(&req()).await.unwrap_err();
        assert!(!is_transport_closed_error(&error));
        assert_eq!(factory.calls.load(Ordering::SeqCst), 0);
    }
}
