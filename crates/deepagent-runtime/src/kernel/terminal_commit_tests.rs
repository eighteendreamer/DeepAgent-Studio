//! H-143: terminal intent is not observable completion until the shared commit.
use super::*;
use crate::agent::{AgentDecision, Observation};
use async_trait::async_trait;
use deepagent_core::clock::FixedClock;
use deepagent_hooks::{Hook, HookContext, HookOutcome, HookPoint};

const RUN: &str = "terminal-commit";

struct CommittedLive {
    db: Arc<Database>,
    events: Mutex<Vec<RuntimeEvent>>,
}

impl RuntimeEventSink for CommittedLive {
    fn emit(&self, event: RuntimeEvent) {
        if event.is_terminal() {
            let store = RunStore::new(&self.db);
            let run = store.get(RUN).unwrap().unwrap();
            assert!(
                run.finished_at.is_some(),
                "live notification preceded commit"
            );
            let expected = match run.terminal_kind.as_deref().unwrap() {
                "succeeded" => "run_completed",
                "cancelled" => "run_cancelled",
                "blocked" => "run_awaiting_approval",
                _ => "run_failed",
            };
            assert_eq!(event.label(), expected);
            let events = store.events_after(RUN, None).unwrap();
            assert_eq!(
                events
                    .iter()
                    .filter(|event| event.event_type == "run_terminal")
                    .count(),
                1
            );
            assert_eq!(
                events
                    .iter()
                    .filter(|event| event.event_type == expected)
                    .count(),
                1
            );
            assert!(events
                .iter()
                .any(|event| event.event_type == "session_terminal_projected"));
        }
        self.events.lock().unwrap().push(event);
    }
}

fn seed(db: Arc<Database>) -> (PersistentEventSink, Arc<CommittedLive>) {
    let clock = FixedClock::new(1);
    let session = Session::create(&db, &clock, None).unwrap();
    RunStore::new(&db)
        .create(RUN, &session.id().to_string(), None, 1)
        .unwrap();
    let live = Arc::new(CommittedLive {
        db: db.clone(),
        events: Mutex::new(Vec::new()),
    });
    (PersistentEventSink::new(db, RUN.into(), live.clone()), live)
}

fn success() -> KernelTerminal {
    KernelTerminal {
        kind: TerminalKind::Succeeded,
        message: Some("committed answer".into()),
        reason: None,
    }
}

#[test]
fn every_terminal_kind_delivers_one_canonical_notification_after_commit() {
    for kind in [
        TerminalKind::Succeeded,
        TerminalKind::Cancelled,
        TerminalKind::Blocked,
        TerminalKind::Failed,
        TerminalKind::MaxTurns,
        TerminalKind::BudgetExceeded,
        TerminalKind::ContextExhausted,
    ] {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let (sink, live) = seed(db.clone());
        sink.emit(RuntimeEvent::RunCompleted {
            message: "premature answer".into(),
            edited_files: vec![crate::events::EditedFile {
                path: "effect.txt".into(),
                lines_added: 3,
                lines_removed: 1,
            }],
        });
        sink.checkpoint().unwrap();
        assert!(live.events.lock().unwrap().is_empty());
        assert!(!RunStore::new(&db)
            .events_after(RUN, None)
            .unwrap()
            .iter()
            .any(|event| event.event_type == "run_completed"));
        let terminal = KernelTerminal {
            kind,
            message: Some("committed answer".into()),
            reason: Some("canonical reason".into()),
        };
        assert!(sink.finish(&terminal, &CancellationTree::new()).unwrap());
        assert!(!sink.finish(&success(), &CancellationTree::new()).unwrap());
        sink.emit(RuntimeEvent::ContentDelta {
            text: "late progress must be ignored".into(),
        });
        assert!(sink.checkpoint().is_err());
        let actual = live.events.lock().unwrap();
        assert_eq!(actual.len(), 1);
        match &actual[0] {
            RuntimeEvent::RunCompleted {
                message,
                edited_files,
            } => {
                assert_eq!(message, "committed answer");
                assert_eq!(edited_files[0].path, "effect.txt");
            }
            RuntimeEvent::RunCancelled { edited_files } => {
                assert_eq!(edited_files[0].lines_added, 3)
            }
            RuntimeEvent::RunFailed { reason } => assert_eq!(reason, "canonical reason"),
            RuntimeEvent::RunAwaitingApproval { message } => {
                assert_eq!(message, "canonical reason")
            }
            _ => panic!("expected one terminal notification"),
        }
    }
}

#[test]
fn rejected_pair_has_no_live_or_replayed_completion_after_reopen() {
    for rejected in ["run_completed", "run_terminal"] {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("terminal.sqlite");
        {
            let db = Arc::new(Database::open(&path).unwrap());
            let (sink, live) = seed(db.clone());
            db.with_conn(|conn| {
                conn.execute_batch(&format!("CREATE TRIGGER reject_terminal BEFORE INSERT ON run_events
                    WHEN NEW.event_type='{rejected}' BEGIN SELECT RAISE(ABORT, 'terminal pair rejected'); END;")).unwrap();
                Ok(())
            }).unwrap();
            sink.emit(RuntimeEvent::RunCompleted {
                message: "pending".into(),
                edited_files: vec![],
            });
            assert!(sink
                .finish(&success(), &CancellationTree::new())
                .unwrap_err()
                .to_string()
                .contains("terminal pair rejected"));
            assert!(live.events.lock().unwrap().is_empty());
        }
        let db = Database::open(path).unwrap();
        let store = RunStore::new(&db);
        assert!(store.get(RUN).unwrap().unwrap().finished_at.is_none());
        let events = store.events_after(RUN, None).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, "run_accepted");
    }
}

#[test]
fn projection_failure_suppresses_delivery_but_retains_atomic_pair_for_repair() {
    let db = Arc::new(Database::open_in_memory().unwrap());
    let (sink, live) = seed(db.clone());
    db.with_conn(|conn| {
        conn.execute_batch("CREATE TRIGGER reject_ack BEFORE INSERT ON run_events
            WHEN NEW.event_type='session_terminal_projected' BEGIN SELECT RAISE(ABORT, 'projection ack rejected'); END;").unwrap();
        Ok(())
    }).unwrap();
    assert!(sink
        .finish(&success(), &CancellationTree::new())
        .unwrap_err()
        .to_string()
        .contains("projection ack rejected"));
    assert!(live.events.lock().unwrap().is_empty());
    let store = RunStore::new(&db);
    let events = store.events_after(RUN, None).unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_type == "run_completed")
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_type == "run_terminal")
            .count(),
        1
    );
    assert_eq!(
        store.get(RUN).unwrap().unwrap().terminal_kind.as_deref(),
        Some("succeeded")
    );
    db.with_conn(|conn| {
        conn.execute_batch("DROP TRIGGER reject_ack;").unwrap();
        Ok(())
    })
    .unwrap();
    assert!(!persist_terminal_and_notify(
        &db,
        RUN,
        &success(),
        &CancellationTree::new(),
        None,
        live.as_ref()
    )
    .unwrap());
    assert!(
        live.events.lock().unwrap().is_empty(),
        "repair is not a new live completion"
    );
    assert!(store
        .events_after(RUN, None)
        .unwrap()
        .iter()
        .any(|event| event.event_type == "session_terminal_projected"));
}

struct FinishAgent;
#[async_trait]
impl Agent for FinishAgent {
    async fn think(&mut self, _: usize, _: &[Observation]) -> Result<AgentDecision> {
        Ok(AgentDecision::Complete("answer before final hook".into()))
    }
}

struct FailedFinalHook;
#[async_trait]
impl Hook for FailedFinalHook {
    fn name(&self) -> &str {
        "terminal-commit-test"
    }
    async fn run(&self, _: &HookContext) -> Result<HookOutcome> {
        Err(CoreError::other("injected final hook failure"))
    }
}

#[tokio::test]
async fn final_hook_failure_replaces_queued_success_before_any_notification() {
    let db = Arc::new(Database::open_in_memory().unwrap());
    let clock = FixedClock::new(1);
    let mut session = Session::create(&db, &clock, None).unwrap();
    let task = session.create_task("finish with failed hook").unwrap();
    let registry = ToolRegistry::new();
    let mut hooks = HookRegistry::new();
    hooks.register(HookPoint::TaskCompleted, Arc::new(FailedFinalHook));
    let live = Arc::new(CommittedLive {
        db: db.clone(),
        events: Mutex::new(Vec::new()),
    });
    let kernel = AgentKernel::new(
        db.clone(),
        &registry,
        Metrics::new(),
        RuntimeConfig::default(),
        RUN,
    )
    .with_hooks(&hooks)
    .with_events(live.clone());
    let terminal = kernel
        .start(RunRequest::new(&mut session, task, &mut FinishAgent))
        .await
        .unwrap();
    assert_eq!(terminal.kind, TerminalKind::Failed);
    let events = RunStore::new(&db).events_after(RUN, None).unwrap();
    assert!(!events
        .iter()
        .any(|event| event.event_type == "run_completed"));
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_type == "run_failed")
            .count(),
        1
    );
    assert_eq!(
        live.events
            .lock()
            .unwrap()
            .iter()
            .filter(|event| event.is_terminal())
            .count(),
        1
    );
}

struct PendingAgent;
#[async_trait]
impl Agent for PendingAgent {
    async fn think(&mut self, _: usize, _: &[Observation]) -> Result<AgentDecision> {
        std::future::pending().await
    }
}

#[tokio::test]
async fn deadline_and_cleanup_publish_one_final_failure_not_a_cancelled_successor() {
    let db = Arc::new(Database::open_in_memory().unwrap());
    let clock = FixedClock::new(1);
    let mut session = Session::create(&db, &clock, None).unwrap();
    let task = session.create_task("deadline").unwrap();
    let registry = ToolRegistry::new();
    let live = Arc::new(CommittedLive {
        db: db.clone(),
        events: Mutex::new(Vec::new()),
    });
    let kernel = AgentKernel::new(
        db.clone(),
        &registry,
        Metrics::new(),
        RuntimeConfig {
            task_timeout: Some(std::time::Duration::from_millis(25)),
            ..RuntimeConfig::default()
        },
        RUN,
    )
    .with_events(live.clone());
    let terminal = kernel
        .start(RunRequest::new(&mut session, task, &mut PendingAgent))
        .await
        .unwrap();
    assert_eq!(terminal.kind, TerminalKind::BudgetExceeded);
    let events = live.events.lock().unwrap();
    let terminals = events
        .iter()
        .filter(|event| event.is_terminal())
        .collect::<Vec<_>>();
    assert_eq!(terminals.len(), 1);
    assert!(
        matches!(terminals[0], RuntimeEvent::RunFailed { reason } if reason.contains("deadline"))
    );
}
