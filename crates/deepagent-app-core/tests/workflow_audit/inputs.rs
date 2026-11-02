//! H-184..190: one executable start-input contract, before any node action.
use super::*;
use deepagent_runtime::workflow::{compile, NodeExecutionEvent};
use serde_json::{json, Value};

fn request(declarations: Value, inputs: Value) -> WorkflowRequest {
    serde_json::from_value(json!({
        "definition":{"version":1,"nodes":[
            {"id":"start","kind":"start","config":{"inputVariables":declarations}},
            {"id":"end","kind":"end","config":{"outputVariables":[
                {"name":"report","type":"object","value":"{{#start.value#}}"}
            ]}}
        ],"edges":[{"id":"a","source":"start","target":"end"}]},
        "inputs":inputs
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
            |_| panic!("input admission must never request approval"),
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
    assert_eq!(terminal.len(), 1);
    assert_eq!(terminal[0].status, NodeExecutionStatus::Completed);
    terminal[0]
}

#[tokio::test]
async fn h184_190_invalid_inputs_fail_before_even_an_independent_side_effect_root() {
    for (index, (declaration, inputs, code)) in [
        (
            json!({"name":"value","type":"string","required":true}),
            json!({}),
            "MissingWorkflowInput",
        ),
        (
            json!({"name":"value","type":"string","required":true,"default":"fallback"}),
            json!({"value":null}),
            "InvalidWorkflowInput",
        ),
        (
            json!({"name":"value","type":"number"}),
            json!({"value":"private-canary"}),
            "InvalidWorkflowInput",
        ),
        (
            json!({"name":"value","type":"boolean"}),
            json!({"value":0}),
            "InvalidWorkflowInput",
        ),
        (
            json!({"name":"value","type":"object"}),
            json!({"value":[]}),
            "InvalidWorkflowInput",
        ),
        (
            json!({"name":"value","type":"array"}),
            json!({"value":{}}),
            "InvalidWorkflowInput",
        ),
        (
            json!({"name":"value","type":"string"}),
            json!({"value":1}),
            "InvalidWorkflowInput",
        ),
        (
            json!({"name":"value","type":"file"}),
            json!({"value":"C:\\private-canary.txt"}),
            "InvalidWorkflowInput",
        ),
        (
            json!({"name":"value","type":"file"}),
            json!({"value":"artifact://"}),
            "InvalidWorkflowInput",
        ),
        (
            json!({"name":"value","type":"string"}),
            json!({"value":"ok","private_canary":"private-canary"}),
            "UnknownWorkflowInput",
        ),
        (
            json!({"name":"value","type":"string","options":["safe","fast"]}),
            json!({"value":"SAFE"}),
            "InvalidWorkflowInput",
        ),
        (
            json!({"name":"value","type":"number","minimum":10,"maximum":20}),
            json!({"value":9}),
            "InvalidWorkflowInput",
        ),
        (
            json!({"name":"value","type":"number","minimum":10,"maximum":20}),
            json!({"value":21}),
            "InvalidWorkflowInput",
        ),
        (
            json!({"name":"value","type":"string","minLength":2,"maxLength":3}),
            json!({"value":"a"}),
            "InvalidWorkflowInput",
        ),
        (
            json!({"name":"value","type":"string","minLength":2,"maxLength":3}),
            json!({"value":"abcd"}),
            "InvalidWorkflowInput",
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let (_root, db, chat, session) = workflow_fixture().await;
        let mut workflow = request(json!([declaration]), inputs);
        // Deliberately sorted before start, with no start ancestor: validation
        // inside execute_start would be too late. A real fs write tool must not run.
        workflow.definition.nodes.insert(0, serde_json::from_value(json!({
            "id":"early","kind":"tool","config":{
                "toolName":"write_file","toolArguments":{"path":"must-not-exist.txt","content":"canary"}
            }
        })).unwrap());
        let id = format!("h184-190-invalid-{index}");
        let (result, events) = run(&chat, session, workflow, &id).await;
        assert!(result.is_err(), "case {index} unexpectedly accepted");
        let error = result.unwrap_err().to_string();
        assert!(error.contains(code), "case {index}: {error}");
        assert!(!error.contains("private-canary"));
        assert!(events.is_empty(), "no node may start: {events:?}");
        assert!(!_root.path().join("must-not-exist.txt").exists());
        let store = RunStore::new(&db);
        assert_eq!(
            store.get(&id).unwrap().unwrap().terminal_kind.as_deref(),
            Some("failed")
        );
        let ledger = store.events_after(&id, None).unwrap();
        assert_eq!(
            ledger
                .iter()
                .filter(|e| e.event_type == "run_terminal")
                .count(),
            1
        );
        assert!(ledger.iter().all(|e| e.event_type != "action_started"));
        assert!(
            deepagent_persistence::run_control::RunControlStore::new(&db)
                .list_actions(&id)
                .unwrap()
                .is_empty()
        );
    }
}

#[tokio::test]
#[ignore = "requires local Node and desktop dependencies; production serializer to real Kernel"]
async fn h184_190_desktop_input_declarations_reach_the_real_kernel() {
    let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../apps/desktop/scripts/audit-workflow-contract.cjs");
    let output = std::process::Command::new("node")
        .arg(script)
        .arg("--emit-input-contract-fixture")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let fixtures: Vec<Value> = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(fixtures.len(), 2);
    let rejected: WorkflowRequest =
        serde_json::from_value(fixtures[1]["workflow"].clone()).unwrap();
    let (_root, db, chat, session) = workflow_fixture().await;
    let (result, events) = run(&chat, session, rejected, "h184-190-desktop-missing").await;
    let record = RunStore::new(&db)
        .get("h184-190-desktop-missing")
        .unwrap()
        .unwrap();
    super::assert_workflow_input_admission_rejected(
        &db,
        &result,
        &record,
        &events,
        "MissingWorkflowInput",
    );
    let workflow: WorkflowRequest =
        serde_json::from_value(fixtures[0]["workflow"].clone()).unwrap();
    assert!(workflow.inputs.is_empty());
    let (_root, _db, chat, session) = workflow_fixture().await;
    let (result, events) = run(&chat, session, workflow, "h184-190-desktop").await;
    assert!(result.is_ok(), "{result:?}");
    assert_eq!(
        completed(&events, "start").outputs,
        Some(json!({
            "value":{"note":"{{#missing.value#}}","nested":[false,0]},"optional":null,
            "nil":null,"flag":false,"count":0,"mode":""
        }))
    );
    assert_eq!(
        completed(&events, "start").updates["inputs"]["sources"],
        json!({
            "value":"default","optional":"absent","nil":"default","flag":"default","count":"default","mode":"default"
        })
    );
    assert_eq!(
        completed(&events, "end").outputs,
        Some(json!({"report":{"note":"{{#missing.value#}}","nested":[false,0]}}))
    );
}

#[test]
fn h184_190_direct_constructor_and_normalized_value_budgets_cannot_be_bypassed() {
    use deepagent_runtime::workflow::{NodeEventPublisher, WorkflowAgent};
    let make = |declarations, inputs| {
        let mut workflow = request(declarations, inputs);
        // These are constructor-only tests; no downstream reference is needed.
        workflow.definition.nodes[1].config.clear();
        WorkflowAgent::new(
            compile(workflow.definition).unwrap(),
            workflow.inputs,
            None,
            NodeEventPublisher::default(),
        )
    };
    assert!(make(
        json!([{"name":"value","type":"number","required":true}]),
        json!({})
    )
    .is_err());
    assert!(make(
        json!([{"name":"value","type":"number"}]),
        json!({"value":"3"})
    )
    .is_err());
    assert!(make(json!([]), json!({"undeclared":1})).is_err());
    assert!(make(
        json!([{"name":"value","type":"object"}]),
        json!({"value":{"dynamic":[0,false]}})
    )
    .is_ok());
    assert!(make(
        json!([{"name":"value","variable":"legacy_annotation","type":"string"}]),
        json!({"value":"canonical name"})
    )
    .is_ok());
    let declarations = json!([
        {"name":"value","type":"string","default":"d".repeat(550_000)},
        {"name":"provided","type":"string"}
    ]);
    assert!(
        make(
            declarations.clone(),
            json!({"provided":"p".repeat(550_000)})
        )
        .is_err(),
        "combined defaults and provided map exceeds 1 MiB"
    );
    assert!(make(declarations, json!({"provided":"small"})).is_ok());
    assert!(make(
        json!([{"name":"value","type":"string"}]),
        json!({"value":"x".repeat(1_048_576)})
    )
    .is_err());
    let mut deep = json!(0);
    for _ in 0..66 {
        deep = json!([deep]);
    }
    assert!(make(
        json!([{"name":"value","type":"array"}]),
        json!({"value":deep})
    )
    .is_err());
    // Hidden constraints retained by a desktop type switch are inactive, not
    // silently interpreted as number/string constraints of the new type.
    assert!(make(
        json!([{"name":"value","type":"boolean","options":["old"],"minimum":9,"maxLength":0}]),
        json!({"value":false})
    )
    .is_ok());
}

#[test]
fn h184_190_schema_defaults_are_validated_even_when_a_caller_overrides_them() {
    for declaration in [
        json!({"name":"value","type":"number","default":"wrong"}),
        json!({"name":"value","type":"string","required":true,"default":null}),
        json!({"name":"value","type":"string","options":["safe"],"default":"bad"}),
        json!({"name":"value","type":"number","minimum":10,"default":9}),
        json!({"name":"value","type":"string","minLength":2,"default":"a"}),
        json!({"name":"value","type":"string","required":"true"}),
        json!({"name":"value","type":"string","minLength":-1}),
        json!({"name":"value","type":"string","maxLength":1.5}),
        json!({"name":"value","type":"string","minLength":3,"maxLength":2}),
        json!({"name":"value","type":"number","minimum":20,"maximum":10}),
        json!({"name":"value","type":"number","minimum":"10"}),
        json!({"name":"value","type":"string","options":["a", "a"]}),
        json!({"name":"value","type":"string","options":[1]}),
    ] {
        assert!(
            compile(request(json!([declaration.clone()]), json!({"value":"override"})).definition)
                .is_err(),
            "{declaration}"
        );
    }
}

#[tokio::test]
async fn h184_190_defaults_optional_null_and_provided_values_have_one_provenance() {
    let declarations = json!([
        {"name":"value","type":"object","required":true,"default":{"nested":[false,0,null,"{{#missing.value#}}"]}},
        {"name":"empty","type":"string","required":true,"default":"fallback"},
        {"name":"flag","type":"boolean","default":true},
        {"name":"zero","type":"number","default":5},
        {"name":"optional","type":"string"},
        {"name":"nil","type":"array","default":null},
        {"name":"explicit","type":"object","default":{"unused":true}},
        {"name":"file","type":"file","default":"artifact://synthetic-reference"}
    ]);
    let (_root, _db, chat, session) = workflow_fixture().await;
    let (result, events) = run(
        &chat,
        session,
        request(
            declarations,
            json!({"empty":"","flag":false,"zero":0,"explicit":null}),
        ),
        "h184-190-values",
    )
    .await;
    assert!(result.is_ok(), "{result:?}");
    let start = completed(&events, "start");
    assert_eq!(
        start.outputs,
        Some(json!({
            "value":{"nested":[false,0,null,"{{#missing.value#}}"]},"empty":"","flag":false,"zero":0,
            "optional":null,"nil":null,"explicit":null,"file":"artifact://synthetic-reference"
        }))
    );
    assert_eq!(
        completed(&events, "end").outputs,
        Some(json!({"report":{"nested":[false,0,null,"{{#missing.value#}}"]}}))
    );
    assert_eq!(start.updates["inputs"]["version"], 1);
    assert_eq!(
        start.updates["inputs"]["sources"],
        json!({"value":"default","empty":"provided","flag":"provided","zero":"provided","optional":"absent","nil":"default","explicit":"provided","file":"default"})
    );
    assert_eq!(start.updates["inputs"]["revision"], start.revision);
}

#[tokio::test]
async fn h188_190_exact_options_unicode_scalars_and_lossless_numeric_limits() {
    for (index, (declaration, value, accepted)) in [
        (
            json!({"name":"value","type":"string","options":["", "é"]}),
            json!(""),
            true,
        ),
        (
            json!({"name":"value","type":"string","options":["é"]}),
            json!("e\u{301}"),
            false,
        ),
        (
            json!({"name":"value","type":"string","options":[]}),
            json!("unconstrained"),
            true,
        ),
        (
            json!({"name":"value","type":"string","minLength":2,"maxLength":2}),
            json!("中😀"),
            true,
        ),
        (
            json!({"name":"value","type":"string","minLength":2,"maxLength":2}),
            json!("e\u{301}"),
            true,
        ),
        (
            json!({"name":"value","type":"number","minimum":-0.5,"maximum":0}),
            json!(-0.5),
            true,
        ),
        (
            json!({"name":"value","type":"number","minimum":-0.5,"maximum":0}),
            json!(0),
            true,
        ),
        (
            json!({"name":"value","type":"number","minimum":9_007_199_254_740_993_u64}),
            json!(9_007_199_254_740_992_u64),
            false,
        ),
        (
            json!({"name":"value","type":"number","maximum":9_007_199_254_740_992_u64}),
            json!(9_007_199_254_740_993_u64),
            false,
        ),
        (
            json!({"name":"value","type":"number","minimum":u64::MAX,"maximum":u64::MAX}),
            json!(u64::MAX),
            true,
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let (_root, _db, chat, session) = workflow_fixture().await;
        let (result, events) = run(
            &chat,
            session,
            request(json!([declaration]), json!({"value":value})),
            &format!("h188-190-bounds-{index}"),
        )
        .await;
        assert_eq!(result.is_ok(), accepted, "case {index}: {result:?}");
        if accepted {
            assert_eq!(
                completed(&events, "start").outputs,
                Some(json!({"value":value}))
            );
        } else {
            assert!(events.is_empty());
        }
    }
}

#[tokio::test]
async fn h184_190_sqlite_reopen_preserves_sources_cursor_failure_and_redaction() {
    for valid in [true, false] {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("inputs.db");
        let db = Arc::new(Database::open(&path).unwrap());
        let (_root, db, chat, session) = workflow_fixture_with_database(root, db).await;
        let declarations = json!([
            {"name":"value","type":"object","required":true,"default":{"nested":[0,false]}},
            {"name":"token","type":"string","default":"private-canary"}
        ]);
        let inputs = if valid {
            json!({})
        } else {
            json!({"value":"private-canary"})
        };
        let (result, events) = run(
            &chat,
            session,
            request(declarations, inputs),
            "h184-190-durable",
        )
        .await;
        assert_eq!(result.is_ok(), valid);
        let before = RunStore::new(&db)
            .events_after("h184-190-durable", None)
            .unwrap();
        let stored: Vec<NodeExecutionEvent> = before
            .iter()
            .filter(|e| e.event_type == "workflow_node")
            .map(|e| serde_json::from_value(e.data["event"].clone()).unwrap())
            .collect();
        assert_eq!(stored, events);
        assert!(!serde_json::to_string(&before)
            .unwrap()
            .contains("private-canary"));
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
        assert_eq!(
            store.events_after("h184-190-durable", None).unwrap(),
            before
        );
        assert_eq!(
            store
                .events_after("h184-190-durable", Some(cursor))
                .unwrap(),
            before
                .iter()
                .filter(|e| e.sequence > cursor)
                .cloned()
                .collect::<Vec<_>>()
        );
        assert_eq!(
            store
                .get("h184-190-durable")
                .unwrap()
                .unwrap()
                .terminal_kind
                .as_deref(),
            Some(if valid { "succeeded" } else { "failed" })
        );
        assert!(
            deepagent_persistence::run_control::RunControlStore::new(&reopened)
                .list_actions("h184-190-durable")
                .unwrap()
                .is_empty()
        );
    }
}
