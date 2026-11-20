//! File bytes may be cached globally, but seen-content proof belongs to a context.
//! Exercise real file tools across parent and isolated hook model histories.

use super::responses_fixture::{call, completed, text};
use super::*;
use deepagent_persistence::run_store::RunStore;
use deepagent_runtime::RuntimeEvent;
use serde_json::{json, Value};
use std::time::Duration;

const MODEL: &str = "deepseek-flash";
const MARKER: &str = "AUDIT_HOOK_FILE_CACHE";
const CANARY: &str = "SYNTHETIC_FILE_BODY_NOT_IN_PROMPT";
const REPLACEMENT: &str = "SYNTHETIC_REPLACEMENT_WITHOUT_SEEING_ORIGINAL";
const RUN: &str = "audit-hook-file-cache";
const FIRST: &str = "cache-parent-first";
const SECOND: &str = "cache-parent-second";
const HOOK_READ: &str = "cache-hook-read";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Order {
    ParentThenHook,
    HookThenParent,
    ParentRepeat,
    HookOnly,
    HookThenWrite,
    NoHookThenWrite,
    ParentReadThenWrite,
    ParentReadThenHookThenWrite,
}

struct CacheModel {
    order: Order,
    explicit_hook_offset: bool,
    parent: Mutex<Vec<Value>>,
    hooks: Mutex<Vec<Value>>,
}

impl CacheModel {
    fn parent_item(&self, index: usize) -> Value {
        let read = |id| call(id, "read_file", json!({"path":"fixture.txt"}));
        let list = |id| call(id, "task_list", json!({}));
        match (self.order, index) {
            (
                Order::ParentThenHook
                | Order::ParentRepeat
                | Order::ParentReadThenWrite
                | Order::ParentReadThenHookThenWrite,
                0,
            ) => read(FIRST),
            (
                Order::HookThenParent
                | Order::HookOnly
                | Order::HookThenWrite
                | Order::NoHookThenWrite,
                0,
            ) => list(FIRST),
            (Order::ParentThenHook | Order::ParentReadThenHookThenWrite, 1) => list(SECOND),
            (Order::HookThenParent | Order::ParentRepeat, 1) => read(SECOND),
            (Order::HookThenWrite | Order::NoHookThenWrite | Order::ParentReadThenWrite, 1) => {
                call(
                    SECOND,
                    "write_file",
                    json!({"path":"fixture.txt","content":REPLACEMENT}),
                )
            }
            (Order::ParentReadThenHookThenWrite, 2) => call(
                "cache-parent-write",
                "write_file",
                json!({"path":"fixture.txt","content":REPLACEMENT}),
            ),
            (Order::HookOnly, 1) | (_, 2) | (Order::ParentReadThenHookThenWrite, 3) => {
                text("cache-parent-final", "Offline read audit complete.")
            }
            _ => panic!("unexpected parent request {index}"),
        }
    }
}

#[async_trait]
impl HttpTransport for CacheModel {
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
                0 => {
                    if self.order == Order::ParentReadThenHookThenWrite {
                        call(HOOK_READ, "task_list", json!({}))
                    } else {
                        let mut arguments = json!({"path":"fixture.txt"});
                        if self.explicit_hook_offset {
                            arguments["offset"] = json!(1);
                        }
                        call(HOOK_READ, "read_file", arguments)
                    }
                }
                1 => text("cache-hook-decision", r#"{"ok":true}"#),
                _ => panic!("unexpected hook request {index}"),
            }
        } else {
            self.parent_item(index)
        };
        emit_responses_events(
            sink,
            &completed(
                &format!("cache-response-{hook}-{index}"),
                MODEL,
                item,
                json!({"input_tokens":12,"output_tokens":6,"total_tokens":18}),
            ),
        )
    }
}

fn wire_output(request: &Value, id: &str) -> Value {
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
    assert_eq!(outputs.len(), 1);
    serde_json::from_str(outputs[0]["output"].as_str().unwrap()).unwrap()
}

fn output(request: &Value, id: &str) -> Value {
    let output = wire_output(request, id);
    assert_eq!(output["status"], "ok");
    output["result"].clone()
}

fn assert_cached_body(result: &Value) {
    assert_eq!(
        result["cache_hit"], true,
        "reuse bytes, not another context's proof"
    );
    assert_ne!(result["unchanged_stub"], true);
    assert!(result["content"].as_str().unwrap().contains(CANARY));
}

async fn configured(db: Arc<Database>, enable_hook: bool) -> Arc<SettingsService> {
    let settings = Arc::new(SettingsService::new(
        db,
        Arc::new(MockTransport::with_get_json(
            r#"{"data":[{"id":"deepseek-flash","object":"model","owned_by":"deepseek"}]}"#,
        )),
        Arc::new(MemorySecretStore::new()),
    ));
    crate::registry_fixture::configure(&settings, "offline-audit-placeholder")
        .await
        .unwrap();
    settings
        .harness_models()
        .select_model("deepseek", MODEL)
        .unwrap();
    settings
        .set_active_permission_preset(PermissionPreset::Default)
        .unwrap();
    settings
        .set_hooks_json(
            &json!({"hooks":{"PreToolUse":[{
                "matcher":if enable_hook {"task_list"} else {"never_called_tool"},
                "hooks":[{"type":"agent","prompt":MARKER,"timeout":5}]
            }]}})
            .to_string(),
        )
        .unwrap();
    settings
}

#[derive(Debug)]
struct Observation {
    parent_body_seen: bool,
    hook_body_seen: bool,
    parent_first: Option<Value>,
    parent_second: Option<Value>,
    hook_read: Option<Value>,
    parent_write: Option<Value>,
    replacement_written: bool,
}

async fn observe(order: Order, explicit_hook_offset: bool) -> Observation {
    let temporary = tempfile::tempdir().unwrap();
    let workspace = temporary.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    std::fs::write(workspace.join("fixture.txt"), CANARY).unwrap();
    let db = Arc::new(Database::open(temporary.path().join("state.sqlite")).unwrap());
    let settings = configured(db.clone(), order != Order::NoHookThenWrite).await;
    let model = Arc::new(CacheModel {
        order,
        explicit_hook_offset,
        parent: Mutex::new(Vec::new()),
        hooks: Mutex::new(Vec::new()),
    });
    let chat = ChatService::new(db.clone(), settings, model.clone(), &workspace);
    let events = Arc::new(Mutex::new(Vec::new()));
    let captured = events.clone();
    tokio::time::timeout(
        Duration::from_secs(8),
        chat.run_in_session(
            "Inspect fixture.txt using the configured tools and reply briefly.",
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
    .expect("formal file-cache workflow must settle")
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
    let disk = std::fs::read_to_string(workspace.join("fixture.txt")).unwrap();
    let observed = summarize(order, &model, &events.lock().unwrap(), &disk);
    eprintln!("File-cache audit: order={order:?} explicit_hook_offset={explicit_hook_offset} observed={observed:?}");
    observed
}

fn summarize(order: Order, model: &CacheModel, events: &[RuntimeEvent], disk: &str) -> Observation {
    assert!(disk == CANARY || disk == REPLACEMENT);
    let replacement_written = disk == REPLACEMENT;
    let parent = model.parent.lock().unwrap();
    let hooks = model.hooks.lock().unwrap();
    let hooked = !matches!(
        order,
        Order::ParentRepeat | Order::NoHookThenWrite | Order::ParentReadThenWrite
    );
    assert_eq!(
        parent.len(),
        match order {
            Order::HookOnly => 2,
            Order::ParentReadThenHookThenWrite => 4,
            _ => 3,
        }
    );
    assert_eq!(hooks.len(), if hooked { 2 } else { 0 });
    assert!(!parent[0].to_string().contains(CANARY));
    if hooked {
        assert!(!hooks[0].to_string().contains(CANARY));
    }
    assert_eq!(events.iter().filter(|event| matches!(event,
        RuntimeEvent::HookCompleted { event, outcome, .. } if event == "PreToolUse" && outcome == "continued"
    )).count(), usize::from(hooked));
    let parent_first = matches!(
        order,
        Order::ParentThenHook
            | Order::ParentRepeat
            | Order::ParentReadThenWrite
            | Order::ParentReadThenHookThenWrite
    )
    .then(|| output(&parent[1], FIRST));
    let parent_second = matches!(order, Order::HookThenParent | Order::ParentRepeat)
        .then(|| output(&parent[2], SECOND));
    let hook_read = (hooked && order != Order::ParentReadThenHookThenWrite)
        .then(|| output(&hooks[1], HOOK_READ));
    let parent_write = matches!(
        order,
        Order::HookThenWrite
            | Order::NoHookThenWrite
            | Order::ParentReadThenWrite
            | Order::ParentReadThenHookThenWrite
    )
    .then(|| {
        if order == Order::ParentReadThenHookThenWrite {
            wire_output(&parent[3], "cache-parent-write")
        } else {
            wire_output(&parent[2], SECOND)
        }
    });
    Observation {
        parent_body_seen: parent.iter().any(|body| body.to_string().contains(CANARY)),
        hook_body_seen: hooks.iter().any(|body| body.to_string().contains(CANARY)),
        parent_first,
        parent_second,
        hook_read,
        parent_write,
        replacement_written,
    }
}

#[tokio::test]
async fn hook_file_cache_same_context_dedup_retains_prior_body() {
    let observed = observe(Order::ParentRepeat, false).await;
    assert!(observed.parent_body_seen);
    assert!(observed.parent_first.unwrap()["content"]
        .as_str()
        .unwrap()
        .contains(CANARY));
    assert_eq!(observed.parent_second.unwrap()["unchanged_stub"], true);
}

#[tokio::test]
async fn hook_file_cache_fresh_hook_receives_body() {
    let observed = observe(Order::HookOnly, false).await;
    assert!(observed.hook_body_seen);
    assert_eq!(observed.hook_read.unwrap()["cache_hit"], false);
}

#[tokio::test]
async fn hook_file_cache_different_range_receives_body() {
    let observed = observe(Order::ParentThenHook, true).await;
    assert!(observed.parent_body_seen && observed.hook_body_seen);
    assert_eq!(observed.hook_read.unwrap()["cache_hit"], true);
}

#[tokio::test]
async fn context_reset_in_readonly_hook_does_not_revoke_parent_read_authority() {
    let observed = observe(Order::ParentReadThenHookThenWrite, false).await;
    assert!(observed.parent_body_seen);
    assert!(!observed.hook_body_seen, "hook only called task_list");
    assert!(
        observed.replacement_written,
        "an unrelated nested context must not revoke the parent's observed file"
    );
    assert_eq!(observed.parent_write.unwrap()["status"], "ok");
}

#[tokio::test]
async fn hook_file_cache_parent_read_does_not_suppress_hook_body() {
    let observed = observe(Order::ParentThenHook, false).await;
    assert!(observed.parent_body_seen);
    assert!(
        observed.hook_body_seen,
        "first hook read must deliver content to its own history"
    );
    assert_cached_body(observed.hook_read.as_ref().unwrap());
    assert!(!observed.replacement_written);
}

#[tokio::test]
async fn hook_file_cache_hook_read_does_not_suppress_parent_body() {
    let observed = observe(Order::HookThenParent, false).await;
    assert!(observed.hook_body_seen);
    assert!(
        observed.parent_body_seen,
        "first parent read must deliver content to its own history"
    );
    assert_cached_body(observed.parent_second.as_ref().unwrap());
    assert!(!observed.replacement_written);
}

#[tokio::test]
async fn hook_file_cache_no_read_still_blocks_existing_file_write() {
    let observed = observe(Order::NoHookThenWrite, false).await;
    assert!(!observed.parent_body_seen && !observed.hook_body_seen);
    assert!(!observed.replacement_written);
    let output = observed.parent_write.unwrap();
    assert_eq!(output["status"], "error");
    assert!(output.to_string().contains("must call read_file"));
}

#[tokio::test]
async fn hook_file_cache_own_read_allows_existing_file_write() {
    let observed = observe(Order::ParentReadThenWrite, false).await;
    assert!(observed.parent_body_seen && !observed.hook_body_seen);
    assert!(observed.replacement_written);
    assert_eq!(observed.parent_write.unwrap()["status"], "ok");
}

#[tokio::test]
async fn hook_file_cache_hook_read_does_not_authorize_unseen_parent_write() {
    let observed = observe(Order::HookThenWrite, false).await;
    assert!(observed.hook_body_seen && !observed.parent_body_seen);
    assert!(
        !observed.replacement_written,
        "parent must not overwrite an existing file it has never seen"
    );
    let output = observed.parent_write.unwrap();
    assert_eq!(output["status"], "error");
    assert!(output.to_string().contains("must call read_file"));
}
