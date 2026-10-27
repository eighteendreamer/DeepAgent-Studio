//! Formal offline ChatService + real write_file, including reopened control/session stores.
use super::*;
use deepagent_core::{event::EventPayload, id::SessionId};
use deepagent_models::ModelRole;
use deepagent_persistence::{
    event_store::EventStore,
    run_control::{RunActionState, RunControlStore},
    run_store::RunStore,
};
use deepagent_runtime::RuntimeEvent;
use std::str::FromStr;
use std::time::Duration;

const RUN: &str = "persistence-run";
const CONTENT: &str = "offline-persistence-side-effect\n";

struct WriteModel {
    batch: bool,
    requests: Mutex<Vec<serde_json::Value>>,
}

fn writes(batch: bool) -> Vec<String> {
    let output: Vec<_> = (0..if batch { 2 } else { 1 })
        .map(|index| {
            responses_fixture::call(
                &format!("write-{index}"),
                "write_file",
                serde_json::json!({"path":format!("effect-{index}.txt"),"content":CONTENT}),
            )
        })
        .collect();
    let mut wire = vec![serde_json::json!({"type":"response.created","response":{
        "id":"offline-response","object":"response","created_at":1,"model":"deepseek-flash",
        "status":"in_progress","output":[]}})];
    for (index, item) in output.iter().enumerate() {
        let mut added = item.clone();
        added["status"] = "in_progress".into();
        added["arguments"] = "".into();
        wire.extend([
            serde_json::json!({"type":"response.output_item.added","output_index":index,"item":added}),
            serde_json::json!({"type":"response.function_call_arguments.delta","output_index":index,
                "item_id":item["id"],"delta":item["arguments"]}),
            serde_json::json!({"type":"response.function_call_arguments.done","output_index":index,
                "item_id":item["id"],"arguments":item["arguments"]}),
            serde_json::json!({"type":"response.output_item.done","output_index":index,"item":item}),
        ]);
    }
    wire.push(serde_json::json!({"type":"response.completed","response":{
        "id":"offline-response","object":"response","created_at":1,"model":"deepseek-flash",
        "status":"completed","output":output,"usage":{},"error":null,"incomplete_details":null}}));
    wire.into_iter()
        .enumerate()
        .map(|(index, mut event)| {
            event["sequence_number"] = index.into();
            event.to_string()
        })
        .collect()
}

#[async_trait]
impl HttpTransport for WriteModel {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        let first = {
            let mut requests = self.requests.lock().unwrap();
            requests.push(serde_json::from_str(&request.body).unwrap());
            requests.len() == 1
        };
        emit_responses_events(
            sink,
            &if first {
                writes(self.batch)
            } else {
                response_text("done")
            },
        )
    }
}

pub(super) async fn settings(db: Arc<Database>) -> Arc<SettingsService> {
    let settings = Arc::new(SettingsService::new(
        db,
        Arc::new(MockTransport::with_get_json(
            r#"{"data":[{"id":"deepseek-flash","object":"model","owned_by":"deepseek"}]}"#,
        )),
        Arc::new(MemorySecretStore::new()),
    ));
    settings
        .initialize("offline-persistence-placeholder")
        .await
        .unwrap();
    for role in [ModelRole::Chat, ModelRole::Reasoner] {
        settings.set_model(role, "deepseek-flash").unwrap();
    }
    settings
}

async fn observe(rejected: Option<&str>, batch: bool) {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("state.sqlite");
    let workspace = temporary.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let model = Arc::new(WriteModel {
        batch,
        requests: Mutex::new(Vec::new()),
    });
    let model_reached = !matches!(rejected, Some("model_request_started" | "context_usage"));
    let executed = model_reached && rejected != Some("tool_started");
    let (before, session_id, first_output) = {
        let db = Arc::new(Database::open(&path).unwrap());
        if let Some(event) = rejected {
            db.with_conn(|conn| {
                conn.execute_batch(&format!(
                    "CREATE TRIGGER reject_product_event BEFORE INSERT ON run_events
                    WHEN NEW.run_id='{RUN}' AND NEW.event_type='{event}'
                    BEGIN SELECT RAISE(ABORT, 'injected product event fault'); END;"
                ))
                .unwrap();
                Ok(())
            })
            .unwrap();
        }
        let chat = ChatService::new(
            db.clone(),
            settings(db.clone()).await,
            model.clone(),
            &workspace,
        );
        let live = Arc::new(Mutex::new(Vec::new()));
        let captured = live.clone();
        let result = tokio::time::timeout(
            Duration::from_secs(8),
            chat.run_in_session(
                "Write the offline audit files.",
                None,
                None,
                None,
                Vec::new(),
                None,
                false,
                Some(RUN),
                move |event| captured.lock().unwrap().push(event),
                |_| {},
            ),
        )
        .await
        .expect("formal persistence failure must settle");
        eprintln!("persistence service: rejected={rejected:?}, batch={batch}, result={:?}, run={:?}, events={:?}, live={:?}, model_calls={}",
            result.as_ref().map(|_| ()).map_err(ToString::to_string), RunStore::new(&db).get(RUN).unwrap(),
            RunStore::new(&db).events_after(RUN, None).unwrap().iter().map(|event| event.event_type.as_str()).collect::<Vec<_>>(),
            live.lock().unwrap().iter().map(RuntimeEvent::label).collect::<Vec<_>>(), model.requests.lock().unwrap().len());
        if let Some(event) = rejected {
            let error = result.unwrap_err().to_string();
            assert!(error.contains(event), "{error}");
            assert_eq!(
                model.requests.lock().unwrap().len(),
                usize::from(model_reached),
                "no model request may cross the failed barrier"
            );
            assert!(!live
                .lock()
                .unwrap()
                .iter()
                .any(|event| matches!(event, RuntimeEvent::RunCompleted { .. })));
            assert!(!live
                .lock()
                .unwrap()
                .iter()
                .any(|event| event.label() == rejected.unwrap()));
        } else {
            result.unwrap();
            assert_eq!(model.requests.lock().unwrap().len(), 2);
            assert!(live
                .lock()
                .unwrap()
                .iter()
                .any(|event| matches!(event, RuntimeEvent::RunCompleted { .. })));
        }
        assert_eq!(workspace.join("effect-0.txt").exists(), executed);
        if executed {
            assert_eq!(
                std::fs::read_to_string(workspace.join("effect-0.txt")).unwrap(),
                CONTENT
            );
        }
        assert_eq!(
            workspace.join("effect-1.txt").exists(),
            batch && rejected.is_none(),
            "later exclusive tool must not run after storage failure"
        );
        let run = RunStore::new(&db).get(RUN).unwrap().unwrap();
        assert_eq!(
            run.terminal_kind.as_deref(),
            Some(if rejected.is_some() {
                "failed"
            } else {
                "succeeded"
            })
        );
        let events = RunStore::new(&db).events_after(RUN, None).unwrap();
        assert_eq!(
            events
                .iter()
                .filter(|event| event.event_type == "run_terminal")
                .count(),
            1
        );
        if let Some(event) = rejected {
            assert!(!events.iter().any(|entry| entry.event_type == event));
            assert!(!events
                .iter()
                .any(|entry| entry.event_type == "run_completed"));
        }
        for (sequence, event) in events.iter().enumerate() {
            assert_eq!(event.sequence, sequence as u64);
        }
        let actions = RunControlStore::new(&db).list_actions(RUN).unwrap();
        assert_eq!(
            actions.len(),
            if model_reached {
                if batch {
                    2
                } else {
                    1
                }
            } else {
                0
            }
        );
        assert!(
            actions.iter().all(|action| action.state.is_terminal()),
            "dangling actions: {actions:?}"
        );
        if model_reached {
            assert_eq!(actions[0].state == RunActionState::Completed, executed);
            if batch && rejected.is_some() {
                assert_eq!(actions[1].state, RunActionState::Failed);
            }
        }
        let history = EventStore::new(&db)
            .load_session(SessionId::from_str(&run.session_id).unwrap())
            .unwrap();
        let first = history
            .iter()
            .filter_map(|event| match &event.payload {
                EventPayload::ToolCallCompleted {
                    call_id,
                    ok,
                    output,
                    ..
                } if call_id == "write-0" => Some((*ok, output.clone())),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(first.len(), usize::from(model_reached));
        if model_reached {
            assert_eq!(first[0].0, executed);
        }
        (
            events,
            run.session_id,
            first
                .first()
                .map(|result| result.1.clone())
                .unwrap_or_default(),
        )
    };
    // Drop every first-run service/handle, then compare durable replay and cursored reads.
    let db = Arc::new(Database::open(&path).unwrap());
    let actual = RunStore::new(&db).events_after(RUN, None).unwrap();
    assert_eq!(actual, before);
    let chat = ChatService::new(
        db.clone(),
        settings(db.clone()).await,
        model.clone(),
        &workspace,
    );
    let cursor = actual[actual.len() / 2].sequence;
    let tail = chat.run_events(RUN, Some(cursor)).unwrap();
    assert_eq!(
        tail,
        actual
            .iter()
            .filter(|event| event.sequence > cursor)
            .cloned()
            .collect::<Vec<_>>()
    );
    assert!(RunControlStore::new(&db)
        .list_actions(RUN)
        .unwrap()
        .iter()
        .all(|action| action.state.is_terminal()));
    if executed && rejected == Some("tool_completed") {
        let previous_requests = model.requests.lock().unwrap().len();
        chat.run_in_session(
            "Report the preceding write; do not repeat it.",
            Some(&session_id),
            None,
            None,
            Vec::new(),
            None,
            false,
            Some("persistence-resume"),
            |_| {},
            |_| {},
        )
        .await
        .unwrap();
        let requests = model.requests.lock().unwrap();
        assert_eq!(requests.len(), previous_requests + 1);
        let outputs = requests[previous_requests]["input"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|item| item["type"] == "function_call_output" && item["call_id"] == "write-0")
            .collect::<Vec<_>>();
        assert_eq!(
            outputs.len(),
            1,
            "reopened model history must retain the real result"
        );
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(outputs[0]["output"].as_str().unwrap())
                .unwrap(),
            serde_json::json!({"status":"ok", "result":first_output})
        );
        assert!(RunControlStore::new(&db)
            .list_actions("persistence-resume")
            .unwrap()
            .is_empty());
    }
}

#[tokio::test]
async fn normal_writes_survive_reopen_and_cursored_read() {
    for batch in [false, true] {
        observe(None, batch).await;
    }
}

#[tokio::test]
async fn rejected_request_events_never_reach_model_transport() {
    for event in ["model_request_started", "context_usage"] {
        observe(Some(event), false).await;
    }
}

#[tokio::test]
async fn rejected_tool_start_never_executes_or_leaves_actions_running() {
    for batch in [true, false] {
        observe(Some("tool_started"), batch).await;
    }
}

#[tokio::test]
async fn rejected_tool_completion_preserves_real_result_and_stops_later_work() {
    for batch in [true, false] {
        observe(Some("tool_completed"), batch).await;
    }
}

#[tokio::test]
async fn workflow_admission_preserves_persistence_failure_instead_of_cancellation_success() {
    for reject_turn in [false, true] {
        let temporary = tempfile::tempdir().unwrap();
        let db = Arc::new(Database::open(temporary.path().join("workflow.sqlite")).unwrap());
        let workspace = temporary.path().join("workspace");
        std::fs::create_dir(&workspace).unwrap();
        if reject_turn {
            db.with_conn(|conn| {
                conn.execute_batch("CREATE TRIGGER reject_workflow_turn BEFORE INSERT ON run_events
                    WHEN NEW.event_type='turn_started' BEGIN SELECT RAISE(ABORT, 'workflow event fault'); END;").unwrap();
                Ok(())
            }).unwrap();
        }
        let chat = ChatService::new(
            db.clone(),
            settings(db.clone()).await,
            Arc::new(MockTransport::default()),
            &workspace,
        );
        let session = chat.create_session(None).unwrap();
        let workflow = serde_json::from_value(serde_json::json!({"definition":{
            "version":1,"nodes":[{"id":"start","kind":"start"},
                {"id":"write","kind":"tool","config":{"toolName":"write_file",
                    "toolArguments":{"path":"effect.txt","content":CONTENT}}},
                {"id":"end","kind":"end"}],
            "edges":[{"id":"a","source":"start","target":"write"},
                {"id":"b","source":"write","target":"end"}]}}))
        .unwrap();
        let live = Arc::new(Mutex::new(Vec::new()));
        let capture = live.clone();
        let result = tokio::time::timeout(
            Duration::from_secs(8),
            chat.run_workflow(
                workflow,
                "workflow-fault".into(),
                session,
                move |event| capture.lock().unwrap().push(event),
                |_| {},
            ),
        )
        .await
        .expect("workflow persistence failure must settle");
        let run = RunStore::new(&db).get("workflow-fault").unwrap().unwrap();
        if reject_turn {
            assert!(result.unwrap_err().to_string().contains("turn_started"));
            assert_eq!(run.terminal_kind.as_deref(), Some("failed"));
            assert!(!live
                .lock()
                .unwrap()
                .iter()
                .any(|event| matches!(event, RuntimeEvent::RunCompleted { .. })));
        } else {
            result.unwrap();
            assert_eq!(run.terminal_kind.as_deref(), Some("succeeded"));
        }
        assert_eq!(workspace.join("effect.txt").exists(), !reject_turn);
        assert_eq!(
            RunStore::new(&db)
                .events_after("workflow-fault", None)
                .unwrap()
                .iter()
                .filter(|event| event.event_type == "run_terminal")
                .count(),
            1
        );
    }
}
