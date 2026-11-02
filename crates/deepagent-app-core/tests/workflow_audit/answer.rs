//! H-182: real version-1 answer contracts, Kernel events and temporary SQLite.
use super::*;
use deepagent_runtime::workflow::{compile, NodeExecutionEvent};
use serde_json::{json, Value};

fn request(config: Value, input: Value) -> WorkflowRequest {
    serde_json::from_value(json!({
        "definition":{"version":1,"nodes":[
            {"id":"start","kind":"start","config":{
                "inputVariables":[{"name":"payload","type":"object"}]
            }},
            {"id":"answer","kind":"answer","config":config},
            {"id":"end","kind":"end","config":{"outputVariables":[
                {"name":"report","type":"string","value":"{{#answer.answer#}}"}
            ]}}
        ],"edges":[
            {"id":"a","source":"start","target":"answer"},
            {"id":"b","source":"answer","target":"end"}
        ]},"inputs":{"payload":input}
    }))
    .unwrap()
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
            |_| panic!("local answer must never request approval"),
        )
        .await;
    let captured = events.lock().unwrap().clone();
    (result, captured)
}

fn completed<'a>(events: &'a [NodeExecutionEvent], id: &str) -> &'a Value {
    let terminal: Vec<_> = events
        .iter()
        .filter(|e| e.node_id == id && e.status.is_terminal())
        .collect();
    assert_eq!(terminal.len(), 1, "exactly one terminal for {id}");
    assert_eq!(terminal[0].status, NodeExecutionStatus::Completed);
    terminal[0]
        .outputs
        .as_ref()
        .expect("actual payload required")
}

#[tokio::test]
async fn h182_plain_text_and_aliases_render_json_and_map_to_end_once() {
    for (index, (config, expected)) in [
        (json!({"answerTemplate":"中文 plain 🦀"}), "中文 plain 🦀"),
        (
            json!({"answerTemplate":"{{n}}|{{b}}|{{o}}|{{a}}|{{nil}}|{{#start.payload.name#}}",
            "answerVariables":[
                {"name":"n","value":7}, {"name":"b","value":false},
                {"name":"o","value":{"k":2}}, {"name":"a","value":[1,true]},
                {"name":"nil","value":null}
            ]}),
            "7|false|{\"k\":2}|[1,true]|null|Eighteen",
        ),
        (
            json!({"answerTemplate":"{{greeting}}!", "answerVariables":[
                {"name":"greeting","value":"{{#start.payload.name#}}"}
            ]}),
            "Eighteen!",
        ),
        (
            json!({"answerTemplate":"{{#start.payload#}}", "answerVariables":[]}),
            "{\"name\":\"Eighteen\"}",
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let (_root, db, chat, session) = workflow_fixture().await;
        let id = format!("h182-render-{index}");
        let (result, events) = run(
            &chat,
            session,
            request(config, json!({"name":"Eighteen"})),
            &id,
        )
        .await;
        assert!(result.is_ok(), "{result:?}");
        assert_eq!(completed(&events, "answer"), &json!({"answer":expected}));
        assert_eq!(completed(&events, "end"), &json!({"report":expected}));
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
async fn h182_inserted_reference_and_template_markers_remain_literal_data() {
    for template in ["{{#start.payload.text#}}", "{{alias}}"] {
        let (_root, _db, chat, session) = workflow_fixture().await;
        let literal = "{{#start.payload.hidden#}} {{unknown}} {% forbidden %}";
        let config = json!({"answerTemplate":template,"answerVariables":[
            {"name":"alias","value":"{{#start.payload.text#}}"}
        ]});
        let (result, events) = run(
            &chat,
            session,
            request(
                config,
                json!({"text":literal,"hidden":"MUST_NOT_SUBSTITUTE"}),
            ),
            "h182-literal",
        )
        .await;
        assert!(
            result.is_ok(),
            "inserted text must not be parsed again: {result:?}"
        );
        assert_eq!(completed(&events, "answer"), &json!({"answer":literal}));
    }
}

#[test]
fn h182_admission_rejects_invalid_config_syntax_aliases_and_references() {
    for config in [
        json!({}),
        json!({"answerTemplate":null}),
        json!({"answerTemplate":7}),
        json!({"answerTemplate":""}),
        json!({"answerTemplate":" \n\t"}),
        json!({"answerTemplate":"{{missing}}"}),
        json!({"answerTemplate":"{{#start.payload#}"}),
        json!({"answerTemplate":"{% if x %}yes{% endif %}"}),
        json!({"answerTemplate":"{{x | upper}}", "answerVariables":[{"name":"x","value":"x"}]}),
        json!({"text":"legacy", "variables":{}}),
        json!({"answerTemplate":"x", "text":"ambiguous"}),
        json!({"answerTemplate":"x", "unknown":"ignored before"}),
        json!({"answerTemplate":"x", "answerVariables":{}}),
        json!({"answerTemplate":"x", "answerVariables":[{"name":"x"}]}),
        json!({"answerTemplate":"x", "answerVariables":[{"name":"constructor","value":1}]}),
        json!({"answerTemplate":"x", "answerVariables":[{"name":"x","value":1},{"name":"x","value":2}]}),
        json!({"answerTemplate":"x", "answerVariables":[{"name":"x","value":1,"typo":true}]}),
        json!({"answerTemplate":"{{#missing.text#}}"}),
        json!({"answerTemplate":"{{#end.report#}}"}),
        json!({"answerTemplate":"{{#start.unknown#}}"}),
    ] {
        let workflow = request(config.clone(), json!({}));
        assert!(
            compile(workflow.definition).is_err(),
            "must reject before execution: {config}"
        );
    }
}

#[tokio::test]
async fn h182_invalid_answer_rejects_before_any_node_or_tool_dispatch() {
    let (_root, db, chat, session) = workflow_fixture().await;
    let mut workflow = request(json!({"answerTemplate":"{{missing}}"}), json!({}));
    workflow.definition.nodes[0].kind = "tool".into();
    workflow.definition.nodes[0].config =
        serde_json::from_value(json!({"toolName":"must_not_dispatch","toolArguments":{}})).unwrap();
    let (result, events) = run(&chat, session, workflow, "h182-invalid").await;
    assert!(result.is_err());
    assert!(
        events.is_empty(),
        "admission failure must precede tool/start execution: {events:?}"
    );
    let store = RunStore::new(&db);
    assert_eq!(
        store
            .get("h182-invalid")
            .unwrap()
            .unwrap()
            .terminal_kind
            .as_deref(),
        Some("failed")
    );
    assert!(
        deepagent_persistence::run_control::RunControlStore::new(&db)
            .list_actions("h182-invalid")
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn h182_empty_render_or_missing_nested_value_never_completes_answer_or_end() {
    for input in [json!({"text":" \n\t"}), json!({})] {
        let (_root, db, chat, session) = workflow_fixture().await;
        let workflow = request(json!({"answerTemplate":"{{#start.payload.text#}}"}), input);
        let (result, events) = run(&chat, session, workflow, "h182-empty-render").await;
        assert!(result.is_err(), "no empty success: {result:?}");
        assert!(events
            .iter()
            .any(|e| e.node_id == "answer" && e.status == NodeExecutionStatus::Failed));
        assert!(events
            .iter()
            .all(|e| !matches!(e.node_id.as_str(), "answer" | "end")
                || e.status != NodeExecutionStatus::Completed));
        assert_eq!(
            RunStore::new(&db)
                .get("h182-empty-render")
                .unwrap()
                .unwrap()
                .terminal_kind
                .as_deref(),
            Some("failed")
        );
    }
}

#[tokio::test]
async fn h182_expansion_and_json_escaping_respect_the_existing_output_budget() {
    for (index, text) in ["x".repeat(600_000), format!("{}x", "\n".repeat(300_000))]
        .into_iter()
        .enumerate()
    {
        let (_root, db, chat, session) = workflow_fixture().await;
        let id = format!("h182-limit-{index}");
        let workflow = request(
            json!({"answerTemplate":"{{value}}{{value}}", "answerVariables":[
                {"name":"value","value":"{{#start.payload.text#}}"}
            ]}),
            json!({"text":text}),
        );
        let (result, events) = run(&chat, session, workflow, &id).await;
        assert!(
            result.is_err(),
            "both raw expansion and serialized output must be bounded"
        );
        assert!(events.iter().any(|e| e.node_id == "answer"
            && e.error
                .as_deref()
                .is_some_and(|error| error.contains("1 MiB"))));
        assert!(events
            .iter()
            .any(|e| e.node_id == "answer" && e.status == NodeExecutionStatus::Failed));
        assert!(events
            .iter()
            .all(|e| e.node_id != "answer" || e.status != NodeExecutionStatus::Completed));
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
async fn h182_control_end_keeps_typed_json_and_literal_mapping_without_an_answer_node() {
    let (_root, db, chat, session) = workflow_fixture().await;
    let payload = json!({"text":"{{#start.payload.hidden#}}", "hidden":"not substituted", "value":[1,false,null]});
    let mut workflow = request(json!({"answerTemplate":"unused"}), payload.clone());
    workflow.definition.nodes.remove(1);
    workflow.definition.edges =
        serde_json::from_value(json!([{"id":"a","source":"start","target":"end"}])).unwrap();
    workflow.definition.nodes[1].config = serde_json::from_value(json!({"outputVariables":[
        {"name":"report","type":"object","value":"{{#start.payload#}}"}
    ]}))
    .unwrap();
    let (result, events) = run(&chat, session, workflow, "h182-end-control").await;
    assert!(result.is_ok(), "{result:?}");
    assert_eq!(completed(&events, "end"), &json!({"report":payload}));
    assert_eq!(
        RunStore::new(&db)
            .get("h182-end-control")
            .unwrap()
            .unwrap()
            .terminal_kind
            .as_deref(),
        Some("succeeded")
    );
}

#[tokio::test]
async fn h182_unused_bindings_share_one_bounded_resolution_budget() {
    let (_root, _db, chat, session) = workflow_fixture().await;
    let workflow = request(
        json!({"answerTemplate":"valid constant", "answerVariables":[
            {"name":"a","value":"{{#start.payload.text#}}"},
            {"name":"b","value":"{{#start.payload.text#}}"}
        ]}),
        json!({"text":"x".repeat(600_000)}),
    );
    let (result, events) = run(&chat, session, workflow, "h182-binding-budget").await;
    assert!(
        result.is_err(),
        "unused bindings must not bypass the combined value budget"
    );
    assert!(events
        .iter()
        .any(|e| e.node_id == "answer" && e.error.as_deref().is_some_and(|e| e.contains("1 MiB"))));
}

#[tokio::test]
async fn h182_real_sqlite_reopen_replays_identical_answer_payload_and_one_terminal() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("answer.db");
    let db = Arc::new(Database::open(&path).unwrap());
    let (_root, db, chat, session) = workflow_fixture_with_database(root, db).await;
    let (result, events) = run(
        &chat,
        session,
        request(
            json!({"answerTemplate":"{{#start.payload.name#}}"}),
            json!({"name":"durable"}),
        ),
        "h182-durable",
    )
    .await;
    assert!(result.is_ok(), "{result:?}");
    assert_eq!(completed(&events, "answer"), &json!({"answer":"durable"}));
    let before = RunStore::new(&db)
        .events_after("h182-durable", None)
        .unwrap();
    let stored: Vec<NodeExecutionEvent> = before
        .iter()
        .filter(|e| e.event_type == "workflow_node")
        .map(|e| serde_json::from_value(e.data["event"].clone()).unwrap())
        .collect();
    assert_eq!(
        stored, events,
        "live payload and durable wire contract must match"
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
    assert_eq!(store.events_after("h182-durable", None).unwrap(), before);
    assert_eq!(
        store.events_after("h182-durable", Some(cursor)).unwrap(),
        before
            .iter()
            .filter(|e| e.sequence > cursor)
            .cloned()
            .collect::<Vec<_>>()
    );
    assert_eq!(
        store
            .get("h182-durable")
            .unwrap()
            .unwrap()
            .terminal_kind
            .as_deref(),
        Some("succeeded")
    );
    assert!(
        deepagent_persistence::run_control::RunControlStore::new(&reopened)
            .list_actions("h182-durable")
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
#[ignore = "requires local Node and desktop dependencies; production schema/defaults/serializer to Kernel"]
async fn h182_desktop_serialized_answer_executes_and_maps_the_actual_payload() {
    let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../apps/desktop/scripts/audit-workflow-contract.cjs");
    let output = std::process::Command::new("node")
        .arg(script)
        .arg("--emit-answer-fixture")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let fixtures: Vec<Value> = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(fixtures.len(), 1);
    let workflow = serde_json::from_value(fixtures[0]["workflow"].clone()).unwrap();
    let (_root, db, chat, session) = workflow_fixture().await;
    let (result, events) = run(&chat, session, workflow, "h182-desktop-seam").await;
    assert!(result.is_ok(), "{result:?}");
    let expected = "你好 Eighteen，Eighteen！";
    assert_eq!(completed(&events, "answer"), &json!({"answer":expected}));
    assert_eq!(completed(&events, "end"), &json!({"report":expected}));
    assert_eq!(
        RunStore::new(&db)
            .get("h182-desktop-seam")
            .unwrap()
            .unwrap()
            .terminal_kind
            .as_deref(),
        Some("succeeded")
    );
}
