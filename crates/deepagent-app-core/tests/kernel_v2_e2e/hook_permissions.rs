//! Compare parent Read/sandbox policy with the same tool in an agent hook.
//! Only model transport is synthetic; policy assembly and file tools are real.

use super::responses_fixture::{call, completed, text};
use super::*;
use deepagent_hooks::PermissionRules;
use deepagent_models::ModelRole;
use deepagent_persistence::run_store::RunStore;
use deepagent_runtime::RuntimeEvent;
use serde_json::{json, Value};
use std::time::Duration;

const MODEL: &str = "deepseek-flash";
const MARKER: &str = "AUDIT_AGENT_HOOK_PERMISSION";
const CANARY: &str = "SYNTHETIC_HOOK_READ_CANARY";
const RUN: &str = "audit-hook-permissions";
const PARENT_READ: &str = "parent-policy-read";
const PARENT_LIST: &str = "parent-policy-list";
const HOOK_READ: &str = "hook-policy-read";

struct PolicyModel {
    path: String,
    parent: Mutex<Vec<Value>>,
    hooks: Mutex<Vec<Value>>,
}

#[async_trait]
impl HttpTransport for PolicyModel {
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
        let item = if hook {
            match index {
                0 => call(HOOK_READ, "read_file", json!({"path":self.path})),
                1 => text("hook-policy-decision", r#"{"ok":true}"#),
                _ => panic!("unexpected hook request {index}"),
            }
        } else {
            match index {
                0 => call(PARENT_READ, "read_file", json!({"path":self.path})),
                1 => call(PARENT_LIST, "task_list", json!({})),
                2 => text("parent-policy-final", "Offline policy comparison finished."),
                _ => panic!("unexpected parent request {index}"),
            }
        };
        emit_responses_events(
            sink,
            &completed(
                &format!("policy-response-{hook}-{index}"),
                MODEL,
                item,
                json!({"input_tokens":12,"output_tokens":6,"total_tokens":18}),
            ),
        )
    }
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
    let outputs: Vec<_> = input
        .iter()
        .filter(|item| item["type"] == "function_call_output" && item["call_id"] == id)
        .collect();
    assert_eq!(outputs.len(), 1, "one result for {id}");
    serde_json::from_str(outputs[0]["output"].as_str().unwrap()).unwrap()
}

async fn configured(db: Arc<Database>, deny: bool, matched: bool) -> Arc<SettingsService> {
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
    if deny {
        settings
            .set_permission_rules(PermissionRules::new([], [], ["Read".to_string()]))
            .unwrap();
    }
    settings
        .set_hooks_json(
            &json!({"hooks":{"PreToolUse":[{
                "matcher":if matched {"task_list"} else {"never_called_tool"},
                "hooks":[{"type":"agent","prompt":MARKER,"timeout":5}]
            }]}})
            .to_string(),
        )
        .unwrap();
    settings
}

#[derive(Debug)]
struct Observation {
    parent: Value,
    hook: Option<Value>,
    hook_has_canary: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FileCase {
    Ordinary,
    Sensitive,
    Outside,
}

async fn observe(deny: bool, matched: bool, file_case: FileCase) -> Observation {
    let temporary = tempfile::tempdir().unwrap();
    let workspace = temporary.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let path = match file_case {
        FileCase::Ordinary => workspace.join("fixture.txt"),
        FileCase::Sensitive => workspace.join(".env"),
        FileCase::Outside => temporary.path().join("outside.txt"),
    };
    std::fs::write(&path, CANARY).unwrap();
    let argument = match file_case {
        FileCase::Outside => path.to_string_lossy().into_owned(),
        _ => path.file_name().unwrap().to_string_lossy().into_owned(),
    };
    let db = Arc::new(Database::open(temporary.path().join("state.sqlite")).unwrap());
    let settings = configured(db.clone(), deny, matched).await;
    let model = Arc::new(PolicyModel {
        path: argument,
        parent: Mutex::new(Vec::new()),
        hooks: Mutex::new(Vec::new()),
    });
    let chat = ChatService::new(db.clone(), settings, model.clone(), &workspace);
    let events = Arc::new(Mutex::new(Vec::new()));
    let captured = events.clone();
    tokio::time::timeout(
        Duration::from_secs(8),
        chat.run_in_session(
            "Inspect the fixture, then list tasks and reply briefly.",
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
    .expect("formal policy workflow must finish")
    .unwrap();
    assert_eq!(
        RunStore::new(&db)
            .get(RUN)
            .unwrap()
            .unwrap()
            .terminal_kind
            .as_deref(),
        Some("succeeded")
    );
    assert_eq!(
        chat.run_events(RUN, None)
            .unwrap()
            .iter()
            .filter(|event| event.event_type == "run_terminal")
            .count(),
        1
    );
    let parent = model.parent.lock().unwrap();
    let hooks = model.hooks.lock().unwrap();
    assert_eq!(parent.len(), 3);
    assert_eq!(hooks.len(), if matched { 2 } else { 0 });
    if matched {
        let tools = hooks[0]["tools"].as_array().unwrap();
        assert!(!tools.iter().any(|tool| tool["name"] == "write_file"));
        if deny || file_case == FileCase::Outside {
            assert!(!hooks[0]["input"].to_string().contains(CANARY));
        }
    }
    assert_eq!(output(&parent[2], PARENT_LIST)["status"], "ok");
    assert_eq!(events.lock().unwrap().iter().filter(|event| matches!(event,
        RuntimeEvent::HookCompleted { event, outcome, .. } if event == "PreToolUse" && outcome == "continued"
    )).count(), usize::from(matched));
    let parent_output = output(&parent[1], PARENT_READ);
    if deny {
        assert_eq!(parent_output["status"], "error");
        assert!(parent_output
            .to_string()
            .contains("denied by permission rule"));
        assert!(!parent[1]["input"].to_string().contains(CANARY));
        assert!(!parent[2]["input"].to_string().contains(CANARY));
    }
    if !deny && file_case == FileCase::Outside {
        assert_eq!(parent_output["status"], "error");
        assert!(parent_output
            .to_string()
            .contains("blocked by the current sandbox mode"));
        assert!(!parent[1]["input"].to_string().contains(CANARY));
        assert!(!parent[2]["input"].to_string().contains(CANARY));
    }
    let hook = matched.then(|| output(&hooks[1], HOOK_READ));
    let hook_has_canary = matched && hooks[1]["input"].to_string().contains(CANARY);
    let observation = Observation {
        parent: parent_output,
        hook,
        hook_has_canary,
    };
    eprintln!("Hook policy audit: deny={deny} matched={matched} file={file_case:?} observation={observation:?}");
    observation
}

#[tokio::test]
async fn hook_permissions_unmatched_hook_keeps_parent_deny() {
    let observed = observe(true, false, FileCase::Ordinary).await;
    assert!(observed.hook.is_none());
    assert!(!observed.hook_has_canary);
}

#[tokio::test]
async fn hook_permissions_builtin_sensitive_guard_still_applies() {
    let observed = observe(true, true, FileCase::Sensitive).await;
    assert_eq!(observed.hook.as_ref().unwrap()["status"], "error");
    assert!(observed
        .hook
        .unwrap()
        .to_string()
        .contains("sensitive file"));
    assert!(!observed.hook_has_canary);
}

#[tokio::test]
async fn hook_permissions_unmatched_hook_keeps_parent_workspace_boundary() {
    let observed = observe(false, false, FileCase::Outside).await;
    assert_eq!(observed.parent["status"], "error");
    assert!(observed.hook.is_none());
    assert!(!observed.hook_has_canary);
}

#[tokio::test]
#[ignore = "audit reproducer: agent hook must retain the parent's workspace boundary"]
async fn hook_permissions_agent_inherits_parent_workspace_boundary() {
    let observed = observe(false, true, FileCase::Outside).await;
    assert!(
        !observed.hook_has_canary,
        "sandbox-denied data must not reach the hook model"
    );
    assert_eq!(observed.hook.unwrap()["status"], "error");
}

#[tokio::test]
async fn hook_permissions_without_rules_can_read_normally() {
    let observed = observe(false, true, FileCase::Ordinary).await;
    assert_eq!(observed.parent["status"], "ok");
    assert!(observed.parent.to_string().contains(CANARY));
    assert_eq!(observed.hook.as_ref().unwrap()["status"], "ok");
    // Shared ReadFile cache may emit an unchanged stub after the parent's read.
}

#[tokio::test]
#[ignore = "audit reproducer: agent hook must retain the parent's explicit Read deny"]
async fn hook_permissions_agent_inherits_explicit_parent_deny() {
    let observed = observe(true, true, FileCase::Ordinary).await;
    assert!(
        !observed.hook_has_canary,
        "the denied file must not reach the hook model"
    );
    assert_eq!(observed.hook.unwrap()["status"], "error");
}
