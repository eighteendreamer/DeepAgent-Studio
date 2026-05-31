//! The Agent Runtime Loop engine.
//!
//! Drives an [`Agent`] through the THINK -> EXECUTE -> OBSERVE cycle, persisting
//! each decision to the append-only event store and updating runtime
//! [`Metrics`]. The loop terminates when the agent completes, requests approval,
//! hits the step limit, or errors.
//!
//! This is the Phase 2 skeleton of the full loop; VERIFY / REFLECT / COMPACT
//! phases are represented as explicit hooks that later phases flesh out.

use deepagent_core::clock::Clock;
use deepagent_core::error::Result;
use deepagent_core::event::EventPayload;
use deepagent_core::id::TaskId;
use deepagent_core::message::{Message, ToolCall};
use deepagent_core::task::TaskState;
use deepagent_hooks::{HookContext, HookData, HookOutcome, HookPoint, HookRegistry};
use deepagent_session::Session;
use deepagent_tools::permission::PermissionSet;
use deepagent_tools::ToolRegistry;
use deepagent_tracing::metrics::{names, Metrics};
use deepagent_verification::reflection::NextAction;
use deepagent_verification::{CommandRunner, ReflectionEngine, VerificationStep, Verifier};

use crate::agent::{Agent, AgentDecision, Observation};
use crate::approval::{ApprovalDecision, ApprovalGate, ApprovalRequest, AutoDenyGate};
use crate::events::{NullEventSink, RuntimeEvent, RuntimeEventSink};

/// An optional post-completion verification plan (开发计划.md Phase 7).
///
/// When attached, the runtime runs these steps after the agent declares
/// completion. On failure (within the reflection engine's retry budget) the
/// agent is given the reflection as an observation and the loop continues, so
/// the agent can self-heal before the run truly completes.
pub struct VerificationPlan {
    /// The build/test/lint steps to run.
    pub steps: Vec<VerificationStep>,
    /// Command runner (real process runner or a mock), behind an Arc.
    pub runner: std::sync::Arc<dyn CommandRunner>,
    /// Max times the same failure may recur before giving up.
    pub max_repeats: u32,
    /// Absolute cap on verification attempts.
    pub max_attempts: u32,
}

impl VerificationPlan {
    /// Build a plan from steps and a runner with default loop-detection limits.
    pub fn new(steps: Vec<VerificationStep>, runner: std::sync::Arc<dyn CommandRunner>) -> Self {
        Self {
            steps,
            runner,
            max_repeats: 2,
            max_attempts: 5,
        }
    }
}

/// A tool call whose I/O already completed concurrently, ready to have its
/// events recorded in order (see [`RuntimeEngine::record_parallel_result`]).
struct CompletedToolCall {
    call_id: String,
    tool_name: String,
    arguments: serde_json::Value,
    result: Result<deepagent_tools::ToolOutput>,
    duration_ms: u64,
}

/// Why a run stopped.
#[derive(Debug, Clone, PartialEq)]
pub enum RunOutcome {
    /// The agent declared completion with a final message.
    Completed(String),
    /// The agent requested human approval and the loop yielded.
    AwaitingApproval(String),
    /// The configured maximum number of steps was reached.
    StepLimitReached,
    /// The run was cancelled by the user (manual stop).
    Cancelled,
}

/// The result of the `UserPromptSubmit` gate (复刻规范 P1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptDecision {
    /// The prompt is accepted (possibly rewritten by a `Modify` hook). Carries
    /// the effective prompt text to turn into a task.
    Accept(String),
    /// A hook requested approval before the prompt may proceed.
    NeedsApproval {
        /// The (unmodified) prompt awaiting approval.
        prompt: String,
        /// Why approval is required.
        reason: String,
    },
    /// A hook rejected the prompt outright.
    Rejected {
        /// Why the prompt was rejected.
        reason: String,
    },
}

impl PromptDecision {
    /// The accepted prompt text, if accepted.
    pub fn accepted(&self) -> Option<&str> {
        match self {
            PromptDecision::Accept(p) => Some(p),
            _ => None,
        }
    }

    /// Whether the prompt was accepted.
    pub fn is_accepted(&self) -> bool {
        matches!(self, PromptDecision::Accept(_))
    }
}

/// Tuning knobs for a run.
#[derive(Debug, Clone)]
pub struct RuntimeConfig {
    /// Hard cap on loop iterations (safety against runaway loops).
    pub max_steps: usize,
    /// Permissions granted to the agent for this run.
    pub permissions: PermissionSet,
    /// Whether high-risk tools are pre-approved for this run.
    pub auto_approve: bool,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            max_steps: 64,
            permissions: PermissionSet::developer(),
            auto_approve: false,
        }
    }
}

/// The runtime engine. Borrows the collaborators it needs for one run.
pub struct RuntimeEngine<'a, C: Clock> {
    registry: &'a ToolRegistry,
    metrics: Metrics,
    config: RuntimeConfig,
    hooks: Option<&'a HookRegistry>,
    verification: Option<&'a VerificationPlan>,
    events: std::sync::Arc<dyn RuntimeEventSink>,
    approvals: std::sync::Arc<dyn ApprovalGate>,
    cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    _clock: std::marker::PhantomData<&'a C>,
}

impl<'a, C: Clock> RuntimeEngine<'a, C> {
    /// Construct an engine.
    pub fn new(registry: &'a ToolRegistry, metrics: Metrics, config: RuntimeConfig) -> Self {
        Self {
            registry,
            metrics,
            config,
            hooks: None,
            verification: None,
            events: std::sync::Arc::new(NullEventSink),
            approvals: std::sync::Arc::new(AutoDenyGate),
            cancel: None,
            _clock: std::marker::PhantomData,
        }
    }

    /// Attach a cancellation flag (builder style). When set to `true` mid-run,
    /// the loop stops at the next step boundary and returns
    /// [`RunOutcome::Cancelled`] (manual stop from the UI).
    pub fn with_cancel(mut self, cancel: std::sync::Arc<std::sync::atomic::AtomicBool>) -> Self {
        self.cancel = Some(cancel);
        self
    }

    /// Whether a cancellation has been requested.
    fn is_cancelled(&self) -> bool {
        self.cancel
            .as_ref()
            .map(|c| c.load(std::sync::atomic::Ordering::Relaxed))
            .unwrap_or(false)
    }

    /// Attach a live event sink (builder style). Events are pushed as the run
    /// progresses, for streaming to a UI. Defaults to a no-op sink.
    pub fn with_events(mut self, events: std::sync::Arc<dyn RuntimeEventSink>) -> Self {
        self.events = events;
        self
    }

    /// Attach an approval gate (builder style). When a tool needs approval
    /// (an `Ask` hook outcome, or a high-risk tool without auto-approve), the
    /// runtime awaits the gate's decision. Defaults to deny-all (safe when no
    /// UI is attached).
    pub fn with_approvals(mut self, approvals: std::sync::Arc<dyn ApprovalGate>) -> Self {
        self.approvals = approvals;
        self
    }

    /// Emit a live runtime event (no-op if no sink attached).
    fn emit(&self, event: RuntimeEvent) {
        self.events.emit(event);
    }

    /// Attach a verification plan (builder style). When set, the runtime runs
    /// the plan after the agent completes and drives self-healing.
    pub fn with_verification(mut self, plan: &'a VerificationPlan) -> Self {
        self.verification = Some(plan);
        self
    }

    /// Attach a hook registry (builder style). Hooks fire at the lifecycle
    /// points described in [`deepagent_hooks::HookPoint`].
    pub fn with_hooks(mut self, hooks: &'a HookRegistry) -> Self {
        self.hooks = Some(hooks);
        self
    }

    /// Dispatch hooks at `point`, returning the effective outcome. If no
    /// registry is attached, always `Continue`.
    async fn fire_hook(
        &self,
        session_id: deepagent_core::id::SessionId,
        point: HookPoint,
        data: HookData,
    ) -> Result<HookOutcome> {
        match self.hooks {
            Some(reg) => {
                let ctx = HookContext::new(session_id, point, data);
                reg.dispatch(&ctx).await
            }
            None => Ok(HookOutcome::Continue),
        }
    }

    /// Run `agent` against `task` within `session` until it terminates.
    ///
    /// Every step is recorded to the event log, so the run is fully replayable
    /// and crash-recoverable.
    pub async fn run(
        &self,
        session: &mut Session<'a, C>,
        task: TaskId,
        agent: &mut dyn Agent,
    ) -> Result<RunOutcome> {
        let session_id = session.id();
        let run_started_at = std::time::Instant::now();

        // SessionStart hook (observational).
        self.fire_hook(session_id, HookPoint::SessionStart, HookData::None)
            .await?;

        self.emit(RuntimeEvent::RunStarted {
            task_id: task.to_string(),
        });

        // Announce the backing session early so a UI can register & navigate to
        // it WHILE it runs (not only on completion). This fixes the case where a
        // user starts a task, switches away, and cannot find the in-flight chat.
        self.emit(RuntimeEvent::SessionRegistered {
            session_id: session_id.to_string(),
            title: session.state().title.clone(),
        });

        // Move the task into Running (validated by the session).
        if session.state().task(task).map(|t| t.state) == Some(TaskState::Queued) {
            session.transition_task(task, TaskState::Running)?;
        }

        let mut last_observations: Vec<Observation> = Vec::new();
        let mut outcome = RunOutcome::StepLimitReached;
        let mut finished = false;

        // Verification state persists across attempts (tracks loop detection).
        let mut reflection_engine = self
            .verification
            .map(|p| ReflectionEngine::new(p.max_repeats, p.max_attempts));

        for step in 0..self.config.max_steps {
            // Honor a manual stop requested from the UI: end cleanly at the
            // step boundary so the partial transcript is preserved.
            if self.is_cancelled() {
                session.transition_task(task, TaskState::Failed)?;
                self.emit(RuntimeEvent::RunCancelled);
                outcome = RunOutcome::Cancelled;
                finished = true;
                break;
            }
            self.emit(RuntimeEvent::TurnStarted { step });
            let decision = agent.think(step, &last_observations).await?;
            tracing::debug!(step, ?decision, "agent decision");

            match decision {
                AgentDecision::Complete(msg) => {
                    // Post-completion verification / self-healing.
                    if let (Some(plan), Some(engine)) =
                        (self.verification, reflection_engine.as_mut())
                    {
                        match self
                            .verify_after_completion(session, session_id, plan, engine)
                            .await?
                        {
                            VerifyStep::Passed | VerifyStep::GaveUp => {
                                // Either clean or exhausted: accept completion.
                            }
                            VerifyStep::Retry(obs) => {
                                // Hand the reflection back; keep the task running.
                                last_observations = vec![obs];
                                continue;
                            }
                        }
                    }

                    session.append(EventPayload::MessageAppended {
                        message: Message::assistant(&msg),
                    })?;
                    session.transition_task(task, TaskState::Completed)?;
                    self.emit(RuntimeEvent::RunCompleted {
                        message: msg.clone(),
                    });
                    outcome = RunOutcome::Completed(msg);
                    finished = true;
                    break;
                }

                AgentDecision::NeedsApproval(msg) => {
                    session.append(EventPayload::Note {
                        text: format!("approval requested: {msg}"),
                    })?;
                    session.transition_task(task, TaskState::WaitingApproval)?;
                    self.emit(RuntimeEvent::RunAwaitingApproval {
                        message: msg.clone(),
                    });
                    outcome = RunOutcome::AwaitingApproval(msg);
                    finished = true;
                    break;
                }

                AgentDecision::CallTool(invocation) => {
                    last_observations =
                        vec![self.execute_tool(session, session_id, invocation).await?];
                }

                AgentDecision::CallTools(invocations) => {
                    last_observations =
                        self.execute_tools(session, session_id, invocations).await?;
                }
            }
        }

        if !finished {
            // Ran out of steps: mark failed so the task does not linger.
            session.transition_task(task, TaskState::Failed)?;
            self.emit(RuntimeEvent::RunFailed {
                reason: format!("step limit reached ({} steps)", self.config.max_steps),
            });
        }

        // Persist the run's cumulative token usage + wall-clock duration so the
        // UI can show per-turn metrics when the session is reopened later.
        if let Some(u) = agent.cumulative_usage() {
            let duration_ms = run_started_at.elapsed().as_millis() as u64;
            session.append(EventPayload::UsageRecorded {
                prompt_tokens: u.prompt_tokens,
                completion_tokens: u.completion_tokens,
                total_tokens: u.total_tokens,
                prompt_cache_hit_tokens: u.prompt_cache_hit_tokens,
                prompt_cache_miss_tokens: u.prompt_cache_miss_tokens,
                duration_ms,
            })?;
        }

        // SessionEnd hook (observational). The session stays open for resumption
        // unless the caller ends it; this just notifies hooks the run finished.
        self.fire_hook(session_id, HookPoint::SessionEnd, HookData::None)
            .await?;

        Ok(outcome)
    }

    /// Run the `UserPromptSubmit` gate for a freshly submitted prompt.
    ///
    /// This is the Prompt Submission layer (复刻规范 P1): before raw input
    /// becomes a task, hooks may **deny** it (reject the prompt), **modify** it
    /// (rewrite/augment the text), or **ask** for approval. The returned
    /// [`PromptDecision`] tells the caller how to proceed.
    ///
    /// If no hook registry is attached, the prompt is accepted unchanged.
    pub async fn submit_prompt(
        &self,
        session_id: deepagent_core::id::SessionId,
        prompt: impl Into<String>,
    ) -> Result<PromptDecision> {
        let prompt = prompt.into();
        let outcome = self
            .fire_hook(
                session_id,
                HookPoint::UserPromptSubmit,
                HookData::prompt(prompt.clone()),
            )
            .await?;

        let decision = match outcome {
            HookOutcome::Continue => PromptDecision::Accept(prompt),
            HookOutcome::Modify { updated_input, .. } => {
                // For a prompt, the rewritten text is taken from `updated_input`:
                // either a bare JSON string or an object with a `text` field.
                let rewritten = updated_input
                    .as_str()
                    .map(|s| s.to_string())
                    .or_else(|| {
                        updated_input
                            .get("text")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string())
                    })
                    .unwrap_or(prompt);
                PromptDecision::Accept(rewritten)
            }
            HookOutcome::Ask { reason, .. } => PromptDecision::NeedsApproval { prompt, reason },
            HookOutcome::Deny { reason, .. } => PromptDecision::Rejected { reason },
        };
        Ok(decision)
    }

    /// Execute several tool invocations requested in the same model turn.
    ///
    /// Mirrors Claude Code's parallel tool calling. To keep the append-only
    /// event log deterministic, the request/gate/completion **events** are
    /// written sequentially in the model's call order; what runs concurrently is
    /// the underlying tool I/O for **read-only, concurrency-safe** tools (file
    /// reads, glob/grep, web fetch/search). Any non-read-only tool (writes,
    /// edits, bash) and any tool needing approval is run sequentially via
    /// [`RuntimeEngine::execute_tool`] to preserve ordering and safety.
    ///
    /// A single invocation short-circuits to [`RuntimeEngine::execute_tool`].
    async fn execute_tools(
        &self,
        session: &mut Session<'a, C>,
        session_id: deepagent_core::id::SessionId,
        invocations: Vec<deepagent_tools::ToolInvocation>,
    ) -> Result<Vec<Observation>> {
        if invocations.len() <= 1 {
            let mut out = Vec::with_capacity(invocations.len());
            for inv in invocations {
                out.push(self.execute_tool(session, session_id, inv).await?);
            }
            return Ok(out);
        }

        // Partition: read-only/concurrency-safe tools can have their I/O run in
        // parallel; everything else runs sequentially for safety + ordering.
        let parallel_idx: Vec<usize> = invocations
            .iter()
            .enumerate()
            .filter(|(_, inv)| self.is_parallel_safe(&inv.name))
            .map(|(i, _)| i)
            .collect();

        // Fast path: nothing parallelizable → just run them in order.
        if parallel_idx.len() <= 1 {
            let mut out = Vec::with_capacity(invocations.len());
            for inv in invocations {
                out.push(self.execute_tool(session, session_id, inv).await?);
            }
            return Ok(out);
        }

        // Pre-assign call ids and emit ToolStarted for EVERY parallel tool up
        // front, so the UI shows them all as "running" immediately — not only
        // after they finish. (This is what makes parallel `task` sub-agents and
        // batched reads show live progress instead of flipping to "completed"
        // all at once at the very end.)
        use futures::stream::{FuturesUnordered, StreamExt};
        let parallel_set: std::collections::HashSet<usize> = parallel_idx.iter().copied().collect();

        let mut call_ids: std::collections::HashMap<usize, String> =
            std::collections::HashMap::new();
        for &i in &parallel_idx {
            let inv = &invocations[i];
            let call_id = inv
                .id
                .clone()
                .unwrap_or_else(|| format!("call_{}", deepagent_core::id::EventId::new()));
            self.emit(RuntimeEvent::ToolStarted {
                name: inv.name.clone(),
                call_id: call_id.clone(),
                arguments: inv.arguments.clone(),
            });
            call_ids.insert(i, call_id);
        }

        // Concurrently invoke the parallel-safe tools' I/O (no session access).
        let mut futs = FuturesUnordered::new();
        for &i in &parallel_idx {
            let inv = &invocations[i];
            let call_id = call_ids[&i].clone();
            let name = inv.name.clone();
            let args = inv.arguments.clone();
            let registry = self.registry;
            let perms = self.config.permissions.clone();
            let auto = self.config.auto_approve;
            futs.push(async move {
                let start = std::time::Instant::now();
                let result = registry.invoke(&name, args, &perms, auto).await;
                let duration_ms = start.elapsed().as_millis() as u64;
                (i, call_id, name, result, duration_ms)
            });
        }
        let mut results: std::collections::HashMap<
            usize,
            (String, String, Result<deepagent_tools::ToolOutput>, u64),
        > = std::collections::HashMap::new();
        // Drain completions AS THEY ARRIVE and emit ToolCompleted immediately,
        // so each parallel tool flips to ok/error the moment it actually
        // finishes (live, out-of-order progress) — the session-log append still
        // happens deterministically below in the model's original order.
        while let Some((i, call_id, name, result, duration_ms)) = futs.next().await {
            let (ok, value) = match &result {
                Ok(out) => (out.ok, out.value.clone()),
                Err(e) => (false, serde_json::json!({ "error": e.to_string() })),
            };
            self.emit(RuntimeEvent::ToolCompleted {
                name: name.clone(),
                call_id: call_id.clone(),
                ok,
                output: value,
                duration_ms,
            });
            results.insert(i, (call_id, name, result, duration_ms));
        }

        // Now stitch observations back together in the model's original order,
        // appending events to the session log sequentially. Parallel-safe
        // entries use their precomputed result (events already emitted live, so
        // we skip re-emitting); the rest run through the normal sequential path.
        let mut observations = Vec::with_capacity(invocations.len());
        for (i, inv) in invocations.into_iter().enumerate() {
            if parallel_set.contains(&i) {
                let (call_id, tool_name, result, duration_ms) =
                    results.remove(&i).expect("parallel result present");
                observations.push(
                    self.record_parallel_result(
                        session,
                        session_id,
                        CompletedToolCall {
                            call_id,
                            tool_name,
                            arguments: inv.arguments,
                            result,
                            duration_ms,
                        },
                    )
                    .await?,
                );
            } else {
                observations.push(self.execute_tool(session, session_id, inv).await?);
            }
        }
        Ok(observations)
    }

    /// Whether a tool is safe to run concurrently with others. Two cases:
    /// - read-only tools (`RiskLevel::Safe`) like file reads, glob/grep, web
    ///   fetch/search — no side effects, so their I/O can overlap;
    /// - the `task` sub-agent tool: each call runs an isolated nested agent on
    ///   its own ephemeral session, so launching several at once is safe and is
    ///   exactly Claude Code's parallel-Task pattern (the big latency win when
    ///   exploring multiple directories/files at once).
    ///
    /// An unknown tool is treated as not parallel-safe (it'll fail in the normal
    /// path with a clean error).
    fn is_parallel_safe(&self, name: &str) -> bool {
        if name == "task" {
            return true;
        }
        match self.registry.get(name) {
            Some(spec) => spec.descriptor.risk == deepagent_tools::RiskLevel::Safe,
            None => false,
        }
    }

    /// Record the request + completion events for a tool whose I/O already ran
    /// concurrently (in [`RuntimeEngine::execute_tools`]), returning its
    /// [`Observation`]. The live `ToolStarted`/`ToolCompleted` UI events were
    /// already emitted by `execute_tools` (up front / as each finished); this
    /// only writes the append-only session log (in deterministic call order)
    /// and fires the AfterToolUse hook. Skips the BeforeToolUse gate (only
    /// invoked for parallel-safe tools that don't need approval).
    async fn record_parallel_result(
        &self,
        session: &mut Session<'a, C>,
        session_id: deepagent_core::id::SessionId,
        completed: CompletedToolCall,
    ) -> Result<Observation> {
        let CompletedToolCall {
            call_id,
            tool_name,
            arguments,
            result,
            duration_ms,
        } = completed;
        self.metrics.incr(names::TOOL_CALLS, 1);
        session.append(EventPayload::ToolCallRequested {
            call: ToolCall {
                id: call_id.clone(),
                name: tool_name.clone(),
                arguments: arguments.clone(),
            },
        })?;

        let (ok, value) = match result {
            Ok(out) => (out.ok, out.value),
            Err(e) => (false, serde_json::json!({ "error": e.to_string() })),
        };
        if !ok {
            self.metrics.incr(names::TOOL_FAILURES, 1);
        }
        session.append(EventPayload::ToolCallCompleted {
            call_id: call_id.clone(),
            ok,
            output: value.clone(),
            duration_ms,
        })?;
        self.fire_hook(
            session_id,
            HookPoint::AfterToolUse,
            HookData::after_tool(tool_name.clone(), arguments, ok),
        )
        .await?;
        Ok(Observation {
            tool: tool_name,
            ok,
            output: value,
            call_id: Some(call_id),
        })
    }

    /// Execute one tool invocation, recording request + completion events and
    /// returning the [`Observation`] to feed back to the agent.
    async fn execute_tool(
        &self,
        session: &mut Session<'a, C>,
        session_id: deepagent_core::id::SessionId,
        mut invocation: deepagent_tools::ToolInvocation,
    ) -> Result<Observation> {
        // Reuse the model's tool-call id when present so the observation
        // correlates with the exact `tool_calls[].id`; else synthesize one.
        let call_id = invocation
            .id
            .clone()
            .unwrap_or_else(|| format!("call_{}", deepagent_core::id::EventId::new()));
        let tool_name = invocation.name.clone();

        // BeforeToolUse gate: a hook may allow, rewrite the input (Modify),
        // request approval (Ask), or veto (Deny) the call. A blocked call
        // becomes a failed observation (recorded to the log) so the agent can
        // react, rather than aborting the whole run.
        let before = self
            .fire_hook(
                session_id,
                HookPoint::BeforeToolUse,
                HookData::before_tool(tool_name.clone(), invocation.arguments.clone()),
            )
            .await?;

        let block_reason: Option<String> = match before {
            HookOutcome::Continue => None,
            HookOutcome::Modify {
                updated_input,
                source,
            } => {
                tracing::info!(
                    tool = %tool_name,
                    source = source.label(),
                    "BeforeToolUse hook rewrote tool arguments"
                );
                invocation.arguments = updated_input;
                None
            }
            HookOutcome::Ask { reason, source } => {
                // `ask` requires explicit approval. With auto-approval on, the
                // call proceeds; otherwise the approval gate is consulted and we
                // await its decision (the desktop dialog, a policy, etc.).
                if self.config.auto_approve {
                    tracing::info!(
                        tool = %tool_name,
                        source = source.label(),
                        "BeforeToolUse hook asked for approval; auto-approved by config"
                    );
                    None
                } else {
                    self.emit(RuntimeEvent::ToolBlocked {
                        name: tool_name.clone(),
                        reason: reason.clone(),
                        needs_approval: true,
                    });
                    let decision = self
                        .approvals
                        .request(ApprovalRequest {
                            call_id: call_id.clone(),
                            tool: tool_name.clone(),
                            reason: reason.clone(),
                            risk: "ask".to_string(),
                            arguments: invocation.arguments.clone(),
                        })
                        .await;
                    match decision {
                        ApprovalDecision::Allow => {
                            tracing::info!(tool = %tool_name, "tool approved by gate");
                            None
                        }
                        ApprovalDecision::Deny => Some(format!("approval denied: {reason}")),
                    }
                }
            }
            HookOutcome::Deny { reason, .. } => Some(format!("blocked by hook: {reason}")),
        };

        if let Some(reason) = block_reason {
            self.metrics.incr(names::TOOL_FAILURES, 1);
            // An approval-pending ToolBlocked was already emitted in the Ask
            // branch; here we emit the terminal block (deny / approval denied).
            self.emit(RuntimeEvent::ToolBlocked {
                name: tool_name.clone(),
                reason: reason.clone(),
                needs_approval: false,
            });
            let err_value = serde_json::json!({ "error": reason });
            session.append(EventPayload::ToolCallRequested {
                call: ToolCall {
                    id: call_id.clone(),
                    name: tool_name.clone(),
                    arguments: invocation.arguments.clone(),
                },
            })?;
            session.append(EventPayload::ToolCallCompleted {
                call_id: call_id.clone(),
                ok: false,
                output: err_value.clone(),
                duration_ms: 0,
            })?;
            self.fire_hook(
                session_id,
                HookPoint::AfterToolUse,
                HookData::after_tool(tool_name.clone(), invocation.arguments.clone(), false),
            )
            .await?;
            return Ok(Observation {
                tool: tool_name,
                ok: false,
                output: err_value,
                call_id: Some(call_id),
            });
        }

        // Record the request (with a synthetic ToolCall for the stream).
        session.append(EventPayload::ToolCallRequested {
            call: ToolCall {
                id: call_id.clone(),
                name: tool_name.clone(),
                arguments: invocation.arguments.clone(),
            },
        })?;
        self.metrics.incr(names::TOOL_CALLS, 1);
        self.emit(RuntimeEvent::ToolStarted {
            name: tool_name.clone(),
            call_id: call_id.clone(),
            arguments: invocation.arguments.clone(),
        });

        let start = std::time::Instant::now();
        let output = self
            .registry
            .invoke(
                &tool_name,
                invocation.arguments.clone(),
                &self.config.permissions,
                self.config.auto_approve,
            )
            .await;
        let duration_ms = start.elapsed().as_millis() as u64;

        let observation = match output {
            Ok(out) => {
                if !out.ok {
                    self.metrics.incr(names::TOOL_FAILURES, 1);
                }
                self.emit(RuntimeEvent::ToolCompleted {
                    name: tool_name.clone(),
                    call_id: call_id.clone(),
                    ok: out.ok,
                    output: out.value.clone(),
                    duration_ms,
                });
                session.append(EventPayload::ToolCallCompleted {
                    call_id: call_id.clone(),
                    ok: out.ok,
                    output: out.value.clone(),
                    duration_ms,
                })?;
                Observation {
                    tool: tool_name.clone(),
                    ok: out.ok,
                    output: out.value,
                    call_id: Some(call_id),
                }
            }
            Err(e) => {
                // Tool was denied or failed to run: surface as a failed
                // observation rather than aborting the whole run, so the agent
                // can reflect / choose an alternative.
                self.metrics.incr(names::TOOL_FAILURES, 1);
                let err_value = serde_json::json!({ "error": e.to_string() });
                self.emit(RuntimeEvent::ToolCompleted {
                    name: tool_name.clone(),
                    call_id: call_id.clone(),
                    ok: false,
                    output: err_value.clone(),
                    duration_ms,
                });
                session.append(EventPayload::ToolCallCompleted {
                    call_id: call_id.clone(),
                    ok: false,
                    output: err_value.clone(),
                    duration_ms,
                })?;
                Observation {
                    tool: tool_name.clone(),
                    ok: false,
                    output: err_value,
                    call_id: Some(call_id),
                }
            }
        };

        // AfterToolUse hook (observational).
        self.fire_hook(
            session_id,
            HookPoint::AfterToolUse,
            HookData::after_tool(tool_name, invocation.arguments, observation.ok),
        )
        .await?;

        Ok(observation)
    }

    /// Run the verification plan after the agent declared completion, applying
    /// reflection + loop detection. Records events and fires the
    /// `VerificationFailed` hook on failure.
    async fn verify_after_completion(
        &self,
        session: &mut Session<'a, C>,
        session_id: deepagent_core::id::SessionId,
        plan: &VerificationPlan,
        engine: &mut ReflectionEngine,
    ) -> Result<VerifyStep> {
        let verifier = Verifier::new(plan.runner.clone());
        let report = verifier.run_suite(&plan.steps).await?;
        let reflection = engine.reflect(&report);

        if report.passed {
            session.append(EventPayload::Note {
                text: "verification passed".to_string(),
            })?;
            self.emit(RuntimeEvent::Verification {
                passed: true,
                detail: "verification passed".to_string(),
            });
            return Ok(VerifyStep::Passed);
        }

        // Record the failure and fire the VerificationFailed hook.
        self.metrics.incr(names::RETRIES, 1);
        let first = report.first_failure();
        let command = first.map(|f| f.name.clone()).unwrap_or_default();
        let detail = first.map(|f| f.detail.clone()).unwrap_or_default();
        session.append(EventPayload::Note {
            text: format!("verification failed: {}", reflection.diagnosis),
        })?;
        self.emit(RuntimeEvent::Verification {
            passed: false,
            detail: reflection.diagnosis.clone(),
        });
        self.fire_hook(
            session_id,
            HookPoint::VerificationFailed,
            HookData::Verification {
                command,
                detail: detail.clone(),
            },
        )
        .await?;

        match reflection.action {
            NextAction::Retry => {
                // Feed the diagnosis back to the agent as a failed observation.
                Ok(VerifyStep::Retry(Observation {
                    tool: "verification".to_string(),
                    ok: false,
                    output: serde_json::json!({
                        "verification_failed": true,
                        "diagnosis": reflection.diagnosis,
                        "failure_kind": reflection.failure_kind,
                    }),
                    call_id: None,
                }))
            }
            // Proceed should not occur for a failing report, but treat it as
            // "accept"; GiveUp also accepts completion (best effort reached).
            NextAction::Proceed | NextAction::GiveUp => Ok(VerifyStep::GaveUp),
        }
    }
}

/// Result of a post-completion verification pass.
enum VerifyStep {
    /// Verification passed; accept completion.
    Passed,
    /// Verification failed but the loop should retry; carry the observation to
    /// feed back to the agent.
    Retry(Observation),
    /// Verification failed and reflection said to stop retrying.
    GaveUp,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::testing::ScriptedAgent;
    use crate::agent::AgentDecision;
    use async_trait::async_trait;
    use deepagent_core::clock::FixedClock;
    use deepagent_hooks::{HookPoint, HookRegistry, ToolAllowlistHook};
    use deepagent_persistence::Database;
    use deepagent_tools::permission::{PermissionSet, RiskLevel};
    use deepagent_tools::{Tool, ToolDescriptor, ToolInvocation, ToolOutput};
    use std::sync::Arc;

    struct AddTool;

    #[async_trait]
    impl Tool for AddTool {
        fn descriptor(&self) -> ToolDescriptor {
            ToolDescriptor {
                name: "add".into(),
                description: "adds a and b".into(),
                parameters: serde_json::json!({"type": "object"}),
                risk: RiskLevel::Safe,
                required_permissions: PermissionSet::read_only(),
            }
        }
        async fn invoke(&self, arguments: serde_json::Value) -> Result<ToolOutput> {
            let a = arguments["a"].as_i64().unwrap_or(0);
            let b = arguments["b"].as_i64().unwrap_or(0);
            Ok(ToolOutput::success(serde_json::json!({ "sum": a + b })))
        }
    }

    fn registry() -> ToolRegistry {
        let mut r = ToolRegistry::new();
        r.register(Arc::new(AddTool)).unwrap();
        r
    }

    #[tokio::test]
    async fn loop_runs_tool_then_completes() {
        let db = Database::open_in_memory().unwrap();
        let clock = FixedClock::new(1);
        let mut session = Session::create(&db, &clock, Some("run")).unwrap();
        let task = session.create_task("add numbers").unwrap();

        let mut agent = ScriptedAgent::new([
            AgentDecision::CallTool(ToolInvocation::new(
                "add",
                serde_json::json!({"a": 2, "b": 3}),
            )),
            AgentDecision::Complete("the sum is 5".into()),
        ]);

        let reg = registry();
        let metrics = Metrics::new();
        let engine = RuntimeEngine::new(&reg, metrics.clone(), RuntimeConfig::default());

        let outcome = engine.run(&mut session, task, &mut agent).await.unwrap();
        assert_eq!(outcome, RunOutcome::Completed("the sum is 5".into()));

        // Task ended Completed.
        assert_eq!(
            session.state().task(task).unwrap().state,
            TaskState::Completed
        );
        // The agent saw the tool observation.
        assert_eq!(agent.observations.len(), 1);
        assert_eq!(agent.observations[0].output["sum"], 5);
        // Metrics recorded the call.
        assert_eq!(metrics.get(names::TOOL_CALLS), 1);
        assert_eq!(metrics.get(names::TOOL_FAILURES), 0);
    }

    #[tokio::test]
    async fn run_is_recoverable_via_event_log() {
        let db = Database::open_in_memory().unwrap();
        let clock = FixedClock::new(1);
        let sid;
        let task;
        {
            let mut session = Session::create(&db, &clock, Some("run")).unwrap();
            sid = session.id();
            task = session.create_task("add").unwrap();
            let mut agent = ScriptedAgent::new([
                AgentDecision::CallTool(ToolInvocation::new(
                    "add",
                    serde_json::json!({"a": 1, "b": 1}),
                )),
                AgentDecision::Complete("done".into()),
            ]);
            let reg = registry();
            let engine = RuntimeEngine::new(&reg, Metrics::new(), RuntimeConfig::default());
            engine.run(&mut session, task, &mut agent).await.unwrap();
        }

        // Recover and confirm the completed task is reconstructed from events.
        let recovered = Session::recover(&db, &clock, sid).unwrap();
        assert_eq!(
            recovered.state().task(task).unwrap().state,
            TaskState::Completed
        );
        assert_eq!(recovered.state().tool_calls_completed, 1);
    }

    #[tokio::test]
    async fn unknown_tool_yields_failed_observation_not_abort() {
        let db = Database::open_in_memory().unwrap();
        let clock = FixedClock::new(1);
        let mut session = Session::create(&db, &clock, None).unwrap();
        let task = session.create_task("x").unwrap();

        let mut agent = ScriptedAgent::new([
            AgentDecision::CallTool(ToolInvocation::new("nonexistent", serde_json::json!({}))),
            AgentDecision::Complete("recovered".into()),
        ]);
        let reg = registry();
        let metrics = Metrics::new();
        let engine = RuntimeEngine::new(&reg, metrics.clone(), RuntimeConfig::default());

        let outcome = engine.run(&mut session, task, &mut agent).await.unwrap();
        assert_eq!(outcome, RunOutcome::Completed("recovered".into()));
        // The agent got a failed observation it could react to.
        assert!(!agent.observations[0].ok);
        assert_eq!(metrics.get(names::TOOL_FAILURES), 1);
    }

    #[tokio::test]
    async fn parallel_tools_run_and_feed_back_all_observations() {
        // A model turn with several read-only (Safe) tool calls runs them all
        // and feeds every observation back, each correlated to its call id.
        let db = Database::open_in_memory().unwrap();
        let clock = FixedClock::new(1);
        let mut session = Session::create(&db, &clock, None).unwrap();
        let task = session.create_task("parallel").unwrap();

        let mut agent = ScriptedAgent::new([
            AgentDecision::CallTools(vec![
                ToolInvocation::new("add", serde_json::json!({"a": 1, "b": 2})).with_id("c1"),
                ToolInvocation::new("add", serde_json::json!({"a": 3, "b": 4})).with_id("c2"),
                ToolInvocation::new("add", serde_json::json!({"a": 5, "b": 6})).with_id("c3"),
            ]),
            AgentDecision::Complete("done".into()),
        ]);
        let reg = registry();
        let metrics = Metrics::new();
        let engine = RuntimeEngine::new(&reg, metrics.clone(), RuntimeConfig::default());
        let outcome = engine.run(&mut session, task, &mut agent).await.unwrap();
        assert_eq!(outcome, RunOutcome::Completed("done".into()));

        // All three observations were fed back on the next think, in order,
        // each tagged with its originating call id.
        assert_eq!(agent.observations.len(), 3);
        assert_eq!(agent.observations[0].call_id.as_deref(), Some("c1"));
        assert_eq!(agent.observations[1].call_id.as_deref(), Some("c2"));
        assert_eq!(agent.observations[2].call_id.as_deref(), Some("c3"));
        assert!(agent.observations.iter().all(|o| o.ok));
        assert_eq!(metrics.get(names::TOOL_CALLS), 3);
        // Three request + three completion events recorded for the session.
        assert_eq!(session.state().tool_calls_completed, 3);
    }

    #[tokio::test]
    async fn step_limit_marks_failed() {
        let db = Database::open_in_memory().unwrap();
        let clock = FixedClock::new(1);
        let mut session = Session::create(&db, &clock, None).unwrap();
        let task = session.create_task("loop forever").unwrap();

        // Agent that never completes: always calls the tool.
        struct Forever;
        #[async_trait]
        impl Agent for Forever {
            async fn think(
                &mut self,
                _step: usize,
                _last: &[Observation],
            ) -> Result<AgentDecision> {
                Ok(AgentDecision::CallTool(ToolInvocation::new(
                    "add",
                    serde_json::json!({"a": 0, "b": 0}),
                )))
            }
        }

        let reg = registry();
        let config = RuntimeConfig {
            max_steps: 3,
            ..Default::default()
        };
        let engine = RuntimeEngine::new(&reg, Metrics::new(), config);
        let mut agent = Forever;
        let outcome = engine.run(&mut session, task, &mut agent).await.unwrap();
        assert_eq!(outcome, RunOutcome::StepLimitReached);
        assert_eq!(session.state().task(task).unwrap().state, TaskState::Failed);
    }

    #[tokio::test]
    async fn before_tool_use_hook_can_deny_call() {
        let db = Database::open_in_memory().unwrap();
        let clock = FixedClock::new(1);
        let mut session = Session::create(&db, &clock, None).unwrap();
        let task = session.create_task("blocked").unwrap();

        // Allow-list that does NOT include "add", so the call is vetoed.
        let mut hooks = HookRegistry::new();
        hooks.register(
            HookPoint::BeforeToolUse,
            Arc::new(ToolAllowlistHook::new(["read_file".to_string()])),
        );

        let mut agent = ScriptedAgent::new([
            AgentDecision::CallTool(ToolInvocation::new("add", serde_json::json!({"a":1,"b":1}))),
            AgentDecision::Complete("done anyway".into()),
        ]);
        let reg = registry();
        let metrics = Metrics::new();
        let engine =
            RuntimeEngine::new(&reg, metrics.clone(), RuntimeConfig::default()).with_hooks(&hooks);

        let outcome = engine.run(&mut session, task, &mut agent).await.unwrap();
        assert_eq!(outcome, RunOutcome::Completed("done anyway".into()));
        // The tool was blocked: failed observation, and the real tool never ran
        // (TOOL_CALLS not incremented, only TOOL_FAILURES).
        assert!(!agent.observations[0].ok);
        assert!(agent.observations[0].output["error"]
            .as_str()
            .unwrap()
            .contains("blocked by hook"));
        assert_eq!(metrics.get(names::TOOL_CALLS), 0);
        assert_eq!(metrics.get(names::TOOL_FAILURES), 1);
    }

    #[tokio::test]
    async fn allowed_tool_passes_hook_gate() {
        let db = Database::open_in_memory().unwrap();
        let clock = FixedClock::new(1);
        let mut session = Session::create(&db, &clock, None).unwrap();
        let task = session.create_task("ok").unwrap();

        let mut hooks = HookRegistry::new();
        hooks.register(
            HookPoint::BeforeToolUse,
            Arc::new(ToolAllowlistHook::new(["add".to_string()])),
        );

        let mut agent = ScriptedAgent::new([
            AgentDecision::CallTool(ToolInvocation::new("add", serde_json::json!({"a":2,"b":2}))),
            AgentDecision::Complete("4".into()),
        ]);
        let reg = registry();
        let metrics = Metrics::new();
        let engine =
            RuntimeEngine::new(&reg, metrics.clone(), RuntimeConfig::default()).with_hooks(&hooks);

        engine.run(&mut session, task, &mut agent).await.unwrap();
        assert!(agent.observations[0].ok);
        assert_eq!(agent.observations[0].output["sum"], 4);
        assert_eq!(metrics.get(names::TOOL_CALLS), 1);
    }

    #[tokio::test]
    async fn verification_passes_accepts_completion() {
        use deepagent_verification::{Command, MockRunner, VerificationStep};

        let db = Database::open_in_memory().unwrap();
        let clock = FixedClock::new(1);
        let mut session = Session::create(&db, &clock, None).unwrap();
        let task = session.create_task("ship it").unwrap();

        let runner = Arc::new(MockRunner::new().with_success("cargo build"));
        let plan = VerificationPlan::new(
            vec![VerificationStep::build(Command::parse("cargo build"))],
            runner,
        );

        let mut agent = ScriptedAgent::new([AgentDecision::Complete("done".into())]);
        let reg = registry();
        let engine = RuntimeEngine::new(&reg, Metrics::new(), RuntimeConfig::default())
            .with_verification(&plan);

        let outcome = engine.run(&mut session, task, &mut agent).await.unwrap();
        assert_eq!(outcome, RunOutcome::Completed("done".into()));
        assert_eq!(
            session.state().task(task).unwrap().state,
            TaskState::Completed
        );
    }

    #[tokio::test]
    async fn verification_failure_feeds_reflection_then_heals() {
        use deepagent_verification::{Command, CommandOutput, CommandRunner, VerificationStep};
        use std::sync::atomic::{AtomicUsize, Ordering};

        // Runner that fails the first verification, passes the second.
        struct HealRunner {
            calls: AtomicUsize,
        }
        #[async_trait]
        impl CommandRunner for HealRunner {
            async fn run(&self, _c: &Command) -> Result<CommandOutput> {
                let n = self.calls.fetch_add(1, Ordering::SeqCst);
                Ok(if n == 0 {
                    CommandOutput {
                        exit_code: Some(1),
                        stdout: String::new(),
                        stderr: "error[E0001]: broken".into(),
                    }
                } else {
                    CommandOutput {
                        exit_code: Some(0),
                        stdout: String::new(),
                        stderr: String::new(),
                    }
                })
            }
        }

        let db = Database::open_in_memory().unwrap();
        let clock = FixedClock::new(1);
        let mut session = Session::create(&db, &clock, None).unwrap();
        let task = session.create_task("fix and ship").unwrap();

        let plan = VerificationPlan::new(
            vec![VerificationStep::build(Command::parse("cargo build"))],
            Arc::new(HealRunner {
                calls: AtomicUsize::new(0),
            }),
        );

        // Agent completes, gets a verification-failed observation, "fixes", and
        // completes again (second verification passes).
        let mut agent = ScriptedAgent::new([
            AgentDecision::Complete("first attempt".into()),
            AgentDecision::Complete("fixed it".into()),
        ]);
        let reg = registry();
        let metrics = Metrics::new();
        let engine = RuntimeEngine::new(&reg, metrics.clone(), RuntimeConfig::default())
            .with_verification(&plan);

        let outcome = engine.run(&mut session, task, &mut agent).await.unwrap();
        assert_eq!(outcome, RunOutcome::Completed("fixed it".into()));
        // The agent saw the verification failure as an observation.
        assert_eq!(agent.observations.len(), 1);
        assert!(!agent.observations[0].ok);
        assert_eq!(agent.observations[0].output["verification_failed"], true);
        // A retry was recorded.
        assert_eq!(metrics.get(names::RETRIES), 1);
    }

    #[tokio::test]
    async fn verification_loop_detection_stops_retrying() {
        use deepagent_verification::{Command, MockRunner, VerificationStep};

        let db = Database::open_in_memory().unwrap();
        let clock = FixedClock::new(1);
        let mut session = Session::create(&db, &clock, None).unwrap();
        let task = session.create_task("never passes").unwrap();

        // Always fails with the same signature.
        let runner =
            Arc::new(MockRunner::new().with_failure("cargo build", "error[E0001]: same failure"));
        let mut plan = VerificationPlan::new(
            vec![VerificationStep::build(Command::parse("cargo build"))],
            runner,
        );
        plan.max_repeats = 1; // give up quickly

        // Agent keeps "completing"; verification keeps failing.
        let mut agent = ScriptedAgent::new([
            AgentDecision::Complete("try 1".into()),
            AgentDecision::Complete("try 2".into()),
            AgentDecision::Complete("try 3".into()),
            AgentDecision::Complete("try 4".into()),
        ]);
        let reg = registry();
        let engine = RuntimeEngine::new(&reg, Metrics::new(), RuntimeConfig::default())
            .with_verification(&plan);

        let outcome = engine.run(&mut session, task, &mut agent).await.unwrap();
        // Eventually loop detection makes it accept completion (GaveUp path)
        // rather than spinning forever.
        assert!(matches!(outcome, RunOutcome::Completed(_)));
        assert_eq!(
            session.state().task(task).unwrap().state,
            TaskState::Completed
        );
    }

    // --- UserPromptSubmit gate (Prompt Submission layer) ---

    /// A hook that rewrites any prompt to a fixed string (Modify), or denies /
    /// asks based on a sentinel substring.
    struct PromptHook;

    #[async_trait]
    impl deepagent_hooks::Hook for PromptHook {
        fn name(&self) -> &str {
            "prompt_hook"
        }
        async fn run(&self, ctx: &HookContext) -> Result<HookOutcome> {
            if let HookData::Prompt { text } = &ctx.data {
                if text.contains("BLOCK") {
                    return Ok(HookOutcome::deny("prompt contains blocked content"));
                }
                if text.contains("CONFIRM") {
                    return Ok(HookOutcome::ask("please confirm this prompt"));
                }
                if text.contains("AUGMENT") {
                    return Ok(HookOutcome::modify(serde_json::json!({
                        "text": format!("{text}\n\n[context added by hook]")
                    })));
                }
            }
            Ok(HookOutcome::Continue)
        }
    }

    fn prompt_engine<'a>(
        reg: &'a ToolRegistry,
        hooks: &'a HookRegistry,
    ) -> RuntimeEngine<'a, FixedClock> {
        RuntimeEngine::new(reg, Metrics::new(), RuntimeConfig::default()).with_hooks(hooks)
    }

    #[tokio::test]
    async fn submit_prompt_accepts_when_no_hooks() {
        let reg = registry();
        let engine =
            RuntimeEngine::<FixedClock>::new(&reg, Metrics::new(), RuntimeConfig::default());
        let decision = engine
            .submit_prompt(deepagent_core::id::SessionId::nil(), "hello")
            .await
            .unwrap();
        assert_eq!(decision, PromptDecision::Accept("hello".into()));
    }

    #[tokio::test]
    async fn submit_prompt_denied_by_hook() {
        let reg = registry();
        let mut hooks = HookRegistry::new();
        hooks.register(HookPoint::UserPromptSubmit, Arc::new(PromptHook));
        let engine = prompt_engine(&reg, &hooks);
        let decision = engine
            .submit_prompt(deepagent_core::id::SessionId::nil(), "please BLOCK this")
            .await
            .unwrap();
        assert!(matches!(decision, PromptDecision::Rejected { .. }));
    }

    #[tokio::test]
    async fn submit_prompt_modified_by_hook() {
        let reg = registry();
        let mut hooks = HookRegistry::new();
        hooks.register(HookPoint::UserPromptSubmit, Arc::new(PromptHook));
        let engine = prompt_engine(&reg, &hooks);
        let decision = engine
            .submit_prompt(deepagent_core::id::SessionId::nil(), "AUGMENT my prompt")
            .await
            .unwrap();
        let prompt = decision.accepted().expect("accepted");
        assert!(prompt.contains("[context added by hook]"));
    }

    #[tokio::test]
    async fn submit_prompt_asks_for_approval() {
        let reg = registry();
        let mut hooks = HookRegistry::new();
        hooks.register(HookPoint::UserPromptSubmit, Arc::new(PromptHook));
        let engine = prompt_engine(&reg, &hooks);
        let decision = engine
            .submit_prompt(deepagent_core::id::SessionId::nil(), "CONFIRM please")
            .await
            .unwrap();
        assert!(matches!(decision, PromptDecision::NeedsApproval { .. }));
    }

    // --- Live event stream (P1-C) ---

    #[tokio::test]
    async fn run_emits_live_event_stream() {
        use crate::events::{ChannelSink, RuntimeEvent};

        let db = Database::open_in_memory().unwrap();
        let clock = FixedClock::new(1);
        let mut session = Session::create(&db, &clock, Some("run")).unwrap();
        let task = session.create_task("add numbers").unwrap();

        let mut agent = ScriptedAgent::new([
            AgentDecision::CallTool(ToolInvocation::new(
                "add",
                serde_json::json!({"a": 2, "b": 3}),
            )),
            AgentDecision::Complete("the sum is 5".into()),
        ]);

        let reg = registry();
        let (sink, mut rx) = ChannelSink::new();
        let engine = RuntimeEngine::new(&reg, Metrics::new(), RuntimeConfig::default())
            .with_events(Arc::new(sink));

        engine.run(&mut session, task, &mut agent).await.unwrap();

        // Drain the stream and assert the phase sequence.
        let mut labels = Vec::new();
        let mut tool_started_args = None;
        let mut final_msg = None;
        while let Ok(ev) = rx.try_recv() {
            labels.push(ev.label().to_string());
            match ev {
                RuntimeEvent::ToolStarted {
                    name, arguments, ..
                } => {
                    assert_eq!(name, "add");
                    tool_started_args = Some(arguments);
                }
                RuntimeEvent::ToolCompleted { ok, output, .. } => {
                    assert!(ok);
                    assert_eq!(output["sum"], 5);
                }
                RuntimeEvent::RunCompleted { message } => final_msg = Some(message),
                _ => {}
            }
        }

        assert_eq!(labels.first().map(String::as_str), Some("run_started"));
        assert_eq!(labels.last().map(String::as_str), Some("run_completed"));
        assert!(labels.iter().any(|l| l == "turn_started"));
        assert!(labels.iter().any(|l| l == "tool_started"));
        assert!(labels.iter().any(|l| l == "tool_completed"));
        assert_eq!(tool_started_args, Some(serde_json::json!({"a": 2, "b": 3})));
        assert_eq!(final_msg.as_deref(), Some("the sum is 5"));
    }

    #[tokio::test]
    async fn blocked_tool_emits_tool_blocked_event() {
        use crate::events::{ChannelSink, RuntimeEvent};

        let db = Database::open_in_memory().unwrap();
        let clock = FixedClock::new(1);
        let mut session = Session::create(&db, &clock, None).unwrap();
        let task = session.create_task("blocked").unwrap();

        let mut hooks = HookRegistry::new();
        hooks.register(
            HookPoint::BeforeToolUse,
            Arc::new(ToolAllowlistHook::new(["read_file".to_string()])),
        );

        let mut agent = ScriptedAgent::new([
            AgentDecision::CallTool(ToolInvocation::new("add", serde_json::json!({"a":1,"b":1}))),
            AgentDecision::Complete("done".into()),
        ]);
        let reg = registry();
        let (sink, mut rx) = ChannelSink::new();
        let engine = RuntimeEngine::new(&reg, Metrics::new(), RuntimeConfig::default())
            .with_hooks(&hooks)
            .with_events(Arc::new(sink));

        engine.run(&mut session, task, &mut agent).await.unwrap();

        let mut saw_blocked = false;
        while let Ok(ev) = rx.try_recv() {
            if let RuntimeEvent::ToolBlocked {
                name,
                needs_approval,
                ..
            } = ev
            {
                assert_eq!(name, "add");
                assert!(!needs_approval); // a hard deny, not an ask
                saw_blocked = true;
            }
        }
        assert!(saw_blocked, "expected a ToolBlocked event");
    }

    // --- Approval gate (Phase A-3 human-in-the-loop) ---

    /// A hook that asks for approval on the `add` tool.
    struct AskHook;
    #[async_trait]
    impl deepagent_hooks::Hook for AskHook {
        fn name(&self) -> &str {
            "ask_hook"
        }
        async fn run(&self, ctx: &HookContext) -> Result<HookOutcome> {
            if let HookData::Tool { name, .. } = &ctx.data {
                if name == "add" {
                    return Ok(HookOutcome::ask("needs human approval"));
                }
            }
            Ok(HookOutcome::Continue)
        }
    }

    /// A gate returning a fixed decision and recording the tools it saw.
    struct FixedGate {
        decision: crate::approval::ApprovalDecision,
        seen: Arc<std::sync::Mutex<Vec<String>>>,
    }
    #[async_trait]
    impl crate::approval::ApprovalGate for FixedGate {
        async fn request(
            &self,
            req: crate::approval::ApprovalRequest,
        ) -> crate::approval::ApprovalDecision {
            self.seen.lock().unwrap().push(req.tool.clone());
            self.decision
        }
    }

    #[tokio::test]
    async fn ask_hook_consults_gate_and_allows() {
        let db = Database::open_in_memory().unwrap();
        let clock = FixedClock::new(1);
        let mut session = Session::create(&db, &clock, None).unwrap();
        let task = session.create_task("ask").unwrap();

        let mut hooks = HookRegistry::new();
        hooks.register(HookPoint::BeforeToolUse, Arc::new(AskHook));

        let mut agent = ScriptedAgent::new([
            AgentDecision::CallTool(ToolInvocation::new("add", serde_json::json!({"a":2,"b":2}))),
            AgentDecision::Complete("4".into()),
        ]);
        let reg = registry();
        let metrics = Metrics::new();
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let gate = Arc::new(FixedGate {
            decision: crate::approval::ApprovalDecision::Allow,
            seen: seen.clone(),
        });
        let engine = RuntimeEngine::new(&reg, metrics.clone(), RuntimeConfig::default())
            .with_hooks(&hooks)
            .with_approvals(gate);

        engine.run(&mut session, task, &mut agent).await.unwrap();
        // Gate was consulted for "add", and the call then ran successfully.
        assert_eq!(*seen.lock().unwrap(), vec!["add".to_string()]);
        assert!(agent.observations[0].ok);
        assert_eq!(metrics.get(names::TOOL_CALLS), 1);
    }

    #[tokio::test]
    async fn ask_hook_gate_denies_blocks_call() {
        let db = Database::open_in_memory().unwrap();
        let clock = FixedClock::new(1);
        let mut session = Session::create(&db, &clock, None).unwrap();
        let task = session.create_task("ask").unwrap();

        let mut hooks = HookRegistry::new();
        hooks.register(HookPoint::BeforeToolUse, Arc::new(AskHook));

        let mut agent = ScriptedAgent::new([
            AgentDecision::CallTool(ToolInvocation::new("add", serde_json::json!({"a":2,"b":2}))),
            AgentDecision::Complete("done anyway".into()),
        ]);
        let reg = registry();
        let metrics = Metrics::new();
        let gate = Arc::new(FixedGate {
            decision: crate::approval::ApprovalDecision::Deny,
            seen: Arc::new(std::sync::Mutex::new(Vec::new())),
        });
        let engine = RuntimeEngine::new(&reg, metrics.clone(), RuntimeConfig::default())
            .with_hooks(&hooks)
            .with_approvals(gate);

        engine.run(&mut session, task, &mut agent).await.unwrap();
        // Denied: failed observation, real tool never ran.
        assert!(!agent.observations[0].ok);
        assert!(agent.observations[0].output["error"]
            .as_str()
            .unwrap()
            .contains("approval denied"));
        assert_eq!(metrics.get(names::TOOL_CALLS), 0);
    }
}
