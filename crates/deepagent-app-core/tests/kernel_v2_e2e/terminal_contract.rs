//! H-208/H-82: terminal acceptance, not item completion, authorizes tool commit.
use super::*;
use deepagent_core::{event::EventPayload, response_item::ResponseItem};
use deepagent_persistence::{
    event_store::EventStore, run_control::RunControlStore, run_store::RunStore,
};
use serde_json::{json, Value};

const RUN: &str = "h208-terminal";
const CALL: &str = "h208-write";
const CANARY: &str = "PRIVATE_H208_TERMINAL";

struct TerminalModel {
    events: Vec<String>,
    workspace: PathBuf,
    calls: AtomicUsize,
}

#[async_trait]
impl HttpTransport for TerminalModel {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        assert!(request.url.ends_with("/responses"));
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        if call == 0 {
            let last = self.events.len() - 1;
            emit_responses_events(sink, &self.events[..last])?;
            assert!(
                !self.workspace.join("effect.txt").exists(),
                "done is only preparation"
            );
            emit_responses_events(sink, &self.events[last..])
        } else {
            assert_eq!(
                call, 1,
                "neither rejection nor normal completion should retry"
            );
            let body: Value = serde_json::from_str(&request.body).unwrap();
            for kind in ["function_call", "function_call_output"] {
                assert_eq!(
                    body["input"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .filter(|item| item["type"] == kind && item["call_id"] == CALL)
                        .count(),
                    1,
                    "normal execution must send one paired call and result"
                );
            }
            emit_responses_events(
                sink,
                &responses_fixture::completed(
                    "h208-answer",
                    "deepseek-flash",
                    responses_fixture::text("h208-text", "Done."),
                    Value::Null,
                ),
            )
        }
    }
}

#[derive(Clone, Copy)]
enum Ending {
    Completed,
    Malformed(&'static str, &'static str),
    Incomplete(Option<&'static str>),
}

async fn observe(ending: Ending) {
    let rejected = !matches!(ending, Ending::Completed);
    let temporary = tempfile::tempdir().unwrap();
    let workspace = temporary.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let path = temporary.path().join("state.sqlite");
    let db = Arc::new(Database::open(&path).unwrap());
    let settings = Arc::new(SettingsService::new(
        db.clone(),
        Arc::new(MockTransport::with_get_json(
            r#"{"data":[{"id":"deepseek-flash","object":"model","owned_by":"deepseek"}]}"#,
        )),
        Arc::new(MemorySecretStore::new()),
    ));
    settings.initialize("offline-placeholder").await.unwrap();
    let mut events = responses_fixture::completed(
        "h208-response",
        "deepseek-flash",
        responses_fixture::call(
            CALL,
            "write_file",
            json!({"path":"effect.txt","content":"synthetic effect"}),
        ),
        Value::Null,
    );
    if let Ending::Malformed(kind, field) = ending {
        let mut event: Value = serde_json::from_str(events.last().unwrap()).unwrap();
        event["type"] = json!(kind);
        event["response"]["status"] = json!(kind.strip_prefix("response.").unwrap());
        event["response"]["id"] = json!(CANARY);
        event["response"].as_object_mut().unwrap().remove(field);
        *events.last_mut().unwrap() = event.to_string();
    }
    if let Ending::Incomplete(reason) = ending {
        let mut event: Value = serde_json::from_str(events.last().unwrap()).unwrap();
        event["type"] = json!("response.incomplete");
        event["response"]["status"] = json!("incomplete");
        event["response"]["incomplete_details"] =
            reason.map(|reason| json!({"reason":reason})).into();
        event["response"]["usage"] = json!({"input_tokens":9,"output_tokens":2,"total_tokens":11});
        *events.last_mut().unwrap() = event.to_string();
    }
    let model = Arc::new(TerminalModel {
        events,
        workspace: workspace.clone(),
        calls: AtomicUsize::new(0),
    });
    let chat = ChatService::new(db.clone(), settings, model.clone(), &workspace)
        .with_cost(Arc::new(CostService::new(db.clone())));
    let live = Arc::new(Mutex::new(Vec::new()));
    let callback = live.clone();
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(8),
        chat.run_in_session(
            "Create the requested synthetic file.",
            None,
            None,
            None,
            vec![],
            None,
            false,
            Some(RUN),
            move |event| {
                callback
                    .lock()
                    .unwrap()
                    .push(serde_json::to_value(event).unwrap())
            },
            |_| panic!("low-risk fixture must not require approval"),
        ),
    )
    .await
    .expect("bounded synthetic run");
    let run = RunStore::new(&db).get(RUN).unwrap().unwrap();
    let events = chat.run_events(RUN, None).unwrap();
    let terminal: Vec<_> = events
        .iter()
        .filter(|event| event.event_type == "run_terminal")
        .collect();
    assert_eq!(terminal.len(), 1);
    assert_eq!(
        run.terminal_kind.as_deref(),
        Some(if rejected { "failed" } else { "succeeded" })
    );
    let actions = RunControlStore::new(&db).list_actions(RUN).unwrap();
    assert!(RunControlStore::new(&db)
        .list_approvals(RUN)
        .unwrap()
        .is_empty());
    let history = EventStore::new(&db)
        .load_session(run.session_id.parse().unwrap())
        .unwrap();
    if rejected {
        let error = result
            .expect_err("rejected terminal must abort prepared write")
            .to_string();
        match ending {
            Ending::Malformed(kind, field) => assert!(
                error.contains("malformed_response_terminal")
                    && error.contains(kind)
                    && error.contains(field),
                "{error}"
            ),
            Ending::Incomplete(reason) => assert!(
                error.contains(match reason {
                    Some("content_filter") => "content_filter",
                    None => "incomplete_reason_unknown",
                    Some(_) => "malformed_response_terminal",
                }),
                "{error}"
            ),
            Ending::Completed => unreachable!(),
        }
        assert!(!error.contains(CANARY));
        assert_eq!(terminal[0].data["kind"], "failed");
        assert_eq!(model.calls.load(Ordering::SeqCst), 1);
        assert!(actions.is_empty());
        assert!(!workspace.join("effect.txt").exists());
        assert!(!events
            .iter()
            .any(|event| matches!(event.event_type.as_str(), "tool_started" | "tool_completed")));
        assert!(!history.iter().any(|event| matches!(
            &event.payload,
            EventPayload::ResponseItemAppended {
                item: ResponseItem::FunctionCall { .. } | ResponseItem::FunctionCallOutput { .. }
            } | EventPayload::ToolCallRequested { .. }
                | EventPayload::ToolCallCompleted { .. }
        )));
        assert!(!serde_json::to_string(&*live.lock().unwrap())
            .unwrap()
            .contains(CANARY));
        assert!(!serde_json::to_string(&events).unwrap().contains(CANARY));
    } else {
        assert!(result.is_ok(), "{result:?}");
        assert_eq!(terminal[0].data["kind"], "succeeded");
        assert_eq!(model.calls.load(Ordering::SeqCst), 2);
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].call_id, CALL);
        assert_eq!(
            actions[0].state,
            deepagent_persistence::run_control::RunActionState::Completed
        );
        assert_eq!(history.iter().filter(|event|matches!(&event.payload,
            EventPayload::ResponseItemAppended {item:ResponseItem::FunctionCall {call_id,..}} if call_id==CALL
        )).count(),1);
        assert_eq!(history.iter().filter(|event|matches!(&event.payload,
            EventPayload::ResponseItemAppended {item:ResponseItem::FunctionCallOutput {call_id,..}} if call_id==CALL
        )).count(),1);
        assert_eq!(
            std::fs::read_to_string(workspace.join("effect.txt")).unwrap(),
            "synthetic effect"
        );
        for kind in ["tool_started", "tool_completed"] {
            assert_eq!(
                events
                    .iter()
                    .filter(|event| event.event_type == kind)
                    .count(),
                1
            );
        }
    }
    assert!(events
        .windows(2)
        .all(|pair| pair[0].sequence < pair[1].sequence));
    let cursor = events[events.len() / 2].sequence;
    let tail = chat.run_events(RUN, Some(cursor)).unwrap();
    assert_eq!(
        tail,
        events
            .iter()
            .filter(|event| event.sequence > cursor)
            .cloned()
            .collect::<Vec<_>>()
    );
    drop(chat);
    drop(db);
    let reopened = Database::open(&path).unwrap();
    if let Ending::Incomplete(reason) = ending {
        let totals = deepagent_persistence::cost_store::CostStore::new(&reopened)
            .session_usage(&run.session_id)
            .unwrap();
        let valid_envelope = reason.is_none() || reason == Some("content_filter");
        assert_eq!(totals.input_tokens, if valid_envelope { 9 } else { 0 });
        assert_eq!(totals.output_tokens, if valid_envelope { 2 } else { 0 });
    }
    assert_eq!(
        RunStore::new(&reopened).events_after(RUN, None).unwrap(),
        events
    );
    assert_eq!(
        RunStore::new(&reopened)
            .events_after(RUN, Some(cursor))
            .unwrap(),
        tail
    );
    assert_eq!(
        RunStore::new(&reopened)
            .get(RUN)
            .unwrap()
            .unwrap()
            .terminal_kind,
        run.terminal_kind
    );
    assert_eq!(
        RunControlStore::new(&reopened).list_actions(RUN).unwrap(),
        actions
    );
    assert!(RunControlStore::new(&reopened)
        .list_approvals(RUN)
        .unwrap()
        .is_empty());
    assert_eq!(
        EventStore::new(&reopened)
            .load_session(run.session_id.parse().unwrap())
            .unwrap(),
        history
    );
    assert_eq!(
        model.calls.load(Ordering::SeqCst),
        if rejected { 1 } else { 2 },
        "reopen must not dispatch"
    );
}

#[tokio::test]
async fn h208_malformed_terminal_aborts_done_write_and_survives_sqlite_reopen() {
    for kind in [
        "response.completed",
        "response.incomplete",
        "response.failed",
    ] {
        for field in ["id", "object", "created_at", "status", "model", "output"] {
            observe(Ending::Malformed(kind, field)).await;
        }
    }
    eprintln!("H-208 verified 18 malformed terminal/kernel/file/SQLite/cursor cases");
}

#[tokio::test]
async fn h208_valid_completed_terminal_commits_write_exactly_once_and_replays() {
    observe(Ending::Completed).await;
}

#[tokio::test]
async fn h82_filtered_or_unspecified_incomplete_aborts_done_write_and_reopens_failed() {
    for reason in [Some("content_filter"), None, Some("PRIVATE_FUTURE_REASON")] {
        observe(Ending::Incomplete(reason)).await;
    }
}
