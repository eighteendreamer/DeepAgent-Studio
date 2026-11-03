//! H-198: deterministic condition truth, using the real Kernel and branch walk.
use super::*;
use deepagent_core::error::Result;
use deepagent_runtime::workflow::NodeExecutionEvent;
use serde_json::{json, Value};

fn request(logic: &str, left: bool, right: bool, legacy: bool) -> WorkflowRequest {
    let group = if legacy {
        json!({"id":"true","logicalOperator":logic,"conditions":[
            {"left":left,"operator":"is","right":true},
            {"left":right,"operator":"is","right":true}
        ]})
    } else {
        json!({"id":"true","logic":logic,"items":[
            {"variable":left,"operator":"is","value":true},
            {"variable":right,"operator":"is","value":true}
        ]})
    };
    serde_json::from_value(json!({"definition":{"version":1,"nodes":[
        {"id":"start","kind":"start"},
        {"id":"decide","kind":"if-else","config":{"conditions":[group]}},
        {"id":"yes","kind":"template-transform","config":{"templateScript":"YES"}},
        {"id":"no","kind":"template-transform","config":{"templateScript":"NO"}},
        {"id":"end","kind":"end"}
    ],"edges":[{"id":"a","source":"start","target":"decide"},
        {"id":"y","source":"decide","source_handle":"true","target":"yes"},
        {"id":"n","source":"decide","source_handle":"false","target":"no"},
        {"id":"ye","source":"yes","target":"end"},{"id":"ne","source":"no","target":"end"}]}}))
    .unwrap()
}

async fn run(
    chat: &ChatService,
    session: &str,
    workflow: WorkflowRequest,
    id: &str,
) -> (Result<String>, Vec<NodeExecutionEvent>) {
    let nodes = Arc::new(Mutex::new(Vec::new()));
    let capture = nodes.clone();
    let result = chat
        .run_workflow(
            workflow,
            id.into(),
            session.into(),
            move |event| {
                if let RuntimeEvent::WorkflowNode { event } = event {
                    capture.lock().unwrap().push(event);
                }
            },
            |_| panic!("pure condition fixture must not request approval"),
        )
        .await;
    let captured = nodes.lock().unwrap().clone();
    (result, captured)
}

fn terminal(db: &Database, id: &str, kind: &str) {
    let store = RunStore::new(db);
    assert_eq!(
        store.get(id).unwrap().unwrap().terminal_kind.as_deref(),
        Some(kind)
    );
    let events = store.events_after(id, None).unwrap();
    let terminals: Vec<_> = events
        .iter()
        .filter(|e| e.event_type == "run_terminal")
        .collect();
    assert_eq!(terminals.len(), 1);
    let mut seen = std::collections::BTreeSet::new();
    for event in events.iter().filter(|e| e.event_type == "workflow_node") {
        let node: NodeExecutionEvent = serde_json::from_value(event.data["event"].clone()).unwrap();
        if node.status.is_terminal() {
            assert!(seen.insert((node.scope, node.node_id)));
            assert!(event.sequence < terminals[0].sequence);
        }
    }
}

#[tokio::test]
async fn h198_truth_tables_select_only_the_actual_branch_in_full_and_target_runs() {
    let (_root, db, chat, session) = workflow_fixture().await;
    for logic in ["or", "and"] {
        for (left, right) in [(false, false), (false, true), (true, false), (true, true)] {
            let expected = match logic {
                "or" => left || right,
                _ => left && right,
            };
            for legacy in [false, true] {
                for targeted in [false, true] {
                    let mut w = request(logic, left, right, legacy);
                    if targeted {
                        w.target_node_id = Some("end".into());
                    }
                    let id = format!("h198-table-{logic}-{left}-{right}-{legacy}-{targeted}");
                    let (result, nodes) = run(&chat, &session, w, &id).await;
                    assert!(result.is_ok(), "{id}: {result:?}");
                    let decision = nodes
                        .iter()
                        .find(|n| {
                            n.node_id == "decide" && n.status == NodeExecutionStatus::Completed
                        })
                        .unwrap();
                    assert_eq!(
                        decision.outputs,
                        Some(json!({"branch":expected.to_string()})),
                        "{id}"
                    );
                    for (node, selected) in [("yes", expected), ("no", !expected)] {
                        assert!(nodes.iter().any(|n| n.node_id == node
                            && n.status
                                == if selected {
                                    NodeExecutionStatus::Completed
                                } else {
                                    NodeExecutionStatus::Skipped
                                }));
                        assert_eq!(
                            nodes
                                .iter()
                                .any(|n| n.node_id == node
                                    && n.status == NodeExecutionStatus::Running),
                            selected
                        );
                    }
                    terminal(&db, &id, "succeeded");
                }
            }
        }
    }
}

#[tokio::test]
async fn h198_invalid_groups_reject_before_any_ancestor_write_or_approval() {
    let (_root, db, chat, session) = workflow_fixture().await;
    let good =
        json!({"id":"true","logic":"and","items":[{"variable":1,"operator":"is","value":1}]});
    let mut invalid = vec![
        json!([]),
        json!(null),
        json!([{"id":"true"}]),
        json!([{"id":"true","items":[]}]),
    ];
    for (field, value) in [
        ("logic", json!("xor")),
        ("logic", json!(null)),
        ("logicalOperator", json!("or")),
        ("conditions", json!([])),
    ] {
        let mut group = good.clone();
        group[field] = value;
        invalid.push(json!([group]));
    }
    for field in ["variable", "operator", "value"] {
        let mut group = good.clone();
        group["items"][0].as_object_mut().unwrap().remove(field);
        invalid.push(json!([group]));
    }
    for (field, value) in [
        ("operator", json!("PRIVATE_OPERATOR_CANARY")),
        ("left", json!(1)),
        ("right", json!(1)),
    ] {
        let mut group = good.clone();
        group["items"][0][field] = value;
        invalid.push(json!([group]));
    }
    for (index, groups) in invalid.into_iter().enumerate() {
        let mut w = request("and", true, true, false);
        w.definition.nodes[1].config["conditions"] = groups;
        w.definition.nodes.push(serde_json::from_value(json!({"id":"write","kind":"tool","config":{"toolName":"write_file","toolArguments":{"path":"must-not-exist.txt","content":"synthetic"}}})).unwrap());
        w.definition.edges[0].source = "write".into();
        w.definition.edges.push(
            serde_json::from_value(json!({"id":"write-entry","source":"start","target":"write"}))
                .unwrap(),
        );
        let id = format!("h198-schema-{index}");
        let (result, nodes) = run(&chat, &session, w, &id).await;
        let error = result.unwrap_err().to_string();
        assert!(
            error.contains("ConditionConfigInvalid")
                || error.contains("conditions must be an array"),
            "{error}"
        );
        assert!(!error.contains("PRIVATE_OPERATOR_CANARY"));
        assert!(nodes.is_empty());
        assert!(!_root.path().join("must-not-exist.txt").exists());
        assert!(RunStore::new(&db)
            .events_after(&id, None)
            .unwrap()
            .iter()
            .all(|e| e.event_type != "tool_started"));
        terminal(&db, &id, "failed");
    }
}

#[tokio::test]
async fn h198_all_comparators_keep_the_existing_typed_and_lossless_semantics() {
    let (_root, db, chat, session) = workflow_fixture().await;
    for (index, (operator, left, right, expected)) in [
        ("is", json!(null), json!(null), true),
        ("is", json!(false), json!(false), true),
        (
            "is",
            json!(9_007_199_254_740_993_u64),
            json!("9007199254740993"),
            true,
        ),
        (
            "is",
            json!(9_007_199_254_740_993_u64),
            json!("9007199254740992"),
            false,
        ),
        (
            "is-not",
            json!({"x":[null,0]}),
            json!({"x":[false,0]}),
            true,
        ),
        ("contains", json!([false, null, 0]), json!(null), true),
        ("not-contains", json!("中文字符"), json!("😀"), true),
        ("starts-with", json!("中文字符"), json!("中文"), true),
        ("ends-with", json!("中文字符"), json!("字符"), true),
        ("empty", json!([]), json!(""), true),
        ("not-empty", json!({"x":false}), json!(""), true),
        ("gt", json!(u64::MAX), json!("18446744073709551614"), true),
        ("gte", json!(-0.5), json!("-0.5"), true),
        ("lt", json!(0), json!("-0.5"), false),
        ("lte", json!(0), json!("0"), true),
    ]
    .into_iter()
    .enumerate()
    {
        let mut w = request("or", false, false, false);
        w.definition.nodes[0].config.insert(
            "inputVariables".into(),
            json!([{"name":"payload","type":"object","required":true}]),
        );
        w.inputs
            .insert("payload".into(), json!({"left":left,"right":right}));
        // One comparison and absent logic retain the established default AND.
        w.definition.nodes[1].config["conditions"] = json!([{"id":"true","items":[{"variable":"{{#start.payload.left#}}","operator":operator,"value":"{{#start.payload.right#}}"}]}]);
        let id = format!("h198-operators-{index}");
        let (result, nodes) = run(&chat, &session, w, &id).await;
        assert!(result.is_ok(), "{operator}: {result:?}");
        assert!(nodes
            .iter()
            .any(|n| n.node_id == "decide"
                && n.outputs == Some(json!({"branch":expected.to_string()}))));
        terminal(&db, &id, "succeeded");
    }
}

#[tokio::test]
async fn h198_comparison_and_reference_errors_never_become_choices_or_reinterpret_inserted_text() {
    let (_root, db, chat, session) = workflow_fixture().await;
    for (index, (logic, decisive, invalid)) in [
        ("or", true, false),
        ("or", true, true),
        ("and", false, true),
    ]
    .into_iter()
    .enumerate()
    {
        let mut w = request(logic, decisive, false, false);
        w.definition.nodes[0].config.insert(
            "inputVariables".into(),
            json!([{"name":"payload","type":"object","required":true}]),
        );
        w.inputs.insert(
            "payload".into(),
            json!({"a":"{{#missing.private_canary#}}","b":"{{#missing.private_canary#}}"}),
        );
        w.definition.nodes[1].config["conditions"][0]["items"][1] = if invalid {
            json!({"variable":{},"operator":"gt","value":"PRIVATE_VALUE_CANARY"})
        } else {
            json!({"variable":"{{#start.payload.a#}}","operator":"is","value":"{{#start.payload.b#}}"})
        };
        let id = format!("h198-eager-{index}");
        let (result, nodes) = run(&chat, &session, w, &id).await;
        assert_eq!(result.is_err(), invalid, "{result:?}");
        if invalid {
            assert!(!result
                .unwrap_err()
                .to_string()
                .contains("PRIVATE_VALUE_CANARY"));
            assert!(nodes.iter().any(|n| n.node_id == "decide"
                && n.status == NodeExecutionStatus::Failed
                && n.outputs.is_none()));
            assert!(nodes
                .iter()
                .filter(|n| ["yes", "no", "end"].contains(&n.node_id.as_str()))
                .all(|n| n.status == NodeExecutionStatus::Skipped));
        }
        terminal(&db, &id, if invalid { "failed" } else { "succeeded" });
    }
    let mut w = request("or", true, false, false);
    w.definition.nodes[1].config["conditions"][0]["items"][1]["variable"] =
        json!("{{#start.absent#}}");
    let (result, nodes) = run(&chat, &session, w, "h198-undeclared-reference").await;
    assert!(result.is_err());
    assert!(
        nodes.is_empty(),
        "undeclared top-level fields fail graph admission"
    );
    terminal(&db, "h198-undeclared-reference", "failed");
    let mut w = request("or", true, false, false);
    w.definition.nodes[0].config.insert(
        "inputVariables".into(),
        json!([{"name":"payload","type":"object","required":true}]),
    );
    w.inputs.insert("payload".into(), json!({}));
    w.definition.nodes[1].config["conditions"][0]["items"][1]["variable"] =
        json!("{{#start.payload.absent#}}");
    let (result, nodes) = run(&chat, &session, w, "h198-missing-reference").await;
    assert!(result.is_err());
    assert!(nodes.iter().any(|n| n.node_id == "decide"
        && n.status == NodeExecutionStatus::Failed
        && n.outputs.is_none()));
    terminal(&db, "h198-missing-reference", "failed");
}

#[tokio::test]
async fn h198_first_match_and_iteration_items_keep_the_existing_scope_and_branch_ids() {
    let (_root, db, chat, session) = workflow_fixture().await;
    for (index, values) in [
        [true, true, true],
        [false, true, true],
        [false, false, true],
        [false, false, false],
    ]
    .into_iter()
    .enumerate()
    {
        let groups:Vec<_>=values.iter().enumerate().map(|(i,v)|json!({"id":format!("case{i}"),"logic":"or","items":[{"variable":false,"operator":"is","value":true},{"variable":v,"operator":"is","value":true}]})).collect();
        let w=serde_json::from_value(json!({"definition":{"version":1,"nodes":[
            {"id":"start","kind":"start"},{"id":"decide","kind":"if-else","config":{"conditions":groups}},
            {"id":"end","kind":"end"}
        ],"edges":[{"id":"a","source":"start","target":"decide"},
            {"id":"c0","source":"decide","source_handle":"case0","target":"end"},
            {"id":"c1","source":"decide","source_handle":"case1","target":"end"},
            {"id":"c2","source":"decide","source_handle":"case2","target":"end"}]}})).unwrap();
        let id = format!("h198-first-match-{index}");
        let (result, nodes) = run(&chat, &session, w, &id).await;
        let matched = index < 3;
        assert_eq!(result.is_ok(), matched, "{result:?}");
        let branch = if matched {
            format!("case{index}")
        } else {
            "false".into()
        };
        assert!(nodes
            .iter()
            .any(|n| n.node_id == "decide" && n.outputs == Some(json!({"branch":branch}))));
        // H-199 still prevents a multi-case false edge. Correct truth must not
        // invent a matching case, or turn the unselected end into a success.
        terminal(&db, &id, if matched { "succeeded" } else { "failed" });
    }
    let w=serde_json::from_value(json!({"definition":{"version":1,"nodes":[
        {"id":"start","kind":"start","config":{"inputVariables":[{"name":"items","type":"array","default":[{"a":false,"b":false,"label":"FIRST"},{"a":false,"b":true,"label":"SECOND"}]}]}},
        {"id":"iterate","kind":"iteration","config":{"inputVariable":"{{#start.items#}}","outputVariable":"{{#item-end.report#}}","parallel":false,"maxConcurrency":1,"errorHandling":"terminate"}},
        {"id":"entry","kind":"iteration-start"},
        {"id":"decide","kind":"if-else","config":{"conditions":[{"id":"true","logic":"or","items":[{"variable":"{{#entry.item.a#}}","operator":"is","value":true},{"variable":"{{#entry.item.b#}}","operator":"is","value":true}]}]}},
        {"id":"item-end","kind":"end","config":{"outputVariables":[{"name":"report","type":"string","value":"{{#entry.item.label#}}"}]}},
        {"id":"end","kind":"end","config":{"outputVariables":[{"name":"report","type":"array","value":"{{#iterate.output#}}"}]}}
    ],"edges":[{"id":"a","source":"start","target":"iterate"},{"id":"b","source":"iterate","source_handle":"item","target":"entry"},
        {"id":"c","source":"entry","target":"decide"},{"id":"it","source":"decide","source_handle":"true","target":"item-end"},
        {"id":"if","source":"decide","source_handle":"false","target":"item-end"},{"id":"d","source":"iterate","source_handle":"done","target":"end"}]}})).unwrap();
    let (result, nodes) = run(&chat, &session, w, "h198-items").await;
    assert!(result.is_ok(), "{result:?}");
    assert!(nodes
        .iter()
        .any(|n| n.node_id == "end" && n.outputs == Some(json!({"report":["FIRST","SECOND"]}))));
    assert_eq!(
        nodes
            .iter()
            .filter(|n| n.node_id == "decide"
                && !n.scope.is_empty()
                && n.status == NodeExecutionStatus::Completed)
            .count(),
        2
    );
    let decisions: Vec<_> = nodes
        .iter()
        .filter(|n| n.node_id == "decide" && n.status == NodeExecutionStatus::Completed)
        .map(|n| n.outputs.clone().unwrap())
        .collect();
    assert_eq!(
        decisions,
        vec![json!({"branch":"false"}), json!({"branch":"true"})]
    );
    terminal(&db, "h198-items", "succeeded");
}

#[derive(Default)]
struct ChoiceBroker {
    calls: Mutex<Vec<String>>,
    waiting: AtomicUsize,
    cancel: bool,
    fail: bool,
}

#[async_trait::async_trait]
impl deepagent_builtins::http_request::HttpRequestClient for ChoiceBroker {
    fn validate(&self, request: &deepagent_builtins::http_request::HttpRequest) -> Result<()> {
        request.validate()
    }
    async fn request(
        &self,
        request: deepagent_builtins::http_request::HttpRequest,
        context: deepagent_tools::ToolExecutionContext,
    ) -> Result<Value> {
        self.calls.lock().unwrap().push(request.url);
        if self.cancel {
            self.waiting.fetch_add(1, Ordering::SeqCst);
            context.cancelled().await;
            self.waiting.fetch_sub(1, Ordering::SeqCst);
            return Err(deepagent_core::error::CoreError::other(
                "synthetic cancelled choice",
            ));
        }
        if self.fail {
            return Err(deepagent_core::error::CoreError::other(
                "synthetic chosen branch failure",
            ));
        }
        Ok(json!({"body":"CHOICE_RESULT","status_code":200,"headers":{}}))
    }
}

#[tokio::test]
async fn h198_only_the_chosen_effect_is_approved_and_reopen_never_redispatches_it() {
    for case in ["false", "true", "denied", "cancelled", "failed"] {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("conditions.db");
        let db = Arc::new(Database::open(&path).unwrap());
        let (_root, db, chat, session) = workflow_fixture_with_database(root, db).await;
        let broker = Arc::new(ChoiceBroker {
            cancel: case == "cancelled",
            fail: case == "failed",
            ..Default::default()
        });
        let chat = chat.with_workflow_http_client(broker.clone());
        let mut w = request("or", case == "true", false, false);
        for (index, branch) in [(2, "yes"), (3, "no")] {
            w.definition.nodes[index].kind = "http-request".into();
            w.definition.nodes[index].config=serde_json::from_value(json!({"httpUrl":format!("https://synthetic.fixture.invalid/{branch}"),"httpMethod":"POST","httpBody":"SYNTHETIC_ONLY","httpTimeout":7,"httpRetryCount":0})).unwrap();
        }
        let worker_chat = chat.clone();
        let responder = chat.clone();
        let approvals = Arc::new(AtomicUsize::new(0));
        let approved = approvals.clone();
        let nodes = Arc::new(Mutex::new(Vec::new()));
        let capture = nodes.clone();
        let id = format!("h198-effect-{case}");
        let worker_id = id.clone();
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
                                "synthetic chosen effect",
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
            matches!(case, "false" | "true" | "cancelled"),
            "{case}: {result:?}"
        );
        assert_eq!(approvals.load(Ordering::SeqCst), 1);
        let expected_calls = if case == "denied" {
            vec![]
        } else {
            vec![format!(
                "https://synthetic.fixture.invalid/{}",
                if case == "true" { "yes" } else { "no" }
            )]
        };
        assert_eq!(*broker.calls.lock().unwrap(), expected_calls);
        assert_eq!(broker.waiting.load(Ordering::SeqCst), 0);
        let nodes = nodes.lock().unwrap().clone();
        assert!(nodes.iter().any(|n| n.node_id == "decide"
            && n.outputs == Some(json!({"branch":(case=="true").to_string()}))));
        let excluded = if case == "true" { "no" } else { "yes" };
        assert!(nodes
            .iter()
            .filter(|n| n.node_id == excluded)
            .all(|n| n.status == NodeExecutionStatus::Skipped));
        let kind = match case {
            "false" | "true" => "succeeded",
            "cancelled" => "cancelled",
            _ => "failed",
        };
        terminal(&db, &id, kind);
        let before = RunStore::new(&db).events_after(&id, None).unwrap();
        let persisted: Vec<NodeExecutionEvent> = before
            .iter()
            .filter(|e| e.event_type == "workflow_node")
            .map(|e| serde_json::from_value(e.data["event"].clone()).unwrap())
            .collect();
        assert_eq!(persisted, nodes);
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
        terminal(&reopened, &id, kind);
        assert_eq!(
            *broker.calls.lock().unwrap(),
            expected_calls,
            "replay is not redispatch or rollback"
        );
    }
}

#[tokio::test]
#[ignore = "requires local Node and desktop dependencies; real condition serializer/Kernel/events consumer"]
async fn h198_desktop_truth_tables_reach_the_real_kernel_and_production_consumer() {
    let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../apps/desktop/scripts/audit-workflow-contract.cjs");
    let emitted = std::process::Command::new("node")
        .arg(&script)
        .arg("--emit-condition-truth-fixtures")
        .output()
        .unwrap();
    assert!(
        emitted.status.success(),
        "{}",
        String::from_utf8_lossy(&emitted.stderr)
    );
    let fixtures: Vec<Value> = serde_json::from_slice(&emitted.stdout).unwrap();
    assert_eq!(fixtures.len(), 8);
    for fixture in fixtures {
        let name = fixture["name"].as_str().unwrap();
        let parts: Vec<_> = name.split('-').collect();
        let (logic, a, b) = (parts[2], parts[3] == "true", parts[4] == "true");
        let expected = if logic == "or" { a || b } else { a && b };
        let w: WorkflowRequest = serde_json::from_value(fixture["workflow"].clone()).unwrap();
        let (_root, db, chat, session) = workflow_fixture().await;
        let (result, nodes) = run(&chat, &session, w, "h198-desktop").await;
        assert!(result.is_ok(), "{name}: {result:?}");
        assert!(nodes
            .iter()
            .any(|n| n.node_id == "decide"
                && n.outputs == Some(json!({"branch":expected.to_string()}))));
        terminal(&db, "h198-desktop", "succeeded");
        let replay = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(
            replay.path(),
            serde_json::to_vec(&json!({"name":name,"events":nodes,"expected":expected})).unwrap(),
        )
        .unwrap();
        let consumed = std::process::Command::new("node")
            .arg(&script)
            .arg("--consume-condition-events")
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
