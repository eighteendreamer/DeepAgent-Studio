//! H-192: admission and settlement require a real workflow exit.
use super::*;
use deepagent_core::error::{CoreError, Result};
use deepagent_models::transport::{EventSink, HttpTransport, TransportRequest};
use deepagent_runtime::workflow::NodeExecutionEvent;
use serde_json::{json, Value};

pub(super) const MISSING_END: &str =
    "InvalidWorkflowGraph: at least one top-level end node is required";
const NO_COMPLETED_END: &str =
    "InvalidWorkflowGraph: workflow exhausted without completing an end node";

pub(super) fn assert_failed(db: &Database, result: &Result<String>, id: &str, reason: &str) {
    let error = result.as_ref().unwrap_err();
    // Kernel settlement preserves its reason through the existing Other
    // adapter; compile-time admission still returns the typed Invalid error.
    if reason == NO_COMPLETED_END {
        assert!(matches!(error, CoreError::Other(_)), "{error}");
    } else {
        assert!(matches!(error, CoreError::Invalid(_)), "{error}");
    }
    assert!(error.to_string().contains(reason), "{error}");
    let store = RunStore::new(db);
    let record = store.get(id).unwrap().unwrap();
    assert_eq!(record.terminal_kind.as_deref(), Some("failed"));
    assert!(record.terminal_reason.as_deref().unwrap().contains(reason));
    let events = store.events_after(id, None).unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|e| e.event_type == "run_terminal")
            .count(),
        1
    );
    assert!(deepagent_persistence::run_control::RunControlStore::new(db)
        .list_actions(id)
        .unwrap()
        .is_empty());
}

async fn run(
    chat: &ChatService,
    session: &str,
    workflow: WorkflowRequest,
    id: &str,
) -> (Result<String>, Vec<NodeExecutionEvent>) {
    let nodes = Arc::new(Mutex::new(Vec::new()));
    let capture = nodes.clone();
    let result = chat
        .run_workflow(
            workflow,
            id.into(),
            session.into(),
            move |event| {
                if let RuntimeEvent::WorkflowNode { event } = event {
                    capture.lock().unwrap().push(event);
                }
            },
            |_| panic!("these exit fixtures must never request approval"),
        )
        .await;
    let captured = nodes.lock().unwrap().clone();
    (result, captured)
}

#[derive(Default)]
struct NoDispatch(AtomicUsize);
#[async_trait::async_trait]
impl HttpTransport for NoDispatch {
    async fn stream(&self, _: TransportRequest, _: &mut dyn EventSink) -> Result<()> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Err(CoreError::other("unexpected synthetic model dispatch"))
    }
}

#[tokio::test]
async fn h192_missing_exit_rejects_effectful_full_and_targeted_graphs_before_dispatch() {
    let root = tempfile::tempdir().unwrap();
    let db = Arc::new(Database::open_in_memory().unwrap());
    let settings = Arc::new(SettingsService::new(
        db.clone(),
        Arc::new(MockTransport::with_get_json(
            r#"{"data":[{"id":"deepseek-v4-flash","object":"model","owned_by":"deepseek"}]}"#,
        )),
        Arc::new(MemorySecretStore::new()),
    ));
    crate::registry_fixture::configure(&settings, "sk-synthetic-placeholder")
        .await
        .unwrap();
    let transport = Arc::new(NoDispatch::default());
    let chat = ChatService::new(db.clone(), settings, transport.clone(), root.path());
    let session = chat.create_session(None).unwrap();
    for (index, target) in [Value::Null, json!("write"), json!("model")]
        .into_iter()
        .enumerate()
    {
        let workflow = serde_json::from_value(json!({
            "definition":{"version":1,"nodes":[
                {"id":"entry","kind":"start"},
                {"id":"write","kind":"tool","config":{"toolName":"write_file","toolArguments":{"path":"must-not-exist.txt","content":"synthetic"}}},
                {"id":"model","kind":"llm","config":{"userPrompt":"synthetic"}}
            ],"edges":[{"id":"a","source":"entry","target":"write"},{"id":"b","source":"write","target":"model"}]},
            "target_node_id":target
        })).unwrap();
        let id = format!("h192-missing-exit-{index}");
        let (result, nodes) = run(&chat, &session, workflow, &id).await;
        assert_failed(&db, &result, &id, MISSING_END);
        assert!(nodes.is_empty(), "admission precedes every node event");
        assert!(!root.path().join("must-not-exist.txt").exists());
        assert_eq!(transport.0.load(Ordering::SeqCst), 0);
        assert!(RunStore::new(&db)
            .events_after(&id, None)
            .unwrap()
            .iter()
            .all(|e| !matches!(e.event_type.as_str(), "workflow_node" | "action_started")));
    }
}

fn branch_workflow(selected: bool, target: Value) -> WorkflowRequest {
    serde_json::from_value(json!({"definition":{"version":1,"nodes":[
        {"id":"finish","kind":"end","config":{"outputVariables":[{"name":"report","type":"string","value":"NORMAL_EXIT"}]}},
        {"id":"entry","kind":"start"},
        {"id":"choose","kind":"if-else","config":{"conditions":[{"id":"true","logic":"and","items":[{"variable":"yes","operator":"is","value":"yes"}]}]}}
    ],"edges":[{"id":"a","source":"entry","target":"choose"},
        {"id":"b","source":"choose","target":"finish","source_handle":if selected {"true"} else {"false"}}]},
        "target_node_id":target
    })).unwrap()
}

#[tokio::test]
async fn h192_exhausted_branch_without_completed_exit_fails_but_normal_end_succeeds() {
    for (index, target) in [Value::Null, json!("choose")].into_iter().enumerate() {
        let (_root, db, chat, session) = workflow_fixture().await;
        let id = format!("h192-skipped-exit-{index}");
        let partial = !target.is_null();
        let (result, nodes) = run(&chat, &session, branch_workflow(false, target), &id).await;
        if partial {
            // H-196: a selected non-end target may settle locally. This does
            // not relax the full-walk requirement to actually complete an end.
            assert_eq!(result.unwrap(), session);
            assert_eq!(
                RunStore::new(&db)
                    .get(&id)
                    .unwrap()
                    .unwrap()
                    .terminal_kind
                    .as_deref(),
                Some("succeeded")
            );
        } else {
            assert_failed(&db, &result, &id, NO_COMPLETED_END);
        }
        assert!(nodes
            .iter()
            .any(|n| n.node_id == "finish" && n.status == NodeExecutionStatus::Skipped));
        assert!(nodes
            .iter()
            .all(|n| n.node_id != "finish" || n.status != NodeExecutionStatus::Completed));
    }
    let (_root, db, chat, session) = workflow_fixture().await;
    let (result, nodes) = run(
        &chat,
        &session,
        branch_workflow(true, Value::Null),
        "h192-normal",
    )
    .await;
    assert_eq!(result.unwrap(), session);
    assert!(nodes.iter().any(|n| n.node_id == "finish"
        && n.status == NodeExecutionStatus::Completed
        && n.outputs == Some(json!({"report":"NORMAL_EXIT"}))));
    let events = RunStore::new(&db)
        .events_after("h192-normal", None)
        .unwrap();
    let terminal = events
        .iter()
        .filter(|e| e.event_type == "run_terminal")
        .collect::<Vec<_>>();
    assert_eq!(terminal.len(), 1);
    assert_eq!(terminal[0].data["kind"], "succeeded");
    assert!(events.iter().any(|e| e.event_type == "workflow_node"
        && e.data["event"]["node_id"] == "finish"
        && e.data["event"]["status"] == "completed"
        && e.sequence < terminal[0].sequence));
}

#[tokio::test]
async fn h192_failed_and_normal_exit_events_survive_sqlite_reopen_without_redispatch() {
    for case in 0..3 {
        let selected = case == 2;
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("exit.db");
        let db = Arc::new(Database::open(&path).unwrap());
        let (_root, db, chat, session) = workflow_fixture_with_database(root, db).await;
        let workflow = if case == 0 {
            serde_json::from_value(json!({"definition":{"version":1,
                "nodes":[{"id":"entry","kind":"start"}],"edges":[]}}))
            .unwrap()
        } else {
            branch_workflow(selected, Value::Null)
        };
        let reason = if case == 0 {
            MISSING_END
        } else {
            NO_COMPLETED_END
        };
        let (result, _) = run(&chat, &session, workflow, "h192-durable").await;
        if selected {
            assert_eq!(result.unwrap(), session);
        } else {
            assert_failed(&db, &result, "h192-durable", reason);
        }
        let before = RunStore::new(&db)
            .events_after("h192-durable", None)
            .unwrap();
        let cursor = before[before.len() / 2].sequence;
        drop(chat);
        drop(db);
        let reopened = Database::open(&path).unwrap();
        let store = RunStore::new(&reopened);
        assert_eq!(store.events_after("h192-durable", None).unwrap(), before);
        assert_eq!(
            store.events_after("h192-durable", Some(cursor)).unwrap(),
            before
                .iter()
                .filter(|e| e.sequence > cursor)
                .cloned()
                .collect::<Vec<_>>()
        );
        assert_eq!(
            store
                .get("h192-durable")
                .unwrap()
                .unwrap()
                .terminal_kind
                .as_deref(),
            Some(if selected { "succeeded" } else { "failed" })
        );
        if !selected {
            assert!(store
                .get("h192-durable")
                .unwrap()
                .unwrap()
                .terminal_reason
                .as_deref()
                .unwrap()
                .contains(reason));
        }
        assert!(
            deepagent_persistence::run_control::RunControlStore::new(&reopened)
                .list_actions("h192-durable")
                .unwrap()
                .is_empty()
        );
    }
}

#[tokio::test]
#[ignore = "requires local Node and desktop dependencies; production exit projection to real Kernel"]
async fn h192_desktop_exit_projection_executes_and_does_not_repair_invalid_requests() {
    let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../apps/desktop/scripts/audit-workflow-contract.cjs");
    let output = std::process::Command::new("node")
        .arg(script)
        .arg("--emit-exit-fixtures")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let fixtures: Vec<Value> = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(fixtures.len(), 6);
    for (index, fixture) in fixtures.into_iter().enumerate() {
        let workflow: WorkflowRequest =
            serde_json::from_value(fixture["workflow"].clone()).unwrap();
        let (_root, db, chat, session) = workflow_fixture().await;
        let id = format!("h192-desktop-{index}");
        let (result, nodes) = run(&chat, &session, workflow.clone(), &id).await;
        let reason = match fixture["name"].as_str().unwrap() {
            "creative-empty" => Some("workflow must contain 1..=256 nodes"),
            "professional-missing-exit" => Some(MISSING_END),
            _ => None,
        };
        if let Some(reason) = reason {
            assert_failed(&db, &result, &id, reason);
            assert!(nodes.is_empty());
            continue;
        }
        assert_eq!(result.unwrap(), session);
        let ends: Vec<_> = workflow
            .definition
            .nodes
            .iter()
            .filter(|n| n.kind == "end")
            .collect();
        assert_eq!(ends.len(), 1);
        let completed: Vec<_> = nodes
            .iter()
            .filter(|n| n.status == NodeExecutionStatus::Completed)
            .collect();
        assert_eq!(completed.len(), workflow.definition.nodes.len());
        assert_eq!(completed.last().unwrap().node_id, ends[0].id);
        let events = RunStore::new(&db).events_after(&id, None).unwrap();
        let terminals: Vec<_> = events
            .iter()
            .filter(|e| e.event_type == "run_terminal")
            .collect();
        assert_eq!(terminals.len(), 1);
        assert_eq!(terminals[0].data["kind"], "succeeded");
        assert!(events.iter().any(|e| e.event_type == "workflow_node"
            && e.data["event"]["node_id"] == ends[0].id
            && e.data["event"]["status"] == "completed"
            && e.sequence < terminals[0].sequence));
    }
}
