//! H-130: one durable replacement claim, over the original dispatcher/executor.
use super::admission_tests::{execute, finished, launch};
use super::*;

fn steer(fixture: &Fixture, old: &str) -> (RpcResponse, Option<TurnLaunch>) {
    fixture.state.lock().unwrap().dispatch(
        rpc(
            "turn/steer",
            json!({"threadId":fixture.thread_id,"turnId":old,"input":"one correction"}),
        ),
        None,
    )
}

fn continuations(fixture: &Fixture, id: &str) -> Vec<Value> {
    let db = fixture.state.lock().unwrap().database.clone();
    RunStore::new(&db)
        .events_after(id, None)
        .unwrap()
        .into_iter()
        .filter(|event| event.event_type == "continuation.created")
        .map(|event| event.data)
        .collect()
}

#[tokio::test]
async fn h130_lost_ack_and_concurrent_duplicates_keep_one_admission_and_survive_reopen() {
    let transport = Arc::new(ScriptedTransport {
        responses: vec![completed_text("single replacement")],
        next_response: AtomicUsize::new(0),
    });
    let fixture = Fixture::with_transport(transport.clone()).await;
    let original = launch(&fixture, "original");
    let old = original.turn_id.clone();
    let (response, replacement) = steer(&fixture, &old);
    assert!(response.is_success());
    let replacement = replacement.unwrap();
    let next = replacement.turn_id.clone();
    drop(response); // ACK loss does not grant another execution identity.
    let stale = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..16)
            .map(|_| scope.spawn(|| steer(&fixture, &old)))
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().unwrap())
            .collect::<Vec<_>>()
    });
    for (response, pending) in stale {
        assert_eq!(
            response.error_code(),
            Some(ERR_INVALID_TURN),
            "{response:?}"
        );
        assert!(pending.is_none());
    }
    assert_eq!(continuations(&fixture, &old).len(), 1);
    assert_eq!(continuations(&fixture, &old)[0]["newTurnId"], next);
    assert_eq!(fixture.state.lock().unwrap().turns.len(), 2);
    execute(&fixture, original);
    execute(&fixture, replacement);
    finished(&fixture, &old).await;
    finished(&fixture, &next).await;
    assert_eq!(transport.next_response.load(Ordering::SeqCst), 1);
    assert_durable_terminal(&fixture.read(), &old, "cancelled");
    assert_durable_terminal(&fixture.read(), &next, "succeeded");
    let mut reopened = fixture.reopened_state();
    dispatch(
        &mut reopened,
        "thread/resume",
        json!({"threadId":fixture.thread_id}),
    );
    let (response, pending) = reopened.dispatch(
        rpc(
            "turn/steer",
            json!({"threadId":fixture.thread_id,"turnId":old,"input":"retry after reopen"}),
        ),
        None,
    );
    assert_eq!(response.error_code(), Some(ERR_INVALID_TURN));
    assert!(pending.is_none());
    let read = dispatch(
        &mut reopened,
        "thread/read",
        json!({"threadId":fixture.thread_id}),
    );
    assert_durable_terminal(&read, &next, "succeeded");
    let runs = read["runEvents"].as_array().unwrap();
    assert_eq!(runs.len(), 2);
    assert_eq!(
        runs.iter()
            .flat_map(|run| run["events"].as_array().unwrap())
            .filter(|event| event["event_type"] == "continuation.created")
            .count(),
        1
    );
}

#[tokio::test]
async fn h130_failed_claim_write_does_not_cancel_original() {
    failed_claim("NEW.event_type='continuation.created'").await;
}

#[tokio::test]
async fn h130_failed_admission_leaves_no_phantom_claim_or_cancel() {
    failed_claim("NEW.event_type='run_accepted'").await;
}

async fn failed_claim(condition: &str) {
    let transport = Arc::new(ScriptedTransport {
        responses: vec![completed_text("original remains healthy")],
        next_response: AtomicUsize::new(0),
    });
    let fixture = Fixture::with_transport(transport.clone()).await;
    let original = launch(&fixture, "must not cancel on failed claim");
    let old = original.turn_id.clone();
    let db = fixture.state.lock().unwrap().database.clone();
    db.with_conn(|conn| {
            conn.execute_batch(&format!("CREATE TRIGGER h130_fail BEFORE INSERT ON run_events WHEN {condition} BEGIN SELECT RAISE(ABORT, 'h130 injected storage failure'); END;")).unwrap();
            Ok(())
        }).unwrap();
    let (response, pending) = steer(&fixture, &old);
    db.with_conn(|conn| {
        conn.execute_batch("DROP TRIGGER h130_fail").unwrap();
        Ok(())
    })
    .unwrap();
    assert_eq!(response.error_code(), Some(ERR_INTERNAL), "{response:?}");
    assert!(pending.is_none());
    assert_eq!(
        RunStore::new(&db)
            .list_for_session(&fixture.thread_id)
            .unwrap()
            .len(),
        1
    );
    assert!(
        continuations(&fixture, &old).is_empty(),
        "failed admission left a phantom replacement"
    );
    execute(&fixture, original);
    finished(&fixture, &old).await;
    assert_durable_terminal(&fixture.read(), &old, "succeeded");
    assert_eq!(transport.next_response.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.terminals(&old), ["turn.completed"]);
}

#[tokio::test]
async fn h130_queued_replacement_can_be_steered_once_without_reopening_ancestors() {
    let transport = Arc::new(ScriptedTransport {
        responses: vec![completed_text("latest correction")],
        next_response: AtomicUsize::new(0),
    });
    let fixture = Fixture::with_transport(transport.clone()).await;
    let first = launch(&fixture, "first");
    let first_id = first.turn_id.clone();
    let (_, second) = steer(&fixture, &first_id);
    let second = second.unwrap();
    let second_id = second.turn_id.clone();
    let (_, third) = steer(&fixture, &second_id);
    let third = third.unwrap();
    let third_id = third.turn_id.clone();
    for old in [&first_id, &second_id] {
        let (response, pending) = steer(&fixture, old);
        assert_eq!(response.error_code(), Some(ERR_INVALID_TURN));
        assert!(pending.is_none());
        assert_eq!(continuations(&fixture, old).len(), 1);
    }
    execute(&fixture, first);
    execute(&fixture, second);
    execute(&fixture, third);
    for id in [&first_id, &second_id, &third_id] {
        finished(&fixture, id).await;
    }
    assert_durable_terminal(&fixture.read(), &first_id, "cancelled");
    assert_durable_terminal(&fixture.read(), &second_id, "cancelled");
    assert_durable_terminal(&fixture.read(), &third_id, "succeeded");
    assert_eq!(transport.next_response.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn h130_failed_replacement_ack_cannot_reclaim_the_original_turn() {
    let transport = Arc::new(ScriptedTransport {
        responses: vec![completed_text("unused")],
        next_response: AtomicUsize::new(0),
    });
    let fixture = Fixture::with_transport(transport.clone()).await;
    let original = launch(&fixture, "original");
    let old = original.turn_id.clone();
    let (_, replacement) = steer(&fixture, &old);
    let replacement = replacement.unwrap();
    let next = replacement.turn_id.clone();
    let result = complete_turn_dispatch(
        fixture.state.clone(),
        Arc::new(|_| {}),
        replacement,
        Err("h130 synthetic ACK write failure".into()),
    )
    .await;
    assert_eq!(result.unwrap_err(), "h130 synthetic ACK write failure");
    let (response, pending) = steer(&fixture, &old);
    assert_eq!(response.error_code(), Some(ERR_INVALID_TURN));
    assert!(pending.is_none());
    assert_eq!(continuations(&fixture, &old).len(), 1);
    execute(&fixture, original);
    finished(&fixture, &old).await;
    assert_durable_terminal(&fixture.read(), &old, "cancelled");
    assert_durable_terminal(&fixture.read(), &next, "cancelled");
    assert_eq!(transport.next_response.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn h130_steer_rejects_a_committed_terminal_even_before_worker_cleanup() {
    let fixture = Fixture::new(completed_text("unused")).await;
    let original = launch(&fixture, "accepted but terminal at boundary");
    let old = original.turn_id.clone();
    let db = original.chat.database();
    RunStore::new(&db)
        .finish_with_event(
            &old,
            "succeeded",
            None,
            &json!({"kind":"succeeded"}),
            None,
            1,
        )
        .unwrap();
    assert!(fixture.state.lock().unwrap().turns[&old].active);
    let (response, pending) = steer(&fixture, &old);
    assert_eq!(response.error_code(), Some(ERR_INVALID_TURN));
    assert!(pending.is_none());
    assert!(continuations(&fixture, &old).is_empty());
    assert_eq!(
        RunStore::new(&db)
            .list_for_session(&fixture.thread_id)
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn h130_foreign_thread_cannot_consume_claim_and_other_threads_remain_independent() {
    let transport = Arc::new(ScriptedTransport {
        responses: vec![completed_text("independent replacement")],
        next_response: AtomicUsize::new(0),
    });
    let fixture = Fixture::with_transport(transport.clone()).await;
    let first = launch(&fixture, "first thread");
    let old = first.turn_id.clone();
    let (other_thread, second) = {
        let mut state = fixture.state.lock().unwrap();
        let other = dispatch(&mut state, "thread/start", json!({"cwd":fixture.workspace}));
        let other = other["threadId"].as_str().unwrap().to_owned();
        let (response, second) = state.dispatch(
            rpc(
                "turn/start",
                json!({"threadId":other,"input":"other thread"}),
            ),
            None,
        );
        assert!(response.is_success());
        let (response, wrong) = state.dispatch(
            rpc(
                "turn/steer",
                json!({"threadId":other,"turnId":old,"input":"foreign"}),
            ),
            None,
        );
        assert_eq!(response.error_code(), Some(ERR_INVALID_TURN));
        assert!(wrong.is_none());
        (other, second.unwrap())
    };
    let second_id = second.turn_id.clone();
    assert!(continuations(&fixture, &old).is_empty());
    let (response, first_next) = steer(&fixture, &old);
    assert!(response.is_success());
    let (response, other_next) = fixture.state.lock().unwrap().dispatch(
        rpc(
            "turn/steer",
            json!({"threadId":other_thread,"turnId":second_id,"input":"other correction"}),
        ),
        None,
    );
    assert!(response.is_success());
    let first_next = first_next.unwrap();
    let first_next_id = first_next.turn_id.clone();
    let other_next = other_next.unwrap();
    let other_next_id = other_next.turn_id.clone();
    for pending in [first, second, first_next, other_next] {
        execute(&fixture, pending);
    }
    for id in [&old, &second_id, &first_next_id, &other_next_id] {
        finished(&fixture, id).await;
    }
    assert_durable_terminal(&fixture.read(), &old, "cancelled");
    assert_durable_terminal(&fixture.read(), &first_next_id, "succeeded");
    let other_read = dispatch(
        &mut fixture.state.lock().unwrap(),
        "thread/read",
        json!({"threadId":other_thread}),
    );
    assert_durable_terminal(&other_read, &second_id, "cancelled");
    assert_durable_terminal(&other_read, &other_next_id, "succeeded");
    assert_eq!(transport.next_response.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn h130_pipe_retry_same_rpc_id_and_eof_keep_exactly_one_replacement() {
    let transport = Arc::new(super::admission_tests::HeldFirstTransport {
        requests: AtomicUsize::new(0),
        released: tokio::sync::Notify::new(),
    });
    let fixture = Fixture::with_transport(transport.clone()).await;
    let (mut client, server) = super::backpressure_tests::pipe_server(&fixture);
    client.call(1, "initialize", json!({"clientName":"h130-pipe","clientVersion":"1","protocolVersion":PROTOCOL_VERSION})).await;
    client
        .call(2, "thread/resume", json!({"threadId":fixture.thread_id}))
        .await;
    let first = client
        .call(
            3,
            "turn/start",
            json!({"threadId":fixture.thread_id,"input":"original"}),
        )
        .await;
    let old = first["turnId"].as_str().unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while transport.requests.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let params = json!({"threadId":fixture.thread_id,"turnId":old,"input":"corrected"});
    let next = client.call(4, "turn/steer", params.clone()).await;
    client
        .input
        .write_all(
            format!(
                "{}\n",
                json!({"jsonrpc":"2.0","id":4,"method":"turn/steer","params":params})
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    let retry = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let value: Value =
                serde_json::from_str(&client.lines.next_line().await.unwrap().unwrap()).unwrap();
            if value["id"] == 4 {
                break value;
            }
        }
    })
    .await
    .unwrap();
    transport.released.notify_one();
    drop(client.input);
    let reader = async { while client.lines.next_line().await.unwrap().is_some() {} };
    let (result, ()) = tokio::time::timeout(Duration::from_secs(15), async {
        tokio::join!(server, reader)
    })
    .await
    .unwrap();
    result.unwrap().unwrap();
    assert_eq!(retry["error"]["code"], ERR_INVALID_TURN);
    assert!(retry["result"].is_null());
    let mut reopened = fixture.reopened_state();
    let read = dispatch(
        &mut reopened,
        "thread/read",
        json!({"threadId":fixture.thread_id}),
    );
    let next = next["turnId"].as_str().unwrap();
    let runs = read["runEvents"].as_array().unwrap();
    assert_eq!(runs.len(), 2);
    assert!(runs
        .iter()
        .any(|run| run["runId"] == next && run["terminalKind"].is_string()));
    assert_eq!(continuations(&fixture, old).len(), 1);
}

#[cfg(windows)]
struct EffectTransport {
    requests: AtomicUsize,
    release: tokio::sync::Notify,
}

#[cfg(windows)]
#[async_trait::async_trait]
impl HttpTransport for EffectTransport {
    async fn stream(&self, _: TransportRequest, sink: &mut dyn EventSink) -> CoreResult<()> {
        let index = self.requests.fetch_add(1, Ordering::SeqCst);
        if index == 0 {
            self.release.notified().await;
        }
        let response = if index % 2 == 1 {
            function_call(
                &format!("h130-effect-{index}"),
                "bash",
                json!({
                    "command":"[IO.File]::AppendAllText('h130-effects.txt', 'effect;')",
                    "shell":"powershell"
                }),
            )
        } else {
            completed_text("done")
        };
        for event in response {
            if sink.on_event(&event)? {
                break;
            }
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

#[cfg(windows)]
#[tokio::test]
async fn h130_live_queue_and_repeated_steer_produce_only_one_actual_shell_effect() {
    let transport = Arc::new(EffectTransport {
        requests: AtomicUsize::new(0),
        release: tokio::sync::Notify::new(),
    });
    let fixture = Fixture::with_transport(transport.clone()).await;
    fixture
        .settings
        .set_approval_policy(ApprovalPolicy::FullAccess)
        .unwrap();
    let logs = Arc::new(
        deepagent_persistence::runtime_log_store::RuntimeLogStore::open_in_memory().unwrap(),
    );
    {
        let mut state = fixture.state.lock().unwrap();
        let thread = state.threads.get_mut(&fixture.thread_id).unwrap();
        thread.chat = thread.chat.clone().with_runtime_logs(logs.clone());
    }
    let original = launch(&fixture, "hold model");
    let old = original.turn_id.clone();
    execute(&fixture, original);
    tokio::time::timeout(Duration::from_secs(5), async {
        while transport.requests.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let (_, first) = steer(&fixture, &old);
    let first = first.unwrap();
    let first_id = first.turn_id.clone();
    let (duplicate_response, duplicate) = steer(&fixture, &old);
    let duplicate_admitted = duplicate.is_some();
    execute(&fixture, first);
    tokio::time::timeout(Duration::from_secs(5), async {
        while !logs.recent(100).unwrap().iter().any(|entry| {
            entry.event == "input_queued" && entry.run_id.as_deref() == Some(&first_id)
        }) {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the actual input lease must queue the replacement");
    transport.release.notify_one();
    finished(&fixture, &old).await;
    finished(&fixture, &first_id).await;
    // Execute any incorrectly admitted duplicate too: a red result must observe
    // its real effect, not infer a duplicate action from a dispatcher token.
    if let Some(duplicate) = duplicate {
        let duplicate_id = duplicate.turn_id.clone();
        execute(&fixture, duplicate);
        finished(&fixture, &duplicate_id).await;
    }
    let effects = std::fs::read_to_string(fixture.workspace.join("h130-effects.txt")).unwrap();
    eprintln!("H-130 actual shell: duplicate_admitted={duplicate_admitted}, model_requests={}, effects={effects:?}", transport.requests.load(Ordering::SeqCst));
    assert!(!duplicate_admitted);
    assert_eq!(duplicate_response.error_code(), Some(ERR_INVALID_TURN));
    assert_eq!(effects, "effect;");
    assert_eq!(transport.requests.load(Ordering::SeqCst), 3);
    assert_eq!(continuations(&fixture, &old).len(), 1);
    assert_durable_terminal(&fixture.read(), &old, "cancelled");
    assert_durable_terminal(&fixture.read(), &first_id, "succeeded");
}
