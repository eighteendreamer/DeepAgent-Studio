//! H-183: published aggregation bindings through the real Kernel and RunStore.
use super::*;
use deepagent_runtime::workflow::{compile, NodeExecutionEvent};
use serde_json::{json, Value};

fn request(kind: &str, bindings: Value, payload: Value) -> WorkflowRequest {
    serde_json::from_value(json!({
        "definition":{"version":1,"nodes":[
            {"id":"start","kind":"start","config":{"inputVariables":[{"name":"payload","type":"object"}]}},
            {"id":"aggregate","kind":"variable-aggregator","config":{
                "aggregatorOutputType":kind,"aggregatorVariables":bindings
            }},
            {"id":"end","kind":"end","config":{"outputVariables":[
                {"name":"report","type":kind,"value":"{{#aggregate.output#}}"}
            ]}}
        ],"edges":[{"id":"a","source":"start","target":"aggregate"},
            {"id":"b","source":"aggregate","target":"end"}]},
        "inputs":{"payload":payload}
    })).unwrap()
}

async fn run(
    chat: &ChatService,
    session: String,
    workflow: WorkflowRequest,
    id: &str,
) -> (
    deepagent_core::error::Result<String>,
    Vec<NodeExecutionEvent>,
) {
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
            |_| panic!("local aggregation never requests approval"),
        )
        .await;
    let captured = events.lock().unwrap().clone();
    (result, captured)
}

fn completed<'a>(events: &'a [NodeExecutionEvent], id: &str) -> &'a NodeExecutionEvent {
    let terminal: Vec<_> = events
        .iter()
        .filter(|e| e.node_id == id && e.status.is_terminal())
        .collect();
    assert_eq!(terminal.len(), 1, "one terminal for {id}");
    assert_eq!(terminal[0].status, NodeExecutionStatus::Completed);
    terminal[0]
}

#[tokio::test]
async fn h183_all_four_output_types_reach_downstream_without_reinterpreting_data() {
    for (index, (kind, payload, expected)) in [
        (
            "string",
            json!({"a":"{{#start.payload.hidden#}}", "b":"second", "hidden":"not substituted"}),
            json!("{{#start.payload.hidden#}}"),
        ),
        ("number", json!({"a":0,"b":12.5}), json!(0)),
        (
            "object",
            json!({"a":{"nested":[false,null]},"b":{}}),
            json!({"nested":[false,null]}),
        ),
        (
            "array",
            json!({"a":[1,null],"b":{"text":"{{unknown}}"}}),
            json!([[1,null],{"text":"{{unknown}}"}]),
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let (_root, db, chat, session) = workflow_fixture().await;
        let id = format!("h183-types-{index}");
        let workflow = request(
            kind,
            json!(["{{#start.payload.a#}}", "{{#start.payload.b#}}"]),
            payload,
        );
        let (result, events) = run(&chat, session, workflow, &id).await;
        assert!(result.is_ok(), "{kind}: {result:?}");
        let aggregate = completed(&events, "aggregate");
        assert_eq!(aggregate.outputs, Some(json!({"output":expected})));
        assert_eq!(
            completed(&events, "end").outputs,
            Some(json!({"report":expected}))
        );
        let selected = if kind == "array" {
            json!([["start", "payload", "a"], ["start", "payload", "b"]])
        } else {
            json!([["start", "payload", "a"]])
        };
        assert_eq!(
            aggregate.updates["aggregation"],
            json!({
                "version":1,"outputType":kind,"configuredCount":2,
                "selectedCount":selected.as_array().unwrap().len(),
                "selectedSources":selected,"skippedSources":[]
            })
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

#[tokio::test]
async fn h183_array_preserves_binding_order_null_false_and_empty_values() {
    let (_root, _db, chat, session) = workflow_fixture().await;
    let workflow = request(
        "array",
        json!([
            "{{#start.payload.z#}}",
            "{{#start.payload.a#}}",
            "{{#start.payload.nil#}}",
            "{{#start.payload.flag#}}",
            "{{#start.payload.empty#}}"
        ]),
        json!({"a":[],"z":"","nil":null,"flag":false,"empty":{}}),
    );
    let (result, events) = run(&chat, session, workflow, "h183-order").await;
    assert!(result.is_ok(), "{result:?}");
    assert_eq!(
        completed(&events, "aggregate").outputs,
        Some(json!({"output":["",[],null,false,{}]}))
    );
}

#[test]
fn h183_admission_rejects_empty_duplicate_legacy_unknown_and_non_reference_bindings() {
    for config in [
        json!({}),
        json!({"aggregatorOutputType":"array"}),
        json!({"aggregatorOutputType":"array","aggregatorVariables":[]}),
        json!({"aggregatorOutputType":"boolean","aggregatorVariables":["{{#start.payload#}}"]}),
        json!({"aggregatorVariables":["{{#start.payload#}}"]}),
        json!({"aggregatorOutputType":"array","variables":["{{#start.payload#}}"]}),
        json!({"aggregatorOutputType":"array","aggregatorVariables":["{{#start.payload#}}"],"typo":true}),
        json!({"aggregatorOutputType":"array","aggregatorVariables":["{{#start.payload#}}","{{#start.payload#}}"]}),
        json!({"aggregatorOutputType":"array","aggregatorVariables":[null]}),
        json!({"aggregatorOutputType":"array","aggregatorVariables":[["start","payload"]]}),
        json!({"aggregatorOutputType":"array","aggregatorVariables":["literal"]}),
        json!({"aggregatorOutputType":"array","aggregatorVariables":["prefix {{#start.payload#}}"]}),
        json!({"aggregatorOutputType":"array","aggregatorVariables":["{{#start.payload#}}{{#start.payload#}}"]}),
        json!({"aggregatorOutputType":"array","aggregatorVariables":["{{#missing.output#}}"]}),
        json!({"aggregatorOutputType":"array","aggregatorVariables":["{{#end.report#}}"]}),
        json!({"aggregatorOutputType":"array","aggregatorVariables":["{{#start.unknown#}}"]}),
    ] {
        let mut workflow = request("array", json!(["{{#start.payload#}}"]), json!({}));
        workflow.definition.nodes[1].config = serde_json::from_value(config.clone()).unwrap();
        assert!(
            compile(workflow.definition).is_err(),
            "must reject {config}"
        );
    }
    let workflow = request("number", json!(["{{#start.payload#}}"]), json!({}));
    assert!(
        compile(workflow.definition).is_err(),
        "statically declared object is not number"
    );
}

#[tokio::test]
async fn h183_invalid_config_fails_before_node_or_tool_dispatch() {
    let (_root, db, chat, session) = workflow_fixture().await;
    let mut workflow = request("array", json!([]), json!({}));
    workflow.definition.nodes[0].kind = "tool".into();
    workflow.definition.nodes[0].config =
        serde_json::from_value(json!({"toolName":"must_not_dispatch","toolArguments":{}})).unwrap();
    let (result, events) = run(&chat, session, workflow, "h183-admission").await;
    assert!(result.is_err());
    assert!(events.is_empty(), "admission precedes all execution");
    assert_eq!(
        RunStore::new(&db)
            .get("h183-admission")
            .unwrap()
            .unwrap()
            .terminal_kind
            .as_deref(),
        Some("failed")
    );
    assert!(
        deepagent_persistence::run_control::RunControlStore::new(&db)
            .list_actions("h183-admission")
            .unwrap()
            .is_empty()
    );
}

fn branching(kind: &str, skip_both: bool, failed: bool) -> WorkflowRequest {
    let mut workflow = request(
        kind,
        json!(["{{#left.answer#}}", "{{#right.answer#}}"]),
        json!({}),
    );
    workflow.definition.nodes.splice(1..1, serde_json::from_value::<Vec<_>>(json!([
        {"id":"choose","kind":"if-else","config":{"conditions":[{"id":"true","items":[{"variable":"x","operator":"is","value":"y"}]}]}},
        {"id":"left","kind":"answer","config":{"answerTemplate":"left"}},
        {"id":"right","kind":"answer","config":{"answerTemplate":if failed {"{{#start.payload.missing#}}"} else {"right"}}}
    ])).unwrap());
    workflow.definition.edges = serde_json::from_value(json!([
        {"id":"a","source":"start","target":"choose"},
        {"id":"b","source":"choose","source_handle":"true","target":"left"},
        {"id":"c","source":"choose","source_handle":if skip_both {"true"} else {"false"},"target":"right"},
        {"id":"d","source":"left","target":"aggregate"},
        {"id":"e","source":"right","target":"aggregate"},
        {"id":"f","source":"start","target":"aggregate"},
        {"id":"g","source":"aggregate","target":"end"}
    ])).unwrap();
    workflow
}

#[tokio::test]
async fn h183_only_explicitly_skipped_branches_are_ignored_and_sources_are_auditable() {
    for kind in ["array", "string"] {
        let (_root, _db, chat, session) = workflow_fixture().await;
        let (result, events) =
            run(&chat, session, branching(kind, false, false), "h183-branch").await;
        assert!(result.is_ok(), "{result:?}");
        assert!(events
            .iter()
            .any(|e| e.node_id == "left" && e.status == NodeExecutionStatus::Skipped));
        let aggregate = completed(&events, "aggregate");
        assert_eq!(
            aggregate.outputs,
            Some(json!({"output":if kind == "array" {json!(["right"])} else {json!("right")}}))
        );
        assert_eq!(
            aggregate.updates["aggregation"]["selectedSources"],
            json!([["right", "answer"]])
        );
        assert_eq!(
            aggregate.updates["aggregation"]["skippedSources"],
            json!([["left", "answer"]])
        );
    }
}

#[tokio::test]
async fn h183_no_available_source_and_failed_upstream_never_report_success() {
    for (index, workflow) in [
        branching("array", true, false),
        branching("string", false, true),
    ]
    .into_iter()
    .enumerate()
    {
        let (_root, db, chat, session) = workflow_fixture().await;
        let id = format!("h183-branch-failure-{index}");
        let (result, events) = run(&chat, session, workflow, &id).await;
        assert!(result.is_err(), "{result:?}");
        assert!(events
            .iter()
            .all(|e| !matches!(e.node_id.as_str(), "aggregate" | "end")
                || e.status != NodeExecutionStatus::Completed));
        assert_eq!(
            RunStore::new(&db)
                .get(&id)
                .unwrap()
                .unwrap()
                .terminal_kind
                .as_deref(),
            Some("failed")
        );
    }
}

#[tokio::test]
async fn h183_missing_or_wrong_typed_completed_source_is_not_a_skipped_branch() {
    for (index, payload) in [
        json!({"b":"fallback"}),
        json!({"a":null,"b":"fallback"}),
        json!({"a":42,"b":"fallback"}),
    ]
    .into_iter()
    .enumerate()
    {
        let (_root, db, chat, session) = workflow_fixture().await;
        let id = format!("h183-invalid-value-{index}");
        let workflow = request(
            "string",
            json!(["{{#start.payload.a#}}", "{{#start.payload.b#}}"]),
            payload,
        );
        let (result, events) = run(&chat, session, workflow, &id).await;
        assert!(
            result.is_err(),
            "must not silently select fallback: {result:?}"
        );
        assert!(events
            .iter()
            .any(|e| e.node_id == "aggregate" && e.status == NodeExecutionStatus::Failed));
        let end: Vec<_> = events.iter().filter(|e| e.node_id == "end").collect();
        assert_eq!(end.len(), 1, "failed aggregate must not dispatch the end");
        assert_eq!(end[0].status, NodeExecutionStatus::Skipped);
        assert_eq!(
            end[0].error.as_deref(),
            Some("workflow stopped before node execution")
        );
        assert_eq!(
            RunStore::new(&db)
                .get(&id)
                .unwrap()
                .unwrap()
                .terminal_kind
                .as_deref(),
            Some("failed")
        );
    }
}

#[tokio::test]
async fn h183_combined_array_expansion_uses_existing_one_mib_value_budget() {
    let (_root, _db, chat, session) = workflow_fixture().await;
    let workflow = request(
        "array",
        json!(["{{#start.payload.text#}}", "{{#start.payload#}}"]),
        json!({"text":"x".repeat(600_000)}),
    );
    let (result, events) = run(&chat, session, workflow, "h183-budget").await;
    assert!(result.is_err());
    assert!(events.iter().any(|e| e.node_id == "aggregate"
        && e.status == NodeExecutionStatus::Failed
        && e.error.as_deref().is_some_and(|e| e.contains("1 MiB"))));
}

#[tokio::test]
async fn h183_iteration_items_share_compiled_plan_but_keep_scoped_values_and_parent_status() {
    let (_root, _db, chat, session) = workflow_fixture().await;
    let workflow = serde_json::from_value(json!({
        "definition":{"version":1,"nodes":[
            {"id":"start","kind":"start","config":{"inputVariables":[{"name":"payload","type":"object"}]}},
            {"id":"iterate","kind":"iteration","config":{"inputVariable":"{{#start.payload.items#}}",
                "outputVariable":"{{#aggregate.output#}}","parallel":true,"maxConcurrency":2}},
            {"id":"entry","kind":"iteration-start"},
            {"id":"aggregate","kind":"variable-aggregator","config":{"aggregatorOutputType":"array",
                "aggregatorVariables":["{{#entry.item#}}","{{#start.payload.suffix#}}"]}},
            {"id":"end","kind":"end","config":{"outputVariables":[
                {"name":"report","type":"array","value":"{{#iterate.output#}}"}]}}
        ],"edges":[{"id":"a","source":"start","target":"iterate"},
            {"id":"b","source":"iterate","source_handle":"item","target":"entry"},
            {"id":"c","source":"entry","target":"aggregate"},
            {"id":"d","source":"iterate","source_handle":"done","target":"end"}]},
        "inputs":{"payload":{"items":[1,2,3],"suffix":"parent"}}
    })).unwrap();
    let (result, events) = run(&chat, session, workflow, "h183-item-scope").await;
    assert!(result.is_ok(), "{result:?}");
    assert_eq!(
        completed(&events, "end").outputs,
        Some(json!({"report":[[1,"parent"],[2,"parent"],[3,"parent"]]}))
    );
    let completed_items: Vec<_> = events
        .iter()
        .filter(|e| e.node_id == "aggregate" && e.status == NodeExecutionStatus::Completed)
        .collect();
    assert_eq!(completed_items.len(), 3);
    for index in 0..3 {
        let event = completed_items
            .iter()
            .find(|e| e.scope == ["iterate".to_owned(), index.to_string()])
            .unwrap();
        assert_eq!(event.outputs, Some(json!({"output":[index+1,"parent"]})));
        assert_eq!(event.updates["aggregation"]["selectedCount"], 2);
    }
}

#[tokio::test]
async fn h183_sqlite_reopen_preserves_values_provenance_cursor_and_failure_without_redispatch() {
    for (index, payload) in [json!({"a":[1,null],"b":false}), json!({})]
        .into_iter()
        .enumerate()
    {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("aggregation.db");
        let db = Arc::new(Database::open(&path).unwrap());
        let (_root, db, chat, session) = workflow_fixture_with_database(root, db).await;
        let id = format!("h183-durable-{index}");
        let workflow = request(
            "array",
            json!(["{{#start.payload.a#}}", "{{#start.payload.b#}}"]),
            payload,
        );
        let (result, events) = run(&chat, session, workflow, &id).await;
        assert_eq!(result.is_ok(), index == 0);
        let before = RunStore::new(&db).events_after(&id, None).unwrap();
        let stored: Vec<NodeExecutionEvent> = before
            .iter()
            .filter(|e| e.event_type == "workflow_node")
            .map(|e| serde_json::from_value(e.data["event"].clone()).unwrap())
            .collect();
        assert_eq!(
            stored, events,
            "live and persisted payload/metadata identical"
        );
        assert_eq!(
            before
                .iter()
                .filter(|e| e.event_type == "run_terminal")
                .count(),
            1
        );
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
        assert_eq!(
            store.get(&id).unwrap().unwrap().terminal_kind.as_deref(),
            Some(if index == 0 { "succeeded" } else { "failed" })
        );
        assert!(
            deepagent_persistence::run_control::RunControlStore::new(&reopened)
                .list_actions(&id)
                .unwrap()
                .is_empty()
        );
    }
}

#[tokio::test]
#[ignore = "requires local Node and desktop dependencies; production serializer to Kernel"]
async fn h183_desktop_serialized_aggregators_execute_all_four_output_types() {
    let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../apps/desktop/scripts/audit-workflow-contract.cjs");
    let output = std::process::Command::new("node")
        .arg(script)
        .arg("--emit-aggregator-fixtures")
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
        let workflow: WorkflowRequest =
            serde_json::from_value(fixture["workflow"].clone()).unwrap();
        let kind = workflow.definition.nodes[1].config["aggregatorOutputType"]
            .as_str()
            .unwrap()
            .to_owned();
        let expected = match kind.as_str() {
            "array" => json!(["FIRST", "SECOND"]),
            "string" => json!("FIRST"),
            "number" => json!(7),
            "object" => json!({"k":1}),
            _ => panic!("unexpected type"),
        };
        let (_root, _db, chat, session) = workflow_fixture().await;
        let (result, events) = run(&chat, session, workflow, &format!("h183-desktop-{kind}")).await;
        assert!(result.is_ok(), "{result:?}");
        assert_eq!(
            completed(&events, "aggregate").outputs,
            Some(json!({"output":expected}))
        );
        assert_eq!(
            completed(&events, "end").outputs,
            Some(json!({"report":expected}))
        );
    }
}
