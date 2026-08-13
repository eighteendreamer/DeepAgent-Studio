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

struct PersistentEventSink {
    db: Arc<Database>,
    run_id: String,
    delegate: Arc<dyn RuntimeEventSink>,
    delta: Mutex<DeltaBuffer>,
    phase: Mutex<RunPhase>,
}

impl PersistentEventSink {
    fn new(db: Arc<Database>, run_id: String, delegate: Arc<dyn RuntimeEventSink>) -> Self {
        Self {
            db,
            run_id,
            delegate,
            delta: Mutex::new(DeltaBuffer::default()),
            phase: Mutex::new(RunPhase::Preparing),
        }
    }

    fn append(&self, phase: RunPhase, status: &str, event_type: &str, data: serde_json::Value) {
        self.transition_if_advanced(phase);
        // Product events are replayed by the UI, so bodies stay intact — but
        // secrets (bearer tokens, sk- keys, secret-named fields) must never
        // reach the database (Phase F unified redaction layer).
        let data = crate::redaction::scrub_secrets_value(data);
        if let Err(error) = RunStore::new(&self.db).append_event(
            &self.run_id,
            now_ms(),
            phase.label(),
            status,
            event_type,
            &data,
        ) {
            tracing::warn!(%error, run_id = self.run_id, "failed to persist kernel event");
        }
    }

    fn transition_if_advanced(&self, phase: RunPhase) {
        if matches!(phase, RunPhase::Accepted | RunPhase::Terminal) {
            return;
        }
        let should_transition = {
            let mut current = self.phase.lock().unwrap_or_else(|p| p.into_inner());
            if phase_rank(phase) <= phase_rank(*current) {
                false
            } else {
                *current = phase;
                true
            }
        };
        if !should_transition {
            return;
        }
        if let Err(error) =
            RunStore::new(&self.db).transition(&self.run_id, phase.label(), now_ms())
        {
            tracing::warn!(
                %error,
                run_id = self.run_id,
                phase = phase.label(),
                "failed to transition kernel run phase"
            );
        }
    }

    fn flush_delta(&self) {
        let buffered = {
            let mut guard = self.delta.lock().unwrap_or_else(|p| p.into_inner());
            if guard.text.is_empty() {
                return;
            }
            std::mem::take(&mut *guard)
        };
        self.append(
            RunPhase::RunningTurn,
            "progress",
            buffered.kind.unwrap_or("content_delta_batch"),
            serde_json::json!({ "text": buffered.text, "chunks": buffered.chunks }),
        );
    }

    fn buffer_delta(&self, kind: &'static str, text: &str) {
        let should_flush = {
            let guard = self.delta.lock().unwrap_or_else(|p| p.into_inner());
            guard.kind.is_some_and(|current| current != kind) || guard.text.len() >= 1024
        };
        if should_flush {
            self.flush_delta();
        }
        let mut guard = self.delta.lock().unwrap_or_else(|p| p.into_inner());
        guard.kind = Some(kind);
        guard.text.push_str(text);
        guard.chunks += 1;
    }
}

impl RuntimeEventSink for PersistentEventSink {
    fn emit(&self, event: RuntimeEvent) {
        match &event {
            RuntimeEvent::ContentDelta { text } => self.buffer_delta("content_delta_batch", text),
            RuntimeEvent::ReasoningDelta { text } => {
                self.buffer_delta("reasoning_delta_batch", text)
            }
            _ => {
                self.flush_delta();
                let (phase, status) = phase_for_event(&event);
                self.append(
                    phase,
                    status,
                    event.label(),
                    serde_json::to_value(&event).unwrap_or(serde_json::Value::Null),
                );
            }
        }
        self.delegate.emit(event);
    }
}

fn phase_for_event(event: &RuntimeEvent) -> (RunPhase, &'static str) {
    match event {
        RuntimeEvent::RunStarted { .. } | RuntimeEvent::SessionRegistered { .. } => {
            (RunPhase::Preparing, "completed")
        }
        RuntimeEvent::TurnStarted { .. }
        | RuntimeEvent::ModelRequestStarted { .. }
        | RuntimeEvent::ModelFirstToken { .. }
        | RuntimeEvent::ModelRequestCompleted { .. }
        | RuntimeEvent::ModelAttemptReset { .. }
        | RuntimeEvent::ResponsesStreamEvent { .. }
        | RuntimeEvent::ResponsesWebSearchCall { .. }
        | RuntimeEvent::Usage { .. }
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
        | RuntimeEvent::WorktreeRemoved { .. } => (RunPhase::ExecutingTools, "progress"),
        RuntimeEvent::Verification { .. } | RuntimeEvent::CompletionEvidence { .. } => {
            (RunPhase::Verifying, "progress")
        }
        RuntimeEvent::RunCompleted { .. }
        | RuntimeEvent::RunAwaitingApproval { .. }
        | RuntimeEvent::RunFailed { .. }
        | RuntimeEvent::RunCancelled => (RunPhase::Finalizing, "completed"),
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
    events: Arc<dyn RuntimeEventSink>,
    handle: RunHandle,
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
            db,
            registry,
            metrics,
            config,
            hooks: None,
            verification: None,
            adversarial_verifier: None,
            approvals: Arc::new(AutoDenyGate),
            events: Arc::new(NullEventSink),
            handle: RunHandle {
                run_id,
                cancellation: CancellationTree::new(),
            },
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
        self.events = events;
        self
    }

    pub fn with_cancellation_flag(mut self, flag: Arc<std::sync::atomic::AtomicBool>) -> Self {
        self.handle.cancellation = CancellationTree::from_legacy_flag(flag);
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
        let run_id = self.handle.run_id.clone();
        let session_id = session.id().to_string();
        let task_id = task.to_string();
        let store = RunStore::new(&self.db);
        if store.get(&run_id)?.is_none() {
            store.create(&run_id, &session_id, Some(&task_id), now_ms())?;
        }
        store.append_event(
            &run_id,
            now_ms(),
            RunPhase::Accepted.label(),
            "completed",
            "run_accepted",
            &serde_json::json!({ "session_id": session_id, "task_id": task_id }),
        )?;
        store.transition(&run_id, RunPhase::Preparing.label(), now_ms())?;

        let persistent = Arc::new(PersistentEventSink::new(
            self.db.clone(),
            run_id.clone(),
            self.events.clone(),
        ));
        let mut engine =
            RuntimeEngine::<C>::new(self.registry, self.metrics.clone(), self.config.clone())
                .with_cancel(self.handle.cancellation.legacy_flag())
                .with_events(persistent.clone())
                .with_approvals(self.approvals.clone());
        if let Some(hooks) = self.hooks {
            engine = engine.with_hooks(hooks);
        }
        if let Some(verification) = self.verification {
            engine = engine.with_verification(verification);
        }
        if let Some(verifier) = self.adversarial_verifier.clone() {
            engine = engine.with_adversarial_verifier(verifier);
        }

        let result = if let Some(deadline) = self.config.task_timeout {
            match tokio::time::timeout(deadline, engine.run(session, task, agent)).await {
                Ok(result) => result,
                Err(_) => {
                    let reason = format!(
                        "run task budget exceeded its {}ms deadline",
                        deadline.as_millis()
                    );
                    persistent.emit(RuntimeEvent::RunFailed {
                        reason: reason.clone(),
                    });
                    Err(CoreError::other(reason))
                }
            }
        } else {
            engine.run(session, task, agent).await
        };
        if let Some(checkpoint) = self.config.checkpoint.as_ref() {
            if let Err(error) = checkpoint.commit() {
                tracing::warn!(error = %error, "failed to persist run checkpoint");
            }
        }
        store.transition(&run_id, RunPhase::Finalizing.label(), now_ms())?;

        let terminal = match result {
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
                reason: Some(format!("step limit reached ({})", self.config.max_steps)),
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
                if let Some(usage) = agent.cumulative_usage() {
                    let _ = session.append(deepagent_core::event::EventPayload::UsageRecorded {
                        prompt_tokens: usage.prompt_tokens,
                        completion_tokens: usage.completion_tokens,
                        reasoning_tokens: usage.reasoning_tokens,
                        total_tokens: usage.total_tokens,
                        prompt_cache_hit_tokens: usage.prompt_cache_hit_tokens,
                        prompt_cache_miss_tokens: usage.prompt_cache_miss_tokens,
                        duration_ms: 0,
                        raw_responses_usage: None,
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

        persistent.flush_delta();
        store.append_event(
            &run_id,
            now_ms(),
            RunPhase::Terminal.label(),
            "completed",
            "run_terminal",
            &crate::redaction::scrub_secrets_value(
                serde_json::to_value(&terminal).unwrap_or(serde_json::Value::Null),
            ),
        )?;
        store.finish(
            &run_id,
            terminal.kind.label(),
            terminal.reason.as_deref(),
            now_ms(),
        )?;
        Ok(terminal)
    }
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
mod tests {
    use super::*;
    use crate::agent::testing::ScriptedAgent;
    use crate::agent::{AgentDecision, Observation, RunUsage};
    use async_trait::async_trait;
    use deepagent_core::clock::FixedClock;

    struct HangingAgent;

    #[async_trait]
    impl crate::agent::Agent for HangingAgent {
        async fn think(&mut self, _step: usize, _last: &[Observation]) -> Result<AgentDecision> {
            std::future::pending().await
        }
    }

    struct OverBudgetAgent;

    struct ContextOverflowAgent;

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
            Some(RunUsage {
                total_tokens: 11,
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
        assert!(
            RunStore::new(&db)
                .events_after("run-v2", None)
                .unwrap()
                .len()
                >= 4
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

        let terminal = kernel
            .start(RunRequest::new(&mut session, task, &mut agent))
            .await
            .unwrap();
        assert_eq!(terminal.kind, TerminalKind::BudgetExceeded);
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
        assert!(terminal.reason.unwrap().contains("used 11 tokens"));
        assert_eq!(session.state().task(task).unwrap().state, TaskState::Failed);
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
}
