//! H-284: ordinary start is start-if-idle, never implicit replacement.
use super::admission_tests::{execute, finished, interrupt, launch};
use super::*;

fn reject_busy(fixture: &Fixture) {
    let mut state = fixture.state.lock().unwrap();
    let turns = state.turns.len();
    let runs = RunStore::new(&state.database)
        .list_for_session(&fixture.thread_id)
        .unwrap();
    let (response, launch) = state.dispatch(
        rpc(
            "turn/start",
            json!({"threadId":fixture.thread_id,"input":"repeated input"}),
        ),
        None,
    );
    assert_eq!(response.error_code(), Some(ERR_THREAD_BUSY));
    assert!(response.result.is_none());
    assert!(launch.is_none());
    assert_eq!(state.turns.len(), turns);
    assert_eq!(
        RunStore::new(&state.database)
            .list_for_session(&fixture.thread_id)
            .unwrap()
            .len(),
        runs.len()
    );
}

#[tokio::test]
async fn start_busy_covers_ack_retry_concurrent_dispatch_resume_and_restart() {
    let transport = Arc::new(ScriptedTransport {
        responses: vec![completed_text("first input only")],
        next_response: AtomicUsize::new(0),
    });
    let mut fixture = Fixture::with_transport(transport.clone()).await;
    let first = launch(&fixture, "first input");
    let first_id = first.turn_id.clone();
    // No worker exists yet: even an identical RPC retry after a lost ACK must
    // not receive a second admission or cancellation registration.
    std::thread::scope(|scope| {
        for _ in 0..16 {
            scope.spawn(|| reject_busy(&fixture));
        }
    });
    dispatch(
        &mut fixture.state.lock().unwrap(),
        "thread/resume",
        json!({"threadId":fixture.thread_id}),
    );
    reject_busy(&fixture);
    assert_eq!(transport.next_response.load(Ordering::SeqCst), 0);
    execute(&fixture, first);
    finished(&fixture, &first_id).await;
    assert_eq!(transport.next_response.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.terminals(&first_id), ["turn.completed"]);
    assert_durable_terminal(&fixture.read(), &first_id, "succeeded");
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
        &first_id,
        "succeeded",
    );
    fixture.state = Arc::new(Mutex::new(reopened));
    let next = launch(&fixture, "after cold reattach");
    let next_id = next.turn_id.clone();
    // The reopened fixture intentionally has no provider. Cancel before its
    // executor, proving it is admitted without stale process-local busy state.
    assert!(interrupt(&fixture, &next_id).is_success());
    execute(&fixture, next);
    finished(&fixture, &next_id).await;
    assert_durable_terminal(&fixture.read(), &next_id, "cancelled");
}

#[tokio::test]
async fn start_busy_uses_session_identity_not_the_resumed_id_spelling() {
    let transport = Arc::new(ScriptedTransport {
        responses: vec![completed_text("unused")],
        next_response: AtomicUsize::new(0),
    });
    let fixture = Fixture::with_transport(transport.clone()).await;
    let first = launch(&fixture, "admitted canonical session");
    let raw = fixture.thread_id.strip_prefix("ses_").unwrap();
    for alias in [raw.to_owned(), format!("ses_{}", raw.to_uppercase())] {
        assert_eq!(
            alias.parse::<deepagent_core::id::SessionId>().unwrap(),
            fixture
                .thread_id
                .parse::<deepagent_core::id::SessionId>()
                .unwrap()
        );
        dispatch(
            &mut fixture.state.lock().unwrap(),
            "thread/resume",
            json!({"threadId":alias}),
        );
        let (response, pending) = fixture.state.lock().unwrap().dispatch(
            rpc(
                "turn/start",
                json!({"threadId":alias,"input":"same session"}),
            ),
            None,
        );
        eprintln!(
            "H-284 session alias: response={response:?}, launch={}",
            pending.is_some()
        );
        assert_eq!(response.error_code(), Some(ERR_THREAD_BUSY));
        assert!(pending.is_none());
        assert_eq!(fixture.state.lock().unwrap().turns.len(), 1);
    }
    assert_eq!(transport.next_response.load(Ordering::SeqCst), 0);
    let first_id = first.turn_id.clone();
    execute(&fixture, first);
    finished(&fixture, &first_id).await;
    assert_eq!(fixture.terminals(&first_id), ["turn.completed"]);
    assert_durable_terminal(&fixture.read(), &first_id, "succeeded");
    assert_eq!(transport.next_response.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn start_busy_does_not_confuse_other_threads_or_recovered_unfinished_runs() {
    let transport = Arc::new(ScriptedTransport {
        responses: vec![completed_text("independent thread")],
        next_response: AtomicUsize::new(0),
    });
    let mut fixture = Fixture::with_transport(transport.clone()).await;
    let abandoned = launch(&fixture, "old admitted input");
    let abandoned_id = abandoned.turn_id.clone();
    let other = dispatch(
        &mut fixture.state.lock().unwrap(),
        "thread/start",
        json!({"cwd":fixture.workspace}),
    );
    let (response, pending) = fixture.state.lock().unwrap().dispatch(
        rpc(
            "turn/start",
            json!({"threadId":other["threadId"],"input":"other thread"}),
        ),
        None,
    );
    assert!(response.is_success());
    let other_id = pending.as_ref().unwrap().turn_id.clone();
    execute(&fixture, pending.unwrap());
    finished(&fixture, &other_id).await;
    reject_busy(&fixture);
    assert_eq!(transport.next_response.load(Ordering::SeqCst), 1);
    assert!(fixture.state.lock().unwrap().turns[&abandoned_id].active);

    let mut reopened = fixture.reopened_state();
    dispatch(
        &mut reopened,
        "thread/resume",
        json!({"threadId":fixture.thread_id}),
    );
    drop(abandoned);
    fixture.state = Arc::new(Mutex::new(reopened));
    let db = fixture.state.lock().unwrap().database.clone();
    let recovered = AppService::from_shared(db)
        .recover_unfinished_runs()
        .unwrap();
    assert_eq!(recovered.len(), 1);
    assert_durable_terminal(&fixture.read(), &abandoned_id, "failed");
    let next = launch(&fixture, "after startup recovery");
    let next_id = next.turn_id.clone();
    assert!(interrupt(&fixture, &next_id).is_success());
    execute(&fixture, next);
    finished(&fixture, &next_id).await;
    assert_durable_terminal(&fixture.read(), &next_id, "cancelled");
}

#[tokio::test]
async fn start_allows_another_thread_to_finish_while_first_model_is_held() {
    let transport = Arc::new(super::admission_tests::HeldFirstTransport {
        requests: AtomicUsize::new(0),
        released: tokio::sync::Notify::new(),
    });
    let fixture = Fixture::with_transport(transport.clone()).await;
    let first = launch(&fixture, "held first thread");
    let first_id = first.turn_id.clone();
    execute(&fixture, first);
    tokio::time::timeout(Duration::from_secs(10), async {
        while transport.requests.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    let other_thread = dispatch(
        &mut fixture.state.lock().unwrap(),
        "thread/start",
        json!({"cwd":fixture.workspace}),
    );
    let (response, other) = fixture.state.lock().unwrap().dispatch(
        rpc(
            "turn/start",
            json!({"threadId":other_thread["threadId"],"input":"independent thread"}),
        ),
        None,
    );
    assert!(response.is_success());
    let other = other.unwrap();
    let other_id = other.turn_id.clone();
    execute(&fixture, other);
    finished(&fixture, &other_id).await;
    let first_still_active = fixture.state.lock().unwrap().turns[&first_id].active;
    transport.released.notify_one();
    finished(&fixture, &first_id).await;
    assert!(
        first_still_active,
        "busy must be thread-local, not a global run lock"
    );
    assert_eq!(transport.requests.load(Ordering::SeqCst), 2);
    assert_eq!(fixture.terminals(&first_id), ["turn.completed"]);
    assert_eq!(
        fixture.terminals_for(other_thread["threadId"].as_str().unwrap(), &other_id),
        ["turn.completed"]
    );
    assert_durable_terminal(&fixture.read(), &first_id, "succeeded");
    let other_read = dispatch(
        &mut fixture.state.lock().unwrap(),
        "thread/read",
        json!({"threadId":other_thread["threadId"]}),
    );
    assert_durable_terminal(&other_read, &other_id, "succeeded");
}

#[tokio::test]
async fn start_busy_keeps_cancelling_and_queued_steering_workers_reserved() {
    // This fixture ignores cancellation until released, retaining the actual
    // input lease while explicit steering waits in the existing AppCore queue.
    let transport = Arc::new(super::admission_tests::HeldFirstTransport {
        requests: AtomicUsize::new(0),
        released: tokio::sync::Notify::new(),
    });
    let fixture = Fixture::with_transport(transport.clone()).await;
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
    assert!(interrupt(&fixture, &first_id).is_success());
    reject_busy(&fixture);
    let (response, queued) = fixture.state.lock().unwrap().dispatch(
        rpc(
            "turn/steer",
            json!({"threadId":fixture.thread_id,"turnId":first_id,"input":"explicit replacement"}),
        ),
        None,
    );
    assert_eq!(response.result()["replacesTurnId"], first_id);
    let queued = queued.unwrap();
    let queued_id = queued.turn_id.clone();
    reject_busy(&fixture);
    execute(&fixture, queued);
    // Neither cancelling the queued waiter nor its terminal frees the first
    // worker's reservation. The existing H-02 test checks real queue ingress.
    assert!(interrupt(&fixture, &queued_id).is_success());
    finished(&fixture, &queued_id).await;
    reject_busy(&fixture);
    transport.released.notify_one();
    finished(&fixture, &first_id).await;
    assert_eq!(transport.requests.load(Ordering::SeqCst), 1);
    assert_durable_terminal(&fixture.read(), &first_id, "cancelled");
    assert_durable_terminal(&fixture.read(), &queued_id, "cancelled");
    let next = fixture.run_turn().await;
    assert_durable_terminal(&fixture.read(), &next, "succeeded");
    assert_eq!(transport.requests.load(Ordering::SeqCst), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn start_busy_during_real_file_tool_and_terminal_delivery_does_not_cancel_it() {
    let fixture = Fixture::new_scripted(vec![
        function_call(
            "file-write",
            "write_file",
            json!({"path":"canary.txt","content":"only one write"}),
        ),
        completed_text("written"),
    ])
    .await;
    let pending = launch(&fixture, "write a file");
    let id = pending.turn_id.clone();
    let stage_seen = Arc::new(AtomicUsize::new(0));
    let callback_stage = stage_seen.clone();
    let (release, waits) = std::sync::mpsc::channel();
    let waits = Mutex::new(waits);
    let events = fixture.events.clone();
    let worker = spawn_turn(
        fixture.state.clone(),
        Arc::new(move |event| {
            let value = serde_json::to_value(&event).unwrap();
            let stage = if value["type"] == "item.started" && value["item"]["kind"] == "tool_call" {
                Some(1)
            } else if value["type"] == "turn.completed" {
                Some(3)
            } else {
                None
            };
            if let Some(stage) = stage {
                callback_stage.store(stage, Ordering::SeqCst);
                waits
                    .lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(10))
                    .expect(
                        "parent must release the callback; never block runtime shutdown forever",
                    );
            }
            events.lock().unwrap().push(value);
        }),
        pending,
    );
    for expected in [1, 3] {
        tokio::time::timeout(Duration::from_secs(10), async {
            while stage_seen.load(Ordering::SeqCst) != expected {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        // Capture rejection before releasing the blocked callback; assert only
        // after release so test failure cannot strand a blocking event pump.
        let (response, next) = fixture.state.lock().unwrap().dispatch(
            rpc(
                "turn/start",
                json!({"threadId":fixture.thread_id,"input":"must not replace file write"}),
            ),
            None,
        );
        release.send(()).unwrap();
        assert_eq!(response.error_code(), Some(ERR_THREAD_BUSY));
        assert!(next.is_none());
    }
    tokio::time::timeout(Duration::from_secs(10), worker)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(fixture.workspace.join("canary.txt")).unwrap(),
        "only one write"
    );
    assert_eq!(fixture.terminals(&id), ["turn.completed"]);
    let read = fixture.read();
    assert_eq!(read["runEvents"].as_array().unwrap().len(), 1);
    assert_durable_terminal(&read, &id, "succeeded");
    assert!(!read["runEvents"][0]["events"]
        .as_array()
        .unwrap()
        .iter()
        .any(|event| event["event_type"] == "cancel.requested"));
}

#[tokio::test]
async fn start_busy_during_approval_keeps_request_and_canary_intact() {
    let fixture = Fixture::new_scripted(vec![
        function_call("delete-canary", "delete_path", json!({"path":"canary.txt"})),
        completed_text("deletion denied"),
    ])
    .await;
    std::fs::write(fixture.workspace.join("canary.txt"), "keep").unwrap();
    let pending = launch(&fixture, "request deletion");
    let id = pending.turn_id.clone();
    execute(&fixture, pending);
    let approval = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Some(event) = fixture
                .events
                .lock()
                .unwrap()
                .iter()
                .find(|event| event["type"] == "approval.requested")
                .cloned()
            {
                break event;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    reject_busy(&fixture);
    let response = fixture
        .state
        .lock()
        .unwrap()
        .dispatch(
            rpc(
                "approval/respond",
                json!({"approvalId":approval["approvalId"],"approved":false}),
            ),
            None,
        )
        .0;
    assert!(response.is_success());
    finished(&fixture, &id).await;
    assert_durable_terminal(&fixture.read(), &id, "succeeded");
    assert_eq!(fixture.terminals(&id), ["turn.completed"]);
    assert_eq!(
        std::fs::read_to_string(fixture.workspace.join("canary.txt")).unwrap(),
        "keep"
    );
}
