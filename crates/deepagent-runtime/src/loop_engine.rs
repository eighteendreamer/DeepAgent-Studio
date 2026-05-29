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

/// Why a run stopped.
#[derive(Debug, Clone, PartialEq)]
pub enum RunOutcome {
    /// The agent declared completion with a final message.
    Completed(String),
    /// The agent requested human approval and the loop yielded.
    AwaitingApproval(String),
    /// The configured maximum number of steps was reached.
    StepLimitReached,
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
            _clock: std::marker::PhantomData,
        }
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

        // SessionStart hook (observational).
        self.fire_hook(session_id, HookPoint::SessionStart, HookData::None)
            .await?;

        // Move the task into Running (validated by the session).
        if session.state().task(task).map(|t| t.state) == Some(TaskState::Queued) {
            session.transition_task(task, TaskState::Running)?;
        }

        let mut last_observation: Option<Observation> = None;
        let mut outcome = RunOutcome::StepLimitReached;
        let mut finished = false;

        // Verification state persists across attempts (tracks loop detection).
        let mut reflection_engine = self
            .verification
            .map(|p| ReflectionEngine::new(p.max_repeats, p.max_attempts));

        for step in 0..self.config.max_steps {
            let decision = agent.think(step, last_observation.as_ref()).await?;
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
                                last_observation = Some(obs);
                                continue;
                            }
                        }
                    }

                    session.append(EventPayload::MessageAppended {
                        message: Message::assistant(&msg),
                    })?;
                    session.transition_task(task, TaskState::Completed)?;
                    outcome = RunOutcome::Completed(msg);
                    finished = true;
                    break;
                }

                AgentDecision::NeedsApproval(msg) => {
                    session.append(EventPayload::Note {
                        text: format!("approval requested: {msg}"),
                    })?;
                    session.transition_task(task, TaskState::WaitingApproval)?;
                    outcome = RunOutcome::AwaitingApproval(msg);
                    finished = true;
                    break;
                }

                AgentDecision::CallTool(invocation) => {
                    last_observation =
                        Some(self.execute_tool(session, session_id, invocation).await?);
                }
            }
        }

        if !finished {
            // Ran out of steps: mark failed so the task does not linger.
            session.transition_task(task, TaskState::Failed)?;
        }

        // SessionEnd hook (observational). The session stays open for resumption
        // unless the caller ends it; this just notifies hooks the run finished.
        self.fire_hook(session_id, HookPoint::SessionEnd, HookData::None)
            .await?;

        Ok(outcome)
    }

    /// Execute one tool invocation, recording request + completion events and
    /// returning the [`Observation`] to feed back to the agent.
    async fn execute_tool(
        &self,
        session: &mut Session<'a, C>,
        session_id: deepagent_core::id::SessionId,
        invocation: deepagent_tools::ToolInvocation,
    ) -> Result<Observation> {
        let call_id = format!("call_{}", deepagent_core::id::EventId::new());
        let tool_name = invocation.name.clone();

        // BeforeToolUse gate: a hook may veto the call. A veto becomes a failed
        // observation (recorded to the log) so the agent can react, rather than
        // aborting the whole run.
        let before = self
            .fire_hook(
                session_id,
                HookPoint::BeforeToolUse,
                HookData::before_tool(tool_name.clone(), invocation.arguments.clone()),
            )
            .await?;
        if let HookOutcome::Deny(reason) = before {
            self.metrics.incr(names::TOOL_FAILURES, 1);
            let err_value = serde_json::json!({ "error": format!("blocked by hook: {reason}") });
            session.append(EventPayload::ToolCallRequested {
                call: ToolCall {
                    id: call_id.clone(),
                    name: tool_name.clone(),
                    arguments: invocation.arguments.clone(),
                },
            })?;
            session.append(EventPayload::ToolCallCompleted {
                call_id,
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
                session.append(EventPayload::ToolCallCompleted {
                    call_id,
                    ok: out.ok,
                    output: out.value.clone(),
                    duration_ms,
                })?;
                Observation {
                    tool: tool_name.clone(),
                    ok: out.ok,
                    output: out.value,
                }
            }
            Err(e) => {
                // Tool was denied or failed to run: surface as a failed
                // observation rather than aborting the whole run, so the agent
                // can reflect / choose an alternative.
                self.metrics.incr(names::TOOL_FAILURES, 1);
                let err_value = serde_json::json!({ "error": e.to_string() });
                session.append(EventPayload::ToolCallCompleted {
                    call_id,
                    ok: false,
                    output: err_value.clone(),
                    duration_ms,
                })?;
                Observation {
                    tool: tool_name.clone(),
                    ok: false,
                    output: err_value,
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
                _last: Option<&Observation>,
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
}
