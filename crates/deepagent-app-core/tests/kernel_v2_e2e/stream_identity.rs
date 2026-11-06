//! H-200: use the real retry/tool/kernel/event-store path and reopen its SQLite ledger.
use super::*;
use deepagent_persistence::{run_store::RunStore, runtime_log_store::RuntimeLogStore};
use serde_json::{json, Value};

const RUN: &str = "h200-stream-identity";
const SERVER_MODEL: &str = "server/model@2026-11-06";

struct IdentityTransport {
    streams: Mutex<VecDeque<Vec<String>>>,
    calls: AtomicUsize,
    workspace: PathBuf,
}

#[async_trait]
impl HttpTransport for IdentityTransport {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        assert!(call < 3, "unexpected dispatch: {call}");
        let request: Value = serde_json::from_str(&request.body).unwrap();
        assert_eq!(request["model"], "deepseek-flash");
        let events = self.streams.lock().unwrap().pop_front().unwrap();
        if call == 1 {
            emit_raw_responses_events(sink, &events[..events.len() - 1])?;
            assert!(!self.workspace.join("effect.txt").exists());
            emit_raw_responses_events(sink, &events[events.len() - 1..])?;
        } else {
            emit_raw_responses_events(sink, &events)?;
        }
        if call == 0 {
            Err(CoreError::other("unexpected EOF in provider stream"))
        } else {
            Ok(())
        }
    }
}

async fn observe(ending: &str) {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let path = root.path().join("state.sqlite");
    let db = Arc::new(Database::open(&path).unwrap());
    let settings = Arc::new(SettingsService::new(
        db.clone(),
        Arc::new(MockTransport::with_get_json(
            r#"{"data":[{"id":"deepseek-flash","object":"model","owned_by":"deepseek"}]}"#,
        )),
        Arc::new(MemorySecretStore::new()),
    ));
    settings.initialize("synthetic-placeholder").await.unwrap();
    let first = vec![
        json!({"type":"response.created","sequence_number":0,"response":{
            "id":"attempt-lost","model":"server-model-before","instructions":"PRIVATE_BODY"}})
        .to_string(),
        json!({"type":"response.output_text.delta","sequence_number":1,
            "delta":"discarded prefix"})
        .to_string(),
    ];
    let tool = responses_fixture::completed(
        "attempt-tool",
        SERVER_MODEL,
        responses_fixture::call(
            "h200-write",
            "write_file",
            json!({"path":"effect.txt","content":"synthetic effect"}),
        ),
        json!({"input_tokens":2,"output_tokens":3,"total_tokens":5}),
    )
    .into_iter()
    .map(|event| {
        let mut event: Value = serde_json::from_str(&event).unwrap();
        let sequence = event["sequence_number"].as_u64().unwrap();
        if sequence >= 2 {
            event["sequence_number"] = json!(sequence + 2);
        }
        event.to_string()
    })
    .collect();
    let mut final_stream = responses_fixture::completed(
        "attempt-answer",
        SERVER_MODEL,
        responses_fixture::text("message-answer", "normal answer"),
        json!({"input_tokens":2,"output_tokens":3,"total_tokens":5}),
    );
    let mut terminal: Value = serde_json::from_str(final_stream.last().unwrap()).unwrap();
    terminal["type"] = json!(format!("response.{ending}"));
    terminal["response"]["status"] = json!(ending);
    if ending == "failed" {
        terminal["response"]["error"] =
            json!({"code":"synthetic_failure","message":"controlled failure"});
    } else if ending == "incomplete" {
        terminal["response"]["incomplete_details"] = json!({"reason":"content_filter"});
    }
    *final_stream.last_mut().unwrap() = terminal.to_string();
    let transport = Arc::new(IdentityTransport {
        streams: Mutex::new(VecDeque::from([first, tool, final_stream])),
        calls: AtomicUsize::new(0),
        workspace: workspace.clone(),
    });
    let logs_path = root.path().join("diagnostics.sqlite");
    let logs = Arc::new(RuntimeLogStore::open(&logs_path).unwrap());
    let chat = ChatService::new(db.clone(), settings, transport.clone(), &workspace)
        .with_runtime_logs(logs.clone());
    let live = Arc::new(Mutex::new(Vec::new()));
    let callback = live.clone();
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        chat.run_in_session(
            "Create the synthetic file, then answer.",
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
            |_| panic!("low-risk synthetic tool must not require approval"),
        ),
    )
    .await
    .expect("bounded offline run");
    assert_eq!(result.is_ok(), ending == "completed", "{result:?}");
    assert_eq!(transport.calls.load(Ordering::SeqCst), 3);
    assert_eq!(
        std::fs::read_to_string(workspace.join("effect.txt")).unwrap(),
        "synthetic effect"
    );
    let live = live.lock().unwrap().clone();
    let mut response_id = "";
    let metadata: Vec<_> = live
        .iter()
        .filter(|event| event["type"] == "responses_stream_event")
        .collect();
    for event in &metadata {
        if event["event_type"] == "response.created" {
            response_id = event["response_id"].as_str().expect("created identity");
        }
        assert_eq!(event["response_id"], response_id);
        assert_eq!(
            event["provider_model"],
            if response_id == "attempt-lost" {
                "server-model-before"
            } else {
                SERVER_MODEL
            }
        );
    }
    assert_eq!(
        live.iter()
            .filter(|event| event["type"] == "model_attempt_reset")
            .count(),
        1
    );
    assert_eq!(
        live.iter()
            .filter(|event| event["type"] == "tool_started" && event["call_id"] == "h200-write")
            .count(),
        1
    );
    let events = chat.run_events(RUN, None).unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_type == "run_terminal")
            .count(),
        1
    );
    let durable: Vec<_> = events
        .iter()
        .filter(|event| event.event_type == "responses_stream_event")
        .collect();
    assert!(durable.iter().all(|event| event.data["provider_model"]
        == if event.data["response_id"] == "attempt-lost" {
            "server-model-before"
        } else {
            SERVER_MODEL
        }));
    for id in ["attempt-lost", "attempt-tool", "attempt-answer"] {
        assert!(durable
            .iter()
            .any(|event| event.data["event_type"] == "response.created"
                && event.data["response_id"] == id
                && event.data["sequence_number"] == 0));
    }
    assert!(durable.iter().any(
        |event| event.data["event_type"] == "response.output_item.added"
            && event.data["item_id"] == "item-h200-write"
            && event.data["response_id"] == "attempt-tool"
    ));
    assert!(durable.iter().any(
        |event| event.data["sequence_gap"] == 2 && event.data["response_id"] == "attempt-tool"
    ));
    assert!(durable.iter().any(
        |event| event.data["event_type"] == format!("response.{ending}")
            && event.data["response_id"] == "attempt-answer"
    ));
    assert!(!serde_json::to_string(&durable)
        .unwrap()
        .contains("PRIVATE_BODY"));
    let run = RunStore::new(&db).get(RUN).unwrap().unwrap();
    assert_eq!(
        run.terminal_kind.as_deref(),
        Some(if ending == "completed" {
            "succeeded"
        } else {
            "failed"
        })
    );
    let diagnostics = logs.recent_for_session(&run.session_id, 1000).unwrap();
    assert!(diagnostics
        .iter()
        .any(|entry| entry.data["response_id"] == "attempt-answer"
            && entry.data["provider_model"] == SERVER_MODEL));
    drop(chat);
    drop(logs);
    drop(db);
    let reopened = Database::open(&path).unwrap();
    let store = RunStore::new(&reopened);
    assert_eq!(store.events_after(RUN, None).unwrap(), events);
    for event in &events {
        assert_eq!(
            store.events_after(RUN, Some(event.sequence)).unwrap(),
            events
                .iter()
                .filter(|later| later.sequence > event.sequence)
                .cloned()
                .collect::<Vec<_>>()
        );
    }
    let diagnostics = RuntimeLogStore::open(logs_path)
        .unwrap()
        .recent_for_session(&run.session_id, 1000)
        .unwrap();
    assert!(diagnostics
        .iter()
        .any(|entry| entry.data["response_id"] == "attempt-answer"
            && entry.data["provider_model"] == SERVER_MODEL));
    assert_eq!(
        transport.calls.load(Ordering::SeqCst),
        3,
        "reopen never redispatches"
    );
}

#[tokio::test]
async fn h200_retry_tool_and_all_terminals_keep_identity_across_live_durable_and_reopen() {
    for ending in ["completed", "incomplete", "failed"] {
        observe(ending).await;
    }
}
