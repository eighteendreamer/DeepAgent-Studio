//! Post-tool hook failures must not erase an already-executed tool's durable result.

use super::*;
use deepagent_core::event::EventPayload;
use deepagent_core::id::SessionId;
use deepagent_models::ModelRole;
use deepagent_persistence::event_store::EventStore;
use deepagent_persistence::run_control::{RunActionState, RunControlStore};
use deepagent_persistence::run_store::RunStore;
use deepagent_runtime::events::RuntimeEvent;
use std::str::FromStr;
use std::time::Duration;

const CALL_ID: &str = "audit-post-hook-write";
const RUN_ID: &str = "audit-post-hook-run";
const CONTENT: &str = "audit-side-effect-was-committed\n";
const HOOK_MARKER: &str = "AUDIT_POST_TOOL_DECISION";

struct PostToolHookModel {
    malformed: bool,
    hook_calls: AtomicUsize,
    main_requests: Mutex<Vec<serde_json::Value>>,
}

#[async_trait]
impl HttpTransport for PostToolHookModel {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        let body: serde_json::Value = serde_json::from_str(&request.body).unwrap();
        let events = if body["input"].to_string().contains(HOOK_MARKER) {
            self.hook_calls.fetch_add(1, Ordering::SeqCst);
            response_text(if self.malformed {
                "Audit hook returned prose instead of a JSON decision."
            } else {
                r#"{"ok":true}"#
            })
        } else {
            let first = {
                let mut requests = self.main_requests.lock().unwrap();
                requests.push(body);
                requests.len() == 1
            };
            if first {
                response_function_call(
                    CALL_ID,
                    "write_file",
                    serde_json::json!({"path":"side-effect.txt","content":CONTENT}),
                )
            } else {
                response_text("Audit main request completed.")
            }
        };
        emit_responses_events(sink, &events)
    }
}

async fn settings(db: Arc<Database>) -> Arc<SettingsService> {
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
        settings.set_model(role, "deepseek-flash").unwrap();
    }
    settings
}

struct Observation {
    terminal: String,
    action_before_restart: RunActionState,
    action_after_recovery: RunActionState,
    durable_results: usize,
    replayed_output: serde_json::Value,
    actual_output: serde_json::Value,
}

async fn observe(malformed: bool) -> Observation {
    let temporary = tempfile::tempdir().unwrap();
    let workspace = temporary.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let database_path = temporary.path().join("state.sqlite");
    let model = Arc::new(PostToolHookModel {
        malformed,
        hook_calls: AtomicUsize::new(0),
        main_requests: Mutex::new(Vec::new()),
    });
    let (session_id, terminal, durable_results, actual_output, action_before_restart) = {
        let db = Arc::new(Database::open(&database_path).unwrap());
        let settings = settings(db.clone()).await;
        settings
            .set_hooks_json(
                &serde_json::json!({"hooks":{"PostToolUse":[{
                    "matcher":"write_file", "hooks":[{
                        "type":"prompt", "prompt":HOOK_MARKER, "timeout":5
                    }]
                }]}})
                .to_string(),
            )
            .unwrap();
        let chat = ChatService::new(db.clone(), settings, model.clone(), &workspace);
        let live_events = Arc::new(Mutex::new(Vec::new()));
        let captured = live_events.clone();
        let result = tokio::time::timeout(
            Duration::from_secs(8),
            chat.run_in_session(
                "Create side-effect.txt with the requested audit content.",
                None,
                None,
                None,
                Vec::new(),
                None,
                false,
                Some(RUN_ID),
                move |event| captured.lock().unwrap().push(event),
                |_| {},
            ),
        )
        .await
        .expect("the write and configured prompt hook must finish");
        if !malformed {
            result.as_ref().unwrap();
        }
        assert_eq!(model.hook_calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            std::fs::read_to_string(workspace.join("side-effect.txt")).unwrap(),
            CONTENT
        );
        let run = RunStore::new(&db).get(RUN_ID).unwrap().unwrap();
        let events = chat.run_events(RUN_ID, None).unwrap();
        assert_eq!(
            events
                .iter()
                .filter(|event| event.event_type == "run_terminal")
                .count(),
            1
        );
        let completions: Vec<_> = events
            .iter()
            .filter(|event| {
                event.event_type == "tool_completed" && event.data["call_id"] == CALL_ID
            })
            .collect();
        assert_eq!(completions.len(), 1);
        assert_eq!(completions[0].data["ok"], true);
        let actual_output = completions[0].data["output"].clone();
        assert!(
            live_events.lock().unwrap().iter().any(|event| matches!(
                event,
                RuntimeEvent::HookCompleted { event, outcome, .. }
                    if event == "PostToolUse"
                        && outcome == if malformed { "error" } else { "continued" }
            )),
            "the formal hook runtime must publish its outcome"
        );
        let history = EventStore::new(&db)
            .load_session(SessionId::from_str(&run.session_id).unwrap())
            .unwrap();
        let durable_results = history.iter().filter(|event| {
            matches!(&event.payload, EventPayload::ToolCallCompleted { call_id, .. } if call_id == CALL_ID)
        }).count();
        let actions = RunControlStore::new(&db).list_actions(RUN_ID).unwrap();
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].call_id, CALL_ID);
        eprintln!("post-tool hook first run: malformed={malformed}, error={}, terminal={:?}, tool_completed=1, durable_results={durable_results}, action={:?}, main_requests={}, hook_requests={}",
            result.as_ref().err().map(ToString::to_string).unwrap_or_default(), run.terminal_kind,
            actions[0].state,
            model.main_requests.lock().unwrap().len(), model.hook_calls.load(Ordering::SeqCst));
        (
            run.session_id,
            run.terminal_kind.unwrap(),
            durable_results,
            actual_output,
            actions[0].state,
        )
    };

    // All first-run services and database handles have been dropped. Inspect the
    // persisted action and resume through a fresh ChatService on the same files.
    let db = Arc::new(Database::open(&database_path).unwrap());
    let service = deepagent_app_core::AppService::from_shared(db.clone());
    let recovered_runs = service.recover_unfinished_runs().unwrap();
    assert!(
        recovered_runs.is_empty(),
        "the first run already reached a terminal"
    );
    let recovered_actions = RunControlStore::new(&db).list_actions(RUN_ID).unwrap();
    assert_eq!(recovered_actions.len(), 1);
    assert_eq!(recovered_actions[0].call_id, CALL_ID);
    let request_index = model.main_requests.lock().unwrap().len();
    let chat = ChatService::new(db.clone(), settings(db).await, model.clone(), &workspace);
    tokio::time::timeout(
        Duration::from_secs(8),
        chat.run_in_session(
            "Report the outcome of the preceding write without repeating it.",
            Some(&session_id),
            None,
            None,
            Vec::new(),
            None,
            false,
            Some("audit-post-hook-resume"),
            |_| {},
            |_| {},
        ),
    )
    .await
    .expect("the resumed run must finish")
    .unwrap();
    let requests = model.main_requests.lock().unwrap();
    assert_eq!(requests.len(), request_index + 1);
    let replayed_outputs: Vec<_> = requests[request_index]["input"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|item| item["type"] == "function_call_output" && item["call_id"] == CALL_ID)
        .collect();
    assert_eq!(
        replayed_outputs.len(),
        1,
        "replay must pair the original call"
    );
    let replayed_output: serde_json::Value =
        serde_json::from_str(replayed_outputs[0]["output"].as_str().unwrap()).unwrap();
    eprintln!("post-tool hook restart: malformed={malformed}, action_before={action_before_restart:?}, action_after={:?}, recovery_reason={:?}, replayed_output={replayed_output}",
        recovered_actions[0].state, recovered_actions[0].blocked_reason);
    Observation {
        terminal,
        action_before_restart,
        action_after_recovery: recovered_actions[0].state,
        durable_results,
        replayed_output,
        actual_output,
    }
}

fn assert_execution_was_committed(observed: Observation) {
    assert_eq!(
        (observed.action_before_restart, observed.durable_results, &observed.replayed_output["status"]),
        (RunActionState::Completed, 1, &serde_json::json!("ok")),
        "an already-successful write must remain completed and retain its real result after a post-tool hook failure"
    );
    assert_eq!(observed.action_after_recovery, RunActionState::Completed);
    assert_eq!(observed.replayed_output["result"], observed.actual_output);
}

#[tokio::test]
async fn valid_post_tool_hook_preserves_executed_write_across_restart() {
    let observed = observe(false).await;
    assert_eq!(observed.terminal, "succeeded");
    assert_execution_was_committed(observed);
}

#[tokio::test]
#[ignore = "audit: a malformed post-tool decision must not erase an executed write's durable result"]
async fn audit_post_tool_hook_error_preserves_executed_write_across_restart() {
    assert_execution_was_committed(observe(true).await);
}
