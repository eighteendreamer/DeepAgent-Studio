//! The agent "brain" abstraction.
//!
//! The runtime loop is model-agnostic: it drives anything implementing
//! [`Agent`]. In production an [`Agent`] wraps a DeepSeek client (with Thinking
//! Mode reasoning persistence); in tests it is a deterministic stub. This keeps
//! the loop logic fully unit-testable without network access.

use async_trait::async_trait;

use deepagent_core::error::Result;
use deepagent_core::message::Message;
use deepagent_core::response_item::ResponseOutputItem;
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
    Continue,
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
    /// The task is finished; carry the provider-native Responses output items
    /// plus the UI/verification text projection. Model-backed agents use this
    /// so the runtime loop can persist exact `ResponseOutputItem` semantics at
    /// the completion boundary instead of relying on the legacy `Message`
    /// projection as the source of truth.
    CompleteItems {
        /// UI/verification projection derived from `items`.
        message: Message,
        /// Provider-native Responses output items for this final turn.
        items: Vec<ResponseOutputItem>,
    },
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
    pub raw_output: Option<serde_json::Value>,
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
            raw_output: None,
            call_id: None,
        }
    }

    /// Attach the originating tool-call id (builder style).
    pub fn with_call_id(mut self, call_id: Option<String>) -> Self {
        self.call_id = call_id;
        self
    }
}

/// Token usage for the current run, carried in two distinct semantics.
///
/// `prompt_tokens`, `total_tokens` and the cache tokens describe the **latest
/// answered request**: they are the context gauge that the usage event, the
/// popover and the auto-compact check read. `billed_*`, `completion_tokens` and
/// `reasoning_tokens` sum over **every** request the run made, which is what the
/// cost ledger books — a run of 60 steps paid for 60 prompts, not one.
/// [`RunUsage::record_call`] is the only place either rule is written.
///
/// **Important**: [`RuntimeEvent::Usage`] emits **per-turn** values (the provider's
/// raw report for THIS request), not these cumulative fields. The frontend displays
/// per-turn usage in `TurnFooter`; cumulative billing totals come from `CostSummary`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RunUsage {
    /// Prompt (input) tokens reported by the most recent provider response
    /// (last-wins, not cumulative). This is the current context window size.
    pub prompt_tokens: u32,
    /// Completion (output) tokens, **cumulative** across the run (for billing).
    /// `RuntimeEvent::Usage` emits the per-turn value, not this cumulative field.
    pub completion_tokens: u32,
    /// Reasoning output tokens (a subset of completion tokens), **cumulative**.
    pub reasoning_tokens: u32,
    /// Total tokens reported by the most recent provider response (last-wins).
    pub total_tokens: u32,
    /// Prompt tokens served from the context cache, last-wins.
    pub prompt_cache_hit_tokens: u32,
    /// Prompt tokens NOT served from cache, last-wins.
    pub prompt_cache_miss_tokens: u32,
    /// Prompt tokens summed over every answered request in the run (billing ledger).
    pub billed_prompt_tokens: u32,
    /// Cache-hit prompt tokens summed over every answered request (billing ledger).
    pub billed_cache_hit_tokens: u32,
    /// Cache-miss prompt tokens summed over every answered request (billing ledger).
    pub billed_cache_miss_tokens: u32,
}

impl RunUsage {
    /// Fold one answered provider call into the run.
    ///
    /// Every step of the loop resends the whole conversation, so a request's
    /// `prompt_tokens` is a fresh total of the context, not an increment: the
    /// gauge replaces it while the billed sum adds it. Retries of the *same*
    /// request keep replacing in both (see `model_compactor::fold_usage`), the
    /// same per-step rule harness uses (`usage-projection.ts` subtracts the
    /// previous record only for the same turn and step).
    pub fn record_call(&mut self, call: &deepagent_models::Usage) {
        self.prompt_tokens = call.prompt_tokens;
        self.total_tokens = call.total_tokens;
        self.prompt_cache_hit_tokens = call.prompt_cache_hit_tokens;
        self.prompt_cache_miss_tokens = call.prompt_cache_miss_tokens;
        self.completion_tokens += call.completion_tokens;
        self.reasoning_tokens += call.reasoning_tokens;
        self.billed_prompt_tokens += call.prompt_tokens;
        self.billed_cache_hit_tokens += call.prompt_cache_hit_tokens;
        self.billed_cache_miss_tokens += call.prompt_cache_miss_tokens;
    }
}

/// Provider usage of a single answered call, expressed as run usage.
///
/// Side calls that bypass the agent loop (pre-run compaction) report one
/// provider `Usage`; the ledger speaks [`RunUsage`]. For a single call the
/// gauge and the billed sum are the same numbers.
impl From<deepagent_models::Usage> for RunUsage {
    fn from(usage: deepagent_models::Usage) -> Self {
        let mut out = Self::default();
        out.record_call(&usage);
        out
    }
}

/// The agent brain. Implementations decide the next [`AgentDecision`] given the
/// accumulated observations so far.
#[async_trait]
pub trait Agent: Send + Sync {
    /// Attach the run's canonical event sink.
    ///
    /// [`crate::kernel::AgentKernel`] calls this after it has wrapped the live
    /// delegate with durable `run_events` persistence. Model-backed agents use
    /// the sink for provider-stream events; deterministic/custom agents may
    /// keep the default no-op implementation.
    fn set_event_sink(&mut self, _events: Arc<dyn crate::events::RuntimeEventSink>) {}

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

    /// Usage of model calls the loop did not make itself — compaction summaries,
    /// including background prefire passes.
    ///
    /// Reported *beside* [`Self::cumulative_usage`] so the cost ledger stays
    /// complete while the per-turn token display keeps describing only the
    /// sampling loop. Hosts must book it as its own ledger entry.
    fn side_usage(&self) -> Option<RunUsage> {
        None
    }

    /// Seed the ids of memories already surfaced earlier in this session,
    /// rebuilt from the replayed event stream. Without it a resumed run would
    /// re-inject the same entries (the de-dup set used to be run-local).
    /// Non-model agents have no memory track and keep the default.
    fn seed_surfaced_memories(&mut self, _ids: &[String]) {}

    /// Drain the background-memory injections produced during this run as
    /// `(entry ids, prefetch latency ms)` so the loop can persist them as
    /// durable `MemoriesInjected` events. Defaults to empty.
    fn drain_memory_injections(&mut self) -> Vec<(Vec<String>, u64)> {
        Vec::new()
    }

    /// Drain provider-native Responses output items produced by the most
    /// recent model turn. Non-model agents return nothing; model-backed
    /// agents use this to persist exact item semantics alongside the legacy
    /// UI `Message` projection.
    fn take_pending_response_items(&mut self) -> Vec<ResponseOutputItem> {
        Vec::new()
    }

    /// Drain raw provider Responses usage objects reported since the previous
    /// runtime checkpoint. Default empty for non-model agents.
    fn take_pending_raw_usage(&mut self) -> Vec<serde_json::Value> {
        Vec::new()
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

#[cfg(test)]
mod tests {
    use super::RunUsage;
    use deepagent_models::Usage;

    fn call(prompt: u32, completion: u32, hit: u32, miss: u32) -> Usage {
        Usage {
            prompt_tokens: prompt,
            completion_tokens: completion,
            total_tokens: prompt + completion,
            prompt_cache_hit_tokens: hit,
            prompt_cache_miss_tokens: miss,
            ..Default::default()
        }
    }

    #[test]
    fn folding_a_run_keeps_the_gauge_latest_and_the_bill_cumulative() {
        let mut run = RunUsage::default();
        for usage in [
            call(1_000, 20, 800, 200),
            call(1_200, 30, 1_000, 200),
            call(1_500, 40, 1_200, 300),
        ] {
            run.record_call(&usage);
        }

        assert_eq!(
            run.prompt_tokens, 1_500,
            "the gauge is the latest request's context: that is what the auto-compact \
             threshold and the occupancy popover compare against"
        );
        assert_eq!(run.total_tokens, 1_540);
        assert_eq!(run.prompt_cache_hit_tokens, 1_200);
        assert_eq!(
            run.billed_prompt_tokens, 3_700,
            "each step resent the conversation and was billed for it: 1000 + 1200 + 1500"
        );
        assert_eq!(run.billed_cache_hit_tokens, 3_000);
        assert_eq!(run.billed_cache_miss_tokens, 700);
        assert_eq!(
            run.completion_tokens, 90,
            "output tokens are genuine per-step deltas, so they sum"
        );
    }

    #[test]
    fn a_single_call_books_exactly_what_it_measured() {
        let run = RunUsage::from(call(900, 60, 700, 200));
        assert_eq!(run.prompt_tokens, run.billed_prompt_tokens);
        assert_eq!(run.prompt_cache_hit_tokens, run.billed_cache_hit_tokens);
        assert_eq!(run.prompt_cache_miss_tokens, run.billed_cache_miss_tokens);
        assert_eq!(run.total_tokens, 960);
    }
}
