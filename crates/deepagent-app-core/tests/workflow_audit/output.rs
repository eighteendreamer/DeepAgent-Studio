//! H-197: declared end outputs are an executable contract, not UI metadata.
use super::*;
use deepagent_core::error::Result;
use deepagent_runtime::workflow::NodeExecutionEvent;
use serde_json::{json, Value};

fn request(kind: &str, value: Value) -> WorkflowRequest {
    serde_json::from_value(json!({"definition":{"version":1,"nodes":[
        {"id":"start","kind":"start","config":{"inputVariables":[{"name":"payload","type":"object","required":true}]}},
        {"id":"end","kind":"end","config":{"outputVariables":[{"name":"report","type":kind,"value":"{{#start.payload.value#}}"}]}}
    ],"edges":[{"id":"a","source":"start","target":"end"}]},"inputs":{"payload":{"value":value}}})).unwrap()
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
            move |e| {
                if let RuntimeEvent::WorkflowNode { event } = e {
                    capture.lock().unwrap().push(event);
                }
            },
            |_| panic!("pure output fixture must never request approval"),
        )
        .await;
    let events = nodes.lock().unwrap().clone();
    (result, events)
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
        let node: NodeExecutionEvent = serde_json::from_value(e.data["event"].clone()).unwrap();
        if node.status.is_terminal() {
            assert!(
                seen.insert((node.scope, node.node_id)),
                "duplicate node terminal"
            );
            assert!(e.sequence < terminals[0].sequence);
        }
    }
    terminals[0].data.clone()
}

#[tokio::test]
async fn h197_wrong_type_fails_before_any_end_output_or_success_publication() {
    let (_root, db, chat, session) = workflow_fixture().await;
    let (result, nodes) = run(
        &chat,
        &session,
        request("number", json!("PRIVATE_OUTPUT_CANARY")),
        "h197-invalid",
    )
    .await;
    let error = result.unwrap_err().to_string();
    assert!(
        error
            .contains("InvalidWorkflowOutput: node end field report expected number actual string"),
        "{error}"
    );
    assert!(!error.contains("PRIVATE_OUTPUT_CANARY"));
    let ends: Vec<_> = nodes
        .iter()
        .filter(|n| n.node_id == "end" && n.status.is_terminal())
        .collect();
    assert_eq!(ends.len(), 1);
    assert_eq!(ends[0].status, NodeExecutionStatus::Failed);
    assert!(ends[0].outputs.is_none());
    assert!(!ends[0]
        .error
        .as_ref()
        .unwrap()
        .contains("PRIVATE_OUTPUT_CANARY"));
    assert!(!terminal(&db, "h197-invalid", "failed")
        .to_string()
        .contains("PRIVATE_OUTPUT_CANARY"));
}

fn completed<'a>(nodes: &'a [NodeExecutionEvent], id: &str) -> &'a NodeExecutionEvent {
    nodes
        .iter()
        .find(|n| n.node_id == id && n.status == NodeExecutionStatus::Completed)
        .unwrap()
}

#[tokio::test]
async fn h197_all_six_output_types_and_null_cross_the_real_full_and_targeted_kernel() {
    let (_root, db, chat, session) = workflow_fixture().await;
    let samples = [
        json!(""),
        json!(9_007_199_254_740_993_u64),
        json!(false),
        json!({"nested":[null,false,0]}),
        json!([{}, null]),
        json!("artifact://h197/synthetic"),
        json!(null),
    ];
    for (type_index, kind) in ["string", "number", "boolean", "object", "array", "file"]
        .iter()
        .enumerate()
    {
        for (value_index, value) in samples.iter().enumerate() {
            for targeted in [false, true] {
                let accepted = type_index == value_index || (type_index == 0 && value_index == 5);
                let mut w = request(kind, value.clone());
                if targeted {
                    w.target_node_id = Some("end".into());
                }
                let id = format!("h197-types-{type_index}-{value_index}-{targeted}");
                let (result, nodes) = run(&chat, &session, w, &id).await;
                assert_eq!(result.is_ok(), accepted, "{id}: {result:?}");
                if accepted {
                    assert_eq!(
                        completed(&nodes, "end").outputs,
                        Some(json!({"report":value}))
                    );
                } else {
                    assert!(nodes.iter().any(|n| n.node_id == "end"
                        && n.status == NodeExecutionStatus::Failed
                        && n.outputs.is_none()));
                }
                terminal(&db, &id, if accepted { "succeeded" } else { "failed" });
            }
        }
    }
}

#[tokio::test]
async fn h197_constants_nested_data_and_multiple_fields_never_publish_partial_or_coerced_output() {
    let (_root, db, chat, session) = workflow_fixture().await;
    for literal in [false, true] {
        let payload = json!({"nested":[false,0,null],"text":"{{#missing.output#}}","extra_inside_payload":true});
        let mut w = request("object", payload.clone());
        if literal {
            w.definition.nodes[1].config["outputVariables"][0]["value"] =
                json!({"literal":[false,0,null]});
        }
        let id = format!("h197-data-{literal}");
        let (result, nodes) = run(&chat, &session, w, &id).await;
        assert!(result.is_ok(), "{result:?}");
        assert_eq!(
            completed(&nodes, "end").outputs,
            Some(json!({"report":if literal {json!({"literal":[false,0,null]})} else {payload}}))
        );
        terminal(&db, &id, "succeeded");
    }
    for (index, value) in [json!("42"), json!(null), json!({})]
        .into_iter()
        .enumerate()
    {
        let mut w = request("number", value);
        w.definition.nodes[1].config["outputVariables"]
            .as_array_mut()
            .unwrap()
            .insert(
                0,
                json!({"name":"valid_first","type":"string","value":"SAFE"}),
            );
        let id = format!("h197-partial-{index}");
        let (result, nodes) = run(&chat, &session, w, &id).await;
        assert!(result.is_err());
        assert!(nodes
            .iter()
            .filter(|n| n.node_id == "end")
            .all(|n| n.outputs.is_none() && n.status != NodeExecutionStatus::Completed));
        terminal(&db, &id, "failed");
    }
    let mut w = request("string", json!("ignored"));
    w.definition.nodes[1].config["outputVariables"][0]["value"] =
        json!("{{#start.payload.absent#}}");
    let (result, nodes) = run(&chat, &session, w, "h197-unavailable").await;
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("output binding unavailable"));
    assert!(nodes
        .iter()
        .any(|n| n.node_id == "end" && n.status == NodeExecutionStatus::Failed));
    terminal(&db, "h197-unavailable", "failed");
}

#[tokio::test]
async fn h197_malformed_output_contract_rejects_before_any_ancestor_file_write() {
    let (_root, db, chat, session) = workflow_fixture().await;
    for (index, field) in [
        "value",
        "schema",
        "properties",
        "items",
        "required",
        "nullable",
        "unknown",
    ]
    .iter()
    .enumerate()
    {
        let mut w = request("string", json!("DATA"));
        let declaration = w.definition.nodes[1].config["outputVariables"][0]
            .as_object_mut()
            .unwrap();
        if *field == "value" {
            declaration.remove("value");
        } else {
            declaration.insert((*field).into(), json!({"nested":"PRIVATE_SCHEMA_CANARY"}));
        }
        w.definition.nodes.push(serde_json::from_value(json!({"id":"write","kind":"tool","config":{"toolName":"write_file","toolArguments":{"path":"must-not-exist.txt","content":"synthetic"}}})).unwrap());
        w.definition.edges=serde_json::from_value(json!([{"id":"a","source":"start","target":"write"},{"id":"b","source":"write","target":"end"}])).unwrap();
        let id = format!("h197-schema-{index}");
        let (result, nodes) = run(&chat, &session, w, &id).await;
        let error = result.unwrap_err().to_string();
        assert!(error.contains("WorkflowOutputSchemaInvalid"), "{error}");
        assert!(!error.contains("PRIVATE_SCHEMA_CANARY"));
        assert!(nodes.is_empty());
        assert!(!_root.path().join("must-not-exist.txt").exists());
        let settled = terminal(&db, &id, "failed");
        assert!(!settled.to_string().contains("PRIVATE_SCHEMA_CANARY"));
        assert!(RunStore::new(&db)
            .events_after(&id, None)
            .unwrap()
            .iter()
            .all(|e| e.event_type != "tool_started"));
    }
}

#[tokio::test]
async fn h197_invalid_end_never_becomes_a_successful_join_or_iteration_item() {
    let (_root, db, chat, session) = workflow_fixture().await;
    for (case, good_value) in [json!("WRONG"), json!(7)].into_iter().enumerate() {
        for order in 0..6 {
            let mut w:WorkflowRequest=serde_json::from_value(json!({"definition":{"version":1,"nodes":[
                {"id":"start","kind":"start"},{"id":"good","kind":"end","config":{"outputVariables":[{"name":"report","type":"string","value":"GOOD"}]}},
                {"id":"checked","kind":"end","config":{"outputVariables":[{"name":"report","type":"number","value":good_value}]}}
            ],"edges":[{"id":"a","source":"start","target":"good"},{"id":"b","source":"start","target":"checked"}]}})).unwrap();
            w.definition.nodes.rotate_left(order % 3);
            if order >= 3 {
                w.definition.nodes.reverse();
            }
            let id = format!("h197-join-{case}-{order}");
            let (result, nodes) = run(&chat, &session, w, &id).await;
            assert_eq!(result.is_ok(), case == 1, "{result:?}");
            if case == 0 {
                assert!(nodes.iter().any(|n| n.node_id == "checked"
                    && n.status == NodeExecutionStatus::Failed
                    && n.outputs.is_none()));
            }
            terminal(&db, &id, if case == 1 { "succeeded" } else { "failed" });
        }
    }
    for valid in [false, true] {
        let items = if valid {
            json!([1, 2])
        } else {
            json!([1, "WRONG"])
        };
        let w=serde_json::from_value(json!({"definition":{"version":1,"nodes":[
            {"id":"start","kind":"start","config":{"inputVariables":[{"name":"items","type":"array"}]}},
            {"id":"iterate","kind":"iteration","config":{"inputVariable":"{{#start.items#}}","outputVariable":"{{#item-end.report#}}","parallel":false,"maxConcurrency":1,"errorHandling":"terminate"}},
            {"id":"entry","kind":"iteration-start"},
            {"id":"item-end","kind":"end","config":{"outputVariables":[{"name":"report","type":"number","value":"{{#entry.item#}}"}]}},
            {"id":"end","kind":"end","config":{"outputVariables":[{"name":"report","type":"array","value":"{{#iterate.output#}}"}]}}
        ],"edges":[{"id":"a","source":"start","target":"iterate"},{"id":"b","source":"iterate","source_handle":"item","target":"entry"},
            {"id":"c","source":"entry","target":"item-end"},{"id":"d","source":"iterate","source_handle":"done","target":"end"}]},"inputs":{"items":items}})).unwrap();
        let id = format!("h197-item-{valid}");
        let (result, nodes) = run(&chat, &session, w, &id).await;
        assert_eq!(result.is_ok(), valid, "{result:?}");
        if valid {
            assert_eq!(
                completed(&nodes, "end").outputs,
                Some(json!({"report":[1,2]}))
            );
        } else {
            assert!(nodes.iter().any(|n| n.node_id == "item-end"
                && !n.scope.is_empty()
                && n.status == NodeExecutionStatus::Failed
                && n.outputs.is_none()));
        }
        terminal(&db, &id, if valid { "succeeded" } else { "failed" });
    }
}

#[derive(Default)]
struct OutputBroker {
    calls: AtomicUsize,
    waiting: AtomicUsize,
    cancel: bool,
    fail: bool,
}
#[async_trait::async_trait]
impl deepagent_builtins::http_request::HttpRequestClient for OutputBroker {
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
                "synthetic cancellation",
            ));
        }
        if self.fail {
            return Err(deepagent_core::error::CoreError::other(
                "synthetic upstream failure",
            ));
        }
        Ok(json!({"body":"BROKER_TYPED_VALUE","status_code":200,"headers":{}}))
    }
}

#[tokio::test]
async fn h197_approved_upstream_results_and_output_failure_survive_reopen_without_redispatch() {
    for case in [
        "normal",
        "typed-failure",
        "upstream-failure",
        "cancelled",
        "denied",
    ] {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("output.db");
        let db = Arc::new(Database::open(&path).unwrap());
        let (_root, db, chat, session) = workflow_fixture_with_database(root, db).await;
        let broker = Arc::new(OutputBroker {
            cancel: case == "cancelled",
            fail: case == "upstream-failure",
            ..Default::default()
        });
        let chat = chat.with_workflow_http_client(broker.clone());
        let worker_chat = chat.clone();
        let responder = chat.clone();
        let approvals = Arc::new(AtomicUsize::new(0));
        let approved = approvals.clone();
        let w=serde_json::from_value(json!({"definition":{"version":1,"nodes":[
            {"id":"start","kind":"start"},{"id":"request","kind":"http-request","config":{"httpUrl":"https://synthetic.fixture.invalid/output","httpMethod":"POST","httpBody":"LOCAL_ONLY","httpTimeout":7,"httpRetryCount":0}},
            {"id":"end","kind":"end","config":{"outputVariables":[{"name":"report","type":if case=="typed-failure" {"number"} else {"string"},"value":"{{#request.body#}}"}]}}
        ],"edges":[{"id":"a","source":"start","target":"request"},{"id":"b","source":"request","target":"end"}]}})).unwrap();
        let id = format!("h197-broker-{case}");
        let worker_id = id.clone();
        let nodes = Arc::new(Mutex::new(Vec::new()));
        let capture = nodes.clone();
        let worker = tokio::spawn(async move {
            worker_chat
                .run_workflow(
                    w,
                    worker_id,
                    session,
                    move |e| {
                        if let RuntimeEvent::WorkflowNode { event } = e {
                            capture.lock().unwrap().push(event);
                        }
                    },
                    move |approval| {
                        approved.fetch_add(1, Ordering::SeqCst);
                        responder
                            .resolve_approval(
                                &approval.call_id,
                                case != "denied",
                                "synthetic output fixture",
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
        if case == "normal" {
            assert_eq!(
                completed(&events, "end").outputs,
                Some(json!({"report":"BROKER_TYPED_VALUE"}))
            );
        } else {
            assert!(events
                .iter()
                .all(|n| n.node_id != "end" || n.status != NodeExecutionStatus::Completed));
        }
        if case == "typed-failure" {
            assert!(events
                .iter()
                .any(|n| n.node_id == "request" && n.status == NodeExecutionStatus::Completed));
            assert!(result
                .unwrap_err()
                .to_string()
                .contains("expected number actual string"));
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
            "replay is not redispatch or rollback"
        );
    }
}

#[tokio::test]
#[ignore = "requires local Node and desktop dependencies; production output serializer and consumer to real Kernel"]
async fn h197_desktop_output_contract_and_real_events_reach_the_production_consumer() {
    let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../apps/desktop/scripts/audit-workflow-contract.cjs");
    let emitted = std::process::Command::new("node")
        .arg(&script)
        .arg("--emit-output-contract-fixtures")
        .output()
        .unwrap();
    assert!(
        emitted.status.success(),
        "{}",
        String::from_utf8_lossy(&emitted.stderr)
    );
    let fixtures: Vec<Value> = serde_json::from_slice(&emitted.stdout).unwrap();
    assert_eq!(fixtures.len(), 5);
    for fixture in fixtures {
        let name = fixture["name"].as_str().unwrap();
        let invalid = name.ends_with("invalid");
        let w: WorkflowRequest = serde_json::from_value(fixture["workflow"].clone()).unwrap();
        let (_root, db, chat, session) = workflow_fixture().await;
        let (result, nodes) = run(&chat, &session, w, "h197-desktop").await;
        assert_eq!(result.is_err(), invalid, "{name}: {result:?}");
        if !invalid {
            let expected = match name {
                "end-contract-string" => json!("DESKTOP_TYPED"),
                "end-contract-number" => json!(0),
                "end-contract-object" => json!({"nested":[false,null]}),
                "end-contract-array" => json!([0, false, {}]),
                _ => panic!("unexpected fixture"),
            };
            assert_eq!(
                completed(&nodes, "end").outputs,
                Some(json!({"report":expected}))
            );
        }
        let settled = terminal(
            &db,
            "h197-desktop",
            if invalid { "failed" } else { "succeeded" },
        );
        let replay = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(
            replay.path(),
            serde_json::to_vec(&json!({"name":name,"events":nodes,"terminal":settled})).unwrap(),
        )
        .unwrap();
        let consumed = std::process::Command::new("node")
            .arg(&script)
            .arg("--consume-output-events")
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
