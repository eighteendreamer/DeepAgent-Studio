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
    hook_tool: &'static str,
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
                0 => call(
                    HOOK_READ,
                    self.hook_tool,
                    if self.hook_tool == "write_file" {
                        json!({"path":self.path,"content":"UNAUTHORIZED_REPLACEMENT"})
                    } else {
                        json!({"path":self.path})
                    },
                ),
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

async fn configured(db: Arc<Database>, case: &PolicyCase) -> Arc<SettingsService> {
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
    settings.set_active_permission_preset(case.preset).unwrap();
    settings.set_permission_rules(case.rules.clone()).unwrap();
    settings
        .set_hooks_json(
            &json!({"hooks":{case.event:[{
                "matcher":if case.matched {"task_list"} else {"never_called_tool"},
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
    parent_has_canary: bool,
    approvals: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FileCase {
    Ordinary,
    Sensitive,
    Outside,
}

struct PolicyCase {
    rules: PermissionRules,
    preset: PermissionPreset,
    file: FileCase,
    matched: bool,
    event: &'static str,
    hook_tool: &'static str,
    project_config: Option<Value>,
}

impl Default for PolicyCase {
    fn default() -> Self {
        Self {
            rules: PermissionRules::default(),
            preset: PermissionPreset::Default,
            file: FileCase::Ordinary,
            matched: true,
            event: "PreToolUse",
            hook_tool: "read_file",
            project_config: None,
        }
    }
}

async fn observe(deny: bool, matched: bool, file_case: FileCase) -> Observation {
    let observed = observe_case(PolicyCase {
        rules: PermissionRules::new(
            [],
            [],
            if deny {
                vec!["Read".to_string()]
            } else {
                vec![]
            },
        ),
        matched,
        file: file_case,
        ..Default::default()
    })
    .await;
    if deny {
        assert_eq!(observed.parent["status"], "error");
        assert!(observed
            .parent
            .to_string()
            .contains("denied by permission rule"));
        assert!(!observed.parent_has_canary);
    }
    if !deny && file_case == FileCase::Outside {
        assert_eq!(observed.parent["status"], "error");
        assert!(observed
            .parent
            .to_string()
            .contains("blocked by the current sandbox mode"));
        assert!(!observed.parent_has_canary);
    }
    observed
}

async fn observe_case(case: PolicyCase) -> Observation {
    let temporary = tempfile::tempdir().unwrap();
    let workspace = temporary.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let path = match case.file {
        FileCase::Ordinary => workspace.join("fixture.txt"),
        FileCase::Sensitive => workspace.join(".env"),
        FileCase::Outside => temporary.path().join("outside.txt"),
    };
    std::fs::write(&path, CANARY).unwrap();
    let argument = match case.file {
        FileCase::Outside => path.to_string_lossy().into_owned(),
        _ => path.file_name().unwrap().to_string_lossy().into_owned(),
    };
    let db = Arc::new(Database::open(temporary.path().join("state.sqlite")).unwrap());
    if let Some(config) = &case.project_config {
        let config_dir = workspace.join(".deepagent");
        std::fs::create_dir(&config_dir).unwrap();
        std::fs::write(config_dir.join("settings.json"), config.to_string()).unwrap();
    }
    let settings = configured(db.clone(), &case).await;
    let model = Arc::new(PolicyModel {
        path: argument,
        hook_tool: case.hook_tool,
        parent: Mutex::new(Vec::new()),
        hooks: Mutex::new(Vec::new()),
    });
    let chat = ChatService::new(db.clone(), settings, model.clone(), &workspace);
    let events = Arc::new(Mutex::new(Vec::new()));
    let captured = events.clone();
    let approvals = Arc::new(AtomicUsize::new(0));
    let approval_count = approvals.clone();
    let approval_chat = chat.clone();
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
            move |request| {
                approval_count.fetch_add(1, Ordering::SeqCst);
                assert!(approval_chat
                    .resolve_approval(&request.call_id, false, "h237_test_user")
                    .unwrap());
            },
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
    assert_eq!(hooks.len(), if case.matched { 2 } else { 0 });
    if case.matched {
        let tools = hooks[0]["tools"].as_array().unwrap();
        assert!(!tools.iter().any(|tool| tool["name"] == "write_file"));
        assert!(!hooks[0]["input"].to_string().contains(CANARY));
    }
    assert_eq!(output(&parent[2], PARENT_LIST)["status"], "ok");
    assert_eq!(events.lock().unwrap().iter().filter(|event| matches!(event,
        RuntimeEvent::HookCompleted { event, outcome, .. } if event == case.event && outcome == "continued"
    )).count(), usize::from(case.matched));
    let parent_output = output(&parent[1], PARENT_READ);
    let hook = case.matched.then(|| output(&hooks[1], HOOK_READ));
    let hook_has_canary = case.matched && hooks[1]["input"].to_string().contains(CANARY);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), CANARY);
    let observation = Observation {
        parent: parent_output,
        hook,
        hook_has_canary,
        parent_has_canary: parent
            .iter()
            .any(|body| body["input"].to_string().contains(CANARY)),
        approvals: approvals.load(Ordering::SeqCst),
    };
    eprintln!(
        "Hook policy audit: file={:?} event={} observation={observation:?}",
        case.file, case.event
    );
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
    // No explicit deny: exercise the sensitive-path policy independently.
    let observed = observe(false, true, FileCase::Sensitive).await;
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
async fn hook_permissions_agent_inherits_explicit_parent_deny() {
    let observed = observe(true, true, FileCase::Ordinary).await;
    assert!(
        !observed.hook_has_canary,
        "the denied file must not reach the hook model"
    );
    assert_eq!(observed.hook.unwrap()["status"], "error");
}

#[tokio::test]
async fn hook_permissions_ask_is_denied_without_child_interaction_even_with_full_access_parent() {
    for preset in [PermissionPreset::Default, PermissionPreset::FullAccess] {
        let observed = observe_case(PolicyCase {
            rules: PermissionRules::new([], ["Read".to_string()], []),
            preset,
            ..Default::default()
        })
        .await;
        assert_eq!(
            observed.approvals,
            usize::from(preset == PermissionPreset::Default)
        );
        assert_eq!(
            observed.parent["status"],
            if preset == PermissionPreset::FullAccess {
                "ok"
            } else {
                "error"
            }
        );
        assert_eq!(observed.hook.as_ref().unwrap()["status"], "error");
        assert!(observed
            .hook
            .unwrap()
            .to_string()
            .contains("approval denied"));
        assert!(!observed.hook_has_canary);
    }
}

#[tokio::test]
async fn hook_permissions_full_access_allows_authorized_outside_reads_but_not_sensitive_files() {
    for file in [FileCase::Outside, FileCase::Sensitive] {
        let observed = observe_case(PolicyCase {
            file,
            preset: PermissionPreset::FullAccess,
            ..Default::default()
        })
        .await;
        assert_eq!(
            observed.parent["status"],
            if file == FileCase::Outside {
                "ok"
            } else {
                "error"
            }
        );
        assert_eq!(
            observed.hook.as_ref().unwrap()["status"],
            observed.parent["status"]
        );
        assert_eq!(observed.approvals, 0);
    }
}

#[tokio::test]
async fn hook_permissions_project_deny_survives_ui_allow_and_post_tool_event() {
    for event in ["PreToolUse", "PostToolUse"] {
        let observed = observe_case(PolicyCase {
            rules: PermissionRules::new(["Read".to_string()], [], []),
            project_config: Some(json!({"permissions":{"deny":["Read"]}})),
            event,
            ..Default::default()
        })
        .await;
        assert_eq!(observed.parent["status"], "error");
        assert_eq!(observed.hook.as_ref().unwrap()["status"], "error");
        assert!(observed
            .hook
            .unwrap()
            .to_string()
            .contains("denied by permission rule"));
        assert!(!observed.hook_has_canary && !observed.parent_has_canary);
        assert_eq!(observed.approvals, 0);
    }
}

#[tokio::test]
async fn hook_permissions_enforces_policy_on_other_safe_tools_and_rejects_forced_hidden_write() {
    for tool in ["list_dir", "write_file"] {
        let observed = observe_case(PolicyCase {
            rules: PermissionRules::new([], [], ["Ls".to_string()]),
            hook_tool: tool,
            ..Default::default()
        })
        .await;
        assert_eq!(observed.parent["status"], "ok");
        assert_eq!(observed.hook.as_ref().unwrap()["status"], "error");
        let expected = if tool == "list_dir" {
            "denied by permission rule"
        } else {
            "unknown tool"
        };
        assert!(observed.hook.unwrap().to_string().contains(expected));
        assert!(!observed.hook_has_canary);
        assert_eq!(observed.approvals, 0);
    }
}
