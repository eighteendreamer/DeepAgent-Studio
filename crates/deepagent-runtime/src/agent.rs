//! The agent "brain" abstraction.
//!
//! The runtime loop is model-agnostic: it drives anything implementing
//! [`Agent`]. In production an [`Agent`] wraps a DeepSeek client (with Thinking
//! Mode reasoning persistence); in tests it is a deterministic stub. This keeps
//! the loop logic fully unit-testable without network access.

use async_trait::async_trait;

use deepagent_core::error::Result;
use deepagent_core::message::Message;
use deepagent_tools::ToolInvocation;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

/// Attempt-scoped receiver for complete tool calls discovered while a model
/// response is still streaming.
///
/// Implementations may parse, validate, normalize and classify calls eagerly,
/// but must not publish side effects until [`ToolAttemptController::commit`].
/// A failed provider attempt is always followed by `abort`, allowing the
/// runtime to discard every call/result associated with that attempt.
pub trait ToolAttemptController: Send {
    /// Start a provider attempt. Attempts are 1-based within one model turn.
    fn begin(&mut self, attempt: usize);

    /// A complete JSON object for one tool call arrived from the stream.
    fn prepare(&mut self, invocation: ToolInvocation);

    /// The provider stream completed successfully and this attempt may be used.
    fn commit(&mut self, attempt: usize);

    /// The provider stream failed or was cancelled. All attempt-local work must
    /// be cancelled/discarded before a retry can begin.
    fn abort(&mut self, attempt: usize, reason: &str);
}

/// What the agent decided to do on a given `think` step.
#[derive(Debug, Clone, PartialEq)]
pub enum AgentDecision {
    /// Invoke a tool. The loop will execute it and feed back an [`Observation`].
    CallTool(ToolInvocation),
    /// Invoke several tools requested in the same model turn. The loop executes
    /// them — concurrency-safe (read-only) ones in parallel, the rest
    /// sequentially — and feeds back all [`Observation`]s together. This mirrors
    /// Claude Code's parallel tool-calling: one assistant turn can carry many
    /// `tool_use` blocks.
    CallTools(Vec<ToolInvocation>),
    /// The task is finished; carry a final message.
    Complete(String),
    /// The task is finished; carry the full final assistant message, including
    /// provider-specific metadata such as DeepSeek `reasoning_content`.
    CompleteMessage(Message),
    /// The agent cannot proceed and needs human input / approval.
    NeedsApproval(String),
}

/// Feedback handed back to the agent after a tool runs (开发提示词.md §17).
#[derive(Debug, Clone, PartialEq)]
pub struct Observation {
    /// The tool that produced this observation.
    pub tool: String,
    /// Whether the tool succeeded.
    pub ok: bool,
    /// JSON output / error detail.
    pub output: serde_json::Value,
    /// The originating tool-call id, when known, so the agent can correlate this
    /// observation with the exact `tool_calls[].id` it emitted (required when a
    /// single turn produced multiple parallel tool calls).
    pub call_id: Option<String>,
}

impl Observation {
    /// Build an observation for `tool` with no correlated call id.
    pub fn new(tool: impl Into<String>, ok: bool, output: serde_json::Value) -> Self {
        Self {
            tool: tool.into(),
            ok,
            output,
            call_id: None,
        }
    }

    /// Attach the originating tool-call id (builder style).
    pub fn with_call_id(mut self, call_id: Option<String>) -> Self {
        self.call_id = call_id;
        self
    }
}

/// Cumulative token usage across an agent run (summed over every model call).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RunUsage {
    /// Prompt (input) tokens.
    pub prompt_tokens: u32,
    /// Completion (output) tokens.
    pub completion_tokens: u32,
    /// Total tokens.
    pub total_tokens: u32,
    /// Prompt tokens served from the context cache (a "hit").
    pub prompt_cache_hit_tokens: u32,
    /// Prompt tokens NOT served from cache (a "miss").
    pub prompt_cache_miss_tokens: u32,
}

/// The agent brain. Implementations decide the next [`AgentDecision`] given the
/// accumulated observations so far.
#[async_trait]
pub trait Agent: Send + Sync {
    /// Produce the next decision. `step` is the 0-based iteration index;
    /// `last` carries the observations from the previous step (empty on the
    /// first step; more than one when the previous step ran tools in parallel).
    async fn think(&mut self, step: usize, last: &[Observation]) -> Result<AgentDecision>;

    /// Cancel-aware decision path. Implementations that can interrupt blocking
    /// work should override this; the default preserves existing agent
    /// implementations and checks only before starting the old `think`.
    async fn think_cancelled(
        &mut self,
        step: usize,
        last: &[Observation],
        cancel: Option<Arc<AtomicBool>>,
    ) -> Result<AgentDecision> {
        if cancel
            .as_ref()
            .map(|flag| flag.load(std::sync::atomic::Ordering::Relaxed))
            .unwrap_or(false)
        {
            return Err(deepagent_core::error::CoreError::other("request cancelled"));
        }
        self.think(step, last).await
    }

    /// Cancel-aware decision path with an attempt-scoped streaming tool-call
    /// receiver. Non-model agents keep the old behavior through this default.
    async fn think_streaming_cancelled(
        &mut self,
        step: usize,
        last: &[Observation],
        cancel: Option<Arc<AtomicBool>>,
        _tools: Option<&mut dyn ToolAttemptController>,
    ) -> Result<AgentDecision> {
        self.think_cancelled(step, last, cancel).await
    }

    /// Cumulative token usage observed so far this run. Defaults to none (for
    /// agents that don't track it); model-backed agents sum each call's usage.
    fn cumulative_usage(&self) -> Option<RunUsage> {
        None
    }
}

#[cfg(test)]
pub(crate) mod testing {
    use super::*;
    use std::collections::VecDeque;

    /// A scripted agent that returns a fixed sequence of decisions, for tests.
    pub struct ScriptedAgent {
        pub script: VecDeque<AgentDecision>,
        pub observations: Vec<Observation>,
    }

    impl ScriptedAgent {
        pub fn new(decisions: impl IntoIterator<Item = AgentDecision>) -> Self {
            Self {
                script: decisions.into_iter().collect(),
                observations: Vec::new(),
            }
        }
    }

    #[async_trait]
    impl Agent for ScriptedAgent {
        async fn think(&mut self, _step: usize, last: &[Observation]) -> Result<AgentDecision> {
            self.observations.extend(last.iter().cloned());
            Ok(self
                .script
                .pop_front()
                .unwrap_or_else(|| AgentDecision::Complete("script exhausted".into())))
        }
    }
}
