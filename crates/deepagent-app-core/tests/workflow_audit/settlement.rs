//! H-194/H-195: an end settles its node, not an unfinished owning walk.
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
            |_| panic!("synthetic settlement graphs must not request approval"),
        )
        .await;
    let events = nodes.lock().unwrap().clone();
    (result, events)
}

fn request(nodes: Value, edges: Value) -> WorkflowRequest {
    serde_json::from_value(json!({"definition":{"version":1,"nodes":nodes,"edges":edges}})).unwrap()
}

fn assert_settled(nodes: &[NodeExecutionEvent], expected: &[(&str, NodeExecutionStatus)]) {
    let mut terminal = BTreeMap::new();
    for node in nodes.iter().filter(|node| node.status.is_terminal()) {
        assert!(
            terminal
                .insert(node.node_id.as_str(), node.status)
                .is_none(),
            "duplicate terminal for {}",
            node.node_id
        );
    }
    assert_eq!(terminal, expected.iter().copied().collect());
}

#[tokio::test]
async fn h194_owned_tail_executes_for_every_node_array_permutation() {
    let nodes = [
        json!({"id":"start","kind":"start"}),
        json!({"id":"end","kind":"end"}),
        json!({"id":"tail","kind":"variable-assigner","config":{"assignments":[]}}),
    ];
    let edges = json!([
        {"id":"a","source":"start","target":"end"},
        {"id":"b","source":"start","target":"tail"}
    ]);
    let (_root, db, chat, session) = workflow_fixture().await;
    for order in [
        [0, 1, 2],
        [0, 2, 1],
        [1, 0, 2],
        [1, 2, 0],
        [2, 0, 1],
        [2, 1, 0],
    ] {
        let id = format!("h194-owned-{}{}{}", order[0], order[1], order[2]);
        let (result, events) = run(
            &chat,
            &session,
            request(order.map(|i| nodes[i].clone()).into(), edges.clone()),
            &id,
        )
        .await;
        assert!(result.is_ok(), "{result:?}");
        eprintln!(
            "order={order:?} terminals={:?}",
            events
                .iter()
                .filter(|node| node.status.is_terminal())
                .map(|node| (&node.node_id, node.status))
                .collect::<Vec<_>>()
        );
        assert_settled(
            &events,
            &[
                ("start", NodeExecutionStatus::Completed),
                ("end", NodeExecutionStatus::Completed),
                ("tail", NodeExecutionStatus::Completed),
            ],
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

fn assert_durable_settlement(db: &Database, id: &str, kind: &str) -> Value {
    let store = RunStore::new(db);
    let record = store.get(id).unwrap().unwrap();
    assert_eq!(record.terminal_kind.as_deref(), Some(kind));
    let events = store.events_after(id, None).unwrap();
    let terminal: Vec<_> = events
        .iter()
        .filter(|e| e.event_type == "run_terminal")
        .collect();
    assert_eq!(terminal.len(), 1);
    assert_eq!(terminal[0].data["kind"], kind);
    let mut nodes = BTreeMap::new();
    for event in events.iter().filter(|e| e.event_type == "workflow_node") {
        let node: NodeExecutionEvent = serde_json::from_value(event.data["event"].clone()).unwrap();
        if node.status.is_terminal() {
            assert!(event.sequence < terminal[0].sequence);
            assert!(nodes
                .insert((node.scope, node.node_id), node.status)
                .is_none());
        }
    }
    assert!(!nodes.is_empty());
    terminal[0].data.clone()
}

fn permutations(nodes: &[Value]) -> Vec<Vec<Value>> {
    fn collect(nodes: &mut [Value], index: usize, out: &mut Vec<Vec<Value>>) {
        if index == nodes.len() {
            out.push(nodes.to_vec());
            return;
        }
        for i in index..nodes.len() {
            nodes.swap(index, i);
            collect(nodes, index + 1, out);
            nodes.swap(index, i);
        }
    }
    let mut out = Vec::new();
    collect(&mut nodes.to_vec(), 0, &mut out);
    out
}

fn parallel_graph() -> (Value, Value) {
    (
        json!([
            {"id":"start","kind":"start"},
            {"id":"left","kind":"variable-assigner","config":{"assignments":[{"variable":"output","value":"LEFT"}]}},
            {"id":"left-end","kind":"end","config":{"outputVariables":[{"name":"report","type":"string","value":"{{#left.output#}}"}]}},
            {"id":"right","kind":"variable-assigner","config":{"assignments":[{"variable":"output","value":"RIGHT"}]}},
            {"id":"right-end","kind":"end","config":{"outputVariables":[{"name":"report","type":"string","value":"{{#right.output#}}"}]}}
        ]),
        json!([
            {"id":"a","source":"start","target":"left"},
            {"id":"b","source":"left","target":"left-end"},
            {"id":"c","source":"start","target":"right"},
            {"id":"d","source":"right","target":"right-end"}
        ]),
    )
}

#[tokio::test]
async fn h195_all_120_parallel_permutations_join_both_end_outputs_before_terminal() {
    let (nodes, edges) = parallel_graph();
    let (_root, db, chat, session) = workflow_fixture().await;
    let orders = permutations(nodes.as_array().unwrap());
    assert_eq!(orders.len(), 120);
    for (index, nodes) in orders.into_iter().enumerate() {
        let mut edges = edges.clone();
        if index % 2 == 0 {
            edges.as_array_mut().unwrap().reverse();
        }
        let id = format!("h195-parallel-{index}");
        let (result, nodes) = run(&chat, &session, request(nodes.into(), edges), &id).await;
        assert!(result.is_ok(), "{result:?}");
        assert_settled(
            &nodes,
            &[
                ("start", NodeExecutionStatus::Completed),
                ("left", NodeExecutionStatus::Completed),
                ("left-end", NodeExecutionStatus::Completed),
                ("right", NodeExecutionStatus::Completed),
                ("right-end", NodeExecutionStatus::Completed),
            ],
        );
        assert_eq!(
            assert_durable_settlement(&db, &id, "succeeded")["message"],
            r#"{"left-end":{"report":"LEFT"},"right-end":{"report":"RIGHT"}}"#
        );
    }
}

#[tokio::test]
async fn h195_single_end_join_and_conditional_choice_keep_the_single_output_contract() {
    let start = json!({"id":"start","kind":"start"});
    let choose = json!({"id":"choose","kind":"if-else","config":{"conditions":[{"id":"true","logic":"and","items":[{"variable":"yes","operator":"is","value":"yes"}]}]}});
    let left = json!({"id":"left-end","kind":"end","config":{"outputVariables":[{"name":"report","type":"string","value":"LEFT"}]}});
    let right = json!({"id":"right-end","kind":"end","config":{"outputVariables":[{"name":"report","type":"string","value":"RIGHT"}]}});
    let (_root, db, chat, session) = workflow_fixture().await;
    for selected in [true, false] {
        for (index, nodes) in
            permutations(&[start.clone(), choose.clone(), left.clone(), right.clone()])
                .into_iter()
                .enumerate()
        {
            let edges = json!([
                {"id":"a","source":"start","target":"choose"},
                {"id":"b","source":"choose","target":"left-end","source_handle":if selected {"true"} else {"false"}},
                {"id":"c","source":"choose","target":"right-end","source_handle":if selected {"false"} else {"true"}}
            ]);
            let id = format!("h195-choice-{selected}-{index}");
            let (result, nodes) = run(&chat, &session, request(nodes.into(), edges), &id).await;
            assert!(result.is_ok(), "{result:?}");
            assert_settled(
                &nodes,
                &[
                    ("start", NodeExecutionStatus::Completed),
                    ("choose", NodeExecutionStatus::Completed),
                    (
                        "left-end",
                        if selected {
                            NodeExecutionStatus::Completed
                        } else {
                            NodeExecutionStatus::Skipped
                        },
                    ),
                    (
                        "right-end",
                        if selected {
                            NodeExecutionStatus::Skipped
                        } else {
                            NodeExecutionStatus::Completed
                        },
                    ),
                ],
            );
            assert!(nodes
                .iter()
                .filter(|n| n.status == NodeExecutionStatus::Skipped)
                .all(|n| n.error.as_deref() == Some("conditional branch not selected")));
            assert_eq!(
                assert_durable_settlement(&db, &id, "succeeded")["message"],
                if selected { "\"LEFT\"" } else { "\"RIGHT\"" }
            );
        }
    }
    let (mut nodes, _) = parallel_graph();
    nodes.as_array_mut().unwrap().retain(|n| n["kind"] != "end");
    nodes
        .as_array_mut()
        .unwrap()
        .push(json!({"id":"end","kind":"end","config":{"outputVariables":[
        {"name":"report","type":"string","value":"{{#left.output#}}:{{#right.output#}}"}]}}));
    let edges = json!([
        {"id":"a","source":"start","target":"left"}, {"id":"b","source":"left","target":"end"},
        {"id":"c","source":"start","target":"right"}, {"id":"d","source":"right","target":"end"}
    ]);
    for (index, nodes) in permutations(nodes.as_array().unwrap())
        .into_iter()
        .enumerate()
    {
        let id = format!("h195-single-join-{index}");
        let (result, nodes) = run(&chat, &session, request(nodes.into(), edges.clone()), &id).await;
        assert!(result.is_ok(), "{result:?}");
        assert_settled(
            &nodes,
            &[
                ("start", NodeExecutionStatus::Completed),
                ("left", NodeExecutionStatus::Completed),
                ("right", NodeExecutionStatus::Completed),
                ("end", NodeExecutionStatus::Completed),
            ],
        );
        assert_eq!(
            assert_durable_settlement(&db, &id, "succeeded")["message"],
            "\"LEFT:RIGHT\""
        );
    }
}

fn early_end_graph(failing: bool) -> WorkflowRequest {
    let tail = if failing {
        json!({"id":"tail","kind":"list-operator","config":{"input":"not-an-array","action":"limit","limit":0}})
    } else {
        json!({"id":"tail","kind":"variable-assigner","config":{"assignments":[]}})
    };
    request(
        json!([
            {"id":"start","kind":"start"}, {"id":"end","kind":"end"}, tail,
            {"id":"after","kind":"variable-assigner","config":{"assignments":[]}}
        ]),
        json!([
            {"id":"a","source":"start","target":"end"}, {"id":"b","source":"start","target":"tail"},
            {"id":"c","source":"tail","target":"after"}
        ]),
    )
}

#[tokio::test]
async fn h194_late_failure_settles_unvisited_nodes_and_never_publishes_success() {
    let (_root, db, chat, session) = workflow_fixture().await;
    let (result, nodes) = run(&chat, &session, early_end_graph(true), "h194-failure").await;
    assert!(result.is_err());
    assert_settled(
        &nodes,
        &[
            ("start", NodeExecutionStatus::Completed),
            ("end", NodeExecutionStatus::Completed),
            ("tail", NodeExecutionStatus::Failed),
            ("after", NodeExecutionStatus::Skipped),
        ],
    );
    assert_eq!(
        nodes
            .iter()
            .find(|n| n.node_id == "after")
            .unwrap()
            .error
            .as_deref(),
        Some("workflow stopped before node execution")
    );
    assert_durable_settlement(&db, "h194-failure", "failed");
}

#[tokio::test]
async fn h194_target_walk_settles_only_the_h196_recompute_scope() {
    for target in ["start", "end"] {
        let (_root, db, chat, session) = workflow_fixture().await;
        let mut workflow = early_end_graph(false);
        workflow.target_node_id = Some(target.into());
        let (result, nodes) = run(&chat, &session, workflow, "h194-target").await;
        assert!(result.is_ok(), "{result:?}");
        let end = if target == "end" {
            NodeExecutionStatus::Completed
        } else {
            NodeExecutionStatus::Skipped
        };
        assert_settled(
            &nodes,
            &[
                ("start", NodeExecutionStatus::Completed),
                ("end", end),
                ("tail", NodeExecutionStatus::Skipped),
                ("after", NodeExecutionStatus::Skipped),
            ],
        );
        if target == "end" {
            assert!(nodes
                .iter()
                .filter(|n| n.status == NodeExecutionStatus::Skipped)
                .all(|n| n.error.is_some()));
        }
        assert_durable_settlement(&db, "h194-target", "succeeded");
    }
}

#[tokio::test]
async fn h194_195_success_failure_and_join_survive_reopen_and_cursor_without_dispatch() {
    for case in 0..3 {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("settlement.db");
        let db = Arc::new(Database::open(&path).unwrap());
        let (_root, db, chat, session) = workflow_fixture_with_database(root, db).await;
        let workflow = if case == 2 {
            let (nodes, edges) = parallel_graph();
            request(nodes, edges)
        } else {
            early_end_graph(case == 1)
        };
        let (result, nodes) = run(&chat, &session, workflow, "h194-durable").await;
        assert_eq!(result.is_err(), case == 1);
        let terminal = assert_durable_settlement(
            &db,
            "h194-durable",
            if case == 1 { "failed" } else { "succeeded" },
        );
        let before = RunStore::new(&db)
            .events_after("h194-durable", None)
            .unwrap();
        let cursor = before[before.len() / 2].sequence;
        assert_eq!(
            nodes.iter().filter(|n| n.status.is_terminal()).count(),
            if case == 2 { 5 } else { 4 }
        );
        drop(chat);
        drop(db);
        let reopened = Database::open(&path).unwrap();
        let store = RunStore::new(&reopened);
        assert_eq!(store.events_after("h194-durable", None).unwrap(), before);
        assert_eq!(
            store.events_after("h194-durable", Some(cursor)).unwrap(),
            before
                .iter()
                .filter(|e| e.sequence > cursor)
                .cloned()
                .collect::<Vec<_>>()
        );
        assert_eq!(
            assert_durable_settlement(
                &reopened,
                "h194-durable",
                if case == 1 { "failed" } else { "succeeded" }
            ),
            terminal
        );
        assert!(
            deepagent_persistence::run_control::RunControlStore::new(&reopened)
                .list_actions("h194-durable")
                .unwrap()
                .is_empty()
        );
    }
}

#[derive(Default)]
struct SettlementBroker {
    calls: AtomicUsize,
    waiting: AtomicUsize,
    wait_for_cancel: bool,
}

#[async_trait::async_trait]
impl deepagent_builtins::http_request::HttpRequestClient for SettlementBroker {
    fn validate(&self, request: &deepagent_builtins::http_request::HttpRequest) -> Result<()> {
        request.validate()
    }
    async fn request(
        &self,
        _: deepagent_builtins::http_request::HttpRequest,
        context: deepagent_tools::ToolExecutionContext,
    ) -> Result<Value> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.wait_for_cancel {
            self.waiting.fetch_add(1, Ordering::SeqCst);
            context.cancelled().await;
            self.waiting.fetch_sub(1, Ordering::SeqCst);
            return Err(deepagent_core::error::CoreError::other(
                "synthetic settlement cancellation",
            ));
        }
        Ok(json!({"body":"LOCAL_RESULT","status_code":200,"headers":{}}))
    }
}

#[tokio::test]
async fn h194_195_post_end_approved_broker_executes_once_and_cancellation_closes_the_walk() {
    for cancel in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("broker-settlement.db");
        let db = Arc::new(Database::open(&path).unwrap());
        let (_root, db, chat, session) = workflow_fixture_with_database(root, db).await;
        let broker = Arc::new(SettlementBroker {
            wait_for_cancel: cancel,
            ..Default::default()
        });
        let chat = chat.with_workflow_http_client(broker.clone());
        let workflow = request(
            json!([
                {"id":"start","kind":"start"}, {"id":"end","kind":"end"},
                {"id":"request","kind":"http-request","config":{"httpUrl":"https://synthetic.fixture.invalid/settlement",
                    "httpMethod":"POST","httpBody":"LOCAL_ONLY","httpTimeout":7,"httpRetryCount":0}},
                {"id":"after","kind":"variable-assigner","config":{"assignments":[]}}
            ]),
            json!([
                {"id":"a","source":"start","target":"end"}, {"id":"b","source":"start","target":"request"},
                {"id":"c","source":"request","target":"after"}
            ]),
        );
        let worker_chat = chat.clone();
        let responder = chat.clone();
        let nodes = Arc::new(Mutex::new(Vec::new()));
        let capture = nodes.clone();
        let worker = tokio::spawn(async move {
            worker_chat
                .run_workflow(
                    workflow,
                    "h194-broker".into(),
                    session,
                    move |event| {
                        if let RuntimeEvent::WorkflowNode { event } = event {
                            capture.lock().unwrap().push(event);
                        }
                    },
                    move |approval| {
                        responder
                            .resolve_approval(
                                &approval.call_id,
                                true,
                                "synthetic settlement fixture",
                            )
                            .unwrap();
                    },
                )
                .await
        });
        if cancel {
            tokio::time::timeout(Duration::from_secs(5), async {
                while broker.waiting.load(Ordering::SeqCst) == 0 {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
            assert!(chat.cancel_session("h194-broker"));
        }
        let result = tokio::time::timeout(Duration::from_secs(5), worker)
            .await
            .unwrap()
            .unwrap();
        // The existing Kernel adapter treats an orderly cancellation as Ok;
        // only the durable terminal below determines success vs cancellation.
        assert!(result.is_ok(), "{result:?}");
        assert_eq!(broker.calls.load(Ordering::SeqCst), 1);
        assert_eq!(broker.waiting.load(Ordering::SeqCst), 0);
        let expected = if cancel {
            NodeExecutionStatus::Cancelled
        } else {
            NodeExecutionStatus::Completed
        };
        assert_settled(
            &nodes.lock().unwrap(),
            &[
                ("start", NodeExecutionStatus::Completed),
                ("end", NodeExecutionStatus::Completed),
                ("request", expected),
                ("after", expected),
            ],
        );
        let kind = if cancel { "cancelled" } else { "succeeded" };
        assert_durable_settlement(&db, "h194-broker", kind);
        let before = RunStore::new(&db)
            .events_after("h194-broker", None)
            .unwrap();
        assert_eq!(
            before
                .iter()
                .filter(|e| e.event_type == "tool_started")
                .count(),
            1
        );
        drop(chat);
        drop(db);
        let reopened = Database::open(&path).unwrap();
        assert_eq!(
            RunStore::new(&reopened)
                .events_after("h194-broker", None)
                .unwrap(),
            before
        );
        assert_eq!(
            broker.calls.load(Ordering::SeqCst),
            1,
            "replay is not redispatch"
        );
        assert_durable_settlement(&reopened, "h194-broker", kind);
    }
}

#[tokio::test]
#[ignore = "requires local Node and desktop dependencies; production serializer to real Kernel"]
async fn h194_195_desktop_serialized_tail_parallel_and_choice_settle_through_the_kernel() {
    let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../apps/desktop/scripts/audit-workflow-contract.cjs");
    let output = std::process::Command::new("node")
        .arg(script)
        .arg("--emit-settlement-fixtures")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let fixtures: Vec<Value> = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(fixtures.len(), 4);
    for fixture in fixtures {
        let request: WorkflowRequest = serde_json::from_value(fixture["workflow"].clone()).unwrap();
        let (_root, db, chat, session) = workflow_fixture().await;
        let name = fixture["name"].as_str().unwrap();
        let (result, nodes) = run(&chat, &session, request.clone(), "h194-desktop").await;
        assert!(result.is_ok(), "{name}: {result:?}");
        let expected: Vec<_> = request
            .definition
            .nodes
            .iter()
            .map(|node| {
                let skipped = (name == "settlement-choice-left" && node.id == "right-end")
                    || (name == "settlement-choice-right" && node.id == "left-end");
                (
                    node.id.as_str(),
                    if skipped {
                        NodeExecutionStatus::Skipped
                    } else {
                        NodeExecutionStatus::Completed
                    },
                )
            })
            .collect();
        assert_settled(&nodes, &expected);
        let terminal = assert_durable_settlement(&db, "h194-desktop", "succeeded");
        assert_eq!(
            terminal["message"],
            match name {
                "settlement-parallel" =>
                    r#"{"left-end":{"report":"LEFT"},"right-end":{"report":"RIGHT"}}"#,
                "settlement-choice-right" => "\"RIGHT\"",
                _ => "\"LEFT\"",
            }
        );
    }
}
