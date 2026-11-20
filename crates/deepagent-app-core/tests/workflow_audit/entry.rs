//! H-191: no execution without an admitted, explicit workflow entry.
use super::*;
use deepagent_core::error::{CoreError, Result};
use deepagent_models::transport::{EventSink, HttpTransport, TransportRequest};
use deepagent_runtime::workflow::NodeExecutionEvent;
use serde_json::{json, Value};

pub(super) fn assert_rejected(
    db: &Database,
    result: &Result<String>,
    run_id: &str,
    nodes: &[NodeExecutionEvent],
) {
    const REASON: &str = "InvalidWorkflowGraph: exactly one start node is required";
    let error = result.as_ref().unwrap_err();
    assert!(matches!(error, CoreError::Invalid(_)), "{error}");
    assert!(error.to_string().contains(REASON), "{error}");
    assert!(
        nodes.is_empty(),
        "entry admission precedes every graph root"
    );
    let store = RunStore::new(db);
    let record = store.get(run_id).unwrap().unwrap();
    assert_eq!(record.terminal_kind.as_deref(), Some("failed"));
    assert!(record.terminal_reason.as_deref().unwrap().contains(REASON));
    let events = store.events_after(run_id, None).unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|e| e.event_type == "run_terminal")
            .count(),
        1
    );
    assert!(events
        .iter()
        .all(|e| !matches!(e.event_type.as_str(), "workflow_node" | "action_started")));
    assert!(deepagent_persistence::run_control::RunControlStore::new(db)
        .list_actions(run_id)
        .unwrap()
        .is_empty());
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
            |_| panic!("entry admission must not request approval"),
        )
        .await;
    let captured = nodes.lock().unwrap().clone();
    (result, captured)
}

#[tokio::test]
async fn h191_missing_entry_rejects_effectful_roots_in_full_and_targeted_runs() {
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
    for (index, target) in [Value::Null, json!("write"), json!("finish")]
        .into_iter()
        .enumerate()
    {
        let workflow = serde_json::from_value(json!({
            "definition":{"version":1,"nodes":[
                {"id":"write","kind":"tool","config":{"toolName":"write_file","toolArguments":{"path":"must-not-exist.txt","content":"synthetic"}}},
                {"id":"model","kind":"llm","config":{"userPrompt":"synthetic"}},
                {"id":"finish","kind":"end"}
            ],"edges":[]}, "target_node_id":target
        })).unwrap();
        let id = format!("h191-no-entry-{index}");
        let (result, events) = run(&chat, &session, workflow, &id).await;
        assert_rejected(&db, &result, &id, &events);
        assert!(!root.path().join("must-not-exist.txt").exists());
        assert_eq!(transport.0.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn h191_rejection_and_normal_entry_survive_sqlite_reopen_without_redispatch() {
    for valid in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("entry.db");
        let db = Arc::new(Database::open(&path).unwrap());
        let (_root, db, chat, session) = workflow_fixture_with_database(root, db).await;
        let nodes = if valid {
            json!([
                {"id":"finish","kind":"end","config":{"outputVariables":[{"name":"report","type":"string","value":"{{#entry.value#}}"}]}},
                {"id":"entry","kind":"start","config":{"inputVariables":[{"name":"value","type":"string","default":"normal"}]}}
            ])
        } else {
            json!([{"id":"finish","kind":"end"}])
        };
        let workflow = serde_json::from_value(json!({"definition":{"version":1,"nodes":nodes,
            "edges":if valid {json!([{"id":"a","source":"entry","target":"finish"}])} else {json!([])}},
            "target_node_id":if valid {json!("finish")} else {Value::Null}
        })).unwrap();
        let (result, events) = run(&chat, &session, workflow, "h191-durable").await;
        if valid {
            result.unwrap();
            let terminals: Vec<_> = events.iter().filter(|e| e.status.is_terminal()).collect();
            assert_eq!(terminals.len(), 2);
            assert!(terminals
                .iter()
                .all(|e| e.status == NodeExecutionStatus::Completed));
            assert_eq!(terminals[1].outputs, Some(json!({"report":"normal"})));
        } else {
            assert_rejected(&db, &result, "h191-durable", &events);
        }
        let before = RunStore::new(&db)
            .events_after("h191-durable", None)
            .unwrap();
        let cursor = before[before.len() / 2].sequence;
        drop(chat);
        drop(db);
        let reopened = Database::open(&path).unwrap();
        let store = RunStore::new(&reopened);
        assert_eq!(store.events_after("h191-durable", None).unwrap(), before);
        assert_eq!(
            store.events_after("h191-durable", Some(cursor)).unwrap(),
            before
                .iter()
                .filter(|e| e.sequence > cursor)
                .cloned()
                .collect::<Vec<_>>()
        );
        assert_eq!(
            store
                .get("h191-durable")
                .unwrap()
                .unwrap()
                .terminal_kind
                .as_deref(),
            Some(if valid { "succeeded" } else { "failed" })
        );
        assert!(
            deepagent_persistence::run_control::RunControlStore::new(&reopened)
                .list_actions("h191-durable")
                .unwrap()
                .is_empty()
        );
    }
}

#[tokio::test]
#[ignore = "requires local Node and desktop dependencies; production creative serializer to real Kernel"]
async fn h191_desktop_projects_creative_entries_but_does_not_repair_invalid_professional_graphs() {
    let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../apps/desktop/scripts/audit-workflow-contract.cjs");
    let output = std::process::Command::new("node")
        .arg(script)
        .arg("--emit-entry-fixtures")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let fixtures: Vec<Value> = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(fixtures.len(), 4);
    for (index, fixture) in fixtures.into_iter().enumerate() {
        let workflow: WorkflowRequest =
            serde_json::from_value(fixture["workflow"].clone()).unwrap();
        let (_root, db, chat, session) = workflow_fixture().await;
        let id = format!("h191-desktop-{index}");
        let (result, events) = run(&chat, &session, workflow.clone(), &id).await;
        if fixture["name"] == "professional-missing-entry" {
            assert_rejected(&db, &result, &id, &events);
        } else {
            result.unwrap();
            assert_eq!(
                workflow
                    .definition
                    .nodes
                    .iter()
                    .filter(|node| node.kind == "start")
                    .count(),
                1
            );
            let completed: Vec<_> = events
                .iter()
                .filter(|event| event.status == NodeExecutionStatus::Completed)
                .collect();
            assert_eq!(completed.len(), workflow.definition.nodes.len());
            assert_eq!(
                completed[0].node_id,
                workflow
                    .definition
                    .nodes
                    .iter()
                    .find(|node| node.kind == "start")
                    .unwrap()
                    .id
            );
            assert_eq!(
                RunStore::new(&db)
                    .get(&id)
                    .unwrap()
                    .unwrap()
                    .terminal_kind
                    .as_deref(),
                Some("succeeded")
            );
        }
    }
}
