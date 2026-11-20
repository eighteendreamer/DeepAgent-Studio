//! H-297: fresh Kernel hosts price durable native history with pinned templates.
use super::*;
use deepagent_app_core::chat_service::HarnessRunOverrides;
use deepagent_app_core::settings::ResponsesApiSettings;
use deepagent_runtime::RuntimeEvent;
use serde_json::{json, Value};

struct Recorder {
    requests: Mutex<Vec<Value>>,
    note_calls: AtomicUsize,
}

#[async_trait]
impl HttpTransport for Recorder {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        let body: Value = serde_json::from_str(&request.body)?;
        // Real usage=60k opens the existing post-run note gate. Route that
        // auxiliary call by its established note identity, not queue position;
        // it is not another main prompt or a token-counter request.
        if request.body.contains("session-memory.md") {
            self.note_calls.fetch_add(1, Ordering::SeqCst);
            return emit_responses_events(sink, &response_text("No note edit in this fixture."));
        }
        self.requests.lock().unwrap().push(body);
        let mut events = response_text("H297 持久化答案");
        let mut terminal: Value = serde_json::from_str(events.last().unwrap())?;
        terminal["response"]["usage"] =
            json!({"input_tokens":60000,"output_tokens":10,"total_tokens":60010});
        *events.last_mut().unwrap() = terminal.to_string();
        emit_responses_events(sink, &events)
    }
}

#[tokio::test]
async fn h297_kernel_cold_reopen_keeps_history_but_not_a_stale_usage_anchor() {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let path = root.path().join("state.sqlite");
    let secrets = Arc::new(MemorySecretStore::new());
    let prompt = "请保留工具调用结果并核对每一步证据。".repeat(100);
    let mut session = None;
    for (generation, output) in [8192, 4096].into_iter().enumerate() {
        let db = Arc::new(Database::open(&path).unwrap());
        let settings = Arc::new(SettingsService::new(
            db.clone(),
            Arc::new(MockTransport::with_get_json(
                json!({"data":[{"id":"deepseek-flash"}]}).to_string(),
            )),
            secrets.clone(),
        ));
        if generation == 0 {
            crate::registry_fixture::configure(&settings, "synthetic-h297")
                .await
                .unwrap();
        }
        db.seed_responses_settings(ResponsesApiSettings {
            max_output_tokens: Some(output),
            ..Default::default()
        })
        .unwrap();
        let snapshots = Arc::new(Mutex::new(Vec::new()));
        let model = Arc::new(Recorder {
            requests: Mutex::new(Vec::new()),
            note_calls: AtomicUsize::new(0),
        });
        let chat = ChatService::new(db.clone(), settings, model.clone(), &workspace);
        let captured = snapshots.clone();
        let run_id = format!("h297-kernel-{generation}");
        session = Some(
            chat.run_in_session_with_overrides(
                &prompt,
                session.as_deref(),
                None,
                None,
                Vec::new(),
                None,
                false,
                Some(&run_id),
                HarnessRunOverrides {
                    model: Some("deepseek-flash".into()),
                    ..Default::default()
                },
                move |event| {
                    if let RuntimeEvent::ContextUsage { snapshot } = event {
                        captured.lock().unwrap().push(snapshot.clone());
                    }
                },
                |_| {},
            )
            .await
            .unwrap(),
        );
        assert!(
            chat.wait_for_session_memory_passes(std::time::Duration::from_secs(10))
                .await
        );
        let requests = model.requests.lock().unwrap();
        eprintln!(
            "h297 kernel routes: {}",
            json!({"generation":generation,"notes":model.note_calls.load(Ordering::SeqCst),"requests":requests.iter().map(|body|json!({"model":body["model"],"output":body["max_output_tokens"],"instructions_bytes":body["instructions"].as_str().unwrap_or_default().len(),"items":body["input"].as_array().map(Vec::len),"main_prompt":body["input"].to_string().contains(&prompt),"compaction":body["instructions"].as_str().unwrap_or_default().starts_with("You compress an agent")})).collect::<Vec<_>>()})
        );
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0]["max_output_tokens"], output);
        if generation == 1 {
            assert!(requests[0]["input"].to_string().contains("H297 持久化答案"));
        }
        let snapshots = snapshots.lock().unwrap();
        assert!(snapshots.iter().all(|snapshot| snapshot
            .blocks
            .iter()
            .all(|block| block.source.contains(":v41:"))));
        assert!(snapshots
            .iter()
            .all(|snapshot| snapshot.estimated_prompt_tokens < 60_000));
        assert!(
            snapshots
                .iter()
                .any(|snapshot| snapshot.estimated_prompt_tokens >= 1000
                    && snapshot.occupancy_tokens() == snapshot.estimated_prompt_tokens),
            "cold request must project local tokens before receiving its own usage"
        );
        let last = snapshots.last().unwrap();
        assert_eq!(last.last_request_prompt_tokens, Some(60000));
        assert!(last.occupancy_tokens() >= 60000);
        assert_eq!(
            last.blocks.iter().map(|block| block.tokens).sum::<usize>(),
            last.occupancy_tokens()
        );
        assert_eq!(
            deepagent_persistence::run_store::RunStore::new(&db)
                .events_after(&run_id, None)
                .unwrap()
                .iter()
                .filter(|event| event.event_type == "run_terminal")
                .count(),
            1
        );
        // Every owner is dropped before the next file-DB reopen.
    }
}
