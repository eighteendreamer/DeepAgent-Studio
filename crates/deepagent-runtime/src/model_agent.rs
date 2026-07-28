//! A model-driven [`Agent`] implementation.
//!
//! [`ModelAgent`] turns the scripted demo brain into a real one backed by a
//! [`ModelClient`]. It maintains the running conversation, advertises the
//! available tools, and translates the model's streamed [`ChatResponse`] into
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
use deepagent_models::chat::{FinishReason, ThinkingDepth};
use deepagent_models::{
    classify_model_error, ChatRequest, ChatResponse, DeltaObserver, ModelClient, ModelFailureKind,
    ModelStreamEvent, ToolSchema,
};
use deepagent_tools::ToolInvocation;

use crate::agent::{Agent, AgentDecision, Observation, ToolAttemptController};
use crate::events::{RuntimeEvent, RuntimeEventSink};

#[derive(Debug, Clone)]
pub struct ReactiveCompaction {
    pub messages: Vec<Message>,
    pub tokens_before: u64,
    pub tokens_after: u64,
    pub summary: String,
}

#[async_trait]
pub trait ReactiveContextCompactor: Send + Sync {
    async fn compact(&self, messages: &[Message]) -> Result<Option<ReactiveCompaction>>;
}

/// Sliding-window size for wall-banging detection across tool observations.
const FAILURE_WINDOW: usize = 12;
/// Same-tool failures inside the window that force a strategy-change hint.
const STRATEGY_SHIFT_THRESHOLD: usize = 4;

/// An [`Agent`] that delegates decision-making to a model.
pub struct ModelAgent {
    client: Arc<ModelClient>,
    model: String,
    /// Running conversation, seeded with system + user messages.
    messages: Vec<Message>,
    /// Tool schemas advertised to the model each turn.
    tools: Vec<ToolSchema>,
    /// Most recent tool call id, used to correlate the next tool result.
    pending_tool_call_id: Option<String>,
    /// Optional live event sink: when set, token/reasoning deltas are forwarded
    /// as [`RuntimeEvent`]s for streaming to a UI.
    events: Option<Arc<dyn RuntimeEventSink>>,
    /// Cumulative token usage summed across every model call this run.
    usage: crate::agent::RunUsage,
    /// DeepSeek Thinking Mode depth applied to every request.
    thinking_depth: ThinkingDepth,
    /// Provider attempts per turn for transient/empty-stream failures.
    max_model_attempts: usize,
    /// Optional same-endpoint model used once when the primary deployment is
    /// overloaded. Authentication/rate-limit errors do not trigger fallback.
    fallback_model: Option<String>,
    reactive_compactor: Option<Arc<dyn ReactiveContextCompactor>>,
    /// Sliding window of recent tool outcomes `(tool, ok)`, used to detect a
    /// wall-banging loop: many failures of the same tool in a short span even
    /// with unrelated successes interleaved. Drives the escalated recovery
    /// hint that forces a strategy change instead of endless retry variants.
    recent_tool_results: std::collections::VecDeque<(String, bool)>,
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
        Self {
            client,
            model: model.into(),
            messages: vec![Message::system(system), Message::user(goal)],
            tools,
            pending_tool_call_id: None,
            events: None,
            usage: crate::agent::RunUsage::default(),
            thinking_depth: ThinkingDepth::default(),
            max_model_attempts: 3,
            fallback_model: None,
            reactive_compactor: None,
            recent_tool_results: std::collections::VecDeque::new(),
        }
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
            self.messages.push(Message::tool_result(call_id, content));
        } else {
            // Verification, Stop-hook and CompletionGate feedback is generated
            // by the kernel, not by a model tool_use block. Sending it as a
            // tool role would create an orphan tool result rejected by strict
            // providers. Keep it structured but feed it back as a user turn.
            self.messages.push(Message::user(content));
        }
    }
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
    request: ChatRequest,
    sink: Option<Arc<dyn RuntimeEventSink>>,
    step: usize,
    started_at: std::time::Instant,
    cancel: Option<Arc<AtomicBool>>,
    tools: Option<&'a mut dyn ToolAttemptController>,
) -> (
    Result<ChatResponse>,
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
            .stream_chat_observed_cancelled(request, &mut observer, cancel)
            .await
    } else {
        client.stream_chat_observed(request, &mut observer).await
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

        let mut request = ChatRequest::new(self.model.clone(), self.messages.clone())
            .with_thinking_depth(self.thinking_depth)
            .with_tools(self.tools.clone());
        let message_count = request.messages.len();
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
                        let current_max = request.max_tokens.unwrap_or(8_192);
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
                            request.max_tokens = Some(escalated_max);
                            max_output_escalated = true;
                            continue;
                        }
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
                            match compactor.compact(&self.messages).await {
                                Ok(Some(compacted)) => {
                                    self.messages = compacted.messages;
                                    request.messages = self.messages.clone();
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
            self.usage.total_tokens += usage.total_tokens;
            self.usage.prompt_cache_hit_tokens += usage.prompt_cache_hit_tokens;
            self.usage.prompt_cache_miss_tokens += usage.prompt_cache_miss_tokens;
            if let Some(sink) = &self.events {
                sink.emit(RuntimeEvent::Usage {
                    prompt_tokens: usage.prompt_tokens,
                    completion_tokens: usage.completion_tokens,
                    total_tokens: usage.total_tokens,
                    prompt_cache_hit_tokens: usage.prompt_cache_hit_tokens,
                    prompt_cache_miss_tokens: usage.prompt_cache_miss_tokens,
                    cost_yuan: None,
                });
            }
        }

        // Persist the assistant turn in the agent's running conversation.
        // Thinking Mode reasoning is preserved for both tool-call and final
        // turns so the outer session log can replay it after refresh.
        let mut assistant = Message::text(Role::Assistant, response.message.content.clone());
        assistant.tool_calls = response.message.tool_calls.clone();
        assistant.reasoning_content = response.message.reasoning_content.clone();
        self.messages.push(assistant);

        // Decide the next action. The model may emit several tool calls in one
        // turn (parallel tool calling) — carry all of them, each tagged with its
        // own id so results correlate back correctly.
        let calls = &response.message.tool_calls;
        if !calls.is_empty() {
            let invocations: Vec<ToolInvocation> = calls
                .iter()
                .map(|c| {
                    ToolInvocation::new(c.name.clone(), c.arguments.clone()).with_id(c.id.clone())
                })
                .collect();
            // Track the last id for the legacy single-call fallback path.
            self.pending_tool_call_id = calls.last().map(|c| c.id.clone());
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
            _ => Ok(AgentDecision::CompleteMessage(response.message)),
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
    std::time::Duration::from_millis(
        (base_ms * 2u64.saturating_pow(attempt.saturating_sub(1) as u32)).min(4_000),
    )
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
        async fn compact(&self, messages: &[Message]) -> Result<Option<ReactiveCompaction>> {
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
        let events = vec![
            r#"{"choices":[{"delta":{"content":"All done."},"finish_reason":"stop"}]}"#.to_string(),
            "[DONE]".to_string(),
        ];
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
    async fn retries_empty_200_stream_without_recording_failed_turn() {
        let transport = Arc::new(AttemptTransport {
            attempts: Mutex::new(VecDeque::from([
                vec!["[DONE]".to_string()],
                vec![
                    r#"{"choices":[{"delta":{"content":"recovered"},"finish_reason":"stop"}]}"#
                        .to_string(),
                    "[DONE]".to_string(),
                ],
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
        assert!(transport.attempts.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn retry_resets_visible_deltas_from_failed_attempt() {
        let transport = Arc::new(AttemptTransport {
            attempts: Mutex::new(VecDeque::from([
                vec![
                    r#"{"choices":[{"delta":{"content":"partial"}}]}"#.to_string(),
                    "__ERROR_EOF__".to_string(),
                ],
                vec![
                    r#"{"choices":[{"delta":{"content":"final"},"finish_reason":"stop"}]}"#
                        .to_string(),
                    "[DONE]".to_string(),
                ],
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
                vec![
                    r#"{"choices":[{"delta":{"content":"must not run"},"finish_reason":"stop"}]}"#
                        .to_string(),
                    "[DONE]".to_string(),
                ],
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
                vec![
                    r#"{"choices":[{"delta":{"content":"after compact"},"finish_reason":"stop"}]}"#
                        .to_string(),
                    "[DONE]".to_string(),
                ],
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
        assert!(requests[0]["messages"].as_array().unwrap().len() > 3);
        assert_eq!(requests[1]["messages"].as_array().unwrap().len(), 3);
    }

    #[tokio::test]
    async fn repeated_context_overflow_trips_compaction_circuit_breaker() {
        let transport = Arc::new(AttemptTransport {
            attempts: Mutex::new(VecDeque::from([
                vec!["__ERROR_CONTEXT__".to_string()],
                vec!["__ERROR_CONTEXT__".to_string()],
                vec![
                    r#"{"choices":[{"delta":{"content":"must not run"},"finish_reason":"stop"}]}"#
                        .to_string(),
                ],
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
    async fn rate_limited_429_retries_and_recovers_within_budget() {
        let transport = Arc::new(AttemptTransport {
            attempts: Mutex::new(VecDeque::from([
                vec!["__ERROR_429__".to_string()],
                vec![
                    r#"{"choices":[{"delta":{"content":"after backoff"},"finish_reason":"stop"}]}"#
                        .to_string(),
                    "[DONE]".to_string(),
                ],
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
        let events = vec![
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"bad","function":{"name":"read_file","arguments":"{not valid json"}}]},"finish_reason":"tool_calls"}]}"#.to_string(),
            "[DONE]".to_string(),
        ];
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
                vec![
                    r#"{"choices":[{"delta":{"content":"fallback ok"},"finish_reason":"stop"}]}"#
                        .to_string(),
                    "[DONE]".to_string(),
                ],
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
                vec![
                    r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"stale","function":{"name":"read_file","arguments":"{\"path\":\"a.txt\"}"}}]},"finish_reason":"length"}]}"#.to_string(),
                    "[DONE]".to_string(),
                ],
                vec![
                    r#"{"choices":[{"delta":{"content":"recovered"},"finish_reason":"stop"}]}"#
                        .to_string(),
                    "[DONE]".to_string(),
                ],
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
        assert_eq!(requests[1]["max_tokens"], 65_536);
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
    async fn final_answer_preserves_reasoning_for_replay() {
        let events = vec![
            r#"{"choices":[{"delta":{"reasoning_content":"I should inspect the image. "}}]}"#
                .to_string(),
            r#"{"choices":[{"delta":{"content":"It shows a compile error."},"finish_reason":"stop"}]}"#
                .to_string(),
            "[DONE]".to_string(),
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
        let events = vec![
            r#"{"choices":[{"delta":{"reasoning_content":"I should add."}}]}"#.to_string(),
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c1","function":{"name":"add","arguments":"{\"a\":1,\"b\":2}"}}]},"finish_reason":"tool_calls"}]}"#.to_string(),
            "[DONE]".to_string(),
        ];
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
        let events = vec![
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c1","function":{"name":"add","arguments":"{\"a\":1,\"b\":2}"}}]}}]}"#.to_string(),
            r#"{"choices":[{"delta":{},"finish_reason":"tool_calls"}]}"#.to_string(),
            "[DONE]".to_string(),
        ];
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
                vec![
                    r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"stale","function":{"name":"add","arguments":"{\"a\":1,\"b\":2}"}}]}}]}"#.to_string(),
                    "__ERROR_EOF__".to_string(),
                ],
                vec![
                    r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"fresh","function":{"name":"add","arguments":"{\"a\":2,\"b\":3}"}}]},"finish_reason":"tool_calls"}]}"#.to_string(),
                    "[DONE]".to_string(),
                ],
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
    async fn feeds_observation_back_as_tool_message() {
        let events = vec![
            r#"{"choices":[{"delta":{"content":"sum is 3"},"finish_reason":"stop"}]}"#.to_string(),
            "[DONE]".to_string(),
        ];
        let mut agent = ModelAgent::new(client(events), "deepseek-v4-flash", "sys", "add", vec![]);
        let obs = Observation {
            tool: "add".to_string(),
            ok: true,
            output: serde_json::json!({"sum": 3}),
            call_id: Some("c1".to_string()),
        };
        agent.think(1, std::slice::from_ref(&obs)).await.unwrap();
        // A tool-role message correlated to c1 was inserted.
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
    async fn failed_observation_is_fed_back_with_recovery_hint() {
        let events = vec![
            r#"{"choices":[{"delta":{"content":"ok let me retry"},"finish_reason":"stop"}]}"#
                .to_string(),
            "[DONE]".to_string(),
        ];
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
        let events = vec![
            r#"{"choices":[{"delta":{"content":"retry complete"},"finish_reason":"stop"}]}"#
                .to_string(),
            "[DONE]".to_string(),
        ];
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
}
