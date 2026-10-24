use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use deepagent_app_core::{
    ChatService, KnowledgeDraftDto, KnowledgeService, MemorySecretStore, SettingsService,
};
use deepagent_models::transport::MockTransport;
use deepagent_persistence::{run_store::RunStore, Database};
use deepagent_runtime::workflow::{NodeExecutionStatus, WorkflowRequest};
use deepagent_runtime::RuntimeEvent;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

#[path = "workflow_audit/media.rs"]
mod media;

async fn workflow_fixture() -> (tempfile::TempDir, Arc<Database>, ChatService, String) {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("input.txt"), "audit input").unwrap();
    let db = Arc::new(Database::open_in_memory().unwrap());
    let settings = Arc::new(SettingsService::new(
        db.clone(),
        Arc::new(MockTransport::with_get_json(
            r#"{"data":[{"id":"deepseek-v4-flash","object":"model","owned_by":"deepseek"}]}"#,
        )),
        Arc::new(MemorySecretStore::new()),
    ));
    settings.initialize("sk-audit-placeholder").await.unwrap();
    let chat = ChatService::new(
        db.clone(),
        settings,
        Arc::new(MockTransport::default()),
        root.path(),
    );
    let session = chat.create_session(None).unwrap();
    (root, db, chat, session)
}

#[tokio::test]
#[ignore = "audit: a failed workflow tool node must not publish completed and a successful run"]
async fn audit_workflow_unknown_tool_propagates_node_and_run_failure() {
    let (_root, db, chat, session) = workflow_fixture().await;
    let workflow: WorkflowRequest = serde_json::from_value(serde_json::json!({
        "definition": {
            "version": 1,
            "nodes": [
                {"id":"start","kind":"start"},
                {"id":"inspect","kind":"tool","config":{"toolName":"read_file","toolArguments":{"path":"input.txt"}}},
                {"id":"end","kind":"end"}
            ],
            "edges": [
                {"id":"e1","source":"start","target":"inspect"},
                {"id":"e2","source":"inspect","target":"end"}
            ]
        }
    })).unwrap();
    let events = Arc::new(Mutex::new(Vec::new()));
    let capture = events.clone();
    let result = chat
        .run_workflow(
            workflow,
            "audit-workflow-tool-failure".into(),
            session,
            move |event| {
                if let RuntimeEvent::WorkflowNode { event } = event {
                    capture.lock().unwrap().push(event);
                }
            },
            |_| {},
        )
        .await;
    let run = RunStore::new(&db)
        .get("audit-workflow-tool-failure")
        .unwrap()
        .unwrap();
    let events = events.lock().unwrap();
    let tool_terminal = events
        .iter()
        .find(|event| event.node_id == "inspect" && event.status.is_terminal())
        .unwrap();
    let end_completed = events
        .iter()
        .any(|event| event.node_id == "end" && event.status == NodeExecutionStatus::Completed);
    eprintln!("api_ok={} tool_status={:?} tool_outputs={:?} node_error={:?} end_completed={end_completed} run_terminal={:?}", result.is_ok(), tool_terminal.status, tool_terminal.outputs, tool_terminal.error, run.terminal_kind);
    assert_eq!(
        tool_terminal.status,
        NodeExecutionStatus::Failed,
        "an execution error cannot be a completed node"
    );
    assert_ne!(run.terminal_kind.as_deref(), Some("succeeded"));
}

#[tokio::test]
#[ignore = "audit: workflow http-request must use the shared network permission and approval boundary"]
async fn audit_workflow_http_node_does_not_bypass_network_approval() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let requests = Arc::new(AtomicUsize::new(0));
    let captured = requests.clone();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        captured.fetch_add(1, Ordering::SeqCst);
        let mut request = [0u8; 4096];
        let _ = socket.read(&mut request).await;
        let response = b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 6\r\nConnection: close\r\n\r\nCANARY";
        let _ = socket.write_all(response).await;
        let _ = socket.shutdown().await;
    });

    let (_root, db, chat, session) = workflow_fixture().await;
    let workflow: WorkflowRequest = serde_json::from_value(serde_json::json!({
        "definition": {
            "version": 1,
            "nodes": [
                {"id":"start","kind":"start"},
                {"id":"request","kind":"http-request","config":{
                    "httpUrl":format!("http://{address}/private-control"),
                    "httpMethod":"GET"
                }},
                {"id":"end","kind":"end"}
            ],
            "edges": [
                {"id":"e1","source":"start","target":"request"},
                {"id":"e2","source":"request","target":"end"}
            ]
        }
    }))
    .unwrap();
    let approvals = Arc::new(AtomicUsize::new(0));
    let approval_count = approvals.clone();

    let result = tokio::time::timeout(
        Duration::from_secs(5),
        chat.run_workflow(
            workflow,
            "audit-workflow-http-network-boundary".into(),
            session,
            |_| {},
            move |_| {
                approval_count.fetch_add(1, Ordering::SeqCst);
            },
        ),
    )
    .await
    .expect("workflow must terminate in the audit fixture");
    let run = RunStore::new(&db)
        .get("audit-workflow-http-network-boundary")
        .unwrap()
        .unwrap();
    let _ = server.await;

    eprintln!(
        "api_ok={} error={:?} requests={} approvals={} run_terminal={:?}",
        result.is_ok(),
        result.as_ref().err(),
        requests.load(Ordering::SeqCst),
        approvals.load(Ordering::SeqCst),
        run.terminal_kind
    );
    assert_eq!(
        requests.load(Ordering::SeqCst),
        0,
        "workflow HTTP must not reach a private/loopback destination without network policy"
    );
    assert!(
        approvals.load(Ordering::SeqCst) > 0,
        "a network-capable workflow node must route through the approval boundary"
    );
}

#[tokio::test]
#[ignore = "audit: workflow knowledge-retrieval must honor the selected knowledge base scope"]
async fn audit_workflow_knowledge_base_id_is_honored() {
    let root = tempfile::tempdir().unwrap();
    let global_root = root.path().join("global");
    let knowledge = Arc::new(KnowledgeService::open(root.path(), &global_root).unwrap());
    knowledge
        .save(KnowledgeDraftDto {
            title: "Project-only runbook".into(),
            body: "scope-audit-needle project vault result".into(),
            kind: Some("note".into()),
            tags: Vec::new(),
            scope: Some("project".into()),
            source_session: None,
            tier: None,
            importance: None,
        })
        .unwrap();
    knowledge
        .save(KnowledgeDraftDto {
            title: "Global-only runbook".into(),
            body: "scope-audit-needle global vault result".into(),
            kind: Some("note".into()),
            tags: Vec::new(),
            scope: Some("global".into()),
            source_session: None,
            tier: None,
            importance: None,
        })
        .unwrap();

    let db = Arc::new(Database::open_in_memory().unwrap());
    let settings = Arc::new(SettingsService::new(
        db.clone(),
        Arc::new(MockTransport::with_get_json(
            r#"{"data":[{"id":"deepseek-v4-flash","object":"model","owned_by":"deepseek"}]}"#,
        )),
        Arc::new(MemorySecretStore::new()),
    ));
    settings.initialize("sk-audit-placeholder").await.unwrap();
    let chat = ChatService::new(
        db.clone(),
        settings,
        Arc::new(MockTransport::default()),
        root.path(),
    )
    .with_knowledge(knowledge);
    let session = chat.create_session(None).unwrap();
    let workflow: WorkflowRequest = serde_json::from_value(serde_json::json!({
        "definition": {
            "version": 1,
            "nodes": [
                {"id":"start","kind":"start"},
                {"id":"retrieve","kind":"knowledge-retrieval","config":{
                    "knowledgeBaseId":"project",
                    "knowledgeTopK":10,
                    "queryVariable":"scope-audit-needle",
                    "scoreThreshold":0
                }},
                {"id":"end","kind":"end"}
            ],
            "edges": [
                {"id":"e1","source":"start","target":"retrieve"},
                {"id":"e2","source":"retrieve","target":"end"}
            ]
        }
    }))
    .unwrap();
    let events = Arc::new(Mutex::new(Vec::new()));
    let capture = events.clone();
    let result = chat
        .run_workflow(
            workflow,
            "audit-workflow-knowledge-scope".into(),
            session,
            move |event| {
                if let RuntimeEvent::WorkflowNode { event } = event {
                    capture.lock().unwrap().push(event);
                }
            },
            |_| {},
        )
        .await;
    let events = events.lock().unwrap();
    let retrieval = events
        .iter()
        .find(|event| event.node_id == "retrieve" && event.status == NodeExecutionStatus::Completed)
        .and_then(|event| event.outputs.clone())
        .expect("knowledge retrieval must publish a completed output");
    let documents = retrieval["documents"]
        .as_array()
        .expect("knowledge retrieval documents must be an array");
    let titles: Vec<_> = documents
        .iter()
        .filter_map(|document| document["title"].as_str())
        .collect();
    eprintln!(
        "api_ok={} error={:?} knowledge_base_id=project titles={titles:?} output={retrieval}",
        result.is_ok(),
        result.as_ref().err()
    );
    assert!(
        result.is_ok(),
        "workflow execution failed: {:?}",
        result.err()
    );
    assert!(
        titles.iter().any(|title| *title == "Project-only runbook"),
        "selected project knowledge must be returned"
    );
    assert!(
        !titles.iter().any(|title| *title == "Global-only runbook"),
        "knowledgeBaseId=project must exclude the global vault"
    );
}

#[tokio::test]
#[ignore = "audit: an unknown workflow target node must fail admission instead of succeeding empty"]
async fn audit_workflow_unknown_target_node_is_rejected() {
    let (_root, db, chat, session) = workflow_fixture().await;
    let workflow: WorkflowRequest = serde_json::from_value(serde_json::json!({
        "definition": {
            "version": 1,
            "nodes": [
                {"id":"start","kind":"start"},
                {"id":"end","kind":"end"}
            ],
            "edges": [
                {"id":"e1","source":"start","target":"end"}
            ]
        },
        "target_node_id":"missing-node"
    }))
    .unwrap();
    let events = Arc::new(Mutex::new(Vec::new()));
    let capture = events.clone();
    let result = chat
        .run_workflow(
            workflow,
            "audit-workflow-unknown-target".into(),
            session,
            move |event| {
                if let RuntimeEvent::WorkflowNode { event } = event {
                    capture.lock().unwrap().push(event);
                }
            },
            |_| {},
        )
        .await;
    let run = RunStore::new(&db)
        .get("audit-workflow-unknown-target")
        .unwrap()
        .unwrap();
    let events = events.lock().unwrap();
    let completed_nodes: Vec<_> = events
        .iter()
        .filter(|event| event.status == NodeExecutionStatus::Completed)
        .map(|event| event.node_id.as_str())
        .collect();
    eprintln!(
        "api_ok={} error={:?} completed_nodes={completed_nodes:?} run_terminal={:?}",
        result.is_ok(),
        result.as_ref().err(),
        run.terminal_kind
    );
    assert!(
        result.is_err(),
        "an unknown target node must be rejected before an empty workflow can succeed"
    );
    assert_ne!(
        run.terminal_kind.as_deref(),
        Some("succeeded"),
        "an unknown target cannot produce a successful run"
    );
}

#[tokio::test]
#[ignore = "audit: a code node must execute its declared script or fail loudly"]
async fn audit_workflow_code_node_executes_script_or_fails_loudly() {
    let (_root, db, chat, session) = workflow_fixture().await;
    let workflow: WorkflowRequest = serde_json::from_value(serde_json::json!({
        "definition": {
            "version": 1,
            "nodes": [
                {"id":"start","kind":"start"},
                {"id":"code","kind":"code","config":{
                    "codeLanguage":"javascript",
                    "codeScript":"this is intentionally invalid JavaScript; throw new Error('CODE_CANARY')",
                    "codeInputVariables":[{"name":"input","value":"SOURCE"}],
                    "codeOutputVariables":[{"name":"result","type":"string"}]
                }},
                {"id":"end","kind":"end"}
            ],
            "edges": [
                {"id":"e1","source":"start","target":"code"},
                {"id":"e2","source":"code","target":"end"}
            ]
        }
    }))
    .unwrap();
    let events = Arc::new(Mutex::new(Vec::new()));
    let capture = events.clone();
    let result = chat
        .run_workflow(
            workflow,
            "audit-workflow-code-execution".into(),
            session,
            move |event| {
                if let RuntimeEvent::WorkflowNode { event } = event {
                    capture.lock().unwrap().push(event);
                }
            },
            |_| {},
        )
        .await;
    let run = RunStore::new(&db)
        .get("audit-workflow-code-execution")
        .unwrap()
        .unwrap();
    let events = events.lock().unwrap();
    let code_terminal = events
        .iter()
        .find(|event| event.node_id == "code" && event.status.is_terminal())
        .unwrap();
    eprintln!(
        "api_ok={} code_status={:?} code_outputs={:?} code_error={:?} run_terminal={:?}",
        result.is_ok(),
        code_terminal.status,
        code_terminal.outputs,
        code_terminal.error,
        run.terminal_kind
    );
    assert!(
        result.is_err() || code_terminal.status == NodeExecutionStatus::Failed,
        "an invalid declared script must not be reported as a successful code node"
    );
}

#[tokio::test]
#[ignore = "audit: an enabled document extractor must have a real executor"]
async fn audit_workflow_enabled_document_extractor_has_executor() {
    let (_root, db, chat, session) = workflow_fixture().await;
    let workflow: WorkflowRequest = serde_json::from_value(serde_json::json!({
        "definition": {
            "version": 1,
            "nodes": [
                {"id":"start","kind":"start"},
                {"id":"extract","kind":"document-extractor","config":{
                    "docExtractorFileVariable":"input_file",
                    "docExtractorIsArray":false
                }},
                {"id":"end","kind":"end"}
            ],
            "edges": [
                {"id":"e1","source":"start","target":"extract"},
                {"id":"e2","source":"extract","target":"end"}
            ]
        }
    }))
    .unwrap();
    let events = Arc::new(Mutex::new(Vec::new()));
    let capture = events.clone();
    let result = chat
        .run_workflow(
            workflow,
            "audit-workflow-document-extractor".into(),
            session,
            move |event| {
                if let RuntimeEvent::WorkflowNode { event } = event {
                    capture.lock().unwrap().push(event);
                }
            },
            |_| {},
        )
        .await;
    let run = RunStore::new(&db)
        .get("audit-workflow-document-extractor")
        .unwrap()
        .unwrap();
    let events = events.lock().unwrap();
    let extractor_terminal = events
        .iter()
        .find(|event| event.node_id == "extract" && event.status.is_terminal())
        .unwrap();
    eprintln!(
        "api_ok={} extractor_status={:?} extractor_outputs={:?} extractor_error={:?} run_terminal={:?}",
        result.is_ok(),
        extractor_terminal.status,
        extractor_terminal.outputs,
        extractor_terminal.error,
        run.terminal_kind
    );
    assert!(
        result.is_ok() && extractor_terminal.status == NodeExecutionStatus::Completed,
        "an enabled document-extractor must not fail as an unimplemented node"
    );
}

#[tokio::test]
#[ignore = "audit: only the selected conditional edge may dispatch its exclusive descendants"]
async fn audit_workflow_unselected_branch_is_not_dispatched() {
    let mut violations = Vec::new();
    for choose_true in [true, false] {
        let (_root, db, chat, session) = workflow_fixture().await;
        // Use the backend's own condition shape to isolate dispatch from the
        // separate desktop/backend configuration mapping boundary.
        let workflow: WorkflowRequest = serde_json::from_value(serde_json::json!({
            "definition": {
                "version": 1,
                "nodes": [
                    {"id":"start","kind":"start"},
                    {"id":"decide","kind":"if-else","config":{"conditions":[{
                        "id":"true","logicalOperator":"and",
                        "conditions":[{"left":choose_true,"operator":"is","right":true}]
                    }]}},
                    {"id":"yes","kind":"template-transform","config":{"template":"TRUE_PATH"}},
                    {"id":"no","kind":"template-transform","config":{"template":"FALSE_PATH"}},
                    {"id":"end","kind":"end"}
                ],
                "edges": [
                    {"id":"e1","source":"start","target":"decide"},
                    {"id":"e2","source":"decide","source_handle":"true","target":"yes"},
                    {"id":"e3","source":"decide","source_handle":"false","target":"no"},
                    {"id":"e4","source":"yes","target":"end"},
                    {"id":"e5","source":"no","target":"end"}
                ]
            }
        }))
        .unwrap();
        let events = Arc::new(Mutex::new(Vec::new()));
        let capture = events.clone();
        chat.run_workflow(
            workflow,
            "audit-workflow-branch".into(),
            session,
            move |event| {
                if let RuntimeEvent::WorkflowNode { event } = event {
                    capture.lock().unwrap().push(event);
                }
            },
            |_| {},
        )
        .await
        .unwrap();
        let events = events.lock().unwrap();
        let decision = events
            .iter()
            .find(|e| e.node_id == "decide" && e.status == NodeExecutionStatus::Completed)
            .unwrap();
        let selected = if choose_true { "yes" } else { "no" };
        let unselected = if choose_true { "no" } else { "yes" };
        assert_eq!(
            decision.outputs.as_ref().unwrap()["branch"],
            if choose_true { "true" } else { "false" }
        );
        assert!(events
            .iter()
            .any(|e| e.node_id == selected && e.status == NodeExecutionStatus::Completed));
        let unselected_started = events
            .iter()
            .any(|e| e.node_id == unselected && e.status == NodeExecutionStatus::Running);
        let unselected_output = events
            .iter()
            .find(|e| e.node_id == unselected && e.status == NodeExecutionStatus::Completed)
            .and_then(|e| e.outputs.as_ref());
        let run = RunStore::new(&db)
            .get("audit-workflow-branch")
            .unwrap()
            .unwrap();
        eprintln!("choose_true={choose_true} decision={:?} unselected_started={unselected_started} unselected_output={unselected_output:?} terminal={:?}", decision.outputs, run.terminal_kind);
        if unselected_started || unselected_output.is_some() {
            violations.push(choose_true);
        }
    }
    assert!(
        violations.is_empty(),
        "exclusive unselected paths executed for decisions {violations:?}"
    );
}

#[tokio::test]
#[ignore = "audit: requires local Node and desktop TypeScript dependencies; tests the actual TS/Rust seam"]
async fn audit_desktop_workflow_config_survives_backend_execution() {
    let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../apps/desktop/scripts/audit-workflow-contract.cjs");
    let output = std::process::Command::new("node")
        .arg(script)
        .arg("--emit-fixtures")
        .output()
        .expect("local Node is required for this opt-in audit");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let fixtures: Vec<serde_json::Value> = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(fixtures.len(), 4);
    let mut violations = Vec::new();
    for fixture in fixtures {
        let name = fixture["name"].as_str().unwrap();
        let workflow: WorkflowRequest =
            serde_json::from_value(fixture["workflow"].clone()).unwrap();
        let (_root, db, chat, session) = workflow_fixture().await;
        let events = Arc::new(Mutex::new(Vec::new()));
        let capture = events.clone();
        let result = chat
            .run_workflow(
                workflow,
                "audit-desktop-workflow-contract".into(),
                session,
                move |event| {
                    if let RuntimeEvent::WorkflowNode { event } = event {
                        capture.lock().unwrap().push(event);
                    }
                },
                |_| {},
            )
            .await;
        let events = events.lock().unwrap();
        let node_id = if matches!(name, "input" | "output") {
            "end"
        } else {
            "inspect"
        };
        let actual = events
            .iter()
            .find(|e| e.node_id == node_id && e.status == NodeExecutionStatus::Completed)
            .and_then(|e| e.outputs.clone());
        let expected = match name {
            "input" => serde_json::json!({"report":"BLUE"}),
            "output" => serde_json::json!({"report":"EXPECTED_REPORT"}),
            "condition" => serde_json::json!({"branch":"true"}),
            "template" => serde_json::json!({"output":"EXPECTED_TEMPLATE"}),
            _ => panic!("unexpected fixture"),
        };
        let terminal = RunStore::new(&db)
            .get("audit-desktop-workflow-contract")
            .unwrap()
            .and_then(|r| r.terminal_kind);
        eprintln!("fixture={name} api_error={:?} actual={actual:?} expected={expected} terminal={terminal:?}", result.as_ref().err());
        if result.is_err() || actual.as_ref() != Some(&expected) {
            violations.push(name.to_owned());
        }
    }
    assert!(
        violations.is_empty(),
        "desktop settings were lost or ignored across the execution seam: {violations:?}"
    );
}
