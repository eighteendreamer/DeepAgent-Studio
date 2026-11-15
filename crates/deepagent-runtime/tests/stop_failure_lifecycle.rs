//! H-117: terminal provider failures notify once without replacing the cause.

use async_trait::async_trait;
use deepagent_core::{
    clock::FixedClock,
    error::{CoreError, Result},
    task::TaskState,
};
use deepagent_hooks::{
    Hook, HookContext, HookData, HookOutcome, HookPoint, HookRegistry, StopFailureError,
};
use deepagent_persistence::Database;
use deepagent_runtime::{
    Agent, AgentDecision, AgentKernel, CancellationTree, Observation, RunRequest, RuntimeConfig,
    TerminalKind,
};
use deepagent_session::Session;
use deepagent_tools::ToolRegistry;
use deepagent_tracing::metrics::Metrics;
use std::sync::{Arc, Mutex};

const PROVIDER_FAILURE: &str = "synthetic provider rejection";

struct FailingAgent {
    error: CoreError,
    cancellation: Option<CancellationTree>,
}

#[async_trait]
impl Agent for FailingAgent {
    async fn think(&mut self, _: usize, _: &[Observation]) -> Result<AgentDecision> {
        if let Some(cancellation) = &self.cancellation {
            cancellation.cancel();
        }
        Err(std::mem::replace(
            &mut self.error,
            CoreError::other("agent was called more than once"),
        ))
    }
}

struct StopFailureRecorder {
    calls: Arc<Mutex<Vec<String>>>,
    data: Arc<Mutex<Vec<HookData>>>,
    fail: bool,
}

#[async_trait]
impl Hook for StopFailureRecorder {
    fn name(&self) -> &str {
        "h117-stop-failure-recorder"
    }

    async fn run(&self, ctx: &HookContext) -> Result<HookOutcome> {
        if ctx.point == HookPoint::StopFailure {
            self.calls.lock().unwrap().push(ctx.point.label().into());
            self.data.lock().unwrap().push(ctx.data.clone());
            if self.fail {
                return Err(CoreError::other("synthetic StopFailure hook error"));
            }
        }
        Ok(HookOutcome::Continue)
    }
}

async fn run(
    error: CoreError,
    cancel: Option<CancellationTree>,
    hook_fails: bool,
) -> (String, Vec<String>, Vec<HookData>) {
    let db = Arc::new(Database::open_in_memory().unwrap());
    let clock = FixedClock::new(1);
    let run_id = "h117-provider-terminal";
    let mut session = Session::create(&db, &clock, Some(run_id)).unwrap();
    let task = session.create_task(run_id).unwrap();
    let registry = ToolRegistry::new();
    let calls = Arc::new(Mutex::new(Vec::new()));
    let data = Arc::new(Mutex::new(Vec::new()));
    let mut hooks = HookRegistry::new();
    hooks.register(
        HookPoint::StopFailure,
        Arc::new(StopFailureRecorder {
            calls: calls.clone(),
            data: data.clone(),
            fail: hook_fails,
        }),
    );
    let mut agent = FailingAgent {
        error,
        cancellation: cancel.clone(),
    };
    let mut kernel = AgentKernel::new(
        db.clone(),
        &registry,
        Metrics::new(),
        RuntimeConfig::default(),
        run_id,
    )
    .with_hooks(&hooks);
    if let Some(cancellation) = cancel {
        kernel = kernel.with_cancellation(cancellation);
    }
    let terminal = kernel
        .start(RunRequest::new(&mut session, task, &mut agent))
        .await
        .unwrap();
    assert_eq!(session.state().task(task).unwrap().state, TaskState::Failed);
    assert!(matches!(
        terminal.kind,
        TerminalKind::Failed | TerminalKind::Cancelled
    ));
    let calls = calls.lock().unwrap().clone();
    let data = data.lock().unwrap().clone();
    (terminal.reason.unwrap_or_default(), calls, data)
}

#[tokio::test]
async fn h117_terminal_provider_error_dispatches_stop_failure_once() {
    let (reason, calls, _) = run(
        CoreError::provider(
            Some(400),
            Some("invalid_request_error".into()),
            PROVIDER_FAILURE,
        ),
        None,
        false,
    )
    .await;
    assert!(reason.contains(PROVIDER_FAILURE));
    assert_eq!(calls, [HookPoint::StopFailure.label()]);
}

#[tokio::test]
async fn h117_agent_errors_and_cancellation_do_not_dispatch_stop_failure() {
    let (reason, calls, _) = run(CoreError::other("synthetic agent failure"), None, false).await;
    assert!(reason.contains("synthetic agent failure"));
    assert!(calls.is_empty());

    let cancellation = CancellationTree::new();
    let (reason, calls, _) = run(
        CoreError::other("synthetic cancelled provider call"),
        Some(cancellation),
        false,
    )
    .await;
    assert_eq!(reason, "cancelled by user");
    assert!(calls.is_empty());
}

#[tokio::test]
async fn h117_stop_failure_hook_error_preserves_provider_failure_terminal() {
    let (reason, calls, _) = run(
        CoreError::provider(None, Some("server_error".into()), PROVIDER_FAILURE),
        None,
        true,
    )
    .await;
    assert!(reason.contains(PROVIDER_FAILURE));
    assert_eq!(calls, [HookPoint::StopFailure.label()]);
}

#[tokio::test]
async fn h106_terminal_provider_failure_keeps_structured_facts_and_legacy_content() {
    for present in [false, true] {
        let status = present.then_some(429);
        let code = present.then(|| "rate_limit_error".to_string());
        let retry_after_ms = present.then_some(1_234);
        let request_id = present.then(|| "synthetic-request".to_string());
        let error = CoreError::provider_with_metadata(
            status,
            code.clone(),
            PROVIDER_FAILURE,
            retry_after_ms,
            request_id.clone(),
        );
        let legacy_content = format!("\n\nStop failure reason: {error}");
        let (reason, calls, data) = run(error, None, false).await;
        assert!(reason.contains(PROVIDER_FAILURE));
        assert_eq!(calls, [HookPoint::StopFailure.label()]);
        assert_eq!(
            data,
            [HookData::StopFailure {
                content: legacy_content,
                error: StopFailureError::Provider {
                    status,
                    code,
                    message: PROVIDER_FAILURE.to_string(),
                    retry_after_ms,
                    request_id,
                },
            }]
        );
    }
}
