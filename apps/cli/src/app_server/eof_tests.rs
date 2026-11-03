//! H-92: close the production connection, not the runtime or a second executor.
use super::backpressure_tests::{pipe_server, PipeClient};
use super::*;

async fn connect(fixture: &Fixture) -> (PipeClient, tokio::task::JoinHandle<Result<(), String>>) {
    let (mut client, server) = pipe_server(fixture);
    client
        .call(
            1,
            "initialize",
            json!({"clientName":"h92","clientVersion":"1",
        "protocolVersion":PROTOCOL_VERSION}),
        )
        .await;
    client
        .call(2, "thread/resume", json!({"threadId":fixture.thread_id}))
        .await;
    (client, server)
}

async fn start(client: &mut PipeClient, id: u64, thread: &str) -> String {
    client
        .call(
            id,
            "turn/start",
            json!({"threadId":thread,"input":"offline EOF regression"}),
        )
        .await["turnId"]
        .as_str()
        .unwrap()
        .to_owned()
}

async fn next_event(client: &mut PipeClient, kind: &str) -> Value {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let value: Value =
                serde_json::from_str(&client.lines.next_line().await.unwrap().unwrap()).unwrap();
            if value["params"]["type"] == kind {
                return value["params"].clone();
            }
        }
    })
    .await
    .unwrap()
}

async fn close(
    client: PipeClient,
    server: tokio::task::JoinHandle<Result<(), String>>,
) -> (Result<(), String>, Vec<Value>) {
    let PipeClient { input, mut lines } = client;
    drop(input);
    finish(&mut lines, server).await
}

async fn finish(
    lines: &mut tokio::io::Lines<tokio::io::BufReader<tokio::io::DuplexStream>>,
    server: tokio::task::JoinHandle<Result<(), String>>,
) -> (Result<(), String>, Vec<Value>) {
    // Continue consuming until the sole writer releases stdout, including all
    // terminal notifications enqueued during cancellation and finalization.
    tokio::time::timeout(Duration::from_secs(15), async {
        let tail = async {
            let mut events = Vec::new();
            while let Some(line) = lines.next_line().await.unwrap() {
                events.push(serde_json::from_str(&line).unwrap());
            }
            events
        };
        let (result, events) = tokio::join!(server, tail);
        (result.unwrap(), events)
    })
    .await
    .expect("connection must drain workers and its writer within the shutdown budget")
}

fn assert_terminal(events: &[Value], turn: &str, kind: &str) {
    let terminals: Vec<_> = events
        .iter()
        .filter(|event| event["params"]["turnId"] == turn)
        .filter_map(|event| event["params"]["type"].as_str())
        .filter(|kind| matches!(*kind, "turn.completed" | "turn.failed" | "turn.interrupted"))
        .collect();
    assert_eq!(terminals, [kind]);
}

#[tokio::test]
async fn h92_eof_cancels_active_model_and_flushes_terminal_before_return() {
    let (started, entered) = tokio::sync::oneshot::channel();
    let fixture = Fixture::with_transport(Arc::new(BlockingTurnTransport {
        started: Mutex::new(Some(started)),
        release: tokio::sync::Notify::new(),
        marker_path: None,
    }))
    .await;
    let (mut client, server) = connect(&fixture).await;
    let turn = start(&mut client, 3, &fixture.thread_id).await;
    tokio::time::timeout(Duration::from_secs(5), entered)
        .await
        .unwrap()
        .unwrap();
    let (result, events) = close(client, server).await;
    assert!(result.is_ok(), "{result:?}");
    assert_terminal(&events, &turn, "turn.interrupted");
    let mut reopened = fixture.reopened_state();
    let read = dispatch(
        &mut reopened,
        "thread/read",
        json!({"threadId":fixture.thread_id}),
    );
    assert_durable_terminal(&read, &turn, "cancelled");
}

#[tokio::test]
async fn h92_eof_cancels_every_active_thread_not_just_the_last_turn() {
    let transport = Arc::new(ConcurrentTurnTransport::new());
    let fixture = Fixture::with_transport(transport.clone()).await;
    let (mut client, server) = connect(&fixture).await;
    let first = start(&mut client, 3, &fixture.thread_id).await;
    let other_thread = client
        .call(4, "thread/start", json!({"cwd":fixture.workspace}))
        .await["threadId"]
        .as_str()
        .unwrap()
        .to_owned();
    let second = start(&mut client, 5, &other_thread).await;
    tokio::time::timeout(Duration::from_secs(5), async {
        while transport.requests.load(Ordering::SeqCst) != 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let (result, events) = close(client, server).await;
    assert!(result.is_ok(), "{result:?}");
    assert_terminal(&events, &first, "turn.interrupted");
    assert_terminal(&events, &second, "turn.interrupted");
    let mut reopened = fixture.reopened_state();
    for (thread, turn) in [(&fixture.thread_id, &first), (&other_thread, &second)] {
        let read = dispatch(&mut reopened, "thread/read", json!({"threadId":thread}));
        assert_durable_terminal(&read, turn, "cancelled");
    }
    assert_eq!(transport.requests.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn h92_eof_cancels_multiple_threads_and_queued_steer_then_replays_without_duplicates() {
    // HeldFirstTransport deliberately retains the first lease after steer's
    // cancellation, so this also observes a real queued replacement at EOF.
    let transport = Arc::new(super::admission_tests::HeldFirstTransport {
        requests: AtomicUsize::new(0),
        released: tokio::sync::Notify::new(),
    });
    let fixture = Fixture::with_transport(transport.clone()).await;
    let (mut client, server) = connect(&fixture).await;
    let first = start(&mut client, 3, &fixture.thread_id).await;
    tokio::time::timeout(Duration::from_secs(5), async {
        while transport.requests.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let queued = client
        .call(
            4,
            "turn/steer",
            json!({"threadId":fixture.thread_id,
        "turnId":first,"input":"queued replacement"}),
        )
        .await["turnId"]
        .as_str()
        .unwrap()
        .to_owned();
    let before = client
        .call(5, "thread/read", json!({"threadId":fixture.thread_id}))
        .await;
    let cursor = before["events"].as_array().unwrap().last().unwrap()["streamSequence"].clone();
    let mut run_cursors = serde_json::Map::new();
    for run in before["runEvents"].as_array().unwrap() {
        if let Some(last) = run["events"].as_array().unwrap().last() {
            run_cursors.insert(
                run["runId"].as_str().unwrap().to_owned(),
                last["streamSequence"].clone(),
            );
        }
    }
    // A separate cancellation-aware transport is unnecessary: the second
    // request in this fixture completes normally, a healthy parallel control.
    let other_thread = client
        .call(6, "thread/start", json!({"cwd":fixture.workspace}))
        .await["threadId"]
        .as_str()
        .unwrap()
        .to_owned();
    let other = start(&mut client, 7, &other_thread).await;
    next_event(&mut client, "turn.completed").await;
    let PipeClient { input, mut lines } = client;
    drop(input);
    // Ensure shutdown started before releasing the noncooperative first model.
    let database = fixture.state.lock().unwrap().database.clone();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if RunStore::new(&database)
                .events_after(&queued, None)
                .unwrap()
                .iter()
                .any(|event| event.event_type == "cancel.requested")
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("EOF must cancel the queued run using the existing cancellation tree");
    transport.released.notify_one();
    let (result, events) = finish(&mut lines, server).await;
    assert!(result.is_ok(), "{result:?}");
    assert_terminal(&events, &first, "turn.interrupted");
    assert_terminal(&events, &queued, "turn.interrupted");
    assert_eq!(transport.requests.load(Ordering::SeqCst), 2);
    let (mut reconnected, replay_server) = connect(&fixture).await;
    let read = reconnected
        .call(
            3,
            "thread/read",
            json!({"threadId":fixture.thread_id,
        "sessionAfterSequence":cursor,"runAfterSequences":run_cursors}),
        )
        .await;
    for id in [&first, &queued] {
        assert_durable_terminal(&read, id, "cancelled");
    }
    let other_read = reconnected
        .call(4, "thread/read", json!({"threadId":other_thread}))
        .await;
    assert_durable_terminal(&other_read, &other, "succeeded");
    assert!(read["events"]
        .as_array()
        .unwrap()
        .iter()
        .all(|e| e["streamSequence"].as_u64().unwrap() > cursor.as_u64().unwrap()));
    for run in read["runEvents"].as_array().unwrap() {
        for event in run["events"].as_array().unwrap() {
            if let Some(before) = run_cursors.get(run["runId"].as_str().unwrap()) {
                assert!(event["streamSequence"].as_u64().unwrap() > before.as_u64().unwrap());
            }
        }
    }
    assert!(close(reconnected, replay_server).await.0.is_ok());
}

#[tokio::test]
async fn h92_eof_during_approval_cancels_without_tool_side_effect() {
    let fixture = Fixture::new(function_call(
        "h92-approval",
        "delete_path",
        json!({"path":"canary.txt"}),
    ))
    .await;
    fixture
        .settings
        .set_approval_policy(ApprovalPolicy::AlwaysAsk)
        .unwrap();
    let canary = fixture.workspace.join("canary.txt");
    std::fs::write(&canary, "keep").unwrap();
    let (mut client, server) = connect(&fixture).await;
    let turn = start(&mut client, 3, &fixture.thread_id).await;
    next_event(&mut client, "approval.requested").await;
    let (result, events) = close(client, server).await;
    assert!(result.is_ok(), "{result:?}");
    assert_terminal(&events, &turn, "turn.interrupted");
    assert_durable_terminal(&fixture.read(), &turn, "cancelled");
    assert_eq!(std::fs::read_to_string(canary).unwrap(), "keep");
}

#[tokio::test]
async fn h92_idle_eof_flushes_thread_notification_and_preserves_completed_turn() {
    let fixture = Fixture::new(completed_text("EOF normal control")).await;
    let (mut client, server) = connect(&fixture).await;
    let turn = start(&mut client, 3, &fixture.thread_id).await;
    next_event(&mut client, "turn.completed").await;
    let last = client
        .call(4, "thread/start", json!({"cwd":fixture.workspace}))
        .await;
    let (result, events) = close(client, server).await;
    assert!(result.is_ok(), "{result:?}");
    // Earlier call() can consume the thread notification before its response,
    // so validate durable state rather than impose response/event ordering.
    assert!(events
        .iter()
        .all(|event| event["params"]["type"] != "turn.interrupted"));
    let mut reopened = fixture.reopened_state();
    let read = dispatch(
        &mut reopened,
        "thread/read",
        json!({"threadId":fixture.thread_id}),
    );
    assert_durable_terminal(&read, &turn, "succeeded");
    assert!(dispatch(
        &mut reopened,
        "thread/read",
        json!({"threadId":last["threadId"]})
    )["threadId"]
        .is_string());
}

#[derive(Default)]
struct FlushGate {
    mode: AtomicUsize,
    reached: AtomicBool,
    waker: Mutex<Option<std::task::Waker>>,
    dropped: AtomicBool,
}

impl FlushGate {
    fn release(&self) {
        self.mode.store(0, Ordering::Release);
        if let Some(waker) = self.waker.lock().unwrap().take() {
            waker.wake();
        }
    }

    async fn entered(&self) {
        tokio::time::timeout(Duration::from_secs(5), async {
            while !self.reached.load(Ordering::Acquire) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the actual writer must reach the controlled flush");
    }
}

struct ControlledWriter {
    writer: tokio::io::DuplexStream,
    gate: Arc<FlushGate>,
}

impl Drop for ControlledWriter {
    fn drop(&mut self) {
        self.gate.dropped.store(true, Ordering::Release);
    }
}

impl AsyncWrite for ControlledWriter {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        bytes: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        if self.gate.mode.load(Ordering::Acquire) == 2 {
            return std::task::Poll::Ready(Err(std::io::ErrorKind::BrokenPipe.into()));
        }
        std::pin::Pin::new(&mut self.writer).poll_write(cx, bytes)
    }

    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        *self.gate.waker.lock().unwrap() = Some(cx.waker().clone());
        match self.gate.mode.load(Ordering::Acquire) {
            1 => {
                self.gate.reached.store(true, Ordering::Release);
                std::task::Poll::Pending
            }
            3 => std::task::Poll::Ready(Err(std::io::ErrorKind::BrokenPipe.into())),
            _ => std::pin::Pin::new(&mut self.writer).poll_flush(cx),
        }
    }

    fn poll_shutdown(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.writer).poll_shutdown(cx)
    }
}

async fn controlled_connection() -> (
    Fixture,
    PipeClient,
    tokio::task::JoinHandle<Result<(), String>>,
    Arc<FlushGate>,
    String,
) {
    let (started, entered) = tokio::sync::oneshot::channel();
    let fixture = Fixture::with_transport(Arc::new(BlockingTurnTransport {
        started: Mutex::new(Some(started)),
        release: tokio::sync::Notify::new(),
        marker_path: None,
    }))
    .await;
    let gate = Arc::new(FlushGate::default());
    let (input, stdin) = tokio::io::duplex(65536);
    let (writer, output) = tokio::io::duplex(65536);
    let server = {
        let state = fixture.state.lock().unwrap();
        tokio::spawn(run_stdio_with_io(
            state.base_chat.clone(),
            fixture.workspace.clone(),
            state.capabilities.clone(),
            stdin,
            ControlledWriter {
                writer,
                gate: gate.clone(),
            },
        ))
    };
    let mut client = PipeClient {
        input,
        lines: tokio::io::BufReader::new(output).lines(),
    };
    client.call(1,"initialize",json!({"clientName":"h92-flush","clientVersion":"1","protocolVersion":PROTOCOL_VERSION})).await;
    client
        .call(2, "thread/resume", json!({"threadId":fixture.thread_id}))
        .await;
    let turn = start(&mut client, 3, &fixture.thread_id).await;
    tokio::time::timeout(Duration::from_secs(5), entered)
        .await
        .unwrap()
        .unwrap();
    (fixture, client, server, gate, turn)
}

#[tokio::test]
async fn h92_eof_waits_for_the_real_writer_flush_not_just_durable_terminal() {
    let (fixture, client, mut server, gate, turn) = controlled_connection().await;
    gate.mode.store(1, Ordering::Release);
    let PipeClient { input, mut lines } = client;
    drop(input);
    gate.entered().await;
    // Runtime events precede the final durable write; wait for that write too.
    tokio::time::timeout(Duration::from_secs(5), async {
        while !fixture.read()["runEvents"]
            .as_array()
            .unwrap()
            .iter()
            .any(|run| run["runId"] == turn && run["terminalKind"] == "cancelled")
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(50), &mut server)
            .await
            .is_err(),
        "shutdown returned before flush"
    );
    gate.release();
    let (result, events) = finish(&mut lines, server).await;
    assert!(result.is_ok(), "{result:?}");
    assert_terminal(&events, &turn, "turn.interrupted");
    assert!(gate.dropped.load(Ordering::Acquire));
}

#[tokio::test]
async fn h92_eof_reports_write_and_flush_failure_but_keeps_one_durable_terminal() {
    for (mode, expected) in [
        (2, "write app-server notification"),
        (3, "flush app-server notification"),
    ] {
        let (fixture, client, server, gate, turn) = controlled_connection().await;
        gate.mode.store(mode, Ordering::Release);
        let (result, _) = close(client, server).await;
        assert!(result.unwrap_err().contains(expected));
        assert_durable_terminal(&fixture.read(), &turn, "cancelled");
        assert!(gate.dropped.load(Ordering::Acquire));
    }
}

#[tokio::test]
async fn h92_eof_stalled_flush_uses_the_existing_ten_second_budget_and_fails() {
    let (fixture, client, server, gate, turn) = controlled_connection().await;
    gate.mode.store(1, Ordering::Release);
    let clock = std::time::Instant::now();
    let (result, _) = close(client, server).await;
    let error = result.unwrap_err();
    assert!(
        error.contains("event outbox did not drain within the 10s shutdown budget"),
        "{error}"
    );
    assert!(gate.reached.load(Ordering::Acquire));
    assert!(clock.elapsed() >= Duration::from_secs(10));
    assert!(gate.dropped.load(Ordering::Acquire));
    assert_durable_terminal(&fixture.read(), &turn, "cancelled");
}

#[tokio::test]
async fn h92_eof_noncooperative_worker_is_reported_unresolved_never_faked_terminal() {
    let transport = Arc::new(super::admission_tests::HeldFirstTransport {
        requests: AtomicUsize::new(0),
        released: tokio::sync::Notify::new(),
    });
    let fixture = Fixture::with_transport(transport.clone()).await;
    let (mut client, server) = connect(&fixture).await;
    let turn = start(&mut client, 3, &fixture.thread_id).await;
    tokio::time::timeout(Duration::from_secs(5), async {
        while transport.requests.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let (result, _) = close(client, server).await;
    assert!(result
        .unwrap_err()
        .contains("turn cancellation did not finish within 10s"));
    let db = fixture.state.lock().unwrap().database.clone();
    assert!(RunStore::new(&db)
        .get(&turn)
        .unwrap()
        .unwrap()
        .finished_at
        .is_none());
    // Test cleanup only. This does not turn the timeout into successful shutdown.
    transport.released.notify_one();
    tokio::time::timeout(Duration::from_secs(5), async {
        while RunStore::new(&db)
            .get(&turn)
            .unwrap()
            .unwrap()
            .finished_at
            .is_none()
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_durable_terminal(&fixture.read(), &turn, "cancelled");
}

#[tokio::test]
async fn h92_completion_eof_race_keeps_exactly_one_executor_terminal() {
    for _ in 0..16 {
        let transport = Arc::new(ConcurrentTurnTransport::new());
        let fixture = Fixture::with_transport(transport.clone()).await;
        let (mut client, server) = connect(&fixture).await;
        let turn = start(&mut client, 3, &fixture.thread_id).await;
        tokio::time::timeout(Duration::from_secs(5), async {
            while transport.requests.load(Ordering::SeqCst) == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        transport.release.notify_one();
        let (result, events) = close(client, server).await;
        assert!(result.is_ok(), "{result:?}");
        let read = fixture.read();
        let run = read["runEvents"]
            .as_array()
            .unwrap()
            .iter()
            .find(|run| run["runId"] == turn)
            .unwrap();
        let (durable, live) = match run["terminalKind"].as_str().unwrap() {
            "cancelled" => ("cancelled", "turn.interrupted"),
            "succeeded" => ("succeeded", "turn.completed"),
            other => panic!("unexpected terminal: {other}"),
        };
        assert_durable_terminal(&read, &turn, durable);
        assert_terminal(&events, &turn, live);
        assert_eq!(transport.requests.load(Ordering::SeqCst), 1);
    }
}

#[cfg(windows)]
#[tokio::test]
async fn h92_eof_during_actual_command_cancels_and_reaps_the_shell() {
    let fixture = Fixture::new(function_call("h92-shell", "bash", json!({
        "command":"[IO.File]::WriteAllText('h92-shell.pid', [string]$PID); Start-Sleep -Seconds 60",
        "shell":"powershell"
    }))).await;
    fixture
        .settings
        .set_approval_policy(ApprovalPolicy::FullAccess)
        .unwrap();
    let (mut client, server) = connect(&fixture).await;
    let turn = start(&mut client, 3, &fixture.thread_id).await;
    let marker = fixture.workspace.join("h92-shell.pid");
    tokio::time::timeout(Duration::from_secs(10), async {
        while !marker.exists() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the actual workspace-local shell must execute before EOF");
    let pid = std::fs::read_to_string(marker).unwrap();
    let (result, events) = close(client, server).await;
    assert!(result.is_ok(), "{result:?}");
    assert_durable_terminal(&fixture.read(), &turn, "cancelled");
    assert_terminal(&events, &turn, "turn.interrupted");
    // Observe only this synthetic child PID, never enumerate unrelated commands.
    let status = tokio::process::Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            &format!(
                "if (Get-Process -Id {} -ErrorAction SilentlyContinue) {{ exit 1 }}; exit 0",
                pid.trim().parse::<u32>().unwrap()
            ),
        ])
        .creation_flags(0x08000000)
        .status()
        .await
        .unwrap();
    assert!(
        status.success(),
        "the actual tool shell remains alive after EOF"
    );
}
