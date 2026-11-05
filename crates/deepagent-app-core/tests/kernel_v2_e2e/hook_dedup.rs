//! Distinct applicable lifecycle policies must survive dispatch deduplication.
//! Real SettingsService/Kernel/file tools; only the model transport is synthetic.
#[path = "hook_dedup/mcp.rs"]
mod mcp;

use super::responses_fixture::{call, completed, text};
use super::*;
use deepagent_core::{event::EventPayload, id::SessionId};
use deepagent_models::ModelRole;
use deepagent_persistence::event_store::EventStore;
use deepagent_persistence::run_store::RunStore;
use deepagent_runtime::RuntimeEvent;
use serde_json::{json, Value};
use std::str::FromStr;
use std::time::Duration;

const FLASH: &str = "deepseek-flash";
const PRO: &str = "deepseek-v4-pro";
const MARKER: &str = "AUDIT_HOOK_DEDUP";
const CANARY: &str = "SYNTHETIC_DEDUP_FILE_BODY";
const DENIAL: &str = "synthetic second-policy denial";
const CALL: &str = "dedup-parent-read";
const RUN: &str = "audit-hook-dedup";

#[derive(Clone, Copy, Debug)]
enum Case {
    Identical,
    ModelOnly,
    DifferentPrompt,
    ConditionOnly,
    DifferentConditionPrompt,
    ReversedConditions,
    SingleDeny,
    Unmatched,
}

fn definitions(case: Case, kind: &str) -> Value {
    let group = |model: &str, condition: Option<&str>, suffix: &str| {
        let mut group = json!({"matcher":"read_file","hooks":[{
            "type":kind,"model":model,"prompt":format!("{MARKER}{suffix}"),"timeout":5
        }]});
        if let Some(condition) = condition {
            group["if"] = json!(condition);
        }
        group
    };
    let groups = match case {
        Case::Identical => vec![group(FLASH, None, ""), group(FLASH, None, "")],
        Case::ModelOnly => vec![group(FLASH, None, ""), group(PRO, None, "")],
        Case::DifferentPrompt => vec![group(FLASH, None, ""), group(PRO, None, " second")],
        Case::ConditionOnly => vec![
            group(PRO, Some("write_file"), ""),
            group(PRO, Some("read_file"), ""),
        ],
        Case::DifferentConditionPrompt => vec![
            group(PRO, Some("write_file"), ""),
            group(PRO, Some("read_file"), " second"),
        ],
        Case::ReversedConditions => vec![
            group(PRO, Some("read_file"), ""),
            group(PRO, Some("write_file"), ""),
        ],
        Case::SingleDeny => vec![group(PRO, None, "")],
        Case::Unmatched => vec![group(PRO, Some("write_file"), "")],
    };
    json!({"hooks":{"PreToolUse":groups}})
}

#[derive(Default)]
struct DedupModel {
    parent: Mutex<Vec<Value>>,
    hooks: Mutex<Vec<Value>>,
}

#[async_trait]
impl HttpTransport for DedupModel {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        let body: Value = serde_json::from_str(&request.body)?;
        let model = body["model"].as_str().unwrap().to_string();
        assert!(model == FLASH || model == PRO);
        let hook = body["input"].to_string().contains(MARKER);
        let index = {
            let mut captured = if hook {
                self.hooks.lock().unwrap()
            } else {
                self.parent.lock().unwrap()
            };
            captured.push(body);
            captured.len() - 1
        };
        let item = if hook {
            text(
                &format!("dedup-decision-{index}"),
                &json!({"ok":model == FLASH,"reason":DENIAL}).to_string(),
            )
        } else {
            assert_eq!(model, FLASH);
            match index {
                0 => call(CALL, "read_file", json!({"path":"fixture.txt"})),
                1 => text("dedup-final", "Offline dedup audit complete."),
                _ => panic!("unexpected parent model step {index}"),
            }
        };
        emit_responses_events(
            sink,
            &completed(
                &format!("dedup-response-{hook}-{index}"),
                &model,
                item,
                json!({"input_tokens":12,"output_tokens":6,"total_tokens":18}),
            ),
        )
    }
}

async fn settings(db: Arc<Database>, case: Case, kind: &str) -> Arc<SettingsService> {
    let settings = Arc::new(SettingsService::new(
        db,
        Arc::new(MockTransport::with_get_json(
            r#"{"data":[{"id":"deepseek-flash","object":"model","owned_by":"deepseek"},{"id":"deepseek-v4-pro","object":"model","owned_by":"deepseek"}]}"#,
        )),
        Arc::new(MemorySecretStore::new()),
    ));
    settings
        .initialize("offline-audit-placeholder")
        .await
        .unwrap();
    for role in [ModelRole::Chat, ModelRole::Reasoner] {
        settings.set_model(role, FLASH).unwrap();
    }
    settings
        .set_active_permission_preset(PermissionPreset::Default)
        .unwrap();
    let config = definitions(case, kind);
    settings.set_hooks_json(&config.to_string()).unwrap();
    assert_eq!(
        settings.hook_definitions().unwrap().action_count(),
        config["hooks"]["PreToolUse"].as_array().unwrap().len()
    );
    settings
}

#[derive(Debug)]
struct Observation {
    hook_models: Vec<String>,
    tool_ok: bool,
    body_reached_parent: bool,
}

fn summarize(model: &DedupModel, events: &[RuntimeEvent]) -> Observation {
    let parent = model.parent.lock().unwrap();
    let hooks = model.hooks.lock().unwrap();
    assert_eq!(parent.len(), 2);
    assert!(!parent[0].to_string().contains(CANARY));
    let input = parent[1]["input"].as_array().unwrap();
    assert_eq!(
        input
            .iter()
            .filter(|item| item["type"] == "function_call" && item["call_id"] == CALL)
            .count(),
        1
    );
    let results: Vec<_> = input
        .iter()
        .filter(|item| item["type"] == "function_call_output" && item["call_id"] == CALL)
        .collect();
    assert_eq!(results.len(), 1);
    let output: Value = serde_json::from_str(results[0]["output"].as_str().unwrap()).unwrap();
    let tool_ok = output["status"] == "ok";
    if !tool_ok {
        assert!(output.to_string().contains(DENIAL));
        assert!(output.to_string().contains("hook_denied"));
    }
    let outcomes: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            RuntimeEvent::HookCompleted { event, outcome, .. } if event == "PreToolUse" => {
                Some(outcome.as_str())
            }
            _ => None,
        })
        .collect();
    assert_eq!(outcomes.len(), hooks.len());
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| **outcome == "blocked")
            .count(),
        hooks.iter().filter(|body| body["model"] == PRO).count()
    );
    Observation {
        hook_models: hooks
            .iter()
            .map(|body| body["model"].as_str().unwrap().to_string())
            .collect(),
        tool_ok,
        body_reached_parent: parent[1].to_string().contains(CANARY),
    }
}

async fn observe(case: Case, kind: &str) -> Observation {
    let temporary = tempfile::tempdir().unwrap();
    let workspace = temporary.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    std::fs::write(workspace.join("fixture.txt"), CANARY).unwrap();
    let db = Arc::new(Database::open(temporary.path().join("state.sqlite")).unwrap());
    let model = Arc::new(DedupModel::default());
    let chat = ChatService::new(
        db.clone(),
        settings(db.clone(), case, kind).await,
        model.clone(),
        &workspace,
    );
    let events = Arc::new(Mutex::new(Vec::new()));
    let captured = events.clone();
    tokio::time::timeout(
        Duration::from_secs(8),
        chat.run_in_session(
            "Read fixture.txt with the supplied tools and reply briefly.",
            None,
            None,
            None,
            Vec::new(),
            None,
            false,
            Some(RUN),
            move |event| captured.lock().unwrap().push(event),
            |_| {},
        ),
    )
    .await
    .expect("dedup audit must settle")
    .unwrap();
    let run = RunStore::new(&db).get(RUN).unwrap().unwrap();
    assert_eq!(run.terminal_kind.as_deref(), Some("succeeded"));
    assert_eq!(
        chat.run_events(RUN, None)
            .unwrap()
            .iter()
            .filter(|event| event.event_type == "run_terminal")
            .count(),
        1
    );
    assert_eq!(
        std::fs::read_to_string(workspace.join("fixture.txt")).unwrap(),
        CANARY
    );
    let observed = summarize(&model, &events.lock().unwrap());
    eprintln!("Hook-dedup audit: case={case:?} kind={kind} observed={observed:?}");
    drop(chat);
    drop(db);
    let reopened = Database::open(temporary.path().join("state.sqlite")).unwrap();
    let history = EventStore::new(&reopened)
        .load_session(SessionId::from_str(&run.session_id).unwrap())
        .unwrap();
    assert_eq!(
        history
            .iter()
            .filter(|event| matches!(&event.payload,
        EventPayload::ToolCallRequested {call} if call.id == CALL))
            .count(),
        1
    );
    assert_eq!(history.iter().filter(|event| matches!(&event.payload,
        EventPayload::ToolCallCompleted {call_id, ok, ..} if call_id == CALL && *ok == observed.tool_ok)).count(), 1);
    assert_eq!(RunStore::new(&reopened).get(RUN).unwrap(), Some(run));
    assert_eq!(
        RunStore::new(&reopened)
            .events_after(RUN, None)
            .unwrap()
            .iter()
            .filter(|event| event.event_type == "run_terminal")
            .count(),
        1
    );
    assert_eq!(
        model.hooks.lock().unwrap().len(),
        observed.hook_models.len()
    );
    observed
}

async fn verify_control(case: Case, models: &[&str], allowed: bool) {
    for kind in ["prompt", "agent"] {
        let observed = observe(case, kind).await;
        let mut actual = observed.hook_models;
        actual.sort();
        let mut expected: Vec<_> = models.iter().map(|model| model.to_string()).collect();
        expected.sort();
        assert_eq!(actual, expected);
        assert_eq!(observed.tool_ok, allowed);
        assert_eq!(observed.body_reached_parent, allowed);
    }
}

#[tokio::test]
async fn hook_dedup_identical_policy_runs_once() {
    verify_control(Case::Identical, &[FLASH], true).await;
}

#[tokio::test]
async fn hook_dedup_single_deny_blocks_read() {
    verify_control(Case::SingleDeny, &[PRO], false).await;
}

#[tokio::test]
async fn hook_dedup_distinct_prompt_preserves_model_policies() {
    verify_control(Case::DifferentPrompt, &[FLASH, PRO], false).await;
}

#[tokio::test]
async fn hook_dedup_distinct_prompt_preserves_applicable_condition() {
    verify_control(Case::DifferentConditionPrompt, &[PRO], false).await;
}

#[tokio::test]
async fn hook_dedup_applicable_condition_first_blocks_read() {
    verify_control(Case::ReversedConditions, &[PRO], false).await;
}

#[tokio::test]
async fn hook_dedup_unmatched_condition_does_not_execute() {
    verify_control(Case::Unmatched, &[], true).await;
}

#[tokio::test]
async fn hook_dedup_different_model_must_not_drop_second_policy() {
    let mut observations = Vec::new();
    for kind in ["prompt", "agent"] {
        observations.push(observe(Case::ModelOnly, kind).await);
    }
    assert!(
        observations.iter().all(|observed| !observed.tool_ok
            && !observed.body_reached_parent
            && observed.hook_models.len() == 2),
        "distinct second-model deny must reach both tool gates: {observations:?}"
    );
}

#[tokio::test]
async fn hook_dedup_different_condition_must_not_drop_applicable_policy() {
    let mut observations = Vec::new();
    for kind in ["prompt", "agent"] {
        observations.push(observe(Case::ConditionOnly, kind).await);
    }
    assert!(
        observations.iter().all(|observed| !observed.tool_ok
            && !observed.body_reached_parent
            && observed.hook_models == vec![PRO]),
        "applicable deny must survive a prior non-applicable declaration: {observations:?}"
    );
}
