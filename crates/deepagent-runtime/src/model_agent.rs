//! A model-driven [`Agent`] implementation.
//!
//! [`ModelAgent`] turns the scripted demo brain into a real one backed by a
//! [`ModelClient`]. It maintains the running conversation, advertises the
//! available tools, and translates the model's streamed [`Response`] into
//! the runtime's [`AgentDecision`] vocabulary.
//!
//! Thinking Mode persistence (开发计划.md Phase 2 §5): when the model returns a
//! turn includes `reasoning_content`, the assistant message preserves it so the
//! session event log can replay the same thinking trace after refresh.

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use async_trait::async_trait;

use deepagent_core::error::{CoreError, Result};
use deepagent_core::message::{Message, Role};
use deepagent_core::response_item::{ResponseInputItem, ResponseOutputItem};
use deepagent_models::chat::{FinishReason, ThinkingDepth};
use deepagent_models::{
    classify_model_error, DeltaObserver, ModelClient, ModelFailureKind, ModelStreamEvent, Response,
    ResponseRequest, ToolSchema,
};
use deepagent_tools::ToolInvocation;

use crate::agent::{Agent, AgentDecision, Observation, ToolAttemptController};
use crate::events::{RuntimeEvent, RuntimeEventSink};
use crate::stall_detector::{
    build_stall_nudge, evaluate_stall, render_stall_transcript_from_response_items,
    StallClassifier, StallDecision, MAX_STALL_NUDGES_PER_RUN,
};

#[derive(Debug, Clone)]
pub struct ReactiveCompaction {
    pub messages: Vec<Message>,
    pub tokens_before: u64,
    pub tokens_after: u64,
    pub summary: String,
}

/// Cached result of a background prefire pass-1 (Grok two-pass compaction).
/// Produced by [`ReactiveContextCompactor::prefire_pass1`] against a stable
/// conversation prefix and later applied by
/// [`ReactiveContextCompactor::apply_prefire`] against the (possibly grown)
/// live conversation. Mirrors Grok `AsyncCompactionCache`
/// (`grok-build/.../session/compaction_config.rs` L38-55).
#[derive(Debug, Clone)]
pub struct PrefireNote {
    /// Pass-1 summary text of the summarized prefix (the NOTE₁).
    pub note: String,
    /// Number of leading live-conversation messages the summary covers (the
    /// prefix boundary as of pass-1; the pass-2 tail is `messages[prefix_end..]`).
    pub prefix_end: usize,
    /// Fingerprint of `messages[..prefix_end]` at pass-1 time. Apply only when
    /// the current conversation still starts with this exact prefix
    /// (edit/rewind/branch changes it → discard).
    pub fingerprint: u64,
}

/// Why a context compaction was requested. Mirrors Claude Code's split
/// between the proactive per-turn threshold check (`autoCompact.ts::
/// shouldAutoCompact`, fired before every API request) and the reactive
/// prompt-too-long recovery path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactionTrigger {
    /// Proactive: the estimated context crossed the auto-compact threshold
    /// before a model request.
    AutoCompactThreshold,
    /// Reactive: the provider rejected the request as over the context limit.
    ContextOverflow,
}

impl CompactionTrigger {
    /// Stable snake_case label for hooks/events/logs.
    pub fn as_str(&self) -> &'static str {
        match self {
            CompactionTrigger::AutoCompactThreshold => "auto_compact_threshold",
            CompactionTrigger::ContextOverflow => "context_overflow",
        }
    }
}

#[async_trait]
pub trait ReactiveContextCompactor: Send + Sync {
    async fn compact(
        &self,
        messages: &[Message],
        trigger: CompactionTrigger,
    ) -> Result<Option<ReactiveCompaction>>;

    /// Prefire pass-1 (Grok two-pass): summarize the stable prefix of
    /// `messages` into a cacheable [`PrefireNote`], run off the critical path.
    /// Default: no prefire support (`None`) — keeps non-model compactors and
    /// tests unaffected.
    async fn prefire_pass1(&self, _messages: &[Message]) -> Result<Option<PrefireNote>> {
        Ok(None)
    }

    /// Prefire pass-2 apply: if `note` still matches the current conversation
    /// prefix (fingerprint check), combine it with the live tail into a full
    /// compaction without a blocking summary call. Default: `None` (caller
    /// falls back to the synchronous [`Self::compact`]).
    async fn apply_prefire(
        &self,
        _note: &PrefireNote,
        _messages: &[Message],
    ) -> Result<Option<ReactiveCompaction>> {
        Ok(None)
    }
}

/// One background-prefetched memory ready for injection (§3.2, Claude Code
/// `relevant_memories` attachment). `id` is a stable entry id used for
/// per-session de-duplication; `block` is the already-rendered content
/// (without the surrounding `<system-reminder>` wrapper).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelevantMemory {
    /// Stable entry id (scope-qualified) for session de-dup.
    pub id: String,
    /// Rendered memory block (title + excerpt), ready to inject.
    pub block: String,
}

/// Retrieves memories relevant to the evolving conversation, off the critical
/// path (Claude Code `startRelevantMemoryPrefetch`). The runtime calls this in
/// a background task at the start of a turn; when it settles, fresh entries are
/// injected as a `<system-reminder>` for the *next* request. Injected via a
/// trait so the runtime crate stays free of any knowledge/app-core dependency
/// (parity with [`ReactiveContextCompactor`]).
#[async_trait]
pub trait RelevantMemoryProvider: Send + Sync {
    /// Retrieve memories relevant to `query`, excluding ids in
    /// `already_surfaced`. Best-first, already length/score filtered by the
    /// implementation. An empty result means nothing relevant (not an error).
    async fn fetch_relevant(
        &self,
        query: &str,
        already_surfaced: &[String],
    ) -> Result<Vec<RelevantMemory>>;
}

/// Cap on distinct memories surfaced by the background prefetch across one
/// run. Bounds cumulative injection (Claude Code caps by
/// `RELEVANT_MEMORIES_CONFIG.MAX_SESSION_BYTES`; we cap by entry count since
/// our entries are short knowledge excerpts). After this the prefetch stops.
const MAX_SURFACED_MEMORIES_PER_RUN: usize = 12;

/// Current todo state for the periodic reminder (§3.1 attachment breadth).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TodoReminderSnapshot {
    /// Whether the list currently has any items.
    pub has_items: bool,
    /// Rendered list (embeddable in the reminder body).
    pub rendered: String,
}

/// Supplies the current todo list for the periodic on-plan reminder. Injected
/// via a trait so the runtime crate stays free of the `TodoStore` concept
/// (parity with [`RelevantMemoryProvider`] / [`ReactiveContextCompactor`]).
pub trait TodoReminderSource: Send + Sync {
    /// Snapshot the current todo list for a reminder.
    fn todo_snapshot(&self) -> TodoReminderSnapshot;
}

/// Assistant turns of todo inactivity before a periodic todo reminder fires
/// (both "turns since last todo_write" and "turns since last reminder" must
/// cross it). Aligned with Claude Code `attachments.ts::TODO_REMINDER_CONFIG`
/// (TURNS_SINCE_WRITE = TURNS_BETWEEN_REMINDERS = 10).
const TODO_REMINDER_TURN_THRESHOLD: usize = 10;

/// Sliding-window size for wall-banging detection across tool observations.
const FAILURE_WINDOW: usize = 12;
/// Same-tool failures inside the window that force a strategy-change hint.
const STRATEGY_SHIFT_THRESHOLD: usize = 4;
/// Stop proactive auto-compact after this many consecutive failures for the
/// rest of the run. Aligned with Claude Code
/// `autoCompact.ts::MAX_CONSECUTIVE_AUTOCOMPACT_FAILURES` (3).
const MAX_CONSECUTIVE_AUTOCOMPACT_FAILURES: u32 = 3;
/// Multi-turn "continue" recoveries after a max-tokens truncation (post
/// escalation). Aligned with Claude Code
/// `query.ts::MAX_OUTPUT_TOKENS_RECOVERY_LIMIT` (3).
const MAX_OUTPUT_TOKENS_RECOVERY_LIMIT: usize = 3;
/// Injected user message that resumes a max-tokens-truncated turn. Wording
/// mirrors Claude Code's recovery message (query.ts L1225).
const MAX_OUTPUT_RECOVERY_PROMPT: &str = "Output token limit hit. Resume directly — no apology, \
    no recap of what you were doing. Pick up mid-thought if that is where the cut happened. \
    Break remaining work into smaller pieces.";
/// Cap on 429 rate-limit attempts, lower than the general retry budget.
/// Aligned with Grok `retry.rs::RATE_LIMIT_RETRY_THRESHOLD` (2): rate-limit
/// waits can be long, so escalate to the caller instead of burning backoff.
const RATE_LIMIT_RETRY_THRESHOLD: usize = 2;
/// Inject a context-efficiency snip nudge after roughly this much token
/// growth without a snip. Aligned with Claude Code
/// `attachments.ts::getContextEfficiencyAttachment` (“every N tokens of
/// growth without a snip”, ~10k pacing).
const SNIP_NUDGE_INTERVAL_TOKENS: u64 = 10_000;
/// Never snip inside the most recent messages: the live exchange (and its
/// tool pairing) must survive. Parity with the compactor's
/// KEEP_RECENT_MESSAGES protected tail.
const SNIP_PROTECTED_RECENT_MESSAGES: usize = 8;
/// Consecutive identical tool calls (same name + arguments) that trip the
/// client-side doom-loop detector. Grok's doom-loop is a SERVER signal
/// (`x-grok-doom-loop-check`); DeepSeek has no such header, so this is the
/// pure-client heuristic the upgrade doc specifies (repeated identical tool
/// calls / outputs), grounded in Grok's `progress_signature` stall idea.
const DOOM_LOOP_REPEAT_THRESHOLD: usize = 3;

/// Provider-native Responses conversation state.
///
/// `ModelAgent` still keeps `messages` as a UI/compaction/stall transcript
/// projection, but every provider request is built from this structure. This
/// mirrors Codex's item-native Responses history model and prevents new tool
/// turns from being re-derived through the old chat `Message.tool_calls`
/// projection.
#[derive(Debug, Clone, Default)]
struct ResponseHistory {
    instructions: Option<String>,
    items: Vec<ResponseInputItem>,
}

impl ResponseHistory {
    fn from_seed(system: String, goal: String) -> Self {
        Self {
            instructions: Some(system),
            items: vec![ResponseInputItem::Message {
                role: "user".to_string(),
                content: goal,
            }],
        }
    }

    fn from_messages(messages: &[Message]) -> Self {
        let (instructions, items) = deepagent_models::response_items_from_messages(messages);
        Self {
            instructions,
            items,
        }
    }

    fn request(&self, model: impl Into<String>) -> ResponseRequest {
        ResponseRequest::from_response_items(model, self.instructions.clone(), self.items.clone())
    }

    fn replace_from_messages(&mut self, messages: &[Message]) {
        *self = Self::from_messages(messages);
    }

    fn push_message_projection(&mut self, message: &Message) {
        match message.role {
            Role::System => {
                self.instructions = Some(match self.instructions.take() {
                    Some(existing) if !existing.is_empty() => {
                        format!("{existing}\n\n{}", message.content)
                    }
                    _ => message.content.clone(),
                });
            }
            Role::Tool => {
                let call_id = message.tool_call_id.clone().unwrap_or_default();
                self.push_tool_output(call_id, message.content.clone());
            }
            _ => {
                if let Some(reasoning) = message
                    .reasoning_content
                    .as_deref()
                    .filter(|text| !text.is_empty())
                {
                    self.items.push(ResponseInputItem::Reasoning {
                        id: None,
                        content: reasoning.to_string(),
                    });
                }
                for call in &message.tool_calls {
                    if call.name == "apply_patch" {
                        self.items.push(ResponseInputItem::CustomToolCall {
                            call_id: call.id.clone(),
                            name: call.name.clone(),
                            input: call
                                .arguments
                                .get("patch")
                                .and_then(serde_json::Value::as_str)
                                .unwrap_or_default()
                                .to_string(),
                        });
                    } else {
                        self.items.push(ResponseInputItem::FunctionCall {
                            call_id: call.id.clone(),
                            name: call.name.clone(),
                            arguments: serde_json::to_string(&call.arguments)
                                .unwrap_or_else(|_| "{}".to_string()),
                        });
                    }
                }
                if !message.content.is_empty() || message.tool_calls.is_empty() {
                    self.items.push(ResponseInputItem::Message {
                        role: message.role.as_str().to_string(),
                        content: message.content.clone(),
                    });
                }
            }
        }
    }

    fn push_tool_output(&mut self, call_id: String, output: String) {
        if self.is_custom_call(&call_id) {
            self.items
                .push(ResponseInputItem::CustomToolCallOutput { call_id, output });
        } else {
            self.items
                .push(ResponseInputItem::FunctionCallOutput { call_id, output });
        }
    }

    fn is_custom_call(&self, call_id: &str) -> bool {
        self.items.iter().any(|item| {
            matches!(
                item,
                ResponseInputItem::CustomToolCall {
                    call_id: existing,
                    ..
                } if existing == call_id
            )
        })
    }

    fn extend_output_items(&mut self, items: &[ResponseOutputItem]) {
        self.items.extend(items.iter().cloned());
    }
}

/// An [`Agent`] that delegates decision-making to a model.
pub struct ModelAgent {
    client: Arc<ModelClient>,
    model: String,
    /// Running conversation, seeded with system + user messages.
    messages: Vec<Message>,
    /// Provider-native Responses history used for every model request.
    response_history: ResponseHistory,
    /// Tool schemas advertised to the model each turn.
    tools: Vec<ToolSchema>,
    /// Most recent tool call id, used to correlate the next tool result.
    pending_tool_call_id: Option<String>,
    /// Optional live event sink: when set, token/reasoning deltas are forwarded
    /// as [`RuntimeEvent`]s for streaming to a UI.
    events: Option<Arc<dyn RuntimeEventSink>>,
    /// Cumulative token usage summed across every model call this run.
    usage: crate::agent::RunUsage,
    /// Provider-native Responses output items from the latest completed model
    /// turn, drained by the runtime loop into the append-only session log.
    pending_response_items: Vec<ResponseOutputItem>,
    pending_raw_usage: Vec<serde_json::Value>,
    /// DeepSeek Thinking Mode depth applied to every request.
    thinking_depth: ThinkingDepth,
    /// Provider attempts per turn for transient/empty-stream failures.
    max_model_attempts: usize,
    /// Optional same-endpoint model used once when the primary deployment is
    /// overloaded. Authentication/rate-limit errors do not trigger fallback.
    fallback_model: Option<String>,
    reactive_compactor: Option<Arc<dyn ReactiveContextCompactor>>,
    /// Proactive auto-compact threshold in tokens (Claude Code
    /// `getAutoCompactThreshold`). `None` disables the per-turn check.
    proactive_compaction_threshold: Option<u64>,
    /// Consecutive proactive auto-compact failures (circuit breaker input).
    autocompact_consecutive_failures: u32,
    /// `prompt + completion` tokens reported by the most recent model call —
    /// the usage-grounded context size estimate (Claude Code
    /// `tokenCountWithEstimation` reads usage off the last assistant turn).
    last_call_context_tokens: u64,
    /// Tokens freed by snips since the last provider usage report. Subtracted
    /// from the usage-based estimate, which cannot see the removal yet
    /// (Claude Code plumbs `snipTokensFreed` into `shouldAutoCompact`).
    snip_tokens_freed_unreflected: u64,
    /// Name of the registered history-snip tool, when available to this run.
    snip_tool: Option<String>,
    /// Estimated context size at the last snip nudge (or last snip/compact).
    snip_nudge_baseline_tokens: Option<u64>,
    /// Prefire (Grok two-pass) start line in tokens: below the real
    /// `proactive_compaction_threshold` by the lead margin. When the estimate
    /// crosses this, a background pass-1 is scheduled. `None` disables prefire.
    prefire_start_threshold: Option<u64>,
    /// Cached pass-1 note ready for a fast pass-2 apply at the real threshold.
    prefire_cache: Option<PrefireNote>,
    /// In-flight background pass-1 task; awaited if compaction fires first.
    prefire_handle: Option<tokio::task::JoinHandle<Result<Option<PrefireNote>>>>,
    /// Sliding window of recent tool outcomes `(tool, ok)`, used to detect a
    /// wall-banging loop: many failures of the same tool in a short span even
    /// with unrelated successes interleaved. Drives the escalated recovery
    /// hint that forces a strategy change instead of endless retry variants.
    recent_tool_results: std::collections::VecDeque<(String, bool)>,
    /// Sliding window of recent tool-call signatures (name + arguments), for
    /// the client-side doom-loop detector (repeated identical calls = stall).
    recent_call_signatures: std::collections::VecDeque<String>,
    /// Signature already nudged for the current stall, so the doom-loop nudge
    /// fires once per stall (not every turn once the threshold is crossed).
    doom_loop_nudged_signature: Option<String>,
    /// Background relevant-memory prefetch provider (§3.2). `None` disables it.
    memory_provider: Option<Arc<dyn RelevantMemoryProvider>>,
    /// In-flight background prefetch task; polled non-blocking each turn.
    memory_prefetch_handle: Option<tokio::task::JoinHandle<Result<Vec<RelevantMemory>>>>,
    /// When the in-flight prefetch started (for injection-latency telemetry).
    memory_prefetch_started_at: Option<std::time::Instant>,
    /// The user query the last prefetch was fired for — a new query (new user
    /// turn) triggers a fresh prefetch; identical queries do not re-fire.
    last_prefetch_query: Option<String>,
    /// Entry ids already surfaced this run (session de-dup + cap).
    surfaced_memory_ids: std::collections::HashSet<String>,
    /// Periodic todo-reminder source (§3.1). `None` disables it.
    todo_source: Option<Arc<dyn TodoReminderSource>>,
    /// Assistant turns since the model last called `todo_write`.
    turns_since_todo_write: usize,
    /// Assistant turns since the last periodic todo reminder was injected.
    turns_since_todo_reminder: usize,
    /// Stall/laziness classifier for final answers (§2.3, Grok
    /// `laziness_classifier.rs`). `None` disables the check entirely.
    stall_classifier: Option<Arc<dyn StallClassifier>>,
    /// Advisory stall nudges already injected this run (cap:
    /// [`MAX_STALL_NUDGES_PER_RUN`]).
    stall_nudges_used: u32,
    /// Total tool calls actually issued this run — the tamper-proof
    /// `tool_calls_made` fact in the stall transcript's `[runtime_state]`.
    tool_calls_made: usize,
    /// Run start instant for the `turn_elapsed_seconds` runtime-state fact.
    run_started_at: std::time::Instant,
}

impl ModelAgent {
    /// Build a model agent.
    ///
    /// `system` is the system prompt; `goal` is the user's task. `tools` are the
    /// schemas the model may call (typically derived from the
    /// `ToolRegistry`'s visible set for the agent's permissions).
    pub fn new(
        client: Arc<ModelClient>,
        model: impl Into<String>,
        system: impl Into<String>,
        goal: impl Into<String>,
        tools: Vec<ToolSchema>,
    ) -> Self {
        let system = system.into();
        let goal = goal.into();
        Self {
            client,
            model: model.into(),
            messages: vec![Message::system(system.clone()), Message::user(goal.clone())],
            response_history: ResponseHistory::from_seed(system, goal),
            tools,
            pending_tool_call_id: None,
            events: None,
            usage: crate::agent::RunUsage::default(),
            pending_response_items: Vec::new(),
            pending_raw_usage: Vec::new(),
            thinking_depth: ThinkingDepth::default(),
            max_model_attempts: 3,
            fallback_model: None,
            reactive_compactor: None,
            proactive_compaction_threshold: None,
            autocompact_consecutive_failures: 0,
            last_call_context_tokens: 0,
            snip_tokens_freed_unreflected: 0,
            snip_tool: None,
            snip_nudge_baseline_tokens: None,
            prefire_start_threshold: None,
            prefire_cache: None,
            prefire_handle: None,
            recent_tool_results: std::collections::VecDeque::new(),
            recent_call_signatures: std::collections::VecDeque::new(),
            doom_loop_nudged_signature: None,
            memory_provider: None,
            memory_prefetch_handle: None,
            memory_prefetch_started_at: None,
            last_prefetch_query: None,
            surfaced_memory_ids: std::collections::HashSet::new(),
            todo_source: None,
            turns_since_todo_write: 0,
            // Start eligible on the reminder axis so the write-inactivity axis
            // is the effective gate for the first reminder.
            turns_since_todo_reminder: TODO_REMINDER_TURN_THRESHOLD,
            stall_classifier: None,
            stall_nudges_used: 0,
            tool_calls_made: 0,
            run_started_at: std::time::Instant::now(),
        }
    }

    fn push_message(&mut self, message: Message) {
        self.response_history.push_message_projection(&message);
        self.messages.push(message);
    }

    fn push_tool_observation(&mut self, call_id: String, tool_name: &str, content: String) {
        if tool_name == "apply_patch" && !self.response_history.is_custom_call(&call_id) {
            tracing::warn!(
                call_id = call_id.as_str(),
                "apply_patch observation did not find a matching custom_tool_call; preserving call_id on custom output"
            );
            self.response_history
                .items
                .push(ResponseInputItem::CustomToolCallOutput {
                    call_id: call_id.clone(),
                    output: content.clone(),
                });
        } else {
            self.response_history
                .push_tool_output(call_id.clone(), content.clone());
        }
        // Keep the provider input item-native while maintaining the
        // compatibility projection used by UI rendering, compaction, and stall
        // transcripts. Do not route this through `push_message`, or it would
        // re-derive a second Responses output item from the tool-role message.
        self.messages.push(Message::tool_result(call_id, content));
    }

    fn replace_messages(&mut self, messages: Vec<Message>) {
        self.messages = messages;
        self.response_history.replace_from_messages(&self.messages);
    }

    fn request_for_current_history(&self) -> ResponseRequest {
        self.response_history.request(self.model.clone())
    }

    fn push_provider_output_items(&mut self, assistant: Message, items: &[ResponseOutputItem]) {
        self.response_history.extend_output_items(items);
        self.messages.push(assistant);
    }

    /// Attach a live event sink so token/reasoning deltas stream out as
    /// [`RuntimeEvent`]s (builder style).
    pub fn with_events(mut self, events: Arc<dyn RuntimeEventSink>) -> Self {
        self.events = Some(events);
        self
    }

    /// Attach the user's DeepSeek Thinking Mode depth.
    pub fn with_thinking_depth(mut self, depth: ThinkingDepth) -> Self {
        self.thinking_depth = depth;
        self
    }

    pub fn with_max_model_attempts(mut self, attempts: usize) -> Self {
        self.max_model_attempts = attempts.max(1);
        self
    }

    pub fn with_fallback_model(mut self, model: Option<String>) -> Self {
        self.fallback_model = model
            .map(|model| model.trim().to_string())
            .filter(|model| !model.is_empty() && model != &self.model);
        self
    }

    pub fn with_reactive_compactor(mut self, compactor: Arc<dyn ReactiveContextCompactor>) -> Self {
        self.reactive_compactor = Some(compactor);
        self
    }

    /// Enable the proactive per-turn auto-compact check with the given token
    /// threshold (builder style). Requires a reactive compactor to act.
    pub fn with_proactive_compaction(mut self, threshold_tokens: u64) -> Self {
        self.proactive_compaction_threshold = Some(threshold_tokens);
        self
    }

    /// Enable prefire (Grok two-pass) background pass-1 with the given start
    /// line in tokens (below the proactive threshold by the lead margin).
    /// Requires a reactive compactor that implements `prefire_pass1`/
    /// `apply_prefire`; otherwise it is a harmless no-op.
    pub fn with_prefire(mut self, start_threshold_tokens: u64) -> Self {
        self.prefire_start_threshold = Some(start_threshold_tokens);
        self
    }

    /// Attach a background relevant-memory prefetch provider (§3.2, Claude Code
    /// `startRelevantMemoryPrefetch`). When set, each new user turn fires an
    /// off-critical-path retrieval; settled results inject as a
    /// `<system-reminder>` before the next request. No-op when `None`.
    pub fn with_relevant_memory_provider(
        mut self,
        provider: Arc<dyn RelevantMemoryProvider>,
    ) -> Self {
        self.memory_provider = Some(provider);
        self
    }

    /// Attach a periodic todo-reminder source (§3.1, Claude Code
    /// `getTodoReminderAttachments`). When set, after
    /// [`TODO_REMINDER_TURN_THRESHOLD`] turns without a `todo_write` (and since
    /// the last reminder), a gentle on-plan `<system-reminder>` is injected.
    /// No-op when `None`.
    pub fn with_todo_reminder_source(mut self, source: Arc<dyn TodoReminderSource>) -> Self {
        self.todo_source = Some(source);
        self
    }

    /// Attach a stall/laziness classifier (§2.3, Grok `laziness_classifier.rs`).
    /// When set, a final answer (no tool calls) is classified against the
    /// conversation tail; a confident stalled verdict injects one advisory
    /// nudge (cap [`MAX_STALL_NUDGES_PER_RUN`]) and re-enters the model turn.
    /// Advisory only — the classifier can never fail or block a run (fail-open,
    /// and after the cap every verdict passes the answer through). No-op when
    /// `None`.
    pub fn with_stall_classifier(mut self, classifier: Arc<dyn StallClassifier>) -> Self {
        self.stall_classifier = Some(classifier);
        self
    }

    /// Register the history-snip tool name so snip results mutate the live
    /// conversation and periodic context-efficiency nudges are injected.
    /// Also tags every seeded non-meta user message with a stable `[id:uN]`
    /// marker the model can reference (Claude Code messages.ts L2345-2364
    /// appends id tags to user messages when HISTORY_SNIP is enabled).
    /// Call after `with_history` so seeded turns are tagged too.
    pub fn with_snip_tool(mut self, name: impl Into<String>) -> Self {
        self.snip_tool = Some(name.into());
        let mut next_id = 0usize;
        for message in &mut self.messages {
            if message.role == deepagent_core::message::Role::User
                && !message.content.starts_with('[')
                && !message.content.starts_with("<system-reminder>")
            {
                next_id += 1;
                if !message.content.contains("[id:u") {
                    message.content.push_str(&format!("\n[id:u{next_id}]"));
                }
            }
        }
        let mut next_item_id = 0usize;
        for item in &mut self.response_history.items {
            let ResponseInputItem::Message { role, content } = item else {
                continue;
            };
            if role == "user"
                && !content.starts_with('[')
                && !content.starts_with("<system-reminder>")
            {
                next_item_id += 1;
                if !content.contains("[id:u") {
                    content.push_str(&format!("\n[id:u{next_item_id}]"));
                }
            }
        }
        self
    }

    /// Seed prior conversation turns (builder style) for **session
    /// continuation**: the `history` messages are inserted between the system
    /// prompt and the current user goal, so the model sees the earlier dialog
    /// when resuming an existing session. Pass plain user/assistant turns;
    /// the agent appends the live turn's messages on top as usual.
    pub fn with_history(mut self, history: Vec<Message>) -> Self {
        if !history.is_empty() {
            // messages == [system, goal]; reinsert as [system, history.., goal].
            let goal = self.messages.pop().expect("seeded with system + goal");
            self.messages.extend(history);
            self.messages.push(goal);
            self.response_history.replace_from_messages(&self.messages);
        }
        self
    }

    /// Seed provider-native Responses items for **session continuation**.
    ///
    /// This keeps resumed model input item-native (`function_call_output`,
    /// `custom_tool_call_output`, `web_search_call`, reasoning, etc.) instead
    /// of forcing the event log through the legacy chat `Message` projection.
    /// The current live user goal remains sourced from `self.messages` so
    /// per-run prompt decorations and history-snip tags stay visible.
    pub fn with_response_history(mut self, history: Vec<ResponseInputItem>) -> Self {
        if history.is_empty() {
            return self;
        }
        let system_messages: Vec<Message> = self
            .messages
            .iter()
            .filter(|message| message.role == Role::System)
            .cloned()
            .collect();
        let live_goal = self
            .messages
            .last()
            .filter(|message| message.role == Role::User)
            .cloned();
        let (instructions, _) = deepagent_models::response_items_from_messages(&system_messages);
        self.response_history = ResponseHistory {
            instructions,
            items: history,
        };
        if let Some(goal) = live_goal {
            self.response_history.push_message_projection(&goal);
        }
        self
    }

    /// The current conversation (for inspection / persistence).
    pub fn conversation(&self) -> &[Message] {
        &self.messages
    }

    /// Record a tool observation as a `tool` role message correlated to its
    /// originating tool-call id.
    ///
    /// The result is wrapped in an explicit envelope (`{"status": "ok" |
    /// "error", "result"|"error": ...}`) so the model can reliably tell success
    /// from failure. On failure a short recovery hint is appended, nudging the
    /// model to retry with corrected arguments or try a different tool rather
    /// than giving up — the behaviour that makes tool use feel "smooth".
    fn record_observation(&mut self, obs: &Observation) {
        // Correlate by the observation's own call id (set by the loop engine).
        // Fall back to the single most-recent pending id, then a synthetic one,
        // so older single-call paths keep working.
        let call_id = if let Some(call_id) = obs.call_id.clone() {
            if self.pending_tool_call_id.as_deref() == Some(call_id.as_str()) {
                self.pending_tool_call_id = None;
            }
            Some(call_id)
        } else {
            self.pending_tool_call_id.take()
        };
        // Track the outcome in the sliding window (kernel feedback without a
        // tool call id is not a real tool execution — skip it).
        if call_id.is_some() {
            self.recent_tool_results
                .push_back((obs.tool.clone(), obs.ok));
            if self.recent_tool_results.len() > FAILURE_WINDOW {
                self.recent_tool_results.pop_front();
            }
        }
        let same_tool_failures = self
            .recent_tool_results
            .iter()
            .filter(|(tool, ok)| !ok && tool.eq_ignore_ascii_case(&obs.tool))
            .count();
        let recovery_hint = if !obs.ok && same_tool_failures >= STRATEGY_SHIFT_THRESHOLD {
            // Wall-banging detected: the same tool keeps failing even after
            // several attempts (possibly with unrelated successes between
            // them). A stronger nudge than "retry with corrected arguments"
            // is required — Claude-grade behaviour is to STOP the failing
            // approach and pick a structurally different one.
            std::borrow::Cow::Owned(format!(
                "APPROACH CHANGE REQUIRED: `{}` has now failed {} times recently. STOP retrying \
                 variants of the same approach — it is not working in this environment. Step back \
                 and (1) restate what the errors actually say, (2) list at least two fundamentally \
                 different ways to reach the user's goal (a different tool, a built-in capability, \
                 a different dependency-free route), then (3) pick the one with the fewest external \
                 dependencies. Prefer the platform's built-in tools over installing packages. If \
                 guidance from a skill or earlier plan keeps failing here, abandon that guidance \
                 and choose another route. Do NOT run another minor variation of the failing call.",
                obs.tool, same_tool_failures
            ))
        } else if obs.tool.eq_ignore_ascii_case("bash") || obs.tool.eq_ignore_ascii_case("shell") {
            std::borrow::Cow::Borrowed(
                "This shell tool call FAILED. Do not repeat the same command or try random shell dialects. \
             Check the error and the current OS. On Windows, prefer shell:\"powershell\" with native \
             commands such as Remove-Item -LiteralPath \"<path>\" -Force (-Recurse for directories), \
             or use dedicated file tools when available. Only report inability after a corrected retry.",
            )
        } else {
            std::borrow::Cow::Borrowed(
                "This tool call FAILED. Do not give up. Read the error, then either retry with corrected \
             arguments or use a different tool/approach to achieve the same goal. Only report inability \
             after genuinely trying.",
            )
        };
        let envelope = if obs.ok {
            serde_json::json!({ "status": "ok", "result": obs.output })
        } else {
            serde_json::json!({
                "status": "error",
                "tool": obs.tool,
                "error": obs.output,
                "recovery_hint": recovery_hint,
            })
        };
        let content = serde_json::to_string(&envelope).unwrap_or_else(|_| obs.output.to_string());
        if let Some(call_id) = call_id {
            self.push_tool_observation(call_id, &obs.tool, content);
        } else {
            // Verification, Stop-hook and CompletionGate feedback is generated
            // by the kernel, not by a model tool_use block. Sending it as a
            // tool role would create an orphan tool result rejected by strict
            // providers. Keep it structured but feed it back as a user turn.
            self.push_message(Message::user(content));
        }
        // History snip (Claude Code HISTORY_SNIP, query.ts L400-410): a
        // successful snip tool call removes the referenced earlier segments
        // from the live conversation, before the next request is built.
        if obs.ok && self.snip_tool.as_deref() == Some(obs.tool.as_str()) {
            self.apply_snip_from_output(&obs.output);
        }
    }

    /// Estimate the token size of the next request's conversation. Combines a
    /// heuristic estimate over the live messages with the usage-grounded size
    /// of the last model call (Claude Code `tokenCountWithEstimation` reads
    /// usage off the protected-tail assistant), minus tokens snip freed that
    /// usage cannot see yet.
    fn estimate_context_tokens(&self) -> u64 {
        let counter = deepagent_context::HeuristicTokenizer::new();
        let heuristic: u64 = self
            .messages
            .iter()
            .map(|message| {
                deepagent_context::TokenCounter::count(&counter, &render_for_estimate(message))
                    as u64
            })
            .sum();
        let usage_based = self
            .last_call_context_tokens
            .saturating_sub(self.snip_tokens_freed_unreflected);
        heuristic.max(usage_based)
    }

    /// Proactive auto-compact (Claude Code `autoCompactIfNeeded`, fired before
    /// every model request): when the estimated context crosses the threshold,
    /// run the shared compactor. Failures trip a per-run circuit breaker after
    /// [`MAX_CONSECUTIVE_AUTOCOMPACT_FAILURES`]; the reactive overflow path
    /// remains as the fallback, so a tripped breaker never fails the run.
    async fn maybe_proactive_compact(&mut self, step: usize) {
        let Some(threshold) = self.proactive_compaction_threshold else {
            return;
        };
        if self.autocompact_consecutive_failures >= MAX_CONSECUTIVE_AUTOCOMPACT_FAILURES {
            return;
        }
        let Some(compactor) = self.reactive_compactor.clone() else {
            return;
        };
        let estimated = self.estimate_context_tokens();
        if estimated < threshold {
            return;
        }
        tracing::info!(
            step,
            estimated_tokens = estimated,
            threshold_tokens = threshold,
            "proactive auto-compact threshold crossed"
        );

        // Prefire fast path (Grok two-pass `try_two_pass_pass2_apply`): await any
        // in-flight pass-1, then apply a still-valid cached note against the
        // live tail — skipping the blocking summary call. Stale note (prefix
        // changed) or no cache → fall through to synchronous compaction.
        if self.prefire_handle.is_some() {
            self.collect_prefire_result().await;
        }
        if let Some(note) = self.prefire_cache.take() {
            match compactor.apply_prefire(&note, &self.messages).await {
                Ok(Some(compacted)) => {
                    tracing::info!(step, "prefire pass-2 applied cached note (prefire hit)");
                    self.adopt_compaction(compacted, "prefire_pass2");
                    return;
                }
                Ok(None) => tracing::info!(
                    step,
                    "prefire note stale/unusable; falling back to synchronous compaction"
                ),
                Err(error) => tracing::warn!(
                    step,
                    error = %error,
                    "prefire apply failed; falling back to synchronous compaction"
                ),
            }
        }

        match compactor
            .compact(&self.messages, CompactionTrigger::AutoCompactThreshold)
            .await
        {
            Ok(Some(compacted)) => self.adopt_compaction(compacted, "proactive_threshold"),
            Ok(None) => self.record_proactive_compaction_failure(step, "compactor returned none"),
            Err(error) => {
                self.record_proactive_compaction_failure(step, &error.to_string());
            }
        }
    }

    /// Replace the live conversation with a completed compaction, reset the
    /// dependent estimates/counters, drop any prefire cache, and emit the
    /// `ContextCompacted` event under `strategy`.
    fn adopt_compaction(&mut self, compacted: ReactiveCompaction, strategy: &str) {
        self.replace_messages(compacted.messages);
        self.autocompact_consecutive_failures = 0;
        self.snip_tokens_freed_unreflected = 0;
        self.snip_nudge_baseline_tokens = None;
        self.prefire_cache = None;
        // The usage-grounded estimate is stale after compaction; seed it with
        // the compactor's post-compact estimate instead.
        self.last_call_context_tokens = compacted.tokens_after;
        if let Some(sink) = &self.events {
            sink.emit(RuntimeEvent::ContextCompacted {
                tokens_before: compacted.tokens_before,
                tokens_after: compacted.tokens_after,
                strategy: strategy.to_string(),
                // Summary text stays in the model context and Hook payload;
                // runtime logs record only bounded metadata.
                summary: None,
            });
        }
    }

    /// Move a finished prefire pass-1 result into the cache. Awaits the handle
    /// (callers gate on `is_finished()` for the non-blocking path, or accept the
    /// blocking wait when compaction fires before pass-1 completes).
    async fn collect_prefire_result(&mut self) {
        let Some(handle) = self.prefire_handle.take() else {
            return;
        };
        match handle.await {
            Ok(Ok(Some(note))) => self.prefire_cache = Some(note),
            Ok(Ok(None)) => {}
            Ok(Err(error)) => tracing::warn!(error = %error, "prefire pass-1 produced no cache"),
            Err(join_error) => {
                tracing::warn!(error = %join_error, "prefire pass-1 task join failed")
            }
        }
    }

    /// Non-blocking: if the background pass-1 has finished, fold its result into
    /// the cache (the `await` returns immediately once `is_finished`).
    async fn collect_finished_prefire(&mut self) {
        if self
            .prefire_handle
            .as_ref()
            .is_some_and(|handle| handle.is_finished())
        {
            self.collect_prefire_result().await;
        }
    }

    /// Prefire scheduling (Grok `should_prefire_two_pass` + `spawn_local`): when
    /// the estimate sits in the lead band `[prefire_start, threshold)` and no
    /// note/handle exists yet, spawn a background pass-1 against a conversation
    /// snapshot so the summary is ready off the critical path before the real
    /// threshold. No-op when the compactor lacks prefire support (returns None).
    fn maybe_schedule_prefire(&mut self) {
        let (Some(start), Some(threshold)) = (
            self.prefire_start_threshold,
            self.proactive_compaction_threshold,
        ) else {
            return;
        };
        if self.prefire_cache.is_some() || self.prefire_handle.is_some() {
            return;
        }
        let Some(compactor) = self.reactive_compactor.clone() else {
            return;
        };
        let estimated = self.estimate_context_tokens();
        if estimated < start || estimated >= threshold {
            return;
        }
        tracing::info!(
            estimated_tokens = estimated,
            prefire_start = start,
            threshold_tokens = threshold,
            "scheduling prefire pass-1 (background)"
        );
        let snapshot = self.messages.clone();
        self.prefire_handle = Some(tokio::spawn(async move {
            compactor.prefire_pass1(&snapshot).await
        }));
    }

    /// The freshest genuine user query: the last `Role::User` message that is
    /// not an injected `<system-reminder>` (nudges, knowledge, tool-feedback).
    /// Used as the relevant-memory prefetch query (Claude Code extracts the
    /// last non-meta user message).
    fn latest_user_query(&self) -> Option<String> {
        self.messages
            .iter()
            .rev()
            .find(|m| {
                m.role == Role::User && !m.content.trim_start().starts_with("<system-reminder>")
            })
            .map(|m| m.content.clone())
            .filter(|c| !c.trim().is_empty())
    }

    /// Relevant-memory prefetch scheduling (§3.2, Claude Code
    /// `startRelevantMemoryPrefetch`): when a *new* user query appears and no
    /// prefetch is in flight, spawn a background retrieval excluding
    /// already-surfaced ids. Off the critical path — never blocks the turn.
    /// No-op without a provider, on a repeated query, on a single-word query
    /// (too little context, CC guard), or once the per-run cap is hit.
    fn maybe_schedule_memory_prefetch(&mut self) {
        let Some(provider) = self.memory_provider.clone() else {
            return;
        };
        if self.memory_prefetch_handle.is_some() {
            return;
        }
        if self.surfaced_memory_ids.len() >= MAX_SURFACED_MEMORIES_PER_RUN {
            return;
        }
        let Some(query) = self.latest_user_query() else {
            return;
        };
        // Single-word prompts lack enough context for meaningful retrieval.
        if !query.trim().contains(char::is_whitespace) {
            return;
        }
        // Only re-fire when the user query actually changed (a new turn).
        if self.last_prefetch_query.as_deref() == Some(query.as_str()) {
            return;
        }
        self.last_prefetch_query = Some(query.clone());
        let already: Vec<String> = self.surfaced_memory_ids.iter().cloned().collect();
        self.memory_prefetch_started_at = Some(std::time::Instant::now());
        self.memory_prefetch_handle = Some(tokio::spawn(async move {
            provider.fetch_relevant(&query, &already).await
        }));
    }

    /// Non-blocking: if the background relevant-memory prefetch has settled,
    /// inject the fresh entries as a single `<system-reminder>` and emit a
    /// [`RuntimeEvent::RelevantMemoriesInjected`]. Deduped against
    /// `surfaced_memory_ids` and bounded by [`MAX_SURFACED_MEMORIES_PER_RUN`].
    async fn collect_finished_memory_prefetch(&mut self) {
        if !self
            .memory_prefetch_handle
            .as_ref()
            .is_some_and(|handle| handle.is_finished())
        {
            return;
        }
        let handle = self.memory_prefetch_handle.take().unwrap();
        let started_at = self.memory_prefetch_started_at.take();
        let memories = match handle.await {
            Ok(Ok(memories)) => memories,
            Ok(Err(error)) => {
                tracing::warn!(error = %error, "relevant-memory prefetch failed");
                return;
            }
            Err(join_error) => {
                tracing::warn!(error = %join_error, "relevant-memory prefetch task join failed");
                return;
            }
        };
        // De-dup + cap: keep only entries not already surfaced this run.
        let mut fresh: Vec<RelevantMemory> = Vec::new();
        for memory in memories {
            if self.surfaced_memory_ids.len() + fresh.len() >= MAX_SURFACED_MEMORIES_PER_RUN {
                break;
            }
            if self.surfaced_memory_ids.contains(&memory.id)
                || fresh.iter().any(|m| m.id == memory.id)
            {
                continue;
            }
            fresh.push(memory);
        }
        if fresh.is_empty() {
            return;
        }
        for memory in &fresh {
            self.surfaced_memory_ids.insert(memory.id.clone());
        }
        let mut block = String::from(
            "<system-reminder>\n# 相关记忆 (relevant memories, retrieved in background)\n",
        );
        for memory in &fresh {
            block.push('\n');
            block.push_str(memory.block.trim());
            block.push('\n');
        }
        block.push_str("</system-reminder>");
        self.push_message(Message::user(block));

        let count = fresh.len();
        let latency_ms = started_at
            .map(|t| t.elapsed().as_millis() as u64)
            .unwrap_or(0);
        tracing::info!(
            count,
            latency_ms,
            "injected background-prefetched relevant memories"
        );
        if let Some(sink) = &self.events {
            sink.emit(RuntimeEvent::RelevantMemoriesInjected { count, latency_ms });
        }
    }

    fn record_proactive_compaction_failure(&mut self, step: usize, reason: &str) {
        self.autocompact_consecutive_failures += 1;
        let tripped = self.autocompact_consecutive_failures >= MAX_CONSECUTIVE_AUTOCOMPACT_FAILURES;
        tracing::warn!(
            step,
            consecutive_failures = self.autocompact_consecutive_failures,
            breaker_tripped = tripped,
            reason,
            "proactive auto-compact attempt failed"
        );
    }

    /// Context-efficiency snip nudge (Claude Code
    /// `attachments.ts::getContextEfficiencyAttachment`): after ~10k tokens of
    /// growth without a snip, remind the model — advisory wording only — that
    /// it may snip finished earlier segments. Paced by a baseline reset on
    /// every nudge, snip and compaction.
    fn maybe_inject_snip_nudge(&mut self) {
        if self.snip_tool.is_none() {
            return;
        }
        let estimated = self.estimate_context_tokens();
        let baseline = *self.snip_nudge_baseline_tokens.get_or_insert(estimated);
        if estimated.saturating_sub(baseline) < SNIP_NUDGE_INTERVAL_TOKENS {
            return;
        }
        self.snip_nudge_baseline_tokens = Some(estimated);
        // No upstream counterpart recoverable (snipCompact.ts::SNIP_NUDGE_TEXT
        // was not restored): wording self-written, kept advisory per the
        // "attachments are reference, not commands" baseline.
        self.push_message(Message::user(
            "<system-reminder>Context is growing. If earlier conversation segments are clearly \
             no longer needed for the remaining work, you may call the history-snip tool with \
             their [id:uN] tags to free context space. Only snip segments you are confident \
             are finished; when unsure, keep them.</system-reminder>"
                .to_string(),
        ));
    }

    /// Client-side doom-loop detection (Grok's doom-loop is a server signal via
    /// `x-grok-doom-loop-check`; DeepSeek has no such header, so this is the
    /// pure-client heuristic the upgrade doc specifies). When the last
    /// [`DOOM_LOOP_REPEAT_THRESHOLD`] tool-call signatures are identical (same
    /// tool + arguments, no new result), inject one advisory nudge to break the
    /// stall. Fires once per stall (tracked by `doom_loop_nudged_signature`);
    /// advisory only — it never aborts the run (误杀更糟 baseline). Orthogonal to
    /// the failure-escalation window (which counts FAILURES); doom-loop counts
    /// REPEATS regardless of success.
    fn maybe_inject_doom_loop_nudge(&mut self) {
        if self.recent_call_signatures.len() < DOOM_LOOP_REPEAT_THRESHOLD {
            return;
        }
        let Some(first) = self.recent_call_signatures.front().cloned() else {
            return;
        };
        let all_same = self.recent_call_signatures.iter().all(|sig| *sig == first);
        if !all_same {
            // The stall broke on its own; allow a future nudge.
            self.doom_loop_nudged_signature = None;
            return;
        }
        if self.doom_loop_nudged_signature.as_deref() == Some(first.as_str()) {
            return; // already nudged for this exact stall
        }
        self.doom_loop_nudged_signature = Some(first);
        tracing::warn!(
            repeats = self.recent_call_signatures.len(),
            "doom-loop: repeated identical tool call detected; injecting nudge"
        );
        self.push_message(Message::user(
            "<system-reminder>You have issued the same tool call with identical arguments \
             several times in a row with no new result — this is a stall (doom-loop). STOP \
             repeating it. Do NOT run the identical call again. Instead: (1) change the \
             arguments or the approach, (2) use a different tool or a built-in capability, \
             or (3) if you are genuinely blocked, say so explicitly and state exactly what \
             you need to proceed.</system-reminder>"
                .to_string(),
        ));
    }

    /// Periodic on-plan todo reminder (§3.1, Claude Code
    /// `getTodoReminderAttachments`). Increments the per-turn inactivity
    /// counters and, once both cross [`TODO_REMINDER_TURN_THRESHOLD`], injects a
    /// gentle `<system-reminder>` nudging the model to track/clean up its plan
    /// (with the current list appended when non-empty). Advisory only — it
    /// never forces `todo_write` (误杀更糟 baseline). No-op without a source.
    fn maybe_inject_todo_reminder(&mut self) {
        let Some(source) = self.todo_source.clone() else {
            return;
        };
        self.turns_since_todo_write = self.turns_since_todo_write.saturating_add(1);
        self.turns_since_todo_reminder = self.turns_since_todo_reminder.saturating_add(1);
        if self.turns_since_todo_write < TODO_REMINDER_TURN_THRESHOLD
            || self.turns_since_todo_reminder < TODO_REMINDER_TURN_THRESHOLD
        {
            return;
        }
        let snapshot = source.todo_snapshot();
        self.turns_since_todo_reminder = 0;
        let mut body = String::from(
            "<system-reminder>The todo-tracking tool hasn't been used in a while. If you are \
             working on a multi-step task that would benefit from tracking progress, consider \
             using the todo_write tool; also clean up the list if it has gone stale and no \
             longer matches what you are working on. Only if relevant to the current work — \
             this is a gentle reminder, ignore if not applicable. Never mention this reminder \
             to the user.",
        );
        if snapshot.has_items {
            body.push_str("\n\nCurrent todo list:\n");
            body.push_str(snapshot.rendered.trim());
        }
        body.push_str("</system-reminder>");
        self.push_message(Message::user(body));
        tracing::info!(
            turns_since_write = self.turns_since_todo_write,
            has_items = snapshot.has_items,
            "injected periodic todo reminder"
        );
    }

    /// Apply a successful snip tool result: remove each referenced user-tagged
    /// segment (that user turn up to the next tagged user turn), never touching
    /// the system prefix or the protected recent tail, and keeping tool-call
    /// pairing intact by pulling boundaries back over orphaned tool results.
    fn apply_snip_from_output(&mut self, output: &serde_json::Value) {
        let Some(ids) = output.get("ids").and_then(|value| value.as_array()) else {
            return;
        };
        let ids: Vec<String> = ids
            .iter()
            .filter_map(|value| value.as_str().map(str::to_string))
            .collect();
        if ids.is_empty() {
            return;
        }
        let counter = deepagent_context::HeuristicTokenizer::new();
        let tokens_before: u64 = self
            .messages
            .iter()
            .map(|m| {
                deepagent_context::TokenCounter::count(&counter, &render_for_estimate(m)) as u64
            })
            .sum();
        let protected_start = self
            .messages
            .len()
            .saturating_sub(SNIP_PROTECTED_RECENT_MESSAGES);
        let mut remove = vec![false; self.messages.len()];
        for id in &ids {
            let tag = format!("[id:{id}]");
            let Some(start) = self.messages.iter().position(|m| {
                m.role == deepagent_core::message::Role::User && m.content.contains(&tag)
            }) else {
                continue;
            };
            if start == 0 || start >= protected_start {
                continue;
            }
            // Segment runs to the next tagged user turn or the protected tail.
            let mut end = self.messages[start + 1..protected_start]
                .iter()
                .position(|m| {
                    m.role == deepagent_core::message::Role::User && m.content.contains("[id:u")
                })
                .map(|offset| start + 1 + offset)
                .unwrap_or(protected_start);
            // Pairing safety: the first retained message must not be a tool
            // result whose requesting assistant sits inside the removal zone.
            while end > start
                && self
                    .messages
                    .get(end)
                    .is_some_and(|m| m.role == deepagent_core::message::Role::Tool)
            {
                end -= 1;
            }
            if end <= start {
                continue;
            }
            for flag in remove.iter_mut().take(end).skip(start) {
                *flag = true;
            }
        }
        if !remove.iter().any(|flag| *flag) {
            return;
        }
        let mut kept = Vec::with_capacity(self.messages.len());
        for (index, message) in self.messages.drain(..).enumerate() {
            if !remove[index] {
                kept.push(message);
            }
        }
        self.replace_messages(kept);
        let tokens_after: u64 = self
            .messages
            .iter()
            .map(|m| {
                deepagent_context::TokenCounter::count(&counter, &render_for_estimate(m)) as u64
            })
            .sum();
        let freed = tokens_before.saturating_sub(tokens_after);
        self.snip_tokens_freed_unreflected += freed;
        self.snip_nudge_baseline_tokens = None;
        tracing::info!(
            snipped_ids = ?ids,
            tokens_freed = freed,
            "history snip applied to live conversation"
        );
        if let Some(sink) = &self.events {
            sink.emit(RuntimeEvent::ContextCompacted {
                tokens_before,
                tokens_after,
                strategy: "history_snip".to_string(),
                summary: None,
            });
        }
    }
}

/// Render one message for heuristic token estimation (content + reasoning +
/// tool calls), mirroring the compactor-side rendering.
fn render_for_estimate(message: &Message) -> String {
    let mut rendered = message.content.clone();
    if let Some(reasoning) = message.reasoning_content.as_deref() {
        rendered.push('\n');
        rendered.push_str(reasoning);
    }
    for call in &message.tool_calls {
        rendered.push_str(&format!("\n{} {}", call.name, call.arguments));
    }
    rendered
}

/// A stable signature of one turn's tool calls (sorted `name(arguments)`) for
/// the doom-loop detector: two turns with the same signature requested the
/// exact same work.
fn doom_loop_signature(invocations: &[ToolInvocation]) -> String {
    let mut parts: Vec<String> = invocations
        .iter()
        .map(|inv| format!("{}:{}", inv.name, inv.arguments))
        .collect();
    parts.sort();
    parts.join("|")
}

/// Forwards model streaming deltas to a [`RuntimeEventSink`] as
/// [`RuntimeEvent`]s, so a UI sees tokens/reasoning live.
struct SinkObserver<'a> {
    sink: Option<Arc<dyn RuntimeEventSink>>,
    tools: Option<&'a mut dyn ToolAttemptController>,
    step: usize,
    request_started_at: std::time::Instant,
    first_token_seen: bool,
    stream_activity_seen: bool,
}

impl DeltaObserver for SinkObserver<'_> {
    fn on_event(&mut self, event: ModelStreamEvent) {
        self.stream_activity_seen = true;
        match event {
            ModelStreamEvent::ResponseStreamEvent {
                event_type,
                item_id,
                item_type,
                delta_chars,
            } => {
                if let Some(sink) = &self.sink {
                    sink.emit(RuntimeEvent::ResponsesStreamEvent {
                        event_type,
                        item_id,
                        item_type,
                        delta_chars,
                    });
                }
            }
            ModelStreamEvent::ContentDelta { text } => {
                self.emit_first_token_once("content");
                if let Some(sink) = &self.sink {
                    sink.emit(RuntimeEvent::ContentDelta { text });
                }
            }
            ModelStreamEvent::ReasoningDelta { text } => {
                self.emit_first_token_once("reasoning");
                if let Some(sink) = &self.sink {
                    sink.emit(RuntimeEvent::ReasoningDelta { text });
                }
            }
            ModelStreamEvent::ToolCallCompleted {
                id,
                name,
                arguments,
                ..
            } => {
                if let Some(tools) = self.tools.as_deref_mut() {
                    tools.prepare(ToolInvocation::new(name, arguments).with_id(id));
                }
            }
            ModelStreamEvent::WebSearchCall { id, status, action } => {
                if let Some(sink) = &self.sink {
                    let action_type = action
                        .as_ref()
                        .and_then(|value| value.get("type"))
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string);
                    let queries_count = action
                        .as_ref()
                        .and_then(|value| value.get("queries"))
                        .and_then(serde_json::Value::as_array)
                        .map_or(0, Vec::len);
                    sink.emit(RuntimeEvent::ResponsesWebSearchCall {
                        call_id: id,
                        status,
                        action_type,
                        queries_count,
                    });
                }
            }
            ModelStreamEvent::ToolCallStarted { .. }
            | ModelStreamEvent::ToolArgumentsDelta { .. }
            | ModelStreamEvent::Usage { .. }
            | ModelStreamEvent::Finished { .. } => {}
        }
    }
    // Tool-call start is emitted by the loop engine (with args + call_id) at the
    // BeforeToolUse gate, so we don't duplicate it here.
}

impl SinkObserver<'_> {
    fn emit_first_token_once(&mut self, kind: &str) {
        if self.first_token_seen {
            return;
        }
        self.first_token_seen = true;
        if let Some(sink) = &self.sink {
            sink.emit(RuntimeEvent::ModelFirstToken {
                step: self.step,
                kind: kind.to_string(),
                elapsed_ms: self.request_started_at.elapsed().as_millis() as u64,
            });
        }
    }
}

async fn stream_model_attempt<'a>(
    client: &ModelClient,
    request: ResponseRequest,
    sink: Option<Arc<dyn RuntimeEventSink>>,
    step: usize,
    started_at: std::time::Instant,
    cancel: Option<Arc<AtomicBool>>,
    tools: Option<&'a mut dyn ToolAttemptController>,
) -> (
    Result<Response>,
    bool,
    Option<&'a mut dyn ToolAttemptController>,
) {
    let mut observer = SinkObserver {
        sink,
        tools,
        step,
        request_started_at: started_at,
        first_token_seen: false,
        stream_activity_seen: false,
    };
    let result = if let Some(cancel) = cancel {
        client
            .stream_response_observed_cancelled(request, &mut observer, cancel)
            .await
    } else {
        client
            .stream_response_observed(request, &mut observer)
            .await
    };
    let stream_activity_seen = observer.stream_activity_seen;
    let tools = observer.tools.take();
    (result, stream_activity_seen, tools)
}

#[async_trait]
impl Agent for ModelAgent {
    async fn think(&mut self, step: usize, last: &[Observation]) -> Result<AgentDecision> {
        self.think_inner(step, last, None, None).await
    }

    async fn think_cancelled(
        &mut self,
        step: usize,
        last: &[Observation],
        cancel: Option<Arc<AtomicBool>>,
    ) -> Result<AgentDecision> {
        self.think_inner(step, last, cancel, None).await
    }

    async fn think_streaming_cancelled(
        &mut self,
        step: usize,
        last: &[Observation],
        cancel: Option<Arc<AtomicBool>>,
        tools: Option<&mut dyn ToolAttemptController>,
    ) -> Result<AgentDecision> {
        self.think_inner(step, last, cancel, tools).await
    }

    fn cumulative_usage(&self) -> Option<crate::agent::RunUsage> {
        // None when nothing was reported (all zero), so callers can skip the
        // persisted usage event for providers that don't return usage.
        if self.usage == crate::agent::RunUsage::default() {
            None
        } else {
            Some(self.usage)
        }
    }

    fn take_pending_response_items(&mut self) -> Vec<ResponseOutputItem> {
        std::mem::take(&mut self.pending_response_items)
    }

    fn take_pending_raw_usage(&mut self) -> Vec<serde_json::Value> {
        std::mem::take(&mut self.pending_raw_usage)
    }
}

impl ModelAgent {
    async fn think_inner(
        &mut self,
        step: usize,
        last: &[Observation],
        cancel: Option<Arc<AtomicBool>>,
        mut tools: Option<&mut dyn ToolAttemptController>,
    ) -> Result<AgentDecision> {
        // Feed back every tool result from the previous step (more than one when
        // the previous turn ran tools in parallel). Order matters: tool results
        // must follow the assistant turn that requested them.
        for obs in last {
            self.record_observation(obs);
        }

        // Per-turn context management chain, aligned with Claude Code's fixed
        // order (query.ts L365-468): snips already applied via observations
        // above → proactive auto-compact threshold check → snip nudge. The
        // reactive overflow path below stays as the fallback. Prefire (Grok
        // two-pass) folds a finished background pass-1 into the cache first,
        // then schedules a new one once the estimate enters the lead band.
        self.collect_finished_prefire().await;
        self.collect_finished_memory_prefetch().await;
        self.maybe_proactive_compact(step).await;
        self.maybe_schedule_prefire();
        self.maybe_schedule_memory_prefetch();
        self.maybe_inject_snip_nudge();
        self.maybe_inject_doom_loop_nudge();
        self.maybe_inject_todo_reminder();

        let mut request = self
            .request_for_current_history()
            .with_thinking_depth(self.thinking_depth)
            .with_tools(self.tools.clone());
        let message_count = self.messages.len();
        let tool_count = request.tools.len();
        let request_started_at = std::time::Instant::now();
        if let Some(sink) = &self.events {
            sink.emit(RuntimeEvent::ModelRequestStarted {
                step,
                model: self.model.clone(),
                thinking_depth: self.thinking_depth.label().to_string(),
                message_count,
                tool_count,
            });
        }

        // Stream the turn, forwarding token/reasoning deltas to the event sink
        // (if any) as they arrive.
        let mut attempt = 0usize;
        let mut fallback_used = false;
        let mut max_output_escalated = false;
        let mut max_output_recoveries = 0usize;
        let mut rate_limited_failures = 0usize;
        let mut reactive_compaction_attempted = false;
        let response = loop {
            attempt += 1;
            if let Some(tools) = tools.as_deref_mut() {
                tools.begin(attempt);
            }
            let attempt_started_at = std::time::Instant::now();
            let current_tools = tools.take();
            let (result, stream_activity_seen, returned_tools) = stream_model_attempt(
                &self.client,
                request.clone(),
                self.events.clone(),
                step,
                attempt_started_at,
                cancel.clone(),
                current_tools,
            )
            .await;
            tools = returned_tools;
            match result {
                Ok(response) => {
                    if response.finish_reason == Some(FinishReason::Length)
                        && !max_output_escalated
                        && attempt < self.max_model_attempts
                    {
                        let current_max = request.max_output_tokens.unwrap_or(8_192);
                        let escalated_max = current_max.max(65_536);
                        if escalated_max > current_max {
                            if let Some(tools) = tools.as_deref_mut() {
                                tools.abort(attempt, "model output reached max_tokens");
                            }
                            emit_attempt_reset(
                                self.events.as_ref(),
                                step,
                                attempt,
                                format!(
                                    "model output reached max_tokens; retrying with max_tokens={escalated_max}"
                                ),
                            );
                            request.max_output_tokens = Some(escalated_max);
                            max_output_escalated = true;
                            continue;
                        }
                    }
                    // Multi-turn continuation (Claude Code query.ts
                    // max_output_tokens_recovery, ≤3): escalation is spent (or
                    // impossible) and the turn was still truncated without
                    // usable tool calls. Keep the partial output in the
                    // conversation and inject the resume prompt instead of
                    // failing the turn and discarding the partial work.
                    let response_projection = response.assistant_message_projection();
                    if response.finish_reason == Some(FinishReason::Length)
                        && response_projection.tool_calls.is_empty()
                        && max_output_recoveries < MAX_OUTPUT_TOKENS_RECOVERY_LIMIT
                    {
                        if let Some(tools) = tools.as_deref_mut() {
                            tools.abort(attempt, "max_tokens truncation; resuming in a new turn");
                        }
                        if let Some(usage) = response.usage {
                            self.usage.prompt_tokens += usage.prompt_tokens;
                            self.usage.completion_tokens += usage.completion_tokens;
                            self.usage.reasoning_tokens += usage.reasoning_tokens;
                            self.usage.total_tokens += usage.total_tokens;
                            self.usage.prompt_cache_hit_tokens += usage.prompt_cache_hit_tokens;
                            self.usage.prompt_cache_miss_tokens += usage.prompt_cache_miss_tokens;
                        }
                        if let Some(raw_usage) = response.raw_usage.clone() {
                            self.pending_raw_usage.push(raw_usage);
                        }
                        let mut partial =
                            Message::text(Role::Assistant, response_projection.content.clone());
                        partial.reasoning_content = response_projection.reasoning_content.clone();
                        self.response_history
                            .extend_output_items(&response.output_items);
                        self.messages.push(partial);
                        self.push_message(Message::user(MAX_OUTPUT_RECOVERY_PROMPT));
                        request = self
                            .request_for_current_history()
                            .with_thinking_depth(self.thinking_depth)
                            .with_tools(self.tools.clone());
                        max_output_recoveries += 1;
                        emit_attempt_reset(
                            self.events.as_ref(),
                            step,
                            attempt,
                            format!(
                                "max_tokens truncation; injected continue prompt (recovery {max_output_recoveries}/{MAX_OUTPUT_TOKENS_RECOVERY_LIMIT})"
                            ),
                        );
                        continue;
                    }
                    if let Some(tools) = tools.as_deref_mut() {
                        tools.commit(attempt);
                    }
                    break response;
                }
                Err(error) => {
                    if let Some(tools) = tools.as_deref_mut() {
                        tools.abort(attempt, &error.to_string());
                    }
                    let failure = classify_model_error(&error);
                    let cancelled = cancel
                        .as_ref()
                        .is_some_and(|flag| flag.load(std::sync::atomic::Ordering::Acquire));
                    if cancelled || failure == ModelFailureKind::Cancelled {
                        return Err(error);
                    }
                    if failure == ModelFailureKind::ContextOverflow
                        && !reactive_compaction_attempted
                    {
                        reactive_compaction_attempted = true;
                        if let Some(compactor) = self.reactive_compactor.as_ref() {
                            match compactor
                                .compact(&self.messages, CompactionTrigger::ContextOverflow)
                                .await
                            {
                                Ok(Some(compacted)) => {
                                    self.replace_messages(compacted.messages);
                                    request = self
                                        .request_for_current_history()
                                        .with_thinking_depth(self.thinking_depth)
                                        .with_tools(self.tools.clone());
                                    emit_attempt_reset(
                                        self.events.as_ref(),
                                        step,
                                        attempt,
                                        format!(
                                            "context overflow recovered by compaction ({} -> {} estimated tokens)",
                                            compacted.tokens_before, compacted.tokens_after
                                        ),
                                    );
                                    if let Some(sink) = &self.events {
                                        sink.emit(RuntimeEvent::ContextCompacted {
                                            tokens_before: compacted.tokens_before,
                                            tokens_after: compacted.tokens_after,
                                            strategy: "reactive_model".to_string(),
                                            // Summary text stays in the model context and Hook
                                            // payload; runtime logs record only bounded metadata.
                                            summary: None,
                                        });
                                    }
                                    continue;
                                }
                                Ok(None) => {}
                                Err(compaction_error) => {
                                    tracing::warn!(
                                        error = %compaction_error,
                                        "reactive context compaction failed"
                                    );
                                }
                            }
                        }
                    }
                    if failure.should_fallback() && !fallback_used {
                        if let Some(fallback) = self.fallback_model.clone() {
                            let primary = request.model.clone();
                            request.model = fallback.clone();
                            self.model = fallback.clone();
                            fallback_used = true;
                            emit_attempt_reset(
                                self.events.as_ref(),
                                step,
                                attempt,
                                format!(
                                    "primary model '{primary}' overloaded; switched to fallback '{fallback}'"
                                ),
                            );
                            continue;
                        }
                    }
                    if attempt >= self.max_model_attempts || !failure.retryable() {
                        return Err(error);
                    }
                    // Rate-limit low cap (Grok retry.rs RATE_LIMIT_RETRY_THRESHOLD):
                    // 429 waits can be long, so cap 429-driven attempts below
                    // the general budget instead of burning backoff.
                    if failure == ModelFailureKind::RateLimited {
                        rate_limited_failures += 1;
                        if rate_limited_failures
                            >= RATE_LIMIT_RETRY_THRESHOLD.min(self.max_model_attempts)
                        {
                            tracing::warn!(
                                step,
                                rate_limited_failures,
                                "rate-limit retry threshold reached; escalating to caller"
                            );
                            return Err(error);
                        }
                    }
                    if stream_activity_seen || self.events.is_some() {
                        emit_attempt_reset(self.events.as_ref(), step, attempt, error.to_string());
                    }
                    let delay = retry_delay(failure, attempt);
                    tracing::warn!(
                        step,
                        attempt,
                        max_attempts = self.max_model_attempts,
                        delay_ms = delay.as_millis() as u64,
                        error = %error,
                        failure = ?failure,
                        "retrying classified model stream failure"
                    );
                    tokio::time::sleep(delay).await;
                }
            }
        };
        if let Some(sink) = &self.events {
            sink.emit(RuntimeEvent::ModelRequestCompleted {
                step,
                elapsed_ms: request_started_at.elapsed().as_millis() as u64,
            });
        }

        // Forward token usage to the event sink so the UI can show input/output
        // and DeepSeek cache hit/miss totals for the run. Also accumulate it so
        // the loop can persist the run's total usage at completion.
        if let Some(usage) = response.usage {
            self.usage.prompt_tokens += usage.prompt_tokens;
            self.usage.completion_tokens += usage.completion_tokens;
            self.usage.reasoning_tokens += usage.reasoning_tokens;
            self.usage.total_tokens += usage.total_tokens;
            self.usage.prompt_cache_hit_tokens += usage.prompt_cache_hit_tokens;
            self.usage.prompt_cache_miss_tokens += usage.prompt_cache_miss_tokens;
            // Usage-grounded context size for the proactive auto-compact check
            // (this response already reflects any snips applied before it).
            self.last_call_context_tokens =
                usage.prompt_tokens as u64 + usage.completion_tokens as u64;
            self.snip_tokens_freed_unreflected = 0;
            if let Some(sink) = &self.events {
                sink.emit(RuntimeEvent::Usage {
                    prompt_tokens: usage.prompt_tokens,
                    completion_tokens: usage.completion_tokens,
                    reasoning_tokens: usage.reasoning_tokens,
                    total_tokens: usage.total_tokens,
                    prompt_cache_hit_tokens: usage.prompt_cache_hit_tokens,
                    prompt_cache_miss_tokens: usage.prompt_cache_miss_tokens,
                    cost_yuan: None,
                });
            }
        }
        if let Some(raw_usage) = response.raw_usage.clone() {
            self.pending_raw_usage.push(raw_usage);
        }
        self.pending_response_items = response.output_items.clone();

        // Persist the assistant turn in the agent's running conversation.
        // Thinking Mode reasoning is preserved for both tool-call and final
        // turns so the outer session log can replay it after refresh.
        let assistant = response.assistant_message_projection();
        self.push_provider_output_items(assistant, &response.output_items);

        // Decide the next action. The model may emit several tool calls in one
        // turn (parallel tool calling) — carry all of them, each tagged with its
        // own id so results correlate back correctly.
        let item_calls = response.tool_invocations_from_items();
        if !item_calls.is_empty() {
            let invocations: Vec<ToolInvocation> = item_calls
                .iter()
                .map(|(id, name, arguments)| {
                    ToolInvocation::new(name.clone(), arguments.clone()).with_id(id.clone())
                })
                .collect();
            // Record this turn's call signature for the client-side doom-loop
            // detector (repeated identical calls with no new result = stall).
            let signature = doom_loop_signature(&invocations);
            self.recent_call_signatures.push_back(signature);
            while self.recent_call_signatures.len() > DOOM_LOOP_REPEAT_THRESHOLD {
                self.recent_call_signatures.pop_front();
            }
            // Tamper-proof fact for the stall detector's `[runtime_state]` line.
            self.tool_calls_made += invocations.len();
            // Reset the todo-inactivity counter when the model tracks its plan
            // (§3.1 todo reminder pacing, Claude Code turns-since-TodoWrite).
            if invocations.iter().any(|inv| inv.name == "todo_write") {
                self.turns_since_todo_write = 0;
            }
            // Track the last id for observations from callers that cannot
            // attach a call_id yet. Normal tool feedback carries the explicit
            // Responses `call_id`.
            self.pending_tool_call_id = item_calls.last().map(|(id, _, _)| id.clone());
            if invocations.len() == 1 {
                return Ok(AgentDecision::CallTool(
                    invocations.into_iter().next().unwrap(),
                ));
            }
            return Ok(AgentDecision::CallTools(invocations));
        }

        match response.finish_reason {
            Some(FinishReason::ContentFilter) => {
                Err(CoreError::other("model stopped due to content filter"))
            }
            Some(FinishReason::Length) => Err(CoreError::other(
                "model output reached max_tokens before completing the turn",
            )),
            _ => {
                // §2.3 stall/laziness check (Grok laziness_classifier): a final
                // answer with an attached classifier gets one advisory audit.
                // A confident stalled verdict injects a nudge and re-enters the
                // model turn (bounded by MAX_STALL_NUDGES_PER_RUN); everything
                // else — including classifier failure — completes normally.
                if let Some(nudge) = self.maybe_stall_nudge(step).await {
                    self.push_message(Message::user(nudge));
                    return Box::pin(self.think_inner(step, &[], cancel, tools)).await;
                }
                Ok(AgentDecision::CompleteMessage(
                    response.assistant_message_projection(),
                ))
            }
        }
    }

    /// Run the stall classifier over the conversation tail (which already
    /// includes the just-pushed final answer). Returns the advisory nudge text
    /// when a confident stalled verdict is within the per-run budget; `None`
    /// on every other path (no classifier, fail-open error, not stalled, low
    /// confidence, cap exhausted).
    async fn maybe_stall_nudge(&mut self, step: usize) -> Option<String> {
        let classifier = self.stall_classifier.clone()?;
        // Budget pre-check: once spent, skip the classifier call entirely so a
        // post-nudge final answer completes with zero added latency.
        if self.stall_nudges_used >= MAX_STALL_NUDGES_PER_RUN {
            return None;
        }
        let transcript = render_stall_transcript_from_response_items(
            &self.response_history.items,
            self.tool_calls_made,
            Some(self.run_started_at.elapsed().as_secs()),
        );
        let verdict = classifier.classify(&transcript).await?;
        match evaluate_stall(&verdict, self.stall_nudges_used) {
            StallDecision::Nudge {
                category,
                confidence,
                evidence,
            } => {
                self.stall_nudges_used += 1;
                tracing::warn!(
                    step,
                    category = category.label(),
                    confidence,
                    evidence = evidence.as_str(),
                    "stall detector flagged the final answer; injecting advisory nudge"
                );
                if let Some(sink) = &self.events {
                    sink.emit(RuntimeEvent::StallNudgeInjected {
                        step,
                        category: category.label().to_string(),
                        confidence,
                        evidence: evidence.clone(),
                    });
                }
                let text = build_stall_nudge(category, &evidence);
                if text.is_empty() {
                    return None; // defensive: non-stalled never reaches here
                }
                Some(text)
            }
            StallDecision::NoNudge {
                category,
                confidence,
                reason,
            } => {
                tracing::info!(
                    step,
                    category = category.label(),
                    confidence,
                    reason = ?reason,
                    "stall detector verdict suppressed"
                );
                None
            }
        }
    }
}

fn emit_attempt_reset(
    events: Option<&Arc<dyn RuntimeEventSink>>,
    step: usize,
    attempt: usize,
    reason: String,
) {
    if let Some(sink) = events {
        sink.emit(RuntimeEvent::ModelAttemptReset {
            step,
            attempt,
            reason,
        });
    }
}

fn retry_delay(failure: ModelFailureKind, attempt: usize) -> std::time::Duration {
    let base_ms = match failure {
        ModelFailureKind::RateLimited | ModelFailureKind::Overloaded => 500,
        ModelFailureKind::Server => 300,
        _ => 200,
    };
    let capped = (base_ms * 2u64.saturating_pow(attempt.saturating_sub(1) as u32)).min(4_000);
    // ±20% jitter (Grok retry.rs retry_backoff_with_jitter) to de-sync
    // concurrent retries and avoid thundering-herd storms.
    let jitter_range = capped / 5;
    if jitter_range == 0 {
        return std::time::Duration::from_millis(capped);
    }
    use std::hash::{Hash, Hasher};
    use std::sync::atomic::{AtomicU64, Ordering};
    static JITTER_SEQ: AtomicU64 = AtomicU64::new(0);
    let mut hasher = std::hash::DefaultHasher::new();
    JITTER_SEQ.fetch_add(1, Ordering::Relaxed).hash(&mut hasher);
    std::thread::current().id().hash(&mut hasher);
    let jitter = hasher.finish() % (jitter_range * 2 + 1);
    std::time::Duration::from_millis(capped - jitter_range + jitter)
}

#[cfg(test)]
mod tests {
    use super::*;
    use deepagent_models::transport::{EventSink, HttpTransport, TransportRequest};
    use deepagent_models::{MockTransport, ModelConfig};
    use std::collections::VecDeque;
    use std::sync::Mutex;

    struct StaticReactiveCompactor {
        calls: Arc<std::sync::atomic::AtomicUsize>,
    }

    #[async_trait]
    impl ReactiveContextCompactor for StaticReactiveCompactor {
        async fn compact(
            &self,
            messages: &[Message],
            _trigger: CompactionTrigger,
        ) -> Result<Option<ReactiveCompaction>> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
            let latest = messages
                .last()
                .cloned()
                .unwrap_or_else(|| Message::user("continue"));
            Ok(Some(ReactiveCompaction {
                messages: vec![
                    Message::system("system"),
                    Message::user("[compacted summary] prior work"),
                    latest,
                ],
                tokens_before: 100,
                tokens_after: 20,
                summary: "prior work".into(),
            }))
        }
    }

    #[derive(Default)]
    struct RecordingToolAttempt {
        events: Vec<String>,
        calls: Vec<ToolInvocation>,
    }

    impl ToolAttemptController for RecordingToolAttempt {
        fn begin(&mut self, attempt: usize) {
            self.events.push(format!("begin:{attempt}"));
            self.calls.clear();
        }

        fn prepare(&mut self, invocation: ToolInvocation) {
            self.events.push(format!(
                "prepare:{}:{}",
                invocation.id.as_deref().unwrap_or("missing"),
                invocation.name
            ));
            self.calls.push(invocation);
        }

        fn commit(&mut self, attempt: usize) {
            self.events.push(format!("commit:{attempt}"));
        }

        fn abort(&mut self, attempt: usize, _reason: &str) {
            self.events.push(format!("abort:{attempt}"));
            self.calls.clear();
        }
    }

    fn client(events: Vec<String>) -> Arc<ModelClient> {
        let transport = Arc::new(MockTransport::new(events));
        Arc::new(ModelClient::new(transport, ModelConfig::deepseek("test")))
    }

    fn response_text_delta(text: &str) -> String {
        serde_json::json!({"type":"response.output_text.delta","delta":text}).to_string()
    }

    fn response_reasoning_delta(text: &str) -> String {
        serde_json::json!({"type":"response.reasoning_text.delta","delta":text}).to_string()
    }

    fn response_completed() -> String {
        serde_json::json!({"type":"response.completed","response":{"status":"completed"}})
            .to_string()
    }

    fn response_incomplete() -> String {
        serde_json::json!({"type":"response.incomplete","response":{"status":"incomplete"}})
            .to_string()
    }

    fn response_incomplete_with_usage(input: u32, output: u32, reasoning: u32) -> String {
        serde_json::json!({
            "type":"response.incomplete",
            "response":{
                "status":"incomplete",
                "usage":{
                    "input_tokens": input,
                    "input_tokens_details": {"cached_tokens": 1},
                    "output_tokens": output,
                    "output_tokens_details": {"reasoning_tokens": reasoning},
                    "total_tokens": input + output
                }
            }
        })
        .to_string()
    }

    fn response_text_completed(text: &str) -> Vec<String> {
        vec![response_text_delta(text), response_completed()]
    }

    fn response_text_incomplete(text: &str) -> Vec<String> {
        vec![response_text_delta(text), response_incomplete()]
    }

    fn response_text_incomplete_with_usage(
        text: &str,
        input: u32,
        output: u32,
        reasoning: u32,
    ) -> Vec<String> {
        vec![
            response_text_delta(text),
            response_incomplete_with_usage(input, output, reasoning),
        ]
    }

    fn response_function_call_done(call_id: &str, name: &str, arguments: &str) -> Vec<String> {
        let item_id = format!("item_{call_id}");
        let item = serde_json::json!({
            "type":"function_call",
            "id":item_id,
            "call_id":call_id,
            "name":name,
            "arguments":arguments,
        });
        vec![
            serde_json::json!({"type":"response.output_item.added","item":item}).to_string(),
            serde_json::json!({"type":"response.output_item.done","item":item}).to_string(),
            response_completed(),
        ]
    }

    fn response_web_search_then_text(id: &str, status: &str, text: &str) -> Vec<String> {
        let item = serde_json::json!({
            "type": "web_search_call",
            "id": id,
            "status": status,
            "action": {"type": "search", "query": "rust"}
        });
        vec![
            serde_json::json!({"type":"response.output_item.added","item":item}).to_string(),
            serde_json::json!({"type":"response.output_item.done","item":item}).to_string(),
            response_text_delta(text),
            response_completed(),
        ]
    }

    fn response_function_call_incomplete(
        call_id: &str,
        name: &str,
        arguments: &str,
    ) -> Vec<String> {
        let item_id = format!("item_{call_id}");
        let item = serde_json::json!({
            "type":"function_call",
            "id":item_id,
            "call_id":call_id,
            "name":name,
            "arguments":arguments,
        });
        vec![
            serde_json::json!({"type":"response.output_item.added","item":item}).to_string(),
            serde_json::json!({"type":"response.output_item.done","item":item}).to_string(),
            response_incomplete(),
        ]
    }

    struct AttemptTransport {
        attempts: Mutex<VecDeque<Vec<String>>>,
        requests: Mutex<Vec<serde_json::Value>>,
    }

    #[async_trait]
    impl HttpTransport for AttemptTransport {
        async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
            self.requests
                .lock()
                .unwrap()
                .push(serde_json::from_str(&request.body)?);
            let events = self
                .attempts
                .lock()
                .unwrap()
                .pop_front()
                .ok_or_else(|| CoreError::other("no attempt"))?;
            for event in events {
                if event == "__ERROR_EOF__" {
                    return Err(CoreError::other("unexpected EOF in provider stream"));
                }
                if event == "__ERROR_OVERLOADED__" {
                    return Err(CoreError::provider(
                        Some(529),
                        Some("overloaded_error".into()),
                        "provider overloaded",
                    ));
                }
                if event == "__ERROR_429__" {
                    return Err(CoreError::provider(
                        Some(429),
                        Some("rate_limit_exceeded".into()),
                        "too many requests",
                    ));
                }
                if event == "__ERROR_503__" {
                    return Err(CoreError::provider(
                        Some(503),
                        Some("service_unavailable".into()),
                        "upstream unavailable",
                    ));
                }
                if event == "__ERROR_CONTEXT__" {
                    return Err(CoreError::provider(
                        Some(413),
                        Some("context_length_exceeded".into()),
                        "maximum context window exceeded",
                    ));
                }
                if sink.on_event(&event)? {
                    break;
                }
            }
            Ok(())
        }
    }

    #[tokio::test]
    async fn completes_when_model_returns_text() {
        let events = response_text_completed("All done.");
        let mut agent =
            ModelAgent::new(client(events), "deepseek-v4-flash", "sys", "do it", vec![]);
        let decision = agent.think(0, &[]).await.unwrap();
        match decision {
            AgentDecision::CompleteMessage(message) => {
                assert_eq!(message.content, "All done.");
                assert!(message.reasoning_content.is_none());
            }
            other => panic!("expected CompleteMessage, got {other:?}"),
        }
        // System + user + assistant.
        assert_eq!(agent.conversation().len(), 3);
    }

    #[tokio::test]
    async fn initial_request_is_built_from_response_history_items() {
        let transport = Arc::new(AttemptTransport {
            attempts: Mutex::new(VecDeque::from([response_text_completed("done")])),
            requests: Mutex::new(Vec::new()),
        });
        let client = Arc::new(ModelClient::new(
            transport.clone(),
            ModelConfig::deepseek("test"),
        ));
        let mut agent = ModelAgent::new(client, "deepseek-v4-flash", "sys", "goal", vec![]);

        let decision = agent.think(0, &[]).await.unwrap();

        assert!(matches!(decision, AgentDecision::CompleteMessage(_)));
        let requests = transport.requests.lock().unwrap();
        let body = &requests[0];
        assert_eq!(body["instructions"], "sys");
        assert_eq!(body["input"][0]["type"], "message");
        assert_eq!(body["input"][0]["role"], "user");
        assert_eq!(body["input"][0]["content"], "goal");
        assert!(
            body.get("messages").is_none(),
            "provider request must not serialize Chat Completions messages"
        );
    }

    #[tokio::test]
    async fn provider_native_items_remain_in_followup_request_history() {
        let transport = Arc::new(AttemptTransport {
            attempts: Mutex::new(VecDeque::from([
                response_web_search_then_text("ws_1", "completed", "found it"),
                response_text_completed("done"),
            ])),
            requests: Mutex::new(Vec::new()),
        });
        let client = Arc::new(ModelClient::new(
            transport.clone(),
            ModelConfig::deepseek("test"),
        ));
        let mut agent = ModelAgent::new(client, "deepseek-v4-flash", "sys", "look up", vec![]);

        let obs = Observation {
            tool: "adversarial_verification".to_string(),
            ok: false,
            output: serde_json::json!({"retry": true}),
            call_id: None,
        };

        agent.think(0, &[]).await.unwrap();
        agent.think(1, &[obs]).await.unwrap();

        let requests = transport.requests.lock().unwrap();
        let input = requests[1]["input"].as_array().unwrap();
        assert!(input.iter().any(|item| {
            item["type"] == "web_search_call"
                && item["id"] == "ws_1"
                && item["status"] == "completed"
        }));
        assert!(input.iter().any(|item| {
            item["type"] == "message"
                && item["role"] == "assistant"
                && item["content"] == "found it"
        }));
    }

    // --- §2.3 stall/laziness detector integration ---------------------------

    struct ScriptedStallClassifier {
        verdict: crate::stall_detector::StallVerdict,
        calls: Arc<std::sync::atomic::AtomicUsize>,
    }

    #[async_trait]
    impl crate::stall_detector::StallClassifier for ScriptedStallClassifier {
        async fn classify(&self, _transcript: &str) -> Option<crate::stall_detector::StallVerdict> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
            Some(self.verdict.clone())
        }
    }

    fn stall_verdict(
        category: crate::stall_detector::StallCategory,
        confidence: f32,
    ) -> crate::stall_detector::StallVerdict {
        crate::stall_detector::StallVerdict {
            category,
            confidence,
            evidence: "final message claims tests ran but no bash call appears".to_string(),
        }
    }

    #[tokio::test]
    async fn stall_verdict_injects_advisory_nudge_and_reenters_turn() {
        use crate::stall_detector::StallCategory;
        // Turn 1: a confident-sounding false completion. Turn 2 (post-nudge):
        // the model actually finishes.
        let transport = Arc::new(AttemptTransport {
            attempts: Mutex::new(VecDeque::from([
                response_text_completed("All done, tests pass."),
                response_text_completed("Here is the real result."),
            ])),
            requests: Mutex::new(Vec::new()),
        });
        let client = Arc::new(ModelClient::new(
            transport.clone(),
            ModelConfig::deepseek("test"),
        ));
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let (sink, mut rx) = crate::events::ChannelSink::new();
        let mut agent = ModelAgent::new(client, "model", "system", "prompt", vec![])
            .with_events(Arc::new(sink))
            .with_stall_classifier(Arc::new(ScriptedStallClassifier {
                verdict: stall_verdict(StallCategory::StalledFalseCompletion, 0.9),
                calls: calls.clone(),
            }));

        let decision = agent.think(0, &[]).await.unwrap();
        // Re-entry produced the second answer, not the flagged first one.
        match decision {
            AgentDecision::CompleteMessage(message) => {
                assert_eq!(message.content, "Here is the real result.")
            }
            other => panic!("expected CompleteMessage, got {other:?}"),
        }
        // Classifier consulted once; both model turns consumed.
        assert_eq!(calls.load(std::sync::atomic::Ordering::Acquire), 1);
        assert!(transport.attempts.lock().unwrap().is_empty());
        // The advisory nudge landed in the conversation as a user message.
        assert!(agent
            .conversation()
            .iter()
            .any(|m| m.role == Role::User && m.content.contains("Stall detector flagged")));
        // A StallNudgeInjected event was emitted.
        let events: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
        assert!(events.iter().any(|e| matches!(
            e,
            RuntimeEvent::StallNudgeInjected {
                category,
                ..
            } if category == "stalled_false_completion"
        )));
    }

    #[tokio::test]
    async fn not_stalled_verdict_completes_without_nudge() {
        use crate::stall_detector::StallCategory;
        let transport = Arc::new(AttemptTransport {
            attempts: Mutex::new(VecDeque::from([response_text_completed("Genuinely done.")])),
            requests: Mutex::new(Vec::new()),
        });
        let client = Arc::new(ModelClient::new(
            transport.clone(),
            ModelConfig::deepseek("test"),
        ));
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut agent = ModelAgent::new(client, "model", "system", "prompt", vec![])
            .with_stall_classifier(Arc::new(ScriptedStallClassifier {
                verdict: stall_verdict(StallCategory::NotStalledComplete, 0.95),
                calls: calls.clone(),
            }));
        let decision = agent.think(0, &[]).await.unwrap();
        assert!(matches!(decision, AgentDecision::CompleteMessage(_)));
        // Classifier ran once; no re-entry, no nudge.
        assert_eq!(calls.load(std::sync::atomic::Ordering::Acquire), 1);
        assert_eq!(agent.conversation().len(), 3);
    }

    #[tokio::test]
    async fn stall_nudge_cap_prevents_infinite_reentry() {
        use crate::stall_detector::StallCategory;
        // Classifier ALWAYS says stalled. With MAX_STALL_NUDGES_PER_RUN=1 the
        // run must still terminate: one nudge, one re-entry, then the second
        // answer passes through (budget pre-check skips the classifier).
        let transport = Arc::new(AttemptTransport {
            attempts: Mutex::new(VecDeque::from([
                response_text_completed("done 1"),
                response_text_completed("done 2"),
            ])),
            requests: Mutex::new(Vec::new()),
        });
        let client = Arc::new(ModelClient::new(
            transport.clone(),
            ModelConfig::deepseek("test"),
        ));
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut agent = ModelAgent::new(client, "model", "system", "prompt", vec![])
            .with_stall_classifier(Arc::new(ScriptedStallClassifier {
                verdict: stall_verdict(StallCategory::StalledNarration, 0.99),
                calls: calls.clone(),
            }));
        let decision = agent.think(0, &[]).await.unwrap();
        match decision {
            AgentDecision::CompleteMessage(message) => assert_eq!(message.content, "done 2"),
            other => panic!("expected CompleteMessage, got {other:?}"),
        }
        // Classifier consulted exactly once (second final answer is capped out,
        // proving no runaway loop). Both attempts consumed, no more requested.
        assert_eq!(calls.load(std::sync::atomic::Ordering::Acquire), 1);
        assert!(transport.attempts.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn stall_classifier_failure_completes_normally() {
        // A classifier that always fails open (returns None) must never block
        // or alter the final answer.
        struct FailOpenClassifier;
        #[async_trait]
        impl crate::stall_detector::StallClassifier for FailOpenClassifier {
            async fn classify(
                &self,
                _transcript: &str,
            ) -> Option<crate::stall_detector::StallVerdict> {
                None
            }
        }
        let events = response_text_completed("All done.");
        let mut agent = ModelAgent::new(client(events), "model", "system", "prompt", vec![])
            .with_stall_classifier(Arc::new(FailOpenClassifier));
        let decision = agent.think(0, &[]).await.unwrap();
        match decision {
            AgentDecision::CompleteMessage(message) => assert_eq!(message.content, "All done."),
            other => panic!("expected CompleteMessage, got {other:?}"),
        }
        assert_eq!(agent.conversation().len(), 3);
    }

    #[tokio::test]
    async fn retries_empty_200_stream_without_recording_failed_turn() {
        let transport = Arc::new(AttemptTransport {
            attempts: Mutex::new(VecDeque::from([
                vec![response_text_delta("")],
                response_text_completed("recovered"),
            ])),
            requests: Mutex::new(Vec::new()),
        });
        let client = Arc::new(ModelClient::new(
            transport.clone(),
            ModelConfig::deepseek("test"),
        ));
        let mut agent =
            ModelAgent::new(client, "model", "system", "prompt", vec![]).with_max_model_attempts(2);

        let decision = agent.think(0, &[]).await.unwrap();
        assert!(matches!(decision, AgentDecision::CompleteMessage(_)));
        assert_eq!(agent.conversation().len(), 3);
        assert!(transport.attempts.lock().unwrap().len() <= 1);
    }

    #[tokio::test]
    async fn retry_resets_visible_deltas_from_failed_attempt() {
        let transport = Arc::new(AttemptTransport {
            attempts: Mutex::new(VecDeque::from([
                vec![response_text_delta("partial"), "__ERROR_EOF__".to_string()],
                response_text_completed("final"),
            ])),
            requests: Mutex::new(Vec::new()),
        });
        let client = Arc::new(ModelClient::new(transport, ModelConfig::deepseek("test")));
        let (sink, mut rx) = crate::events::ChannelSink::new();
        let mut agent = ModelAgent::new(client, "model", "system", "prompt", vec![])
            .with_events(Arc::new(sink))
            .with_max_model_attempts(2);

        let decision = agent.think(0, &[]).await.unwrap();
        assert!(matches!(decision, AgentDecision::CompleteMessage(_)));
        let events: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
        assert!(events.iter().any(|event| matches!(
            event,
            RuntimeEvent::ModelAttemptReset {
                step: 0,
                attempt: 1,
                ..
            }
        )));
    }

    #[tokio::test]
    async fn context_overflow_is_not_blindly_retried() {
        let transport = Arc::new(AttemptTransport {
            attempts: Mutex::new(VecDeque::from([
                vec!["__ERROR_CONTEXT__".to_string()],
                response_text_completed("must not run"),
            ])),
            requests: Mutex::new(Vec::new()),
        });
        let client = Arc::new(ModelClient::new(
            transport.clone(),
            ModelConfig::deepseek("test"),
        ));
        let mut agent = ModelAgent::new(client, "primary", "system", "prompt", vec![]);

        let error = agent.think(0, &[]).await.unwrap_err();

        assert_eq!(
            classify_model_error(&error),
            ModelFailureKind::ContextOverflow
        );
        assert_eq!(transport.requests.lock().unwrap().len(), 1);
        assert_eq!(transport.attempts.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn context_overflow_compacts_once_then_retries_clean_request() {
        let transport = Arc::new(AttemptTransport {
            attempts: Mutex::new(VecDeque::from([
                vec!["__ERROR_CONTEXT__".to_string()],
                response_text_completed("after compact"),
            ])),
            requests: Mutex::new(Vec::new()),
        });
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let client = Arc::new(ModelClient::new(
            transport.clone(),
            ModelConfig::deepseek("test"),
        ));
        let history = (0..12)
            .map(|index| {
                if index % 2 == 0 {
                    Message::user(format!("old user {index}"))
                } else {
                    Message::assistant(format!("old assistant {index}"))
                }
            })
            .collect();
        let mut agent = ModelAgent::new(client, "model", "system", "current", vec![])
            .with_history(history)
            .with_reactive_compactor(Arc::new(StaticReactiveCompactor {
                calls: calls.clone(),
            }));

        let decision = agent.think(0, &[]).await.unwrap();

        assert!(matches!(decision, AgentDecision::CompleteMessage(_)));
        assert_eq!(calls.load(std::sync::atomic::Ordering::Acquire), 1);
        let requests = transport.requests.lock().unwrap();
        assert!(requests[0]["input"].as_array().unwrap().len() > 3);
        assert_eq!(requests[1]["input"].as_array().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn repeated_context_overflow_trips_compaction_circuit_breaker() {
        let transport = Arc::new(AttemptTransport {
            attempts: Mutex::new(VecDeque::from([
                vec!["__ERROR_CONTEXT__".to_string()],
                vec!["__ERROR_CONTEXT__".to_string()],
                response_text_completed("must not run"),
            ])),
            requests: Mutex::new(Vec::new()),
        });
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let client = Arc::new(ModelClient::new(
            transport.clone(),
            ModelConfig::deepseek("test"),
        ));
        let mut agent = ModelAgent::new(client, "model", "system", "current", vec![])
            .with_reactive_compactor(Arc::new(StaticReactiveCompactor {
                calls: calls.clone(),
            }));

        let error = agent.think(0, &[]).await.unwrap_err();

        assert_eq!(
            classify_model_error(&error),
            ModelFailureKind::ContextOverflow
        );
        assert_eq!(calls.load(std::sync::atomic::Ordering::Acquire), 1);
        assert_eq!(transport.requests.lock().unwrap().len(), 2);
        assert_eq!(transport.attempts.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn proactive_compaction_fires_before_request_when_threshold_crossed() {
        // A tiny threshold guarantees the pre-request estimate crosses it on
        // step 0, so proactive compaction runs before the (only) model call.
        let transport = Arc::new(AttemptTransport {
            attempts: Mutex::new(VecDeque::from([response_text_completed("done")])),
            requests: Mutex::new(Vec::new()),
        });
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let client = Arc::new(ModelClient::new(
            transport.clone(),
            ModelConfig::deepseek("test"),
        ));
        let history = (0..12)
            .map(|index| {
                if index % 2 == 0 {
                    Message::user(format!("old user {index}"))
                } else {
                    Message::assistant(format!("old assistant {index}"))
                }
            })
            .collect();
        let mut agent = ModelAgent::new(client, "model", "system", "current", vec![])
            .with_history(history)
            .with_reactive_compactor(Arc::new(StaticReactiveCompactor {
                calls: calls.clone(),
            }))
            .with_proactive_compaction(1);

        let decision = agent.think(0, &[]).await.unwrap();

        assert!(matches!(decision, AgentDecision::CompleteMessage(_)));
        // Compaction ran once, before the single model request.
        assert_eq!(calls.load(std::sync::atomic::Ordering::Acquire), 1);
        let requests = transport.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        // The request used the compacted (3-message) window, not the full one.
        assert_eq!(requests[0]["input"].as_array().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn proactive_compaction_skipped_below_threshold() {
        let transport = Arc::new(AttemptTransport {
            attempts: Mutex::new(VecDeque::from([response_text_completed("done")])),
            requests: Mutex::new(Vec::new()),
        });
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let client = Arc::new(ModelClient::new(
            transport.clone(),
            ModelConfig::deepseek("test"),
        ));
        let mut agent = ModelAgent::new(client, "model", "system", "current", vec![])
            .with_reactive_compactor(Arc::new(StaticReactiveCompactor {
                calls: calls.clone(),
            }))
            // Huge threshold: a short conversation never crosses it.
            .with_proactive_compaction(10_000_000);

        let _ = agent.think(0, &[]).await.unwrap();

        assert_eq!(calls.load(std::sync::atomic::Ordering::Acquire), 0);
        assert_eq!(transport.requests.lock().unwrap().len(), 1);
    }

    #[test]
    fn snip_tool_tags_seeded_user_turns() {
        let history = vec![
            Message::user("first task"),
            Message::assistant("did first"),
            Message::user("second task"),
        ];
        let agent = ModelAgent::new(client(vec![]), "m", "system", "current goal", vec![])
            .with_history(history)
            .with_snip_tool("snip_history");
        let tagged: Vec<_> = agent
            .conversation()
            .iter()
            .filter(|m| m.content.contains("[id:u"))
            .collect();
        // Both seeded user turns plus the current goal user turn get tagged.
        assert_eq!(tagged.len(), 3);
    }

    #[tokio::test]
    async fn snip_from_output_removes_tagged_segment_and_frees_tokens() {
        // 12 messages so the protected tail (last 8) leaves an earlier zone.
        let history: Vec<Message> = (0..11)
            .map(|index| {
                if index % 2 == 0 {
                    Message::user(format!("user turn {index} with some payload"))
                } else {
                    Message::assistant(format!("assistant turn {index}"))
                }
            })
            .collect();
        let mut agent = ModelAgent::new(client(vec![]), "m", "system", "goal payload", vec![])
            .with_history(history)
            .with_snip_tool("snip_history");
        let before = agent.conversation().len();

        // Snip the first tagged user segment (u1).
        agent.apply_snip_from_output(&serde_json::json!({ "ids": ["u1"] }));

        let after = agent.conversation().len();
        assert!(after < before, "snip should remove at least one message");
        assert!(
            agent.snip_tokens_freed_unreflected > 0,
            "freed-token estimate should be recorded for the autocompact check"
        );
        // The system prefix and the current goal survive.
        assert_eq!(agent.conversation()[0].role, Role::System);
        assert!(agent
            .conversation()
            .iter()
            .any(|m| m.content.contains("goal payload")));
    }

    #[test]
    fn doom_loop_signature_is_order_stable_and_arg_sensitive() {
        let a = ToolInvocation::new("read_file", serde_json::json!({"path": "x"}));
        let b = ToolInvocation::new("grep", serde_json::json!({"q": "foo"}));
        // Same set, different order → same signature.
        assert_eq!(
            doom_loop_signature(&[a.clone(), b.clone()]),
            doom_loop_signature(&[b.clone(), a.clone()])
        );
        // Different arguments → different signature.
        let a2 = ToolInvocation::new("read_file", serde_json::json!({"path": "y"}));
        assert_ne!(doom_loop_signature(&[a]), doom_loop_signature(&[a2]));
    }

    #[test]
    fn doom_loop_nudge_fires_once_after_repeated_identical_calls() {
        let mut agent = ModelAgent::new(client(vec![]), "m", "system", "goal", vec![]);
        let sig = doom_loop_signature(&[ToolInvocation::new(
            "read_file",
            serde_json::json!({"path": "x"}),
        )]);
        for _ in 0..DOOM_LOOP_REPEAT_THRESHOLD {
            agent.recent_call_signatures.push_back(sig.clone());
        }
        let before = agent.conversation().len();
        agent.maybe_inject_doom_loop_nudge();
        assert_eq!(
            agent.conversation().len(),
            before + 1,
            "a stall should inject exactly one nudge"
        );
        assert!(agent
            .conversation()
            .last()
            .unwrap()
            .content
            .contains("doom-loop"));
        // Same stall again → no re-nudge.
        agent.maybe_inject_doom_loop_nudge();
        assert_eq!(agent.conversation().len(), before + 1);
    }

    #[test]
    fn doom_loop_nudge_skips_when_calls_differ() {
        let mut agent = ModelAgent::new(client(vec![]), "m", "system", "goal", vec![]);
        agent
            .recent_call_signatures
            .push_back("read_file:{}".to_string());
        agent
            .recent_call_signatures
            .push_back("grep:{}".to_string());
        agent
            .recent_call_signatures
            .push_back("read_file:{}".to_string());
        let before = agent.conversation().len();
        agent.maybe_inject_doom_loop_nudge();
        assert_eq!(
            agent.conversation().len(),
            before,
            "varied calls are not a stall"
        );
    }

    #[tokio::test]
    async fn rate_limited_429_retries_and_recovers_within_budget() {
        let transport = Arc::new(AttemptTransport {
            attempts: Mutex::new(VecDeque::from([
                vec!["__ERROR_429__".to_string()],
                response_text_completed("after backoff"),
            ])),
            requests: Mutex::new(Vec::new()),
        });
        let client = Arc::new(ModelClient::new(
            transport.clone(),
            ModelConfig::deepseek("test"),
        ));
        let mut agent =
            ModelAgent::new(client, "model", "system", "prompt", vec![]).with_max_model_attempts(3);

        let decision = agent.think(0, &[]).await.unwrap();

        assert!(matches!(decision, AgentDecision::CompleteMessage(_)));
        // Both attempts hit the SAME model (429 is retry, not fallback).
        let requests = transport.requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0]["model"], requests[1]["model"]);
    }

    #[tokio::test]
    async fn server_503_retries_then_fails_terminally_when_budget_exhausted() {
        let transport = Arc::new(AttemptTransport {
            attempts: Mutex::new(VecDeque::from([
                vec!["__ERROR_503__".to_string()],
                vec!["__ERROR_503__".to_string()],
                vec!["__ERROR_503__".to_string()],
            ])),
            requests: Mutex::new(Vec::new()),
        });
        let client = Arc::new(ModelClient::new(
            transport.clone(),
            ModelConfig::deepseek("test"),
        ));
        let mut agent =
            ModelAgent::new(client, "model", "system", "prompt", vec![]).with_max_model_attempts(3);

        let error = agent.think(0, &[]).await.unwrap_err();

        assert_eq!(classify_model_error(&error), ModelFailureKind::Server);
        // Exactly max_model_attempts requests were made, then a terminal error.
        assert_eq!(transport.requests.lock().unwrap().len(), 3);
    }

    #[tokio::test]
    async fn malformed_tool_call_arguments_do_not_panic_and_yield_a_decision() {
        // Model streams a tool call whose arguments are NOT valid JSON. The
        // agent must neither panic nor emit an unpaired tool_use: it may
        // surface the call (the pipeline's schema gate rejects it and pairs a
        // failure result) or degrade to a message — both are acceptable, a
        // crash is not.
        let events = response_function_call_done("bad", "read_file", "{not valid json");
        let mut agent = ModelAgent::new(client(events), "model", "system", "go", vec![]);

        let decision = agent.think(0, &[]).await.unwrap();

        match decision {
            AgentDecision::CallTool(invocation) => {
                assert_eq!(invocation.id.as_deref(), Some("bad"));
                // Raw arguments survive for the pipeline's schema gate to
                // reject deterministically (never executed).
            }
            AgentDecision::CallTools(invocations) => {
                assert_eq!(invocations.len(), 1);
            }
            AgentDecision::Complete(_) | AgentDecision::CompleteMessage(_) => {}
            other => panic!("unexpected decision for malformed arguments: {other:?}"),
        }
    }

    #[tokio::test]
    async fn overload_switches_once_to_configured_fallback_model() {
        let transport = Arc::new(AttemptTransport {
            attempts: Mutex::new(VecDeque::from([
                vec!["__ERROR_OVERLOADED__".to_string()],
                response_text_completed("fallback ok"),
            ])),
            requests: Mutex::new(Vec::new()),
        });
        let client = Arc::new(ModelClient::new(
            transport.clone(),
            ModelConfig::deepseek("test"),
        ));
        let mut agent = ModelAgent::new(client, "primary", "system", "prompt", vec![])
            .with_fallback_model(Some("fallback".into()));

        let decision = agent.think(0, &[]).await.unwrap();

        assert!(matches!(decision, AgentDecision::CompleteMessage(_)));
        let models = transport
            .requests
            .lock()
            .unwrap()
            .iter()
            .map(|request| request["model"].as_str().unwrap().to_string())
            .collect::<Vec<_>>();
        assert_eq!(models, vec!["primary", "fallback"]);
    }

    #[tokio::test]
    async fn max_output_retries_same_turn_with_larger_limit_and_aborts_old_tools() {
        let transport = Arc::new(AttemptTransport {
            attempts: Mutex::new(VecDeque::from([
                response_function_call_incomplete("stale", "read_file", r#"{"path":"a.txt"}"#),
                response_text_completed("recovered"),
            ])),
            requests: Mutex::new(Vec::new()),
        });
        let client = Arc::new(ModelClient::new(
            transport.clone(),
            ModelConfig::deepseek("test"),
        ));
        let mut agent = ModelAgent::new(client, "model", "system", "prompt", vec![]);
        let mut tool_attempt = RecordingToolAttempt::default();

        let decision = agent
            .think_streaming_cancelled(0, &[], None, Some(&mut tool_attempt))
            .await
            .unwrap();

        assert!(matches!(decision, AgentDecision::CompleteMessage(_)));
        let requests = transport.requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[1]["max_output_tokens"], 65_536);
        assert_eq!(
            tool_attempt.events,
            vec![
                "begin:1",
                "prepare:stale:read_file",
                "abort:1",
                "begin:2",
                "commit:2",
            ]
        );
        assert_eq!(agent.conversation().len(), 3);
        assert_eq!(agent.conversation()[2].content, "recovered");
    }

    #[tokio::test]
    async fn max_tokens_truncation_injects_continue_prompt_and_resumes() {
        // Attempt 1: escalation (8k default -> 64k) after a truncated turn.
        // Attempt 2: still truncated at 64k with partial prose (no tools) ->
        // multi-turn continue: partial output + resume prompt are appended.
        // Attempt 3: completes normally.
        let transport = Arc::new(AttemptTransport {
            attempts: Mutex::new(VecDeque::from([
                response_text_incomplete("part one"),
                response_text_incomplete_with_usage(" part two", 11, 7, 3),
                response_text_completed(" done"),
            ])),
            requests: Mutex::new(Vec::new()),
        });
        let client = Arc::new(ModelClient::new(
            transport.clone(),
            ModelConfig::deepseek("test"),
        ));
        // Budget must exceed the number of stream attempts (3).
        let mut agent =
            ModelAgent::new(client, "model", "system", "prompt", vec![]).with_max_model_attempts(5);

        let decision = agent.think(0, &[]).await.unwrap();

        assert!(matches!(decision, AgentDecision::CompleteMessage(_)));
        let requests = transport.requests.lock().unwrap();
        assert_eq!(requests.len(), 3, "escalate, then continue, then finish");
        // Second attempt escalated max_tokens; third carries the resume prompt.
        assert_eq!(requests[1]["max_output_tokens"], 65_536);
        let third = requests[2]["input"].as_array().unwrap();
        let last = third.last().unwrap();
        assert_eq!(last["role"], "user");
        assert!(last["content"]
            .as_str()
            .unwrap()
            .contains("Output token limit hit"));
        assert!(third.iter().any(|item| {
            item["type"] == "message"
                && item["role"] == "assistant"
                && item["content"]
                    .as_str()
                    .is_some_and(|text| text == " part two")
        }));
        // Partial output is preserved (not discarded) in the conversation.
        assert!(agent
            .conversation()
            .iter()
            .any(|m| m.content.contains("part two")));
        let raw_usage = agent.take_pending_raw_usage();
        assert_eq!(raw_usage.len(), 1);
        assert_eq!(raw_usage[0]["input_tokens"], 11);
        assert_eq!(raw_usage[0]["output_tokens_details"]["reasoning_tokens"], 3);
    }

    #[tokio::test]
    async fn max_tokens_recovery_gives_up_after_limit_and_surfaces_error() {
        // Escalation once, then MAX_OUTPUT_TOKENS_RECOVERY_LIMIT (3) continues,
        // all still truncated -> terminal max_tokens error (no infinite loop).
        let truncated = || response_text_incomplete("x");
        let transport = Arc::new(AttemptTransport {
            attempts: Mutex::new(VecDeque::from([
                truncated(),
                truncated(),
                truncated(),
                truncated(),
                truncated(),
            ])),
            requests: Mutex::new(Vec::new()),
        });
        let client = Arc::new(ModelClient::new(
            transport.clone(),
            ModelConfig::deepseek("test"),
        ));
        let mut agent = ModelAgent::new(client, "model", "system", "prompt", vec![])
            .with_max_model_attempts(20);

        let error = agent.think(0, &[]).await.unwrap_err();
        assert!(error.to_string().contains("max_tokens"));
        // 1 escalation attempt + 3 continuation attempts + 1 final (recovery
        // budget exhausted) = 5 total requests, then a terminal error.
        assert_eq!(transport.requests.lock().unwrap().len(), 5);
    }

    #[tokio::test]
    async fn rate_limit_429_caps_below_general_budget() {
        // Budget is 5, but 429 is capped at RATE_LIMIT_RETRY_THRESHOLD (2):
        // two 429s escalate to the caller rather than burning the full budget.
        let transport = Arc::new(AttemptTransport {
            attempts: Mutex::new(VecDeque::from([
                vec!["__ERROR_429__".to_string()],
                vec!["__ERROR_429__".to_string()],
                response_text_completed("unreached"),
            ])),
            requests: Mutex::new(Vec::new()),
        });
        let client = Arc::new(ModelClient::new(
            transport.clone(),
            ModelConfig::deepseek("test"),
        ));
        let mut agent =
            ModelAgent::new(client, "model", "system", "prompt", vec![]).with_max_model_attempts(5);

        let error = agent.think(0, &[]).await.unwrap_err();
        assert_eq!(classify_model_error(&error), ModelFailureKind::RateLimited);
        // Capped at 2 attempts despite a budget of 5 (never reached attempt 3).
        assert_eq!(transport.requests.lock().unwrap().len(), 2);
    }

    #[test]
    fn retry_delay_stays_within_jittered_bounds() {
        // ±20% jitter around the capped exponential base; never exceeds cap.
        for attempt in 1..=6 {
            let d = retry_delay(ModelFailureKind::Server, attempt).as_millis() as u64;
            let capped = (300u64 * 2u64.saturating_pow(attempt as u32 - 1)).min(4_000);
            let low = capped - capped / 5;
            let high = capped + capped / 5;
            assert!(
                d >= low && d <= high,
                "attempt {attempt}: {d} not in [{low},{high}]"
            );
        }
    }

    #[tokio::test]
    async fn final_answer_preserves_reasoning_for_replay() {
        let events = vec![
            response_reasoning_delta("I should inspect the image. "),
            response_text_delta("It shows a compile error."),
            response_completed(),
        ];
        let mut agent = ModelAgent::new(
            client(events),
            "deepseek-v4-flash",
            "sys",
            "describe",
            vec![],
        );
        let decision = agent.think(0, &[]).await.unwrap();
        match decision {
            AgentDecision::CompleteMessage(message) => {
                assert_eq!(message.content, "It shows a compile error.");
                assert_eq!(
                    message.reasoning_content.as_deref(),
                    Some("I should inspect the image. ")
                );
            }
            other => panic!("expected CompleteMessage, got {other:?}"),
        }

        let assistant = agent
            .conversation()
            .iter()
            .find(|m| m.role == Role::Assistant)
            .unwrap();
        assert_eq!(
            assistant.reasoning_content.as_deref(),
            Some("I should inspect the image. ")
        );
    }

    #[tokio::test]
    async fn requests_tool_then_persists_reasoning() {
        let mut events = vec![response_reasoning_delta("I should add.")];
        events.extend(response_function_call_done("c1", "add", r#"{"a":1,"b":2}"#));
        let mut agent = ModelAgent::new(client(events), "deepseek-v4-pro", "sys", "add", vec![]);
        let decision = agent.think(0, &[]).await.unwrap();
        match decision {
            AgentDecision::CallTool(inv) => {
                assert_eq!(inv.name, "add");
                assert_eq!(inv.arguments, serde_json::json!({"a":1,"b":2}));
                assert_eq!(inv.id.as_deref(), Some("c1"));
            }
            other => panic!("expected CallTool, got {other:?}"),
        }
        // The assistant turn retained reasoning_content (tool-call turn).
        let assistant = agent
            .conversation()
            .iter()
            .find(|m| m.role == Role::Assistant)
            .unwrap();
        assert_eq!(
            assistant.reasoning_content.as_deref(),
            Some("I should add.")
        );
        assert_eq!(agent.pending_tool_call_id.as_deref(), Some("c1"));
    }

    #[tokio::test]
    async fn prepares_complete_tool_call_before_committing_stream_attempt() {
        let events = response_function_call_done("c1", "add", r#"{"a":1,"b":2}"#);
        let mut agent = ModelAgent::new(client(events), "model", "system", "add", vec![]);
        let mut attempt = RecordingToolAttempt::default();

        let decision = agent
            .think_streaming_cancelled(0, &[], None, Some(&mut attempt))
            .await
            .unwrap();

        assert!(matches!(decision, AgentDecision::CallTool(_)));
        assert_eq!(
            attempt.events,
            vec!["begin:1", "prepare:c1:add", "commit:1"]
        );
        assert_eq!(attempt.calls.len(), 1);
    }

    #[tokio::test]
    async fn aborts_failed_tool_attempt_and_commits_only_retry_calls() {
        let transport = Arc::new(AttemptTransport {
            attempts: Mutex::new(VecDeque::from([
                {
                    let mut events =
                        response_function_call_done("stale", "add", r#"{"a":1,"b":2}"#);
                    events.pop();
                    events.push("__ERROR_EOF__".to_string());
                    events
                },
                response_function_call_done("fresh", "add", r#"{"a":2,"b":3}"#),
            ])),
            requests: Mutex::new(Vec::new()),
        });
        let client = Arc::new(ModelClient::new(transport, ModelConfig::deepseek("test")));
        let mut agent =
            ModelAgent::new(client, "model", "system", "add", vec![]).with_max_model_attempts(2);
        let mut attempt = RecordingToolAttempt::default();

        let decision = agent
            .think_streaming_cancelled(0, &[], None, Some(&mut attempt))
            .await
            .unwrap();

        match decision {
            AgentDecision::CallTool(invocation) => {
                assert_eq!(invocation.id.as_deref(), Some("fresh"));
            }
            other => panic!("expected retry tool call, got {other:?}"),
        }
        assert_eq!(
            attempt.events,
            vec![
                "begin:1",
                "prepare:stale:add",
                "abort:1",
                "begin:2",
                "prepare:fresh:add",
                "commit:2",
            ]
        );
        assert_eq!(attempt.calls.len(), 1);
        assert_eq!(attempt.calls[0].id.as_deref(), Some("fresh"));
    }

    #[tokio::test]
    async fn feeds_observation_back_as_response_item_and_projection() {
        let events = response_text_completed("sum is 3");
        let mut agent = ModelAgent::new(client(events), "deepseek-v4-flash", "sys", "add", vec![]);
        let obs = Observation {
            tool: "add".to_string(),
            ok: true,
            output: serde_json::json!({"sum": 3}),
            call_id: Some("c1".to_string()),
        };
        agent.think(1, std::slice::from_ref(&obs)).await.unwrap();
        assert!(agent.response_history.items.iter().any(|item| {
            matches!(
                item,
                ResponseInputItem::FunctionCallOutput { call_id, output }
                    if call_id == "c1" && output.contains("\"sum\":3")
            )
        }));
        // A tool-role compatibility projection correlated to c1 is still kept
        // for UI, compaction, and stall transcripts.
        let tool_msg = agent
            .conversation()
            .iter()
            .find(|m| m.role == Role::Tool)
            .unwrap();
        assert_eq!(tool_msg.tool_call_id.as_deref(), Some("c1"));
        // Success is wrapped in an ok envelope carrying the result.
        let v: serde_json::Value = serde_json::from_str(&tool_msg.content).unwrap();
        assert_eq!(v["status"], "ok");
        assert_eq!(v["result"]["sum"], 3);
    }

    #[tokio::test]
    async fn observation_feedback_enters_request_as_response_item() {
        let transport = Arc::new(AttemptTransport {
            attempts: Mutex::new(VecDeque::from([response_text_completed("sum is 3")])),
            requests: Mutex::new(Vec::new()),
        });
        let client = Arc::new(ModelClient::new(
            transport.clone(),
            ModelConfig::deepseek("test"),
        ));
        let mut agent = ModelAgent::new(client, "deepseek-v4-flash", "sys", "add", vec![]);
        let obs = Observation {
            tool: "add".to_string(),
            ok: true,
            output: serde_json::json!({"sum": 3}),
            call_id: Some("c1".to_string()),
        };

        agent.think(1, &[obs]).await.unwrap();

        let requests = transport.requests.lock().unwrap();
        let input = requests[0]["input"].as_array().unwrap();
        assert!(input.iter().any(|item| {
            item["type"] == "function_call_output"
                && item["call_id"] == "c1"
                && item["output"]
                    .as_str()
                    .is_some_and(|text| text.contains("\"sum\":3"))
        }));
    }

    #[tokio::test]
    async fn apply_patch_observation_feedback_enters_request_as_custom_tool_output() {
        let transport = Arc::new(AttemptTransport {
            attempts: Mutex::new(VecDeque::from([response_text_completed("patched")])),
            requests: Mutex::new(Vec::new()),
        });
        let client = Arc::new(ModelClient::new(
            transport.clone(),
            ModelConfig::deepseek("test"),
        ));
        let mut agent = ModelAgent::new(client, "deepseek-v4-flash", "sys", "patch", vec![]);
        let obs = Observation {
            tool: "apply_patch".to_string(),
            ok: true,
            output: serde_json::json!({"status": "ok"}),
            call_id: Some("call-patch".to_string()),
        };

        agent.think(1, &[obs]).await.unwrap();

        let requests = transport.requests.lock().unwrap();
        let input = requests[0]["input"].as_array().unwrap();
        assert!(input.iter().any(|item| {
            item["type"] == "custom_tool_call_output"
                && item["call_id"] == "call-patch"
                && item["output"]
                    .as_str()
                    .is_some_and(|text| text.contains("\"status\":\"ok\""))
        }));
        assert!(
            !input.iter().any(|item| {
                item["type"] == "function_call_output" && item["call_id"] == "call-patch"
            }),
            "apply_patch must stay on Responses custom tool output path"
        );
    }

    #[tokio::test]
    async fn failed_observation_is_fed_back_with_recovery_hint() {
        let events = response_text_completed("ok let me retry");
        let mut agent =
            ModelAgent::new(client(events), "deepseek-v4-flash", "sys", "search", vec![]);
        let obs = Observation {
            tool: "web_search".to_string(),
            ok: false,
            output: serde_json::json!({"error": "search failed: timeout"}),
            call_id: Some("c9".to_string()),
        };
        agent.think(1, std::slice::from_ref(&obs)).await.unwrap();
        let tool_msg = agent
            .conversation()
            .iter()
            .find(|m| m.role == Role::Tool)
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&tool_msg.content).unwrap();
        // Failure is explicitly marked and carries a recovery hint.
        assert_eq!(v["status"], "error");
        assert_eq!(v["tool"], "web_search");
        assert!(v["error"]["error"].as_str().unwrap().contains("timeout"));
        assert!(v["recovery_hint"].as_str().unwrap().contains("retry"));
    }

    #[tokio::test]
    async fn repeated_same_tool_failures_escalate_to_strategy_change_hint() {
        // Wall-banging regression (manual acceptance 2026-07-28): 12+ bash
        // failures with interleaved successes never escalated beyond "retry
        // with corrected arguments", so the model kept trying variants of a
        // broken approach. After STRATEGY_SHIFT_THRESHOLD same-tool failures
        // in the sliding window the hint must demand a different approach.
        let mut agent = ModelAgent::new(client(vec![]), "m", "sys", "goal", vec![]);
        let fail = |id: &str| Observation {
            tool: "bash".to_string(),
            ok: false,
            output: serde_json::json!({"exit_code": 1, "stdout": "", "stderr": ""}),
            call_id: Some(id.to_string()),
        };
        let ok = |id: &str| Observation {
            tool: "bash".to_string(),
            ok: true,
            output: serde_json::json!({"exit_code": 0}),
            call_id: Some(id.to_string()),
        };
        // Interleaved successes must NOT reset the failure count.
        agent.record_observation(&fail("c1"));
        agent.record_observation(&ok("c2"));
        agent.record_observation(&fail("c3"));
        agent.record_observation(&fail("c4"));
        let hints: Vec<String> = agent
            .conversation()
            .iter()
            .filter(|m| m.role == Role::Tool)
            .filter_map(|m| serde_json::from_str::<serde_json::Value>(&m.content).ok())
            .filter_map(|v| v["recovery_hint"].as_str().map(str::to_string))
            .collect();
        // First three failures: standard shell hint, no escalation yet.
        assert!(hints[..3].iter().all(|h| !h.contains("APPROACH CHANGE")));
        // Fourth same-tool failure crosses the threshold and escalates.
        agent.record_observation(&fail("c5"));
        let last = agent
            .conversation()
            .iter()
            .rev()
            .find(|m| m.role == Role::Tool)
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&last.content).unwrap();
        let hint = v["recovery_hint"].as_str().unwrap();
        assert!(hint.contains("APPROACH CHANGE REQUIRED"), "hint: {hint}");
        assert!(hint.contains("built-in tools"), "hint: {hint}");
        // A different tool failing once keeps the standard hint.
        agent.record_observation(&Observation {
            tool: "web_search".to_string(),
            ok: false,
            output: serde_json::json!({"error": "timeout"}),
            call_id: Some("c6".to_string()),
        });
        let last = agent
            .conversation()
            .iter()
            .rev()
            .find(|m| m.role == Role::Tool)
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&last.content).unwrap();
        assert!(!v["recovery_hint"]
            .as_str()
            .unwrap()
            .contains("APPROACH CHANGE"));
    }

    #[tokio::test]
    async fn kernel_feedback_without_call_id_is_not_an_orphan_tool_result() {
        let events = response_text_completed("retry complete");
        let mut agent = ModelAgent::new(client(events), "model", "system", "goal", vec![]);
        let feedback = Observation {
            tool: "completion_gate".to_string(),
            ok: false,
            output: serde_json::json!({"reason": "missing deletion evidence"}),
            call_id: None,
        };

        agent.think(1, &[feedback]).await.unwrap();
        assert!(!agent
            .conversation()
            .iter()
            .any(|message| message.role == Role::Tool));
        assert_eq!(
            agent.conversation()[2].role,
            Role::User,
            "kernel feedback must be a paired-safe user message"
        );
    }

    struct ScriptedMemoryProvider {
        mems: Vec<RelevantMemory>,
        calls: Arc<std::sync::atomic::AtomicUsize>,
    }

    #[async_trait]
    impl RelevantMemoryProvider for ScriptedMemoryProvider {
        async fn fetch_relevant(
            &self,
            _query: &str,
            already_surfaced: &[String],
        ) -> Result<Vec<RelevantMemory>> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
            // Honor the dedup contract: never return an already-surfaced id.
            Ok(self
                .mems
                .iter()
                .filter(|m| !already_surfaced.contains(&m.id))
                .cloned()
                .collect())
        }
    }

    #[tokio::test]
    async fn relevant_memory_prefetch_injects_on_next_turn() {
        // Turn 0 emits a tool call (run continues); turn 1 completes. The
        // prefetch scheduled in turn 0 must be injected before turn 1's request.
        let transport = Arc::new(AttemptTransport {
            attempts: Mutex::new(VecDeque::from([
                response_function_call_done("c1", "read_file", r#"{"path":"a.txt"}"#),
                response_text_completed("done"),
            ])),
            requests: Mutex::new(Vec::new()),
        });
        let client = Arc::new(ModelClient::new(
            transport.clone(),
            ModelConfig::deepseek("test"),
        ));
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let provider = Arc::new(ScriptedMemoryProvider {
            mems: vec![RelevantMemory {
                id: "project:build-fix".to_string(),
                block: "[source: Fixing the build (project) · pitfall]\nrun cargo clean first"
                    .to_string(),
            }],
            calls: calls.clone(),
        });
        let mut agent = ModelAgent::new(
            client,
            "model",
            "system",
            "investigate the failing build error and fix it",
            vec![],
        )
        .with_relevant_memory_provider(provider);

        // Turn 0: schedules the background prefetch, returns the tool call.
        let d0 = agent.think(0, &[]).await.unwrap();
        assert!(matches!(
            d0,
            AgentDecision::CallTool(_) | AgentDecision::CallTools(_)
        ));
        // Let the spawned prefetch settle before the next turn polls it.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Turn 1: collects the settled prefetch and injects it as a reminder.
        agent.think(1, &[]).await.unwrap();

        assert_eq!(calls.load(std::sync::atomic::Ordering::Acquire), 1);
        let injected = agent.conversation().iter().any(|m| {
            m.role == Role::User
                && m.content.contains("相关记忆")
                && m.content.contains("cargo clean first")
        });
        assert!(injected, "background-prefetched memory must be injected");
    }

    #[tokio::test]
    async fn relevant_memory_prefetch_skips_single_word_query() {
        let transport = Arc::new(AttemptTransport {
            attempts: Mutex::new(VecDeque::from([response_text_completed("ok")])),
            requests: Mutex::new(Vec::new()),
        });
        let client = Arc::new(ModelClient::new(
            transport.clone(),
            ModelConfig::deepseek("test"),
        ));
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let provider = Arc::new(ScriptedMemoryProvider {
            mems: vec![RelevantMemory {
                id: "k1".to_string(),
                block: "b".to_string(),
            }],
            calls: calls.clone(),
        });
        // Single-word goal → too little context; prefetch must not fire.
        let mut agent = ModelAgent::new(client, "model", "system", "hi", vec![])
            .with_relevant_memory_provider(provider);
        agent.think(0, &[]).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        assert_eq!(calls.load(std::sync::atomic::Ordering::Acquire), 0);
    }

    struct StaticTodoSource {
        snapshot: TodoReminderSnapshot,
    }

    impl TodoReminderSource for StaticTodoSource {
        fn todo_snapshot(&self) -> TodoReminderSnapshot {
            self.snapshot.clone()
        }
    }

    fn todo_reminders_in(agent: &ModelAgent) -> usize {
        agent
            .conversation()
            .iter()
            .filter(|m| m.content.contains("todo-tracking tool hasn't been used"))
            .count()
    }

    #[test]
    fn todo_reminder_fires_after_inactivity_threshold_with_list() {
        let source = Arc::new(StaticTodoSource {
            snapshot: TodoReminderSnapshot {
                has_items: true,
                rendered: "- [ ] wire the parser\n- [~] running the tests".to_string(),
            },
        });
        let mut agent = ModelAgent::new(client(vec![]), "m", "sys", "goal", vec![])
            .with_todo_reminder_source(source);

        // Turns 1-9: below threshold, no reminder.
        for _ in 0..(TODO_REMINDER_TURN_THRESHOLD - 1) {
            agent.maybe_inject_todo_reminder();
        }
        assert_eq!(todo_reminders_in(&agent), 0);

        // Turn 10: both counters reach the threshold → one reminder with list.
        agent.maybe_inject_todo_reminder();
        assert_eq!(todo_reminders_in(&agent), 1);
        assert!(agent
            .conversation()
            .iter()
            .any(|m| m.content.contains("wire the parser")));

        // Immediately after: reminder counter reset, so no back-to-back nag.
        agent.maybe_inject_todo_reminder();
        assert_eq!(todo_reminders_in(&agent), 1);

        // A todo_write resets the write counter → no reminder for another
        // full window even as turns accrue.
        agent.turns_since_todo_write = 0;
        for _ in 0..(TODO_REMINDER_TURN_THRESHOLD - 1) {
            agent.maybe_inject_todo_reminder();
        }
        assert_eq!(todo_reminders_in(&agent), 1);
    }

    #[test]
    fn todo_reminder_empty_list_nags_without_appending_items() {
        let source = Arc::new(StaticTodoSource {
            snapshot: TodoReminderSnapshot {
                has_items: false,
                rendered: "Current todo list is empty.".to_string(),
            },
        });
        let mut agent = ModelAgent::new(client(vec![]), "m", "sys", "goal", vec![])
            .with_todo_reminder_source(source);
        for _ in 0..TODO_REMINDER_TURN_THRESHOLD {
            agent.maybe_inject_todo_reminder();
        }
        assert_eq!(todo_reminders_in(&agent), 1);
        // Empty list → nag only, no "Current todo list:" section appended.
        assert!(!agent
            .conversation()
            .iter()
            .any(|m| m.content.contains("Current todo list:")));
    }

    #[test]
    fn todo_reminder_noop_without_source() {
        let mut agent = ModelAgent::new(client(vec![]), "m", "sys", "goal", vec![]);
        for _ in 0..(TODO_REMINDER_TURN_THRESHOLD * 2) {
            agent.maybe_inject_todo_reminder();
        }
        assert_eq!(todo_reminders_in(&agent), 0);
    }
}
