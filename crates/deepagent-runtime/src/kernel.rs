//! Agent Kernel v2 lifecycle wrapper.

use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use deepagent_core::clock::Clock;
use deepagent_core::error::{CoreError, Result};
use deepagent_core::id::TaskId;
use deepagent_core::task::TaskState;
use deepagent_hooks::HookRegistry;
use deepagent_persistence::run_store::RunStore;
use deepagent_persistence::Database;
use deepagent_session::Session;
use deepagent_tools::ToolRegistry;
use deepagent_tracing::metrics::Metrics;
use serde::{Deserialize, Serialize};
use sha2::Digest;

use crate::agent::Agent;
use crate::approval::{ApprovalGate, AutoDenyGate};
use crate::cancellation::CancellationTree;
use crate::events::{NullEventSink, RuntimeEvent, RuntimeEventSink};
use crate::loop_engine::{RunOutcome, RuntimeConfig, RuntimeEngine, VerificationPlan};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunPhase {
    Accepted,
    Preparing,
    RunningTurn,
    ExecutingTools,
    Verifying,
    Finalizing,
    Terminal,
}

impl RunPhase {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Accepted => "accepted",
            Self::Preparing => "preparing",
            Self::RunningTurn => "running_turn",
            Self::ExecutingTools => "executing_tools",
            Self::Verifying => "verifying",
            Self::Finalizing => "finalizing",
            Self::Terminal => "terminal",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalKind {
    Succeeded,
    Cancelled,
    Blocked,
    Failed,
    MaxTurns,
    BudgetExceeded,
    ContextExhausted,
}

impl TerminalKind {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Succeeded => "succeeded",
            Self::Cancelled => "cancelled",
            Self::Blocked => "blocked",
            Self::Failed => "failed",
            Self::MaxTurns => "max_turns",
            Self::BudgetExceeded => "budget_exceeded",
            Self::ContextExhausted => "context_exhausted",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KernelTerminal {
    pub kind: TerminalKind,
    pub message: Option<String>,
    pub reason: Option<String>,
}

impl KernelTerminal {
    pub fn succeeded(&self) -> bool {
        self.kind == TerminalKind::Succeeded
    }

    pub fn into_completion_result(self) -> Result<()> {
        match self.kind {
            TerminalKind::Failed
            | TerminalKind::BudgetExceeded
            | TerminalKind::ContextExhausted => Err(CoreError::other(
                self.reason.unwrap_or_else(|| self.kind.label().to_string()),
            )),
            _ => Ok(()),
        }
    }
}

#[derive(Clone, Debug)]
pub struct RunHandle {
    run_id: String,
    cancellation: CancellationTree,
}

impl RunHandle {
    pub fn run_id(&self) -> &str {
        &self.run_id
    }

    pub fn cancel(&self) -> bool {
        self.cancellation.cancel()
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancellation.is_cancelled()
    }
}

pub struct RunRequest<'run, 'session, C: Clock> {
    pub session: &'run mut Session<'session, C>,
    pub task: TaskId,
    pub agent: &'run mut dyn Agent,
}

impl<'run, 'session, C: Clock> RunRequest<'run, 'session, C> {
    pub fn new(
        session: &'run mut Session<'session, C>,
        task: TaskId,
        agent: &'run mut dyn Agent,
    ) -> Self {
        Self {
            session,
            task,
            agent,
        }
    }
}

#[derive(Default)]
struct DeltaBuffer {
    kind: Option<&'static str>,
    text: String,
    chunks: usize,
}

const PERSISTED_DELTA_BATCH_BYTES: usize = 16 * 1024;

// A task deadline is a budget boundary, not permission to detach the
// execution future. Give cancellation-aware work a short, bounded window to
// unwind child agents, tools and their durable finalizers.
const DEADLINE_CLEANUP_GRACE: std::time::Duration = std::time::Duration::from_secs(1);

struct PersistentEventSink {
    db: Arc<Database>,
    run_id: String,
    delegate: Arc<dyn RuntimeEventSink>,
    state: Mutex<EventPersistenceState>,
}

struct EventPersistenceState {
    delta: DeltaBuffer,
    phase: RunPhase,
    failure: Option<String>,
    cancellation: Option<CancellationTree>,
}

impl PersistentEventSink {
    fn new(db: Arc<Database>, run_id: String, delegate: Arc<dyn RuntimeEventSink>) -> Self {
        Self {
            db,
            run_id,
            delegate,
            state: Mutex::new(EventPersistenceState {
                delta: DeltaBuffer::default(),
                phase: RunPhase::Preparing,
                failure: None,
                cancellation: None,
            }),
        }
    }

    fn bind_cancellation(&self, cancellation: CancellationTree) {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        if state.failure.is_some() {
            cancellation.cancel();
        }
        state.cancellation = Some(cancellation);
    }

    fn failure(&self) -> Option<String> {
        self.state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .failure
            .clone()
    }

    fn finalization_error(&self, error: CoreError) -> CoreError {
        match self.failure() {
            Some(failure) => CoreError::Persistence(format!(
                "{failure}; finalization also failed: {}",
                crate::redaction::scrub_secret_literals(&error.to_string())
            )),
            None => error,
        }
    }

    fn fail(
        &self,
        state: &mut EventPersistenceState,
        event_type: &str,
        error: CoreError,
    ) -> CoreError {
        let reason = state
            .failure
            .get_or_insert_with(|| {
                let reason = format!(
                    "run event {event_type} was not committed: {}",
                    crate::redaction::scrub_secret_literals(&error.to_string())
                );
                tracing::error!(run_id = self.run_id, %reason, "kernel event persistence failed");
                reason
            })
            .clone();
        if let Some(cancellation) = &state.cancellation {
            cancellation.cancel();
        }
        CoreError::Persistence(reason)
    }

    fn append(
        &self,
        state: &mut EventPersistenceState,
        phase: RunPhase,
        status: &str,
        event_type: &str,
        data: serde_json::Value,
    ) -> Result<()> {
        if let Some(failure) = &state.failure {
            return Err(CoreError::Persistence(failure.clone()));
        }
        if !matches!(phase, RunPhase::Accepted | RunPhase::Terminal)
            && phase_rank(phase) > phase_rank(state.phase)
        {
            RunStore::new(&self.db)
                .transition(&self.run_id, phase.label(), now_ms())
                .map_err(|error| self.fail(state, event_type, error))?;
            state.phase = phase;
        }
        // Product events are replayed by the UI, so bodies stay intact — but
        // secrets (bearer tokens, sk- keys, secret-named fields) must never
        // reach the database (Phase F unified redaction layer).
        let data = crate::redaction::scrub_secrets_value(data);
        RunStore::new(&self.db)
            .append_event(
                &self.run_id,
                now_ms(),
                phase.label(),
                status,
                event_type,
                &data,
            )
            .map(|_| ())
            .map_err(|error| self.fail(state, event_type, error))
    }

    fn flush_delta(&self, state: &mut EventPersistenceState) -> Result<()> {
        if let Some(failure) = &state.failure {
            return Err(CoreError::Persistence(failure.clone()));
        }
        if state.delta.text.is_empty() {
            return Ok(());
        }
        let kind = state.delta.kind.unwrap_or("content_delta_batch");
        let data = serde_json::json!({
            "text": state.delta.text, "chunks": state.delta.chunks
        });
        self.append(state, RunPhase::RunningTurn, "progress", kind, data)?;
        // Retain a rejected batch, but never automatically retry it or allow
        // later actions past the latched failure.
        state.delta = DeltaBuffer::default();
        Ok(())
    }

    fn buffer_delta(
        &self,
        state: &mut EventPersistenceState,
        kind: &'static str,
        text: &str,
    ) -> Result<()> {
        if state.delta.kind.is_some_and(|current| current != kind)
            || state.delta.text.len() >= PERSISTED_DELTA_BATCH_BYTES
        {
            self.flush_delta(state)?;
        }
        state.delta.kind = Some(kind);
        state.delta.text.push_str(text);
        state.delta.chunks += 1;
        Ok(())
    }

    // The one permitted post-failure event is an explicit, durably appended
    // failure diagnostic. It does not clear the latch or resume normal traffic.
    fn report_failure(&self, reason: String) -> Result<()> {
        let _state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        let event = RuntimeEvent::RunFailed { reason };
        RunStore::new(&self.db).append_event(
            &self.run_id,
            now_ms(),
            RunPhase::Finalizing.label(),
            "completed",
            event.label(),
            &crate::redaction::scrub_secrets_value(serde_json::to_value(&event)?),
        )?;
        self.delegate.emit(event);
        Ok(())
    }
}

impl RuntimeEventSink for PersistentEventSink {
    fn emit(&self, event: RuntimeEvent) {
        // Serialize commit and delivery together, including helper events
        // emitted concurrently with the model. Delegates must not re-enter.
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        if state.failure.is_some() {
            return;
        }
        let result = (|| -> Result<()> {
            match &event {
                RuntimeEvent::ContentDelta { text } => {
                    self.buffer_delta(&mut state, "content_delta_batch", text)?
                }
                RuntimeEvent::ReasoningDelta { text } => {
                    self.buffer_delta(&mut state, "reasoning_delta_batch", text)?;
                }
                // ModelAgent emits one sanitized Responses metadata event directly
                // before the matching ContentDelta/ReasoningDelta. Persisting that
                // per-token metadata would both duplicate the stream signal and
                // force-flush the 16 KiB text buffer on every token. It remains on
                // the live delegate path below; only the durable duplicate is
                // suppressed.
                RuntimeEvent::ResponsesStreamEvent {
                    delta_chars: Some(_),
                    ..
                } => {}
                _ => {
                    self.flush_delta(&mut state)?;
                    let (phase, status) = phase_for_event(&event);
                    if matches!(&event, RuntimeEvent::RunStarted { .. }) {
                        // Record the kernel setup boundary separately from the
                        // lifecycle event. This gives reconnecting clients a
                        // durable marker for the point at which execution became
                        // ready without introducing a second runtime pipeline.
                        let snapshot = serde_json::json!({
                            "os": std::env::consts::OS,
                            "arch": std::env::consts::ARCH,
                            "cwd": std::env::current_dir()
                                .ok()
                                .map(|path| path.to_string_lossy().into_owned()),
                        });
                        let snapshot_bytes =
                            serde_json::to_vec(&snapshot).unwrap_or_else(|_| b"{}".to_vec());
                        let snapshot_hash = format!(
                            "sha256:{:x}",
                            sha2::Sha256::digest(snapshot_bytes.as_slice())
                        );
                        self.append(
                            &mut state,
                            RunPhase::Preparing,
                            "completed",
                            "setup.completed",
                            serde_json::json!({
                                "component": "agent_kernel",
                                "environment": snapshot,
                                "snapshotHash": snapshot_hash,
                            }),
                        )?;
                    }
                    if matches!(&event, RuntimeEvent::RunCancelled { .. }) {
                        // Keep the operator intent and the runtime observation as
                        // separate durable facts. The terminal event remains the
                        // final lifecycle record for replay compatibility.
                        self.append(
                            &mut state,
                            RunPhase::Finalizing,
                            "progress",
                            "cancel.observed",
                            serde_json::json!({ "run_id": self.run_id }),
                        )?;
                    }
                    self.append(
                        &mut state,
                        phase,
                        status,
                        event.label(),
                        serde_json::to_value(&event).unwrap_or(serde_json::Value::Null),
                    )?;
                }
            }
            Ok(())
        })();
        if result.is_ok() {
            self.delegate.emit(event);
        }
    }

    fn checkpoint(&self) -> Result<()> {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        self.flush_delta(&mut state)
    }
}

fn phase_for_event(event: &RuntimeEvent) -> (RunPhase, &'static str) {
    match event {
        RuntimeEvent::RunStarted { .. } | RuntimeEvent::SessionRegistered { .. } => {
            (RunPhase::Preparing, "completed")
        }
        RuntimeEvent::McpLifecycle { .. } => (RunPhase::Preparing, "progress"),
        RuntimeEvent::TurnStarted { .. }
        | RuntimeEvent::ModelRequestStarted { .. }
        | RuntimeEvent::ModelFirstToken { .. }
        | RuntimeEvent::ModelRequestCompleted { .. }
        | RuntimeEvent::ModelAttemptReset { .. }
        | RuntimeEvent::ResponsesStreamEvent { .. }
        | RuntimeEvent::ResponsesWebSearchCall { .. }
        | RuntimeEvent::Usage { .. }
        | RuntimeEvent::CostRecorded { .. }
        | RuntimeEvent::ContextUsage { .. }
        | RuntimeEvent::ContextCompacted { .. }
        | RuntimeEvent::RelevantMemoriesInjected { .. }
        | RuntimeEvent::StallNudgeInjected { .. } => (RunPhase::RunningTurn, "progress"),
        RuntimeEvent::ToolStarted { .. }
        | RuntimeEvent::ToolCompleted { .. }
        | RuntimeEvent::ToolBlocked { .. }
        | RuntimeEvent::HookStarted { .. }
        | RuntimeEvent::HookCompleted { .. }
        | RuntimeEvent::SubagentStarted { .. }
        | RuntimeEvent::SubagentCompleted { .. }
        | RuntimeEvent::SubagentCancelled { .. }
        | RuntimeEvent::SubagentNotification { .. }
        | RuntimeEvent::WorktreeCreated { .. }
        | RuntimeEvent::WorktreeRemoved { .. }
        | RuntimeEvent::WorkflowNode { .. } => (RunPhase::ExecutingTools, "progress"),
        RuntimeEvent::Verification { .. } | RuntimeEvent::CompletionEvidence { .. } => {
            (RunPhase::Verifying, "progress")
        }
        RuntimeEvent::RunCompleted { .. }
        | RuntimeEvent::RunAwaitingApproval { .. }
        | RuntimeEvent::RunFailed { .. }
        | RuntimeEvent::RunCancelled { .. } => (RunPhase::Finalizing, "completed"),
        RuntimeEvent::ContentDelta { .. } | RuntimeEvent::ReasoningDelta { .. } => {
            (RunPhase::RunningTurn, "progress")
        }
    }
}

fn phase_rank(phase: RunPhase) -> u8 {
    match phase {
        RunPhase::Accepted => 0,
        RunPhase::Preparing => 1,
        RunPhase::RunningTurn => 2,
        RunPhase::ExecutingTools => 3,
        RunPhase::Verifying => 4,
        RunPhase::Finalizing => 5,
        RunPhase::Terminal => 6,
    }
}

pub struct AgentKernel<'a, C: Clock> {
    db: Arc<Database>,
    registry: &'a ToolRegistry,
    metrics: Metrics,
    config: RuntimeConfig,
    hooks: Option<&'a HookRegistry>,
    verification: Option<&'a VerificationPlan>,
    adversarial_verifier: Option<Arc<dyn crate::adversarial::AdversarialVerifier>>,
    approvals: Arc<dyn ApprovalGate>,
    events: Arc<PersistentEventSink>,
    handle: RunHandle,
    /// Session-level accumulation of edited files across all turns in this run.
    edited_files: Arc<
        std::sync::Mutex<std::collections::HashMap<std::path::PathBuf, crate::events::EditedFile>>,
    >,
    _clock: std::marker::PhantomData<&'a C>,
}

impl<'a, C: Clock> AgentKernel<'a, C> {
    pub fn new(
        db: Arc<Database>,
        registry: &'a ToolRegistry,
        metrics: Metrics,
        config: RuntimeConfig,
        run_id: impl Into<String>,
    ) -> Self {
        let run_id = run_id.into();
        Self {
            events: Arc::new(PersistentEventSink::new(
                db.clone(),
                run_id.clone(),
                Arc::new(NullEventSink),
            )),
            db,
            registry,
            metrics,
            config,
            hooks: None,
            verification: None,
            adversarial_verifier: None,
            approvals: Arc::new(AutoDenyGate),
            handle: RunHandle {
                run_id,
                cancellation: CancellationTree::new(),
            },
            edited_files: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            _clock: std::marker::PhantomData,
        }
    }

    pub fn handle(&self) -> RunHandle {
        self.handle.clone()
    }

    pub fn with_hooks(mut self, hooks: &'a HookRegistry) -> Self {
        self.hooks = Some(hooks);
        self
    }

    pub fn with_verification(mut self, verification: &'a VerificationPlan) -> Self {
        self.verification = Some(verification);
        self
    }

    /// Attach an advisory adversarial goal verifier (§2.2). Passed through to
    /// the [`RuntimeEngine`]; never hard-fails a run.
    pub fn with_adversarial_verifier(
        mut self,
        verifier: Arc<dyn crate::adversarial::AdversarialVerifier>,
    ) -> Self {
        self.adversarial_verifier = Some(verifier);
        self
    }

    pub fn with_approvals(mut self, approvals: Arc<dyn ApprovalGate>) -> Self {
        self.approvals = approvals;
        self
    }

    pub fn with_events(mut self, events: Arc<dyn RuntimeEventSink>) -> Self {
        self.events = Arc::new(PersistentEventSink::new(
            self.db.clone(),
            self.handle.run_id.clone(),
            events,
        ));
        self
    }

    /// The same durable event entry point used by this kernel and its agent.
    /// Bind host-owned runtime helpers after configuring `with_events` and
    /// before `run`; never wrap this sink in a second persistence layer.
    pub fn event_sink(&self) -> Arc<dyn RuntimeEventSink> {
        self.events.clone()
    }

    pub fn with_cancellation_flag(mut self, flag: Arc<std::sync::atomic::AtomicBool>) -> Self {
        self.handle.cancellation = CancellationTree::from_legacy_flag(flag);
        self
    }

    /// Attach the coordinator-owned cancellation tree. This keeps token
    /// propagation intact while `with_cancellation_flag` remains available for
    /// older embedders.
    pub fn with_cancellation(mut self, cancellation: CancellationTree) -> Self {
        self.handle.cancellation = cancellation;
        self
    }

    pub async fn start(&self, request: RunRequest<'_, 'a, C>) -> Result<KernelTerminal> {
        self.start_prepared(request.session, request.task, request.agent)
            .await
    }

    pub async fn start_prepared(
        &self,
        session: &mut Session<'a, C>,
        task: TaskId,
        agent: &mut dyn Agent,
    ) -> Result<KernelTerminal> {
        let _session_activity = session.acquire_activity()?;
        self.events
            .bind_cancellation(self.handle.cancellation.clone());
        let run_id = self.handle.run_id.clone();
        let session_id = session.id().to_string();
        let task_id = task.to_string();
        let store = RunStore::new(&self.db);
        if store.get(&run_id)?.is_none() {
            store.create(&run_id, &session_id, Some(&task_id), now_ms())?;
        }
        store.bind_task(&run_id, &session_id, &task_id)?;
        store.append_event(
            &run_id,
            now_ms(),
            RunPhase::Preparing.label(),
            "started",
            "setup.started",
            &serde_json::json!({ "component": "agent_kernel" }),
        )?;
        let capabilities: Vec<_> = self
            .registry
            .iter_specs()
            .map(|spec| {
                serde_json::json!({
                    "name": spec.descriptor.name,
                    "description": spec.descriptor.description,
                    "parameters": spec.descriptor.parameters,
                    "risk": spec.descriptor.risk,
                    "requiredPermissions": spec.descriptor.required_permissions,
                })
            })
            .collect();
        let capability_bytes = serde_json::to_vec(&capabilities).unwrap_or_else(|_| b"[]".to_vec());
        let schema_hash = format!("sha256:{:x}", sha2::Sha256::digest(&capability_bytes));
        store.append_event(
            &run_id,
            now_ms(),
            RunPhase::Preparing.label(),
            "completed",
            "capability.snapshot",
            &serde_json::json!({
                "toolCount": capabilities.len(),
                "toolSchemaHash": schema_hash,
            }),
        )?;
        store.transition(&run_id, RunPhase::Preparing.label(), now_ms())?;

        let persistent = self.events.clone();
        // Provider streaming originates inside the Agent rather than the loop
        // engine. Inject the same canonical sink here so model deltas cannot
        // bypass run_events persistence while still reaching the live UI.
        agent.set_event_sink(persistent.clone());
        let mut engine =
            RuntimeEngine::<C>::new(self.registry, self.metrics.clone(), self.config.clone())
                .with_cancel(self.handle.cancellation.legacy_flag())
                .with_cancellation_token(self.handle.cancellation.token())
                .with_events(persistent.clone())
                .with_approvals(self.approvals.clone())
                .with_edited_files(self.edited_files.clone());
        if let Some(hooks) = self.hooks {
            engine = engine.with_hooks(hooks);
        }
        if let Some(verification) = self.verification {
            engine = engine.with_verification(verification);
        }
        if let Some(verifier) = self.adversarial_verifier.clone() {
            engine = engine.with_adversarial_verifier(verifier);
        }

        let (result, deadline_reason) = if let Some(deadline) = self.config.task_timeout {
            // Keep the execution future pinned and owned by this scope. A
            // plain timeout(future) drops it on expiry, which skips
            // run_tracked's transcript/store/event finalization for a
            // foreground child.
            let mut run = Box::pin(engine.run(session, task, agent));
            tokio::select! {
                result = &mut run => (result, None),
                _ = tokio::time::sleep(deadline) => {
                    let reason = format!(
                        "run task budget exceeded its {}ms deadline",
                        deadline.as_millis()
                    );
                    // Use the canonical cancellation tree so the engine and
                    // any foreground child observe the same stop request.
                    self.handle.cancellation.cancel();
                    persistent.emit(RuntimeEvent::RunFailed {
                        reason: reason.clone(),
                    });
                    match tokio::time::timeout(DEADLINE_CLEANUP_GRACE, &mut run).await {
                        Ok(result) => {
                            // The engine records usage before returning Ok,
                            // including cooperative cancellation. Preserve
                            // that result so the error fallback cannot bill
                            // the same usage a second time.
                            let reason = if let Err(error) = &result {
                                tracing::warn!(%run_id, %error, "deadline cancellation cleanup returned an error");
                                format!("{reason}; cancellation cleanup: {error}")
                            } else {
                                reason
                            };
                            (result, Some(reason))
                        }
                        Err(_) => {
                            tracing::warn!(%run_id, "deadline cancellation cleanup grace exhausted");
                            let reason = format!("{reason}; cancellation cleanup exceeded {}ms; external work may still be stopping",
                                DEADLINE_CLEANUP_GRACE.as_millis());
                            (Err(CoreError::other(reason.clone())), Some(reason))
                        }
                    }
                }
            }
        } else {
            (engine.run(session, task, agent).await, None)
        };
        // Runtime-owned background work (for example ModelAgent prefire
        // compaction) must be settled before finalization reads side usage and
        // before the caller drops the agent. The Agent hook is deliberately
        // separate from the run outcome: a terminal model result must not be
        // rewritten merely because its speculative helper was cancelled.
        agent.finish_background_tasks().await;
        if let Some(checkpoint) = self.config.checkpoint.as_ref() {
            if let Err(error) = checkpoint.commit() {
                tracing::warn!(error = %error, "failed to persist run checkpoint");
            }
        }
        let event_result = persistent.checkpoint();
        store
            .transition(&run_id, RunPhase::Finalizing.label(), now_ms())
            .map_err(|error| persistent.finalization_error(error))?;

        let mut terminal = match result {
            Ok(RunOutcome::Completed(message)) => KernelTerminal {
                kind: TerminalKind::Succeeded,
                message: Some(message),
                reason: None,
            },
            Ok(RunOutcome::Cancelled) => KernelTerminal {
                kind: TerminalKind::Cancelled,
                message: None,
                reason: Some("cancelled by user".into()),
            },
            Ok(RunOutcome::AwaitingApproval(reason)) => KernelTerminal {
                kind: TerminalKind::Blocked,
                message: None,
                reason: Some(reason),
            },
            Ok(RunOutcome::StepLimitReached) => KernelTerminal {
                kind: TerminalKind::MaxTurns,
                message: None,
                reason: Some(match self.config.max_steps {
                    Some(limit) => format!("step limit reached ({limit} steps)"),
                    None => "step limit reached".to_string(),
                }),
            },
            Ok(RunOutcome::BudgetExceeded(reason)) => KernelTerminal {
                kind: TerminalKind::BudgetExceeded,
                message: None,
                reason: Some(reason),
            },
            Ok(RunOutcome::CompletionFailed(reason)) => KernelTerminal {
                kind: TerminalKind::Failed,
                message: None,
                reason: Some(reason),
            },
            Err(error) => {
                force_fail_active_task(session, task);
                let raw_responses_usage = agent.take_pending_raw_usage();
                if let Some(usage) = agent.cumulative_usage() {
                    let _ = session.append(deepagent_core::event::EventPayload::UsageRecorded {
                        prompt_tokens: usage.prompt_tokens,
                        completion_tokens: usage.completion_tokens,
                        reasoning_tokens: usage.reasoning_tokens,
                        total_tokens: usage.total_tokens,
                        prompt_cache_hit_tokens: usage.prompt_cache_hit_tokens,
                        prompt_cache_miss_tokens: usage.prompt_cache_miss_tokens,
                        duration_ms: 0,
                        raw_responses_usage: (!raw_responses_usage.is_empty())
                            .then_some(serde_json::Value::Array(raw_responses_usage)),
                    });
                }
                KernelTerminal {
                    kind: if self.handle.is_cancelled() {
                        TerminalKind::Cancelled
                    } else {
                        classify_terminal_error(&error)
                    },
                    message: None,
                    reason: Some(error.to_string()),
                }
            }
        };

        if let Some(reason) = deadline_reason {
            force_fail_active_task(session, task);
            terminal = KernelTerminal {
                kind: TerminalKind::BudgetExceeded,
                message: None,
                reason: Some(reason),
            };
        }

        // Persistence stops are operational failures, not user cancellation
        // or a successful model response. Keep the original cause even when
        // cancellation was needed to unwind active work.
        let mut finalization_errors = Vec::new();
        if let Err(error) = event_result {
            force_fail_active_task(session, task);
            terminal = KernelTerminal {
                kind: TerminalKind::Failed,
                message: None,
                reason: Some(error.to_string()),
            };
            if let Some(actions) = &self.config.action_persistence {
                if let Err(error) = actions.fail_unfinished(&error.to_string()) {
                    finalization_errors.push(error);
                }
            }
            if let Err(error) = persistent.report_failure(error.to_string()) {
                finalization_errors.push(error);
            }
        }
        if let Err(error) =
            persist_terminal(&self.db, &run_id, &terminal, &self.handle.cancellation)
        {
            finalization_errors.push(error);
        }
        let mut errors = finalization_errors.into_iter();
        if let Some(first) = errors.next() {
            let combined = errors.fold(first, |first, next| {
                CoreError::Persistence(format!("{first}; additional finalization failure: {next}"))
            });
            return Err(persistent.finalization_error(combined));
        }
        Ok(terminal)
    }
}

/// Shared terminal boundary for kernel execution and admission/setup failures.
pub fn persist_terminal(
    db: &Database,
    run_id: &str,
    terminal: &KernelTerminal,
    cancellation: &CancellationTree,
) -> Result<bool> {
    let changed = RunStore::new(db).finish_with_event(
        run_id,
        terminal.kind.label(),
        terminal.reason.as_deref(),
        &crate::redaction::scrub_secrets_value(serde_json::to_value(terminal)?),
        cancellation.requested_by().as_deref(),
        now_ms(),
    )?;
    deepagent_persistence::event_store::EventStore::new(db).project_run_terminal(
        run_id,
        deepagent_core::clock::Timestamp::from_millis(now_ms()),
    )?;
    Ok(changed)
}

fn classify_terminal_error(error: &CoreError) -> TerminalKind {
    if deepagent_models::classify_model_error(error)
        == deepagent_models::ModelFailureKind::ContextOverflow
    {
        return TerminalKind::ContextExhausted;
    }
    let message = error.to_string().to_ascii_lowercase();
    if message.contains("budget") || message.contains("cost limit") {
        TerminalKind::BudgetExceeded
    } else if message.contains("context length")
        || message.contains("context window")
        || message.contains("max_tokens")
        || message.contains("context exhausted")
    {
        TerminalKind::ContextExhausted
    } else {
        TerminalKind::Failed
    }
}

fn force_fail_active_task<C: Clock>(session: &mut Session<'_, C>, task: TaskId) {
    let state = session.state().task(task).map(|task| task.state);
    if state.is_some_and(|state| state.is_active()) {
        let _ = session.transition_task(task, TaskState::Failed);
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
}

#[cfg(test)]
#[path = "kernel/event_persistence_tests.rs"]
mod event_persistence_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::testing::ScriptedAgent;
    use crate::agent::{AgentDecision, Observation, RunUsage};
    use async_trait::async_trait;
    use deepagent_core::clock::FixedClock;
    use std::sync::atomic::{AtomicBool, Ordering};

    struct HangingAgent;

    struct HistoryControlProbe {
        db: Arc<Database>,
        session_id: deepagent_core::id::SessionId,
        calls: usize,
    }

    #[async_trait]
    impl crate::agent::Agent for HistoryControlProbe {
        async fn think(&mut self, _step: usize, _last: &[Observation]) -> Result<AgentDecision> {
            let error = deepagent_persistence::event_store::EventStore::new(&self.db)
                .truncate_after(self.session_id, 0)
                .unwrap_err();
            assert!(error.to_string().contains("is busy"), "{error}");
            self.calls += 1;
            Ok(AgentDecision::Complete("kept while running".into()))
        }
    }

    #[tokio::test]
    async fn direct_runtime_and_kernel_hold_history_control_until_they_return() {
        for kernel_mode in [false, true] {
            let db = Arc::new(Database::open_in_memory().unwrap());
            let clock = FixedClock::new(1);
            let mut session = Session::create(&db, &clock, None).unwrap();
            let task = session.create_task("finish").unwrap();
            let id = session.id();
            let mut agent = HistoryControlProbe {
                db: db.clone(),
                session_id: id,
                calls: 0,
            };
            let registry = ToolRegistry::new();
            // The reverse race: history control wins before runtime entry. No
            // agent step/task transition/run record may be fabricated.
            let control = deepagent_persistence::event_store::EventStore::new(&db)
                .begin_control(id)
                .unwrap();
            let kernel = AgentKernel::<FixedClock>::new(
                db.clone(),
                &registry,
                Metrics::new(),
                RuntimeConfig::default(),
                "control-probe",
            );
            let engine = RuntimeEngine::new(&registry, Metrics::new(), RuntimeConfig::default());
            let error = if kernel_mode {
                kernel
                    .start_prepared(&mut session, task, &mut agent)
                    .await
                    .unwrap_err()
            } else {
                engine
                    .run(&mut session, task, &mut agent)
                    .await
                    .unwrap_err()
            };
            assert!(error.to_string().contains("is busy"));
            assert_eq!(agent.calls, 0);
            assert_eq!(session.state().task(task).unwrap().state, TaskState::Queued);
            assert!(RunStore::new(&db).get("control-probe").unwrap().is_none());
            drop(control);
            if kernel_mode {
                let terminal = kernel
                    .start_prepared(&mut session, task, &mut agent)
                    .await
                    .unwrap();
                assert_eq!(terminal.kind, TerminalKind::Succeeded);
                assert_eq!(
                    RunStore::new(&db)
                        .events_after("control-probe", None)
                        .unwrap()
                        .iter()
                        .filter(|event| event.event_type == "run_terminal")
                        .count(),
                    1
                );
            } else {
                assert_eq!(
                    engine.run(&mut session, task, &mut agent).await.unwrap(),
                    RunOutcome::Completed("kept while running".into())
                );
            }
            assert_eq!(agent.calls, 1);
            deepagent_persistence::event_store::EventStore::new(&db)
                .truncate_after(id, 0)
                .unwrap();
        }
    }

    #[async_trait]
    impl crate::agent::Agent for HangingAgent {
        async fn think(&mut self, _step: usize, _last: &[Observation]) -> Result<AgentDecision> {
            std::future::pending().await
        }
    }

    struct OverBudgetAgent;

    struct ContextOverflowAgent;

    struct StreamingAgent {
        events: Option<Arc<dyn RuntimeEventSink>>,
    }

    #[derive(Clone, Default)]
    struct LiveEventLabels {
        labels: Arc<Mutex<Vec<&'static str>>>,
    }

    impl RuntimeEventSink for LiveEventLabels {
        fn emit(&self, event: RuntimeEvent) {
            self.labels
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(event.label());
        }
    }

    struct ToolEventAgent {
        events: Option<Arc<dyn RuntimeEventSink>>,
    }

    #[async_trait]
    impl crate::agent::Agent for ToolEventAgent {
        fn set_event_sink(&mut self, events: Arc<dyn RuntimeEventSink>) {
            self.events = Some(events);
        }

        async fn think(&mut self, _step: usize, _last: &[Observation]) -> Result<AgentDecision> {
            let events = self.events.as_ref().expect("kernel event sink");
            events.emit(RuntimeEvent::ToolStarted {
                name: "audit_tool".into(),
                call_id: "call-audit".into(),
                arguments: serde_json::json!({"value": "input"}),
                tool_kind: None,
                file_path: None,
                summary: None,
                meta: None,
            });
            events.emit(RuntimeEvent::ToolCompleted {
                name: "audit_tool".into(),
                call_id: "call-audit".into(),
                ok: true,
                output: serde_json::json!({"value": "output"}),
                duration_ms: 1,
                tool_kind: None,
                file_path: None,
                summary: None,
                meta: None,
            });
            Ok(AgentDecision::Complete("done".into()))
        }
    }

    #[async_trait]
    impl crate::agent::Agent for StreamingAgent {
        fn set_event_sink(&mut self, events: Arc<dyn RuntimeEventSink>) {
            self.events = Some(events);
        }

        async fn think(&mut self, _step: usize, _last: &[Observation]) -> Result<AgentDecision> {
            let events = self.events.as_ref().expect("kernel event sink");
            for _ in 0..100 {
                events.emit(RuntimeEvent::ResponsesStreamEvent {
                    event_type: "response.output_text.delta".into(),
                    item_id: Some("msg_1".into()),
                    item_type: None,
                    delta_chars: Some(8),
                });
                events.emit(RuntimeEvent::ContentDelta {
                    text: "12345678".into(),
                });
            }
            Ok(AgentDecision::Complete("done".into()))
        }
    }

    struct ErrorAfterUsageAgent {
        raw_usage: Vec<serde_json::Value>,
    }

    #[async_trait]
    impl crate::agent::Agent for ErrorAfterUsageAgent {
        async fn think(&mut self, _step: usize, _last: &[Observation]) -> Result<AgentDecision> {
            Err(CoreError::other("boom after provider usage"))
        }

        fn cumulative_usage(&self) -> Option<RunUsage> {
            Some(RunUsage {
                prompt_tokens: 4,
                completion_tokens: 5,
                reasoning_tokens: 3,
                total_tokens: 9,
                prompt_cache_hit_tokens: 2,
                ..RunUsage::default()
            })
        }

        fn take_pending_raw_usage(&mut self) -> Vec<serde_json::Value> {
            std::mem::take(&mut self.raw_usage)
        }
    }

    #[async_trait]
    impl crate::agent::Agent for ContextOverflowAgent {
        async fn think(&mut self, _step: usize, _last: &[Observation]) -> Result<AgentDecision> {
            Err(CoreError::provider(
                Some(413),
                Some("context_length_exceeded".into()),
                "maximum context window exceeded",
            ))
        }
    }

    #[async_trait]
    impl crate::agent::Agent for OverBudgetAgent {
        async fn think(&mut self, _step: usize, _last: &[Observation]) -> Result<AgentDecision> {
            Ok(AgentDecision::Complete("must not be accepted".into()))
        }

        fn cumulative_usage(&self) -> Option<RunUsage> {
            // The budget gate reads billed totals, so this run has spent 11
            // input tokens against a limit of 10.
            Some(RunUsage {
                prompt_tokens: 11,
                total_tokens: 11,
                billed_prompt_tokens: 11,
                ..RunUsage::default()
            })
        }
    }

    #[tokio::test]
    async fn kernel_persists_exact_terminal_state() {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let clock = FixedClock::new(1);
        let mut session = Session::create(&db, &clock, Some("v2")).unwrap();
        let task = session.create_task("finish").unwrap();
        let registry = ToolRegistry::new();
        let mut agent = ScriptedAgent::new([AgentDecision::Complete("done".into())]);
        let kernel = AgentKernel::<FixedClock>::new(
            db.clone(),
            &registry,
            Metrics::new(),
            RuntimeConfig::default(),
            "run-v2",
        );
        let terminal = kernel
            .start(RunRequest::new(&mut session, task, &mut agent))
            .await
            .unwrap();
        assert_eq!(terminal.kind, TerminalKind::Succeeded);
        let record = RunStore::new(&db).get("run-v2").unwrap().unwrap();
        assert_eq!(record.terminal_kind.as_deref(), Some("succeeded"));
        let session_events = deepagent_persistence::event_store::EventStore::new(&db)
            .load_session(session.id())
            .unwrap();
        assert_eq!(
            session_events
                .iter()
                .filter(|event| matches!(&event.payload,
                    deepagent_core::event::EventPayload::RunTerminal {
                        run_id: Some(run_id), task_id: Some(found), kind, ..
                    } if run_id == "run-v2" && *found == task && kind == "succeeded"
                ))
                .count(),
            1
        );
        let events = RunStore::new(&db).events_after("run-v2", None).unwrap();
        assert_eq!(
            events
                .iter()
                .filter(|event| event.event_type == "session_terminal_projected")
                .count(),
            1
        );
        assert!(events.len() >= 4);
        assert!(events
            .iter()
            .any(|event| event.event_type == "setup.started"));
        let setup = events
            .iter()
            .find(|event| event.event_type == "setup.completed")
            .expect("setup event");
        assert_eq!(setup.data["component"], "agent_kernel");
        assert!(setup.data["snapshotHash"]
            .as_str()
            .is_some_and(|hash| hash.starts_with("sha256:")));
        assert!(setup.data["environment"]["os"].as_str().is_some());
        let capability = events
            .iter()
            .find(|event| event.event_type == "capability.snapshot")
            .expect("capability snapshot");
        assert_eq!(capability.data["toolCount"], 0);
        assert!(capability.data["toolSchemaHash"]
            .as_str()
            .is_some_and(|hash| hash.starts_with("sha256:")));
        assert!(capability.data.get("tools").is_none());
    }

    struct FinishHookAgent {
        finished: Arc<AtomicBool>,
    }

    #[async_trait]
    impl crate::agent::Agent for FinishHookAgent {
        async fn think(&mut self, _step: usize, _last: &[Observation]) -> Result<AgentDecision> {
            Ok(AgentDecision::Complete("done".into()))
        }

        async fn finish_background_tasks(&mut self) {
            self.finished.store(true, Ordering::Release);
        }
    }

    #[tokio::test]
    async fn kernel_finishes_agent_background_tasks_before_terminal_return() {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let clock = FixedClock::new(1);
        let mut session = Session::create(&db, &clock, Some("finish-hook")).unwrap();
        let task = session.create_task("finish").unwrap();
        let registry = ToolRegistry::new();
        let finished = Arc::new(AtomicBool::new(false));
        let mut agent = FinishHookAgent {
            finished: finished.clone(),
        };
        let kernel = AgentKernel::<FixedClock>::new(
            db.clone(),
            &registry,
            Metrics::new(),
            RuntimeConfig::default(),
            "finish-hook-run",
        );

        let terminal = kernel
            .start(RunRequest::new(&mut session, task, &mut agent))
            .await
            .unwrap();

        assert_eq!(terminal.kind, TerminalKind::Succeeded);
        assert!(finished.load(Ordering::Acquire));
    }

    #[test]
    fn persistent_delta_batching_reduces_run_event_rows_by_at_least_eighty_percent() {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let clock = FixedClock::new(1);
        let session = Session::create(&db, &clock, Some("delta batching")).unwrap();
        let store = RunStore::new(&db);
        store
            .create("run-deltas", &session.id().to_string(), Some("task"), 1)
            .unwrap();
        let sink = PersistentEventSink::new(
            db.clone(),
            "run-deltas".to_string(),
            Arc::new(NullEventSink),
        );
        for _ in 0..20 {
            sink.emit(RuntimeEvent::ContentDelta {
                text: "x".repeat(1024),
            });
        }
        sink.emit(RuntimeEvent::RunCompleted {
            message: "done".into(),
            edited_files: vec![],
        });

        let events = store.events_after("run-deltas", None).unwrap();
        let delta_rows = events
            .iter()
            .filter(|event| event.event_type == "content_delta_batch")
            .count();
        assert!(delta_rows <= 4, "20 chunks produced {delta_rows} rows");
    }

    #[test]
    fn persistent_delta_batching_reduces_run_event_wal_bytes_against_previous_threshold() {
        const DELTA_CHUNKS: usize = 100;
        let directory = tempfile::tempdir().unwrap();
        let old_path = directory.path().join("old-threshold.db");
        let new_path = directory.path().join("new-threshold.db");

        let old_db = Arc::new(Database::open(&old_path).unwrap());
        let old_clock = FixedClock::new(1);
        let old_session = Session::create(&old_db, &old_clock, Some("old")).unwrap();
        let old_store = RunStore::new(&old_db);
        old_store
            .create("old-run", &old_session.id().to_string(), Some("task"), 1)
            .unwrap();
        old_db
            .with_conn(|conn| {
                conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
                    .unwrap();
                Ok(())
            })
            .unwrap();
        // Before this change the 1 KiB threshold flushed each 1 KiB delta
        // into a separate run_events row. Replay that exact write pattern.
        for _ in 0..DELTA_CHUNKS {
            old_store
                .append_event(
                    "old-run",
                    1,
                    "running_turn",
                    "progress",
                    "content_delta_batch",
                    &serde_json::json!({ "text": "x".repeat(1024), "chunks": 1 }),
                )
                .unwrap();
        }
        old_store
            .append_event(
                "old-run",
                1,
                "terminal",
                "completed",
                "run_completed",
                &serde_json::json!({ "message": "done" }),
            )
            .unwrap();

        let new_db = Arc::new(Database::open(&new_path).unwrap());
        let new_clock = FixedClock::new(1);
        let new_session = Session::create(&new_db, &new_clock, Some("new")).unwrap();
        let new_store = RunStore::new(&new_db);
        new_store
            .create("new-run", &new_session.id().to_string(), Some("task"), 1)
            .unwrap();
        new_db
            .with_conn(|conn| {
                conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
                    .unwrap();
                Ok(())
            })
            .unwrap();
        let sink = PersistentEventSink::new(
            new_db.clone(),
            "new-run".to_string(),
            Arc::new(NullEventSink),
        );
        for _ in 0..DELTA_CHUNKS {
            sink.emit(RuntimeEvent::ContentDelta {
                text: "x".repeat(1024),
            });
        }
        sink.emit(RuntimeEvent::RunCompleted {
            message: "done".into(),
            edited_files: vec![],
        });

        for (store, run_id) in [(&old_store, "old-run"), (&new_store, "new-run")] {
            let replayed = store
                .events_after(run_id, None)
                .unwrap()
                .into_iter()
                .filter(|event| event.event_type == "content_delta_batch")
                .map(|event| event.data["text"].as_str().unwrap().to_owned())
                .collect::<String>();
            assert_eq!(replayed, "x".repeat(DELTA_CHUNKS * 1024));
        }

        let wal_bytes = |path: &std::path::Path| {
            let file_name = path.file_name().unwrap().to_string_lossy();
            std::fs::metadata(path.with_file_name(format!("{file_name}-wal")))
                .unwrap()
                .len()
        };
        let old_bytes = wal_bytes(&old_path);
        let new_bytes = wal_bytes(&new_path);
        eprintln!("{DELTA_CHUNKS} KiB stream WAL: old={old_bytes}, new={new_bytes}");
        assert!(
            new_bytes * 5 <= old_bytes,
            "run_events WAL did not shrink by 80%: old={old_bytes}, new={new_bytes}"
        );
    }

    #[test]
    fn responses_delta_metadata_does_not_break_persistent_batching() {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let clock = FixedClock::new(1);
        let session = Session::create(&db, &clock, Some("real stream batching")).unwrap();
        let store = RunStore::new(&db);
        store
            .create(
                "run-stream-deltas",
                &session.id().to_string(),
                Some("task"),
                1,
            )
            .unwrap();
        let sink = PersistentEventSink::new(
            db.clone(),
            "run-stream-deltas".to_string(),
            Arc::new(NullEventSink),
        );
        for _ in 0..100 {
            sink.emit(RuntimeEvent::ResponsesStreamEvent {
                event_type: "response.output_text.delta".into(),
                item_id: Some("msg_1".into()),
                item_type: None,
                delta_chars: Some(8),
            });
            sink.emit(RuntimeEvent::ContentDelta {
                text: "12345678".into(),
            });
        }
        sink.emit(RuntimeEvent::RunCompleted {
            message: "done".into(),
            edited_files: vec![],
        });

        let events = store.events_after("run-stream-deltas", None).unwrap();
        assert_eq!(
            events
                .iter()
                .filter(|event| event.event_type == "content_delta_batch")
                .count(),
            1
        );
        assert!(!events
            .iter()
            .any(|event| event.event_type == "responses_stream_event"));
    }

    #[tokio::test]
    async fn kernel_injects_durable_sink_into_streaming_agent() {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let clock = FixedClock::new(1);
        let mut session = Session::create(&db, &clock, Some("kernel stream")).unwrap();
        let task = session.create_task("stream").unwrap();
        let registry = ToolRegistry::new();
        let mut agent = StreamingAgent { events: None };
        let kernel = AgentKernel::<FixedClock>::new(
            db.clone(),
            &registry,
            Metrics::new(),
            RuntimeConfig::default(),
            "run-kernel-stream",
        );

        let terminal = kernel
            .start(RunRequest::new(&mut session, task, &mut agent))
            .await
            .unwrap();
        assert!(terminal.succeeded());
        let events = RunStore::new(&db)
            .events_after("run-kernel-stream", None)
            .unwrap();
        let batches: Vec<_> = events
            .iter()
            .filter(|event| event.event_type == "content_delta_batch")
            .collect();
        assert_eq!(batches.len(), 1);
        assert_eq!(batches[0].data["chunks"], 100);
        assert_eq!(batches[0].data["text"].as_str().unwrap().len(), 800);
        assert!(!events
            .iter()
            .any(|event| event.event_type == "responses_stream_event"));
    }

    #[tokio::test]
    async fn audit_event_persistence_failure_does_not_split_live_and_replay() {
        let db = Arc::new(Database::open_in_memory().unwrap());
        db.with_conn(|conn| {
            conn.execute_batch(
                "CREATE TRIGGER audit_fail_tool_completed
                 BEFORE INSERT ON run_events
                 WHEN NEW.event_type='tool_completed'
                 BEGIN
                   SELECT RAISE(ABORT, 'injected tool event failure');
                 END;",
            )
            .unwrap();
            Ok(())
        })
        .unwrap();

        let clock = FixedClock::new(1);
        let mut session = Session::create(&db, &clock, Some("event persistence audit")).unwrap();
        let task = session.create_task("tool event").unwrap();
        let registry = ToolRegistry::new();
        let live = LiveEventLabels::default();
        let mut agent = ToolEventAgent { events: None };
        let kernel = AgentKernel::<FixedClock>::new(
            db.clone(),
            &registry,
            Metrics::new(),
            RuntimeConfig::default(),
            "run-event-persistence-audit",
        )
        .with_events(Arc::new(live.clone()));

        let terminal = kernel
            .start(RunRequest::new(&mut session, task, &mut agent))
            .await
            .unwrap();
        assert_eq!(terminal.kind, TerminalKind::Failed);
        assert!(terminal
            .reason
            .as_deref()
            .unwrap()
            .contains("tool_completed"));

        let live_labels = live
            .labels
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let durable_events = RunStore::new(&db)
            .events_after("run-event-persistence-audit", None)
            .unwrap();
        let durable_labels: Vec<_> = durable_events
            .iter()
            .map(|event| event.event_type.as_str())
            .collect();

        assert!(!live_labels.contains(&"tool_completed"));
        assert!(!live_labels.contains(&"run_completed"));
        assert!(
            !durable_labels.contains(&"tool_completed"),
            "live event labels={live_labels:?}, durable event labels={durable_labels:?}"
        );
        assert!(live_labels.contains(&"run_failed"));
        assert!(durable_labels.contains(&"run_failed"));
        assert_eq!(
            durable_events
                .iter()
                .filter(|event| event.event_type == "run_terminal")
                .count(),
            1
        );
        assert_eq!(
            RunStore::new(&db)
                .get("run-event-persistence-audit")
                .unwrap()
                .unwrap()
                .terminal_kind
                .as_deref(),
            Some("failed")
        );
        for (sequence, event) in durable_events.iter().enumerate() {
            assert_eq!(event.sequence, sequence as u64);
        }
    }

    #[tokio::test]
    #[ignore = "audit: terminal persistence failure must not publish a live completion"]
    async fn audit_terminal_persistence_failure_does_not_claim_live_completion() {
        let db = Arc::new(Database::open_in_memory().unwrap());
        db.with_conn(|conn| {
            conn.execute_batch(
                "CREATE TRIGGER audit_fail_run_terminal
                 BEFORE INSERT ON run_events
                 WHEN NEW.event_type='run_terminal'
                 BEGIN
                   SELECT RAISE(ABORT, 'injected terminal event failure');
                 END;",
            )
            .unwrap();
            Ok(())
        })
        .unwrap();

        let clock = FixedClock::new(1);
        let mut session = Session::create(&db, &clock, Some("terminal persistence audit")).unwrap();
        let task = session.create_task("finish").unwrap();
        let registry = ToolRegistry::new();
        let live = LiveEventLabels::default();
        let mut agent = ScriptedAgent::new([AgentDecision::Complete("done".into())]);
        let kernel = AgentKernel::<FixedClock>::new(
            db.clone(),
            &registry,
            Metrics::new(),
            RuntimeConfig::default(),
            "run-terminal-persistence-audit",
        )
        .with_events(Arc::new(live.clone()));

        let result = kernel
            .start(RunRequest::new(&mut session, task, &mut agent))
            .await;
        assert!(
            result.is_err(),
            "terminal persistence failure must be visible"
        );

        let live_labels = live
            .labels
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let durable_events = RunStore::new(&db)
            .events_after("run-terminal-persistence-audit", None)
            .unwrap();
        let durable_labels: Vec<_> = durable_events
            .iter()
            .map(|event| event.event_type.as_str())
            .collect();
        let record = RunStore::new(&db)
            .get("run-terminal-persistence-audit")
            .unwrap()
            .unwrap();

        assert!(
            !live_labels.contains(&"run_completed"),
            "live completion was published before durable terminal commit: {live_labels:?}"
        );
        assert!(!durable_labels.contains(&"run_terminal"));
        assert!(
            record.finished_at.is_none(),
            "live labels={live_labels:?}, durable event labels={durable_labels:?}, record={record:?}"
        );
    }

    #[test]
    fn persistent_event_sink_advances_run_state_without_rewinding() {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let clock = FixedClock::new(1);
        let session = Session::create(&db, &clock, Some("state")).unwrap();
        let store = RunStore::new(&db);
        store
            .create(
                "run-state",
                &session.id().to_string(),
                Some("task-state"),
                1,
            )
            .unwrap();
        store
            .transition("run-state", RunPhase::Preparing.label(), 2)
            .unwrap();

        let sink =
            PersistentEventSink::new(db.clone(), "run-state".to_string(), Arc::new(NullEventSink));
        sink.emit(RuntimeEvent::TurnStarted { step: 0 });
        assert_eq!(
            store.get("run-state").unwrap().unwrap().state,
            RunPhase::RunningTurn.label()
        );

        sink.emit(RuntimeEvent::ToolStarted {
            name: "read_file".into(),
            call_id: "call-1".into(),
            arguments: serde_json::json!({"path":"README.md"}),
            tool_kind: None,
            file_path: None,
            summary: None,
            meta: None,
        });
        assert_eq!(
            store.get("run-state").unwrap().unwrap().state,
            RunPhase::ExecutingTools.label()
        );

        sink.emit(RuntimeEvent::ModelRequestCompleted {
            step: 1,
            elapsed_ms: 12,
        });
        assert_eq!(
            store.get("run-state").unwrap().unwrap().state,
            RunPhase::ExecutingTools.label()
        );

        sink.emit(RuntimeEvent::RunCompleted {
            message: "done".into(),
            edited_files: vec![],
        });
        assert_eq!(
            store.get("run-state").unwrap().unwrap().state,
            RunPhase::Finalizing.label()
        );
    }

    #[tokio::test]
    async fn kernel_deadline_finalizes_and_fails_active_task() {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let clock = FixedClock::new(1);
        let mut session = Session::create(&db, &clock, Some("deadline")).unwrap();
        let task = session.create_task("hang").unwrap();
        let registry = ToolRegistry::new();
        let mut agent = HangingAgent;
        let config = RuntimeConfig {
            task_timeout: Some(std::time::Duration::from_millis(25)),
            ..RuntimeConfig::default()
        };
        let kernel = AgentKernel::<FixedClock>::new(
            db.clone(),
            &registry,
            Metrics::new(),
            config,
            "run-deadline",
        );
        let control = kernel.handle();
        let started = std::time::Instant::now();
        let terminal = kernel
            .start(RunRequest::new(&mut session, task, &mut agent))
            .await
            .unwrap();
        assert_eq!(terminal.kind, TerminalKind::BudgetExceeded);
        assert!(control.is_cancelled());
        assert!(terminal
            .reason
            .as_deref()
            .unwrap()
            .contains("cleanup exceeded"));
        assert!(started.elapsed() < std::time::Duration::from_secs(3));
        assert_eq!(session.state().task(task).unwrap().state, TaskState::Failed);
        let record = RunStore::new(&db).get("run-deadline").unwrap().unwrap();
        assert_eq!(record.terminal_kind.as_deref(), Some("budget_exceeded"));
    }

    #[tokio::test]
    async fn token_budget_prevents_tool_or_completion_commit() {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let clock = FixedClock::new(1);
        let mut session = Session::create(&db, &clock, Some("tokens")).unwrap();
        let task = session.create_task("over budget").unwrap();
        let registry = ToolRegistry::new();
        let mut agent = OverBudgetAgent;
        let config = RuntimeConfig {
            max_total_tokens: Some(10),
            ..RuntimeConfig::default()
        };
        let kernel = AgentKernel::<FixedClock>::new(
            db.clone(),
            &registry,
            Metrics::new(),
            config,
            "run-token-budget",
        );

        let terminal = kernel
            .start(RunRequest::new(&mut session, task, &mut agent))
            .await
            .unwrap();
        assert_eq!(terminal.kind, TerminalKind::BudgetExceeded);
        assert!(terminal.reason.unwrap().contains("run budget exceeded"));
        assert_eq!(session.state().task(task).unwrap().state, TaskState::Failed);
    }

    #[tokio::test]
    async fn h45_deadline_cleanup_records_usage_once() {
        struct UsageOnCancel;

        #[async_trait]
        impl Agent for UsageOnCancel {
            async fn think(
                &mut self,
                _step: usize,
                _last: &[Observation],
            ) -> Result<AgentDecision> {
                std::future::pending().await
            }

            async fn think_cancelled(
                &mut self,
                _step: usize,
                _last: &[Observation],
                cancel: Option<Arc<AtomicBool>>,
            ) -> Result<AgentDecision> {
                crate::cancellation::wait_for_legacy_flag(cancel.unwrap()).await;
                Err(CoreError::other("H-45 usage fixture cancelled"))
            }

            fn cumulative_usage(&self) -> Option<RunUsage> {
                Some(RunUsage {
                    prompt_tokens: 10,
                    completion_tokens: 5,
                    total_tokens: 15,
                    billed_prompt_tokens: 10,
                    ..Default::default()
                })
            }
        }

        let db = Arc::new(Database::open_in_memory().unwrap());
        let clock = FixedClock::new(1);
        let mut session = Session::create(&db, &clock, Some("H-45 usage")).unwrap();
        let task = session.create_task("cancel after usage").unwrap();
        let registry = ToolRegistry::new();
        let kernel = AgentKernel::new(
            db.clone(),
            &registry,
            Default::default(),
            RuntimeConfig {
                task_timeout: Some(std::time::Duration::from_millis(25)),
                ..Default::default()
            },
            "h45-usage",
        );
        let terminal = kernel
            .start(RunRequest::new(&mut session, task, &mut UsageOnCancel))
            .await
            .unwrap();
        assert_eq!(terminal.kind, TerminalKind::BudgetExceeded);
        let events = deepagent_persistence::event_store::EventStore::new(&db)
            .load_session(session.id())
            .unwrap();
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(
                    event.payload,
                    deepagent_core::event::EventPayload::UsageRecorded { .. }
                ))
                .count(),
            1,
            "cooperative engine cleanup already recorded usage; the kernel must not duplicate it"
        );
    }

    #[tokio::test]
    async fn structured_context_overflow_persists_context_exhausted_terminal() {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let clock = FixedClock::new(1);
        let mut session = Session::create(&db, &clock, Some("context overflow")).unwrap();
        let task = session.create_task("too much context").unwrap();
        let registry = ToolRegistry::new();
        let mut agent = ContextOverflowAgent;
        let kernel = AgentKernel::<FixedClock>::new(
            db.clone(),
            &registry,
            Metrics::new(),
            RuntimeConfig::default(),
            "run-context-overflow",
        );

        let terminal = kernel
            .start(RunRequest::new(&mut session, task, &mut agent))
            .await
            .unwrap();

        assert_eq!(terminal.kind, TerminalKind::ContextExhausted);
        assert_eq!(session.state().task(task).unwrap().state, TaskState::Failed);
        let record = RunStore::new(&db)
            .get("run-context-overflow")
            .unwrap()
            .unwrap();
        assert_eq!(record.terminal_kind.as_deref(), Some("context_exhausted"));
    }

    #[tokio::test]
    async fn error_path_persists_raw_responses_usage() {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let clock = FixedClock::new(1);
        let mut session = Session::create(&db, &clock, Some("usage error")).unwrap();
        let task = session.create_task("fail after usage").unwrap();
        let registry = ToolRegistry::new();
        let mut agent = ErrorAfterUsageAgent {
            raw_usage: vec![serde_json::json!({
                "input_tokens": 4,
                "input_tokens_details": {"cached_tokens": 2},
                "output_tokens": 5,
                "output_tokens_details": {"reasoning_tokens": 3},
                "total_tokens": 9
            })],
        };
        let kernel = AgentKernel::<FixedClock>::new(
            db.clone(),
            &registry,
            Metrics::new(),
            RuntimeConfig::default(),
            "run-usage-error",
        );

        let terminal = kernel
            .start(RunRequest::new(&mut session, task, &mut agent))
            .await
            .unwrap();

        assert_eq!(terminal.kind, TerminalKind::Failed);
        let usage = deepagent_persistence::event_store::EventStore::new(&db)
            .load_session(session.id())
            .unwrap()
            .into_iter()
            .find_map(|event| match event.payload {
                deepagent_core::event::EventPayload::UsageRecorded {
                    raw_responses_usage,
                    ..
                } => raw_responses_usage,
                _ => None,
            })
            .expect("usage event should carry raw responses usage");
        assert_eq!(usage[0]["input_tokens"], 4);
        assert_eq!(usage[0]["output_tokens_details"]["reasoning_tokens"], 3);
    }
}
