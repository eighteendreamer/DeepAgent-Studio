//! Fault injection at the canonical sink, plus real Kernel control-flow checks.
use super::*;
use crate::agent::{AgentDecision, Observation};
use async_trait::async_trait;
use deepagent_core::clock::FixedClock;
use deepagent_persistence::run_store::StoredRunEvent;
use deepagent_tools::{PermissionSet, RiskLevel, Tool, ToolDescriptor, ToolInvocation, ToolOutput};
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Default)]
struct Live(Mutex<Vec<RuntimeEvent>>);

impl RuntimeEventSink for Live {
    fn emit(&self, event: RuntimeEvent) {
        self.0.lock().unwrap().push(event);
    }
}

fn sql(db: &Database, statement: &str) {
    db.with_conn(|conn| {
        conn.execute_batch(statement).unwrap();
        Ok(())
    })
    .unwrap();
}

fn reject(db: &Database, event: &str) {
    sql(
        db,
        &format!(
            "CREATE TRIGGER reject_event BEFORE INSERT ON run_events
         WHEN NEW.event_type='{event}' BEGIN SELECT RAISE(ABORT, 'injected storage fault'); END;"
        ),
    );
}

fn events(db: &Database) -> Vec<StoredRunEvent> {
    let events = RunStore::new(db).events_after("fault-run", None).unwrap();
    for (sequence, event) in events.iter().enumerate() {
        assert_eq!(event.sequence, sequence as u64);
    }
    events
}

fn sink(db: Arc<Database>, live: Arc<Live>) -> PersistentEventSink {
    let clock = FixedClock::new(1);
    let session = Session::create(&db, &clock, None).unwrap();
    RunStore::new(&db)
        .create("fault-run", &session.id().to_string(), None, 1)
        .unwrap();
    PersistentEventSink::new(db, "fault-run".into(), live)
}

fn started(id: &str) -> RuntimeEvent {
    RuntimeEvent::ToolStarted {
        name: "counter".into(),
        call_id: id.into(),
        arguments: serde_json::json!({}),
        tool_kind: None,
        file_path: None,
        summary: None,
        meta: None,
    }
}

fn completed() -> RuntimeEvent {
    RuntimeEvent::ToolCompleted {
        name: "counter".into(),
        call_id: "call".into(),
        ok: true,
        output: serde_json::json!({"executed":true}),
        duration_ms: 0,
        tool_kind: None,
        file_path: None,
        summary: None,
        meta: None,
    }
}

#[test]
fn structured_failure_latches_first_cause_and_never_resumes_automatically() {
    for rejected in [
        "setup.completed",
        "run_started",
        "turn_started",
        "tool_started",
        "tool_completed",
    ] {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let live = Arc::new(Live::default());
        let sink = sink(db.clone(), live.clone());
        let cancellation = CancellationTree::new();
        sink.bind_cancellation(cancellation.clone());
        reject(&db, rejected);
        for event in [
            RuntimeEvent::RunStarted {
                task_id: "task".into(),
            },
            RuntimeEvent::TurnStarted { step: 0 },
            started("call"),
            completed(),
        ] {
            sink.emit(event);
        }
        let error = sink.checkpoint().unwrap_err().to_string();
        assert!(error.contains(rejected), "{error}");
        assert!(cancellation.is_cancelled());
        assert!(!live
            .0
            .lock()
            .unwrap()
            .iter()
            .any(|event| event.label() == rejected));
        assert!(!events(&db).iter().any(|event| event.event_type == rejected));
        let before = events(&db).len();
        let live_before = live.0.lock().unwrap().len();
        sql(&db, "DROP TRIGGER reject_event;");
        sink.emit(RuntimeEvent::RunCompleted {
            message: "must not resume".into(),
            edited_files: vec![],
        });
        assert_eq!(sink.checkpoint().unwrap_err().to_string(), error);
        assert_eq!(events(&db).len(), before);
        assert_eq!(live.0.lock().unwrap().len(), live_before);
    }
}

#[test]
fn phase_transition_failure_also_rejects_delivery() {
    let db = Arc::new(Database::open_in_memory().unwrap());
    let live = Arc::new(Live::default());
    let sink = sink(db.clone(), live.clone());
    sql(
        &db,
        "CREATE TRIGGER reject_phase BEFORE UPDATE OF state ON runs
        WHEN NEW.state='executing_tools' BEGIN SELECT RAISE(ABORT, 'injected phase fault'); END;",
    );
    sink.emit(started("call"));
    let error = sink.checkpoint().unwrap_err().to_string();
    assert!(error.contains("tool_started") && error.contains("injected phase fault"));
    assert!(live.0.lock().unwrap().is_empty());
    assert_eq!(events(&db).len(), 1);
}

#[test]
fn rejected_delta_batch_is_retained_but_not_retried_or_overtaken() {
    for reasoning in [false, true] {
        for flush in ["checkpoint", "switch", "threshold"] {
            let db = Arc::new(Database::open_in_memory().unwrap());
            let live = Arc::new(Live::default());
            let sink = sink(db.clone(), live.clone());
            let kind = if reasoning {
                "reasoning_delta_batch"
            } else {
                "content_delta_batch"
            };
            reject(&db, kind);
            let text = "x".repeat(if flush == "threshold" {
                PERSISTED_DELTA_BATCH_BYTES
            } else {
                12
            });
            sink.emit(if reasoning {
                RuntimeEvent::ReasoningDelta { text: text.clone() }
            } else {
                RuntimeEvent::ContentDelta { text: text.clone() }
            });
            assert_eq!(
                live.0.lock().unwrap().len(),
                1,
                "early delta is explicitly provisional"
            );
            if flush == "switch" {
                sink.emit(if reasoning {
                    RuntimeEvent::ContentDelta {
                        text: "later".into(),
                    }
                } else {
                    RuntimeEvent::ReasoningDelta {
                        text: "later".into(),
                    }
                });
            } else if flush == "threshold" {
                sink.emit(if reasoning {
                    RuntimeEvent::ReasoningDelta {
                        text: "later".into(),
                    }
                } else {
                    RuntimeEvent::ContentDelta {
                        text: "later".into(),
                    }
                });
            }
            let first = sink.checkpoint().unwrap_err().to_string();
            assert!(first.contains(kind));
            assert_eq!(sink.state.lock().unwrap().delta.text, text);
            sql(&db, "DROP TRIGGER reject_event;");
            sink.emit(started("must-not-start"));
            assert_eq!(sink.checkpoint().unwrap_err().to_string(), first);
            assert_eq!(live.0.lock().unwrap().len(), 1);
            assert_eq!(events(&db).len(), 1);
        }
    }
}

#[test]
fn normal_delta_switches_commit_both_kinds_in_order() {
    let db = Arc::new(Database::open_in_memory().unwrap());
    let live = Arc::new(Live::default());
    let sink = sink(db.clone(), live.clone());
    sink.emit(RuntimeEvent::ReasoningDelta { text: "思".into() });
    sink.emit(RuntimeEvent::ReasoningDelta { text: "考".into() });
    sink.emit(RuntimeEvent::ContentDelta {
        text: "回答".into(),
    });
    sink.checkpoint().unwrap();
    let actual = events(&db)
        .into_iter()
        .skip(1)
        .map(|event| {
            (
                event.event_type,
                event.data["text"].clone(),
                event.data["chunks"].clone(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        actual,
        vec![
            (
                "reasoning_delta_batch".into(),
                serde_json::json!("思考"),
                serde_json::json!(2)
            ),
            (
                "content_delta_batch".into(),
                serde_json::json!("回答"),
                serde_json::json!(1)
            ),
        ]
    );
    assert_eq!(live.0.lock().unwrap().len(), 3);
}

#[test]
fn concurrent_delivery_matches_gapless_durable_order_after_reopen() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("events.sqlite");
    let live = Arc::new(Live::default());
    let expected = {
        let db = Arc::new(Database::open(&path).unwrap());
        let sink = Arc::new(sink(db.clone(), live.clone()));
        std::thread::scope(|scope| {
            for index in 0..32 {
                let sink = sink.clone();
                scope.spawn(move || sink.emit(started(&format!("call-{index}"))));
            }
        });
        sink.checkpoint().unwrap();
        let actual = events(&db)
            .into_iter()
            .filter(|event| event.event_type == "tool_started")
            .map(|event| event.data)
            .collect::<Vec<_>>();
        let delivered = live
            .0
            .lock()
            .unwrap()
            .iter()
            .map(|event| serde_json::to_value(event).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(actual, delivered);
        assert_eq!(actual.len(), 32);
        events(&db)
    };
    let reopened = Database::open(path).unwrap();
    let actual = events(&reopened);
    assert_eq!(
        serde_json::to_value(&actual).unwrap(),
        serde_json::to_value(&expected).unwrap()
    );
    let cursor = actual[15].sequence;
    assert_eq!(
        RunStore::new(&reopened)
            .events_after("fault-run", Some(cursor))
            .unwrap()
            .len(),
        actual.len() - 16
    );
}

struct Counter(Arc<AtomicUsize>);

#[async_trait]
impl Tool for Counter {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "counter".into(),
            description: "offline side effect".into(),
            parameters: serde_json::json!({"type":"object"}),
            risk: RiskLevel::Safe,
            required_permissions: PermissionSet::empty(),
        }
    }
    async fn invoke(&self, _: serde_json::Value) -> Result<ToolOutput> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(ToolOutput::success(serde_json::json!({"executed":true})))
    }
}

#[derive(Default)]
struct ProbeAgent {
    sink: Option<Arc<dyn RuntimeEventSink>>,
    calls: usize,
    delta: bool,
}

#[async_trait]
impl Agent for ProbeAgent {
    fn set_event_sink(&mut self, sink: Arc<dyn RuntimeEventSink>) {
        self.sink = Some(sink);
    }
    async fn think(&mut self, _: usize, _: &[Observation]) -> Result<AgentDecision> {
        self.calls += 1;
        if self.delta {
            self.sink
                .as_ref()
                .unwrap()
                .emit(RuntimeEvent::ContentDelta {
                    text: "uncommitted".into(),
                });
            Ok(AgentDecision::CallTool(
                ToolInvocation::new("counter", serde_json::json!({})).with_id("call"),
            ))
        } else {
            Ok(AgentDecision::Complete("done".into()))
        }
    }
}

#[tokio::test]
async fn kernel_fail_closed_covers_lifecycle_delta_and_double_storage_failure() {
    for rejected in [
        None,
        Some("run_started"),
        Some("session_registered"),
        Some("turn_started"),
        Some("content_delta_batch"),
    ] {
        for terminal_failure in [false, true] {
            // The success + terminal-only fault belongs to the independent H-143 probe.
            if rejected.is_none() && terminal_failure {
                continue;
            }
            let db = Arc::new(Database::open_in_memory().unwrap());
            if let Some(event) = rejected {
                reject(&db, event);
            }
            if terminal_failure {
                sql(&db, "CREATE TRIGGER reject_terminal BEFORE INSERT ON run_events
                    WHEN NEW.event_type='run_terminal' BEGIN SELECT RAISE(ABORT, 'injected terminal fault'); END;");
            }
            let clock = FixedClock::new(1);
            let mut session = Session::create(&db, &clock, None).unwrap();
            let task = session.create_task("offline persistence probe").unwrap();
            let count = Arc::new(AtomicUsize::new(0));
            let mut registry = ToolRegistry::new();
            registry.register(Arc::new(Counter(count.clone()))).unwrap();
            let live = Arc::new(Live::default());
            let kernel = AgentKernel::<FixedClock>::new(
                db.clone(),
                &registry,
                Metrics::new(),
                RuntimeConfig::default(),
                "fault-run",
            )
            .with_events(live.clone());
            let mut agent = ProbeAgent {
                delta: rejected == Some("content_delta_batch"),
                ..Default::default()
            };
            let result = kernel
                .start(RunRequest::new(&mut session, task, &mut agent))
                .await;
            let durable = events(&db);
            if terminal_failure {
                let error = result.unwrap_err().to_string();
                assert!(
                    error.contains(rejected.unwrap()) && error.contains("injected terminal fault"),
                    "{error}"
                );
                assert!(RunStore::new(&db)
                    .get("fault-run")
                    .unwrap()
                    .unwrap()
                    .finished_at
                    .is_none());
            } else {
                let terminal = result.unwrap();
                assert_eq!(
                    terminal.kind,
                    if rejected.is_some() {
                        TerminalKind::Failed
                    } else {
                        TerminalKind::Succeeded
                    }
                );
                assert_eq!(
                    durable
                        .iter()
                        .filter(|event| event.event_type == "run_terminal")
                        .count(),
                    1
                );
            }
            assert_eq!(
                agent.calls,
                usize::from(rejected.is_none() || rejected == Some("content_delta_batch"))
            );
            assert_eq!(count.load(Ordering::SeqCst), 0);
            if let Some(event) = rejected {
                assert!(!live
                    .0
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|event| event.label() == "run_completed"));
                assert!(!durable.iter().any(|entry| entry.event_type == event));
                assert_eq!(
                    durable
                        .iter()
                        .filter(|event| event.event_type == "run_failed")
                        .count(),
                    1
                );
                assert_eq!(session.state().task(task).unwrap().state, TaskState::Failed);
            } else {
                assert!(live
                    .0
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|event| event.label() == "run_completed"));
            }
        }
    }
}
