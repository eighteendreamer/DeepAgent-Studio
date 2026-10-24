//! Public RuntimeEngine audit, not the hook-equipped AppCore default path.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use deepagent_builtins::{TodoItem, TodoStatus, TodoStore, TodoWriteTool};
use deepagent_core::clock::FixedClock;
use deepagent_core::error::{CoreError, Result};
use deepagent_core::event::EventPayload;
use deepagent_hooks::HookRegistry;
use deepagent_models::transport::{EventSink, HttpTransport, TransportRequest};
use deepagent_models::{ModelClient, ModelConfig, ToolSchema};
use deepagent_persistence::event_store::EventStore;
use deepagent_persistence::Database;
use deepagent_runtime::{ModelAgent, RunOutcome, RuntimeConfig, RuntimeEngine};
use deepagent_session::Session;
use deepagent_tools::permission::PermissionSet;
use deepagent_tools::{Tool, ToolRegistry};
use deepagent_tracing::metrics::Metrics;
use serde_json::{json, Value};

const CALL: &str = "audit-speculative-todo";
const CANARY: &str = "AUDIT_UNCOMMITTED_TODO";

fn todo(content: &str) -> TodoItem {
    TodoItem {
        content: content.into(),
        status: TodoStatus::Pending,
        active_form: format!("Tracking {content}"),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Ending {
    Complete,
    Failed,
    Disconnect,
}

struct Model {
    store: TodoStore,
    ending: Ending,
    expect_early: bool,
    early_observations: AtomicUsize,
    requests: Mutex<Vec<Value>>,
}

fn emit(sink: &mut dyn EventSink, sequence: &mut usize, mut event: Value) -> Result<bool> {
    event["sequence_number"] = (*sequence).into();
    *sequence += 1;
    sink.on_event(&event.to_string())
}

#[async_trait]
impl HttpTransport for Model {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        let index = {
            let mut requests = self.requests.lock().unwrap();
            requests.push(serde_json::from_str(&request.body).unwrap());
            requests.len() - 1
        };
        assert!(index < 2, "one initial stream and one continuation only");
        let id = format!("audit-response-{index}");
        let mut sequence = 0;
        assert!(!emit(
            sink,
            &mut sequence,
            json!({"type":"response.created",
            "response":{"id":id,"object":"response","created_at":1,
                "model":"deepseek-flash","status":"in_progress","output":[]}})
        )?);
        let output = if index == 0 {
            let arguments = json!({"todos":[todo(CANARY)]}).to_string();
            let item = json!({"type":"function_call","id":"audit-todo-item",
                "call_id":CALL,"name":"todo_write","arguments":arguments,"status":"completed"});
            for event in [
                json!({"type":"response.output_item.added","output_index":0,
                    "item":{"type":"function_call","id":"audit-todo-item","call_id":CALL,
                        "name":"todo_write","arguments":"","status":"in_progress"}}),
                json!({"type":"response.function_call_arguments.delta","output_index":0,
                    "item_id":"audit-todo-item","delta":arguments}),
                json!({"type":"response.function_call_arguments.done","output_index":0,
                    "item_id":"audit-todo-item","arguments":arguments}),
                json!({"type":"response.output_item.done","output_index":0,"item":item}),
            ] {
                assert!(!emit(sink, &mut sequence, event)?);
            }
            if self.expect_early {
                // Observe actual mutation AND its pending flag before injecting
                // failure. A future implementation may legitimately defer the
                // state tool, so expiry is an observation, not a product error.
                let _early = tokio::time::timeout(Duration::from_secs(2), async {
                    while self.store.snapshot() != vec![todo(CANARY)]
                        || !self.store.is_snapshot_pending()
                    {
                        tokio::task::yield_now().await;
                    }
                })
                .await;
            } else {
                // Give any accidentally eligible task a chance to run too.
                for _ in 0..100 {
                    tokio::task::yield_now().await;
                }
            }
            let changed = self.store.snapshot() == vec![todo(CANARY)];
            self.early_observations
                .store(usize::from(changed), Ordering::Release);
            if !self.expect_early {
                assert!(!changed, "hook-equipped runtime must not speculate");
            }
            vec![item]
        } else {
            let item = json!({"type":"message","id":"audit-final-message","role":"assistant",
                "status":"completed","content":[{"type":"output_text","text":"audit done","annotations":[]}]});
            assert!(!emit(
                sink,
                &mut sequence,
                json!({"type":"response.output_item.added","output_index":0,
                    "item":{"type":"message","id":"audit-final-message","role":"assistant",
                        "status":"in_progress","content":[]}})
            )?);
            assert!(!emit(
                sink,
                &mut sequence,
                json!({"type":"response.output_item.done","output_index":0,"item":item})
            )?);
            vec![item]
        };
        if index == 0 && self.ending == Ending::Disconnect {
            return Err(CoreError::other(
                "unexpected eof: controlled audit disconnect",
            ));
        }
        let failed = index == 0 && self.ending == Ending::Failed;
        let mut response = json!({"id":id,"object":"response","created_at":1,
            "model":"deepseek-flash","status":if failed {"failed"} else {"completed"},
            "output":output,"usage":{"input_tokens":8,"output_tokens":6,"total_tokens":14},
            "error":null,"incomplete_details":null});
        if failed {
            // Unknown/nonretryable by the current classifier; this is an
            // injected failure, not a claim about a real provider error code.
            response["error"] = json!({"code":"audit_controlled_failure",
                "message":"controlled audit failure after tool item completion"});
        }
        assert!(emit(
            sink,
            &mut sequence,
            json!({
            "type":if failed {"response.failed"} else {"response.completed"},
            "response":response})
        )?);
        Ok(())
    }
}

async fn observe(ending: Ending, attach_hooks: bool) -> Vec<TodoItem> {
    let temporary = tempfile::tempdir().unwrap();
    let db = Database::open(temporary.path().join("state.sqlite")).unwrap();
    let clock = FixedClock::new(1);
    let mut session = Session::create(&db, &clock, Some("speculation audit")).unwrap();
    let task = session.create_task("update task list").unwrap();
    let store = TodoStore::new();
    store.replace(vec![todo("original task")]);
    let tool = Arc::new(TodoWriteTool::new(store.clone()));
    let descriptor = tool.descriptor();
    let mut registry = ToolRegistry::new();
    registry.register(tool).unwrap();
    let transport = Arc::new(Model {
        store: store.clone(),
        ending,
        expect_early: !attach_hooks,
        early_observations: AtomicUsize::new(0),
        requests: Mutex::new(Vec::new()),
    });
    let mut agent = ModelAgent::new(
        Arc::new(ModelClient::new(
            transport.clone(),
            ModelConfig::deepseek("offline-placeholder"),
        )),
        "deepseek-flash",
        "system",
        "update task list",
        vec![ToolSchema::function(
            descriptor.name,
            descriptor.description,
            descriptor.parameters,
        )],
    );
    let hooks = HookRegistry::new();
    let mut engine = RuntimeEngine::new(
        &registry,
        Metrics::new(),
        RuntimeConfig {
            permissions: PermissionSet::read_only(),
            auto_approve: true,
            ..RuntimeConfig::default()
        },
    );
    if attach_hooks {
        engine = engine.with_hooks(&hooks);
    }
    let result = tokio::time::timeout(
        Duration::from_secs(5),
        engine.run(&mut session, task, &mut agent),
    )
    .await
    .unwrap();
    if ending == Ending::Failed {
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("controlled audit failure"));
        assert_eq!(
            transport.requests.lock().unwrap().len(),
            1,
            "no provider retry"
        );
        let task = session.create_task("continue").unwrap();
        assert_eq!(
            tokio::time::timeout(
                Duration::from_secs(5),
                engine.run(&mut session, task, &mut agent)
            )
            .await
            .unwrap()
            .unwrap(),
            RunOutcome::Completed("audit done".into())
        );
    } else {
        assert_eq!(result.unwrap(), RunOutcome::Completed("audit done".into()));
    }
    let events = EventStore::new(&db).load_session(session.id()).unwrap();
    let requested = events
        .iter()
        .filter(|event| {
            matches!(&event.payload,
        EventPayload::ToolCallRequested { call } if call.id == CALL)
        })
        .count();
    let completed = events
        .iter()
        .filter(|event| {
            matches!(&event.payload,
        EventPayload::ToolCallCompleted { call_id, ok: true, .. } if call_id == CALL)
        })
        .count();
    let committed = ending == Ending::Complete;
    assert_eq!(
        (requested, completed),
        if committed { (1, 1) } else { (0, 0) }
    );
    let requests = transport.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    for kind in ["function_call", "function_call_output"] {
        let count = requests[1]["input"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|item| item["type"] == kind && item["call_id"] == CALL)
            .count();
        assert_eq!(count, usize::from(committed), "next request: {kind}");
    }
    if !committed {
        assert!(!requests[1]["input"].to_string().contains(CANARY));
        assert!(!serde_json::to_string(&events).unwrap().contains(CANARY));
    }
    let actual = store.snapshot();
    eprintln!("speculation audit: ending={ending:?} hooks={attach_hooks} early={} durable_calls={requested} durable_results={completed} pending={} live_todos={actual:?}",
        transport.early_observations.load(Ordering::Acquire), store.is_snapshot_pending());
    actual
}

#[tokio::test]
async fn completed_stream_commits_real_todo_and_paired_history() {
    assert_eq!(observe(Ending::Complete, false).await, vec![todo(CANARY)]);
}

#[tokio::test]
async fn hooked_completed_stream_commits_real_todo_and_paired_history() {
    assert_eq!(observe(Ending::Complete, true).await, vec![todo(CANARY)]);
}

#[tokio::test]
async fn hooked_failed_stream_does_not_mutate_real_todo() {
    assert_eq!(
        observe(Ending::Failed, true).await,
        vec![todo("original task")]
    );
}

#[tokio::test]
#[ignore = "audit: discarded provider attempt must not leave unrecorded TodoWrite state"]
async fn audit_failed_stream_does_not_mutate_real_todo() {
    assert_eq!(
        observe(Ending::Failed, false).await,
        vec![todo("original task")]
    );
}

#[tokio::test]
async fn hooked_stream_retry_does_not_mutate_real_todo() {
    assert_eq!(
        observe(Ending::Disconnect, true).await,
        vec![todo("original task")]
    );
}

#[tokio::test]
#[ignore = "audit: a successful provider retry must not hide an earlier unrecorded TodoWrite"]
async fn audit_stream_retry_does_not_mutate_real_todo() {
    assert_eq!(
        observe(Ending::Disconnect, false).await,
        vec![todo("original task")]
    );
}
