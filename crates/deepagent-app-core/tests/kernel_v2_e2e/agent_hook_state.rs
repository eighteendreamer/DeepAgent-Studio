//! A successful, isolated/read-only agent hook must not replace the parent's plan.
//! Only the provider transport is a fixture; tools and run assembly are production.

use super::responses_fixture::{call, completed, text};
use super::*;
use deepagent_core::event::EventPayload;
use deepagent_core::id::SessionId;
use deepagent_models::ModelRole;
use deepagent_persistence::event_store::EventStore;
use deepagent_persistence::run_store::RunStore;
use deepagent_runtime::events::RuntimeEvent;
use serde_json::{json, Value};
use std::str::FromStr;
use std::time::Duration;

const RUN: &str = "audit-hook-state-run";
const MARKER: &str = "AUDIT_AGENT_HOOK_STATE_DECISION";
const ORIGINAL: &str = "ORIGINAL_PARENT_TODO";
const CANARY: &str = "HOOK_TODO_CANARY";
const SEED_CALL: &str = "audit-parent-seed";
const READ_CALL: &str = "audit-parent-read";
const LIST_CALL: &str = "audit-parent-list";
const HOOK_CALL: &str = "audit-private-hook-tool";

fn todos(content: &str) -> Value {
    json!([{"content":content,"status":"pending","active_form":format!("Tracking {content}")}])
}

struct HookStateModel {
    write: bool,
    deny: bool,
    parent_requests: Mutex<Vec<Value>>,
    hook_requests: Mutex<Vec<Value>>,
}

#[async_trait]
impl HttpTransport for HookStateModel {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        let body: Value = serde_json::from_str(&request.body)?;
        let hook = body["input"].to_string().contains(MARKER);
        let index = {
            let mut requests = if hook {
                self.hook_requests.lock().unwrap()
            } else {
                self.parent_requests.lock().unwrap()
            };
            requests.push(body);
            requests.len() - 1
        };
        let item = if hook {
            match index {
                0 if self.write => call(HOOK_CALL, "todo_write", json!({"todos":todos(CANARY)})),
                0 => call(HOOK_CALL, "task_list", json!({})),
                1 => text(
                    "hook-decision",
                    if self.deny {
                        r#"{"ok":false,"reason":"controlled hook denial"}"#
                    } else {
                        r#"{"ok":true}"#
                    },
                ),
                _ => panic!("unexpected hook model continuation {index}"),
            }
        } else {
            match index {
                0 => call(SEED_CALL, "todo_write", json!({"todos":todos(ORIGINAL)})),
                1 => call(READ_CALL, "read_file", json!({"path":"audit.txt"})),
                2 => call(LIST_CALL, "task_list", json!({})),
                3 => text("parent-final", "Offline state isolation audit completed."),
                _ => panic!("unexpected parent model continuation {index}"),
            }
        };
        emit_responses_events(
            sink,
            &completed(
                &format!("response-{hook}-{index}"),
                "deepseek-flash",
                item,
                json!({"input_tokens":12,"output_tokens":6,"total_tokens":18}),
            ),
        )
    }
}

fn output(request: &Value, id: &str) -> Value {
    let outputs: Vec<_> = request["input"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|item| item["type"] == "function_call_output" && item["call_id"] == id)
        .collect();
    assert_eq!(outputs.len(), 1, "one wire result for {id}");
    assert_eq!(
        request["input"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|item| { item["type"] == "function_call" && item["call_id"] == id })
            .count(),
        1,
        "one paired wire call for {id}"
    );
    serde_json::from_str(outputs[0]["output"].as_str().unwrap()).unwrap()
}

async fn observe(write: bool, deny: bool) -> Value {
    let temporary = tempfile::tempdir().unwrap();
    let workspace = temporary.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    std::fs::write(workspace.join("audit.txt"), "AUDIT_FILE_CONTENT").unwrap();
    let db = Arc::new(Database::open(temporary.path().join("state.sqlite")).unwrap());
    let settings = Arc::new(SettingsService::new(
        db.clone(),
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
        settings.set_model(role, "deepseek-flash").unwrap();
    }
    settings
        .set_hooks_json(
            &json!({"hooks":{"PreToolUse":[{
                "matcher":"read_file","hooks":[{"type":"agent","prompt":MARKER,"timeout":5}]
            }]}})
            .to_string(),
        )
        .unwrap();
    let model = Arc::new(HookStateModel {
        write,
        deny,
        parent_requests: Mutex::new(Vec::new()),
        hook_requests: Mutex::new(Vec::new()),
    });
    let chat = ChatService::new(db.clone(), settings, model.clone(), &workspace);
    let events = Arc::new(Mutex::new(Vec::new()));
    let captured = events.clone();
    tokio::time::timeout(
        Duration::from_secs(8),
        chat.run_in_session(
            "Seed the parent plan, inspect audit.txt, then list the parent tasks.",
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
    .expect("formal agent-hook run must finish")
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
    let events = events.lock().unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event,
                RuntimeEvent::HookCompleted { event, outcome, .. }
                    if event == "PreToolUse" && outcome == if deny {"blocked"} else {"continued"}
            ))
            .count(),
        1
    );
    let history = EventStore::new(&db)
        .load_session(SessionId::from_str(&run.session_id).unwrap())
        .unwrap();
    for id in [SEED_CALL, READ_CALL, LIST_CALL] {
        assert_eq!(
            history
                .iter()
                .filter(|event| matches!(&event.payload,
                    EventPayload::ToolCallRequested { call } if call.id == id
                ))
                .count(),
            1
        );
        assert_eq!(
            history
                .iter()
                .filter(|event| matches!(&event.payload,
                    EventPayload::ToolCallCompleted { call_id, .. } if call_id == id
                ))
                .count(),
            1
        );
    }
    assert!(
        !history.iter().any(|event| match &event.payload {
            EventPayload::ToolCallRequested { call } => call.id == HOOK_CALL,
            EventPayload::ToolCallCompleted { call_id, .. } => call_id == HOOK_CALL,
            _ => false,
        }),
        "hook-private tools must not be attributed to the parent agent"
    );

    let parent = model.parent_requests.lock().unwrap();
    let hook = model.hook_requests.lock().unwrap();
    assert_eq!((parent.len(), hook.len()), (4, 2));
    assert!(hook[0].to_string().contains("supplied read-only tools"));
    assert_eq!(
        output(&parent[1], SEED_CALL)["result"]["todos"],
        todos(ORIGINAL)
    );
    let hook_output = output(&hook[1], HOOK_CALL);
    if !write {
        assert_eq!(hook_output["status"], "ok");
        assert_eq!(hook_output["result"]["todos"], todos(ORIGINAL));
    }
    let read_output = output(&parent[2], READ_CALL);
    assert_eq!(read_output["status"], if deny { "error" } else { "ok" });
    if deny {
        assert!(read_output.to_string().contains("controlled hook denial"));
        assert!(!read_output.to_string().contains("AUDIT_FILE_CONTENT"));
    } else {
        assert!(read_output.to_string().contains("AUDIT_FILE_CONTENT"));
    }
    let list_output = output(&parent[3], LIST_CALL);
    assert_eq!(list_output["status"], "ok");
    let advertised_write = hook[0]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .any(|tool| tool["name"] == "todo_write");
    eprintln!("agent-hook state: write={write}, deny={deny}, advertised_write={advertised_write}, hook_result={hook_output}, parent_list={}, read_status={}, terminal=succeeded, parent_hook_tool_records=0",
        list_output["result"]["todos"], read_output["status"]);
    list_output["result"]["todos"].clone()
}

#[tokio::test]
async fn read_only_agent_hook_allow_preserves_parent_plan() {
    assert_eq!(observe(false, false).await, todos(ORIGINAL));
}

#[tokio::test]
async fn read_only_agent_hook_deny_preserves_parent_plan() {
    assert_eq!(observe(false, true).await, todos(ORIGINAL));
}

#[tokio::test]
#[ignore = "audit: a successful isolated agent hook must not replace the parent plan"]
async fn audit_agent_hook_allow_preserves_parent_plan() {
    assert_eq!(observe(true, false).await, todos(ORIGINAL));
}

#[tokio::test]
#[ignore = "audit: denying the parent tool must not leave the hook's task list in the parent"]
async fn audit_agent_hook_deny_preserves_parent_plan() {
    assert_eq!(observe(true, true).await, todos(ORIGINAL));
}
