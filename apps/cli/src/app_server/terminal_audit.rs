//! Exercise the real dispatcher/executor and durable read projection without network access.

use super::*;
use deepagent_app_core::{ApprovalPolicy, MemorySecretStore, SettingsService};
use deepagent_core::error::Result as CoreResult;
use deepagent_models::transport::EventSink;
use deepagent_models::{HttpTransport, MockTransport, ModelRole, TransportRequest};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

struct ScriptedTransport {
    responses: Vec<Vec<String>>,
    next_response: AtomicUsize,
}

#[async_trait::async_trait]
impl HttpTransport for ScriptedTransport {
    async fn stream(&self, _request: TransportRequest, sink: &mut dyn EventSink) -> CoreResult<()> {
        let index = self.next_response.fetch_add(1, Ordering::Relaxed);
        if let Some(response) = self.responses.get(index).or_else(|| self.responses.last()) {
            for event in response {
                if sink.on_event(event)? {
                    break;
                }
            }
        }
        Ok(())
    }

    async fn get_json(&self, _url: &str, _api_key: &str) -> CoreResult<String> {
        Ok(json!({"data":[{"id":"deepseek-flash","object":"model",
            "owned_by":"deepseek"}]})
        .to_string())
    }
}

struct Fixture {
    _temporary: Option<tempfile::TempDir>,
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

fn function_call(call_id: &str, name: &str, arguments: Value) -> Vec<String> {
    let item = json!({
        "type":"function_call",
        "id":format!("item_{call_id}"),
        "call_id":call_id,
        "name":name,
        "arguments":serde_json::to_string(&arguments).unwrap()
    });
    vec![
        json!({"type":"response.output_item.added","item":item}).to_string(),
        json!({"type":"response.output_item.done","item":item}).to_string(),
        json!({"type":"response.completed","response":{"status":"completed"}}).to_string(),
    ]
}

impl Fixture {
    async fn new(events: Vec<String>) -> Self {
        let transport = Arc::new(MockTransport {
            events,
            get_response: Some(
                json!({"data":[{"id":"deepseek-flash","object":"model",
                    "owned_by":"deepseek"}]})
                .to_string(),
            ),
        });
        Self::with_transport(transport).await
    }

    async fn new_scripted(responses: Vec<Vec<String>>) -> Self {
        Self::with_transport(Arc::new(ScriptedTransport {
            responses,
            next_response: AtomicUsize::new(0),
        }))
        .await
    }

    async fn with_transport(transport: Arc<dyn HttpTransport>) -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().to_path_buf();
        Self::with_transport_root(transport, root, Some(temporary)).await
    }

    async fn with_transport_root(
        transport: Arc<dyn HttpTransport>,
        root: PathBuf,
        temporary: Option<tempfile::TempDir>,
    ) -> Self {
        std::fs::create_dir_all(&root).unwrap();
        let workspace = root.join("workspace");
        std::fs::create_dir(&workspace).unwrap();
        let database_path = root.join("state.sqlite");
        let database = Arc::new(Database::open(&database_path).unwrap());
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

struct BlockingTurnTransport {
    started: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    release: tokio::sync::Notify,
    marker_path: Option<PathBuf>,
}

#[async_trait::async_trait]
impl HttpTransport for BlockingTurnTransport {
    async fn stream(
        &self,
        _request: TransportRequest,
        _sink: &mut dyn EventSink,
    ) -> CoreResult<()> {
        if let Some(path) = &self.marker_path {
            std::fs::write(path, b"model request entered").unwrap();
        }
        if let Some(started) = self.started.lock().unwrap().take() {
            let _ = started.send(());
        }
        self.release.notified().await;
        Ok(())
    }

    async fn get_json(&self, _url: &str, _api_key: &str) -> CoreResult<String> {
        Ok(json!({"data":[{"id":"deepseek-flash","object":"model",
            "owned_by":"deepseek"}]})
        .to_string())
    }
}

#[test]
#[ignore = "audit H-92: stdio shutdown must finalize active turns before runtime exit"]
fn audit_runtime_shutdown_finalizes_in_flight_stdio_turn() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let transport = Arc::new(BlockingTurnTransport {
        started: Mutex::new(Some(started_tx)),
        release: tokio::sync::Notify::new(),
        marker_path: None,
    });
    let fixture = runtime.block_on(Fixture::with_transport(transport));
    let (response, launch) = fixture.state.lock().unwrap().dispatch(
        rpc(
            "turn/start",
            json!({"threadId":fixture.thread_id,"input":"Audit shutdown cleanup."}),
        ),
        None,
    );
    assert!(response.is_success());
    let turn_id = response.result()["turnId"].as_str().unwrap().to_owned();
    let events = fixture.events.clone();
    let emitter: EventEmitter = Arc::new(move |event| {
        events
            .lock()
            .unwrap()
            .push(serde_json::to_value(event).unwrap());
    });
    runtime.block_on(async {
        spawn_turn(fixture.state.clone(), emitter, launch.unwrap());
    });

    runtime.block_on(async {
        tokio::time::timeout(Duration::from_secs(5), started_rx)
            .await
            .expect("model request must start before shutdown")
            .expect("transport start signal");
    });

    let database = fixture.state.lock().unwrap().database.clone();
    let before = RunStore::new(&database)
        .get(&turn_id)
        .unwrap()
        .expect("accepted turn must be durable before its model request");
    assert!(before.finished_at.is_none());

    drop(runtime);

    let store = RunStore::new(&database);
    let after = store.get(&turn_id).unwrap().expect("run remains readable");
    let terminal_events = store
        .events_after(&turn_id, None)
        .unwrap()
        .iter()
        .filter(|event| event.event_type == "run_terminal")
        .count();
    eprintln!(
        "stdio shutdown audit: before={:?}, after={:?}, finished_at={:?}, terminal_events={terminal_events}",
        before.state, after.state, after.finished_at
    );
    assert!(
        after.finished_at.is_some() && terminal_events == 1,
        "runtime exit dropped the active turn before it wrote one durable terminal"
    );
}

const STDIO_CHILD_ROOT_ENV: &str = "DEEPAGENT_AUDIT_STDIO_CHILD_ROOT";

#[test]
#[ignore = "audit H-92: verify active-turn EOF behavior across a real child-process stdio pipe"]
fn audit_stdio_eof_child_entrypoint() {
    let Some(root) = std::env::var_os(STDIO_CHILD_ROOT_ENV).map(PathBuf::from) else {
        return;
    };
    let marker_path = root.join("model-request-entered");
    let transport = Arc::new(BlockingTurnTransport {
        started: Mutex::new(None),
        release: tokio::sync::Notify::new(),
        marker_path: Some(marker_path),
    });
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let fixture = runtime.block_on(Fixture::with_transport_root(transport, root, None));
    let chat = fixture.state.lock().unwrap().base_chat.clone();
    runtime
        .block_on(run_stdio(
            chat,
            fixture.workspace.clone(),
            SandboxCapabilities {
                kind: deepagent_app_core::SandboxBackendKind::Direct,
                available: true,
                supports_one_shot: true,
                supports_interactive_pty: true,
                supports_network_toggle: false,
                supports_readonly_mapping: false,
                message: "offline stdio audit backend".into(),
            },
        ))
        .unwrap();
}

#[test]
#[ignore = "audit H-92: stdio EOF must finalize an active turn before process exit"]
fn audit_stdio_eof_drops_active_turn_in_child_process() {
    use std::io::{BufRead, BufReader, Write};
    use std::process::{Command, Stdio};
    use std::sync::mpsc;
    use std::time::Instant;

    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().to_path_buf();
    let database_path = root.join("state.sqlite");
    let marker_path = root.join("model-request-entered");
    let executable = std::env::current_exe().unwrap();
    let mut child = Command::new(executable)
        .args([
            "--exact",
            "app_server::terminal_audit::audit_stdio_eof_child_entrypoint",
            "--ignored",
            "--nocapture",
        ])
        .env(STDIO_CHILD_ROOT_ENV, &root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let (line_tx, line_rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        loop {
            let mut line = String::new();
            match reader.read_line(&mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    if line_tx.send(line).is_err() {
                        break;
                    }
                }
            }
        }
    });

    let mut send = |id: u64, method: &str, params: Value| {
        let request = json!({"jsonrpc":"2.0", "id":id, "method":method, "params":params});
        writeln!(stdin, "{request}").unwrap();
        stdin.flush().unwrap();
    };
    let receive_response = |expected_id: u64| {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            assert!(
                Instant::now() < deadline,
                "timed out waiting for RPC {expected_id}"
            );
            let line = line_rx
                .recv_timeout(Duration::from_secs(1))
                .expect("child stdout should remain open while the server is active");
            let Ok(value) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if value["id"] == expected_id {
                return value;
            }
        }
    };

    send(
        1,
        "initialize",
        json!({"clientName":"h92-subprocess-audit", "clientVersion":"0.1.0",
            "protocolVersion":PROTOCOL_VERSION}),
    );
    assert!(receive_response(1)["result"].is_object());
    send(2, "thread/start", json!({"cwd":root.join("workspace")}));
    let thread_response = receive_response(2);
    let thread_id = thread_response["result"]["threadId"]
        .as_str()
        .expect("thread/start response contains threadId")
        .to_owned();
    send(
        3,
        "turn/start",
        json!({"threadId":thread_id,"input":"Wait at the local audit transport."}),
    );
    let turn_response = receive_response(3);
    let turn_id = turn_response["result"]["turnId"]
        .as_str()
        .expect("turn/start response contains turnId")
        .to_owned();

    let deadline = Instant::now() + Duration::from_secs(20);
    while !marker_path.exists() {
        assert!(
            Instant::now() < deadline,
            "model transport was never entered"
        );
        assert!(
            child.try_wait().unwrap().is_none(),
            "stdio child exited before transport entry"
        );
        std::thread::sleep(Duration::from_millis(10));
    }

    drop(stdin);
    let status = child.wait().unwrap();
    assert!(status.success(), "child stdio server exited with {status}");

    let database = Arc::new(Database::open(&database_path).unwrap());
    let store = RunStore::new(&database);
    let run = store
        .get(&turn_id)
        .unwrap()
        .expect("accepted run remains readable");
    let terminal_events = store
        .events_after(&turn_id, None)
        .unwrap()
        .iter()
        .filter(|event| event.event_type == "run_terminal")
        .count();
    eprintln!(
        "stdio subprocess EOF audit: child_status={status}, run_state={:?}, finished_at={:?}, terminal_events={terminal_events}",
        run.state, run.finished_at
    );
    assert!(
        run.finished_at.is_some() && terminal_events == 1,
        "stdio EOF exited the child with an active turn lacking one durable terminal"
    );
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
    assert!(!fixture
        .events
        .lock()
        .unwrap()
        .iter()
        .any(|event| { event["type"] == "approval.requested" && event["turnId"] == turn_id }));
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
#[ignore = "audit: one tool approval must produce one correlated approval.requested event"]
async fn audit_tool_approval_emits_one_correlated_event() {
    let call_id = "audit-approval-call";
    let fixture = Fixture::new(function_call(
        call_id,
        "delete_path",
        json!({"path":"must-not-be-deleted"}),
    ))
    .await;
    let (response, launch) = fixture.state.lock().unwrap().dispatch(
        rpc(
            "turn/start",
            json!({"threadId":fixture.thread_id,"input":"Request a guarded tool."}),
        ),
        None,
    );
    assert!(response.is_success());
    let turn_id = response.result()["turnId"].as_str().unwrap().to_owned();
    let launch = launch.expect("turn launch");
    let chat = launch.chat.clone();
    let events = fixture.events.clone();
    let emitter: EventEmitter = Arc::new(move |event| {
        events
            .lock()
            .unwrap()
            .push(serde_json::to_value(event).unwrap());
    });
    spawn_turn(fixture.state.clone(), emitter, launch);

    let observed = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let count = fixture
                .events
                .lock()
                .unwrap()
                .iter()
                .filter(|event| event["type"] == "approval.requested" && event["turnId"] == turn_id)
                .count();
            let active = fixture
                .state
                .lock()
                .unwrap()
                .turns
                .get(&turn_id)
                .is_some_and(|turn| turn.active);
            if count >= 2 || !active {
                break count;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await;

    let requests: Vec<_> = fixture
        .events
        .lock()
        .unwrap()
        .iter()
        .filter(|event| event["type"] == "approval.requested" && event["turnId"] == turn_id)
        .cloned()
        .collect();
    assert!(chat.request_cancel(&turn_id).unwrap().accepted);
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if !fixture
                .state
                .lock()
                .unwrap()
                .turns
                .get(&turn_id)
                .is_some_and(|turn| turn.active)
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("cancel the approval wait and finish the run");

    assert!(!fixture.workspace.join("must-not-be-deleted").exists());
    eprintln!("approval event audit: observed={observed:?}, requests={requests:?}");
    assert!(
        observed.is_ok(),
        "approval request was not emitted before timeout"
    );
    assert_eq!(
        requests.len(),
        1,
        "one approval generated multiple protocol events"
    );
    assert_eq!(requests[0]["approvalId"], call_id);
}

#[tokio::test]
#[ignore = "audit: AutoReview hook ask publishes before the policy auto-allows"]
async fn audit_auto_review_hook_ask_emits_before_auto_allow() {
    let call_id = "audit-auto-review-read";
    let fixture = Fixture::new_scripted(vec![
        function_call(call_id, "read_file", json!({"path":"safe.txt"})),
        completed_text("AUDIT_AFTER_AUTO_REVIEW"),
    ])
    .await;
    std::fs::write(fixture.workspace.join("safe.txt"), "safe audit fixture").unwrap();
    fixture
        .settings
        .set_approval_policy(ApprovalPolicy::AutoReview)
        .unwrap();

    let hook_result =
        r#"{"permissionDecision":"ask","permissionDecisionReason":"audit hook asks for approval"}"#;
    let (command, shell) = if cfg!(windows) {
        (format!("Write-Output '{hook_result}'"), "powershell")
    } else {
        (format!("printf '%s' '{hook_result}'"), "sh")
    };
    fixture
        .settings
        .set_hooks_json(
            &json!({"hooks":{"PreToolUse":[{"matcher":"read_file","hooks":[{
                "type":"command","command":command,"shell":shell,"timeout":5
            }]}]}})
            .to_string(),
        )
        .unwrap();

    let turn_id = fixture.run_turn().await;

    let requests: Vec<_> = fixture
        .events
        .lock()
        .unwrap()
        .iter()
        .filter(|event| event["type"] == "approval.requested" && event["turnId"] == turn_id)
        .cloned()
        .collect();
    let tool_result = fixture
        .events
        .lock()
        .unwrap()
        .iter()
        .find(|event| {
            event["type"] == "item.completed"
                && event["turnId"] == turn_id
                && event["item"]["kind"] == "tool_result"
                && event["item"]["callId"] == call_id
        })
        .cloned();

    eprintln!(
        "AutoReview hook ask audit: requests={requests:?}, tool_result={tool_result:?}, terminals={:?}",
        fixture.terminals(&turn_id)
    );
    assert_eq!(fixture.terminals(&turn_id), ["turn.completed"]);
    assert_eq!(
        requests.len(),
        1,
        "the auto-allowed hook request should produce one early ToolBlocked projection"
    );
    assert_eq!(requests[0]["reason"], "audit hook asks for approval");
    assert!(
        requests
            .iter()
            .all(|event| event.get("approvalId").is_none()),
        "the default classifier should auto-allow read_file without entering ChannelApprovalGate"
    );
    assert!(tool_result.is_some_and(|event| event["item"]["ok"] == true));
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
    let terminal = events
        .iter()
        .position(|event| event["event_type"] == "run_terminal")
        .unwrap();
    assert!(completed < terminal);
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
