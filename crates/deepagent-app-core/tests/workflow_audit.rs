use std::sync::{Arc, Mutex};

use deepagent_app_core::{ChatService, MemorySecretStore, SettingsService};
use deepagent_models::transport::MockTransport;
use deepagent_persistence::{run_store::RunStore, Database};
use deepagent_runtime::workflow::{NodeExecutionStatus, WorkflowRequest};
use deepagent_runtime::RuntimeEvent;

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
