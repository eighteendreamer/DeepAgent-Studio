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
use deepagent_core::response_item::ResponseOutputItem;
use deepagent_core::task::TaskState;
use deepagent_hooks::{HookContext, HookData, HookOutcome, HookPoint, HookRegistry, ToolBatchItem};
use deepagent_session::Session;
use deepagent_tools::permission::PermissionSet;
use deepagent_tools::{ToolExecutionContext, ToolRegistry};
use deepagent_tracing::metrics::{names, Metrics};
use deepagent_verification::reflection::NextAction;
use deepagent_verification::{CommandRunner, ReflectionEngine, VerificationStep, Verifier};

use crate::agent::{Agent, AgentDecision, Observation, ToolAttemptController};
use crate::approval::{ApprovalDecision, ApprovalGate, ApprovalRequest, AutoDenyGate};
use crate::empty_stub::ensure_non_empty_output;
use crate::events::{tool_ui_metadata, NullEventSink, RuntimeEvent, RuntimeEventSink};
use crate::tool_budget::{
    apply_tool_result_budget, cleanup_tool_result_paths, saved_path, ToolResultBudgetConfig,
};
use crate::tool_result_decorator::ToolResultDecorator;

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
    /// A configured token/cost/task budget was exhausted.
    BudgetExceeded(String),
    /// Factual completion requirements repeatedly failed.
    CompletionFailed(String),
}

fn persist_response_items<C: Clock>(
    session: &mut Session<'_, C>,
    items: Vec<ResponseOutputItem>,
) -> Result<bool> {
    if items.is_empty() {
        return Ok(false);
    }
    for item in items {
        session.append_response_item(item)?;
    }
    Ok(true)
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
    /// Hard wall-clock deadline for the complete kernel run, including model,
    /// hooks, approvals, tools and verification. `None` disables it.
    pub task_timeout: Option<std::time::Duration>,
    /// Maximum cumulative provider-reported tokens for this run. The boundary
    /// is checked after every model response and before any requested tool is
    /// allowed to execute.
    pub max_total_tokens: Option<u64>,
    /// Factual filesystem effects required before a final answer is accepted.
    pub completion_policy: crate::completion::CompletionPolicy,
    /// Number of times completion evidence may be fed back for self-repair.
    pub max_completion_retries: usize,
    /// SessionStart is emitted only for a newly-created session, not every
    /// user turn appended to an existing session.
    pub fire_session_start: bool,
    /// Permissions granted to the agent for this run.
    pub permissions: PermissionSet,
    /// Whether high-risk tools are pre-approved for this run.
    pub auto_approve: bool,
    /// Deadline for one tool invocation. Process-backed tools enforce this
    /// while killing the complete process tree on expiry.
    pub tool_timeout: std::time::Duration,
    /// Incremental file checkpoint for this user turn.
    pub checkpoint: Option<std::sync::Arc<crate::checkpoint::CheckpointManager>>,
    /// Durable index for large tool output artifact files.
    pub artifact_persistence: Option<crate::tool_pipeline::ToolArtifactPersistence>,
    /// Tool result truncation and persistence budget.
    pub tool_result_budget: ToolResultBudgetConfig,
    /// Optional decorator that mutates each tool result after invocation.
    /// Higher-level crates (e.g. `deepagent-app-core`) plug in plan-mode
    /// reminders, todo snapshots, and verification annotations through this
    /// extension point. `None` means "no decoration" — keeps zero overhead
    /// for embed contexts that don't need it.
    pub tool_result_decorator: Option<std::sync::Arc<dyn ToolResultDecorator>>,
    /// Advisory adversarial-verification re-entries per run (§2.2). After this
    /// many refuting verdicts the completion is accepted regardless (从宽:
    /// never trap a run). Default 1.
    pub max_adversarial_retries: usize,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            max_steps: 64,
            task_timeout: Some(std::time::Duration::from_secs(30 * 60)),
            max_total_tokens: Some(1_000_000),
            completion_policy: crate::completion::CompletionPolicy::default(),
            max_completion_retries: 2,
            fire_session_start: true,
            permissions: PermissionSet::developer(),
            auto_approve: false,
            tool_timeout: std::time::Duration::from_secs(120),
            checkpoint: None,
            artifact_persistence: None,
            tool_result_budget: ToolResultBudgetConfig::default(),
            tool_result_decorator: None,
            max_adversarial_retries: 1,
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
    adversarial_verifier: Option<std::sync::Arc<dyn crate::adversarial::AdversarialVerifier>>,
    events: std::sync::Arc<dyn RuntimeEventSink>,
    approvals: std::sync::Arc<dyn ApprovalGate>,
    cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    created_tool_result_paths: std::sync::Arc<std::sync::Mutex<Vec<std::path::PathBuf>>>,
    _clock: std::marker::PhantomData<&'a C>,
}

/// Attempt-local tool-call cache populated directly from semantic model stream
/// events. Only validation and value normalization happen here. Hooks,
/// permissions, approvals and execution remain in ToolExecutionPipeline after
/// the provider attempt commits.
struct StreamingToolAttempt<'a> {
    registry: &'a ToolRegistry,
    permissions: PermissionSet,
    runtime_cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
    timeout: std::time::Duration,
    speculation_enabled: bool,
    active_attempt: Option<usize>,
    committed_attempt: Option<usize>,
    prepared: std::collections::HashMap<String, StreamPreparedCall>,
}

struct StreamPreparedCall {
    invocation: deepagent_tools::ToolInvocation,
    speculative: Option<SpeculativeToolExecution>,
}

struct SpeculativeToolExecution {
    cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
    handle: tokio::task::JoinHandle<(Result<deepagent_tools::ToolOutput>, u64)>,
}

impl<'a> StreamingToolAttempt<'a> {
    fn new(
        registry: &'a ToolRegistry,
        permissions: PermissionSet,
        runtime_cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
        timeout: std::time::Duration,
        speculation_enabled: bool,
    ) -> Self {
        Self {
            registry,
            permissions,
            runtime_cancel,
            timeout,
            speculation_enabled,
            active_attempt: None,
            committed_attempt: None,
            prepared: std::collections::HashMap::new(),
        }
    }

    fn normalize_decision(&self, decision: AgentDecision) -> AgentDecision {
        if self.committed_attempt != self.active_attempt {
            return decision;
        }
        match decision {
            AgentDecision::CallTool(invocation) => {
                AgentDecision::CallTool(self.normalized(invocation))
            }
            AgentDecision::CallTools(invocations) => AgentDecision::CallTools(
                invocations
                    .into_iter()
                    .map(|invocation| self.normalized(invocation))
                    .collect(),
            ),
            other => other,
        }
    }

    fn normalized(
        &self,
        invocation: deepagent_tools::ToolInvocation,
    ) -> deepagent_tools::ToolInvocation {
        let Some(call_id) = invocation.id.as_deref() else {
            return invocation;
        };
        self.prepared
            .get(call_id)
            .map(|prepared| &prepared.invocation)
            .filter(|prepared| prepared.name == invocation.name)
            .cloned()
            .unwrap_or(invocation)
    }

    fn cancel_speculation(&mut self) {
        for prepared in self.prepared.values_mut() {
            if let Some(speculative) = prepared.speculative.take() {
                speculative
                    .cancel
                    .store(true, std::sync::atomic::Ordering::Release);
                speculative.handle.abort();
            }
        }
    }

    fn take_speculative(
        &mut self,
        invocation: &deepagent_tools::ToolInvocation,
    ) -> Option<SpeculativeToolExecution> {
        if self.committed_attempt != self.active_attempt {
            return None;
        }
        let call_id = invocation.id.as_deref()?;
        let prepared = self.prepared.get_mut(call_id)?;
        if prepared.invocation.name != invocation.name {
            return None;
        }
        prepared.speculative.take()
    }

    fn take_speculative_batch(
        &mut self,
        invocations: &[deepagent_tools::ToolInvocation],
    ) -> std::collections::HashMap<String, SpeculativeToolExecution> {
        invocations
            .iter()
            .filter_map(|invocation| {
                let call_id = invocation.id.clone()?;
                self.take_speculative(invocation)
                    .map(|speculative| (call_id, speculative))
            })
            .collect()
    }
}

impl Drop for StreamingToolAttempt<'_> {
    fn drop(&mut self) {
        self.cancel_speculation();
    }
}

impl ToolAttemptController for StreamingToolAttempt<'_> {
    fn begin(&mut self, attempt: usize) {
        self.cancel_speculation();
        self.active_attempt = Some(attempt);
        self.committed_attempt = None;
        self.prepared.clear();
    }

    fn prepare(&mut self, invocation: deepagent_tools::ToolInvocation) {
        let Some(call_id) = invocation.id.clone() else {
            return;
        };
        // Invalid calls intentionally remain uncached. The canonical pipeline
        // will turn them into a paired validation observation after commit.
        let Ok(validated) = self
            .registry
            .validate_invocation(&invocation.name, invocation.arguments)
        else {
            return;
        };
        let normalized = deepagent_tools::ToolInvocation::new(validated.name, validated.arguments)
            .with_id(call_id.clone());
        let speculative = if self.speculation_enabled
            && !self
                .runtime_cancel
                .load(std::sync::atomic::Ordering::Acquire)
        {
            self.registry.get(&normalized.name).and_then(|spec| {
                let safe = spec.descriptor.risk == deepagent_tools::permission::RiskLevel::Safe
                    && spec.tool.is_concurrency_safe(&normalized.arguments)
                    && self
                        .registry
                        .check(&normalized.name, &self.permissions, true)
                        .is_ok();
                if !safe {
                    return None;
                }
                let registry = self.registry.clone();
                let permissions = self.permissions.clone();
                let name = normalized.name.clone();
                let arguments = normalized.arguments.clone();
                let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
                let execution_cancel = cancel.clone();
                let timeout = self.timeout;
                let handle = tokio::spawn(async move {
                    let started = std::time::Instant::now();
                    let result = registry
                        .invoke_with_context(
                            &name,
                            arguments,
                            &permissions,
                            true,
                            deepagent_tools::ToolExecutionContext::new(execution_cancel)
                                .with_timeout(timeout),
                        )
                        .await;
                    (result, started.elapsed().as_millis() as u64)
                });
                Some(SpeculativeToolExecution { cancel, handle })
            })
        } else {
            None
        };
        self.prepared.insert(
            call_id,
            StreamPreparedCall {
                invocation: normalized,
                speculative,
            },
        );
    }

    fn commit(&mut self, attempt: usize) {
        if self.active_attempt == Some(attempt) {
            self.committed_attempt = Some(attempt);
        }
    }

    fn abort(&mut self, attempt: usize, _reason: &str) {
        if self.active_attempt == Some(attempt) {
            self.cancel_speculation();
            self.prepared.clear();
            self.committed_attempt = None;
        }
    }
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
            adversarial_verifier: None,
            events: std::sync::Arc::new(NullEventSink),
            approvals: std::sync::Arc::new(AutoDenyGate),
            cancel: None,
            created_tool_result_paths: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
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

    /// Attach an advisory adversarial goal verifier (§2.2). When set, a
    /// completed run that mutated files is judged once by the skeptic panel;
    /// a refuting verdict feeds its gaps back as an observation (bounded by
    /// [`RuntimeConfig::max_adversarial_retries`]). Advisory only — never
    /// hard-fails a run.
    pub fn with_adversarial_verifier(
        mut self,
        verifier: std::sync::Arc<dyn crate::adversarial::AdversarialVerifier>,
    ) -> Self {
        self.adversarial_verifier = Some(verifier);
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
                if let Some(cancel) = self.cancel.clone() {
                    tokio::select! {
                        result = reg.dispatch(&ctx) => result,
                        _ = wait_for_runtime_cancel(cancel) => Err(
                            deepagent_core::error::CoreError::other(
                                format!("hook '{}' cancelled by user", point.label())
                            )
                        ),
                    }
                } else {
                    reg.dispatch(&ctx).await
                }
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
        if self.config.fire_session_start {
            self.fire_hook(session_id, HookPoint::SessionStart, HookData::None)
                .await?;
        }

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
        self.fire_task_hook(session, session_id, task, HookPoint::TaskCreated)
            .await?;

        // Move the task into Running (validated by the session).
        if session.state().task(task).map(|t| t.state) == Some(TaskState::Queued) {
            session.transition_task(task, TaskState::Running)?;
        }

        let mut last_observations: Vec<Observation> = Vec::new();
        let mut outcome = RunOutcome::StepLimitReached;
        let mut finished = false;
        let mut completion_failures = 0usize;
        let mut adversarial_refutes = 0usize;
        let mut raw_responses_usage: Vec<serde_json::Value> = Vec::new();

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
            // BeforePlan hook (observational unless denied): fires before the
            // model decides its next move for this step. A Deny ends the run
            // cleanly instead of silently continuing — the hook is vetoable.
            if let HookOutcome::Deny { reason, .. } = self
                .fire_hook(session_id, HookPoint::BeforePlan, HookData::None)
                .await?
            {
                session.transition_task(task, TaskState::Failed)?;
                let reason = format!("run blocked by BeforePlan hook: {reason}");
                self.emit(RuntimeEvent::RunFailed {
                    reason: reason.clone(),
                });
                outcome = RunOutcome::CompletionFailed(reason);
                finished = true;
                break;
            }
            let runtime_cancel = self
                .cancel
                .clone()
                .unwrap_or_else(|| std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)));
            let mut streaming_tools = StreamingToolAttempt::new(
                self.registry,
                self.config.permissions.clone(),
                runtime_cancel,
                self.config.tool_timeout,
                self.hooks.is_none(),
            );
            let decision = match agent
                .think_streaming_cancelled(
                    step,
                    &last_observations,
                    self.cancel.clone(),
                    Some(&mut streaming_tools),
                )
                .await
            {
                Ok(decision) => streaming_tools.normalize_decision(decision),
                Err(_e) if self.is_cancelled() => {
                    session.transition_task(task, TaskState::Failed)?;
                    self.emit(RuntimeEvent::RunCancelled);
                    outcome = RunOutcome::Cancelled;
                    finished = true;
                    break;
                }
                Err(e) => return Err(e),
            };
            if self.is_cancelled() {
                session.transition_task(task, TaskState::Failed)?;
                self.emit(RuntimeEvent::RunCancelled);
                outcome = RunOutcome::Cancelled;
                finished = true;
                break;
            }
            if let (Some(limit), Some(usage)) =
                (self.config.max_total_tokens, agent.cumulative_usage())
            {
                if usage.total_tokens as u64 > limit {
                    let reason = format!(
                        "run token budget exceeded: used {} tokens, limit {limit}",
                        usage.total_tokens
                    );
                    session.transition_task(task, TaskState::Failed)?;
                    self.emit(RuntimeEvent::RunFailed {
                        reason: reason.clone(),
                    });
                    outcome = RunOutcome::BudgetExceeded(reason);
                    finished = true;
                    break;
                }
            }
            tracing::debug!(step, ?decision, "agent decision");
            let mut provider_items_persisted =
                persist_response_items(session, agent.take_pending_response_items())?;
            raw_responses_usage.extend(agent.take_pending_raw_usage());
            let decision = match decision {
                AgentDecision::CompleteItems { message, items } => {
                    provider_items_persisted =
                        persist_response_items(session, items)? || provider_items_persisted;
                    AgentDecision::CompleteMessage(message)
                }
                other => other,
            };

            match decision {
                AgentDecision::Complete(msg) => {
                    let mut content = msg;
                    // Post-completion verification / self-healing — only when
                    // the run actually mutated files (fact-based gate).
                    if let (Some(plan), Some(engine)) =
                        (self.verification, reflection_engine.as_mut())
                    {
                        if self.run_mutated_files() {
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
                    }

                    if let Some(observation) = self.completion_evidence_feedback()? {
                        completion_failures += 1;
                        if completion_failures <= self.config.max_completion_retries {
                            last_observations = vec![observation];
                            continue;
                        }
                        // Upstream parity (grok stop_gate.rs: continuation cap
                        // => AllowStop; Claude Code stop hooks never fail the
                        // run): once the retry budget is exhausted this gate
                        // FAILS OPEN — record the unmet requirement and accept
                        // the completion. The extractor is heuristic; killing
                        // a run whose work may be correct is worse than
                        // letting one slip through.
                        let reason = completion_failure_reason(&observation);
                        session.append(EventPayload::Note {
                            text: format!(
                                "completion gate failed open after {completion_failures} attempts: {reason}"
                            ),
                        })?;
                        self.emit(RuntimeEvent::Verification {
                            passed: false,
                            detail: format!(
                                "completion gate exhausted its retry budget and failed open: {reason}"
                            ),
                        });
                    }
                    match self.completion_gate(session_id, content).await? {
                        CompletionDecision::Accept(updated) => content = updated,
                        CompletionDecision::Retry(observation) => {
                            last_observations = vec![observation];
                            continue;
                        }
                    }

                    let message = Message::assistant(&content);
                    if provider_items_persisted {
                        session.append_without_response_projection(
                            EventPayload::MessageAppended { message },
                        )?;
                    } else {
                        session.append(EventPayload::MessageAppended { message })?;
                    }
                    session.transition_task(task, TaskState::Completed)?;
                    self.emit(RuntimeEvent::RunCompleted {
                        message: content.clone(),
                    });
                    outcome = RunOutcome::Completed(content);
                    finished = true;
                    break;
                }

                AgentDecision::CompleteMessage(message) => {
                    let mut message = message;
                    let mut content = message.content.clone();
                    // Post-completion verification / self-healing — only when
                    // the run actually mutated files (fact-based gate).
                    if let (Some(plan), Some(engine)) =
                        (self.verification, reflection_engine.as_mut())
                    {
                        if self.run_mutated_files() {
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
                    }

                    // Advisory adversarial goal verification (§2.2, LLM 辅):
                    // after the fact-gate accepts, an opt-in skeptic panel
                    // judges whether the change actually met the goal. A
                    // refuting verdict feeds its gaps back ONCE per budget
                    // slot; it can never hard-fail the run (从宽 — the
                    // CompletionGate 连环误杀 lesson). Fact-gated on file
                    // mutation, same as verify_after_completion.
                    if let Some(verifier) = self.adversarial_verifier.clone() {
                        if self.run_mutated_files()
                            && adversarial_refutes < self.config.max_adversarial_retries
                        {
                            let changed = self.changed_file_paths();
                            match verifier.verify(&content, &changed).await {
                                crate::adversarial::AdversarialVerdict::Accepted => {
                                    self.emit(RuntimeEvent::Verification {
                                        passed: true,
                                        detail: "adversarial panel accepted the completion"
                                            .to_string(),
                                    });
                                }
                                crate::adversarial::AdversarialVerdict::Refuted { gaps } => {
                                    adversarial_refutes += 1;
                                    let detail = if gaps.is_empty() {
                                        "adversarial panel refuted the completion".to_string()
                                    } else {
                                        format!(
                                            "adversarial panel refuted the completion: {}",
                                            gaps.join("; ")
                                        )
                                    };
                                    tracing::warn!(refute = adversarial_refutes, "{detail}");
                                    self.emit(RuntimeEvent::Verification {
                                        passed: false,
                                        detail: detail.clone(),
                                    });
                                    last_observations = vec![Observation::new(
                                        "adversarial_verification",
                                        false,
                                        serde_json::json!({
                                            "advisory": true,
                                            "gaps": gaps,
                                            "guidance": "An independent verification panel could \
                                                not confirm the goal was met (see gaps). Address \
                                                them with concrete tool calls, or if you believe \
                                                the work is complete, restate the completion with \
                                                the specific evidence already in the transcript."
                                        }),
                                    )];
                                    continue;
                                }
                            }
                        }
                    }

                    if let Some(observation) = self.completion_evidence_feedback()? {
                        completion_failures += 1;
                        if completion_failures <= self.config.max_completion_retries {
                            last_observations = vec![observation];
                            continue;
                        }
                        // Same fail-open semantics as the Complete arm above.
                        let reason = completion_failure_reason(&observation);
                        session.append(EventPayload::Note {
                            text: format!(
                                "completion gate failed open after {completion_failures} attempts: {reason}"
                            ),
                        })?;
                        self.emit(RuntimeEvent::Verification {
                            passed: false,
                            detail: format!(
                                "completion gate exhausted its retry budget and failed open: {reason}"
                            ),
                        });
                    }
                    match self.completion_gate(session_id, content).await? {
                        CompletionDecision::Accept(updated) => {
                            content = updated;
                            message.content = content.clone();
                        }
                        CompletionDecision::Retry(observation) => {
                            last_observations = vec![observation];
                            continue;
                        }
                    }

                    if provider_items_persisted {
                        session.append_without_response_projection(
                            EventPayload::MessageAppended { message },
                        )?;
                    } else {
                        session.append(EventPayload::MessageAppended { message })?;
                    }
                    session.transition_task(task, TaskState::Completed)?;
                    self.emit(RuntimeEvent::RunCompleted {
                        message: content.clone(),
                    });
                    outcome = RunOutcome::Completed(content);
                    finished = true;
                    break;
                }

                AgentDecision::CompleteItems { .. } => {
                    unreachable!("CompleteItems is normalized before decision dispatch")
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
                    let speculative = streaming_tools.take_speculative(&invocation);
                    let observation = match speculative {
                        Some(speculative) => {
                            self.execute_tool_with_speculative(
                                session,
                                session_id,
                                invocation,
                                Some(speculative),
                                provider_items_persisted,
                            )
                            .await?
                        }
                        None => {
                            self.execute_tool(
                                session,
                                session_id,
                                invocation,
                                provider_items_persisted,
                            )
                            .await?
                        }
                    };
                    last_observations = vec![observation];
                }

                AgentDecision::CallTools(invocations) => {
                    let speculative = streaming_tools.take_speculative_batch(&invocations);
                    last_observations = self
                        .execute_tools(
                            session,
                            session_id,
                            invocations,
                            speculative,
                            provider_items_persisted,
                        )
                        .await?;
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
        self.fire_task_hook(session, session_id, task, HookPoint::TaskCompleted)
            .await?;

        // Persist the run's cumulative token usage + wall-clock duration so the
        // UI can show per-turn metrics when the session is reopened later.
        if let Some(u) = agent.cumulative_usage() {
            let duration_ms = run_started_at.elapsed().as_millis() as u64;
            session.append(EventPayload::UsageRecorded {
                prompt_tokens: u.prompt_tokens,
                completion_tokens: u.completion_tokens,
                reasoning_tokens: u.reasoning_tokens,
                total_tokens: u.total_tokens,
                prompt_cache_hit_tokens: u.prompt_cache_hit_tokens,
                prompt_cache_miss_tokens: u.prompt_cache_miss_tokens,
                duration_ms,
                raw_responses_usage: (!raw_responses_usage.is_empty())
                    .then_some(serde_json::Value::Array(raw_responses_usage)),
            })?;
        }

        if self.config.tool_result_budget.cleanup_on_run_end {
            let paths = {
                let mut guard = self
                    .created_tool_result_paths
                    .lock()
                    .unwrap_or_else(|p| p.into_inner());
                std::mem::take(&mut *guard)
            };
            cleanup_tool_result_paths(paths).await;
        }
        self.fire_hook(session_id, HookPoint::SessionEnd, HookData::None)
            .await?;

        Ok(outcome)
    }

    async fn fire_task_hook(
        &self,
        session: &Session<'a, C>,
        session_id: deepagent_core::id::SessionId,
        task: TaskId,
        point: HookPoint,
    ) -> Result<()> {
        let subject = session
            .state()
            .task(task)
            .map(|task| task.goal.clone())
            .unwrap_or_else(|| task.to_string());
        self.fire_hook(
            session_id,
            point,
            HookData::Task {
                task_id: task.to_string(),
                subject,
            },
        )
        .await?;
        Ok(())
    }

    fn remember_tool_result_path(&self, output: &deepagent_tools::ToolOutput) {
        if let Some(path) = saved_path(output) {
            self.created_tool_result_paths
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .push(path);
        }
    }

    /// Whether this run actually created or modified files. The build/type
    /// verification plan is gated on this FACT (not on prompt keywords):
    /// aligns with Claude Code running verification only when non-trivial
    /// implementation happened. Tests without a checkpoint keep the old
    /// unconditional behaviour.
    fn run_mutated_files(&self) -> bool {
        match self.config.checkpoint.as_ref() {
            Some(checkpoint) => checkpoint
                .mutation_evidence()
                .map(|mutations| {
                    mutations.iter().any(|m| {
                        matches!(
                            m.kind,
                            crate::checkpoint::MutationKind::Created
                                | crate::checkpoint::MutationKind::Modified
                        )
                    })
                })
                .unwrap_or(false),
            None => true,
        }
    }

    /// Created/modified file paths this run, for the adversarial panel's
    /// mutation evidence (§2.2). Empty when no checkpoint tracks mutations.
    fn changed_file_paths(&self) -> Vec<String> {
        let Some(checkpoint) = self.config.checkpoint.as_ref() else {
            return Vec::new();
        };
        checkpoint
            .mutation_evidence()
            .map(|mutations| {
                mutations
                    .iter()
                    .filter(|m| {
                        matches!(
                            m.kind,
                            crate::checkpoint::MutationKind::Created
                                | crate::checkpoint::MutationKind::Modified
                        )
                    })
                    .map(|m| m.path.display().to_string())
                    .collect()
            })
            .unwrap_or_default()
    }

    fn completion_evidence_feedback(&self) -> Result<Option<Observation>> {
        let mutations = match self.config.checkpoint.as_ref() {
            Some(checkpoint) => checkpoint.mutation_evidence()?,
            None => Vec::new(),
        };
        if self.config.checkpoint.is_some() || !self.config.completion_policy.is_empty() {
            self.emit(RuntimeEvent::CompletionEvidence {
                mutations: mutations.clone(),
            });
        }
        match self.config.completion_policy.validate(&mutations) {
            Ok(()) => Ok(None),
            Err(failure) => {
                self.emit(RuntimeEvent::Verification {
                    passed: false,
                    detail: failure.reason.clone(),
                });
                Ok(Some(Observation {
                    tool: "completion_gate".to_string(),
                    ok: false,
                    output: serde_json::json!({
                        "completion_blocked": true,
                        "reason": failure.reason,
                        "required_effects": failure.required_effects,
                        "mutations": mutations,
                        "recovery_hint": "Perform and verify the requested filesystem operation. Prefer write_file/edit_file/delete_path/move_path over shell commands, then provide the final response.",
                    }),
                    call_id: None,
                }))
            }
        }
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
        mut invocations: Vec<deepagent_tools::ToolInvocation>,
        mut speculative: std::collections::HashMap<String, SpeculativeToolExecution>,
        provider_tool_calls_persisted: bool,
    ) -> Result<Vec<Observation>> {
        if invocations.len() <= 1 {
            let mut out = Vec::with_capacity(invocations.len());
            for inv in invocations {
                let early = inv
                    .id
                    .as_ref()
                    .and_then(|call_id| speculative.remove(call_id));
                out.push(match early {
                    Some(early) => {
                        self.execute_tool_with_speculative(
                            session,
                            session_id,
                            inv,
                            Some(early),
                            provider_tool_calls_persisted,
                        )
                        .await?
                    }
                    None => {
                        self.execute_tool(session, session_id, inv, provider_tool_calls_persisted)
                            .await?
                    }
                });
            }
            return Ok(out);
        }
        // Partition: read-only/concurrency-safe tools can have their I/O run in
        // parallel; everything else runs sequentially for safety + ordering.
        // Validate and run each candidate's PreToolUse gate before starting
        // any parallel I/O. Outcomes are retained so blocked/ask calls do not
        // execute the same hook twice when they fall back to serial handling.
        let mut preflight: std::collections::HashMap<usize, HookOutcome> =
            std::collections::HashMap::new();
        let mut parallel_idx = Vec::new();
        for (i, invocation) in invocations.iter_mut().enumerate() {
            if !self.is_parallel_safe(&invocation.name, &invocation.arguments) {
                continue;
            }
            let validated = match self
                .registry
                .validate_invocation(&invocation.name, invocation.arguments.clone())
            {
                Ok(validated) => validated,
                Err(_) => continue,
            };
            invocation.arguments = validated.arguments;
            let outcome = self
                .fire_hook(
                    session_id,
                    HookPoint::BeforeToolUse,
                    HookData::before_tool(invocation.name.clone(), invocation.arguments.clone()),
                )
                .await?;
            if let HookOutcome::Modify { updated_input, .. } = &outcome {
                match self
                    .registry
                    .validate_invocation(&invocation.name, updated_input.clone())
                {
                    Ok(validated) => invocation.arguments = validated.arguments,
                    Err(_) => {
                        preflight.insert(i, outcome);
                        continue;
                    }
                }
            }
            if matches!(outcome, HookOutcome::Continue | HookOutcome::Modify { .. }) {
                parallel_idx.push(i);
            }
            preflight.insert(i, outcome);
        }
        let mut batch_inputs = invocations
            .iter()
            .map(|invocation| {
                (
                    invocation.name.clone(),
                    invocation.id.clone(),
                    invocation.arguments.clone(),
                )
            })
            .collect::<Vec<_>>();

        // Fast path: nothing parallelizable → just run them in order.
        if parallel_idx.len() <= 1 {
            let mut out = Vec::with_capacity(invocations.len());
            for inv in invocations {
                let outcome = preflight.remove(&out.len());
                let early = inv
                    .id
                    .as_ref()
                    .and_then(|call_id| speculative.remove(call_id));
                out.push(
                    self.execute_tool_with_before_and_speculative(
                        session,
                        session_id,
                        inv,
                        outcome,
                        early,
                        provider_tool_calls_persisted,
                    )
                    .await?,
                );
            }
            for (index, observation) in out.iter().enumerate() {
                if let Some(input) = batch_inputs.get_mut(index) {
                    input.1 = observation.call_id.clone().or_else(|| input.1.clone());
                }
            }
            if let Some(feedback) = self
                .post_tool_batch_feedback(session_id, &batch_inputs, &out)
                .await?
            {
                out.push(feedback);
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
            let metadata = tool_ui_metadata(&inv.name, &inv.arguments, None);
            self.emit(RuntimeEvent::ToolStarted {
                name: inv.name.clone(),
                call_id: call_id.clone(),
                arguments: inv.arguments.clone(),
                tool_kind: metadata.tool_kind,
                file_path: metadata.file_path,
                summary: metadata.summary,
                meta: metadata.meta,
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
            let metadata_args = args.clone();
            let registry = self.registry;
            let perms = self.config.permissions.clone();
            let auto = self.config.auto_approve;
            let execution_context =
                ToolExecutionContext::new(self.cancel.clone().unwrap_or_else(|| {
                    std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false))
                }))
                .with_timeout(self.config.tool_timeout);
            let budget = self.config.tool_result_budget.clone();
            let session_id_str = session_id.to_string();
            let created_paths = self.created_tool_result_paths.clone();
            let decorator = self.config.tool_result_decorator.clone();
            let early = speculative.remove(&call_id);
            futs.push(async move {
                let invoke = || async {
                    let started = std::time::Instant::now();
                    let result = registry
                        .invoke_with_context(&name, args, &perms, auto, execution_context)
                        .await;
                    (result, started.elapsed().as_millis() as u64)
                };
                let (raw_result, duration_ms) = match early {
                    Some(early) => match early.handle.await {
                        Ok(completed) => completed,
                        Err(_) => invoke().await,
                    },
                    None => invoke().await,
                };
                let result = match raw_result {
                    Ok(out) => {
                        let mut out = apply_tool_result_budget(
                            &budget,
                            &session_id_str,
                            &name,
                            &call_id,
                            out,
                        )
                        .await;
                        ensure_non_empty_output(&mut out, &name);
                        if let Some(decorator) = decorator.as_ref() {
                            decorator.decorate(&name, &mut out).await;
                        }
                        if let Some(path) = saved_path(&out) {
                            created_paths
                                .lock()
                                .unwrap_or_else(|p| p.into_inner())
                                .push(path);
                        }
                        Ok(out)
                    }
                    Err(e) => Err(e),
                };
                (i, call_id, name, metadata_args, result, duration_ms)
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
        while let Some((i, call_id, name, arguments, result, duration_ms)) = futs.next().await {
            let (ok, value) = match &result {
                Ok(out) => (out.ok, out.value.clone()),
                Err(e) => (false, serde_json::json!({ "error": e.to_string() })),
            };
            let metadata = tool_ui_metadata(&name, &arguments, Some(&value));
            self.emit(RuntimeEvent::ToolCompleted {
                name: name.clone(),
                call_id: call_id.clone(),
                ok,
                output: value,
                duration_ms,
                tool_kind: metadata.tool_kind,
                file_path: metadata.file_path,
                summary: metadata.summary,
                meta: metadata.meta,
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
                        provider_tool_calls_persisted,
                    )
                    .await?,
                );
            } else {
                observations.push(
                    self.execute_tool_with_before(
                        session,
                        session_id,
                        inv,
                        preflight.remove(&i),
                        provider_tool_calls_persisted,
                    )
                    .await?,
                );
            }
        }
        for (index, observation) in observations.iter().enumerate() {
            if let Some(input) = batch_inputs.get_mut(index) {
                input.1 = observation.call_id.clone().or_else(|| input.1.clone());
            }
        }
        if let Some(feedback) = self
            .post_tool_batch_feedback(session_id, &batch_inputs, &observations)
            .await?
        {
            observations.push(feedback);
        }
        Ok(observations)
    }

    async fn post_tool_batch_feedback(
        &self,
        session_id: deepagent_core::id::SessionId,
        inputs: &[(String, Option<String>, serde_json::Value)],
        observations: &[Observation],
    ) -> Result<Option<Observation>> {
        if observations.len() <= 1 {
            return Ok(None);
        }
        let tools = observations
            .iter()
            .enumerate()
            .map(|(index, observation)| {
                let (name, call_id, arguments) = inputs.get(index).cloned().unwrap_or_else(|| {
                    (
                        observation.tool.clone(),
                        observation.call_id.clone(),
                        serde_json::Value::Null,
                    )
                });
                ToolBatchItem {
                    name,
                    call_id: observation.call_id.clone().or(call_id),
                    arguments,
                    ok: observation.ok,
                    output_preview: bounded_hook_output_preview(&observation.output, 4),
                }
            })
            .collect::<Vec<_>>();
        match self
            .fire_hook(
                session_id,
                HookPoint::PostToolBatch,
                HookData::tool_batch(tools.clone()),
            )
            .await?
        {
            HookOutcome::Continue | HookOutcome::Modify { .. } => Ok(None),
            HookOutcome::Ask { reason, source } => Ok(Some(Observation {
                tool: "post_tool_batch".to_string(),
                ok: false,
                output: serde_json::json!({
                    "blocked": true,
                    "needs_approval": true,
                    "reason": reason,
                    "source": source.label(),
                    "tool_count": tools.len(),
                }),
                call_id: None,
            })),
            HookOutcome::Deny { reason, source } => Ok(Some(Observation {
                tool: "post_tool_batch".to_string(),
                ok: false,
                output: serde_json::json!({
                    "blocked": true,
                    "needs_approval": false,
                    "reason": reason,
                    "source": source.label(),
                    "tool_count": tools.len(),
                }),
                call_id: None,
            })),
        }
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
    fn is_parallel_safe(&self, name: &str, arguments: &serde_json::Value) -> bool {
        if name == "task" {
            return true;
        }
        match self.registry.get(name) {
            Some(spec) => spec.tool.is_concurrency_safe(arguments),
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
        provider_tool_calls_persisted: bool,
    ) -> Result<Observation> {
        let CompletedToolCall {
            call_id,
            tool_name,
            arguments,
            result,
            duration_ms,
        } = completed;
        self.metrics.incr(names::TOOL_CALLS, 1);
        let requested = EventPayload::ToolCallRequested {
            call: ToolCall {
                id: call_id.clone(),
                name: tool_name.clone(),
                arguments: arguments.clone(),
            },
        };
        if provider_tool_calls_persisted {
            session.append_without_response_projection(requested)?;
        } else {
            session.append(requested)?;
        }

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
            if ok {
                HookPoint::AfterToolUse
            } else {
                HookPoint::PostToolUseFailure
            },
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
        invocation: deepagent_tools::ToolInvocation,
        provider_tool_calls_persisted: bool,
    ) -> Result<Observation> {
        self.execute_tool_with_before_and_speculative(
            session,
            session_id,
            invocation,
            None,
            None,
            provider_tool_calls_persisted,
        )
        .await
    }

    async fn execute_tool_with_speculative(
        &self,
        session: &mut Session<'a, C>,
        session_id: deepagent_core::id::SessionId,
        invocation: deepagent_tools::ToolInvocation,
        speculative: Option<SpeculativeToolExecution>,
        provider_tool_calls_persisted: bool,
    ) -> Result<Observation> {
        self.execute_tool_with_before_and_speculative(
            session,
            session_id,
            invocation,
            None,
            speculative,
            provider_tool_calls_persisted,
        )
        .await
    }

    async fn execute_tool_with_before(
        &self,
        session: &mut Session<'a, C>,
        session_id: deepagent_core::id::SessionId,
        invocation: deepagent_tools::ToolInvocation,
        before_override: Option<HookOutcome>,
        provider_tool_calls_persisted: bool,
    ) -> Result<Observation> {
        self.execute_tool_with_before_and_speculative(
            session,
            session_id,
            invocation,
            before_override,
            None,
            provider_tool_calls_persisted,
        )
        .await
    }

    async fn execute_tool_with_before_and_speculative(
        &self,
        session: &mut Session<'a, C>,
        session_id: deepagent_core::id::SessionId,
        invocation: deepagent_tools::ToolInvocation,
        before_override: Option<HookOutcome>,
        speculative: Option<SpeculativeToolExecution>,
        provider_tool_calls_persisted: bool,
    ) -> Result<Observation> {
        let cancel = self
            .cancel
            .clone()
            .unwrap_or_else(|| std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)));
        let pipeline = crate::tool_pipeline::ToolExecutionPipeline::new(
            self.registry,
            session_id,
            self.config.permissions.clone(),
        )
        .with_hooks(self.hooks)
        .with_approvals(self.approvals.clone())
        .with_events(self.events.clone())
        .with_auto_approve(self.config.auto_approve)
        .with_execution_controls(cancel, self.config.tool_timeout)
        .with_result_policy(
            self.config.tool_result_budget.clone(),
            self.config.tool_result_decorator.clone(),
            self.created_tool_result_paths.clone(),
        )
        .with_checkpoint(self.config.checkpoint.clone());
        let pipeline = pipeline.with_artifact_persistence(self.config.artifact_persistence.clone());

        let prepared = pipeline.prepare(invocation, before_override).await?;
        let result = match prepared {
            crate::tool_pipeline::ToolPreparation::Ready(prepared) => {
                let requested = EventPayload::ToolCallRequested {
                    call: ToolCall {
                        id: prepared.call_id.clone(),
                        name: prepared.name.clone(),
                        arguments: prepared.arguments.clone(),
                    },
                };
                if provider_tool_calls_persisted {
                    session.append_without_response_projection(requested)?;
                } else {
                    session.append(requested)?;
                }
                self.metrics.incr(names::TOOL_CALLS, 1);
                match speculative {
                    Some(speculative) => match speculative.handle.await {
                        Ok((result, duration_ms)) => {
                            pipeline
                                .complete_prepared(prepared, result, duration_ms)
                                .await?
                        }
                        Err(error) => {
                            tracing::debug!(
                                call_id = %prepared.call_id,
                                error = %error,
                                "speculative read was unavailable; executing after commit"
                            );
                            pipeline.execute_prepared(prepared).await?
                        }
                    },
                    None => pipeline.execute_prepared(prepared).await?,
                }
            }
            crate::tool_pipeline::ToolPreparation::Blocked(blocked) => {
                if let Some(speculative) = speculative {
                    speculative
                        .cancel
                        .store(true, std::sync::atomic::Ordering::Release);
                    speculative.handle.abort();
                }
                let requested = EventPayload::ToolCallRequested {
                    call: ToolCall {
                        id: blocked.call_id.clone(),
                        name: blocked.name.clone(),
                        arguments: blocked.arguments.clone(),
                    },
                };
                if provider_tool_calls_persisted {
                    session.append_without_response_projection(requested)?;
                } else {
                    session.append(requested)?;
                }
                blocked
            }
        };

        if !result.output.ok {
            self.metrics.incr(names::TOOL_FAILURES, 1);
        }
        session.append(EventPayload::ToolCallCompleted {
            call_id: result.call_id.clone(),
            ok: result.output.ok,
            output: result.output.value.clone(),
            duration_ms: result.duration_ms,
        })?;
        if !result.output.ok && result.stage != crate::tool_pipeline::ToolPipelineStage::Execution {
            self.fire_hook(
                session_id,
                HookPoint::PermissionDenied,
                HookData::Permission {
                    tool: result.name.clone(),
                    arguments: result.arguments.clone(),
                    reason: result.output.value["error"]
                        .as_str()
                        .unwrap_or("tool blocked")
                        .to_string(),
                },
            )
            .await?;
        }
        Ok(Observation {
            tool: result.name,
            ok: result.output.ok,
            output: result.output.value,
            call_id: Some(result.call_id),
        })
    }

    #[allow(dead_code)]
    async fn execute_tool_with_before_legacy(
        &self,
        session: &mut Session<'a, C>,
        session_id: deepagent_core::id::SessionId,
        mut invocation: deepagent_tools::ToolInvocation,
        before_override: Option<HookOutcome>,
    ) -> Result<Observation> {
        // Reuse the model's tool-call id when present so the observation
        // correlates with the exact `tool_calls[].id`; else synthesize one.
        let call_id = invocation
            .id
            .clone()
            .unwrap_or_else(|| format!("call_{}", deepagent_core::id::EventId::new()));
        let tool_name = invocation.name.clone();

        // Claude-compatible boundary: descriptor schema and tool-specific
        // value validation happen before hooks or permission evaluation.
        match self
            .registry
            .validate_invocation(&tool_name, invocation.arguments.clone())
        {
            Ok(validated) => invocation.arguments = validated.arguments,
            Err(error) => {
                let reason = error.to_string();
                self.metrics.incr(names::TOOL_FAILURES, 1);
                self.emit(RuntimeEvent::ToolBlocked {
                    name: tool_name.clone(),
                    reason: reason.clone(),
                    needs_approval: false,
                });
                let value = serde_json::json!({
                    "error": reason,
                    "error_type": "input_validation_error"
                });
                session.append(EventPayload::ToolCallRequested {
                    call: ToolCall {
                        id: call_id.clone(),
                        name: tool_name.clone(),
                        arguments: invocation.arguments,
                    },
                })?;
                session.append(EventPayload::ToolCallCompleted {
                    call_id: call_id.clone(),
                    ok: false,
                    output: value.clone(),
                    duration_ms: 0,
                })?;
                return Ok(Observation {
                    tool: tool_name,
                    ok: false,
                    output: value,
                    call_id: Some(call_id),
                });
            }
        }

        // BeforeToolUse gate: a hook may allow, rewrite the input (Modify),
        // request approval (Ask), or veto (Deny) the call. A blocked call
        // becomes a failed observation (recorded to the log) so the agent can
        // react, rather than aborting the whole run.
        let before = match before_override {
            Some(outcome) => outcome,
            None => {
                self.fire_hook(
                    session_id,
                    HookPoint::BeforeToolUse,
                    HookData::before_tool(tool_name.clone(), invocation.arguments.clone()),
                )
                .await?
            }
        };

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
                match self.registry.validate_invocation(&tool_name, updated_input) {
                    Ok(validated) => {
                        invocation.arguments = validated.arguments;
                        None
                    }
                    Err(error) => Some(format!("hook produced invalid tool input: {error}")),
                }
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
                    let permission_hook = self
                        .fire_hook(
                            session_id,
                            HookPoint::PermissionRequest,
                            HookData::Permission {
                                tool: tool_name.clone(),
                                arguments: invocation.arguments.clone(),
                                reason: reason.clone(),
                            },
                        )
                        .await?;
                    if let HookOutcome::Deny {
                        reason: hook_reason,
                        ..
                    } = permission_hook
                    {
                        return self
                            .record_permission_denied(
                                session,
                                session_id,
                                call_id,
                                tool_name,
                                invocation.arguments,
                                hook_reason,
                            )
                            .await;
                    }
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
                HookPoint::PermissionDenied,
                HookData::Permission {
                    tool: tool_name.clone(),
                    arguments: invocation.arguments.clone(),
                    reason: reason.clone(),
                },
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
        let metadata = tool_ui_metadata(&tool_name, &invocation.arguments, None);
        self.emit(RuntimeEvent::ToolStarted {
            name: tool_name.clone(),
            call_id: call_id.clone(),
            arguments: invocation.arguments.clone(),
            tool_kind: metadata.tool_kind,
            file_path: metadata.file_path,
            summary: metadata.summary,
            meta: metadata.meta,
        });

        let start = std::time::Instant::now();
        let execution_context = ToolExecutionContext::new(
            self.cancel
                .clone()
                .unwrap_or_else(|| std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false))),
        )
        .with_timeout(self.config.tool_timeout);
        let output = self
            .registry
            .invoke_with_context(
                &tool_name,
                invocation.arguments.clone(),
                &self.config.permissions,
                self.config.auto_approve,
                execution_context,
            )
            .await;
        let duration_ms = start.elapsed().as_millis() as u64;

        let observation = match output {
            Ok(out) => {
                let mut out = apply_tool_result_budget(
                    &self.config.tool_result_budget,
                    &session_id.to_string(),
                    &tool_name,
                    &call_id,
                    out,
                )
                .await;
                ensure_non_empty_output(&mut out, &tool_name);
                if let Some(decorator) = self.config.tool_result_decorator.as_ref() {
                    decorator.decorate(&tool_name, &mut out).await;
                }
                self.remember_tool_result_path(&out);
                if !out.ok {
                    self.metrics.incr(names::TOOL_FAILURES, 1);
                }
                let metadata =
                    tool_ui_metadata(&tool_name, &invocation.arguments, Some(&out.value));
                self.emit(RuntimeEvent::ToolCompleted {
                    name: tool_name.clone(),
                    call_id: call_id.clone(),
                    ok: out.ok,
                    output: out.value.clone(),
                    duration_ms,
                    tool_kind: metadata.tool_kind,
                    file_path: metadata.file_path,
                    summary: metadata.summary,
                    meta: metadata.meta,
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
                let metadata =
                    tool_ui_metadata(&tool_name, &invocation.arguments, Some(&err_value));
                self.emit(RuntimeEvent::ToolCompleted {
                    name: tool_name.clone(),
                    call_id: call_id.clone(),
                    ok: false,
                    output: err_value.clone(),
                    duration_ms,
                    tool_kind: metadata.tool_kind,
                    file_path: metadata.file_path,
                    summary: metadata.summary,
                    meta: metadata.meta,
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

        // Success and failure have distinct Claude-compatible hook events.
        self.fire_hook(
            session_id,
            if observation.ok {
                HookPoint::AfterToolUse
            } else {
                HookPoint::PostToolUseFailure
            },
            HookData::after_tool(tool_name, invocation.arguments, observation.ok),
        )
        .await?;

        Ok(observation)
    }

    async fn record_permission_denied(
        &self,
        session: &mut Session<'a, C>,
        session_id: deepagent_core::id::SessionId,
        call_id: String,
        tool_name: String,
        arguments: serde_json::Value,
        reason: String,
    ) -> Result<Observation> {
        let value = serde_json::json!({ "error": reason, "error_type": "permission_denied" });
        session.append(EventPayload::ToolCallRequested {
            call: ToolCall {
                id: call_id.clone(),
                name: tool_name.clone(),
                arguments: arguments.clone(),
            },
        })?;
        session.append(EventPayload::ToolCallCompleted {
            call_id: call_id.clone(),
            ok: false,
            output: value.clone(),
            duration_ms: 0,
        })?;
        self.fire_hook(
            session_id,
            HookPoint::PermissionDenied,
            HookData::Permission {
                tool: tool_name.clone(),
                arguments,
                reason: value["error"].as_str().unwrap_or_default().to_string(),
            },
        )
        .await?;
        Ok(Observation {
            tool: tool_name,
            ok: false,
            output: value,
            call_id: Some(call_id),
        })
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

    async fn completion_gate(
        &self,
        session_id: deepagent_core::id::SessionId,
        candidate: String,
    ) -> Result<CompletionDecision> {
        let before_response = self
            .fire_hook(
                session_id,
                HookPoint::BeforeResponse,
                HookData::Response {
                    content: candidate.clone(),
                },
            )
            .await?;
        let candidate = match before_response {
            HookOutcome::Continue => candidate,
            HookOutcome::Modify { updated_input, .. } => updated_input
                .get("content")
                .and_then(serde_json::Value::as_str)
                .unwrap_or(&candidate)
                .to_string(),
            HookOutcome::Ask { reason, .. } | HookOutcome::Deny { reason, .. } => {
                self.fire_stop_failure(session_id, &candidate, &reason)
                    .await?;
                return Ok(CompletionDecision::Retry(completion_feedback(reason)));
            }
        };
        match self
            .fire_hook(
                session_id,
                HookPoint::Stop,
                HookData::Response {
                    content: candidate.clone(),
                },
            )
            .await?
        {
            HookOutcome::Continue | HookOutcome::Modify { .. } => {
                Ok(CompletionDecision::Accept(candidate))
            }
            HookOutcome::Ask { reason, .. } | HookOutcome::Deny { reason, .. } => {
                self.fire_stop_failure(session_id, &candidate, &reason)
                    .await?;
                Ok(CompletionDecision::Retry(completion_feedback(reason)))
            }
        }
    }

    async fn fire_stop_failure(
        &self,
        session_id: deepagent_core::id::SessionId,
        candidate: &str,
        reason: &str,
    ) -> Result<()> {
        let content = if reason.is_empty() {
            candidate.to_string()
        } else {
            format!("{candidate}\n\nStop failure reason: {reason}")
        };
        self.fire_hook(
            session_id,
            HookPoint::StopFailure,
            HookData::Response { content },
        )
        .await?;
        Ok(())
    }
}

async fn wait_for_runtime_cancel(cancel: std::sync::Arc<std::sync::atomic::AtomicBool>) {
    while !cancel.load(std::sync::atomic::Ordering::Acquire) {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
}

fn completion_feedback(reason: String) -> Observation {
    Observation {
        tool: "stop_hook".into(),
        ok: false,
        output: serde_json::json!({
            "completion_blocked": true,
            "reason": reason,
            "recovery_hint": "Address the stop-hook feedback, then produce a corrected final response."
        }),
        call_id: None,
    }
}

fn completion_failure_reason(observation: &Observation) -> String {
    observation
        .output
        .get("reason")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("completion requirements were not satisfied")
        .to_string()
}

fn bounded_hook_output_preview(value: &serde_json::Value, depth: usize) -> serde_json::Value {
    const MAX_STRING_CHARS: usize = 512;
    const MAX_ARRAY_ITEMS: usize = 20;
    const MAX_OBJECT_KEYS: usize = 40;

    if depth == 0 {
        return serde_json::json!({"truncated": true, "reason": "max_depth"});
    }
    match value {
        serde_json::Value::String(text) => {
            let mut chars = text.chars();
            let preview = chars.by_ref().take(MAX_STRING_CHARS).collect::<String>();
            if chars.next().is_some() {
                serde_json::json!({
                    "preview": preview,
                    "truncated": true,
                    "original_chars_at_least": MAX_STRING_CHARS + 1,
                })
            } else {
                serde_json::Value::String(text.clone())
            }
        }
        serde_json::Value::Array(items) => {
            let truncated = items.len() > MAX_ARRAY_ITEMS;
            let values = items
                .iter()
                .take(MAX_ARRAY_ITEMS)
                .map(|item| bounded_hook_output_preview(item, depth - 1))
                .collect::<Vec<_>>();
            if truncated {
                serde_json::json!({
                    "items": values,
                    "truncated": true,
                    "original_len": items.len(),
                })
            } else {
                serde_json::Value::Array(values)
            }
        }
        serde_json::Value::Object(map) => {
            let mut out = serde_json::Map::new();
            for (key, item) in map.iter().take(MAX_OBJECT_KEYS) {
                out.insert(key.clone(), bounded_hook_output_preview(item, depth - 1));
            }
            if map.len() > MAX_OBJECT_KEYS {
                out.insert("truncated".into(), serde_json::Value::Bool(true));
                out.insert("original_key_count".into(), serde_json::json!(map.len()));
            }
            serde_json::Value::Object(out)
        }
        _ => value.clone(),
    }
}

enum CompletionDecision {
    Accept(String),
    Retry(Observation),
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
    use deepagent_hooks::{
        Hook, HookContext, HookData, HookOutcome, HookPoint, HookRegistry, ToolAllowlistHook,
    };
    use deepagent_persistence::{event_store::EventStore, Database};
    use deepagent_tools::permission::{PermissionSet, RiskLevel};
    use deepagent_tools::{Tool, ToolDescriptor, ToolInvocation, ToolOutput};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    struct AddTool;
    struct BigTool;

    struct CountingReadTool {
        calls: Arc<std::sync::atomic::AtomicUsize>,
    }

    struct HangingReadTool {
        started: Arc<AtomicBool>,
        completed: Arc<AtomicBool>,
    }

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

    #[async_trait]
    impl Tool for BigTool {
        fn descriptor(&self) -> ToolDescriptor {
            ToolDescriptor {
                name: "big".into(),
                description: "returns a large payload".into(),
                parameters: serde_json::json!({"type": "object"}),
                risk: RiskLevel::Safe,
                required_permissions: PermissionSet::read_only(),
            }
        }
        async fn invoke(&self, _arguments: serde_json::Value) -> Result<ToolOutput> {
            Ok(ToolOutput::success(serde_json::json!({
                "content": "x".repeat(200)
            })))
        }
    }

    #[async_trait]
    impl Tool for CountingReadTool {
        fn descriptor(&self) -> ToolDescriptor {
            ToolDescriptor {
                name: "counting_read".into(),
                description: "counts read-only invocations".into(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {"path": {"type": "string"}},
                    "required": ["path"]
                }),
                risk: RiskLevel::Safe,
                required_permissions: PermissionSet::read_only(),
            }
        }

        async fn invoke(&self, arguments: serde_json::Value) -> Result<ToolOutput> {
            self.calls.fetch_add(1, Ordering::AcqRel);
            Ok(ToolOutput::success(serde_json::json!({
                "path": arguments["path"],
                "content": "ready"
            })))
        }
    }

    #[async_trait]
    impl Tool for HangingReadTool {
        fn descriptor(&self) -> ToolDescriptor {
            ToolDescriptor {
                name: "hanging_read".into(),
                description: "read-only cancellation probe".into(),
                parameters: serde_json::json!({"type": "object"}),
                risk: RiskLevel::Safe,
                required_permissions: PermissionSet::read_only(),
            }
        }

        async fn invoke(&self, _arguments: serde_json::Value) -> Result<ToolOutput> {
            self.started.store(true, Ordering::Release);
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
            self.completed.store(true, Ordering::Release);
            Ok(ToolOutput::success(serde_json::json!({"done": true})))
        }
    }

    struct StreamingReadAgent {
        calls: Arc<std::sync::atomic::AtomicUsize>,
    }

    struct StreamingBatchReadAgent {
        calls: Arc<std::sync::atomic::AtomicUsize>,
    }

    struct NativeResponseItemAgent {
        items: Vec<ResponseOutputItem>,
        final_items: Vec<ResponseOutputItem>,
    }

    struct RawUsageAgent {
        usage: crate::agent::RunUsage,
        raw_usage: Vec<serde_json::Value>,
    }

    #[async_trait]
    impl Agent for NativeResponseItemAgent {
        async fn think(&mut self, step: usize, _last: &[Observation]) -> Result<AgentDecision> {
            if step == 0 {
                Ok(AgentDecision::CallTool(
                    ToolInvocation::new("counting_read", serde_json::json!({"path": "native.txt"}))
                        .with_id("native-call"),
                ))
            } else {
                Ok(AgentDecision::CompleteItems {
                    message: Message::assistant("done"),
                    items: std::mem::take(&mut self.final_items),
                })
            }
        }

        fn take_pending_response_items(&mut self) -> Vec<ResponseOutputItem> {
            std::mem::take(&mut self.items)
        }
    }

    #[async_trait]
    impl Agent for RawUsageAgent {
        async fn think(&mut self, _step: usize, _last: &[Observation]) -> Result<AgentDecision> {
            Ok(AgentDecision::Complete("done".into()))
        }

        fn cumulative_usage(&self) -> Option<crate::agent::RunUsage> {
            Some(self.usage)
        }

        fn take_pending_raw_usage(&mut self) -> Vec<serde_json::Value> {
            std::mem::take(&mut self.raw_usage)
        }
    }

    #[async_trait]
    impl Agent for StreamingReadAgent {
        async fn think(&mut self, step: usize, _last: &[Observation]) -> Result<AgentDecision> {
            if step == 0 {
                Ok(AgentDecision::CallTool(
                    ToolInvocation::new(
                        "counting_read",
                        serde_json::json!({"path": "fixture.txt"}),
                    )
                    .with_id("stream-call"),
                ))
            } else {
                Ok(AgentDecision::Complete("done".into()))
            }
        }

        async fn think_streaming_cancelled(
            &mut self,
            step: usize,
            last: &[Observation],
            _cancel: Option<Arc<AtomicBool>>,
            tools: Option<&mut dyn ToolAttemptController>,
        ) -> Result<AgentDecision> {
            if step != 0 {
                assert_eq!(last.len(), 1);
                return Ok(AgentDecision::Complete("done".into()));
            }
            let invocation =
                ToolInvocation::new("counting_read", serde_json::json!({"path": "fixture.txt"}))
                    .with_id("stream-call");
            let tools = tools.expect("runtime supplies streaming tool controller");
            tools.begin(1);
            tools.prepare(invocation.clone());
            for _ in 0..100 {
                if self.calls.load(Ordering::Acquire) == 1 {
                    break;
                }
                tokio::task::yield_now().await;
            }
            assert_eq!(
                self.calls.load(Ordering::Acquire),
                1,
                "safe tool must begin before the model attempt commits"
            );
            tools.commit(1);
            Ok(AgentDecision::CallTool(invocation))
        }
    }

    #[async_trait]
    impl Agent for StreamingBatchReadAgent {
        async fn think(&mut self, _step: usize, _last: &[Observation]) -> Result<AgentDecision> {
            Ok(AgentDecision::Complete("unused".into()))
        }

        async fn think_streaming_cancelled(
            &mut self,
            step: usize,
            last: &[Observation],
            _cancel: Option<Arc<AtomicBool>>,
            tools: Option<&mut dyn ToolAttemptController>,
        ) -> Result<AgentDecision> {
            if step != 0 {
                assert_eq!(last.len(), 2);
                return Ok(AgentDecision::Complete("batch done".into()));
            }
            let invocations = vec![
                ToolInvocation::new("counting_read", serde_json::json!({"path": "one.txt"}))
                    .with_id("stream-one"),
                ToolInvocation::new("counting_read", serde_json::json!({"path": "two.txt"}))
                    .with_id("stream-two"),
            ];
            let tools = tools.expect("runtime supplies streaming tool controller");
            tools.begin(1);
            for invocation in &invocations {
                tools.prepare(invocation.clone());
            }
            for _ in 0..100 {
                if self.calls.load(Ordering::Acquire) == 2 {
                    break;
                }
                tokio::task::yield_now().await;
            }
            assert_eq!(self.calls.load(Ordering::Acquire), 2);
            tools.commit(1);
            Ok(AgentDecision::CallTools(invocations))
        }
    }

    fn registry() -> ToolRegistry {
        let mut r = ToolRegistry::new();
        r.register(Arc::new(AddTool)).unwrap();
        r
    }

    fn registry_with_big() -> ToolRegistry {
        let mut r = registry();
        r.register(Arc::new(BigTool)).unwrap();
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

    // --- §2.2 advisory adversarial verification wiring ----------------------

    struct ScriptedVerifier {
        verdicts:
            std::sync::Mutex<std::collections::VecDeque<crate::adversarial::AdversarialVerdict>>,
        calls: Arc<std::sync::atomic::AtomicUsize>,
    }

    #[async_trait]
    impl crate::adversarial::AdversarialVerifier for ScriptedVerifier {
        async fn verify(
            &self,
            _final_answer: &str,
            _changed_files: &[String],
        ) -> crate::adversarial::AdversarialVerdict {
            self.calls.fetch_add(1, Ordering::AcqRel);
            self.verdicts
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(crate::adversarial::AdversarialVerdict::Accepted)
        }
    }

    fn scripted_verifier(
        verdicts: impl IntoIterator<Item = crate::adversarial::AdversarialVerdict>,
        calls: Arc<std::sync::atomic::AtomicUsize>,
    ) -> Arc<ScriptedVerifier> {
        Arc::new(ScriptedVerifier {
            verdicts: std::sync::Mutex::new(verdicts.into_iter().collect()),
            calls,
        })
    }

    #[tokio::test]
    async fn adversarial_refute_feeds_gaps_back_then_accepts_within_budget() {
        use crate::adversarial::AdversarialVerdict;
        let db = Database::open_in_memory().unwrap();
        let clock = FixedClock::new(1);
        let mut session = Session::create(&db, &clock, Some("run")).unwrap();
        let task = session.create_task("do the work").unwrap();

        // First completion refuted → re-entry; second completion accepted
        // (budget=1: the panel is not consulted again, it passes through).
        let mut agent = ScriptedAgent::new([
            AgentDecision::CompleteMessage(Message::assistant("done v1")),
            AgentDecision::CompleteMessage(Message::assistant("done v2")),
        ]);
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let verifier = scripted_verifier(
            [AdversarialVerdict::Refuted {
                gaps: vec!["skeptic 0: tests look mocked out".to_string()],
            }],
            calls.clone(),
        );
        let reg = registry();
        // Default config: no checkpoint → run_mutated_files() == true.
        let engine = RuntimeEngine::new(&reg, Metrics::new(), RuntimeConfig::default())
            .with_adversarial_verifier(verifier);
        let outcome = engine.run(&mut session, task, &mut agent).await.unwrap();
        assert_eq!(outcome, RunOutcome::Completed("done v2".into()));
        // Panel consulted exactly once (budget=1); the agent saw the advisory.
        assert_eq!(calls.load(Ordering::Acquire), 1);
        let saw_advisory = agent.observations.iter().any(|o| {
            o.tool == "adversarial_verification"
                && o.output["advisory"] == true
                && o.output["gaps"][0]
                    .as_str()
                    .is_some_and(|g| g.contains("mocked out"))
        });
        assert!(
            saw_advisory,
            "agent must receive the advisory gaps observation"
        );
    }

    #[tokio::test]
    async fn adversarial_accept_completes_without_reentry() {
        use crate::adversarial::AdversarialVerdict;
        let db = Database::open_in_memory().unwrap();
        let clock = FixedClock::new(1);
        let mut session = Session::create(&db, &clock, Some("run")).unwrap();
        let task = session.create_task("do the work").unwrap();
        let mut agent =
            ScriptedAgent::new([AgentDecision::CompleteMessage(Message::assistant("done"))]);
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let verifier = scripted_verifier([AdversarialVerdict::Accepted], calls.clone());
        let reg = registry();
        let engine = RuntimeEngine::new(&reg, Metrics::new(), RuntimeConfig::default())
            .with_adversarial_verifier(verifier);
        let outcome = engine.run(&mut session, task, &mut agent).await.unwrap();
        assert_eq!(outcome, RunOutcome::Completed("done".into()));
        assert_eq!(calls.load(Ordering::Acquire), 1);
        // No advisory observation was fed back.
        assert!(agent
            .observations
            .iter()
            .all(|o| o.tool != "adversarial_verification"));
    }

    #[tokio::test]
    async fn adversarial_cap_prevents_infinite_reentry() {
        use crate::adversarial::AdversarialVerdict;
        let db = Database::open_in_memory().unwrap();
        let clock = FixedClock::new(1);
        let mut session = Session::create(&db, &clock, Some("run")).unwrap();
        let task = session.create_task("do the work").unwrap();
        // Panel ALWAYS refutes; run must still terminate at budget=1.
        let mut agent = ScriptedAgent::new([
            AgentDecision::CompleteMessage(Message::assistant("v1")),
            AgentDecision::CompleteMessage(Message::assistant("v2")),
        ]);
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let verifier = scripted_verifier(
            [
                AdversarialVerdict::Refuted {
                    gaps: vec!["skeptic 0: unmet".to_string()],
                },
                AdversarialVerdict::Refuted {
                    gaps: vec!["skeptic 0: still unmet".to_string()],
                },
            ],
            calls.clone(),
        );
        let reg = registry();
        let engine = RuntimeEngine::new(&reg, Metrics::new(), RuntimeConfig::default())
            .with_adversarial_verifier(verifier);
        let outcome = engine.run(&mut session, task, &mut agent).await.unwrap();
        // Second completion accepted despite a refuting panel (budget spent).
        assert_eq!(outcome, RunOutcome::Completed("v2".into()));
        // Panel consulted exactly once (cap short-circuits the second check).
        assert_eq!(calls.load(Ordering::Acquire), 1);
    }

    #[tokio::test]
    async fn committed_streaming_read_executes_early_but_only_once() {
        let db = Database::open_in_memory().unwrap();
        let clock = FixedClock::new(1);
        let mut session = Session::create(&db, &clock, Some("streaming read")).unwrap();
        let task = session.create_task("read fixture").unwrap();
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut registry = ToolRegistry::new();
        registry
            .register(Arc::new(CountingReadTool {
                calls: calls.clone(),
            }))
            .unwrap();
        let mut agent = StreamingReadAgent {
            calls: calls.clone(),
        };
        let (sink, mut events) = crate::events::ChannelSink::new();
        let engine = RuntimeEngine::new(&registry, Metrics::new(), RuntimeConfig::default())
            .with_events(Arc::new(sink));

        let outcome = engine.run(&mut session, task, &mut agent).await.unwrap();

        assert_eq!(outcome, RunOutcome::Completed("done".into()));
        assert_eq!(calls.load(Ordering::Acquire), 1);
        let completed = std::iter::from_fn(|| events.try_recv().ok())
            .filter(|event| matches!(event, RuntimeEvent::ToolCompleted { .. }))
            .count();
        assert_eq!(
            completed, 1,
            "commit must publish exactly one paired result"
        );
    }

    #[tokio::test]
    async fn native_response_items_suppress_synthetic_assistant_projection() {
        let db = Database::open_in_memory().unwrap();
        let clock = FixedClock::new(1);
        let mut session = Session::create(&db, &clock, Some("native items")).unwrap();
        let task = session.create_task("read fixture").unwrap();
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut registry = ToolRegistry::new();
        registry
            .register(Arc::new(CountingReadTool {
                calls: calls.clone(),
            }))
            .unwrap();
        let mut agent = NativeResponseItemAgent {
            items: vec![
                ResponseOutputItem::Reasoning {
                    id: Some("rs_1".into()),
                    content: "checking".into(),
                },
                ResponseOutputItem::FunctionCall {
                    call_id: "native-call".into(),
                    name: "counting_read".into(),
                    arguments: r#"{"path":"native.txt"}"#.into(),
                },
            ],
            final_items: vec![ResponseOutputItem::Message {
                role: "assistant".into(),
                content: "done".into(),
            }],
        };
        let engine = RuntimeEngine::new(&registry, Metrics::new(), RuntimeConfig::default());

        let outcome = engine.run(&mut session, task, &mut agent).await.unwrap();

        assert_eq!(outcome, RunOutcome::Completed("done".into()));
        let response_items: Vec<_> = EventStore::new(&db)
            .load_session(session.id())
            .unwrap()
            .into_iter()
            .filter_map(|event| match event.payload {
                EventPayload::ResponseItemAppended { item } => Some(item),
                _ => None,
            })
            .collect();
        assert_eq!(
            response_items
                .iter()
                .filter(|item| matches!(
                    item,
                    ResponseOutputItem::FunctionCall { call_id, .. } if call_id == "native-call"
                ))
                .count(),
            1,
            "provider function_call must not be duplicated by ToolCallRequested projection"
        );
        assert!(response_items.iter().any(|item| matches!(
            item,
            ResponseOutputItem::FunctionCallOutput { call_id, .. } if call_id == "native-call"
        )));
        assert!(response_items.iter().any(|item| matches!(
            item,
            ResponseOutputItem::Message { role, content }
                if role == "assistant" && content == "done"
        )));
    }

    #[tokio::test]
    async fn usage_recorded_persists_raw_responses_usage() {
        let db = Database::open_in_memory().unwrap();
        let clock = FixedClock::new(1);
        let mut session = Session::create(&db, &clock, Some("usage")).unwrap();
        let task = session.create_task("finish").unwrap();
        let mut agent = RawUsageAgent {
            usage: crate::agent::RunUsage {
                prompt_tokens: 4,
                completion_tokens: 5,
                reasoning_tokens: 3,
                total_tokens: 9,
                prompt_cache_hit_tokens: 2,
                prompt_cache_miss_tokens: 0,
            },
            raw_usage: vec![serde_json::json!({
                "input_tokens": 4,
                "input_tokens_details": {"cached_tokens": 2},
                "output_tokens": 5,
                "output_tokens_details": {"reasoning_tokens": 3},
                "total_tokens": 9
            })],
        };
        let registry = registry();
        let engine = RuntimeEngine::new(&registry, Metrics::new(), RuntimeConfig::default());

        engine.run(&mut session, task, &mut agent).await.unwrap();

        let usage = EventStore::new(&db)
            .load_session(session.id())
            .unwrap()
            .into_iter()
            .find_map(|event| match event.payload {
                EventPayload::UsageRecorded {
                    raw_responses_usage,
                    ..
                } => raw_responses_usage,
                _ => None,
            })
            .expect("raw usage persisted");
        assert_eq!(usage[0]["input_tokens"], 4);
        assert_eq!(usage[0]["output_tokens_details"]["reasoning_tokens"], 3);
    }

    #[tokio::test]
    async fn aborted_model_attempt_cancels_speculative_read() {
        let started = Arc::new(AtomicBool::new(false));
        let completed = Arc::new(AtomicBool::new(false));
        let mut registry = ToolRegistry::new();
        registry
            .register(Arc::new(HangingReadTool {
                started: started.clone(),
                completed: completed.clone(),
            }))
            .unwrap();
        let mut attempt = StreamingToolAttempt::new(
            &registry,
            PermissionSet::read_only(),
            Arc::new(AtomicBool::new(false)),
            std::time::Duration::from_secs(60),
            true,
        );

        attempt.begin(1);
        attempt.prepare(
            ToolInvocation::new("hanging_read", serde_json::json!({})).with_id("stale-call"),
        );
        for _ in 0..100 {
            if started.load(Ordering::Acquire) {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert!(started.load(Ordering::Acquire));

        attempt.abort(1, "provider stream failed");
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        assert!(!completed.load(Ordering::Acquire));
        assert!(attempt.prepared.is_empty());
        assert_eq!(attempt.committed_attempt, None);
    }

    #[tokio::test]
    async fn committed_streaming_batch_reuses_all_early_read_results() {
        let db = Database::open_in_memory().unwrap();
        let clock = FixedClock::new(1);
        let mut session = Session::create(&db, &clock, Some("streaming batch")).unwrap();
        let task = session.create_task("read two fixtures").unwrap();
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut registry = ToolRegistry::new();
        registry
            .register(Arc::new(CountingReadTool {
                calls: calls.clone(),
            }))
            .unwrap();
        let mut agent = StreamingBatchReadAgent {
            calls: calls.clone(),
        };
        let (sink, mut events) = crate::events::ChannelSink::new();
        let engine = RuntimeEngine::new(&registry, Metrics::new(), RuntimeConfig::default())
            .with_events(Arc::new(sink));

        let outcome = engine.run(&mut session, task, &mut agent).await.unwrap();

        assert_eq!(outcome, RunOutcome::Completed("batch done".into()));
        assert_eq!(calls.load(Ordering::Acquire), 2);
        let completed = std::iter::from_fn(|| events.try_recv().ok())
            .filter(|event| matches!(event, RuntimeEvent::ToolCompleted { .. }))
            .count();
        assert_eq!(completed, 2);
    }

    #[tokio::test]
    async fn complete_message_persists_final_reasoning() {
        let db = Database::open_in_memory().unwrap();
        let clock = FixedClock::new(1);
        let mut session = Session::create(&db, &clock, Some("run")).unwrap();
        let session_id = session.id();
        let task = session.create_task("explain screenshot").unwrap();
        let final_message =
            Message::assistant("final answer").with_reasoning("reasoning visible after refresh");
        let mut agent = ScriptedAgent::new([AgentDecision::CompleteMessage(final_message)]);

        let reg = registry();
        let engine = RuntimeEngine::new(&reg, Metrics::new(), RuntimeConfig::default());
        let outcome = engine.run(&mut session, task, &mut agent).await.unwrap();
        assert_eq!(outcome, RunOutcome::Completed("final answer".into()));

        let events = deepagent_persistence::event_store::EventStore::new(&db)
            .load_session(session_id)
            .unwrap();
        let assistant = events
            .iter()
            .find_map(|ev| match &ev.payload {
                EventPayload::MessageAppended { message }
                    if message.role == deepagent_core::message::Role::Assistant =>
                {
                    Some(message)
                }
                _ => None,
            })
            .expect("assistant message event");
        assert_eq!(
            assistant.reasoning_content.as_deref(),
            Some("reasoning visible after refresh")
        );
    }

    struct WaitingAgent {
        entered: Arc<tokio::sync::Notify>,
    }

    #[async_trait]
    impl Agent for WaitingAgent {
        async fn think(&mut self, _step: usize, _last: &[Observation]) -> Result<AgentDecision> {
            std::future::pending().await
        }

        async fn think_cancelled(
            &mut self,
            _step: usize,
            _last: &[Observation],
            cancel: Option<Arc<AtomicBool>>,
        ) -> Result<AgentDecision> {
            self.entered.notify_waiters();
            loop {
                if cancel
                    .as_ref()
                    .map(|flag| flag.load(Ordering::Relaxed))
                    .unwrap_or(false)
                {
                    return Err(deepagent_core::error::CoreError::other("request cancelled"));
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        }
    }

    #[tokio::test]
    async fn cancellation_interrupts_agent_think() {
        let db = Database::open_in_memory().unwrap();
        let clock = FixedClock::new(1);
        let mut session = Session::create(&db, &clock, Some("run")).unwrap();
        let task = session.create_task("wait").unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let entered = Arc::new(tokio::sync::Notify::new());
        let mut agent = WaitingAgent {
            entered: entered.clone(),
        };
        let reg = registry();
        let engine = RuntimeEngine::new(&reg, Metrics::new(), RuntimeConfig::default())
            .with_cancel(cancel.clone());

        let outcome = {
            let run = engine.run(&mut session, task, &mut agent);
            tokio::pin!(run);
            tokio::select! {
                result = &mut run => {
                    panic!("run completed before cancellation: {result:?}");
                }
                _ = entered.notified() => {}
                _ = tokio::time::sleep(std::time::Duration::from_secs(1)) => {
                    panic!("agent did not enter think_cancelled");
                }
            }
            cancel.store(true, Ordering::Relaxed);

            tokio::time::timeout(std::time::Duration::from_secs(1), run)
                .await
                .expect("run should stop promptly")
                .unwrap()
        };
        assert_eq!(outcome, RunOutcome::Cancelled);
        assert_eq!(session.state().task(task).unwrap().state, TaskState::Failed);
    }

    #[tokio::test]
    async fn oversized_tool_result_is_truncated_and_persisted() {
        let db = Database::open_in_memory().unwrap();
        let clock = FixedClock::new(1);
        let mut session = Session::create(&db, &clock, Some("budget")).unwrap();
        let task = session.create_task("big output").unwrap();
        let temp = tempfile::tempdir().unwrap();

        let mut agent = ScriptedAgent::new([
            AgentDecision::CallTool(ToolInvocation::new("big", serde_json::json!({}))),
            AgentDecision::Complete("done".into()),
        ]);

        let reg = registry_with_big();
        let config = RuntimeConfig {
            tool_result_budget: ToolResultBudgetConfig {
                max_tokens: 10,
                preview_tokens: 4,
                output_dir: temp.path().to_path_buf(),
                cleanup_on_run_end: false,
                ..Default::default()
            },
            ..Default::default()
        };
        let engine = RuntimeEngine::new(&reg, Metrics::new(), config);

        let outcome = engine.run(&mut session, task, &mut agent).await.unwrap();
        assert_eq!(outcome, RunOutcome::Completed("done".into()));

        let observation = &agent.observations[0];
        assert!(observation.output["truncated"].as_bool().unwrap());
        assert!(observation.output["message"]
            .as_str()
            .unwrap()
            .contains("output truncated"));
        let saved = std::path::PathBuf::from(observation.output["saved_path"].as_str().unwrap());
        assert!(saved.exists());
        let full = tokio::fs::read_to_string(saved).await.unwrap();
        assert!(full.contains(&"x".repeat(200)));

        let events = deepagent_persistence::event_store::EventStore::new(&db)
            .load_session(session.id())
            .unwrap();
        let completed = events
            .iter()
            .find_map(|ev| match &ev.payload {
                EventPayload::ToolCallCompleted { output, .. } => Some(output),
                _ => None,
            })
            .expect("tool completion event");
        assert_eq!(completed["truncated"], true);
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

    struct PostToolBatchRecorder {
        batches: Arc<std::sync::Mutex<Vec<Vec<String>>>>,
        deny: bool,
    }

    #[async_trait]
    impl Hook for PostToolBatchRecorder {
        fn name(&self) -> &str {
            "post_tool_batch_recorder"
        }

        async fn run(&self, ctx: &HookContext) -> Result<HookOutcome> {
            if ctx.point != HookPoint::PostToolBatch {
                return Ok(HookOutcome::Continue);
            }
            let HookData::ToolBatch { tools } = &ctx.data else {
                panic!("PostToolBatch must carry ToolBatch payload");
            };
            self.batches
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .push(tools.iter().map(|tool| tool.name.clone()).collect());
            assert_eq!(tools[0].call_id.as_deref(), Some("c1"));
            assert_eq!(tools[1].call_id.as_deref(), Some("c2"));
            assert_eq!(tools[0].output_preview["sum"], 3);
            if self.deny {
                Ok(HookOutcome::deny("batch result requires model repair"))
            } else {
                Ok(HookOutcome::Continue)
            }
        }
    }

    #[tokio::test]
    async fn post_tool_batch_hook_receives_ordered_results() {
        let db = Database::open_in_memory().unwrap();
        let clock = FixedClock::new(1);
        let mut session = Session::create(&db, &clock, None).unwrap();
        let task = session.create_task("parallel hook").unwrap();
        let batches = Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut hooks = HookRegistry::new();
        hooks.register(
            HookPoint::PostToolBatch,
            Arc::new(PostToolBatchRecorder {
                batches: batches.clone(),
                deny: false,
            }),
        );

        let mut agent = ScriptedAgent::new([
            AgentDecision::CallTools(vec![
                ToolInvocation::new("add", serde_json::json!({"a": 1, "b": 2})).with_id("c1"),
                ToolInvocation::new("add", serde_json::json!({"a": 3, "b": 4})).with_id("c2"),
            ]),
            AgentDecision::Complete("done".into()),
        ]);
        let reg = registry();
        let engine =
            RuntimeEngine::new(&reg, Metrics::new(), RuntimeConfig::default()).with_hooks(&hooks);

        let outcome = engine.run(&mut session, task, &mut agent).await.unwrap();

        assert_eq!(outcome, RunOutcome::Completed("done".into()));
        assert_eq!(
            batches.lock().unwrap().as_slice(),
            &[vec!["add".to_string(), "add".to_string()]]
        );
        assert_eq!(agent.observations.len(), 2);
    }

    #[tokio::test]
    async fn post_tool_batch_deny_is_returned_as_structured_feedback() {
        let db = Database::open_in_memory().unwrap();
        let clock = FixedClock::new(1);
        let mut session = Session::create(&db, &clock, None).unwrap();
        let task = session.create_task("parallel hook deny").unwrap();
        let batches = Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut hooks = HookRegistry::new();
        hooks.register(
            HookPoint::PostToolBatch,
            Arc::new(PostToolBatchRecorder {
                batches,
                deny: true,
            }),
        );

        let mut agent = ScriptedAgent::new([
            AgentDecision::CallTools(vec![
                ToolInvocation::new("add", serde_json::json!({"a": 1, "b": 2})).with_id("c1"),
                ToolInvocation::new("add", serde_json::json!({"a": 3, "b": 4})).with_id("c2"),
            ]),
            AgentDecision::Complete("handled batch feedback".into()),
        ]);
        let reg = registry();
        let engine =
            RuntimeEngine::new(&reg, Metrics::new(), RuntimeConfig::default()).with_hooks(&hooks);

        let outcome = engine.run(&mut session, task, &mut agent).await.unwrap();

        assert_eq!(
            outcome,
            RunOutcome::Completed("handled batch feedback".into())
        );
        assert_eq!(agent.observations.len(), 3);
        let feedback = agent.observations.last().unwrap();
        assert_eq!(feedback.tool, "post_tool_batch");
        assert!(!feedback.ok);
        assert_eq!(feedback.output["blocked"], true);
        assert_eq!(
            feedback.output["reason"],
            "batch result requires model repair"
        );
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

    struct StopFailureRecorder {
        stop_calls: std::sync::atomic::AtomicUsize,
        failures: Arc<std::sync::Mutex<Vec<String>>>,
    }

    #[async_trait]
    impl deepagent_hooks::Hook for StopFailureRecorder {
        fn name(&self) -> &str {
            "stop_failure_recorder"
        }

        async fn run(&self, ctx: &HookContext) -> Result<HookOutcome> {
            match ctx.point {
                HookPoint::Stop => {
                    if self
                        .stop_calls
                        .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
                        == 0
                    {
                        Ok(HookOutcome::deny("revise final answer"))
                    } else {
                        Ok(HookOutcome::Continue)
                    }
                }
                HookPoint::StopFailure => {
                    if let HookData::Response { content } = &ctx.data {
                        self.failures
                            .lock()
                            .unwrap_or_else(|p| p.into_inner())
                            .push(content.clone());
                    }
                    Ok(HookOutcome::Continue)
                }
                _ => Ok(HookOutcome::Continue),
            }
        }
    }

    #[tokio::test]
    async fn stop_failure_hook_fires_when_stop_blocks_completion() {
        let db = Database::open_in_memory().unwrap();
        let clock = FixedClock::new(1);
        let mut session = Session::create(&db, &clock, None).unwrap();
        let task = session.create_task("answer with stop feedback").unwrap();
        let failures = Arc::new(std::sync::Mutex::new(Vec::new()));
        let hook = Arc::new(StopFailureRecorder {
            stop_calls: std::sync::atomic::AtomicUsize::new(0),
            failures: failures.clone(),
        });
        let mut hooks = HookRegistry::new();
        hooks.register(HookPoint::Stop, hook.clone());
        hooks.register(HookPoint::StopFailure, hook);

        let reg = registry();
        let engine =
            RuntimeEngine::new(&reg, Metrics::new(), RuntimeConfig::default()).with_hooks(&hooks);
        let mut agent = ScriptedAgent::new([
            AgentDecision::Complete("draft".into()),
            AgentDecision::Complete("revised".into()),
        ]);

        let outcome = engine.run(&mut session, task, &mut agent).await.unwrap();
        assert_eq!(outcome, RunOutcome::Completed("revised".into()));
        let recorded = failures.lock().unwrap().clone();
        assert_eq!(recorded.len(), 1);
        assert!(recorded[0].contains("draft"));
        assert!(recorded[0].contains("revise final answer"));
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
