//! H-181: published iteration configuration through the real workflow Kernel.
//! Synthetic values and temporary SQLite only; no SDK, provider or host tools.
use super::*;
use deepagent_core::error::Result;
use deepagent_persistence::run_store::StoredRunEvent;
use deepagent_runtime::workflow::NodeExecutionEvent;
use serde_json::{json, Value};
use std::collections::BTreeSet;

fn request(items: Value, selector: &str) -> WorkflowRequest {
    serde_json::from_value(json!({
        "definition":{"version":1,"nodes":[
            {"id":"start","kind":"start","config":{
                "inputVariables":[{"name":"items","variable":"items","type":"array"}]
            }},
            {"id":"iterate","kind":"iteration","config":{
                "inputVariable":"{{#start.items#}}","outputVariable":selector,
                "parallel":false,"maxConcurrency":1,"errorHandling":"terminate","flatten":false
            }},
            {"id":"entry","kind":"iteration-start"},
            {"id":"body","kind":"template-transform","config":{
                "templateScript":"seen:{{#entry.item#}}","templateInputVariables":[]
            }},
            {"id":"end","kind":"end"}
        ],"edges":[
            {"id":"a","source":"start","target":"iterate"},
            {"id":"b","source":"iterate","source_handle":"item","target":"entry"},
            {"id":"c","source":"entry","target":"body"},
            {"id":"d","source":"iterate","source_handle":"done","target":"end"}
        ]},"inputs":{"items":items}
    }))
    .unwrap()
}

async fn run(
    chat: &ChatService,
    session: String,
    workflow: WorkflowRequest,
    id: &str,
) -> (Result<String>, Vec<NodeExecutionEvent>) {
    let events = Arc::new(Mutex::new(Vec::new()));
    let capture = events.clone();
    let result = chat
        .run_workflow(
            workflow,
            id.into(),
            session,
            move |event| {
                if let RuntimeEvent::WorkflowNode { event } = event {
                    capture.lock().unwrap().push(event);
                }
            },
            |_| panic!("a local template iteration must not request tool approval"),
        )
        .await;
    let events = events.lock().unwrap().clone();
    (result, events)
}

fn aggregate(events: &[NodeExecutionEvent]) -> &Value {
    let terminal: Vec<_> = events
        .iter()
        .filter(|event| {
            event.node_id == "iterate" && event.scope.is_empty() && event.status.is_terminal()
        })
        .collect();
    assert_eq!(
        terminal.len(),
        1,
        "one aggregate terminal, not one per item"
    );
    assert_eq!(terminal[0].status, NodeExecutionStatus::Completed);
    terminal[0].outputs.as_ref().expect("real aggregate output")
}

fn body_completions(events: &[NodeExecutionEvent]) -> Vec<&NodeExecutionEvent> {
    events
        .iter()
        .filter(|event| event.node_id == "body" && event.status == NodeExecutionStatus::Completed)
        .collect()
}

fn assert_terminal(db: &Database, id: &str, kind: &str) -> Vec<StoredRunEvent> {
    let store = RunStore::new(db);
    assert_eq!(
        store.get(id).unwrap().unwrap().terminal_kind.as_deref(),
        Some(kind)
    );
    let ledger = store.events_after(id, None).unwrap();
    assert_eq!(
        ledger
            .iter()
            .filter(|event| event.event_type == "run_terminal")
            .count(),
        1
    );
    ledger
}

#[tokio::test]
async fn h181_control_ordinary_template_workflow_keeps_its_existing_success() {
    let (_root, db, chat, session) = workflow_fixture().await;
    let mut workflow = request(json!(["ONE"]), "item");
    workflow
        .definition
        .nodes
        .retain(|node| !matches!(node.id.as_str(), "iterate" | "entry"));
    workflow.definition.nodes[1]
        .config
        .insert("templateScript".into(), json!("normal-control"));
    workflow.definition.edges = serde_json::from_value(json!([
        {"id":"a","source":"start","target":"body"},
        {"id":"b","source":"body","target":"end"}
    ]))
    .unwrap();
    let (result, events) = run(&chat, session, workflow, "h181-control").await;
    assert!(
        result.is_ok(),
        "non-iteration control must remain usable: {result:?}"
    );
    assert_eq!(body_completions(&events).len(), 1);
    assert_eq!(
        body_completions(&events)[0].outputs,
        Some(json!({"output":"normal-control"}))
    );
    assert!(events.iter().all(|event| event.scope.is_empty()));
    assert_terminal(&db, "h181-control", "succeeded");
}

#[tokio::test]
async fn h181_serial_body_uses_each_item_and_done_runs_once_after_aggregation() {
    let (_root, db, chat, session) = workflow_fixture().await;
    let (result, events) = run(
        &chat,
        session,
        request(json!(["ONE", "TWO"]), "{{#body.output#}}"),
        "h181-serial",
    )
    .await;
    assert!(
        result.is_ok(),
        "body selector must be admitted in its own scope: {result:?}"
    );
    assert_eq!(
        aggregate(&events),
        &json!({"output":["seen:ONE","seen:TWO"]})
    );
    let completed = body_completions(&events);
    assert_eq!(completed.len(), 2, "the actual body must execute twice");
    for (index, event) in completed.iter().enumerate() {
        assert_eq!(event.scope, vec!["iterate".to_owned(), index.to_string()]);
        assert_eq!(event.attempt, 1);
    }
    let aggregate_index = events
        .iter()
        .position(|event| {
            event.node_id == "iterate" && event.status == NodeExecutionStatus::Completed
        })
        .unwrap();
    let done: Vec<_> = events
        .iter()
        .enumerate()
        .filter(|(_, event)| {
            event.node_id == "end"
                && event.status == NodeExecutionStatus::Completed
                && event.scope.is_empty()
        })
        .collect();
    assert_eq!(done.len(), 1);
    assert!(done[0].0 > aggregate_index);
    assert_terminal(&db, "h181-serial", "succeeded");
}

#[tokio::test]
async fn h181_empty_input_does_not_execute_a_body_or_publish_fake_item_events() {
    let (_root, db, chat, session) = workflow_fixture().await;
    let (result, events) = run(
        &chat,
        session,
        request(json!([]), "{{#body.output#}}"),
        "h181-empty",
    )
    .await;
    assert!(
        result.is_ok(),
        "empty collection is a valid zero-iteration run: {result:?}"
    );
    assert_eq!(aggregate(&events), &json!({"output":[]}));
    assert!(!events
        .iter()
        .any(|event| matches!(event.node_id.as_str(), "entry" | "body")));
    assert_eq!(events.iter().filter(|event| event.node_id == "end"
        && event.status == NodeExecutionStatus::Completed).count(), 1);
    assert_terminal(&db, "h181-empty", "succeeded");
}

#[tokio::test]
async fn h181_identity_preserves_object_and_opaque_artifact_items_without_coercion() {
    let (_root, db, chat, session) = workflow_fixture().await;
    let input = json!([{"name":"first","count":2}, "artifact://h181-synthetic", null, false, 7]);
    let (result, events) = run(
        &chat,
        session,
        request(input.clone(), "item"),
        "h181-identity",
    )
    .await;
    assert!(
        result.is_ok(),
        "identity selector still executes its real body: {result:?}"
    );
    assert_eq!(aggregate(&events), &json!({"output":input}));
    assert_eq!(body_completions(&events).len(), 5);
    assert_terminal(&db, "h181-identity", "succeeded");
}

#[tokio::test]
async fn h181_flatten_is_one_level_and_never_stringifies_or_loses_an_item() {
    let (_root, db, chat, session) = workflow_fixture().await;
    let mut workflow = request(json!([[1, 2], [], [3], {"nested":[4]}]), "item");
    workflow.definition.nodes[1]
        .config
        .insert("flatten".into(), json!(true));
    let (result, events) = run(&chat, session, workflow, "h181-flatten").await;
    assert!(
        result.is_ok(),
        "flatten must apply to typed results: {result:?}"
    );
    assert_eq!(
        aggregate(&events),
        &json!({"output":[1,2,3,{"nested":[4]}]})
    );
    assert_eq!(body_completions(&events).len(), 4);
    assert_terminal(&db, "h181-flatten", "succeeded");
}

#[tokio::test]
async fn h181_partial_failure_strategies_preserve_order_and_actual_scoped_failures() {
    for (policy, expected) in [
        ("continue", json!(["first", null, "third"])),
        ("remove", json!(["first", "third"])),
    ] {
        let (_root, db, chat, session) = workflow_fixture().await;
        let mut workflow = request(
            json!([{"value":"first"},{},{"value":"third"}]),
            "{{#body.output#}}",
        );
        workflow.definition.nodes[1]
            .config
            .insert("errorHandling".into(), json!(policy));
        workflow.definition.nodes[3]
            .config
            .insert("templateScript".into(), json!("{{#entry.item.value#}}"));
        let id = format!("h181-partial-{policy}");
        let (result, events) = run(&chat, session, workflow, &id).await;
        assert!(
            result.is_ok(),
            "the selected policy handles only the failed item: {result:?}"
        );
        assert_eq!(aggregate(&events), &json!({"output":expected}));
        assert_eq!(body_completions(&events).len(), 2);
        let failed: Vec<_> = events
            .iter()
            .filter(|event| event.node_id == "body" && event.status == NodeExecutionStatus::Failed)
            .collect();
        assert_eq!(failed.len(), 1);
        assert_eq!(failed[0].scope, vec!["iterate", "1"]);
        assert_terminal(&db, &id, "succeeded");
    }
}

#[tokio::test]
async fn h181_terminate_stops_before_admitting_later_serial_items_and_never_emits_done() {
    let (_root, db, chat, session) = workflow_fixture().await;
    let mut workflow = request(
        json!([{"value":"first"},{},{"value":"must-not-start"}]),
        "{{#body.output#}}",
    );
    workflow.definition.nodes[3]
        .config
        .insert("templateScript".into(), json!("{{#entry.item.value#}}"));
    let (result, events) = run(&chat, session, workflow, "h181-terminate").await;
    assert!(result.is_err());
    assert_eq!(
        body_completions(&events).len(),
        1,
        "first item really ran before the failure"
    );
    assert!(!events
        .iter()
        .any(|event| event.scope == vec!["iterate", "2"]));
    assert!(!events
        .iter()
        .any(|event| event.node_id == "end" && event.status == NodeExecutionStatus::Completed));
    let aggregate: Vec<_> = events
        .iter()
        .filter(|event| {
            event.node_id == "iterate" && event.scope.is_empty() && event.status.is_terminal()
        })
        .collect();
    assert_eq!(aggregate.len(), 1);
    assert_eq!(aggregate[0].status, NodeExecutionStatus::Failed);
    assert!(aggregate[0].outputs.is_none());
    assert_terminal(&db, "h181-terminate", "failed");
}

#[tokio::test]
async fn h181_scoped_results_and_aggregate_survive_real_sqlite_reopen_and_cursor_replay() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("iteration.db");
    let db = Arc::new(Database::open(&path).unwrap());
    let (_root, db, chat, session) = workflow_fixture_with_database(root, db).await;
    let (result, events) = run(
        &chat,
        session,
        request(json!(["ONE", "TWO"]), "{{#body.output#}}"),
        "h181-durable",
    )
    .await;
    assert!(
        result.is_ok(),
        "real scoped workflow must finish before persistence acceptance: {result:?}"
    );
    assert_eq!(
        aggregate(&events),
        &json!({"output":["seen:ONE","seen:TWO"]})
    );
    let before = assert_terminal(&db, "h181-durable", "succeeded");
    assert!(before
        .windows(2)
        .all(|events| events[0].sequence < events[1].sequence));
    let cursor = before[before.len() / 2].sequence;
    drop(chat);
    drop(db);
    let reopened = Database::open(&path).unwrap();
    let after = assert_terminal(&reopened, "h181-durable", "succeeded");
    assert_eq!(after, before);
    assert_eq!(
        RunStore::new(&reopened)
            .events_after("h181-durable", Some(cursor))
            .unwrap(),
        before
            .iter()
            .filter(|event| event.sequence > cursor)
            .cloned()
            .collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn h181_parallel_code_items_each_require_kernel_approval_and_denial_never_dispatches() {
    let (_root, db, chat, session) = workflow_fixture().await;
    let mut workflow = request(json!(["one", "two", "three"]), "{{#body.result#}}");
    workflow.definition.nodes[1].config.extend(
        json!({
            "parallel":true,"maxConcurrency":3,"errorHandling":"continue"
        })
        .as_object()
        .unwrap()
        .clone(),
    );
    workflow.definition.nodes[3] = serde_json::from_value(json!({
        "id":"body","kind":"code","config":{
            "codeLanguage":"javascript", "codeScript":"function main(x){return {result:x.input};}",
            "codeInputVariables":[{"name":"input","value":"{{#entry.item#}}"}],
            "codeOutputVariables":[{"name":"result","type":"string"}]
        }
    }))
    .unwrap();
    let responder = chat.clone();
    let approvals = Arc::new(AtomicUsize::new(0));
    let count = approvals.clone();
    let events = Arc::new(Mutex::new(Vec::new()));
    let capture = events.clone();
    chat.run_workflow(
        workflow,
        "h181-denied-parallel".into(),
        session,
        move |event| {
            if let RuntimeEvent::WorkflowNode { event } = event {
                capture.lock().unwrap().push(event);
            }
        },
        move |approval| {
            assert_eq!(approval.tool, "canvas_code_execute");
            count.fetch_add(1, Ordering::SeqCst);
            assert!(responder
                .resolve_approval(&approval.call_id, false, "synthetic iteration denial")
                .unwrap());
        },
    )
    .await
    .unwrap();
    assert_eq!(approvals.load(Ordering::SeqCst), 3);
    let events = events.lock().unwrap();
    assert_eq!(aggregate(&events), &json!({"output":[null,null,null]}));
    assert_eq!(
        events
            .iter()
            .filter(|event| event.node_id == "body" && event.status == NodeExecutionStatus::Failed)
            .count(),
        3
    );
    let ledger = assert_terminal(&db, "h181-denied-parallel", "succeeded");
    assert_eq!(
        ledger
            .iter()
            .filter(|event| event.event_type == "approval.requested")
            .count(),
        3
    );
    assert!(!ledger
        .iter()
        .any(|event| event.event_type == "tool_started"));
}

#[derive(Default)]
struct ItemHttp {
    calls: Mutex<Vec<String>>,
    live: AtomicUsize,
    maximum: Mutex<usize>,
    wait_for_cancel: bool,
}

struct ActiveHttp<'a>(&'a AtomicUsize);
impl Drop for ActiveHttp<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

#[async_trait::async_trait]
impl deepagent_builtins::http_request::HttpRequestClient for ItemHttp {
    fn validate(&self, request: &deepagent_builtins::http_request::HttpRequest) -> Result<()> {
        request.validate()
    }
    async fn request(
        &self,
        request: deepagent_builtins::http_request::HttpRequest,
        context: deepagent_tools::ToolExecutionContext,
    ) -> Result<Value> {
        let active = self.live.fetch_add(1, Ordering::SeqCst) + 1;
        let _active = ActiveHttp(&self.live);
        {
            let mut max = self.maximum.lock().unwrap();
            *max = (*max).max(active);
        }
        self.calls.lock().unwrap().push(request.body.clone());
        if self.wait_for_cancel {
            context.cancelled().await;
            Err(deepagent_core::error::CoreError::other(
                "synthetic cancellation",
            ))
        } else {
            tokio::time::sleep(Duration::from_millis(5)).await;
            Ok(json!({"body":request.body,"status_code":201,"headers":{}}))
        }
    }
}

fn http_workflow() -> WorkflowRequest {
    let mut workflow = request(json!(["one", "two", "three"]), "{{#body.body#}}");
    workflow.definition.nodes[1].config.extend(
        json!({"parallel":true,"maxConcurrency":3})
            .as_object()
            .unwrap()
            .clone(),
    );
    workflow.definition.nodes[3] = serde_json::from_value(json!({
        "id":"body","kind":"http-request","config":{
            "httpUrl":"https://never-connected.fixture.invalid/", "httpMethod":"POST",
            "httpBody":"{{#entry.item#}}", "httpTimeout":7,"httpRetryCount":0
        }
    }))
    .unwrap();
    workflow
}

#[tokio::test]
async fn h181_parallel_post_items_preserve_kernel_exclusive_barrier_and_durable_call_identity() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("iteration-http.db");
    let db = Arc::new(Database::open(&path).unwrap());
    let (_root, db, chat, session) = workflow_fixture_with_database(root, db).await;
    let client = Arc::new(ItemHttp::default());
    let chat = chat.with_workflow_http_client(client.clone());
    let responder = chat.clone();
    let approvals = Arc::new(AtomicUsize::new(0));
    let count = approvals.clone();
    let nodes = Arc::new(Mutex::new(Vec::new()));
    let capture = nodes.clone();
    chat.run_workflow(
        http_workflow(),
        "h181-post".into(),
        session,
        move |event| {
            if let RuntimeEvent::WorkflowNode { event } = event {
                capture.lock().unwrap().push(event);
            }
        },
        move |approval| {
            assert_eq!(approval.tool, "http_request");
            count.fetch_add(1, Ordering::SeqCst);
            assert!(responder
                .resolve_approval(&approval.call_id, true, "synthetic item approval")
                .unwrap());
        },
    )
    .await
    .unwrap();
    assert_eq!(approvals.load(Ordering::SeqCst), 3);
    assert_eq!(
        *client.maximum.lock().unwrap(),
        1,
        "parallel=true must not weaken Kernel's write barrier"
    );
    assert_eq!(*client.calls.lock().unwrap(), vec!["one", "two", "three"]);
    assert_eq!(
        aggregate(&nodes.lock().unwrap()),
        &json!({"output":["one","two","three"]})
    );
    let before = assert_terminal(&db, "h181-post", "succeeded");
    let calls: BTreeSet<_> = before
        .iter()
        .filter(|event| event.event_type == "tool_started")
        .map(|event| event.data["call_id"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(
        calls.len(),
        3,
        "physical calls keep distinct Kernel identities"
    );
    drop(chat);
    drop(db);
    let reopened = Database::open(&path).unwrap();
    assert_eq!(assert_terminal(&reopened, "h181-post", "succeeded"), before);
    assert_eq!(
        client.calls.lock().unwrap().len(),
        3,
        "replay never dispatches again"
    );
}

#[tokio::test]
async fn h181_cancellation_during_first_post_cleans_scopes_and_never_dispatches_queued_items() {
    let (_root, db, chat, session) = workflow_fixture().await;
    let client = Arc::new(ItemHttp {
        wait_for_cancel: true,
        ..Default::default()
    });
    let chat = chat.with_workflow_http_client(client.clone());
    let responder = chat.clone();
    let worker_chat = chat.clone();
    let nodes = Arc::new(Mutex::new(Vec::new()));
    let capture = nodes.clone();
    let worker = tokio::spawn(async move {
        worker_chat
            .run_workflow(
                http_workflow(),
                "h181-post-cancel".into(),
                session,
                move |event| {
                    if let RuntimeEvent::WorkflowNode { event } = event {
                        capture.lock().unwrap().push(event);
                    }
                },
                move |approval| {
                    responder
                        .resolve_approval(&approval.call_id, true, "synthetic cancellation fixture")
                        .unwrap();
                },
            )
            .await
    });
    tokio::time::timeout(Duration::from_secs(3), async {
        while client.live.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(chat.cancel_session("h181-post-cancel"));
    tokio::time::timeout(Duration::from_secs(3), worker)
        .await
        .unwrap()
        .unwrap()
        .ok();
    assert_eq!(client.live.load(Ordering::SeqCst), 0);
    assert_eq!(*client.calls.lock().unwrap(), vec!["one"]);
    let ledger = assert_terminal(&db, "h181-post-cancel", "cancelled");
    assert_eq!(
        ledger
            .iter()
            .filter(|event| event.event_type == "tool_started")
            .count(),
        1
    );
    let nodes = nodes.lock().unwrap();
    assert_eq!(
        nodes
            .iter()
            .filter(
                |event| event.node_id == "body" && event.status == NodeExecutionStatus::Cancelled
            )
            .count(),
        3
    );
    assert_eq!(
        nodes
            .iter()
            .filter(|event| event.node_id == "iterate"
                && event.status == NodeExecutionStatus::Cancelled)
            .count(),
        1
    );
    assert!(!nodes
        .iter()
        .any(|event| event.node_id == "end" && event.status == NodeExecutionStatus::Completed));
}

#[tokio::test]
async fn h181_abandoned_inflight_iteration_recovers_as_failed_without_retrying_unknown_post() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("abandoned-iteration.db");
    let db = Arc::new(Database::open(&path).unwrap());
    let (_root, db, chat, session) = workflow_fixture_with_database(root, db).await;
    let client = Arc::new(ItemHttp {
        wait_for_cancel: true,
        ..Default::default()
    });
    let chat = chat.with_workflow_http_client(client.clone());
    let worker_chat = chat.clone();
    let responder = chat.clone();
    let worker = tokio::spawn(async move {
        worker_chat
            .run_workflow(
                http_workflow(),
                "h181-abandoned".into(),
                session,
                |_| {},
                move |approval| {
                    responder
                        .resolve_approval(&approval.call_id, true, "synthetic abandoned fixture")
                        .unwrap();
                },
            )
            .await
    });
    tokio::time::timeout(Duration::from_secs(3), async {
        while client.live.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let before = RunStore::new(&db)
        .events_after("h181-abandoned", None)
        .unwrap();
    let scopes: BTreeSet<_> = before
        .iter()
        .filter_map(|record| {
            match serde_json::from_value::<RuntimeEvent>(record.data.clone()).ok()? {
                RuntimeEvent::WorkflowNode { event }
                    if event.node_id == "body" && event.status == NodeExecutionStatus::Running =>
                {
                    Some(event.scope)
                }
                _ => None,
            }
        })
        .collect();
    assert_eq!(
        scopes,
        BTreeSet::from([
            vec!["iterate".to_owned(), "0".to_owned()],
            vec!["iterate".to_owned(), "1".to_owned()],
            vec!["iterate".to_owned(), "2".to_owned()],
        ])
    );
    assert_eq!(
        before
            .iter()
            .filter(|event| event.event_type == "tool_started")
            .count(),
        1
    );
    assert!(!before
        .iter()
        .any(|event| event.event_type == "run_terminal"));
    // Drop the actual owning future rather than request normal cancellation.
    // This models lost execution ownership, not OS power-loss durability.
    worker.abort();
    assert!(worker.await.unwrap_err().is_cancelled());
    assert_eq!(client.live.load(Ordering::SeqCst), 0);
    drop(chat);
    drop(db);
    let app = deepagent_app_core::AppService::open(&path).unwrap();
    let recovered = app.recover_unfinished_runs().unwrap();
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].run_id, "h181-abandoned");
    let after = assert_terminal(app.database(), "h181-abandoned", "failed");
    let actions = deepagent_persistence::run_control::RunControlStore::new(app.database())
        .list_actions("h181-abandoned")
        .unwrap();
    let started: Vec<_> = actions
        .iter()
        .filter(|action| action.started_at.is_some())
        .collect();
    assert_eq!(started.len(), 1);
    assert_eq!(
        started[0].state,
        deepagent_persistence::run_control::RunActionState::Failed
    );
    assert!(
        after.starts_with(&before),
        "startup must preserve partial scoped evidence"
    );
    assert!(after
        .windows(2)
        .all(|events| events[0].sequence < events[1].sequence));
    assert_eq!(
        after
            .iter()
            .filter(|event| event.event_type == "tool_started")
            .count(),
        1
    );
    assert!(app.recover_unfinished_runs().unwrap().is_empty());
    assert_eq!(
        RunStore::new(app.database())
            .events_after("h181-abandoned", None)
            .unwrap(),
        after
    );
    assert_eq!(
        *client.calls.lock().unwrap(),
        vec!["one"],
        "recovery must not dispatch unknown or queued side effects"
    );
}

#[tokio::test]
#[ignore = "requires local Node and desktop TypeScript dependencies; actual published TS serializer to Kernel"]
async fn h181_desktop_serialized_iteration_runs_its_body_through_the_real_kernel() {
    let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../apps/desktop/scripts/audit-workflow-contract.cjs");
    let output = std::process::Command::new("node")
        .arg(script)
        .arg("--emit-iteration-fixture")
        .output()
        .expect("explicit local Node seam gate");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let fixtures: Vec<Value> = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(fixtures.len(), 1);
    let workflow: WorkflowRequest =
        serde_json::from_value(fixtures[0]["workflow"].clone()).unwrap();
    let (_root, db, chat, session) = workflow_fixture().await;
    let (result, events) = run(&chat, session, workflow, "h181-desktop-seam").await;
    assert!(
        result.is_ok(),
        "actual schema/defaults/serializer must execute: {result:?}"
    );
    assert_eq!(
        aggregate(&events),
        &json!({"output":["seen:ONE","seen:TWO"]})
    );
    assert_eq!(body_completions(&events).len(), 2);
    assert_terminal(&db, "h181-desktop-seam", "succeeded");
}
