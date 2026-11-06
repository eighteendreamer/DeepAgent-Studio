//! H-02: ACK, cancellation and execution share the existing durable admission.
use super::*;
use deepagent_persistence::runtime_log_store::RuntimeLogStore;

pub(super) fn launch(fixture: &Fixture, input: &str) -> TurnLaunch {
    let (response, launch) = fixture.state.lock().unwrap().dispatch(
        rpc(
            "turn/start",
            json!({"threadId":fixture.thread_id,"input":input}),
        ),
        None,
    );
    assert!(response.is_success(), "{response:?}");
    let launch = launch.unwrap();
    let db = launch.chat.database();
    let run = RunStore::new(&db).get(&launch.turn_id).unwrap().unwrap();
    assert_eq!(run.session_id, fixture.thread_id);
    assert!(run.finished_at.is_none());
    launch
}

pub(super) fn interrupt(fixture: &Fixture, turn_id: &str) -> RpcResponse {
    fixture
        .state
        .lock()
        .unwrap()
        .dispatch(
            rpc(
                "turn/interrupt",
                json!({"threadId":fixture.thread_id,"turnId":turn_id}),
            ),
            None,
        )
        .0
}

pub(super) fn execute(fixture: &Fixture, launch: TurnLaunch) {
    let events = fixture.events.clone();
    spawn_turn(
        fixture.state.clone(),
        Arc::new(move |event| {
            events
                .lock()
                .unwrap()
                .push(serde_json::to_value(event).unwrap());
        }),
        launch,
    );
}

pub(super) async fn finished(fixture: &Fixture, id: &str) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while fixture.state.lock().unwrap().turns[id].active {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("executor must finalize the admitted turn");
}

fn assert_cancelled(fixture: &Fixture, id: &str) {
    assert_durable_terminal(&fixture.read(), id, "cancelled");
    assert_eq!(fixture.terminals(id), ["turn.interrupted"]);
    let state = fixture.state.lock().unwrap();
    assert!(!state.turns[id].chat.request_cancel(id).unwrap().accepted);
}

#[tokio::test]
async fn admission_immediate_interrupt_skips_model_and_tools_and_allows_next_input() {
    let transport = Arc::new(ScriptedTransport {
        responses: vec![
            function_call(
                "create",
                "write_file",
                json!({
                    "path":"canary.txt","content":"normal follower"
                }),
            ),
            completed_text("done"),
        ],
        next_response: AtomicUsize::new(0),
    });
    let fixture = Fixture::with_transport(transport.clone()).await;
    let pending = launch(&fixture, "Create canary.txt");
    let id = pending.turn_id.clone();
    for _ in 0..2 {
        assert_eq!(interrupt(&fixture, &id).result()["status"], "cancelling");
    }
    execute(&fixture, pending);
    finished(&fixture, &id).await;
    assert_cancelled(&fixture, &id);
    assert_eq!(transport.next_response.load(Ordering::SeqCst), 0);
    assert!(!fixture.workspace.join("canary.txt").exists());
    assert!(!fixture.events.lock().unwrap().iter().any(|event| {
        event["turnId"] == id
            && event["type"] == "item.started"
            && event["item"]["kind"] == "tool_call"
    }));
    let mut reopened = fixture.reopened_state();
    dispatch(
        &mut reopened,
        "thread/resume",
        json!({"threadId":fixture.thread_id}),
    );
    assert_durable_terminal(
        &dispatch(
            &mut reopened,
            "thread/read",
            json!({"threadId":fixture.thread_id}),
        ),
        &id,
        "cancelled",
    );
    let follower = fixture.run_turn().await;
    assert_durable_terminal(&fixture.read(), &follower, "succeeded");
    assert_eq!(transport.next_response.load(Ordering::SeqCst), 2);
    assert!(fixture.events.lock().unwrap().iter().any(|event| {
        event["turnId"] == follower
            && event["type"] == "item.started"
            && event["item"]["kind"] == "tool_call"
            && event["item"]["name"] == "write_file"
    }));
    assert_eq!(
        std::fs::read_to_string(fixture.workspace.join("canary.txt")).unwrap(),
        "normal follower"
    );
    assert_eq!(
        interrupt(&fixture, &id).error_code(),
        Some(ERR_INVALID_TURN)
    );
}

#[tokio::test]
async fn admission_failure_does_not_publish_started_or_install_a_turn() {
    let fixture = Fixture::new(completed_text("unused")).await;
    let db = fixture.state.lock().unwrap().database.clone();
    let store = EventStore::new(&db);
    let _control = store
        .begin_control(fixture.thread_id.parse().unwrap())
        .unwrap();
    let mut state = fixture.state.lock().unwrap();
    let (response, launch) = state.dispatch(
        rpc(
            "turn/start",
            json!({"threadId":fixture.thread_id,"input":"blocked admission"}),
        ),
        None,
    );
    assert_eq!(response.error_code(), Some(ERR_INTERNAL));
    assert!(launch.is_none());
    assert!(state.turns.is_empty());
    assert!(RunStore::new(&db)
        .list_for_session(&fixture.thread_id)
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn admission_failed_ack_finalizes_before_returning_the_transport_error() {
    let transport = Arc::new(ScriptedTransport {
        responses: vec![completed_text("unused")],
        next_response: AtomicUsize::new(0),
    });
    let fixture = Fixture::with_transport(transport.clone()).await;
    let pending = launch(&fixture, "undeliverable ACK");
    let id = pending.turn_id.clone();
    let events = fixture.events.clone();
    let result = complete_turn_dispatch(
        fixture.state.clone(),
        Arc::new(move |event| {
            events
                .lock()
                .unwrap()
                .push(serde_json::to_value(event).unwrap());
        }),
        pending,
        Err("fixture: stdout broken pipe".into()),
    )
    .await;
    assert_eq!(result.unwrap_err(), "fixture: stdout broken pipe");
    assert!(!fixture.state.lock().unwrap().turns[&id].active);
    assert_cancelled(&fixture, &id);
    assert_eq!(transport.next_response.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn admission_interrupt_rejects_a_finished_run_even_when_connection_view_is_active() {
    let fixture = Fixture::new(completed_text("normal completion")).await;
    let id = fixture.run_turn().await;
    fixture
        .state
        .lock()
        .unwrap()
        .turns
        .get_mut(&id)
        .unwrap()
        .active = true;
    assert_eq!(
        interrupt(&fixture, &id).error_code(),
        Some(ERR_INVALID_TURN)
    );
    assert_durable_terminal(&fixture.read(), &id, "succeeded");
    assert_eq!(fixture.terminals(&id), ["turn.completed"]);
    // A stale connection entry with no execution-side registration must also
    // fail, even if no durable record can settle the question.
    let mut state = fixture.state.lock().unwrap();
    let mut stale = state.turns[&id].clone();
    stale.active = true;
    state.turns.insert("unknown-registration".into(), stale);
    drop(state);
    assert_eq!(
        interrupt(&fixture, "unknown-registration").error_code(),
        Some(ERR_INVALID_TURN)
    );
}

#[tokio::test]
async fn admission_steering_replacement_is_cancellable_before_execution() {
    let transport = Arc::new(ScriptedTransport {
        responses: vec![completed_text("unused")],
        next_response: AtomicUsize::new(0),
    });
    let fixture = Fixture::with_transport(transport.clone()).await;
    let original = launch(&fixture, "original");
    let original_id = original.turn_id.clone();
    let (response, replacement) = fixture.state.lock().unwrap().dispatch(
        rpc(
            "turn/steer",
            json!({"threadId":fixture.thread_id,"turnId":original_id,"input":"replacement"}),
        ),
        None,
    );
    assert!(response.is_success(), "{response:?}");
    let replacement = replacement.unwrap();
    let replacement_id = replacement.turn_id.clone();
    assert_eq!(
        interrupt(&fixture, &replacement_id).result()["status"],
        "cancelling"
    );
    execute(&fixture, original);
    execute(&fixture, replacement);
    finished(&fixture, &original_id).await;
    finished(&fixture, &replacement_id).await;
    assert_cancelled(&fixture, &original_id);
    assert_cancelled(&fixture, &replacement_id);
    assert_eq!(transport.next_response.load(Ordering::SeqCst), 0);
}

pub(super) struct HeldFirstTransport {
    pub(super) requests: AtomicUsize,
    pub(super) released: tokio::sync::Notify,
}

#[async_trait::async_trait]
impl HttpTransport for HeldFirstTransport {
    async fn stream(&self, _: TransportRequest, _: &mut dyn EventSink) -> CoreResult<()> {
        panic!("must use cancellation-aware path")
    }

    async fn stream_cancelled(
        &self,
        _: TransportRequest,
        sink: &mut dyn EventSink,
        _: Arc<AtomicBool>,
    ) -> CoreResult<()> {
        let sink: &mut dyn deepagent_models::transport::EventSink =
            &mut crate::test_sequence::sink(sink);
        if self.requests.fetch_add(1, Ordering::SeqCst) == 0 {
            // Hold the real input lease until the queue/cancellation barrier
            // is observed; this models a provider slow to react to cancellation.
            self.released.notified().await;
        }
        for event in completed_text("released") {
            sink.on_event(&event)?;
        }
        Ok(())
    }

    async fn get_json(&self, _: &str, _: &str) -> CoreResult<String> {
        Ok(
            json!({"data":[{"id":"deepseek-flash","object":"model","owned_by":"deepseek"}]})
                .to_string(),
        )
    }
}

#[tokio::test]
async fn admission_queued_interrupt_removes_only_its_waiter_without_a_model_request() {
    let transport = Arc::new(HeldFirstTransport {
        requests: AtomicUsize::new(0),
        released: tokio::sync::Notify::new(),
    });
    let fixture = Fixture::with_transport(transport.clone()).await;
    let logs = Arc::new(RuntimeLogStore::open_in_memory().unwrap());
    {
        let mut state = fixture.state.lock().unwrap();
        let thread = state.threads.get_mut(&fixture.thread_id).unwrap();
        thread.chat = thread.chat.clone().with_runtime_logs(logs.clone());
    }
    let first = launch(&fixture, "active");
    let first_id = first.turn_id.clone();
    execute(&fixture, first);
    tokio::time::timeout(Duration::from_secs(10), async {
        while transport.requests.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    // Ordinary start now rejects a busy thread (H-284). Explicit steering is
    // still the supported producer of queued replacement input for H-02.
    let (response, queued) = fixture.state.lock().unwrap().dispatch(
        rpc(
            "turn/steer",
            json!({"threadId":fixture.thread_id,"turnId":first_id,"input":"queued"}),
        ),
        None,
    );
    assert!(response.is_success(), "{response:?}");
    let queued = queued.unwrap();
    let queued_id = queued.turn_id.clone();
    execute(&fixture, queued);
    tokio::time::timeout(Duration::from_secs(10), async {
        while !logs.recent(100).unwrap().iter().any(|entry| {
            entry.event == "input_queued" && entry.run_id.as_deref() == Some(&queued_id)
        }) {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("observe the production input queue before interrupting");
    assert_eq!(
        interrupt(&fixture, &queued_id).result()["status"],
        "cancelling"
    );
    finished(&fixture, &queued_id).await;
    assert_cancelled(&fixture, &queued_id);
    assert_eq!(transport.requests.load(Ordering::SeqCst), 1);
    assert!(fixture.state.lock().unwrap().turns[&first_id].active);
    transport.released.notify_one();
    finished(&fixture, &first_id).await;
    assert_cancelled(&fixture, &first_id);
    let follower = fixture.run_turn().await;
    assert_durable_terminal(&fixture.read(), &follower, "succeeded");
    assert_eq!(transport.requests.load(Ordering::SeqCst), 2);
}

const CHILD_ROOT: &str = "DEEPAGENT_H02_STDIO_TEST_ROOT";

#[test]
#[ignore = "H-02 child fixture; invoked only by the real-pipe parent test"]
fn admission_stdio_child() {
    let root = PathBuf::from(std::env::var_os(CHILD_ROOT).expect("parent fixture root"));
    let transport = Arc::new(ConcurrentTurnTransport::new());
    let rt = tokio::runtime::Runtime::new().unwrap();
    let fixture = rt.block_on(Fixture::with_transport_root(
        transport.clone(),
        root.clone(),
        None,
    ));
    let (chat, capabilities) = {
        let state = fixture.state.lock().unwrap();
        (state.base_chat.clone(), state.capabilities.clone())
    };
    rt.block_on(run_stdio(chat, fixture.workspace.clone(), capabilities))
        .unwrap();
    std::fs::write(
        root.join("request-count"),
        transport.requests.load(Ordering::SeqCst).to_string(),
    )
    .unwrap();
}

#[test]
fn admission_real_stdio_immediate_interrupt_reaches_one_terminal_for_every_ack() {
    use std::io::{BufRead, BufReader, Write};
    use std::process::{Child, Command, Stdio};
    use std::sync::mpsc;
    use std::time::Instant;

    struct ChildGuard(Child);
    impl Drop for ChildGuard {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let root = tempfile::tempdir().unwrap();
    let mut child = ChildGuard(
        Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "app_server::terminal_audit::admission_tests::admission_stdio_child",
                "--ignored",
                "--nocapture",
            ])
            .env(CHILD_ROOT, root.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap(),
    );
    let mut stdin = child.0.stdin.take().unwrap();
    let stdout = child.0.stdout.take().unwrap();
    let (tx, rx) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            if let Ok(value) = serde_json::from_str::<Value>(&line) {
                if tx.send(value).is_err() {
                    break;
                }
            }
        }
    });
    let mut send = |id: u64, method: &str, params: Value| {
        writeln!(
            stdin,
            "{}",
            json!({"jsonrpc":"2.0","id":id,"method":method,"params":params})
        )
        .unwrap();
        stdin.flush().unwrap();
    };
    let mut notifications = Vec::new();
    let receive = |id: u64, notifications: &mut Vec<Value>| -> Value {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let value = rx
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .expect("real stdio RPC response deadline");
            if value["id"] == id {
                return value;
            }
            if value["method"] == NOTIFICATION_METHOD {
                notifications.push(value["params"].clone());
            }
        }
    };
    send(
        1,
        "initialize",
        json!({"clientName":"h02-pipe","clientVersion":"1",
        "protocolVersion":PROTOCOL_VERSION}),
    );
    assert!(receive(1, &mut notifications)["result"].is_object());
    send(
        2,
        "thread/start",
        json!({"cwd":root.path().join("workspace")}),
    );
    let thread = receive(2, &mut notifications)["result"]["threadId"]
        .as_str()
        .unwrap()
        .to_owned();
    let db = Database::open(root.path().join("state.sqlite")).unwrap();
    let mut turn_ids = Vec::new();
    let mut request_id = 3;
    for _ in 0..32 {
        send(
            request_id,
            "turn/start",
            json!({"threadId":thread,"input":"Wait until cancelled."}),
        );
        let response = receive(request_id, &mut notifications);
        let id = response["result"]["turnId"]
            .as_str()
            .expect("started ACK")
            .to_owned();
        assert_eq!(response["result"]["status"], "started");
        // Check the durable identity at the actual wire ACK, before sending interrupt.
        assert!(RunStore::new(&db).get(&id).unwrap().is_some());
        request_id += 1;
        // Repeat the input after its wire ACK (including the pre-execution
        // window). The error must not create another durable admission.
        send(
            request_id,
            "turn/start",
            json!({"threadId":thread,"input":"Wait until cancelled."}),
        );
        assert_eq!(
            receive(request_id, &mut notifications)["error"]["code"],
            ERR_THREAD_BUSY
        );
        assert_eq!(
            RunStore::new(&db).list_for_session(&thread).unwrap().len(),
            turn_ids.len() + 1
        );
        request_id += 1;
        send(
            request_id,
            "turn/interrupt",
            json!({"threadId":thread,"turnId":id}),
        );
        assert_eq!(
            receive(request_id, &mut notifications)["result"]["status"],
            "cancelling"
        );
        request_id += 1;
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            send(request_id, "thread/read", json!({"threadId":thread}));
            let read = receive(request_id, &mut notifications);
            request_id += 1;
            let terminal = read["result"]["runEvents"]
                .as_array()
                .unwrap()
                .iter()
                .any(|run| run["runId"] == id && run["terminalKind"] == "cancelled");
            let notified = notifications
                .iter()
                .any(|event| event["turnId"] == id && event["type"] == "turn.interrupted");
            if terminal && notified {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "ACKed interrupt must reach a durable and live terminal"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
        turn_ids.push(id);
    }
    drop(stdin);
    let deadline = Instant::now() + Duration::from_secs(10);
    let status = loop {
        if let Some(status) = child.0.try_wait().unwrap() {
            break status;
        }
        assert!(Instant::now() < deadline, "child exit deadline");
        std::thread::sleep(Duration::from_millis(5));
    };
    assert!(status.success());
    reader.join().unwrap();
    for value in rx.try_iter() {
        if value["method"] == NOTIFICATION_METHOD {
            notifications.push(value["params"].clone());
        }
    }
    for id in &turn_ids {
        let events = RunStore::new(&db).events_after(id, None).unwrap();
        assert_eq!(
            events
                .iter()
                .filter(|event| event.event_type == "run_terminal")
                .count(),
            1
        );
        let terminals: Vec<_> = notifications
            .iter()
            .filter(|event| {
                event["turnId"] == *id
                    && matches!(
                        event["type"].as_str(),
                        Some("turn.interrupted" | "turn.completed" | "turn.failed")
                    )
            })
            .collect();
        assert_eq!(terminals.len(), 1);
        assert_eq!(terminals[0]["type"], "turn.interrupted");
    }
    let requests: usize = std::fs::read_to_string(root.path().join("request-count"))
        .unwrap()
        .parse()
        .unwrap();
    assert!(requests <= 32);
    eprintln!("H-02 real stdio: {} ACKs, {} interrupts, {} unique cancelled terminals; model_requests={requests}",
        turn_ids.len(), turn_ids.len(), turn_ids.len());
}
