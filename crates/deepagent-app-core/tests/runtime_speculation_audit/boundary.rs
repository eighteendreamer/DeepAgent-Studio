//! A public Agent cannot publish native state through attempt preparation.
use super::*;
use deepagent_runtime::{Agent, AgentDecision, Observation, ToolAttemptController};
use deepagent_tools::{ToolDescriptor, ToolInvocation, ToolOutput};
use std::sync::atomic::AtomicBool;
use tokio::sync::Notify;

const RENAMED: &str = "h233_safe_state_capability";

struct StateTool {
    todo: TodoWriteTool,
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl Tool for StateTool {
    fn descriptor(&self) -> ToolDescriptor {
        let mut descriptor = self.todo.descriptor();
        descriptor.name = RENAMED.into();
        descriptor
    }

    async fn invoke(&self, args: Value) -> Result<ToolOutput> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.todo.invoke(args).await
    }
}

#[derive(Clone, Copy, Debug)]
enum Decision {
    Accept,
    Reject,
    Retry,
    Wait,
}

struct PublicAgent {
    decision: Decision,
    store: TodoStore,
    calls: Arc<AtomicUsize>,
    entered: Arc<Notify>,
    dropped: Arc<AtomicBool>,
}

struct WaitGuard(Arc<AtomicBool>);
impl Drop for WaitGuard {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

#[async_trait]
impl Agent for PublicAgent {
    async fn think(&mut self, _: usize, _: &[Observation]) -> Result<AgentDecision> {
        unreachable!("this fixture exercises the public streaming controller")
    }

    async fn think_streaming_cancelled(
        &mut self,
        step: usize,
        last: &[Observation],
        cancel: Option<Arc<AtomicBool>>,
        tools: Option<&mut dyn ToolAttemptController>,
    ) -> Result<AgentDecision> {
        if step != 0 {
            assert_eq!(last.len(), 1);
            assert!(last[0].ok);
            assert_eq!(self.calls.load(Ordering::SeqCst), 1);
            return Ok(AgentDecision::Complete("accepted once".into()));
        }
        let tools = tools.unwrap();
        let call = ToolInvocation::new(RENAMED, json!({"todos":[todo(CANARY)]})).with_id(CALL);
        tools.begin(1);
        tools.prepare(call.clone());
        tools.prepare(call.clone());
        for _ in 0..100 {
            tokio::task::yield_now().await;
        }
        assert_eq!(self.calls.load(Ordering::SeqCst), 0);
        assert_eq!(self.store.snapshot(), vec![todo("original task")]);
        assert!(!self.store.is_snapshot_pending());
        match self.decision {
            Decision::Accept => {
                tools.commit(1);
                assert_eq!(self.calls.load(Ordering::SeqCst), 0);
                Ok(AgentDecision::CallTool(call))
            }
            Decision::Reject => {
                tools.abort(1, "controlled public agent failure");
                Err(CoreError::other("controlled public agent failure"))
            }
            Decision::Retry => {
                tools.abort(1, "controlled public agent retry");
                tools.begin(2);
                tools.commit(2);
                Ok(AgentDecision::Complete("retry without tool".into()))
            }
            Decision::Wait => {
                let _guard = WaitGuard(self.dropped.clone());
                self.entered.notify_one();
                let cancel = cancel.expect("cancellable public agent");
                while !cancel.load(Ordering::Acquire) {
                    tokio::task::yield_now().await;
                }
                tools.abort(1, "controlled cancellation");
                Err(CoreError::other("controlled cancellation"))
            }
        }
    }
}

async fn exercise(decision: Decision, drop_wait: bool) {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("public.sqlite");
    let db = Database::open(&path).unwrap();
    let clock = FixedClock::new(1);
    let mut session = Session::create(&db, &clock, Some("H233 public boundary")).unwrap();
    let session_id = session.id();
    let task = session.create_task("observe attempt boundary").unwrap();
    let store = TodoStore::new();
    store.replace(vec![todo("original task")]);
    let calls = Arc::new(AtomicUsize::new(0));
    let tool = Arc::new(StateTool {
        todo: TodoWriteTool::new(store.clone()),
        calls: calls.clone(),
    });
    assert!(tool.is_concurrency_safe(&json!({})));
    assert_eq!(
        tool.descriptor().required_permissions,
        PermissionSet::read_only()
    );
    let mut registry = ToolRegistry::new();
    registry.register(tool).unwrap();
    let cancel = Arc::new(AtomicBool::new(false));
    let engine = RuntimeEngine::new(&registry, Metrics::new(), RuntimeConfig::default())
        .with_cancel(cancel.clone());
    let entered = Arc::new(Notify::new());
    let dropped = Arc::new(AtomicBool::new(false));
    let mut agent = PublicAgent {
        decision,
        store: store.clone(),
        calls: calls.clone(),
        entered: entered.clone(),
        dropped: dropped.clone(),
    };
    let mut run = Box::pin(engine.run(&mut session, task, &mut agent));
    if matches!(decision, Decision::Wait) {
        tokio::time::timeout(Duration::from_secs(2), async {
            tokio::select! {
                _ = entered.notified() => {},
                result = &mut run => panic!("must reach pending attempt: {result:?}"),
            }
        })
        .await
        .unwrap();
        if !drop_wait {
            cancel.store(true, Ordering::Release);
            assert_eq!(
                tokio::time::timeout(Duration::from_secs(2), &mut run)
                    .await
                    .unwrap()
                    .unwrap(),
                RunOutcome::Cancelled
            );
        }
        drop(run);
        assert!(dropped.load(Ordering::SeqCst));
    } else {
        let result = tokio::time::timeout(Duration::from_secs(2), run)
            .await
            .unwrap();
        match decision {
            Decision::Accept => assert_eq!(
                result.unwrap(),
                RunOutcome::Completed("accepted once".into())
            ),
            Decision::Reject => assert!(result
                .unwrap_err()
                .to_string()
                .contains("controlled public agent failure")),
            Decision::Retry => assert_eq!(
                result.unwrap(),
                RunOutcome::Completed("retry without tool".into())
            ),
            Decision::Wait => unreachable!(),
        }
    }
    for _ in 0..100 {
        tokio::task::yield_now().await;
    }
    let accepted = matches!(decision, Decision::Accept);
    assert_eq!(calls.load(Ordering::SeqCst), usize::from(accepted));
    assert_eq!(
        store.snapshot(),
        vec![todo(if accepted { CANARY } else { "original task" })]
    );
    assert_eq!(store.is_snapshot_pending(), accepted);
    drop(session);
    drop(db);
    let reopened = Database::open(&path).unwrap();
    let events = EventStore::new(&reopened).load_session(session_id).unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(&event.payload,
        EventPayload::ToolCallRequested {call} if call.id == CALL))
            .count(),
        usize::from(accepted)
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(&event.payload,
        EventPayload::ToolCallCompleted {call_id, ok: true, ..} if call_id == CALL))
            .count(),
        usize::from(accepted)
    );
    if !accepted {
        assert!(!serde_json::to_string(&events).unwrap().contains(CANARY));
    }
}

#[tokio::test]
async fn h233_public_agent_preparation_rejection_retry_and_acceptance_keep_real_state_and_history_aligned(
) {
    for decision in [Decision::Reject, Decision::Retry, Decision::Accept] {
        exercise(decision, false).await;
    }
}

#[tokio::test]
async fn h233_cancellation_and_drop_before_acceptance_never_start_a_native_state_tool() {
    exercise(Decision::Wait, false).await;
    exercise(Decision::Wait, true).await;
}
