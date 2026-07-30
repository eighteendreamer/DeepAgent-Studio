//! Proactive MCP liveness probing via the spec's `ping` utility.
//!
//! # Source alignment
//!
//! - **MCP specification** (`2025-06-18/basic/utilities/ping`): implementations
//!   **SHOULD** periodically issue `ping` to detect connection health, the
//!   frequency **SHOULD** be configurable, a timeout **SHOULD** be treated as a
//!   connection failure, multiple failed pings **MAY** trigger connection
//!   reset, and ping failures **SHOULD** be logged for diagnostics. This module
//!   implements exactly that contract.
//! - **Grok** (`xai-grok-shell/src/session/liveness.rs`): a per-client
//!   background poller that detects transport death proactively (instead of
//!   waiting for the next tool call) and hands recovery to the restart
//!   machinery. Grok polls a local `is_transport_closed()` flag on its
//!   `rmcp` service; our [`crate::transport::McpTransport`] has no local
//!   liveness state, so the spec's `ping` RPC is the transport-agnostic
//!   equivalent probe.
//!
//! # How recovery composes
//!
//! The probe holds an [`McpClient`] whose transport is (in production) the
//! self-healing [`crate::reconnect::ReconnectingTransport`]. A `ping` that hits
//! a dead transport therefore *is* the recovery trigger: the reconnect wrapper
//! runs its bounded backoff respawn underneath the ping. The probe only counts
//! a failure after that budget is exhausted, and parks (stops probing, logs)
//! after [`DEFAULT_PING_FAILURE_LIMIT`] consecutive failures — mirroring
//! Grok's bounded "park as unavailable" philosophy.
//!
//! Lenient by design: only transport-level failures and timeouts count as
//! failures. A server that answers `ping` with a JSON-RPC error (e.g. an older
//! server without the utility) is *alive*; the probe logs once and retires
//! instead of flagging a healthy server.

use std::sync::Arc;
use std::time::Duration;

use tokio::task::JoinHandle;

use crate::client::McpClient;
use crate::reconnect::is_transport_closed_error;

/// Default probe interval. The spec mandates a configurable frequency and that
/// "excessive pinging SHOULD be avoided"; 30s keeps a stdio/HTTP server warm
/// without measurable overhead.
pub const DEFAULT_PING_INTERVAL: Duration = Duration::from_secs(30);

/// Per-ping reply timeout ("timeouts SHOULD be treated as connection
/// failures"). Generous enough to cover a reconnect-wrapper recovery cycle
/// (~21s backoff window) plus the retried ping itself.
pub const DEFAULT_PING_TIMEOUT: Duration = Duration::from_secs(30);

/// Consecutive ping failures before the probe parks the server as unavailable
/// (spec: "multiple failed pings MAY trigger connection reset"; Grok parks
/// after its 3-attempt restart budget).
pub const DEFAULT_PING_FAILURE_LIMIT: u32 = 3;

/// Tuning knobs for [`LivenessProbe`].
#[derive(Debug, Clone)]
pub struct LivenessConfig {
    /// Delay between probes.
    pub interval: Duration,
    /// How long to wait for a single ping reply.
    pub timeout: Duration,
    /// Consecutive failures before parking.
    pub failure_limit: u32,
}

impl Default for LivenessConfig {
    fn default() -> Self {
        Self {
            interval: DEFAULT_PING_INTERVAL,
            timeout: DEFAULT_PING_TIMEOUT,
            failure_limit: DEFAULT_PING_FAILURE_LIMIT,
        }
    }
}

/// Handle to a background liveness probe. Aborts the probe task when dropped,
/// so tying the probe's lifetime to the connection cache entry that owns it
/// stops probing exactly when the connection is torn down.
pub struct LivenessProbe {
    handle: JoinHandle<()>,
}

impl LivenessProbe {
    /// Spawn a background probe that periodically pings `client` (named
    /// `server` for logs) per `config`.
    pub fn spawn(
        client: Arc<McpClient>,
        server: impl Into<String>,
        config: LivenessConfig,
    ) -> Self {
        let server = server.into();
        let handle = tokio::spawn(async move {
            probe_loop(client, server, config).await;
        });
        Self { handle }
    }

    /// Whether the probe task has exited (parked, retired, or aborted).
    pub fn is_finished(&self) -> bool {
        self.handle.is_finished()
    }
}

impl Drop for LivenessProbe {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

/// One probe iteration outcome (extracted for direct unit testing).
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ProbeStep {
    /// Ping replied — connection alive, reset the failure streak.
    Alive,
    /// Transport-level failure or timeout — count towards the park limit.
    Failure,
    /// Server answered with a JSON-RPC error: it is alive but does not speak
    /// the ping utility. Retire the probe without flagging the server.
    Unsupported,
}

/// Classify a single ping result. `timed_out` covers the spec's "timeouts
/// SHOULD be treated as connection failures".
pub(crate) fn classify_ping(result: Option<deepagent_core::error::Result<()>>) -> ProbeStep {
    match result {
        None => ProbeStep::Failure, // timeout
        Some(Ok(())) => ProbeStep::Alive,
        Some(Err(error)) if is_transport_closed_error(&error) => ProbeStep::Failure,
        // Any other reply (JSON-RPC error such as -32601 method-not-found)
        // came from a live server — lenient: never park a healthy server.
        Some(Err(_)) => ProbeStep::Unsupported,
    }
}

async fn probe_loop(client: Arc<McpClient>, server: String, config: LivenessConfig) {
    let mut consecutive_failures: u32 = 0;
    loop {
        tokio::time::sleep(config.interval).await;
        let outcome = tokio::time::timeout(config.timeout, client.ping()).await;
        match classify_ping(outcome.ok()) {
            ProbeStep::Alive => {
                if consecutive_failures > 0 {
                    tracing::info!(
                        server = server.as_str(),
                        "MCP liveness ping recovered after failures"
                    );
                }
                consecutive_failures = 0;
            }
            ProbeStep::Unsupported => {
                tracing::info!(
                    server = server.as_str(),
                    "MCP server does not support the ping utility; retiring liveness probe"
                );
                return;
            }
            ProbeStep::Failure => {
                consecutive_failures += 1;
                tracing::warn!(
                    server = server.as_str(),
                    consecutive_failures,
                    limit = config.failure_limit,
                    "MCP liveness ping failed"
                );
                if consecutive_failures >= config.failure_limit {
                    tracing::warn!(
                        server = server.as_str(),
                        "MCP liveness failure limit reached; parking probe (server unavailable)"
                    );
                    return;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::MockTransport;
    use deepagent_core::error::CoreError;

    fn fast_config(failure_limit: u32) -> LivenessConfig {
        LivenessConfig {
            interval: Duration::from_millis(1),
            timeout: Duration::from_millis(200),
            failure_limit,
        }
    }

    #[test]
    fn classifies_alive_failure_and_unsupported() {
        assert_eq!(classify_ping(Some(Ok(()))), ProbeStep::Alive);
        // Timeout.
        assert_eq!(classify_ping(None), ProbeStep::Failure);
        // Transport death.
        assert_eq!(
            classify_ping(Some(Err(CoreError::other(
                "MCP server closed the connection"
            )))),
            ProbeStep::Failure
        );
        // JSON-RPC error from a live server (no ping utility).
        assert_eq!(
            classify_ping(Some(Err(CoreError::other(
                "MCP error -32601 on ping: method not found"
            )))),
            ProbeStep::Unsupported
        );
    }

    #[tokio::test]
    async fn probe_stays_alive_on_healthy_server() {
        let client = Arc::new(McpClient::new(Arc::new(
            MockTransport::new().with_result("ping", serde_json::json!({})),
        )));
        let probe = LivenessProbe::spawn(client, "healthy", fast_config(3));
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!probe.is_finished());
    }

    #[tokio::test]
    async fn probe_parks_after_consecutive_transport_failures() {
        // Every send fails with a transport-closed error.
        let client = Arc::new(McpClient::new(Arc::new(
            MockTransport::new().with_failure_after(0, "MCP server closed the connection"),
        )));
        let probe = LivenessProbe::spawn(client, "dead", fast_config(3));
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(probe.is_finished());
    }

    #[tokio::test]
    async fn probe_retires_when_server_lacks_ping() {
        // Mock has no "ping" entry -> non-transport error -> Unsupported.
        let client = Arc::new(McpClient::new(Arc::new(MockTransport::new())));
        let probe = LivenessProbe::spawn(client, "legacy", fast_config(3));
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(probe.is_finished());
    }

    #[tokio::test]
    async fn dropping_probe_aborts_task() {
        let transport = Arc::new(MockTransport::new().with_result("ping", serde_json::json!({})));
        let client = Arc::new(McpClient::new(transport.clone()));
        let probe = LivenessProbe::spawn(client, "owned", fast_config(3));
        tokio::time::sleep(Duration::from_millis(30)).await;
        drop(probe);
        // Give the runtime a beat to process the abort, then confirm pinging
        // has stopped: no new sends accumulate after the drop.
        tokio::time::sleep(Duration::from_millis(10)).await;
        let sends_after_drop = transport.sent_methods().len();
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert_eq!(transport.sent_methods().len(), sends_after_drop);
    }
}
