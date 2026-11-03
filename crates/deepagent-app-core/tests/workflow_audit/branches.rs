//! H-199: the branch catalog must be closed over every real evaluator result.
use super::conditions::{run, terminal};
use super::*;
use serde_json::{json, Value};

fn request(ids: &[&str], matches: &[bool], legacy: bool) -> WorkflowRequest {
    assert_eq!(ids.len(), matches.len());
    let groups: Vec<_> = ids.iter().zip(matches).map(|(id, matched)| {
        if legacy {
            json!({"id":id,"logicalOperator":"and","conditions":[{"left":matched,"operator":"is","right":true}]})
        } else {
            json!({"id":id,"logic":"and","items":[{"variable":matched,"operator":"is","value":true}]})
        }
    }).collect();
    let mut nodes = vec![
        json!({"id":"start","kind":"start"}),
        json!({"id":"decide","kind":"if-else","config":{"conditions":groups}}),
        json!({"id":"end","kind":"end"}),
    ];
    let mut edges = vec![json!({"id":"entry","source":"start","target":"decide"})];
    let mut handles: Vec<_> = if ids.len() == 1 {
        vec!["true"]
    } else {
        ids.to_vec()
    };
    handles.push("false");
    for (index, handle) in handles.iter().enumerate() {
        let node = format!("path{index}");
        nodes.push(
            json!({"id":node,"kind":"template-transform","config":{"templateScript":handle}}),
        );
        edges.push(json!({"id":format!("choice{index}"),"source":"decide","source_handle":handle,"target":node}));
        edges.push(json!({"id":format!("exit{index}"),"source":node,"target":"end"}));
    }
    serde_json::from_value(json!({"definition":{"version":1,"nodes":nodes,"edges":edges}})).unwrap()
}

async fn check(ids: &[&str], matches: &[bool], legacy: bool, targeted: bool) {
    let (_root, db, chat, session) = workflow_fixture().await;
    let mut workflow = request(ids, matches, legacy);
    if targeted {
        workflow.target_node_id = Some("end".into());
    }
    let chosen = matches
        .iter()
        .position(|matched| *matched)
        .unwrap_or(ids.len());
    let branch = if chosen == ids.len() {
        "false"
    } else if ids.len() == 1 {
        "true"
    } else {
        ids[chosen]
    };
    let (result, nodes) = run(&chat, &session, workflow, "h199-connectable").await;
    assert!(
        result.is_ok(),
        "{ids:?}/{matches:?}/{targeted}/{legacy}: {result:?}"
    );
    assert!(nodes.iter().any(|n| n.node_id == "decide"
        && n.status == NodeExecutionStatus::Completed
        && n.outputs == Some(json!({"branch":branch}))));
    for index in 0..=ids.len() {
        let path = format!("path{index}");
        let selected = index == chosen;
        assert!(nodes.iter().any(|n| n.node_id == path
            && n.status
                == if selected {
                    NodeExecutionStatus::Completed
                } else {
                    NodeExecutionStatus::Skipped
                }));
        assert_eq!(
            nodes
                .iter()
                .any(|n| n.node_id == path && n.status == NodeExecutionStatus::Running),
            selected
        );
    }
    assert!(nodes
        .iter()
        .any(|n| n.node_id == "end" && n.status == NodeExecutionStatus::Completed));
    terminal(&db, "h199-connectable", "succeeded");
    let durable: Vec<deepagent_runtime::workflow::NodeExecutionEvent> = RunStore::new(&db)
        .events_after("h199-connectable", None)
        .unwrap()
        .into_iter()
        .filter(|e| e.event_type == "workflow_node")
        .map(|e| serde_json::from_value(e.data["event"].clone()).unwrap())
        .collect();
    assert_eq!(durable, nodes);
}

#[tokio::test]
async fn h199_single_case_default_and_legacy_ids_select_the_visible_true_false_ports() {
    for id in ["if", "true", "d5b4fd30-7b79-4d25-9ff3-aba124fd73ae"] {
        for matched in [true, false] {
            for legacy in [false, true] {
                for targeted in [false, true] {
                    check(&[id], &[matched], legacy, targeted).await;
                }
            }
        }
    }
}

#[tokio::test]
async fn h199_multiple_cases_select_first_middle_last_or_the_explicit_else() {
    for matches in [
        [true, true, true],
        [false, true, true],
        [false, false, true],
        [false, false, false],
    ] {
        for legacy in [false, true] {
            for targeted in [false, true] {
                check(&["red", "blue", "yellow"], &matches, legacy, targeted).await;
            }
        }
    }
}

#[tokio::test]
async fn h199_ambiguous_catalogs_and_stale_handles_reject_before_any_effect() {
    let (root, db, chat, session) = workflow_fixture().await;
    let mut invalid: Vec<WorkflowRequest> = [
        vec![],
        vec!["false"],
        vec!["true", "b"],
        vec!["a", "true"],
        vec!["a", "false"],
        vec!["a", "a"],
        vec![""],
        vec!["bad/id"],
    ]
    .into_iter()
    .map(|ids| request(&ids, &vec![true; ids.len()], false))
    .collect();
    for groups in [Value::Null, json!([{}]), json!([{"id":false}])] {
        let mut w = request(&["if"], &[true], false);
        w.definition.nodes[1]
            .config
            .insert("conditions".into(), groups);
        invalid.push(w);
    }
    for (ids, handle) in [
        (vec!["red", "blue"], "true"),
        (vec!["if"], "if"),
        (vec!["if"], "unknown"),
    ] {
        let mut w = request(&ids, &vec![true; ids.len()], false);
        w.definition.edges[1].source_handle = Some(handle.into());
        invalid.push(w);
    }
    for (index, mut w) in invalid.into_iter().enumerate() {
        w.definition.nodes.push(serde_json::from_value(json!({"id":"write","kind":"tool","config":{
            "toolName":"write_file","toolArguments":{"path":"must-not-exist.txt","content":"synthetic"}}})).unwrap());
        w.definition.edges[0].source = "write".into();
        w.definition.edges.push(
            serde_json::from_value(json!({"id":"write-entry","source":"start","target":"write"}))
                .unwrap(),
        );
        let id = format!("h199-invalid-{index}");
        let (result, nodes) = run(&chat, &session, w, &id).await;
        assert!(result.is_err());
        assert!(
            nodes.is_empty(),
            "invalid branch catalog must fail admission, not after ancestor tools"
        );
        assert!(!root.path().join("must-not-exist.txt").exists());
        assert!(RunStore::new(&db)
            .events_after(&id, None)
            .unwrap()
            .iter()
            .all(|e| e.event_type != "tool_started"));
        terminal(&db, &id, "failed");
    }
}

#[tokio::test]
async fn h199_cases_and_fallback_approve_only_the_chosen_effect_and_replay_without_redispatch() {
    for (matches, branch, path) in [
        ([true, true, true], "red", "path0"),
        ([false, true, true], "blue", "path1"),
        ([false, false, true], "yellow", "path2"),
        ([false, false, false], "false", "path3"),
    ] {
        super::conditions::check_chosen_effect(
            request(&["red", "blue", "yellow"], &matches, false),
            "true",
            branch,
            path,
        )
        .await;
    }
    for case in ["denied", "failed", "cancelled"] {
        super::conditions::check_chosen_effect(
            request(&["red", "blue", "yellow"], &[false, false, false], false),
            case,
            "false",
            "path3",
        )
        .await;
    }
}

#[tokio::test]
async fn h199_iteration_items_keep_the_same_catalog_and_only_their_selected_path() {
    let (_root, db, chat, session) = workflow_fixture().await;
    let w = serde_json::from_value(json!({"definition":{"version":1,"nodes":[
        {"id":"start","kind":"start","config":{"inputVariables":[{"name":"items","type":"array","default":["red","blue","other"]}]}},
        {"id":"iterate","kind":"iteration","config":{"inputVariable":"{{#start.items#}}","outputVariable":"{{#item-end.report#}}","parallel":false,"maxConcurrency":1,"errorHandling":"terminate"}},
        {"id":"entry","kind":"iteration-start"},
        {"id":"decide","kind":"if-else","config":{"conditions":[
            {"id":"red","logic":"and","items":[{"variable":"{{#entry.item#}}","operator":"is","value":"red"}]},
            {"id":"blue","logic":"and","items":[{"variable":"{{#entry.item#}}","operator":"is","value":"blue"}]}]}},
        {"id":"red-path","kind":"template-transform","config":{"templateScript":"RED"}},
        {"id":"blue-path","kind":"template-transform","config":{"templateScript":"BLUE"}},
        {"id":"else-path","kind":"template-transform","config":{"templateScript":"ELSE"}},
        {"id":"item-end","kind":"end","config":{"outputVariables":[{"name":"report","type":"string","value":"{{#entry.item#}}"}]}},
        {"id":"end","kind":"end","config":{"outputVariables":[{"name":"report","type":"array","value":"{{#iterate.output#}}"}]}}
    ],"edges":[{"id":"a","source":"start","target":"iterate"},
        {"id":"b","source":"iterate","source_handle":"item","target":"entry"},
        {"id":"c","source":"entry","target":"decide"},
        {"id":"r","source":"decide","source_handle":"red","target":"red-path"},
        {"id":"bl","source":"decide","source_handle":"blue","target":"blue-path"},
        {"id":"f","source":"decide","source_handle":"false","target":"else-path"},
        {"id":"re","source":"red-path","target":"item-end"},
        {"id":"ble","source":"blue-path","target":"item-end"},
        {"id":"fe","source":"else-path","target":"item-end"},
        {"id":"d","source":"iterate","source_handle":"done","target":"end"}]}})).unwrap();
    let (result, nodes) = run(&chat, &session, w, "h199-items").await;
    assert!(result.is_ok(), "{result:?}");
    let decisions: Vec<_> = nodes
        .iter()
        .filter(|n| n.node_id == "decide" && n.status == NodeExecutionStatus::Completed)
        .collect();
    assert_eq!(decisions.len(), 3);
    for (decision, (branch, path)) in decisions.iter().zip([
        ("red", "red-path"),
        ("blue", "blue-path"),
        ("false", "else-path"),
    ]) {
        assert_eq!(decision.outputs, Some(json!({"branch":branch})));
        assert_eq!(
            nodes
                .iter()
                .filter(|n| n.scope == decision.scope
                    && n.node_id.ends_with("-path")
                    && n.status == NodeExecutionStatus::Running)
                .count(),
            1
        );
        assert!(nodes.iter().any(|n| n.scope == decision.scope
            && n.node_id == path
            && n.status == NodeExecutionStatus::Completed));
    }
    assert!(
        nodes
            .iter()
            .any(|n| n.node_id == "end"
                && n.outputs == Some(json!({"report":["red","blue","other"]})))
    );
    terminal(&db, "h199-items", "succeeded");
}

#[tokio::test]
#[ignore = "requires local Node and desktop dependencies; real branch serializer/Kernel/events consumer"]
async fn h199_desktop_branch_catalog_matches_the_kernel_and_production_consumer() {
    let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../apps/desktop/scripts/audit-workflow-contract.cjs");
    let emitted = std::process::Command::new("node")
        .arg(&script)
        .arg("--emit-branch-fixtures")
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
        let workflow: WorkflowRequest =
            serde_json::from_value(fixture["workflow"].clone()).unwrap();
        let catalog = &fixture["branchCatalog"];
        assert_eq!(catalog["version"], 1);
        assert_eq!(catalog["fallback"]["id"], "false");
        let expected = catalog["expectedBranch"].as_str().unwrap();
        let (_root, db, chat, session) = workflow_fixture().await;
        let (result, nodes) = run(&chat, &session, workflow, "h199-desktop").await;
        assert!(result.is_ok(), "{name}: {result:?}");
        assert!(nodes
            .iter()
            .any(|n| n.node_id == "decide" && n.outputs == Some(json!({"branch":expected}))));
        let paths: Vec<_> = nodes
            .iter()
            .filter(|n| n.node_id.starts_with("path") && n.status == NodeExecutionStatus::Running)
            .collect();
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0].node_id, catalog["expectedPath"].as_str().unwrap());
        terminal(&db, "h199-desktop", "succeeded");
        let replay = tempfile::NamedTempFile::new().unwrap();
        let mut replay_data = fixture.clone();
        replay_data
            .as_object_mut()
            .unwrap()
            .insert("events".into(), serde_json::to_value(nodes).unwrap());
        std::fs::write(replay.path(), serde_json::to_vec(&replay_data).unwrap()).unwrap();
        let consumed = std::process::Command::new("node")
            .arg(&script)
            .arg("--consume-branch-events")
            .arg(replay.path())
            .output()
            .unwrap();
        assert!(
            consumed.status.success(),
            "{name}: {}",
            String::from_utf8_lossy(&consumed.stderr)
        );
    }
}
