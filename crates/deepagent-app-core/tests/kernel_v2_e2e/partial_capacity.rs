//! H-296: settings projection, durable catalog, real Kernel wire and meter agree.
use super::*;
use deepagent_app_core::chat_service::HarnessRunOverrides;
use deepagent_app_core::settings::ResponsesApiSettings;
use deepagent_models::CapabilitySource;
use deepagent_runtime::RuntimeEvent;

fn discovery(output: u32) -> Arc<MockTransport> {
    Arc::new(MockTransport::with_get_json(
        serde_json::json!({"data":[
            {"id":"deepseek-flash","max_output_tokens":output}
        ]})
        .to_string(),
    ))
}

#[derive(Default)]
struct Recorder(Mutex<Vec<serde_json::Value>>);

#[async_trait]
impl HttpTransport for Recorder {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        self.0
            .lock()
            .unwrap()
            .push(serde_json::from_str(&request.body)?);
        let sink: &mut dyn EventSink = &mut crate::sequence_fixture::sink(sink);
        emit_responses_events(sink, &response_text("h296 persisted reply"))
    }
}

#[tokio::test]
async fn h296_partial_capacity_survives_reopen_and_refresh_changes_next_kernel_request() {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let path = root.path().join("settings.db");
    let secrets = Arc::new(MemorySecretStore::new());
    let mut session_id = None;
    for (generation, output) in [8_192, 2_048].into_iter().enumerate() {
        let db = Arc::new(Database::open(&path).unwrap());
        let settings = Arc::new(SettingsService::new(
            db.clone(),
            discovery(output),
            secrets.clone(),
        ));
        if generation == 0 {
            settings.initialize("synthetic-h296").await.unwrap();
            settings
                .set_responses_settings(ResponsesApiSettings {
                    max_output_tokens: Some(32_768),
                    ..Default::default()
                })
                .unwrap();
        } else {
            let old = settings.view().unwrap().unwrap();
            assert_eq!(old.model_capabilities[0].max_output_tokens, 8_192);
            settings.refresh_models().await.unwrap();
        }
        let view = settings.view().unwrap().unwrap();
        let cap = &view.model_capabilities[0];
        assert_eq!(cap.context_window, 1_000_000);
        assert_eq!(cap.max_output_tokens, output as usize);
        assert_eq!(cap.capability_source, CapabilitySource::Mixed);
        let encoded = serde_json::to_value(&view).unwrap();
        assert_eq!(
            encoded["model_capabilities"][0]["capability_source"],
            "mixed"
        );
        let raw = &settings.load().unwrap().unwrap().catalog.available[0];
        assert_eq!(raw.context_window, None);
        assert_eq!(raw.max_output_tokens, Some(u64::from(output)));
        let transport = Arc::new(Recorder::default());
        let chat = ChatService::new(db.clone(), settings, transport.clone(), &workspace);
        let snapshots = Arc::new(Mutex::new(Vec::new()));
        let captured = snapshots.clone();
        let run_id = format!("h296-kernel-{generation}");
        let id = chat
            .run_in_session_with_overrides(
                "Reply briefly without tools.",
                session_id.as_deref(),
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
            .unwrap();
        session_id = Some(id);
        let requests = transport.0.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0]["max_output_tokens"], output);
        if generation == 1 {
            assert!(requests[0]["input"]
                .to_string()
                .contains("h296 persisted reply"));
        }
        let meter = snapshots.lock().unwrap();
        assert!(!meter.is_empty());
        assert!(meter
            .iter()
            .all(|s| s.context_window == 1_000_000 && s.reserved_output_tokens == output as usize));
        let runs = deepagent_persistence::run_store::RunStore::new(&db);
        assert_eq!(
            runs.events_after(&run_id, None)
                .unwrap()
                .iter()
                .filter(|e| e.event_type == "run_terminal")
                .count(),
            1
        );
    }
}
