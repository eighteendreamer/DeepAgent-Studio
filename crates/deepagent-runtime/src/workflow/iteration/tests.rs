use super::super::graph::{WorkflowEdgeSpec, WorkflowNodeSpec};
use super::*;
use serde_json::Map;

fn definition() -> WorkflowDefinition {
    serde_json::from_value(json!({"version":1,"nodes":[
        {"id":"start","kind":"start","config":{"inputVariables":[{"name":"items","type":"array"}]}},
        {"id":"iter","kind":"iteration","config":{"inputVariable":"{{#start.items#}}","outputVariable":"{{#body.output#}}"}},
        {"id":"entry","kind":"iteration-start"},
        {"id":"body","kind":"template-transform"},
        {"id":"end","kind":"end"}
    ],"edges":[
        {"id":"a","source":"start","target":"iter"},
        {"id":"b","source":"iter","source_handle":"item","target":"entry"},
        {"id":"c","source":"entry","target":"body"},
        {"id":"d","source":"iter","source_handle":"done","target":"end"}
    ]})).unwrap()
}

#[test]
fn plans_exclusive_item_body_and_done_boundary_without_rewriting_the_definition() {
    let definition = definition();
    let before = serde_json::to_vec(&definition).unwrap();
    let plans = compile_plans(&definition).unwrap();
    let plan = &plans["iter"];
    assert_eq!(plan.body, BTreeSet::from(["entry".into(), "body".into()]));
    assert_eq!(plan.roots, BTreeSet::from(["entry".into()]));
    assert_eq!(plan.concurrency, 1);
    assert_eq!(plan.error_handling, ErrorHandling::Terminate);
    assert!(!plan.flatten);
    assert_eq!(scope_of(&plans, "body"), vec!["iter"]);
    assert!(scope_of(&plans, "end").is_empty());
    assert_eq!(before, serde_json::to_vec(&definition).unwrap());
}

#[test]
fn rejects_wrong_types_unknown_policy_concurrency_and_unpublished_legacy_map_fields() {
    for (key, value) in [
        ("parallel", json!("true")),
        ("flatten", json!("false")),
        ("maxConcurrency", json!(0)),
        ("maxConcurrency", json!(51)),
        ("maxConcurrency", json!(-1)),
        ("maxConcurrency", json!(2.5)),
        ("errorHandling", json!("ignore")),
        ("iterationCollection", json!([])),
        ("inputVariable", json!([])),
        ("inputVariable", json!("prefix {{#start.items#}}")),
        ("outputVariable", json!("{{#end.output#}}")),
        ("outputVariable", json!("{{#body.missing#}}")),
    ] {
        let mut definition = definition();
        definition.nodes[1].config.insert(key.into(), value);
        assert!(compile_plans(&definition).is_err(), "{key}");
    }
}

#[test]
fn published_policies_and_concurrency_are_typed_and_serial_never_claims_parallelism() {
    for policy in ["terminate", "continue", "remove"] {
        let mut definition = definition();
        let config = &mut definition.nodes[1].config;
        config.insert("errorHandling".into(), json!(policy));
        config.insert("maxConcurrency".into(), json!(50));
        assert_eq!(compile_plans(&definition).unwrap()["iter"].concurrency, 1);
        definition.nodes[1]
            .config
            .insert("parallel".into(), json!(true));
        assert_eq!(compile_plans(&definition).unwrap()["iter"].concurrency, 50);
    }
}

#[test]
fn immutable_input_and_output_selection_keep_arrays_objects_and_artifacts_typed() {
    let definition = definition();
    let plans = compile_plans(&definition).unwrap();
    let plan = &plans["iter"];
    let original = json!([{"a":1}, "artifact://synthetic", [1, 2], null]);
    let outputs = BTreeMap::from([("start".into(), json!({"items":original}))]);
    let mut items = plan.items(&outputs).unwrap();
    items[0] = json!("changed snapshot only");
    assert_eq!(outputs["start"]["items"], original);
    let result = plan
        .select(
            &json!("unused"),
            &BTreeMap::from([("body".into(), json!({"output":{"typed":[1,2]}}))]),
        )
        .unwrap();
    assert_eq!(result, json!({"typed":[1,2]}));
    assert!(plan
        .items(&BTreeMap::from([(
            "start".into(),
            json!({"items":"not an array"})
        )]))
        .is_err());
    assert!(plan.select(&Value::Null, &BTreeMap::new()).is_err());
}

#[test]
fn rejects_cross_scope_entry_control_edges_but_keeps_done_reachable_joins_outside_body() {
    let mut definition = definition();
    definition.edges.push(WorkflowEdgeSpec {
        id: "cross".into(),
        source: "start".into(),
        target: "body".into(),
        source_handle: None,
        target_handle: None,
    });
    assert!(compile_plans(&definition).is_err());
    definition.edges.pop();
    definition.edges.push(WorkflowEdgeSpec {
        id: "join".into(),
        source: "body".into(),
        target: "end".into(),
        source_handle: None,
        target_handle: None,
    });
    assert!(!compile_plans(&definition).unwrap()["iter"]
        .body
        .contains("end"));
}

#[test]
fn nested_selectors_must_use_nested_aggregate_not_last_nested_item() {
    let mut definition = definition();
    definition.nodes.push(WorkflowNodeSpec {
        id: "inner".into(),
        kind: "iteration".into(),
        config: json!({"inputVariable":"{{#entry.item#}}","outputVariable":"item"})
            .as_object()
            .unwrap()
            .clone(),
    });
    definition.nodes.push(WorkflowNodeSpec {
        id: "inner-entry".into(),
        kind: "iteration-start".into(),
        config: Map::new(),
    });
    definition.edges.retain(|edge| edge.id != "c");
    for (id, source, target, port) in [
        ("c", "entry", "inner", None),
        ("e", "inner", "inner-entry", Some("item")),
        ("f", "inner", "body", Some("done")),
    ] {
        definition.edges.push(WorkflowEdgeSpec {
            id: id.into(),
            source: source.into(),
            target: target.into(),
            source_handle: port.map(str::to_owned),
            target_handle: None,
        });
    }
    definition.nodes[1]
        .config
        .insert("outputVariable".into(), json!("{{#inner.output#}}"));
    let plans = compile_plans(&definition).unwrap();
    assert_eq!(scope_of(&plans, "inner-entry"), vec!["iter", "inner"]);
    definition.nodes[1]
        .config
        .insert("outputVariable".into(), json!("{{#inner-entry.item#}}"));
    assert!(compile_plans(&definition).is_err());
}

#[test]
fn graph_admission_allows_own_aggregate_selector_but_never_exposes_last_item_outputs() {
    let mut definition = definition();
    definition.nodes[3]
        .config
        .insert("templateScript".into(), json!("{{#entry.item#}}"));
    assert!(super::super::graph::compile(definition.clone()).is_ok());
    definition.edges.push(WorkflowEdgeSpec {
        id: "join".into(),
        source: "body".into(),
        target: "end".into(),
        source_handle: None,
        target_handle: None,
    });
    definition.nodes[4].config.insert(
        "outputVariables".into(),
        json!([
            {"name":"leaked","type":"string","value":"{{#body.output#}}"}
        ]),
    );
    let error = super::super::graph::compile(definition)
        .unwrap_err()
        .to_string();
    assert!(error.contains("cannot escape its scope"), "{error}");
}

#[test]
fn graph_admission_rejects_orphan_iteration_start_instead_of_fabricating_empty_item() {
    let definition = WorkflowDefinition {
        version: 1,
        nodes: vec![
            WorkflowNodeSpec {
                id: "start".into(),
                kind: "start".into(),
                config: Map::new(),
            },
            WorkflowNodeSpec {
                id: "entry".into(),
                kind: "iteration-start".into(),
                config: Map::new(),
            },
        ],
        edges: vec![super::super::graph::WorkflowEdgeSpec {
            id: "entry-edge".into(),
            source: "start".into(),
            target: "entry".into(),
            source_handle: None,
            target_handle: None,
        }],
    };
    let error = super::super::graph::compile(definition)
        .unwrap_err()
        .to_string();
    assert!(error.contains("owning item scope"));
}

#[test]
fn item_cannot_read_its_owners_unfinished_aggregate() {
    let mut definition = definition();
    definition.nodes[3]
        .config
        .insert("templateScript".into(), json!("{{#iter.output#}}"));
    let error = super::super::graph::compile(definition)
        .unwrap_err()
        .to_string();
    assert!(error.contains("unfinished owner aggregate"), "{error}");
}
