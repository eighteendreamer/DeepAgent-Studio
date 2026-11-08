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
//! before a subsequent request is dispatched. A failed business request is
//! never replayed: its remote outcome may be unknown. Only the protocol's
//! `ping` and `tools/list` control requests get one retry after recovery.
//! The budget belongs to a disconnected connection identity, not a send or
//! probe. Local exhaustion is stable until the host explicitly replaces that
//! identity (retry, config replacement or restart). HTTP/SSE factory policy
//! deliberately retains next-request recovery rather than permanent parking.
//!
//! Non-transport errors (a JSON-RPC error from a healthy server, a bad
//! request) are returned unchanged: only genuine transport failures trigger a
//! reconnect, so we never mask a real tool error as a connectivity blip.

use std::sync::{Arc, Mutex as StateMutex};
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::{Mutex, Notify};

use deepagent_core::error::{CoreError, Result};
use deepagent_tools::ToolExecutionContext;

use crate::client::McpClient;
use crate::config::{McpServerConfig, TransportType};
use crate::connect::connect_transport;
use crate::protocol::{JsonRpcRequest, JsonRpcResponse};
use crate::transport::{
    McpConnectionStatus, McpTransport, MCP_CANCELLED_BEFORE_DISPATCH, MCP_CANCELLED_UNKNOWN_OUTCOME,
};

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
    if matches!(
        error,
        CoreError::RemoteOutcomeUnknown { .. } | CoreError::McpUnavailable { .. }
    ) {
        return false;
    }
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
/// already succeeded, so the caller can dispatch a new request.
#[async_trait]
pub trait ReconnectFactory: Send + Sync {
    /// Local connection identities stay parked until replaced explicitly.
    /// Network factories override this to retain next-request recovery.
    fn parks_on_exhaustion(&self) -> bool {
        true
    }

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
    fn parks_on_exhaustion(&self) -> bool {
        matches!(self.config.effective_type(), Ok(TransportType::Stdio))
    }

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
    inner: StateMutex<ConnectionState>,
    recovery: Mutex<()>,
    stopped: Notify,
    factory: Arc<dyn ReconnectFactory>,
    backoff: Vec<Duration>,
}

struct ConnectionState {
    transport: Arc<dyn McpTransport>,
    phase: ConnectionPhase,
}

enum ConnectionPhase {
    Ready,
    Recovering { attempts: usize, last_error: String },
    Unavailable { attempts: usize, reason: String },
    Closed,
}

/// Preserve exhaustion even when a caller drops the final handshake future.
struct RecoveryOwner<'a>(&'a ReconnectingTransport);

impl Drop for RecoveryOwner<'_> {
    fn drop(&mut self) {
        let transport = self.0;
        if !transport.factory.parks_on_exhaustion() {
            return;
        }
        let mut state = transport.state();
        if let ConnectionPhase::Recovering {
            attempts,
            last_error,
        } = &state.phase
        {
            if *attempts >= transport.backoff.len() {
                state.phase = ConnectionPhase::Unavailable {
                    attempts: *attempts,
                    reason: last_error.clone(),
                };
                transport.stopped.notify_waiters();
            }
        }
    }
}

impl ReconnectingTransport {
    /// Wrap an already-connected `inner` transport with self-healing reconnect
    /// using the default Grok-aligned backoff.
    pub fn new(inner: Arc<dyn McpTransport>, factory: Arc<dyn ReconnectFactory>) -> Self {
        Self {
            inner: StateMutex::new(ConnectionState {
                transport: inner,
                phase: ConnectionPhase::Ready,
            }),
            recovery: Mutex::new(()),
            stopped: Notify::new(),
            factory,
            backoff: DEFAULT_RECONNECT_BACKOFF.to_vec(),
        }
    }

    /// Override the reconnect backoff schedule (tests use near-zero delays).
    pub fn with_backoff(mut self, backoff: Vec<Duration>) -> Self {
        self.backoff = backoff;
        self
    }

    fn state(&self) -> std::sync::MutexGuard<'_, ConnectionState> {
        self.inner.lock().unwrap_or_else(|error| error.into_inner())
    }

    fn stopped_error(&self) -> CoreError {
        match &self.state().phase {
            ConnectionPhase::Unavailable { attempts, reason } => CoreError::McpUnavailable {
                recovery_attempts: *attempts,
                reason: reason.clone(),
            },
            _ => CoreError::other("MCP transport is closed"),
        }
    }

    async fn stopped(&self) {
        loop {
            let notified = self.stopped.notified();
            tokio::pin!(notified);
            // Register before checking state: close cannot be lost in between.
            notified.as_mut().enable();
            if matches!(
                self.state().phase,
                ConnectionPhase::Closed | ConnectionPhase::Unavailable { .. }
            ) {
                return;
            }
            notified.await;
        }
    }

    fn disconnected(&self, current: &Arc<dyn McpTransport>) {
        let mut state = self.state();
        // Neither late failures nor concurrent callers renew a recovery budget.
        if Arc::ptr_eq(&state.transport, current) && matches!(state.phase, ConnectionPhase::Ready) {
            state.phase = ConnectionPhase::Recovering {
                attempts: 0,
                last_error: "MCP transport closed; reconnect not attempted".into(),
            };
        }
    }

    /// One recovery owner, but no state lock spans I/O. Started attempts survive
    /// waiter cancellation; close can stop the owner without awaiting a handshake.
    async fn ready_transport(&self) -> Result<Arc<dyn McpTransport>> {
        let _owner = self.recovery.lock().await;
        let _completion = RecoveryOwner(self);
        loop {
            let index = {
                let mut state = self.state();
                match &mut state.phase {
                    ConnectionPhase::Ready => return Ok(state.transport.clone()),
                    ConnectionPhase::Closed => {
                        return Err(CoreError::other("MCP transport is closed"))
                    }
                    ConnectionPhase::Unavailable { attempts, reason } => {
                        return Err(CoreError::McpUnavailable {
                            recovery_attempts: *attempts,
                            reason: reason.clone(),
                        })
                    }
                    ConnectionPhase::Recovering {
                        attempts,
                        last_error,
                    } if *attempts >= self.backoff.len() => {
                        let error = last_error.clone();
                        if self.factory.parks_on_exhaustion() {
                            let attempts = *attempts;
                            state.phase = ConnectionPhase::Unavailable {
                                attempts,
                                reason: error.clone(),
                            };
                            tracing::warn!(
                                attempts,
                                "MCP local recovery budget exhausted; explicit retry required"
                            );
                            self.stopped.notify_waiters();
                            return Err(CoreError::McpUnavailable {
                                recovery_attempts: attempts,
                                reason: error,
                            });
                        }
                        // Preserve the existing HTTP/SSE next-request policy.
                        *attempts = 0;
                        return Err(CoreError::other(error));
                    }
                    ConnectionPhase::Recovering { attempts, .. } => *attempts,
                }
            };
            tokio::select! {
                biased;
                _ = self.stopped() => return Err(self.stopped_error()),
                _ = tokio::time::sleep(self.backoff[index]) => {}
            }
            {
                let mut state = self.state();
                let ConnectionPhase::Recovering { attempts, .. } = &mut state.phase else {
                    drop(state);
                    return Err(self.stopped_error());
                };
                // Reserve before starting the factory. Dropping this await may
                // end a caller, never refund a process that was already started.
                *attempts += 1;
            }
            let attempt = index + 1;
            let result = tokio::select! {
                biased;
                _ = self.stopped() => return Err(self.stopped_error()),
                result = self.factory.reconnect() => result,
            };
            match result {
                Ok(fresh) => {
                    let accepted = {
                        let mut state = self.state();
                        if matches!(state.phase, ConnectionPhase::Recovering { .. }) {
                            state.transport = fresh.clone();
                            state.phase = ConnectionPhase::Ready;
                            true
                        } else {
                            false
                        }
                    };
                    if !accepted {
                        if let Err(error) = fresh.close().await {
                            tracing::warn!(%error, "MCP superseded recovery transport shutdown failed");
                        }
                        return Err(self.stopped_error());
                    }
                    tracing::info!(attempt, "MCP connection restored for subsequent dispatch");
                    return Ok(fresh);
                }
                Err(error) => {
                    tracing::warn!(attempt, error = %error, "MCP reconnect attempt failed");
                    if let ConnectionPhase::Recovering { last_error, .. } = &mut self.state().phase
                    {
                        *last_error = error.to_string();
                    }
                }
            }
        }
    }

    async fn send_request(
        &self,
        request: &JsonRpcRequest,
        context: ToolExecutionContext,
    ) -> Result<JsonRpcResponse> {
        // These are protocol-defined control operations, not guesses based on
        // a tool name or untrusted readOnly annotations. All other methods
        // receive exactly one dispatch, including initialize and tools/call.
        let attempts = if matches!(request.method.as_str(), "ping" | "tools/list") {
            2
        } else {
            1
        };
        for attempt in 0..attempts {
            let current = tokio::select! {
                biased;
                _ = context.cancelled() => return Err(CoreError::other(MCP_CANCELLED_BEFORE_DISPATCH)),
                result = self.ready_transport() => result?,
            };
            if context.is_cancelled() {
                return Err(CoreError::other(MCP_CANCELLED_BEFORE_DISPATCH));
            }
            let error = match current.send_with_context(request, context.clone()).await {
                Ok(response) => return Ok(response),
                Err(error) => error,
            };
            let disconnected = is_transport_closed_error(&error);
            if disconnected {
                self.disconnected(&current);
                tracing::warn!(method = %request.method, request_id = request.id,
                    "MCP connection lost; failed business request will not be replayed");
            }
            if request.method == "tools/call"
                && (disconnected
                    || matches!(error, CoreError::McpResponseTooLarge { .. })
                    || error.to_string() == MCP_CANCELLED_UNKNOWN_OUTCOME)
            {
                return Err(CoreError::RemoteOutcomeUnknown {
                    method: request.method.clone(),
                    request_id: request.id,
                    source: Box::new(error),
                });
            }
            if !disconnected || context.is_cancelled() || attempt + 1 == attempts {
                return Err(error);
            }
        }
        unreachable!("at least one dispatch attempt")
    }
}

#[async_trait]
impl McpTransport for ReconnectingTransport {
    fn connection_status(&self) -> McpConnectionStatus {
        match &self.state().phase {
            ConnectionPhase::Ready => McpConnectionStatus::Ready,
            ConnectionPhase::Recovering { .. } => McpConnectionStatus::Recovering,
            ConnectionPhase::Unavailable { attempts, reason } => McpConnectionStatus::Unavailable {
                recovery_attempts: *attempts,
                reason: reason.clone(),
            },
            ConnectionPhase::Closed => McpConnectionStatus::Closed,
        }
    }

    async fn park_after_liveness_failure(&self, reason: &str) {
        if !self.factory.parks_on_exhaustion() {
            return;
        }
        let transport = {
            let mut state = self.state();
            let attempts = match &state.phase {
                ConnectionPhase::Closed | ConnectionPhase::Unavailable { .. } => return,
                ConnectionPhase::Recovering { attempts, .. } => *attempts,
                ConnectionPhase::Ready => 0,
            };
            state.phase = ConnectionPhase::Unavailable {
                attempts,
                reason: reason.into(),
            };
            state.transport.clone()
        };
        self.stopped.notify_waiters();
        if let Err(error) = transport.close().await {
            tracing::warn!(%error, "MCP parked transport shutdown failed");
        }
    }

    async fn send(&self, request: &JsonRpcRequest) -> Result<JsonRpcResponse> {
        self.send_request(request, ToolExecutionContext::default())
            .await
    }

    async fn send_with_context(
        &self,
        request: &JsonRpcRequest,
        context: ToolExecutionContext,
    ) -> Result<JsonRpcResponse> {
        self.send_request(request, context).await
    }

    async fn notify(&self, method: &str, params: Option<serde_json::Value>) -> Result<()> {
        let current = self.ready_transport().await?;
        let result = current.notify(method, params).await;
        if let Err(error) = &result {
            if is_transport_closed_error(error) {
                self.disconnected(&current);
            }
        }
        result
    }

    async fn close(&self) -> Result<()> {
        let transport = {
            let mut state = self.state();
            state.phase = ConnectionPhase::Closed;
            state.transport.clone()
        };
        self.stopped.notify_waiters();
        transport.close().await
    }
}

#[cfg(test)]
#[path = "reconnect_boundary_tests.rs"]
mod boundary_tests;

#[cfg(test)]
#[path = "reconnect_lifecycle_tests.rs"]
mod lifecycle_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::MockTransport;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn req() -> JsonRpcRequest {
        JsonRpcRequest::new(1, "tools/call", None)
    }

    #[tokio::test]
    async fn audit_disconnect_after_tool_effect_does_not_replay_mutation() {
        struct AppliedThenDisconnected {
            effects: Arc<AtomicUsize>,
            lose_reply: bool,
        }

        #[async_trait]
        impl McpTransport for AppliedThenDisconnected {
            async fn send(&self, request: &JsonRpcRequest) -> Result<JsonRpcResponse> {
                assert_eq!(request.method, "tools/call");
                self.effects.fetch_add(1, Ordering::SeqCst);
                if self.lose_reply {
                    return Err(deepagent_core::error::CoreError::other("broken pipe"));
                }
                Ok(JsonRpcResponse {
                    jsonrpc: "2.0".into(),
                    id: request.id,
                    result: Some(serde_json::json!({"content":[],"isError":false})),
                    error: None,
                })
            }
        }

        struct Factory(Arc<AtomicUsize>);

        #[async_trait]
        impl ReconnectFactory for Factory {
            async fn reconnect(&self) -> Result<Arc<dyn McpTransport>> {
                Ok(Arc::new(AppliedThenDisconnected {
                    effects: self.0.clone(),
                    lose_reply: false,
                }))
            }
        }

        let effects = Arc::new(AtomicUsize::new(0));
        let transport = ReconnectingTransport::new(
            Arc::new(AppliedThenDisconnected {
                effects: effects.clone(),
                lose_reply: true,
            }),
            Arc::new(Factory(effects.clone())),
        )
        .with_backoff(vec![Duration::ZERO]);
        let client = McpClient::new(Arc::new(transport));
        let result = client
            .call_tool("append_record", serde_json::json!({"value":"audit"}))
            .await;
        eprintln!(
            "audit MCP replay: result_ok={}, applied_effects={}",
            result.is_ok(),
            effects.load(Ordering::SeqCst)
        );
        assert_eq!(
            effects.load(Ordering::SeqCst),
            1,
            "a lost reply must not cause an unknown-outcome mutation to be applied twice"
        );
        assert!(matches!(
            result,
            Err(CoreError::RemoteOutcomeUnknown { .. })
        ));
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
                "ping",
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
    async fn reconnects_and_retries_control_ping_after_transport_death() {
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
        let resp = t.send(&JsonRpcRequest::new(1, "ping", None)).await.unwrap();
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
        let resp = t.send(&JsonRpcRequest::new(1, "ping", None)).await.unwrap();
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
        let error = t
            .send(&JsonRpcRequest::new(1, "ping", None))
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            CoreError::McpUnavailable {
                recovery_attempts: 3,
                ..
            }
        ));
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
        let error = t
            .send(&JsonRpcRequest::new(1, "ping", None))
            .await
            .unwrap_err();
        assert!(!is_transport_closed_error(&error));
        assert_eq!(factory.calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn tool_json_rpc_error_is_authoritative_not_unknown() {
        let inner = Arc::new(MockTransport::new().with_result(
            "tools/call",
            serde_json::json!({"content":[],"isError":true}),
        ));
        let factory = Arc::new(CountingFactory {
            calls: AtomicUsize::new(0),
            fail_first: 0,
        });
        let t = ReconnectingTransport::new(inner, factory.clone()).with_backoff(fast_backoff());
        let response = t
            .send(&JsonRpcRequest::new(1, "tools/call", None))
            .await
            .unwrap();
        assert_eq!(response.result.unwrap()["isError"], true);
        assert_eq!(factory.calls.load(Ordering::SeqCst), 0);
    }
}
