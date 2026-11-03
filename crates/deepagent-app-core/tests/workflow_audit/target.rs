//! H-196: targeted v1 execution recomputes its ancestor closure, never stale outputs.
use super::*;
use deepagent_core::error::Result;
use deepagent_runtime::workflow::NodeExecutionEvent;
use serde_json::{json, Value};
use std::collections::BTreeMap;

async fn run(
    chat: &ChatService,
    session: &str,
    request: WorkflowRequest,
    id: &str,
) -> (Result<String>, Vec<NodeExecutionEvent>) {
    let events = Arc::new(Mutex::new(Vec::new()));
    let capture = events.clone();
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
            |_| panic!("pure target fixture must not request approval"),
        )
        .await;
    let nodes = events.lock().unwrap().clone();
    (result, nodes)
}

fn workflow(value: &str, target: Option<&str>) -> WorkflowRequest {
    serde_json::from_value(json!({"definition":{"version":1,"nodes":[
        {"id":"end","kind":"end","config":{"outputVariables":[{"name":"report","type":"string","value":"{{#target.answer#}}"}]}},
        {"id":"target","kind":"answer","config":{"answerTemplate":"{{#middle.output#}}"}},
        {"id":"source","kind":"variable-assigner","config":{"assignments":[{"variable":"output","value":value}]}},
        {"id":"middle","kind":"variable-assigner","config":{"assignments":[{"variable":"output","inputType":"variable","value":"{{#source.output#}}"}]}},
        {"id":"start","kind":"start"},
        {"id":"independent","kind":"variable-assigner","config":{"assignments":[]}}
    ],"edges":[
        {"id":"a","source":"start","target":"source"},{"id":"b","source":"source","target":"middle"},
        {"id":"c","source":"middle","target":"target"},{"id":"d","source":"target","target":"end"},
        {"id":"e","source":"start","target":"independent"}
    ]},"target_node_id":target})).unwrap()
}

fn assert_statuses(nodes: &[NodeExecutionEvent], expected: &[(&str, NodeExecutionStatus)]) {
    let mut actual = BTreeMap::new();
    for node in nodes
        .iter()
        .filter(|n| n.scope.is_empty() && n.status.is_terminal())
    {
        assert!(
            actual.insert(node.node_id.as_str(), node.status).is_none(),
            "duplicate node terminal"
        );
        if node.status == NodeExecutionStatus::Skipped {
            assert!(node.error.is_some());
        }
    }
    assert_eq!(actual, expected.iter().copied().collect());
}

#[tokio::test]
async fn h196_recompute_runs_all_ancestors_but_not_descendants_or_unrelated_nodes() {
    let (_root, db, chat, session) = workflow_fixture().await;
    let (result, nodes) = run(
        &chat,
        &session,
        workflow("FRESH", Some("target")),
        "h196-closure",
    )
    .await;
    assert!(result.is_ok(), "{result:?}");
    assert_statuses(
        &nodes,
        &[
            ("start", NodeExecutionStatus::Completed),
            ("source", NodeExecutionStatus::Completed),
            ("middle", NodeExecutionStatus::Completed),
            ("target", NodeExecutionStatus::Completed),
            ("end", NodeExecutionStatus::Skipped),
            ("independent", NodeExecutionStatus::Skipped),
        ],
    );
    assert_eq!(
        nodes
            .iter()
            .find(|n| n.node_id == "target" && n.status == NodeExecutionStatus::Completed)
            .unwrap()
            .outputs
            .as_ref()
            .unwrap()["answer"],
        "FRESH"
    );
    assert_eq!(
        RunStore::new(&db)
            .get("h196-closure")
            .unwrap()
            .unwrap()
            .terminal_kind
            .as_deref(),
        Some("succeeded")
    );
}

fn output<'a>(nodes: &'a [NodeExecutionEvent], id: &str) -> &'a Value {
    nodes
        .iter()
        .find(|n| n.node_id == id && n.status == NodeExecutionStatus::Completed)
        .unwrap()
        .outputs
        .as_ref()
        .unwrap()
}

fn terminal(db: &Database, id: &str, kind: &str) -> Value {
    let store = RunStore::new(db);
    assert_eq!(
        store.get(id).unwrap().unwrap().terminal_kind.as_deref(),
        Some(kind)
    );
    let ledger = store.events_after(id, None).unwrap();
    let terminals: Vec<_> = ledger
        .iter()
        .filter(|e| e.event_type == "run_terminal")
        .collect();
    assert_eq!(terminals.len(), 1);
    let mut seen = std::collections::BTreeSet::new();
    for e in ledger.iter().filter(|e| e.event_type == "workflow_node") {
        let n: NodeExecutionEvent = serde_json::from_value(e.data["event"].clone()).unwrap();
        if n.status.is_terminal() {
            assert!(seen.insert((n.scope, n.node_id)), "duplicate node terminal");
            assert!(e.sequence < terminals[0].sequence);
        }
    }
    terminals[0].data.clone()
}

#[tokio::test]
async fn h196_current_revision_inputs_defaults_and_opaque_references_are_recomputed() {
    let (_root, db, chat, session) = workflow_fixture().await;
    let mut revisions = std::collections::BTreeSet::new();
    for (i, value) in ["OLD", "NEW", "artifact://h196/synthetic-current"]
        .iter()
        .enumerate()
    {
        let mut w = workflow("{{#start.value#}}", Some("target"));
        w.definition
            .nodes
            .iter_mut()
            .find(|n| n.id == "start")
            .unwrap()
            .config = serde_json::from_value(
            json!({"inputVariables":[{"name":"value","type":"string","default":value}]}),
        )
        .unwrap();
        // Supplied input must supersede the current default, never old output.
        if i == 1 {
            w.inputs.insert("value".into(), json!("EXPLICIT_NEW"));
        }
        let id = format!("h196-fresh-{i}");
        let (result, nodes) = run(&chat, &session, w, &id).await;
        assert!(result.is_ok(), "{result:?}");
        assert_eq!(
            output(&nodes, "target")["answer"],
            if i == 1 { "EXPLICIT_NEW" } else { value }
        );
        revisions.insert(nodes[0].revision.clone());
        terminal(&db, &id, "succeeded");
    }
    assert_eq!(revisions.len(), 3);
}

fn branch(selected: bool) -> WorkflowRequest {
    serde_json::from_value(json!({"definition":{"version":1,"nodes":[
        {"id":"start","kind":"start"},
        {"id":"choose","kind":"if-else","config":{"conditions":[{"id":"true","logic":"and","items":[{"variable":"yes","operator":"is","value":"yes"}]}]}},
        {"id":"left","kind":"variable-assigner","config":{"assignments":[{"variable":"output","value":"LEFT"}]}},
        {"id":"right","kind":"variable-assigner","config":{"assignments":[{"variable":"output","value":"RIGHT"}]}},
        {"id":"aggregate","kind":"variable-aggregator","config":{"aggregatorOutputType":"string","aggregatorVariables":["{{#left.output#}}","{{#right.output#}}"]}},
        {"id":"end","kind":"end"}
    ],"edges":[
        {"id":"a","source":"start","target":"choose"},
        {"id":"b","source":"choose","target":"left","source_handle":if selected {"true"} else {"false"}},
        {"id":"c","source":"choose","target":"right","source_handle":if selected {"false"} else {"true"}},
        {"id":"d","source":"left","target":"aggregate"},
        {"id":"e","source":"right","target":"aggregate"},
        {"id":"f","source":"aggregate","target":"end"}
    ]},"target_node_id":"aggregate"})).unwrap()
}

#[tokio::test]
async fn h196_branch_ancestors_respect_activation_and_target_must_actually_complete() {
    let (_root, db, chat, session) = workflow_fixture().await;
    for selected in [false, true] {
        // All six rotations, both node directions and alternating edge order.
        for order in 0..12 {
            let mut w = branch(selected);
            w.definition.nodes.rotate_left(order % 6);
            if order >= 6 {
                w.definition.nodes.reverse();
            }
            if order % 2 == 0 {
                w.definition.edges.reverse();
            }
            let id = format!("h196-branch-{selected}-{order}");
            let (result, nodes) = run(&chat, &session, w, &id).await;
            assert!(result.is_ok(), "{result:?}");
            assert_eq!(
                output(&nodes, "aggregate")["output"],
                if selected { "LEFT" } else { "RIGHT" }
            );
            let status = |active| {
                if active {
                    NodeExecutionStatus::Completed
                } else {
                    NodeExecutionStatus::Skipped
                }
            };
            assert_statuses(
                &nodes,
                &[
                    ("start", status(true)),
                    ("choose", status(true)),
                    ("left", status(selected)),
                    ("right", status(!selected)),
                    ("aggregate", status(true)),
                    ("end", status(false)),
                ],
            );
            terminal(&db, &id, "succeeded");
        }
    }
    let mut w = branch(false);
    w.target_node_id = Some("left".into());
    let (result, nodes) = run(&chat, &session, w, "h196-unselected").await;
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("target was not selected by control flow"));
    assert_statuses(
        &nodes,
        &[
            ("start", NodeExecutionStatus::Completed),
            ("choose", NodeExecutionStatus::Completed),
            ("left", NodeExecutionStatus::Skipped),
            ("right", NodeExecutionStatus::Skipped),
            ("aggregate", NodeExecutionStatus::Skipped),
            ("end", NodeExecutionStatus::Skipped),
        ],
    );
    terminal(&db, "h196-unselected", "failed");
}

fn iteration() -> WorkflowRequest {
    serde_json::from_value(json!({"definition":{"version":1,"nodes":[
        {"id":"start","kind":"start","config":{"inputVariables":[{"name":"items","type":"array","default":["ONE","TWO"]}]}},
        {"id":"iterate","kind":"iteration","config":{"inputVariable":"{{#start.items#}}","outputVariable":"{{#body.output#}}","parallel":true,"maxConcurrency":2}},
        {"id":"entry","kind":"iteration-start"},
        {"id":"body","kind":"template-transform","config":{"templateScript":"seen:{{#entry.item#}}","templateInputVariables":[]}},
        {"id":"end","kind":"end"}
    ],"edges":[{"id":"a","source":"start","target":"iterate"},
        {"id":"b","source":"iterate","source_handle":"item","target":"entry"},
        {"id":"c","source":"entry","target":"body"},{"id":"d","source":"iterate","source_handle":"done","target":"end"}]},
        "target_node_id":"iterate"})).unwrap()
}

#[tokio::test]
async fn h196_iteration_owner_uses_real_item_scopes_but_unscoped_body_and_unknown_reject_before_dispatch(
) {
    let (_root, db, chat, session) = workflow_fixture().await;
    let (result, nodes) = run(&chat, &session, iteration(), "h196-owner").await;
    assert!(result.is_ok(), "{result:?}");
    assert_eq!(
        output(&nodes, "iterate")["output"],
        json!(["seen:ONE", "seen:TWO"])
    );
    let bodies: Vec<_> = nodes
        .iter()
        .filter(|n| n.node_id == "body" && n.status == NodeExecutionStatus::Completed)
        .collect();
    assert_eq!(bodies.len(), 2);
    assert!(bodies
        .iter()
        .all(|n| n.scope.len() == 2 && n.scope[0] == "iterate"));
    assert!(nodes
        .iter()
        .all(|n| n.node_id != "end" || n.status == NodeExecutionStatus::Skipped));
    terminal(&db, "h196-owner", "succeeded");
    for target in ["body", "entry", "unknown"] {
        let mut w = iteration();
        // If admission were late, this real file tool would have a side effect.
        w.definition.nodes.push(serde_json::from_value(json!({"id":"write","kind":"tool","config":{
            "toolName":"write_file","toolArguments":{"path":"must-not-exist.txt","content":"synthetic"}}})).unwrap());
        w.definition.edges.push(
            serde_json::from_value(json!({"id":"w","source":"start","target":"write"})).unwrap(),
        );
        w.target_node_id = Some(target.into());
        let id = format!("h196-invalid-{target}");
        let (result, nodes) = run(&chat, &session, w, &id).await;
        assert!(result
            .unwrap_err()
            .to_string()
            .contains(if target == "unknown" {
                "target node does not exist"
            } else {
                "item-scoped target requires an explicit item context"
            }));
        assert!(nodes.is_empty());
        assert!(!_root.path().join("must-not-exist.txt").exists());
        terminal(&db, &id, "failed");
        assert!(RunStore::new(&db)
            .events_after(&id, None)
            .unwrap()
            .iter()
            .all(|e| e.event_type != "tool_started"));
    }
}

#[derive(Default)]
struct TargetBroker {
    calls: AtomicUsize,
    waiting: AtomicUsize,
    cancel: bool,
    fail: bool,
}
#[async_trait::async_trait]
impl deepagent_builtins::http_request::HttpRequestClient for TargetBroker {
    fn validate(&self, request: &deepagent_builtins::http_request::HttpRequest) -> Result<()> {
        request.validate()
    }
    async fn request(
        &self,
        _: deepagent_builtins::http_request::HttpRequest,
        context: deepagent_tools::ToolExecutionContext,
    ) -> Result<Value> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.cancel {
            self.waiting.fetch_add(1, Ordering::SeqCst);
            context.cancelled().await;
            self.waiting.fetch_sub(1, Ordering::SeqCst);
            return Err(deepagent_core::error::CoreError::other(
                "synthetic target cancellation",
            ));
        }
        if self.fail {
            return Err(deepagent_core::error::CoreError::other(
                "synthetic ancestor failure",
            ));
        }
        Ok(json!({"body":"BROKER_VALUE","status_code":200,"headers":{}}))
    }
}

fn effectful() -> WorkflowRequest {
    serde_json::from_value(json!({"definition":{"version":1,"nodes":[
        {"id":"start","kind":"start"},
        {"id":"request","kind":"http-request","config":{"httpUrl":"https://synthetic.fixture.invalid/h196","httpMethod":"POST","httpBody":"LOCAL_ONLY","httpTimeout":7,"httpRetryCount":0}},
        {"id":"target","kind":"answer","config":{"answerTemplate":"{{#request.body#}}"}},
        {"id":"end","kind":"end"},
        {"id":"descendant","kind":"http-request","config":{"httpUrl":"https://synthetic.fixture.invalid/never","httpMethod":"POST"}},
        {"id":"unrelated","kind":"http-request","config":{"httpUrl":"https://synthetic.fixture.invalid/never","httpMethod":"POST"}}
    ],"edges":[{"id":"a","source":"start","target":"request"},{"id":"b","source":"request","target":"target"},
        {"id":"c","source":"target","target":"end"},{"id":"d","source":"target","target":"descendant"},
        {"id":"e","source":"start","target":"unrelated"}]},"target_node_id":"target"})).unwrap()
}

#[tokio::test]
async fn h196_ancestor_approval_denial_failure_cancel_and_reopen_never_dispatch_excluded_effects() {
    for case in ["normal", "denied", "failed", "cancelled"] {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("target.db");
        let db = Arc::new(Database::open(&path).unwrap());
        let (_root, db, chat, session) = workflow_fixture_with_database(root, db).await;
        let broker = Arc::new(TargetBroker {
            cancel: case == "cancelled",
            fail: case == "failed",
            ..Default::default()
        });
        let chat = chat.with_workflow_http_client(broker.clone());
        let worker_chat = chat.clone();
        let responder = chat.clone();
        let approvals = Arc::new(AtomicUsize::new(0));
        let approved = approvals.clone();
        let nodes = Arc::new(Mutex::new(Vec::new()));
        let capture = nodes.clone();
        let id = format!("h196-{case}");
        let worker_id = id.clone();
        let worker = tokio::spawn(async move {
            worker_chat
                .run_workflow(
                    effectful(),
                    worker_id,
                    session,
                    move |event| {
                        if let RuntimeEvent::WorkflowNode { event } = event {
                            capture.lock().unwrap().push(event);
                        }
                    },
                    move |approval| {
                        approved.fetch_add(1, Ordering::SeqCst);
                        responder
                            .resolve_approval(
                                &approval.call_id,
                                case != "denied",
                                "synthetic target approval",
                            )
                            .unwrap();
                    },
                )
                .await
        });
        if case == "cancelled" {
            tokio::time::timeout(Duration::from_secs(5), async {
                while broker.waiting.load(Ordering::SeqCst) == 0 {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
            assert!(chat.cancel_session(&id));
        }
        let result = tokio::time::timeout(Duration::from_secs(5), worker)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            result.is_ok(),
            matches!(case, "normal" | "cancelled"),
            "{case}: {result:?}"
        );
        assert_eq!(approvals.load(Ordering::SeqCst), 1);
        assert_eq!(
            broker.calls.load(Ordering::SeqCst),
            usize::from(case != "denied")
        );
        assert_eq!(broker.waiting.load(Ordering::SeqCst), 0);
        let events = nodes.lock().unwrap().clone();
        assert!(events
            .iter()
            .filter(|e| matches!(e.node_id.as_str(), "descendant" | "unrelated" | "end"))
            .all(|e| matches!(
                e.status,
                NodeExecutionStatus::Skipped | NodeExecutionStatus::Cancelled
            )));
        if case == "normal" {
            assert_eq!(output(&events, "target")["answer"], "BROKER_VALUE");
        }
        let kind = match case {
            "normal" => "succeeded",
            "cancelled" => "cancelled",
            _ => "failed",
        };
        let settled = terminal(&db, &id, kind);
        let before = RunStore::new(&db).events_after(&id, None).unwrap();
        let cursor = before[before.len() / 2].sequence;
        drop(chat);
        drop(db);
        let reopened = Database::open(&path).unwrap();
        let store = RunStore::new(&reopened);
        assert_eq!(store.events_after(&id, None).unwrap(), before);
        assert_eq!(
            store.events_after(&id, Some(cursor)).unwrap(),
            before
                .iter()
                .filter(|e| e.sequence > cursor)
                .cloned()
                .collect::<Vec<_>>()
        );
        assert_eq!(terminal(&reopened, &id, kind), settled);
        assert_eq!(
            broker.calls.load(Ordering::SeqCst),
            usize::from(case != "denied"),
            "replay must not redispatch"
        );
    }
}

#[tokio::test]
#[ignore = "requires local Node and desktop dependencies; production target serializer and consumer to real Kernel"]
async fn h196_desktop_target_request_and_real_kernel_events_reach_the_production_consumer() {
    let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../apps/desktop/scripts/audit-workflow-contract.cjs");
    let emitted = std::process::Command::new("node")
        .arg(&script)
        .arg("--emit-target-fixtures")
        .output()
        .unwrap();
    assert!(
        emitted.status.success(),
        "{}",
        String::from_utf8_lossy(&emitted.stderr)
    );
    let fixtures: Vec<Value> = serde_json::from_slice(&emitted.stdout).unwrap();
    assert_eq!(fixtures.len(), 3);
    for fixture in fixtures {
        let name = fixture["name"].as_str().unwrap();
        let w: WorkflowRequest = serde_json::from_value(fixture["workflow"].clone()).unwrap();
        assert_eq!(
            w.target_node_id.as_deref(),
            Some(name.strip_prefix("target-").unwrap())
        );
        let (_root, db, chat, session) = workflow_fixture().await;
        let (result, nodes) = run(&chat, &session, w, "h196-desktop").await;
        assert!(result.is_ok(), "{result:?}");
        terminal(&db, "h196-desktop", "succeeded");
        if name == "target-answer" {
            assert_eq!(output(&nodes, "answer")["answer"], "DESKTOP_FRESH");
        }
        let replay = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(
            replay.path(),
            serde_json::to_vec(&json!({"name":name,"events":nodes})).unwrap(),
        )
        .unwrap();
        let consumed = std::process::Command::new("node")
            .arg(&script)
            .arg("--consume-target-events")
            .arg(replay.path())
            .output()
            .unwrap();
        assert!(
            consumed.status.success(),
            "{}",
            String::from_utf8_lossy(&consumed.stderr)
        );
    }
}

#[derive(Default)]
struct TargetModel(Mutex<Vec<Value>>);
#[async_trait::async_trait]
impl deepagent_models::transport::HttpTransport for TargetModel {
    async fn stream(
        &self,
        request: deepagent_models::transport::TransportRequest,
        sink: &mut dyn deepagent_models::transport::EventSink,
    ) -> Result<()> {
        self.0
            .lock()
            .unwrap()
            .push(serde_json::from_str(&request.body).unwrap());
        deepagent_models::transport::HttpTransport::stream(
            &MockTransport::new([
                json!({"choices":[{"delta":{"content":"MODEL_FRESH"},"finish_reason":null}]})
                    .to_string(),
                json!({"choices":[{"delta":{},"finish_reason":"stop"}]}).to_string(),
                "[DONE]".into(),
            ]),
            request,
            sink,
        )
        .await
    }
}

#[tokio::test]
async fn h196_real_model_transport_and_registry_tool_still_use_the_shared_kernel() {
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
    let model = Arc::new(TargetModel::default());
    let chat = ChatService::new(db.clone(), settings, model.clone(), root.path());
    let session = chat.create_session(None).unwrap();
    let mut w = workflow("CURRENT_INPUT", Some("target"));
    let middle = w
        .definition
        .nodes
        .iter_mut()
        .find(|n| n.id == "middle")
        .unwrap();
    middle.kind = "llm".into();
    middle.config = serde_json::from_value(json!({"llmPrompt":"{{#source.output#}}"})).unwrap();
    w.definition
        .nodes
        .iter_mut()
        .find(|n| n.id == "target")
        .unwrap()
        .config = serde_json::from_value(json!({"answerTemplate":"{{#middle.text#}}"})).unwrap();
    let (result, nodes) = run(&chat, &session, w, "h196-model").await;
    assert!(result.is_ok(), "{result:?}");
    assert_eq!(output(&nodes, "target")["answer"], "MODEL_FRESH");
    {
        let calls = model.0.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert!(calls[0]["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["content"] == "CURRENT_INPUT"));
    }
    terminal(&db, "h196-model", "succeeded");
    std::fs::write(root.path().join("input.txt"), "SYNTHETIC_TOOL_VALUE").unwrap();
    let w=serde_json::from_value(json!({"definition":{"version":1,"nodes":[
        {"id":"start","kind":"start"},{"id":"inspect","kind":"tool","config":{"toolName":"read_file","toolArguments":{"path":"input.txt"}}},
        {"id":"end","kind":"end"}],"edges":[{"id":"a","source":"start","target":"inspect"},{"id":"b","source":"inspect","target":"end"}]},"target_node_id":"inspect"})).unwrap();
    let (result, nodes) = run(&chat, &session, w, "h196-tool").await;
    assert!(result.is_ok(), "{result:?}");
    assert_eq!(output(&nodes, "inspect")["success"], true);
    assert!(output(&nodes, "inspect")
        .to_string()
        .contains("SYNTHETIC_TOOL_VALUE"));
    assert!(nodes
        .iter()
        .all(|n| n.node_id != "end" || n.status == NodeExecutionStatus::Skipped));
    terminal(&db, "h196-tool", "succeeded");
    assert_eq!(
        model.0.lock().unwrap().len(),
        1,
        "tool target must not dispatch model"
    );
}

#[tokio::test]
async fn h196_target_does_not_bypass_existing_end_graph_admission() {
    let (_root, db, chat, session) = workflow_fixture().await;
    let w=serde_json::from_value(json!({"definition":{"version":1,"nodes":[
        {"id":"start","kind":"start"},
        {"id":"earlier","kind":"end","config":{"outputVariables":[{"name":"report","type":"string","value":"EARLIER"}]}},
        {"id":"target","kind":"end","config":{"outputVariables":[{"name":"report","type":"string","value":"TARGET"}]}}
    ],"edges":[{"id":"a","source":"start","target":"earlier"},{"id":"b","source":"earlier","target":"target"}]},"target_node_id":"target"})).unwrap();
    let (result, nodes) = run(&chat, &session, w, "h196-end-summary").await;
    // Current compiler forbids end successors. Targeting the last node must
    // not reinterpret that invalid graph as a valid local execution.
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("end nodes cannot have outgoing edges"));
    assert!(nodes.is_empty());
    terminal(&db, "h196-end-summary", "failed");
}
