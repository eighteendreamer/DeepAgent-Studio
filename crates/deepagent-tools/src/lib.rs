//! # deepagent-tools
//!
//! The tool runtime (开发计划.md Phase 4; 开发提示词.md §15–§16).
//!
//! Core ideas implemented here:
//! - [`Tool`] — the async trait every tool implements, exposing a JSON-schema
//!   description and an `invoke` method.
//! - [`permission`] — the [`permission::Permission`] scopes and
//!   [`permission::RiskLevel`] used to gate dangerous operations.
//! - [`registry`] — the Capability Registry / [`registry::ToolRegistry`] that
//!   stores tools, filters them by an agent's granted permissions, and routes
//!   invocations.
//! - [`sandbox`] — the engine-independent [`sandbox::SandboxPolicy`] (memory /
//!   fuel / timeout / capability limits) + [`sandbox::Sandbox`] trait and JSON
//!   ABI helpers. The real WebAssembly backend
//!   ([`wasm::WasmSandbox`](crate::wasm), behind `--features wasm`) executes
//!   untrusted tool modules with no ambient authority.
//!
//! Sandboxing uses WebAssembly (Wasmtime): the [`Tool`] abstraction is designed
//! so a sandboxed executor can wrap any tool transparently, and [`sandbox`]
//! holds the policy/ABI while [`wasm`](crate::wasm) holds the engine backend.

mod observation;
pub mod permission;
pub mod registry;
pub mod sandbox;
pub mod sandboxed_tool;
#[cfg(feature = "wasm")]
pub mod wasm;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio_util::sync::CancellationToken;

use deepagent_core::error::Result;

pub use observation::{ToolObservation, ToolObservationScope};
pub use permission::{Permission, PermissionSet, RiskLevel};
pub use registry::{
    validate_tool_arguments, validate_tool_name, validate_tool_schema, ToolRegistry, ToolSpec,
    ValidatedInvocation, MAX_TOOL_NAME_BYTES,
};
pub use sandbox::{Capabilities, Sandbox, SandboxPolicy, SandboxStats};
pub use sandboxed_tool::SandboxedTool;

#[cfg(feature = "wasm")]
pub use wasm::WasmSandbox;

/// A tool invocation request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolInvocation {
    /// The tool name to invoke.
    pub name: String,
    /// JSON arguments.
    pub arguments: serde_json::Value,
    /// Optional provider-assigned call id (from the model's `tool_calls[].id`).
    /// Carried through so the runtime can correlate a tool result back to the
    /// exact call when the model emits several tool calls in one turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
}

impl ToolInvocation {
    /// Build an invocation.
    pub fn new(name: impl Into<String>, arguments: serde_json::Value) -> Self {
        Self {
            name: name.into(),
            arguments,
            id: None,
        }
    }

    /// Build an invocation carrying the model's tool-call id (builder style).
    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }
}

/// The outcome of a tool invocation.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolOutput {
    /// Host/client-only data. Deliberately excluded from generic ToolOutput
    /// serialization (including nested tool results) and Debug diagnostics.
    /// The runtime explicitly publishes it through run-event `meta` only;
    /// model observations, Session results and budget artifacts use value/content.
    #[serde(skip)]
    pub client_metadata: Option<serde_json::Value>,
    /// Whether the tool succeeded.
    pub ok: bool,
    /// JSON-encoded result (or error detail when `ok == false`).
    pub value: serde_json::Value,
    /// Optional ordered model content with durable, host-admitted image refs.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub content: Vec<deepagent_core::tool_content::ToolResultPart>,
    /// Whether the model-visible result was truncated by the runtime budget.
    #[serde(default)]
    pub truncated: bool,
}

impl ToolOutput {
    /// A successful result.
    pub fn success(value: serde_json::Value) -> Self {
        Self {
            client_metadata: None,
            ok: true,
            value,
            content: Vec::new(),
            truncated: false,
        }
    }

    /// A failure result with a message.
    pub fn failure(message: impl Into<String>) -> Self {
        Self {
            client_metadata: None,
            ok: false,
            value: serde_json::json!({ "error": message.into() }),
            content: Vec::new(),
            truncated: false,
        }
    }
}

impl std::fmt::Debug for ToolOutput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolOutput")
            .field("ok", &self.ok)
            .field("value", &self.value)
            .field("content", &self.content)
            .field("truncated", &self.truncated)
            .field("client_metadata_present", &self.client_metadata.is_some())
            .finish()
    }
}

/// Metadata describing a tool for routing and for the model's tool schema.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolDescriptor {
    /// Unique tool name.
    pub name: String,
    /// Human / model-facing description.
    pub description: String,
    /// JSON Schema for the arguments.
    pub parameters: serde_json::Value,
    /// Risk classification used for permission routing.
    pub risk: RiskLevel,
    /// Permissions required to invoke this tool.
    pub required_permissions: PermissionSet,
}

/// Per-invocation execution controls supplied by the agent kernel.
///
/// This lives in `deepagent-tools` so tools can observe cancellation without
/// creating a dependency cycle back to the runtime crate.
#[derive(Clone, Debug)]
pub struct ToolExecutionContext {
    cancel: Arc<AtomicBool>,
    token: CancellationToken,
    timeout: Duration,
    observation: Option<ToolObservation>,
}

impl Default for ToolExecutionContext {
    fn default() -> Self {
        Self {
            cancel: Arc::new(AtomicBool::new(false)),
            token: CancellationToken::new(),
            timeout: Self::DEFAULT_TIMEOUT,
            observation: None,
        }
    }
}

impl ToolExecutionContext {
    /// The system's one tool deadline, in one place.
    ///
    /// The pipeline's outer cap, `RuntimeConfig::tool_timeout`, and every
    /// process-backed executor that has to pick a deadline for itself all read
    /// this; a duration literal anywhere else means two owners for one rule.
    pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);
    pub fn new(cancel: Arc<AtomicBool>) -> Self {
        Self {
            cancel,
            token: CancellationToken::new(),
            ..Self::default()
        }
    }

    /// Build a context from the hierarchical cancellation signal used by the
    /// runtime. `legacy_flag` remains available for process-backed tools that
    /// still expose the older AtomicBool seam; the token is the authoritative
    /// signal for new tools and for async cancellation gates.
    pub fn from_token(token: CancellationToken) -> Self {
        Self {
            cancel: Arc::new(AtomicBool::new(false)),
            token,
            ..Self::default()
        }
    }

    /// Build a token-backed context while preserving a legacy flag for tools
    /// such as the shell executor. Cancelling either signal is observable via
    /// [`Self::is_cancelled`] and [`Self::cancelled`].
    pub fn from_token_and_flag(token: CancellationToken, cancel: Arc<AtomicBool>) -> Self {
        Self {
            cancel,
            token,
            ..Self::default()
        }
    }

    /// Derive an invocation-local child signal. Cancelling the child never
    /// cancels its parent, while parent cancellation propagates automatically.
    pub fn child(&self) -> Self {
        Self {
            cancel: Arc::new(AtomicBool::new(false)),
            token: self.token.child_token(),
            timeout: self.timeout,
            observation: self.observation.clone(),
        }
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Runtime and pipeline owners attach their captured model generation.
    /// The legacy direct Tool API leaves this unset: its shared file cache is
    /// its observation boundary, and can never inherit a scoped runtime read.
    pub fn with_observation(mut self, observation: ToolObservation) -> Self {
        self.observation = Some(observation);
        self
    }

    pub fn observation(&self) -> Option<&ToolObservation> {
        self.observation.as_ref()
    }

    pub fn observation_is_current(&self) -> bool {
        self.observation
            .as_ref()
            .is_none_or(ToolObservation::is_current)
    }

    pub fn cancel_flag(&self) -> Arc<AtomicBool> {
        self.cancel.clone()
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::Acquire) || self.token.is_cancelled()
    }

    pub fn cancellation_token(&self) -> CancellationToken {
        self.token.clone()
    }

    /// Await either the hierarchical token or the compatibility flag. The
    /// latter is polled only for legacy callers; token-backed tools wake
    /// immediately when their parent is cancelled.
    pub async fn cancelled(&self) {
        tokio::select! {
            _ = self.token.cancelled() => {}
            _ = wait_for_legacy_cancel(self.cancel.clone()) => {}
        }
    }

    pub fn timeout(&self) -> Duration {
        self.timeout
    }
}

async fn wait_for_legacy_cancel(cancel: Arc<AtomicBool>) {
    while !cancel.load(Ordering::Acquire) {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Revokes tool-local facts derived from a model's previous context. This is
/// not a permission grant, cancellation signal, or a second tool dispatcher.
pub type ToolContextReset = Arc<dyn Fn() + Send + Sync>;

/// The trait every tool implements.
#[async_trait]
pub trait Tool: Send + Sync {
    /// Static descriptor (name, schema, risk, permissions).
    fn descriptor(&self) -> ToolDescriptor;

    /// Execute the tool with the given JSON arguments.
    async fn invoke(&self, arguments: serde_json::Value) -> Result<ToolOutput>;

    /// Execute with run-scoped cancellation and timeout controls. Existing
    /// tools inherit this compatibility implementation; process-backed tools
    /// override it so Stop can interrupt in-flight work immediately.
    async fn invoke_with_context(
        &self,
        arguments: serde_json::Value,
        _context: ToolExecutionContext,
    ) -> Result<ToolOutput> {
        self.invoke(arguments).await
    }

    /// Whether the pipeline may abandon this tool once the run's per-tool budget
    /// expires.
    ///
    /// Default `true`. The budget reaches tools as data (`ToolExecutionContext::
    /// timeout`), and a tool is free to ignore it, so without an outer deadline a
    /// single stuck tool holds the whole run open forever. Two shapes opt out
    /// honestly: a nested run that carries its own, longer deadline (subagent
    /// dispatch), and a wait on a human that is deliberately unbounded.
    fn bounded_by_run_timeout(&self) -> bool {
        true
    }

    /// Validate and optionally normalize schema-valid input before hooks and
    /// permission rules observe it. Tools can override this for value-level
    /// constraints such as path normalization or mutually-exclusive fields.
    fn validate_input(&self, arguments: serde_json::Value) -> Result<serde_json::Value> {
        Ok(arguments)
    }

    /// Whether this concrete invocation is safe to overlap with other safe
    /// invocations. The conservative default only permits `Safe` tools.
    fn is_concurrency_safe(&self, _arguments: &serde_json::Value) -> bool {
        self.descriptor().risk == RiskLevel::Safe
    }

    /// Project this capability for an isolated, read-only observer. Unlike
    /// risk, permissions or concurrency safety, this promises no application
    /// state writes, commands, nested agents or capability discovery. Unknown
    /// implementations fail closed. A projection must keep the descriptor and
    /// all authorization checks, and isolate any actor-local mutable cache.
    /// Observing shared state is allowed; changing it or consuming its pending
    /// notifications is not. This is a trusted native implementation contract,
    /// not an OS sandbox or a claim that remote readOnlyHint is authoritative.
    /// Create a fresh projection per observer invocation, not per parent run.
    fn read_only_projection(self: Arc<Self>) -> Option<Arc<dyn Tool>> {
        None
    }

    /// Whether this tool should be **deferred**: hidden from the per-request
    /// `tools` array until the model explicitly discovers it via `tool_search`.
    /// Default: `false` (always loaded, current behavior).
    ///
    /// MCP tools override this to `true` so a server with dozens of tools
    /// doesn't burn token budget on every request. Built-ins generally stay
    /// `false` so the model sees the core toolset upfront.
    fn should_defer(&self) -> bool {
        false
    }

    /// Source advertised for discovery (for example an MCP server). Tools from
    /// one source share a catalog entry; individual names and schemas stay in
    /// the host registry until searched. This metadata grants no permissions.
    fn discovery_source(&self) -> Option<&str> {
        None
    }

    /// Whether this tool MUST always be loaded — overrides `should_defer`.
    /// Default: `false`. Used by `tool_search` itself (the discovery channel
    /// can never be deferred behind itself) and by escape-hatch built-ins
    /// that need to be reachable before any discovery roundtrip.
    fn always_load(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn h138_client_metadata_is_explicit_not_generic_serialization_or_debug() {
        let mut output = ToolOutput::success(serde_json::json!({"public":true}));
        output.client_metadata = Some(serde_json::json!({"detail":"h138-private-canary"}));
        let encoded = serde_json::to_string(&output).unwrap();
        assert!(!encoded.contains("client_metadata"));
        assert!(!format!("{output:?}").contains("h138-private-canary"));
        let nested = serde_json::json!({"nested":[output.clone()]});
        assert!(!nested.to_string().contains("h138-private-canary"));
        let decoded: ToolOutput = serde_json::from_str(&encoded).unwrap();
        assert!(decoded.client_metadata.is_none());
        assert_eq!(decoded.value, output.value);
    }

    #[test]
    fn output_helpers() {
        let ok = ToolOutput::success(serde_json::json!({"a": 1}));
        assert!(ok.ok);
        let err = ToolOutput::failure("boom");
        assert!(!err.ok);
        assert_eq!(err.value["error"], "boom");
    }

    /// A minimal stub tool that takes the trait defaults so we can verify the
    /// defer / always-load defaults are `false` without touching a live
    /// built-in. (Built-ins inherit these defaults; the explicit overrides
    /// live in the MCP adapter and `ToolSearchTool`.)
    struct StubTool;

    #[async_trait]
    impl Tool for StubTool {
        fn descriptor(&self) -> ToolDescriptor {
            ToolDescriptor {
                name: "stub".into(),
                description: "stub".into(),
                parameters: serde_json::json!({"type": "object"}),
                risk: RiskLevel::Safe,
                required_permissions: PermissionSet::read_only(),
            }
        }
        async fn invoke(&self, _: serde_json::Value) -> Result<ToolOutput> {
            Ok(ToolOutput::success(serde_json::json!(null)))
        }
    }

    #[test]
    fn deferral_defaults_are_false() {
        let t = StubTool;
        assert!(
            !t.should_defer(),
            "Tool::should_defer must default to false"
        );
        assert!(!t.always_load(), "Tool::always_load must default to false");
        assert!(Arc::new(StubTool).read_only_projection().is_none());
    }

    #[tokio::test]
    async fn execution_context_child_cancellation_isolated_from_parent() {
        let parent = ToolExecutionContext::from_token(CancellationToken::new());
        let child = parent.child();
        child.cancellation_token().cancel();

        assert!(child.is_cancelled());
        assert!(!parent.is_cancelled());

        parent.cancellation_token().cancel();
        assert!(child.is_cancelled());
        assert!(parent.is_cancelled());
    }
}
