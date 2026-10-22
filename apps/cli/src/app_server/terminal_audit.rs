//! Exercise the real dispatcher/executor and durable read projection without network access.

use super::*;
use deepagent_app_core::{MemorySecretStore, SettingsService};
use deepagent_models::{MockTransport, ModelRole};
use serde_json::{json, Value};
use std::time::Duration;

struct Fixture {
    _temporary: tempfile::TempDir,
    database_path: PathBuf,
    workspace: PathBuf,
    settings: Arc<SettingsService>,
    state: Arc<Mutex<ServerState>>,
    events: Arc<Mutex<Vec<Value>>>,
    thread_id: String,
}

fn rpc(method: &str, params: Value) -> RpcRequest {
    RpcRequest {
        jsonrpc: "2.0".into(),
        id: json!(1),
        method: method.into(),
        params,
    }
}

fn dispatch(state: &mut ServerState, method: &str, params: Value) -> Value {
    let (response, launch) = state.dispatch(rpc(method, params), None);
    assert!(response.is_success(), "{method}: {response:?}");
    assert!(launch.is_none());
    response.result().clone()
}

fn initialize(state: &mut ServerState) {
    dispatch(
        state,
        "initialize",
        json!({"clientName":"terminal-audit", "clientVersion":"0.1.0",
            "protocolVersion":PROTOCOL_VERSION}),
    );
}

fn completed_text(text: &str) -> Vec<String> {
    vec![
        json!({"type":"response.output_text.delta","delta":text}).to_string(),
        json!({"type":"response.completed","response":{"status":"completed",
            "usage":{"input_tokens":12,"output_tokens":6,"total_tokens":18}}})
        .to_string(),
    ]
}

impl Fixture {
    async fn new(events: Vec<String>) -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let workspace = temporary.path().join("workspace");
        std::fs::create_dir(&workspace).unwrap();
        let database_path = temporary.path().join("state.sqlite");
        let database = Arc::new(Database::open(&database_path).unwrap());
        let transport = Arc::new(MockTransport {
            events,
            get_response: Some(
                json!({"data":[{"id":"deepseek-flash","object":"model",
                    "owned_by":"deepseek"}]})
                .to_string(),
            ),
        });
        let settings = Arc::new(SettingsService::new(
            database.clone(),
            transport.clone(),
            Arc::new(MemorySecretStore::new()),
        ));
        settings
            .initialize("offline-audit-placeholder")
            .await
            .unwrap();
        for role in [ModelRole::Chat, ModelRole::Reasoner] {
            settings.set_model(role, "deepseek-flash").unwrap();
        }
        let chat = ChatService::new(database, settings.clone(), transport, workspace.clone());
        let mut state = ServerState::new(
            chat,
            workspace.clone(),
            SandboxCapabilities {
                kind: deepagent_app_core::SandboxBackendKind::Direct,
                available: true,
                supports_one_shot: true,
                supports_interactive_pty: true,
                supports_network_toggle: false,
                supports_readonly_mapping: false,
                message: "offline audit backend".into(),
            },
        );
        initialize(&mut state);
        let started = dispatch(&mut state, "thread/start", json!({"cwd":workspace}));
        let thread_id = started["threadId"].as_str().unwrap().to_owned();
        Self {
            _temporary: temporary,
            database_path,
            workspace,
            settings,
            state: Arc::new(Mutex::new(state)),
            events: Arc::new(Mutex::new(Vec::new())),
            thread_id,
        }
    }

    fn enable_task_completed_hook(&self) {
        self.settings
            .set_hooks_json(
                &json!({"hooks":{"TaskCompleted":[{"hooks":[{
                    "type":"prompt", "prompt":"Return JSON with an ok boolean.", "timeout":5
                }]}]}})
                .to_string(),
            )
            .unwrap();
    }

    async fn run_turn(&self) -> String {
        let (response, launch) = self.state.lock().unwrap().dispatch(
            rpc(
                "turn/start",
                json!({"threadId":self.thread_id,"input":"Audit terminal projection."}),
            ),
            None,
        );
        assert!(response.is_success());
        let turn_id = response.result()["turnId"].as_str().unwrap().to_owned();
        let events = self.events.clone();
        let emitter: EventEmitter = Arc::new(move |event| {
            events
                .lock()
                .unwrap()
                .push(serde_json::to_value(event).unwrap());
        });
        spawn_turn(self.state.clone(), emitter, launch.unwrap());
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if !self.state.lock().unwrap().turns[&turn_id].active {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("wait for the entire executor, not just its first terminal notification");
        turn_id
    }

    fn read(&self) -> Value {
        dispatch(
            &mut self.state.lock().unwrap(),
            "thread/read",
            json!({"threadId":self.thread_id}),
        )
    }

    fn reopened_state(&self) -> ServerState {
        let database = Arc::new(Database::open(&self.database_path).unwrap());
        let mut state = ServerState::new_for_test_with_database(database, self.workspace.clone());
        initialize(&mut state);
        state
    }

    fn terminals(&self, turn_id: &str) -> Vec<String> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .filter(|event| event["turnId"] == turn_id && event["threadId"] == self.thread_id)
            .filter_map(|event| match event["type"].as_str() {
                Some(kind @ ("turn.completed" | "turn.failed" | "turn.interrupted")) => {
                    Some(kind.to_owned())
                }
                _ => None,
            })
            .collect()
    }
}

fn assert_durable_terminal(read: &Value, turn_id: &str, kind: &str) {
    let run = read["runEvents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|run| run["runId"] == turn_id)
        .expect("the real run must exist in thread/read");
    assert_eq!(run["terminalKind"], kind);
    let terminals: Vec<_> = run["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|event| event["event_type"] == "run_terminal")
        .collect();
    assert_eq!(terminals.len(), 1);
    assert_eq!(terminals[0]["data"]["kind"], kind);
}

#[tokio::test]
async fn successful_final_hook_has_one_matching_live_and_durable_terminal() {
    let fixture = Fixture::new(completed_text(r#"{"ok":true}"#)).await;
    fixture.enable_task_completed_hook();
    let turn_id = fixture.run_turn().await;
    assert_durable_terminal(&fixture.read(), &turn_id, "succeeded");
    assert_eq!(fixture.terminals(&turn_id), ["turn.completed"]);
    assert!(fixture.events.lock().unwrap().iter().any(|event| {
        event["item"]["eventType"] == "hook_completed"
            && event["item"]["data"]["event"] == "TaskCompleted"
            && event["item"]["data"]["outcome"] == "continued"
    }));
}

#[tokio::test]
async fn provider_error_has_one_matching_live_and_durable_terminal() {
    let fixture = Fixture::new(vec![json!({"type":"response.failed","response":{
        "status":"failed","error":{"message":"audit invalid request","code":"invalid_request_error"}
    }})
    .to_string()])
    .await;
    let turn_id = fixture.run_turn().await;
    assert_durable_terminal(&fixture.read(), &turn_id, "failed");
    assert_eq!(fixture.terminals(&turn_id), ["turn.failed"]);
}

#[tokio::test]
#[ignore = "audit: a failing final hook must not follow a completed turn notification"]
async fn audit_final_hook_error_has_one_matching_live_and_durable_terminal() {
    // This is valid assistant prose, but invalid JSON for a configured prompt hook.
    // The same offline transport serves both real model calls; no synthetic RuntimeEvent is emitted.
    let fixture = Fixture::new(completed_text("AUDIT_MAIN_COMPLETED")).await;
    fixture.enable_task_completed_hook();
    let turn_id = fixture.run_turn().await;
    let read = fixture.read();
    assert_durable_terminal(&read, &turn_id, "failed");
    assert!(fixture.events.lock().unwrap().iter().any(|event| {
        event["item"]["eventType"] == "hook_completed"
            && event["item"]["data"]["event"] == "TaskCompleted"
            && event["item"]["data"]["outcome"] == "error"
            && event["item"]["data"]["stderr"]
                .as_str()
                .unwrap()
                .contains("model-backed hook must return JSON")
    }));
    let run = read["runEvents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|run| run["runId"] == turn_id)
        .unwrap();
    let events = run["events"].as_array().unwrap();
    let completed = events
        .iter()
        .position(|event| event["event_type"] == "run_completed")
        .unwrap();
    let hook_error = events
        .iter()
        .position(|event| {
            event["event_type"] == "hook_completed" && event["data"]["event"] == "TaskCompleted"
        })
        .unwrap();
    let terminal = events
        .iter()
        .position(|event| event["event_type"] == "run_terminal")
        .unwrap();
    assert!(completed < hook_error && hook_error < terminal);
    let reopened = dispatch(
        &mut fixture.reopened_state(),
        "thread/read",
        json!({"threadId":fixture.thread_id}),
    );
    assert_eq!(reopened, read);
    let terminals = fixture.terminals(&turn_id);
    eprintln!("terminal audit: live={terminals:?}, durable=failed, durable_terminal_count=1");
    assert_eq!(terminals, ["turn.failed"]);
}

#[tokio::test]
async fn reopened_thread_read_keeps_session_and_each_run_cursor_independent() {
    let fixture = Fixture::new(completed_text("AUDIT_CURSOR_CONTROL")).await;
    let first_turn = fixture.run_turn().await;
    let second_turn = fixture.run_turn().await;
    let original = fixture.read();
    for turn_id in [&first_turn, &second_turn] {
        assert_durable_terminal(&original, turn_id, "succeeded");
        assert_eq!(fixture.terminals(turn_id), ["turn.completed"]);
    }

    // Open an independent reader on the same durable files, with no shared turn map.
    let mut reopened = fixture.reopened_state();
    let full = dispatch(
        &mut reopened,
        "thread/read",
        json!({"threadId":fixture.thread_id}),
    );
    assert_eq!(full, original);
    let session = full["events"].as_array().unwrap();
    let session_after = session[session.len() / 2]["streamSequence"]
        .as_u64()
        .unwrap();
    let runs = full["runEvents"].as_array().unwrap();
    assert_eq!(runs.len(), 2);
    let mut cursors = serde_json::Map::new();
    for run in runs {
        let events = run["events"].as_array().unwrap();
        let after = if run["runId"] == first_turn {
            events.last().unwrap()["streamSequence"].as_u64().unwrap()
        } else {
            events[events.len() / 2]["streamSequence"].as_u64().unwrap()
        };
        cursors.insert(run["runId"].as_str().unwrap().to_owned(), json!(after));
    }
    let incremental = dispatch(
        &mut reopened,
        "thread/read",
        json!({"threadId":fixture.thread_id, "afterSequence":u64::MAX,
            "sessionAfterSequence":session_after, "runAfterSequence":u64::MAX,
            "runAfterSequences":cursors}),
    );
    let expected_session: Vec<_> = session
        .iter()
        .filter(|event| event["streamSequence"].as_u64().unwrap() > session_after)
        .cloned()
        .collect();
    assert_eq!(incremental["events"], json!(expected_session));
    for run in incremental["runEvents"].as_array().unwrap() {
        let run_id = run["runId"].as_str().unwrap();
        let after = cursors[run_id].as_u64().unwrap();
        let original_run = runs
            .iter()
            .find(|original| original["runId"] == run_id)
            .unwrap();
        let expected: Vec<_> = original_run["events"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|event| event["streamSequence"].as_u64().unwrap() > after)
            .cloned()
            .collect();
        assert_eq!(run["events"], json!(expected));
        for event in run["events"].as_array().unwrap() {
            assert_eq!(event["stream"], "run");
            assert_eq!(event["runId"], run_id);
            assert_eq!(event["streamSequence"], event["sequence"]);
        }
    }
    eprintln!("cursor audit: two durable runs, independent cursors and file-backed reread matched");
}
