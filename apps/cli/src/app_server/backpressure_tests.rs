//! H-01: fault visibility, existing executor cancellation and durable recovery.
use super::*;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::io::{AsyncWrite, DuplexStream};

fn terminal() -> HarnessEvent {
    HarnessEvent::TurnCompleted {
        thread_id: Some("outbox-control".into()),
        turn_id: Some("terminal".into()),
        message: "done".into(),
    }
}

struct FailedWriter(bool);
impl AsyncWrite for FailedWriter {
    fn poll_write(
        self: Pin<&mut Self>,
        _: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        if self.0 {
            Poll::Ready(Ok(bytes.len()))
        } else {
            Poll::Ready(Err(std::io::ErrorKind::BrokenPipe.into()))
        }
    }
    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Err(std::io::ErrorKind::BrokenPipe.into()))
    }
    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

#[tokio::test]
async fn backpressure_writer_and_flush_errors_poison_connection_without_ack_progress() {
    for flush_only in [false, true] {
        let outbox = EventOutbox::new(Arc::new(tokio::sync::Mutex::new(FailedWriter(flush_only))));
        outbox.emit(terminal());
        let error = tokio::time::timeout(Duration::from_secs(2), outbox.output.failed())
            .await
            .unwrap();
        assert!(
            error.contains(if flush_only { "flush" } else { "write" }),
            "{error}"
        );
        assert_eq!(outbox.output.delivered_sequence.load(Ordering::Acquire), 0);
        outbox.emit(terminal());
        assert_eq!(
            outbox.output.failure_reason().as_deref(),
            Some(error.as_str())
        );
    }
    let (outbox, rx) = EventOutbox::channel(1);
    drop(rx);
    outbox.emit(terminal());
    assert_eq!(outbox.output.failed().await, "event outbox writer closed");
}

#[tokio::test]
async fn backpressure_ack_requires_flush_and_resets_on_new_connection() {
    let (writer, reader) = tokio::io::duplex(1024);
    let writer = Arc::new(tokio::sync::Mutex::new(writer));
    let gate = writer.lock().await;
    let outbox = EventOutbox::new(writer.clone());
    let mut state = ServerState::new_for_test();
    initialize(&mut state);
    state.output = outbox.output.clone();
    outbox.emit(terminal());
    let ack = |state: &mut ServerState, sequence| {
        state
            .dispatch(rpc("event/ack", json!({"eventSequence":sequence})), None)
            .0
    };
    assert_eq!(ack(&mut state, 1).error_code(), Some(ERR_INVALID_PARAMS));
    drop(gate);
    let mut lines = tokio::io::BufReader::new(reader).lines();
    for sequence in 1..=600 {
        if sequence > 1 {
            outbox.emit(terminal());
        }
        let value: Value =
            serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
        assert_eq!(value["eventSequence"], sequence);
        // Read completion can race the writer's post-flush high-water update.
        while outbox.output.delivered_sequence.load(Ordering::Acquire) < sequence {
            tokio::task::yield_now().await;
        }
        assert!(ack(&mut state, sequence).is_success());
        assert!(ack(&mut state, sequence).is_success());
    }
    assert_eq!(ack(&mut state, 599).error_code(), Some(ERR_INVALID_PARAMS));
    assert_eq!(ack(&mut state, 601).error_code(), Some(ERR_INVALID_PARAMS));
    assert!(outbox.output.failure_reason().is_none());
    let mut reconnected = ServerState::new_for_test();
    initialize(&mut reconnected);
    assert_eq!(
        ack(&mut reconnected, 600).error_code(),
        Some(ERR_INVALID_PARAMS)
    );
}

pub(super) struct BurstTransport {
    pub(super) requests: AtomicUsize,
    pub(super) release: tokio::sync::Notify,
    pub(super) burst_done: tokio::sync::Notify,
    ignore_cancel: AtomicBool,
}

impl BurstTransport {
    pub(super) fn new() -> Self {
        Self {
            requests: AtomicUsize::new(0),
            release: tokio::sync::Notify::new(),
            burst_done: tokio::sync::Notify::new(),
            ignore_cancel: AtomicBool::new(false),
        }
    }
}

#[async_trait::async_trait]
impl HttpTransport for BurstTransport {
    async fn stream(&self, _: TransportRequest, _: &mut dyn EventSink) -> CoreResult<()> {
        panic!("cancellation-aware path required")
    }
    async fn stream_cancelled(
        &self,
        _: TransportRequest,
        sink: &mut dyn EventSink,
        cancel: Arc<AtomicBool>,
    ) -> CoreResult<()> {
        self.requests.fetch_add(1, Ordering::SeqCst);
        self.release.notified().await;
        let delta =
            json!({"type":"response.output_text.delta","delta":"x".repeat(4096)}).to_string();
        for _ in 0..1024 {
            sink.on_event(&delta)?;
        }
        self.burst_done.notify_one();
        while !cancel.load(Ordering::Acquire) || self.ignore_cancel.load(Ordering::Acquire) {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        Err(CoreError::other("fixture observed existing cancel token"))
    }
    async fn get_json(&self, _: &str, _: &str) -> CoreResult<String> {
        Ok(
            json!({"data":[{"id":"deepseek-flash","object":"model","owned_by":"deepseek"}]})
                .to_string(),
        )
    }
}

struct PipeClient {
    input: DuplexStream,
    lines: tokio::io::Lines<tokio::io::BufReader<DuplexStream>>,
}
impl PipeClient {
    async fn call(&mut self, id: u64, method: &str, params: Value) -> Value {
        self.input
            .write_all(
                format!(
                    "{}\n",
                    json!({"jsonrpc":"2.0","id":id,"method":method,"params":params})
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let line = self
                    .lines
                    .next_line()
                    .await
                    .unwrap()
                    .expect("connection response");
                let value: Value = serde_json::from_str(&line).unwrap();
                if value["id"] == id {
                    assert!(value["result"].is_object(), "{value}");
                    return value["result"].clone();
                }
            }
        })
        .await
        .unwrap()
    }
}

fn pipe_server(fixture: &Fixture) -> (PipeClient, tokio::task::JoinHandle<Result<(), String>>) {
    let (input, stdin) = tokio::io::duplex(65536);
    let (stdout, output) = tokio::io::duplex(1024);
    let (chat, capabilities) = {
        let state = fixture.state.lock().unwrap();
        (state.base_chat.clone(), state.capabilities.clone())
    };
    let task = tokio::spawn(run_stdio_with_io(
        chat,
        fixture.workspace.clone(),
        capabilities,
        stdin,
        stdout,
    ));
    (
        PipeClient {
            input,
            lines: tokio::io::BufReader::new(output).lines(),
        },
        task,
    )
}

#[tokio::test]
async fn backpressure_fault_while_stdin_open_cancels_active_and_queued_then_recovers_durably() {
    let transport = Arc::new(BurstTransport::new());
    let fixture = Fixture::with_transport(transport.clone()).await;
    let (mut client, server) = pipe_server(&fixture);
    client
        .call(
            1,
            "initialize",
            json!({"clientName":"h01","clientVersion":"1","protocolVersion":PROTOCOL_VERSION}),
        )
        .await;
    client
        .call(2, "thread/resume", json!({"threadId":fixture.thread_id}))
        .await;
    let active = client
        .call(
            3,
            "turn/start",
            json!({"threadId":fixture.thread_id,"input":"active"}),
        )
        .await["turnId"]
        .as_str()
        .unwrap()
        .to_owned();
    let queued = client
        .call(
            4,
            "turn/steer",
            json!({"threadId":fixture.thread_id,"turnId":active,"input":"queued"}),
        )
        .await["turnId"]
        .as_str()
        .unwrap()
        .to_owned();
    let before = client
        .call(5, "thread/read", json!({"threadId":fixture.thread_id}))
        .await;
    let session_cursor = before["events"].as_array().unwrap().last().unwrap()["streamSequence"]
        .as_u64()
        .unwrap();
    let mut run_cursors = serde_json::Map::new();
    for run in before["runEvents"].as_array().unwrap() {
        if let Some(last) = run["events"].as_array().unwrap().last() {
            run_cursors.insert(
                run["runId"].as_str().unwrap().to_owned(),
                last["streamSequence"].clone(),
            );
        }
    }
    // Keep both stdin and unread stdout open. Only event failure can wake the server.
    transport.release.notify_one();
    tokio::time::timeout(Duration::from_secs(5), transport.burst_done.notified())
        .await
        .unwrap();
    let error = tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert_eq!(error, "event outbox capacity exceeded");
    assert_eq!(transport.requests.load(Ordering::SeqCst), 1);
    let mut recovered = fixture.reopened_state();
    dispatch(
        &mut recovered,
        "thread/resume",
        json!({"threadId":fixture.thread_id}),
    );
    let read = dispatch(
        &mut recovered,
        "thread/read",
        json!({"threadId":fixture.thread_id}),
    );
    for id in [&active, &queued] {
        assert_durable_terminal(&read, id, "cancelled");
    }
    // Recover via the very same stdio implementation, not a new replay store.
    let (mut healthy, healthy_server) = pipe_server(&fixture);
    healthy.call(1, "initialize", json!({"clientName":"h01-recover","clientVersion":"1","protocolVersion":PROTOCOL_VERSION})).await;
    let recovered = healthy
        .call(
            2,
            "thread/read",
            json!({"threadId":fixture.thread_id,
            "sessionAfterSequence":session_cursor,"runAfterSequences":run_cursors}),
        )
        .await;
    for id in [&active, &queued] {
        assert_durable_terminal(&recovered, id, "cancelled");
    }
    let mut tails = run_cursors.clone();
    for run in recovered["runEvents"].as_array().unwrap() {
        let id = run["runId"].as_str().unwrap();
        for event in run["events"].as_array().unwrap() {
            if let Some(before) = run_cursors.get(id) {
                assert!(event["streamSequence"].as_u64().unwrap() > before.as_u64().unwrap());
            }
            tails.insert(id.to_owned(), event["streamSequence"].clone());
        }
    }
    assert!(recovered["events"]
        .as_array()
        .unwrap()
        .iter()
        .all(|event| event["streamSequence"].as_u64().unwrap() > session_cursor));
    let session_tail = recovered["events"]
        .as_array()
        .unwrap()
        .last()
        .map(|event| event["streamSequence"].as_u64().unwrap())
        .unwrap_or(session_cursor);
    let tail = healthy
        .call(
            3,
            "thread/read",
            json!({"threadId":fixture.thread_id,
        "sessionAfterSequence":session_tail,"runAfterSequences":tails}),
        )
        .await;
    assert!(tail["events"].as_array().unwrap().is_empty());
    assert!(tail["runEvents"]
        .as_array()
        .unwrap()
        .iter()
        .all(|run| run["events"].as_array().unwrap().is_empty()
            && run["terminalKind"] == "cancelled"));
    drop(healthy);
    assert!(healthy_server.await.unwrap().is_ok());
    drop(client);
}

#[tokio::test]
async fn backpressure_healthy_connection_keeps_normal_completed_run_and_terminal() {
    let fixture = Fixture::new(completed_text("normal h01 control")).await;
    let (mut client, server) = pipe_server(&fixture);
    client.call(1, "initialize", json!({"clientName":"h01-control","clientVersion":"1","protocolVersion":PROTOCOL_VERSION})).await;
    client
        .call(2, "thread/resume", json!({"threadId":fixture.thread_id}))
        .await;
    let turn = client
        .call(
            3,
            "turn/start",
            json!({"threadId":fixture.thread_id,"input":"normal"}),
        )
        .await["turnId"]
        .as_str()
        .unwrap()
        .to_owned();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let value: Value =
                serde_json::from_str(&client.lines.next_line().await.unwrap().unwrap()).unwrap();
            if value["params"]["type"] == "turn.completed" {
                assert_eq!(value["params"]["turnId"], turn);
                break;
            }
        }
    })
    .await
    .unwrap();
    let read = client
        .call(4, "thread/read", json!({"threadId":fixture.thread_id}))
        .await;
    assert_durable_terminal(&read, &turn, "succeeded");
    drop(client);
    assert!(server.await.unwrap().is_ok());
}

#[tokio::test]
async fn backpressure_noncooperative_cancel_reports_unresolved_not_fake_terminal() {
    let transport = Arc::new(BurstTransport::new());
    transport.ignore_cancel.store(true, Ordering::Release);
    let fixture = Fixture::with_transport(transport.clone()).await;
    let (mut client, server) = pipe_server(&fixture);
    client.call(1, "initialize", json!({"clientName":"h01-timeout","clientVersion":"1","protocolVersion":PROTOCOL_VERSION})).await;
    client
        .call(2, "thread/resume", json!({"threadId":fixture.thread_id}))
        .await;
    let turn = client
        .call(
            3,
            "turn/start",
            json!({"threadId":fixture.thread_id,"input":"ignore cancellation temporarily"}),
        )
        .await["turnId"]
        .as_str()
        .unwrap()
        .to_owned();
    transport.release.notify_one();
    let error = tokio::time::timeout(Duration::from_secs(15), server)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert!(
        error.contains("turn cancellation did not finish within 10s"),
        "{error}"
    );
    let db = fixture.state.lock().unwrap().database.clone();
    assert!(RunStore::new(&db)
        .get(&turn)
        .unwrap()
        .unwrap()
        .finished_at
        .is_none());
    // Test cleanup only: let the same detached executor observe the existing
    // cancellation token. This does not retroactively turn the timeout green.
    transport.ignore_cancel.store(false, Ordering::Release);
    tokio::time::timeout(Duration::from_secs(5), async {
        while RunStore::new(&db)
            .get(&turn)
            .unwrap()
            .unwrap()
            .finished_at
            .is_none()
        {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    assert_durable_terminal(&fixture.read(), &turn, "cancelled");
}

#[tokio::test]
async fn backpressure_broken_response_cancels_real_pending_approval_without_tool_effect() {
    let fixture = Fixture::new(function_call(
        "h01-approval",
        "delete_path",
        json!({"path":"approval-canary.txt"}),
    ))
    .await;
    fixture
        .settings
        .set_approval_policy(ApprovalPolicy::AlwaysAsk)
        .unwrap();
    let canary = fixture.workspace.join("approval-canary.txt");
    std::fs::write(&canary, "unchanged").unwrap();
    let (mut client, server) = pipe_server(&fixture);
    client.call(1, "initialize", json!({"clientName":"h01-approval","clientVersion":"1","protocolVersion":PROTOCOL_VERSION})).await;
    client
        .call(2, "thread/resume", json!({"threadId":fixture.thread_id}))
        .await;
    let turn = client
        .call(
            3,
            "turn/start",
            json!({"threadId":fixture.thread_id,"input":"request approval"}),
        )
        .await["turnId"]
        .as_str()
        .unwrap()
        .to_owned();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let value: Value =
                serde_json::from_str(&client.lines.next_line().await.unwrap().unwrap()).unwrap();
            if value["params"]["type"] == "approval.requested" {
                break;
            }
        }
    })
    .await
    .expect("observe actual approval wait before disconnecting output");
    let PipeClient { mut input, lines } = client;
    drop(lines);
    input
        .write_all(
            format!(
                "{}\n",
                json!({"jsonrpc":"2.0","id":4,"method":"config/read","params":{}})
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    let error = tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert!(error.contains("write app-server"), "{error}");
    assert_durable_terminal(&fixture.read(), &turn, "cancelled");
    assert_eq!(std::fs::read_to_string(canary).unwrap(), "unchanged");
}

const PIPE_ROOT: &str = "DEEPAGENT_H01_PIPE_ROOT";

#[test]
#[ignore = "H-01 child helper, run by real-pipe parent"]
fn backpressure_real_run_child() {
    // Test-only watchdog bounds a parent blocked in a native pipe read.
    std::thread::spawn(|| {
        std::thread::sleep(Duration::from_secs(20));
        std::process::exit(74);
    });
    let root = PathBuf::from(std::env::var_os(PIPE_ROOT).expect("parent temp root"));
    let rt = tokio::runtime::Runtime::new().unwrap();
    let transport = Arc::new(BurstTransport::new());
    transport.release.notify_one();
    let fixture = rt.block_on(Fixture::with_transport_root(
        transport.clone(),
        root.clone(),
        None,
    ));
    let (chat, capabilities) = {
        let state = fixture.state.lock().unwrap();
        (state.base_chat.clone(), state.capabilities.clone())
    };
    let error = rt
        .block_on(run_stdio(chat, fixture.workspace.clone(), capabilities))
        .unwrap_err();
    std::fs::write(root.join("connection-failure"), error).unwrap();
    // Same explicit error-exit boundary as main; never wait for an unread OS
    // stdout's blocking write during Tokio runtime destruction.
    std::process::exit(1);
}

#[test]
fn backpressure_real_pipe_stalled_stdout_exits_before_reader_or_stdin_closes_and_recovers() {
    use std::io::{BufRead, BufReader, Write};
    use std::process::{Child, Command, Stdio};
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
                "app_server::terminal_audit::backpressure_tests::backpressure_real_run_child",
                "--ignored",
                "--nocapture",
            ])
            .env(PIPE_ROOT, root.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap(),
    );
    let mut input = child.0.stdin.take().unwrap();
    let mut output = BufReader::new(child.0.stdout.take().unwrap());
    let mut call = |id: u64, method: &str, params: Value| {
        writeln!(
            input,
            "{}",
            json!({"jsonrpc":"2.0","id":id,"method":method,"params":params})
        )
        .unwrap();
        input.flush().unwrap();
        loop {
            let mut line = String::new();
            assert!(output.read_line(&mut line).unwrap() > 0);
            if let Ok(value) = serde_json::from_str::<Value>(&line) {
                if value["id"] == id {
                    assert!(value["result"].is_object(), "{value}");
                    break value["result"].clone();
                }
            }
        }
    };
    call(
        1,
        "initialize",
        json!({"clientName":"h01-pipe","clientVersion":"1","protocolVersion":PROTOCOL_VERSION}),
    );
    let thread = call(
        2,
        "thread/start",
        json!({"cwd":root.path().join("workspace")}),
    )["threadId"]
        .as_str()
        .unwrap()
        .to_owned();
    let turn = call(
        3,
        "turn/start",
        json!({"threadId":thread,"input":"burst on real stdout"}),
    )["turnId"]
        .as_str()
        .unwrap()
        .to_owned();
    // Do not drain output or close stdin. The process must fault without either.
    let deadline = Instant::now() + Duration::from_secs(15);
    let status = loop {
        if let Some(status) = child.0.try_wait().unwrap() {
            break status;
        }
        assert!(
            Instant::now() < deadline,
            "connection remained alive after overflow"
        );
        std::thread::sleep(Duration::from_millis(5));
    };
    assert_eq!(status.code(), Some(1));
    assert_eq!(
        std::fs::read_to_string(root.path().join("connection-failure")).unwrap(),
        "event outbox capacity exceeded"
    );
    let db = Arc::new(Database::open(root.path().join("state.sqlite")).unwrap());
    let mut recovered = ServerState::new_for_test_with_database(db, root.path().join("workspace"));
    initialize(&mut recovered);
    let read = dispatch(&mut recovered, "thread/read", json!({"threadId":thread}));
    assert_durable_terminal(&read, &turn, "cancelled");
    eprintln!("H-01 actual AppCore + OS pipe: failed exit=1, stdin/output kept open, durable cancelled exactly once");
}
