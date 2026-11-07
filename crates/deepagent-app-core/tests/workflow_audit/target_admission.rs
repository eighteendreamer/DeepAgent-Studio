//! H177 historical-fix verification; production admission belongs to H196.
use super::*;
use serde_json::json;

fn request(target: Option<&str>) -> WorkflowRequest {
    serde_json::from_value(json!({"definition":{"version":1,"nodes":[
        {"id":"start","kind":"start"},
        {"id":"answer","kind":"answer","config":{"answerTemplate":"CURRENT"}},
        {"id":"end","kind":"end"}
    ],"edges":[{"id":"a","source":"start","target":"answer"},{"id":"b","source":"answer","target":"end"}]},"target_node_id":target})).unwrap()
}

async fn execute(
    chat: &ChatService,
    session: &str,
    request: WorkflowRequest,
    id: &str,
) -> (
    deepagent_core::Result<String>,
    Vec<deepagent_runtime::workflow::NodeExecutionEvent>,
) {
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
            |_| panic!("target admission fixture must never request approval"),
        )
        .await;
    let live = nodes.lock().unwrap().clone();
    (result, live)
}

fn durable(
    db: &Database,
    id: &str,
    expected: &str,
) -> Vec<deepagent_persistence::run_store::StoredRunEvent> {
    let store = RunStore::new(db);
    assert_eq!(
        store.get(id).unwrap().unwrap().terminal_kind.as_deref(),
        Some(expected)
    );
    let events = store.events_after(id, None).unwrap();
    assert!(events
        .windows(2)
        .all(|pair| pair[0].sequence < pair[1].sequence));
    let terminals: Vec<_> = events
        .iter()
        .filter(|event| event.event_type == "run_terminal")
        .collect();
    assert_eq!(terminals.len(), 1);
    assert_eq!(terminals[0].data["kind"], expected);
    events
}

#[tokio::test]
async fn h177_missing_deleted_and_empty_targets_fail_durably_before_any_node_or_tool() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("admission.sqlite");
    let db = Arc::new(Database::open(&path).unwrap());
    let (root, db, chat, session) = workflow_fixture_with_database(root, db).await;
    let mut saved = Vec::new();
    for (index, target) in ["missing-node", "removed-node", "", " end "]
        .into_iter()
        .enumerate()
    {
        let mut workflow = request(Some(target));
        // A deleted identity is carried by a stale request while the supplied
        // current graph remains valid. v1 is recomputation, not snapshot resume.
        if target == "removed-node" {
            workflow.definition.nodes[1].id = target.into();
            workflow.definition.edges[0].target = target.into();
            workflow.definition.edges[1].source = target.into();
            workflow.definition.nodes.remove(1);
            workflow.definition.edges =
                vec![
                    serde_json::from_value(json!({"id":"a","source":"start","target":"end"}))
                        .unwrap(),
                ];
        }
        workflow.definition.nodes.push(serde_json::from_value(json!({"id":"write","kind":"tool","config":{"toolName":"write_file","toolArguments":{"path":"must-not-exist.txt","content":"synthetic"}}})).unwrap());
        workflow.definition.edges.push(
            serde_json::from_value(json!({"id":"w","source":"start","target":"write"})).unwrap(),
        );
        let id = format!("h177-reject-{index}");
        let (result, nodes) = execute(&chat, &session, workflow, &id).await;
        let error = result.unwrap_err().to_string();
        assert!(
            error.contains("InvalidWorkflowTarget: target node does not exist"),
            "{error}"
        );
        assert!(nodes.is_empty());
        assert!(!root.path().join("must-not-exist.txt").exists());
        let events = durable(&db, &id, "failed");
        assert!(
            events
                .iter()
                .all(|event| event.event_type != "workflow_node"
                    && event.event_type != "tool_started")
        );
        assert!(events
            .iter()
            .any(|event| event.data.to_string().contains("InvalidWorkflowTarget")));
        saved.push((id, events));
    }
    // Rejection releases the original admission/session lease: a later valid
    // request on that same session must still execute normally.
    let (result, nodes) = execute(&chat, &session, request(None), "h177-after-reject").await;
    result.unwrap();
    assert!(nodes
        .iter()
        .any(|event| event.node_id == "end" && event.status == NodeExecutionStatus::Completed));
    let succeeded = durable(&db, "h177-after-reject", "succeeded");
    saved.push(("h177-after-reject".into(), succeeded));
    drop(chat);
    drop(db);
    let reopened = Database::open(&path).unwrap();
    for (id, events) in saved {
        let replay = RunStore::new(&reopened).events_after(&id, None).unwrap();
        assert_eq!(replay, events);
        let cursor = events[0].sequence;
        assert_eq!(
            RunStore::new(&reopened)
                .events_after(&id, Some(cursor))
                .unwrap(),
            events
                .into_iter()
                .filter(|event| event.sequence > cursor)
                .collect::<Vec<_>>()
        );
    }
    assert!(!root.path().join("must-not-exist.txt").exists());
}

#[tokio::test]
async fn h177_valid_none_start_answer_end_targets_keep_the_existing_v1_contract() {
    let (_root, db, chat, session) = workflow_fixture().await;
    for (index, target) in [None, Some("start"), Some("answer"), Some("end")]
        .into_iter()
        .enumerate()
    {
        let id = format!("h177-valid-{index}");
        let (result, nodes) = execute(&chat, &session, request(target), &id).await;
        result.unwrap();
        durable(&db, &id, "succeeded");
        let completed: Vec<_> = nodes
            .iter()
            .filter(|event| event.status == NodeExecutionStatus::Completed)
            .map(|event| event.node_id.as_str())
            .collect();
        assert_eq!(
            completed,
            match target {
                Some("start") => vec!["start"],
                Some("answer") => vec!["start", "answer"],
                _ => vec!["start", "answer", "end"],
            }
        );
    }
}
