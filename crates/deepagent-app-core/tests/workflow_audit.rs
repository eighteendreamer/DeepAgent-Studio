#[path = "../../deepagent-models/tests/support/sequence.rs"]
pub mod sequence_fixture;
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

#[path = "workflow_audit/aggregator.rs"]
mod aggregator;
#[path = "workflow_audit/answer.rs"]
mod answer;
#[path = "workflow_audit/branches.rs"]
mod branches;
#[path = "workflow_audit/code.rs"]
mod code;
#[path = "workflow_audit/conditions.rs"]
mod conditions;
#[path = "workflow_audit/entry.rs"]
mod entry;
#[path = "workflow_audit/exit.rs"]
mod exit;
#[path = "workflow_audit/http.rs"]
mod http;
#[path = "workflow_audit/inputs.rs"]
mod inputs;
#[path = "workflow_audit/iteration.rs"]
mod iteration;
#[path = "workflow_audit/output.rs"]
mod output;
#[path = "workflow_audit/reachability.rs"]
mod reachability;
#[path = "workflow_audit/settlement.rs"]
mod settlement;
#[path = "workflow_audit/target.rs"]
mod target;
#[path = "workflow_audit/usage.rs"]
mod usage;

async fn workflow_fixture() -> (tempfile::TempDir, Arc<Database>, ChatService, String) {
    let root = tempfile::tempdir().unwrap();
    let db = Arc::new(Database::open_in_memory().unwrap());
    workflow_fixture_with_database(root, db).await
}

async fn workflow_fixture_with_database(
    root: tempfile::TempDir,
    db: Arc<Database>,
) -> (tempfile::TempDir, Arc<Database>, ChatService, String) {
    std::fs::write(root.path().join("input.txt"), "audit input").unwrap();
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
async fn h252_invalid_workspace_is_rejected_even_without_a_canvas_gateway() {
    for (index, scope) in ["", " ", "bad/path"].iter().enumerate() {
        let (_root, db, chat, session) = workflow_fixture().await;
        let workflow = serde_json::from_value(serde_json::json!({
            "workspace_id": scope,
            "definition": {"version":1, "nodes":[{"id":"end","kind":"end"}],"edges":[]}
        }))
        .unwrap();
        let run_id = format!("h252-invalid-scope-{index}");
        let result = chat
            .run_workflow(workflow, run_id.clone(), session, |_| {}, |_| {})
            .await;
        assert!(result.unwrap_err().to_string().contains("workspace id"));
        let terminal: Vec<_> = RunStore::new(&db)
            .events_after(&run_id, None)
            .unwrap()
            .into_iter()
            .filter(|event| event.event_type == "run_terminal")
            .collect();
        assert_eq!(terminal.len(), 1);
        assert_eq!(terminal[0].data["kind"], "failed");
    }
}

#[tokio::test]
async fn workflow_projects_one_identity_bound_session_terminal() {
    let (_root, db, chat, session) = workflow_fixture().await;
    let workflow: WorkflowRequest = serde_json::from_value(serde_json::json!({
        "definition": {"version":1, "nodes":[{"id":"start","kind":"start"},{"id":"end","kind":"end"}],
            "edges":[{"id":"edge","source":"start","target":"end"}]}
    })).unwrap();
    chat.run_workflow(
        workflow,
        "workflow-projection".into(),
        session.clone(),
        |_| {},
        |_| {},
    )
    .await
    .unwrap();
    let store = RunStore::new(&db);
    let record = store.get("workflow-projection").unwrap().unwrap();
    assert_eq!(record.terminal_kind.as_deref(), Some("succeeded"));
    let events = deepagent_persistence::event_store::EventStore::new(&db)
        .load_session(session.parse().unwrap())
        .unwrap();
    assert_eq!(events.iter().filter(|event| matches!(&event.payload,
        deepagent_core::event::EventPayload::RunTerminal { run_id: Some(run), task_id: Some(task), kind, .. }
            if run == "workflow-projection" && record.task_id.as_deref() == Some(task.to_string().as_str()) && kind == "succeeded"
    )).count(), 1);
    let ledger = store.events_after("workflow-projection", None).unwrap();
    for event_type in ["run_terminal", "session_terminal_projected"] {
        assert_eq!(
            ledger
                .iter()
                .filter(|event| event.event_type == event_type)
                .count(),
            1
        );
    }
}

#[tokio::test]
async fn audit_workflow_unknown_tool_propagates_node_and_run_failure() {
    let (_root, db, chat, session) = workflow_fixture().await;
    let workflow: WorkflowRequest = serde_json::from_value(serde_json::json!({
        "definition": {
            "version": 1,
            "nodes": [
                {"id":"start","kind":"start"},
                {"id":"inspect","kind":"tool","config":{"toolName":"audit_unknown_tool","toolArguments":{}}},
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
async fn workflow_read_file_uses_shared_registry_and_completes() {
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
    chat.run_workflow(
        workflow,
        "workflow-read-file-success".into(),
        session,
        move |event| {
            if let RuntimeEvent::WorkflowNode { event } = event {
                capture.lock().unwrap().push(event);
            }
        },
        |_| {},
    )
    .await
    .expect("registered read_file must execute");

    let run = RunStore::new(&db)
        .get("workflow-read-file-success")
        .unwrap()
        .unwrap();
    let events = events.lock().unwrap();
    let inspect = events
        .iter()
        .find(|event| event.node_id == "inspect" && event.status.is_terminal())
        .expect("tool node terminal event");
    assert_eq!(inspect.status, NodeExecutionStatus::Completed);
    assert_eq!(
        inspect
            .outputs
            .as_ref()
            .and_then(|value| value.get("success")),
        Some(&serde_json::Value::Bool(true))
    );
    assert!(events
        .iter()
        .any(|event| event.node_id == "end" && event.status == NodeExecutionStatus::Completed));
    assert_eq!(run.terminal_kind.as_deref(), Some("succeeded"));
}

#[tokio::test]
async fn workflow_tool_output_failure_propagates_without_downstream_completion() {
    let (_root, db, chat, session) = workflow_fixture().await;
    let workflow: WorkflowRequest = serde_json::from_value(serde_json::json!({
        "definition": {
            "version": 1,
            "nodes": [
                {"id":"start","kind":"start"},
                {"id":"inspect","kind":"tool","config":{"toolName":"read_file","toolArguments":{"path":"missing-input.txt"}}},
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
            "workflow-tool-output-failure".into(),
            session,
            move |event| {
                if let RuntimeEvent::WorkflowNode { event } = event {
                    capture.lock().unwrap().push(event);
                }
            },
            |_| {},
        )
        .await;
    assert!(
        result.is_err(),
        "a failed ToolOutput must fail the workflow"
    );
    let run = RunStore::new(&db)
        .get("workflow-tool-output-failure")
        .unwrap()
        .unwrap();
    let events = events.lock().unwrap();
    let inspect = events
        .iter()
        .find(|event| event.node_id == "inspect" && event.status.is_terminal())
        .expect("failed tool node terminal event");
    assert_eq!(inspect.status, NodeExecutionStatus::Failed);
    assert!(inspect.error.is_some());
    assert!(!events
        .iter()
        .any(|event| event.node_id == "end" && event.status == NodeExecutionStatus::Completed));
    assert_ne!(run.terminal_kind.as_deref(), Some("succeeded"));
}

#[tokio::test]
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
    let nodes = Arc::new(Mutex::new(Vec::new()));
    let captured_nodes = nodes.clone();

    let result = tokio::time::timeout(
        Duration::from_secs(5),
        chat.run_workflow(
            workflow,
            "audit-workflow-http-network-boundary".into(),
            session,
            move |event| {
                if let RuntimeEvent::WorkflowNode { event } = event {
                    captured_nodes.lock().unwrap().push(event);
                }
            },
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
    // A correct denial never connects. The original unbounded accept waiter
    // would hang precisely when the regression was fixed.
    let mut server = server;
    if tokio::time::timeout(Duration::from_millis(100), &mut server)
        .await
        .is_err()
    {
        server.abort();
        assert!(server.await.unwrap_err().is_cancelled());
    }

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
    assert_eq!(
        approvals.load(Ordering::SeqCst),
        0,
        "a hard destination denial is not overridable by approval"
    );
    assert!(result.is_err());
    assert_ne!(run.terminal_kind.as_deref(), Some("succeeded"));
    let nodes = nodes.lock().unwrap();
    assert!(nodes
        .iter()
        .any(|node| node.node_id == "request" && node.status == NodeExecutionStatus::Failed));
    assert!(!nodes
        .iter()
        .any(|node| matches!(node.node_id.as_str(), "request" | "end")
            && node.status == NodeExecutionStatus::Completed));
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
async fn audit_workflow_code_node_executes_script_or_fails_loudly() {
    let (_root, db, chat, session) = workflow_fixture().await;
    let responder = chat.clone();
    let approvals = Arc::new(AtomicUsize::new(0));
    let count = approvals.clone();
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
            move |approval| {
                count.fetch_add(1, Ordering::SeqCst);
                assert_eq!(approval.tool, "canvas_code_execute");
                assert!(responder
                    .resolve_approval(&approval.call_id, true, "h178 isolated audit fixture")
                    .unwrap());
            },
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
    assert_eq!(approvals.load(Ordering::SeqCst), 1);
    code::assert_failed(&db, "audit-workflow-code-execution", &result, &events);
    // The original invalid source is retained. This fixture has no Broker:
    // rejection is not proof that JavaScript was parsed or executed.
    assert!(code_terminal
        .error
        .as_deref()
        .unwrap()
        .contains("CodeRuntimeUnavailable"));
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
#[ignore = "audit: an enabled human-input node must enter the shared input/approval lifecycle"]
async fn audit_workflow_human_input_uses_shared_waiting_lifecycle() {
    let (_root, db, chat, session) = workflow_fixture().await;
    let workflow: WorkflowRequest = serde_json::from_value(serde_json::json!({
        "definition": {
            "version": 1,
            "nodes": [
                {"id":"start","kind":"start"},
                {"id":"review","kind":"human-input","config":{
                    "humanInputPrompt":"Approve this audit fixture?",
                    "humanInputFields":[{"name":"approved","type":"boolean","label":"Approved","required":true}],
                    "timeoutSeconds":30
                }},
                {"id":"end","kind":"end"}
            ],
            "edges": [
                {"id":"e1","source":"start","target":"review"},
                {"id":"e2","source":"review","source_handle":"action-0","target":"end"}
            ]
        }
    }))
    .unwrap();
    let events = Arc::new(Mutex::new(Vec::new()));
    let capture = events.clone();
    let result = chat
        .run_workflow(
            workflow,
            "audit-workflow-human-input".into(),
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
        .get("audit-workflow-human-input")
        .unwrap()
        .unwrap();
    let events = events.lock().unwrap();
    let review_events: Vec<_> = events
        .iter()
        .filter(|event| event.node_id == "review")
        .map(|event| (event.status, event.outputs.clone(), event.error.clone()))
        .collect();
    eprintln!(
        "api_ok={} api_error={:?} review_events={review_events:?} run_terminal={:?}",
        result.is_ok(),
        result.as_ref().err(),
        run.terminal_kind
    );
    assert!(
        result.is_ok(),
        "an enabled human-input node must enter a shared waiting/input lifecycle instead of failing"
    );
}

#[tokio::test]
#[ignore = "audit: an enabled iteration node must process its declared input array"]
async fn audit_workflow_iteration_processes_frontend_input_contract() {
    let (_root, db, chat, session) = workflow_fixture().await;
    let workflow: WorkflowRequest = serde_json::from_value(serde_json::json!({
        "definition": {
            "version": 1,
            "nodes": [
                {"id":"start","kind":"start","config":{
                    "inputVariables":[{"name":"items","variable":"items","type":"array"}]
                }},
                {"id":"iterate","kind":"iteration","config":{
                    "inputVariable":"{{#start.items#}}",
                    "outputVariable":"item",
                    "parallel":false,
                    "maxConcurrency":1,
                    "errorHandling":"terminate",
                    "flatten":false
                }},
                {"id":"end","kind":"end"}
            ],
            "edges": [
                {"id":"e1","source":"start","target":"iterate"},
                {"id":"e2","source":"iterate","source_handle":"item","target":"end"}
            ]
        },
        "inputs":{"items":["ONE","TWO"]}
    }))
    .unwrap();
    let events = Arc::new(Mutex::new(Vec::new()));
    let capture = events.clone();
    let result = chat
        .run_workflow(
            workflow,
            "audit-workflow-iteration-contract".into(),
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
        .get("audit-workflow-iteration-contract")
        .unwrap()
        .unwrap();
    let events = events.lock().unwrap();
    let iteration_events: Vec<_> = events
        .iter()
        .filter(|event| event.node_id == "iterate")
        .map(|event| (event.status, event.outputs.clone(), event.error.clone()))
        .collect();
    eprintln!(
        "api_ok={} api_error={:?} iteration_events={iteration_events:?} run_terminal={:?}",
        result.is_ok(),
        result.as_ref().err(),
        run.terminal_kind
    );
    assert!(
        result.is_ok()
            && iteration_events.iter().any(|(status, outputs, _)| {
                *status == NodeExecutionStatus::Completed
                    && *outputs == Some(serde_json::json!({"output":["ONE","TWO"]}))
            }),
        "the enabled iteration node must process the frontend inputVariable contract"
    );
}

#[tokio::test]
#[ignore = "audit: an enabled answer node must render its declared frontend template"]
async fn audit_workflow_answer_node_renders_frontend_contract() {
    let (_root, db, chat, session) = workflow_fixture().await;
    let workflow: WorkflowRequest = serde_json::from_value(serde_json::json!({
        "definition": {
            "version": 1,
            "nodes": [
                {"id":"start","kind":"start"},
                {"id":"answer","kind":"answer","config":{
                    "answerTemplate":"EXPECTED_ANSWER",
                    "answerVariables":[]
                }},
                {"id":"end","kind":"end"}
            ],
            "edges": [
                {"id":"e1","source":"start","target":"answer"},
                {"id":"e2","source":"answer","target":"end"}
            ]
        }
    }))
    .unwrap();
    let events = Arc::new(Mutex::new(Vec::new()));
    let capture = events.clone();
    let result = chat
        .run_workflow(
            workflow,
            "audit-workflow-answer-contract".into(),
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
        .get("audit-workflow-answer-contract")
        .unwrap()
        .unwrap();
    let events = events.lock().unwrap();
    let answer = events
        .iter()
        .find(|event| event.node_id == "answer" && event.status == NodeExecutionStatus::Completed)
        .unwrap();
    eprintln!(
        "api_ok={} answer_outputs={:?} run_terminal={:?}",
        result.is_ok(),
        answer.outputs,
        run.terminal_kind
    );
    assert_eq!(
        answer.outputs,
        Some(serde_json::json!({"answer":"EXPECTED_ANSWER"})),
        "the enabled answer node must render answerTemplate instead of an empty fallback"
    );
}

#[tokio::test]
#[ignore = "audit: an enabled variable aggregator must consume declared frontend bindings"]
async fn audit_workflow_variable_aggregator_consumes_frontend_bindings() {
    let (_root, db, chat, session) = workflow_fixture().await;
    let workflow: WorkflowRequest = serde_json::from_value(serde_json::json!({
        "definition": {
            "version": 1,
            "nodes": [
                {"id":"start","kind":"start","config":{
                    "inputVariables":[
                        {"name":"first","variable":"first","type":"string"},
                        {"name":"second","variable":"second","type":"string"}
                    ]
                }},
                {"id":"aggregate","kind":"variable-aggregator","config":{
                    "aggregatorOutputType":"array",
                    "aggregatorVariables":["{{#start.first#}}","{{#start.second#}}"]
                }},
                {"id":"end","kind":"end"}
            ],
            "edges": [
                {"id":"e1","source":"start","target":"aggregate"},
                {"id":"e2","source":"aggregate","target":"end"}
            ]
        },
        "inputs":{"first":"FIRST","second":"SECOND"}
    }))
    .unwrap();
    let events = Arc::new(Mutex::new(Vec::new()));
    let capture = events.clone();
    let result = chat
        .run_workflow(
            workflow,
            "audit-workflow-aggregator-contract".into(),
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
        .get("audit-workflow-aggregator-contract")
        .unwrap()
        .unwrap();
    let events = events.lock().unwrap();
    let aggregate = events
        .iter()
        .find(|event| {
            event.node_id == "aggregate" && event.status == NodeExecutionStatus::Completed
        })
        .unwrap();
    eprintln!(
        "api_ok={} aggregate_outputs={:?} run_terminal={:?}",
        result.is_ok(),
        aggregate.outputs,
        run.terminal_kind
    );
    assert_eq!(
        aggregate.outputs,
        Some(serde_json::json!({"output":["FIRST","SECOND"]})),
        "the enabled aggregator must consume aggregatorVariables"
    );
}

// The original probes assumed a Completed start existed even on rejection.
// Admission now happens before every node: assert the stronger safety boundary,
// rather than fabricating a success event to satisfy that historical unwrap.
fn assert_workflow_input_admission_rejected(
    db: &Database,
    result: &deepagent_core::error::Result<String>,
    run: &deepagent_persistence::run_store::RunRecord,
    events: &[deepagent_runtime::workflow::NodeExecutionEvent],
    code: &str,
) {
    eprintln!(
        "api_ok={} node_events={} run_terminal={:?} reason={:?}",
        result.is_ok(),
        events.len(),
        run.terminal_kind,
        run.terminal_reason
    );
    assert!(result.as_ref().unwrap_err().to_string().contains(code));
    assert_eq!(run.terminal_kind.as_deref(), Some("failed"));
    assert!(run.terminal_reason.as_deref().unwrap().contains(code));
    assert!(events.is_empty(), "no node may start for rejected inputs");
    let ledger = RunStore::new(db).events_after(&run.id, None).unwrap();
    assert_eq!(
        ledger
            .iter()
            .filter(|event| event.event_type == "run_terminal")
            .count(),
        1
    );
    assert!(deepagent_persistence::run_control::RunControlStore::new(db)
        .list_actions(&run.id)
        .unwrap()
        .is_empty());
}

#[tokio::test]
#[ignore = "audit: missing required workflow input must fail admission instead of becoming null"]
async fn audit_workflow_missing_required_input_is_rejected() {
    let (_root, db, chat, session) = workflow_fixture().await;
    let workflow: WorkflowRequest = serde_json::from_value(serde_json::json!({
        "definition": {
            "version": 1,
            "nodes": [
                {"id":"start","kind":"start","config":{
                    "inputVariables":[{
                        "name":"customer",
                        "variable":"customer",
                        "type":"string",
                        "required":true
                    }]
                }},
                {"id":"end","kind":"end"}
            ],
            "edges":[{"id":"e1","source":"start","target":"end"}]
        },
        "inputs":{}
    }))
    .unwrap();
    let events = Arc::new(Mutex::new(Vec::new()));
    let capture = events.clone();
    let result = chat
        .run_workflow(
            workflow,
            "audit-workflow-required-input".into(),
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
        .get("audit-workflow-required-input")
        .unwrap()
        .unwrap();
    let events = events.lock().unwrap();
    assert_workflow_input_admission_rejected(&db, &result, &run, &events, "MissingWorkflowInput");
}

#[tokio::test]
#[ignore = "audit: workflow input values must match declared variable types"]
async fn audit_workflow_input_type_mismatch_is_rejected() {
    let (_root, db, chat, session) = workflow_fixture().await;
    let workflow: WorkflowRequest = serde_json::from_value(serde_json::json!({
        "definition": {
            "version": 1,
            "nodes": [
                {"id":"start","kind":"start","config":{
                    "inputVariables":[{
                        "name":"count",
                        "variable":"count",
                        "type":"number",
                        "required":true
                    }]
                }},
                {"id":"end","kind":"end"}
            ],
            "edges":[{"id":"e1","source":"start","target":"end"}]
        },
        "inputs":{"count":"not-a-number"}
    }))
    .unwrap();
    let events = Arc::new(Mutex::new(Vec::new()));
    let capture = events.clone();
    let result = chat
        .run_workflow(
            workflow,
            "audit-workflow-input-type".into(),
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
        .get("audit-workflow-input-type")
        .unwrap()
        .unwrap();
    let events = events.lock().unwrap();
    assert_workflow_input_admission_rejected(&db, &result, &run, &events, "InvalidWorkflowInput");
}

#[tokio::test]
#[ignore = "audit: undeclared workflow inputs must not enter the start output"]
async fn audit_workflow_undeclared_input_is_rejected_or_ignored() {
    let (_root, db, chat, session) = workflow_fixture().await;
    let workflow: WorkflowRequest = serde_json::from_value(serde_json::json!({
        "definition": {
            "version": 1,
            "nodes": [
                {"id":"start","kind":"start","config":{
                    "inputVariables":[{
                        "name":"known",
                        "variable":"known",
                        "type":"string",
                        "required":true
                    }]
                }},
                {"id":"end","kind":"end"}
            ],
            "edges":[{"id":"e1","source":"start","target":"end"}]
        },
        "inputs":{"known":"KNOWN","unexpected":"UNDECLARED_CANARY"}
    }))
    .unwrap();
    let events = Arc::new(Mutex::new(Vec::new()));
    let capture = events.clone();
    let result = chat
        .run_workflow(
            workflow,
            "audit-workflow-undeclared-input".into(),
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
        .get("audit-workflow-undeclared-input")
        .unwrap()
        .unwrap();
    let events = events.lock().unwrap();
    assert_workflow_input_admission_rejected(&db, &result, &run, &events, "UnknownWorkflowInput");
}

#[tokio::test]
#[ignore = "audit: declared workflow defaults must be applied before execution"]
async fn audit_workflow_declared_default_is_applied_when_input_is_missing() {
    let (_root, db, chat, session) = workflow_fixture().await;
    let workflow: WorkflowRequest = serde_json::from_value(serde_json::json!({
        "definition": {
            "version": 1,
            "nodes": [
                {"id":"start","kind":"start","config":{
                    "inputVariables":[{
                        "name":"greeting",
                        "variable":"greeting",
                        "type":"string",
                        "default":"FALLBACK"
                    }]
                }},
                {"id":"end","kind":"end"}
            ],
            "edges":[{"id":"e1","source":"start","target":"end"}]
        },
        "inputs":{}
    }))
    .unwrap();
    let events = Arc::new(Mutex::new(Vec::new()));
    let capture = events.clone();
    let result = chat
        .run_workflow(
            workflow,
            "audit-workflow-default-input".into(),
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
        .get("audit-workflow-default-input")
        .unwrap()
        .unwrap();
    let events = events.lock().unwrap();
    let start = events
        .iter()
        .find(|event| event.node_id == "start" && event.status == NodeExecutionStatus::Completed)
        .unwrap();
    let greeting = start
        .outputs
        .as_ref()
        .and_then(|outputs| outputs.get("greeting"))
        .cloned();
    eprintln!(
        "api_ok={} greeting={greeting:?} run_terminal={:?}",
        result.is_ok(),
        run.terminal_kind
    );
    assert_eq!(
        greeting,
        Some(serde_json::json!("FALLBACK")),
        "a missing optional input with a declared default must use that default"
    );
}

#[tokio::test]
#[ignore = "audit: declared workflow options must constrain string inputs"]
async fn audit_workflow_string_options_are_enforced() {
    let (_root, db, chat, session) = workflow_fixture().await;
    let workflow: WorkflowRequest = serde_json::from_value(serde_json::json!({
        "definition": {
            "version": 1,
            "nodes": [
                {"id":"start","kind":"start","config":{
                    "inputVariables":[{
                        "name":"mode",
                        "variable":"mode",
                        "type":"string",
                        "options":["safe","fast"]
                    }]
                }},
                {"id":"end","kind":"end"}
            ],
            "edges":[{"id":"e1","source":"start","target":"end"}]
        },
        "inputs":{"mode":"UNDECLARED_MODE"}
    }))
    .unwrap();
    let events = Arc::new(Mutex::new(Vec::new()));
    let capture = events.clone();
    let result = chat
        .run_workflow(
            workflow,
            "audit-workflow-input-options".into(),
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
        .get("audit-workflow-input-options")
        .unwrap()
        .unwrap();
    let events = events.lock().unwrap();
    assert_workflow_input_admission_rejected(&db, &result, &run, &events, "InvalidWorkflowInput");
}

#[tokio::test]
#[ignore = "audit: declared workflow numeric bounds must constrain inputs"]
async fn audit_workflow_numeric_bounds_are_enforced() {
    let (_root, db, chat, session) = workflow_fixture().await;
    let workflow: WorkflowRequest = serde_json::from_value(serde_json::json!({
        "definition": {
            "version": 1,
            "nodes": [
                {"id":"start","kind":"start","config":{
                    "inputVariables":[{
                        "name":"retries",
                        "variable":"retries",
                        "type":"number",
                        "minimum":10,
                        "maximum":20
                    }]
                }},
                {"id":"end","kind":"end"}
            ],
            "edges":[{"id":"e1","source":"start","target":"end"}]
        },
        "inputs":{"retries":3}
    }))
    .unwrap();
    let events = Arc::new(Mutex::new(Vec::new()));
    let capture = events.clone();
    let result = chat
        .run_workflow(
            workflow,
            "audit-workflow-numeric-bounds".into(),
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
        .get("audit-workflow-numeric-bounds")
        .unwrap()
        .unwrap();
    let events = events.lock().unwrap();
    assert_workflow_input_admission_rejected(&db, &result, &run, &events, "InvalidWorkflowInput");
}

#[tokio::test]
#[ignore = "audit: declared workflow string length bounds must constrain inputs"]
async fn audit_workflow_string_length_bounds_are_enforced() {
    let (_root, db, chat, session) = workflow_fixture().await;
    let workflow: WorkflowRequest = serde_json::from_value(serde_json::json!({
        "definition": {
            "version": 1,
            "nodes": [
                {"id":"start","kind":"start","config":{
                    "inputVariables":[{
                        "name":"token",
                        "variable":"token",
                        "type":"string",
                        "minLength":5,
                        "maxLength":8
                    }]
                }},
                {"id":"end","kind":"end"}
            ],
            "edges":[{"id":"e1","source":"start","target":"end"}]
        },
        "inputs":{"token":"x"}
    }))
    .unwrap();
    let events = Arc::new(Mutex::new(Vec::new()));
    let capture = events.clone();
    let result = chat
        .run_workflow(
            workflow,
            "audit-workflow-string-length-bounds".into(),
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
        .get("audit-workflow-string-length-bounds")
        .unwrap()
        .unwrap();
    let events = events.lock().unwrap();
    assert_workflow_input_admission_rejected(&db, &result, &run, &events, "InvalidWorkflowInput");
}

#[tokio::test]
async fn audit_workflow_requires_a_start_node() {
    let (_root, db, chat, session) = workflow_fixture().await;
    let workflow: WorkflowRequest = serde_json::from_value(serde_json::json!({
        "definition": {
            "version": 1,
            "nodes": [
                {"id":"end","kind":"end"}
            ],
            "edges":[]
        },
        "inputs":{}
    }))
    .unwrap();
    let events = Arc::new(Mutex::new(Vec::new()));
    let capture = events.clone();
    let result = chat
        .run_workflow(
            workflow,
            "audit-workflow-missing-start".into(),
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
        .get("audit-workflow-missing-start")
        .unwrap()
        .unwrap();
    let events = events.lock().unwrap();
    eprintln!(
        "api_ok={} node_events={} run_terminal={:?}",
        result.is_ok(),
        events.len(),
        run.terminal_kind
    );
    entry::assert_rejected(&db, &result, &run.id, &events);
}

#[tokio::test]
async fn audit_workflow_requires_an_end_node() {
    let (_root, db, chat, session) = workflow_fixture().await;
    let workflow: WorkflowRequest = serde_json::from_value(serde_json::json!({
        "definition": {
            "version": 1,
            "nodes": [
                {"id":"start","kind":"start","config":{
                    "inputVariables":[]
                }}
            ],
            "edges":[]
        },
        "inputs":{}
    }))
    .unwrap();
    let events = Arc::new(Mutex::new(Vec::new()));
    let capture = events.clone();
    let result = chat
        .run_workflow(
            workflow,
            "audit-workflow-missing-end".into(),
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
        .get("audit-workflow-missing-end")
        .unwrap()
        .unwrap();
    let events = events.lock().unwrap();
    eprintln!(
        "api_ok={} node_events={} run_terminal={:?}",
        result.is_ok(),
        events.len(),
        run.terminal_kind
    );
    exit::assert_failed(&db, &result, &run.id, exit::MISSING_END);
    assert!(events.is_empty(), "no end means no node execution");
}

#[tokio::test]
async fn audit_workflow_requires_a_reachable_end_node() {
    let (_root, db, chat, session) = workflow_fixture().await;
    let workflow: WorkflowRequest = serde_json::from_value(serde_json::json!({
        "definition": {
            "version": 1,
            "nodes": [
                {"id":"start","kind":"start","config":{"inputVariables":[]}},
                {"id":"end","kind":"end"}
            ],
            "edges":[]
        },
        "inputs":{}
    }))
    .unwrap();
    let events = Arc::new(Mutex::new(Vec::new()));
    let capture = events.clone();
    let result = chat
        .run_workflow(
            workflow,
            "audit-workflow-unreachable-end".into(),
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
        .get("audit-workflow-unreachable-end")
        .unwrap()
        .unwrap();
    let events = events.lock().unwrap();
    eprintln!(
        "api_ok={} node_events={} run_terminal={:?}",
        result.is_ok(),
        events.len(),
        run.terminal_kind
    );
    reachability::assert_rejected(&db, &result, &run.id, &events);
}

#[tokio::test]
async fn audit_workflow_execution_scope_is_not_node_order_dependent() {
    async fn run_variant(nodes: serde_json::Value, run_id: &str) -> (Vec<String>, Option<String>) {
        let (_root, db, chat, session) = workflow_fixture().await;
        let workflow: WorkflowRequest = serde_json::from_value(serde_json::json!({
            "definition": {
                "version": 1,
                "nodes": nodes,
                "edges": [
                    {"id":"e1","source":"start","target":"end"}
                ]
            },
            "inputs":{}
        }))
        .unwrap();
        let events = Arc::new(Mutex::new(Vec::new()));
        let capture = events.clone();
        let result = chat
            .run_workflow(
                workflow,
                run_id.into(),
                session,
                move |event| {
                    if let RuntimeEvent::WorkflowNode { event } = event {
                        if event.status == NodeExecutionStatus::Completed {
                            capture.lock().unwrap().push(event.node_id);
                        }
                    }
                },
                |_| {},
            )
            .await;
        // H-193 now rejects both original orphan samples before dispatch.
        // The legal owned-tail regression lives in settlement, not here.
        let error = result.unwrap_err().to_string();
        assert!(
            error.contains("node orphan is unreachable from start start"),
            "{error}"
        );
        let terminal = RunStore::new(&db)
            .get(run_id)
            .unwrap()
            .unwrap()
            .terminal_kind;
        let completed = events.lock().unwrap().clone();
        (completed, terminal)
    }

    let (end_first, end_first_terminal) = run_variant(
        serde_json::json!([
            {"id":"start","kind":"start","config":{"inputVariables":[]}},
            {"id":"end","kind":"end"},
            {"id":"orphan","kind":"variable-assigner","config":{"assignments":[]}}
        ]),
        "audit-workflow-end-before-orphan",
    )
    .await;
    let (orphan_first, orphan_first_terminal) = run_variant(
        serde_json::json!([
            {"id":"start","kind":"start","config":{"inputVariables":[]}},
            {"id":"orphan","kind":"variable-assigner","config":{"assignments":[]}},
            {"id":"end","kind":"end"}
        ]),
        "audit-workflow-orphan-before-end",
    )
    .await;

    eprintln!(
        "end_first={end_first:?} terminal={end_first_terminal:?} orphan_first={orphan_first:?} terminal={orphan_first_terminal:?}"
    );
    assert_eq!(
        end_first, orphan_first,
        "reordering the same logical graph must not change which nodes execute"
    );
    assert!(end_first.is_empty());
    assert_eq!(end_first_terminal.as_deref(), Some("failed"));
    assert_eq!(orphan_first_terminal.as_deref(), Some("failed"));
}

#[tokio::test]
async fn audit_workflow_does_not_succeed_with_a_reachable_branch_unsettled() {
    let (_root, db, chat, session) = workflow_fixture().await;
    let workflow: WorkflowRequest = serde_json::from_value(serde_json::json!({
        "definition": {
            "version": 1,
            "nodes": [
                {"id":"start","kind":"start","config":{"inputVariables":[]}},
                {"id":"left","kind":"variable-assigner","config":{"assignments":[]}},
                {"id":"left-end","kind":"end"},
                {"id":"right","kind":"variable-assigner","config":{"assignments":[]}},
                {"id":"right-end","kind":"end"}
            ],
            "edges": [
                {"id":"left-1","source":"start","target":"left"},
                {"id":"left-2","source":"left","target":"left-end"},
                {"id":"right-1","source":"start","target":"right"},
                {"id":"right-2","source":"right","target":"right-end"}
            ]
        },
        "inputs":{}
    }))
    .unwrap();
    let events = Arc::new(Mutex::new(Vec::new()));
    let capture = events.clone();
    let result = chat
        .run_workflow(
            workflow,
            "audit-workflow-multiple-reachable-ends".into(),
            session,
            move |event| {
                if let RuntimeEvent::WorkflowNode { event } = event {
                    if event.status.is_terminal() {
                        capture.lock().unwrap().push((event.node_id, event.status));
                    }
                }
            },
            |_| {},
        )
        .await;
    let run = RunStore::new(&db)
        .get("audit-workflow-multiple-reachable-ends")
        .unwrap()
        .unwrap();
    let events = events.lock().unwrap();
    let right_settled = events.iter().any(|(node_id, _)| node_id == "right");
    eprintln!(
        "api_ok={} terminal_events={events:?} run_terminal={:?}",
        result.is_ok(),
        run.terminal_kind
    );
    assert!(result.is_ok(), "{result:?}");
    assert!(right_settled);
    assert_eq!(run.terminal_kind.as_deref(), Some("succeeded"));
    assert_eq!(events.len(), 5);
    assert!(events
        .iter()
        .all(|(_, status)| *status == NodeExecutionStatus::Completed));
    let ids: std::collections::BTreeSet<_> = events.iter().map(|(id, _)| id.as_str()).collect();
    assert_eq!(
        ids,
        ["start", "left", "left-end", "right", "right-end"]
            .into_iter()
            .collect()
    );
}

#[tokio::test]
async fn audit_workflow_target_node_keeps_required_ancestor_outputs() {
    fn workflow(target_node_id: Option<&str>) -> WorkflowRequest {
        serde_json::from_value(serde_json::json!({
            "definition": {
                "version": 1,
                "nodes": [
                    {"id":"start","kind":"start","config":{"inputVariables":[]}},
                    {"id":"source","kind":"variable-assigner","config":{"assignments":[{
                        "variable":"output",
                        "inputType":"constant",
                        "value":["SOURCE_VALUE"]
                    }]}},
                    {"id":"target","kind":"list-operator","config":{
                        "input":"{{#source.output#}}",
                        "action":"limit",
                        "orderBy":"",
                        "limit":0
                    }},
                    {"id":"end","kind":"end"}
                ],
                "edges": [
                    {"id":"e1","source":"start","target":"source"},
                    {"id":"e2","source":"source","target":"target"},
                    {"id":"e3","source":"target","target":"end"}
                ]
            },
            "inputs":{},
            "target_node_id":target_node_id
        }))
        .unwrap()
    }

    let (_full_root, full_db, full_chat, full_session) = workflow_fixture().await;
    let full_events = Arc::new(Mutex::new(Vec::new()));
    let full_capture = full_events.clone();
    let full_result = full_chat
        .run_workflow(
            workflow(None),
            "audit-workflow-target-baseline".into(),
            full_session,
            move |event| {
                if let RuntimeEvent::WorkflowNode { event } = event {
                    if event.status.is_terminal() {
                        full_capture.lock().unwrap().push(event);
                    }
                }
            },
            |_| {},
        )
        .await;
    let full_terminal = RunStore::new(&full_db)
        .get("audit-workflow-target-baseline")
        .unwrap()
        .unwrap()
        .terminal_kind;
    assert!(full_result.is_ok(), "the complete workflow is the control");

    let (_target_root, target_db, target_chat, target_session) = workflow_fixture().await;
    let target_events = Arc::new(Mutex::new(Vec::new()));
    let target_capture = target_events.clone();
    let target_result = target_chat
        .run_workflow(
            workflow(Some("target")),
            "audit-workflow-valid-target".into(),
            target_session,
            move |event| {
                if let RuntimeEvent::WorkflowNode { event } = event {
                    if event.status.is_terminal() {
                        target_capture.lock().unwrap().push(event);
                    }
                }
            },
            |_| {},
        )
        .await;
    let target_terminal = RunStore::new(&target_db)
        .get("audit-workflow-valid-target")
        .unwrap()
        .unwrap()
        .terminal_kind;
    let full_events = full_events.lock().unwrap();
    let target_events = target_events.lock().unwrap();
    let full_target = full_events
        .iter()
        .find(|event| event.node_id == "target")
        .and_then(|event| event.outputs.clone());
    let targeted_target = target_events.iter().find(|event| event.node_id == "target");
    let targeted_summary: Vec<_> = target_events
        .iter()
        .map(|event| (event.node_id.as_str(), event.status, event.error.as_deref()))
        .collect();
    eprintln!(
        "full_ok={} full_target={full_target:?} full_terminal={full_terminal:?} targeted_ok={} targeted_events={targeted_summary:?} targeted_terminal={target_terminal:?}",
        full_result.is_ok(),
        target_result.is_ok(),
    );
    assert!(
        target_result.is_ok()
            && targeted_target.is_some_and(|event| {
                event.status == NodeExecutionStatus::Completed && event.outputs == full_target
            }),
        "a valid target-node run must retain the ancestor data needed by that node"
    );
}

#[tokio::test]
async fn audit_workflow_end_output_matches_declared_type() {
    let (_root, db, chat, session) = workflow_fixture().await;
    let workflow: WorkflowRequest = serde_json::from_value(serde_json::json!({
        "definition": {
            "version": 1,
            "nodes": [
                {"id":"start","kind":"start","config":{"inputVariables":[{
                    "name":"raw",
                    "variable":"raw",
                    "type":"string"
                }]}},
                {"id":"end","kind":"end","config":{"outputVariables":[{
                    "name":"count",
                    "variable":"count",
                    "type":"number",
                    "value":"{{#start.raw#}}"
                }]}}
            ],
            "edges": [
                {"id":"e1","source":"start","target":"end"}
            ]
        },
        "inputs":{"raw":"NOT_A_NUMBER"}
    }))
    .unwrap();
    let events = Arc::new(Mutex::new(Vec::new()));
    let capture = events.clone();
    let result = chat
        .run_workflow(
            workflow,
            "audit-workflow-end-output-type".into(),
            session,
            move |event| {
                if let RuntimeEvent::WorkflowNode { event } = event {
                    if event.status.is_terminal() {
                        capture.lock().unwrap().push(event);
                    }
                }
            },
            |_| {},
        )
        .await;
    let run = RunStore::new(&db)
        .get("audit-workflow-end-output-type")
        .unwrap()
        .unwrap();
    let events = events.lock().unwrap();
    let end = events.iter().find(|event| event.node_id == "end");
    let count = end
        .and_then(|event| event.outputs.as_ref())
        .and_then(|outputs| outputs.get("count"));
    eprintln!(
        "api_ok={} end_status={:?} end_outputs={:?} run_terminal={:?}",
        result.is_ok(),
        end.map(|event| event.status),
        end.and_then(|event| event.outputs.as_ref()),
        run.terminal_kind
    );
    assert!(
        result.is_err()
            || end.is_some_and(|event| event.status == NodeExecutionStatus::Failed)
            || count.is_some_and(serde_json::Value::is_number),
        "a workflow cannot succeed with an output value that violates its declared type"
    );
}

#[tokio::test]
async fn audit_workflow_or_condition_does_not_start_from_true() {
    let (_root, db, chat, session) = workflow_fixture().await;
    let workflow: WorkflowRequest = serde_json::from_value(serde_json::json!({
        "definition": {
            "version": 1,
            "nodes": [
                {"id":"start","kind":"start"},
                {"id":"decide","kind":"if-else","config":{"conditions":[{
                    "id":"true",
                    "logicalOperator":"or",
                    "conditions":[
                        {"left":false,"operator":"is","right":true},
                        {"left":0,"operator":"gt","right":1}
                    ]
                }]}},
                {"id":"end","kind":"end"}
            ],
            "edges": [
                {"id":"e1","source":"start","target":"decide"},
                {"id":"e2","source":"decide","source_handle":"true","target":"end"},
                {"id":"e3","source":"decide","source_handle":"false","target":"end"}
            ]
        },
        "inputs":{}
    }))
    .unwrap();
    let events = Arc::new(Mutex::new(Vec::new()));
    let capture = events.clone();
    let result = chat
        .run_workflow(
            workflow,
            "audit-workflow-or-condition".into(),
            session,
            move |event| {
                if let RuntimeEvent::WorkflowNode { event } = event {
                    if event.status.is_terminal() {
                        capture.lock().unwrap().push(event);
                    }
                }
            },
            |_| {},
        )
        .await;
    let run = RunStore::new(&db)
        .get("audit-workflow-or-condition")
        .unwrap()
        .unwrap();
    let events = events.lock().unwrap();
    let decision = events
        .iter()
        .find(|event| event.node_id == "decide" && event.status == NodeExecutionStatus::Completed)
        .and_then(|event| event.outputs.as_ref())
        .and_then(|outputs| outputs.get("branch"));
    eprintln!(
        "api_ok={} decision={decision:?} run_terminal={:?}",
        result.is_ok(),
        run.terminal_kind
    );
    assert_eq!(
        decision,
        Some(&serde_json::Value::String("false".to_string())),
        "OR must use false as its identity and select true only when at least one operand is true"
    );
}

#[tokio::test]
async fn audit_workflow_multi_case_condition_keeps_else_handle() {
    let (_root, db, chat, session) = workflow_fixture().await;
    let workflow: WorkflowRequest = serde_json::from_value(serde_json::json!({
        "definition": {
            "version": 1,
            "nodes": [
                {"id":"start","kind":"start"},
                {"id":"decide","kind":"if-else","config":{"conditions":[
                    {"id":"red","logicalOperator":"and","conditions":[
                        {"left":"green","operator":"is","right":"red"}
                    ]},
                    {"id":"blue","logicalOperator":"and","conditions":[
                        {"left":"green","operator":"is","right":"blue"}
                    ]}
                ]}},
                {"id":"end","kind":"end"}
            ],
            "edges": [
                {"id":"e1","source":"start","target":"decide"},
                {"id":"e2","source":"decide","source_handle":"false","target":"end"}
            ]
        },
        "inputs":{}
    }))
    .unwrap();
    let result = chat
        .run_workflow(
            workflow,
            "audit-workflow-multi-case-else".into(),
            session,
            |_| {},
            |_| {},
        )
        .await;
    let terminal = RunStore::new(&db)
        .get("audit-workflow-multi-case-else")
        .unwrap()
        .and_then(|run| run.terminal_kind);
    eprintln!(
        "api_ok={} error={:?} run_terminal={terminal:?}",
        result.is_ok(),
        result.as_ref().err()
    );
    assert!(
        result.is_ok(),
        "a multi-case condition must allow the false branch that its executor returns when no case matches"
    );
}

#[tokio::test]
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
        assert!(events
            .iter()
            .any(|e| e.node_id == unselected && e.status == NodeExecutionStatus::Skipped));
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
