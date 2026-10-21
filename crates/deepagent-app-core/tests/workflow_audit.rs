use std::sync::{Arc, Mutex};

use deepagent_app_core::{ChatService, MemorySecretStore, SettingsService};
use deepagent_models::transport::MockTransport;
use deepagent_persistence::{run_store::RunStore, Database};
use deepagent_runtime::workflow::{NodeExecutionStatus, WorkflowRequest};
use deepagent_runtime::RuntimeEvent;

#[tokio::test]
#[ignore = "audit: a failed workflow tool node must not publish completed and a successful run"]
async fn audit_workflow_unknown_tool_propagates_node_and_run_failure() {
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
