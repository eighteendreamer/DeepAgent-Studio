//! H-296: settings projection, durable catalog, real Kernel wire and meter agree.
use super::*;
use deepagent_app_core::chat_service::HarnessRunOverrides;
use deepagent_app_core::settings::ResponsesApiSettings;
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
            crate::registry_fixture::configure(&settings, "synthetic-h296")
                .await
                .unwrap();
            db.seed_responses_settings(ResponsesApiSettings {
                max_output_tokens: Some(32_768),
                ..Default::default()
            })
            .unwrap();
        } else {
            let models = settings.harness_models().list_models("deepseek").unwrap();
            assert_eq!(models[0].output_limit, 8_192);
            settings
                .harness_models()
                .fetch_models("deepseek")
                .await
                .unwrap();
            assert_eq!(
                settings.harness_models().list_models("deepseek").unwrap()[0].output_limit,
                8_192,
                "discovery cannot overwrite a user's persisted configuration"
            );
            let mut configured = db.list_harness_models("deepseek").unwrap().remove(0);
            configured.output_limit = output.into();
            db.save_harness_models("deepseek", &[configured]).unwrap();
        }
        let cap = crate::registry_fixture::client(&settings).capability("deepseek-flash");
        assert_eq!(cap.context_window, 1_000_000);
        assert_eq!(cap.max_output_tokens, output as usize);
        let raw = db.list_harness_models("deepseek").unwrap().remove(0);
        assert_eq!(raw.output_limit, i64::from(output));
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
