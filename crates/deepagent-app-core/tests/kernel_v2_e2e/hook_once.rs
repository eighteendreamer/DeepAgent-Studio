//! One-shot hooks must not be consumed by unrelated lifecycle dispatches.
//! The transport supplies decisions; the formal host performs every file operation.

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

const MODEL: &str = "deepseek-flash";
const MARKER: &str = "AUDIT_ONCE_HOOK";
const DENIAL: &str = "synthetic once-hook denial";
const PRIMER: &str = "SYNTHETIC_UNRELATED_READ";
const WRITTEN: &str = "SYNTHETIC_WRITE_WITHOUT_ONCE_REVIEW";
const FIRST: &str = "once-first-tool";
const SECOND: &str = "once-second-tool";
const RUN: &str = "audit-once-hook";

#[derive(Clone, Copy, Debug)]
enum Case {
    Direct,
    MatcherOnce,
    MatcherRepeat,
    ConditionOnce,
    ConditionRepeat,
    RepeatedOnce,
    RepeatedAlways,
}

impl Case {
    fn once(self) -> bool {
        !matches!(
            self,
            Self::MatcherRepeat | Self::ConditionRepeat | Self::RepeatedAlways
        )
    }

    fn conditional(self) -> bool {
        matches!(self, Self::ConditionOnce | Self::ConditionRepeat)
    }

    fn first_is_write(self) -> bool {
        matches!(
            self,
            Self::Direct | Self::RepeatedOnce | Self::RepeatedAlways
        )
    }
}

struct OnceModel {
    case: Case,
    parent: Mutex<Vec<Value>>,
    hooks: Mutex<Vec<Value>>,
}

#[async_trait]
impl HttpTransport for OnceModel {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        let body: Value = serde_json::from_str(&request.body)?;
        assert_eq!(body["model"], MODEL);
        let hook = body["input"].to_string().contains(MARKER);
        let index = {
            let mut requests = if hook {
                self.hooks.lock().unwrap()
            } else {
                self.parent.lock().unwrap()
            };
            requests.push(body);
            requests.len() - 1
        };
        let write = |id| {
            call(
                id,
                "write_file",
                json!({"path":"target.txt","content":WRITTEN}),
            )
        };
        let item = if hook {
            text(
                &format!("once-decision-{index}"),
                &json!({"ok":false,"reason":DENIAL}).to_string(),
            )
        } else {
            match index {
                0 if self.case.first_is_write() => write(FIRST),
                0 => call(FIRST, "read_file", json!({"path":"primer.txt"})),
                1 if !matches!(self.case, Case::Direct) => write(SECOND),
                1 | 2 => text("once-parent-final", "Offline one-shot audit complete."),
                _ => panic!("unexpected parent request {index}"),
            }
        };
        emit_responses_events(
            sink,
            &completed(
                &format!("once-response-{hook}-{index}"),
                MODEL,
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
            r#"{"data":[{"id":"deepseek-flash","object":"model","owned_by":"deepseek"}]}"#,
        )),
        Arc::new(MemorySecretStore::new()),
    ));
    settings
        .initialize("offline-audit-placeholder")
        .await
        .unwrap();
    for role in [ModelRole::Chat, ModelRole::Reasoner] {
        settings.set_model(role, MODEL).unwrap();
    }
    settings
        .set_active_permission_preset(PermissionPreset::Default)
        .unwrap();
    let mut group = json!({"matcher":if case.conditional() {"*"} else {"write_file"},
        "once":case.once(),"hooks":[{"type":kind,"prompt":MARKER,"timeout":5}]});
    if case.conditional() {
        group["if"] = json!("write_file");
    }
    settings
        .set_hooks_json(&json!({"hooks":{"PreToolUse":[group]}}).to_string())
        .unwrap();
    assert_eq!(settings.hook_definitions().unwrap().action_count(), 1);
    settings
}

fn output(request: &Value, id: &str) -> Value {
    let input = request["input"].as_array().unwrap();
    assert_eq!(
        input
            .iter()
            .filter(|item| item["type"] == "function_call" && item["call_id"] == id)
            .count(),
        1
    );
    let results: Vec<_> = input
        .iter()
        .filter(|item| item["type"] == "function_call_output" && item["call_id"] == id)
        .collect();
    assert_eq!(results.len(), 1);
    let value: Value = serde_json::from_str(results[0]["output"].as_str().unwrap()).unwrap();
    if value["status"] == "error" {
        assert!(value.to_string().contains(DENIAL));
        assert!(value.to_string().contains("hook_denied"));
    } else {
        assert_eq!(value["status"], "ok");
    }
    value
}

#[derive(Debug)]
struct Observation {
    hook_calls: usize,
    first_ok: bool,
    second_ok: Option<bool>,
    target_written: bool,
}

fn summarize(model: &OnceModel, events: &[RuntimeEvent], target: &std::path::Path) -> Observation {
    let parent = model.parent.lock().unwrap();
    let hooks = model.hooks.lock().unwrap();
    assert_eq!(
        parent.len(),
        if matches!(model.case, Case::Direct) {
            2
        } else {
            3
        }
    );
    let first = output(&parent[1], FIRST);
    if !model.case.first_is_write() {
        assert_eq!(first["status"], "ok");
        assert!(first.to_string().contains(PRIMER));
    }
    let second_ok =
        (!matches!(model.case, Case::Direct)).then(|| output(&parent[2], SECOND)["status"] == "ok");
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
    assert!(outcomes.iter().all(|outcome| *outcome == "blocked"));
    if target.exists() {
        assert_eq!(std::fs::read_to_string(target).unwrap(), WRITTEN);
    }
    Observation {
        hook_calls: hooks.len(),
        first_ok: first["status"] == "ok",
        second_ok,
        target_written: target.exists(),
    }
}

async fn observe(case: Case, kind: &str) -> Observation {
    let temporary = tempfile::tempdir().unwrap();
    let workspace = temporary.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    std::fs::write(workspace.join("primer.txt"), PRIMER).unwrap();
    let target = workspace.join("target.txt");
    assert!(!target.exists());
    let db = Arc::new(Database::open(temporary.path().join("state.sqlite")).unwrap());
    let model = Arc::new(OnceModel {
        case,
        parent: Mutex::new(Vec::new()),
        hooks: Mutex::new(Vec::new()),
    });
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
            "Use the supplied tools for this controlled local workflow and reply briefly.",
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
    .expect("one-shot workflow must settle")
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
        std::fs::read_to_string(workspace.join("primer.txt")).unwrap(),
        PRIMER
    );
    let observed = summarize(&model, &events.lock().unwrap(), &target);
    eprintln!("Once-hook audit: case={case:?} kind={kind} observed={observed:?}");
    drop(chat);
    drop(db);
    let reopened = Database::open(temporary.path().join("state.sqlite")).unwrap();
    let history = EventStore::new(&reopened)
        .load_session(SessionId::from_str(&run.session_id).unwrap())
        .unwrap();
    for (id, ok) in [
        (FIRST, Some(observed.first_ok)),
        (SECOND, observed.second_ok),
    ] {
        let requests = history
            .iter()
            .filter(|event| {
                matches!(&event.payload,
            EventPayload::ToolCallRequested {call} if call.id == id)
            })
            .count();
        let results = history.iter().filter(|event| matches!(&event.payload,
            EventPayload::ToolCallCompleted {call_id, ok: actual, ..} if call_id == id && Some(*actual) == ok)).count();
        assert_eq!(requests, usize::from(ok.is_some()));
        assert_eq!(results, usize::from(ok.is_some()));
    }
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
    observed
}

async fn verify_control(
    case: Case,
    calls: usize,
    first_ok: bool,
    second_ok: Option<bool>,
    written: bool,
) {
    for kind in ["prompt", "agent"] {
        let observed = observe(case, kind).await;
        assert_eq!(observed.hook_calls, calls);
        assert_eq!(observed.first_ok, first_ok);
        assert_eq!(observed.second_ok, second_ok);
        assert_eq!(observed.target_written, written);
    }
}

#[tokio::test]
async fn hook_once_first_matching_write_is_blocked() {
    verify_control(Case::Direct, 1, false, None, false).await;
}

#[tokio::test]
async fn hook_once_without_once_matcher_survives_unrelated_read() {
    verify_control(Case::MatcherRepeat, 1, true, Some(false), false).await;
}

#[tokio::test]
async fn hook_once_without_once_condition_survives_unrelated_read() {
    verify_control(Case::ConditionRepeat, 1, true, Some(false), false).await;
}

#[tokio::test]
async fn hook_once_actual_execution_consumes_single_use() {
    verify_control(Case::RepeatedOnce, 1, false, Some(true), true).await;
}

#[tokio::test]
async fn hook_once_without_once_both_matching_writes_are_blocked() {
    verify_control(Case::RepeatedAlways, 2, false, Some(false), false).await;
}

async fn verify_unrelated_dispatch_does_not_consume(case: Case) {
    let mut observations = Vec::new();
    for kind in ["prompt", "agent"] {
        observations.push(observe(case, kind).await);
    }
    assert!(
        observations.iter().all(|observed| observed.hook_calls == 1
            && observed.first_ok
            && observed.second_ok == Some(false)
            && !observed.target_written),
        "unrelated read must not consume the one-shot write policy: {observations:?}"
    );
}

#[tokio::test]
async fn hook_once_matcher_mismatch_must_preserve_first_applicable_execution() {
    verify_unrelated_dispatch_does_not_consume(Case::MatcherOnce).await;
}

#[tokio::test]
async fn hook_once_condition_mismatch_must_preserve_first_applicable_execution() {
    verify_unrelated_dispatch_does_not_consume(Case::ConditionOnce).await;
}
