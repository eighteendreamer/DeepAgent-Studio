//! H-193: every executable graph member belongs to the admitted start.
use super::*;
use deepagent_app_core::settings::PermissionPreset;
use deepagent_builtins::http_request::{HttpRequest, HttpRequestClient};
use deepagent_core::error::{CoreError, Result};
use deepagent_models::transport::{EventSink, HttpTransport, TransportRequest};
use deepagent_runtime::workflow::NodeExecutionEvent;
use deepagent_tools::ToolExecutionContext;
use serde_json::{json, Value};

pub(super) const REASON: &str = "InvalidWorkflowGraph: node";

pub(super) fn assert_rejected(
    db: &Database,
    result: &Result<String>,
    id: &str,
    nodes: &[NodeExecutionEvent],
) {
    let error = result.as_ref().unwrap_err();
    assert!(matches!(error, CoreError::Invalid(_)), "{error}");
    assert!(
        error.to_string().contains(REASON)
            && error.to_string().contains("is unreachable from start"),
        "{error}"
    );
    assert!(
        nodes.is_empty(),
        "control ownership must be admitted before dispatch"
    );
    let store = RunStore::new(db);
    let record = store.get(id).unwrap().unwrap();
    assert_eq!(record.terminal_kind.as_deref(), Some("failed"));
    assert!(record
        .terminal_reason
        .as_deref()
        .unwrap()
        .contains("is unreachable from start"));
    let events = store.events_after(id, None).unwrap();
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
        .list_actions(id)
        .unwrap()
        .is_empty());
}

#[derive(Default)]
struct DispatchProbe {
    model: AtomicUsize,
    http: AtomicUsize,
}

#[async_trait::async_trait]
impl HttpTransport for DispatchProbe {
    async fn stream(&self, _: TransportRequest, _: &mut dyn EventSink) -> Result<()> {
        self.model.fetch_add(1, Ordering::SeqCst);
        Err(CoreError::other("synthetic model transport reached"))
    }
}

#[async_trait::async_trait]
impl HttpRequestClient for DispatchProbe {
    fn validate(&self, request: &HttpRequest) -> Result<()> {
        request.validate()
    }
    async fn request(&self, _: HttpRequest, _: ToolExecutionContext) -> Result<Value> {
        self.http.fetch_add(1, Ordering::SeqCst);
        Ok(json!({"body":"synthetic", "status_code":200, "headers":{}}))
    }
}

async fn run(
    chat: &ChatService,
    session: &str,
    request: WorkflowRequest,
    id: &str,
) -> (Result<String>, Vec<NodeExecutionEvent>) {
    let nodes = Arc::new(Mutex::new(Vec::new()));
    let capture = nodes.clone();
    let result = chat
        .run_workflow(
            request,
            id.into(),
            session.into(),
            move |event| {
                if let RuntimeEvent::WorkflowNode { event } = event {
                    capture.lock().unwrap().push(event);
                }
            },
            |_| panic!("these synthetic fixtures must not ask for approval"),
        )
        .await;
    let events = nodes.lock().unwrap().clone();
    (result, events)
}

async fn effectful_orphan(kind: &str, config: Value) {
    let root = tempfile::tempdir().unwrap();
    let db = Arc::new(Database::open_in_memory().unwrap());
    let settings = Arc::new(SettingsService::new(
        db.clone(),
        Arc::new(MockTransport::with_get_json(
            r#"{"data":[{"id":"deepseek-v4-flash","object":"model","owned_by":"deepseek"}]}"#,
        )),
        Arc::new(MemorySecretStore::new()),
    ));
    settings
        .initialize("sk-synthetic-placeholder")
        .await
        .unwrap();
    settings
        .set_active_permission_preset(PermissionPreset::FullAccess)
        .unwrap();
    let probe = Arc::new(DispatchProbe::default());
    let chat = ChatService::new(db.clone(), settings, probe.clone(), root.path())
        .with_workflow_http_client(probe.clone());
    let session = chat.create_session(None).unwrap();
    for (index, target) in [Value::Null, json!("orphan"), json!("finish")]
        .into_iter()
        .enumerate()
    {
        let request = serde_json::from_value(json!({"definition":{"version":1,"nodes":[
            {"id":"orphan","kind":kind,"config":config}, {"id":"entry","kind":"start"}, {"id":"finish","kind":"end"}
        ],"edges":[{"id":"a","source":"entry","target":"finish"}]},"target_node_id":target})).unwrap();
        let id = format!("h193-{kind}-{index}");
        let (result, nodes) = run(&chat, &session, request, &id).await;
        assert_rejected(&db, &result, &id, &nodes);
        assert!(!root.path().join("must-not-exist.txt").exists());
        assert_eq!(probe.model.load(Ordering::SeqCst), 0);
        assert_eq!(probe.http.load(Ordering::SeqCst), 0);
    }
    // A real connected control proves the capability isn't disabled in the
    // fixture merely to get zero calls in the rejection cases.
    let control = serde_json::from_value(json!({"definition":{"version":1,"nodes":[
        {"id":"orphan","kind":kind,"config":config}, {"id":"entry","kind":"start"}, {"id":"finish","kind":"end"}
    ],"edges":[{"id":"a","source":"entry","target":"orphan"},{"id":"b","source":"orphan","target":"finish"}]}})).unwrap();
    let (result, nodes) = run(&chat, &session, control, "h193-connected").await;
    assert!(nodes
        .iter()
        .any(|n| n.node_id == "orphan" && n.status == NodeExecutionStatus::Running));
    match kind {
        "llm" => {
            assert!(result
                .unwrap_err()
                .to_string()
                .contains("synthetic model transport reached"));
            assert_eq!(probe.model.load(Ordering::SeqCst), 1);
        }
        "http-request" => {
            assert_eq!(result.unwrap(), session);
            assert_eq!(probe.http.load(Ordering::SeqCst), 1);
        }
        "tool" => {
            assert_eq!(result.unwrap(), session);
            assert_eq!(
                std::fs::read_to_string(root.path().join("must-not-exist.txt")).unwrap(),
                "synthetic"
            );
        }
        _ => panic!("unknown fixture kind"),
    }
}

#[tokio::test]
async fn h193_orphan_model_never_reaches_the_transport_in_full_or_target_runs() {
    effectful_orphan("llm", json!({"userPrompt":"synthetic"})).await;
}
#[tokio::test]
async fn h193_orphan_file_tool_never_reaches_the_registry_in_full_or_target_runs() {
    effectful_orphan("tool", json!({"toolName":"write_file","toolArguments":{"path":"must-not-exist.txt","content":"synthetic"}})).await;
}
#[tokio::test]
async fn h193_orphan_http_never_reaches_the_broker_in_full_or_target_runs() {
    effectful_orphan(
        "http-request",
        json!({"httpUrl":"https://synthetic.fixture.invalid","httpMethod":"GET"}),
    )
    .await;
}

#[tokio::test]
async fn h193_control_ownership_and_normal_exit_survive_sqlite_reopen_without_redispatch() {
    for valid in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("reachability.db");
        let db = Arc::new(Database::open(&path).unwrap());
        let (_root, db, chat, session) = workflow_fixture_with_database(root, db).await;
        let request = serde_json::from_value(json!({"definition":{"version":1,"nodes":[
            {"id":"finish","kind":"end","config":{"outputVariables":[{"name":"report","type":"string","value":"ROOTED"}]}},
            {"id":"entry","kind":"start"}
        ],"edges":if valid {json!([{"id":"a","source":"entry","target":"finish"}])} else {json!([])}}, "target_node_id":"finish"})).unwrap();
        let (result, nodes) = run(&chat, &session, request, "h193-durable").await;
        if valid {
            assert_eq!(result.unwrap(), session);
            assert!(nodes.iter().any(|n| n.node_id == "finish"
                && n.status == NodeExecutionStatus::Completed
                && n.outputs == Some(json!({"report":"ROOTED"}))));
        } else {
            assert_rejected(&db, &result, "h193-durable", &nodes);
        }
        let before = RunStore::new(&db)
            .events_after("h193-durable", None)
            .unwrap();
        let terminals: Vec<_> = before
            .iter()
            .filter(|e| e.event_type == "run_terminal")
            .collect();
        assert_eq!(terminals.len(), 1);
        assert_eq!(
            terminals[0].data["kind"],
            if valid { "succeeded" } else { "failed" }
        );
        let reason = RunStore::new(&db)
            .get("h193-durable")
            .unwrap()
            .unwrap()
            .terminal_reason;
        let cursor = before[before.len() / 2].sequence;
        drop(chat);
        drop(db);
        let reopened = Database::open(&path).unwrap();
        let store = RunStore::new(&reopened);
        assert_eq!(store.events_after("h193-durable", None).unwrap(), before);
        assert_eq!(
            store.events_after("h193-durable", Some(cursor)).unwrap(),
            before
                .iter()
                .filter(|e| e.sequence > cursor)
                .cloned()
                .collect::<Vec<_>>()
        );
        let record = store.get("h193-durable").unwrap().unwrap();
        assert_eq!(
            record.terminal_kind.as_deref(),
            Some(if valid { "succeeded" } else { "failed" })
        );
        assert_eq!(record.terminal_reason, reason);
        assert!(
            deepagent_persistence::run_control::RunControlStore::new(&reopened)
                .list_actions("h193-durable")
                .unwrap()
                .is_empty()
        );
    }
}

#[tokio::test]
#[ignore = "requires local Node and desktop dependencies; production control ownership to real Kernel"]
async fn h193_desktop_control_ownership_reaches_the_real_kernel() {
    let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../apps/desktop/scripts/audit-workflow-contract.cjs");
    let output = std::process::Command::new("node")
        .arg(script)
        .arg("--emit-reachability-fixtures")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let fixtures: Vec<Value> = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(fixtures.len(), 5);
    for (index, fixture) in fixtures.into_iter().enumerate() {
        let request: WorkflowRequest = serde_json::from_value(fixture["workflow"].clone()).unwrap();
        let (_root, db, chat, session) = workflow_fixture().await;
        let id = format!("h193-desktop-{index}");
        let (result, nodes) = run(&chat, &session, request.clone(), &id).await;
        if matches!(
            fixture["name"].as_str(),
            Some("professional-unreachable-end" | "professional-orphan")
        ) {
            assert_rejected(&db, &result, &id, &nodes);
        } else {
            assert_eq!(result.unwrap(), session);
            let completed: std::collections::BTreeSet<_> = nodes
                .iter()
                .filter(|n| n.status == NodeExecutionStatus::Completed)
                .map(|n| &n.node_id)
                .collect();
            assert_eq!(
                completed,
                request.definition.nodes.iter().map(|n| &n.id).collect()
            );
            let events = RunStore::new(&db).events_after(&id, None).unwrap();
            let terminals: Vec<_> = events
                .iter()
                .filter(|e| e.event_type == "run_terminal")
                .collect();
            assert_eq!(terminals.len(), 1);
            assert_eq!(terminals[0].data["kind"], "succeeded");
        }
    }
}
