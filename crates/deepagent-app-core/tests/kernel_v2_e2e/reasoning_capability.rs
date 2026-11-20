//! H-157: real admission, Kernel request construction and durable continuation.
use super::*;
use deepagent_app_core::chat_service::HarnessRunOverrides;

fn discovery(default: &str, allow_high: bool) -> Arc<MockTransport> {
    let levels = if allow_high {
        vec!["low", "high"]
    } else {
        vec!["low"]
    };
    Arc::new(MockTransport::with_get_json(
        serde_json::json!({"data":[
            {"id":"deepseek-flash", "effort":{"supported_levels":levels,"default_level":default}},
            {"id":"deepseek-v4-pro", "effort":{"supported_levels":["high"],"default_level":"high"}}
        ]})
        .to_string(),
    ))
}

fn configure_thinking(settings: &SettingsService, default: &str, allow_high: bool) {
    use deepagent_app_core::harness_model_service::HarnessModelInput;
    let service = settings.harness_models();
    let model = service
        .list_models("deepseek")
        .unwrap()
        .into_iter()
        .find(|model| model.model_name == "deepseek-flash")
        .unwrap();
    service
        .save_model(HarnessModelInput {
            provider_id: model.provider_id,
            model_name: model.model_name,
            display_name: model.display_name,
            protocol: model.protocol,
            context_window: model.context_window,
            output_limit: model.output_limit,
            reasoning_efforts: Some(
                serde_json::from_value(if allow_high {
                    serde_json::json!({"low":"low","high":"high"})
                } else {
                    serde_json::json!({"low":"low"})
                })
                .unwrap(),
            ),
            default_reasoning_effort: Some(
                serde_json::from_value(serde_json::json!(default)).unwrap(),
            ),
            thinking_format: None,
        })
        .unwrap();
}

#[derive(Default)]
struct Recorder(Mutex<Vec<serde_json::Value>>);

#[async_trait]
impl HttpTransport for Recorder {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        let sink: &mut dyn EventSink = &mut crate::sequence_fixture::sink(sink);
        self.0
            .lock()
            .unwrap()
            .push(serde_json::from_str(&request.body)?);
        emit_responses_events(sink, &response_text("h157 persisted reply"))
    }
}

fn overrides(effort: Option<&str>) -> HarnessRunOverrides {
    HarnessRunOverrides {
        model: Some("deepseek-flash".into()),
        reasoning_effort: effort.map(str::to_owned),
        ..Default::default()
    }
}

#[tokio::test]
async fn h157_kernel_user_selection_survives_reopen_and_initial_level_edits() {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let database = root.path().join("state.db");
    let secrets = Arc::new(MemorySecretStore::new());
    let session_id;
    {
        let db = Arc::new(Database::open(&database).unwrap());
        let settings = Arc::new(SettingsService::new(
            db.clone(),
            discovery("low", true),
            secrets.clone(),
        ));
        crate::registry_fixture::configure(&settings, "synthetic-h157")
            .await
            .unwrap();
        configure_thinking(&settings, "low", true);
        settings
            .set_reasoning_effort(deepagent_models::ReasoningEffort::Low)
            .unwrap();
        let transport = Arc::new(Recorder::default());
        let chat = ChatService::new(db, settings, transport.clone(), &workspace);
        session_id = chat
            .run_in_session_with_overrides(
                "Reply briefly without tools.",
                None,
                None,
                None,
                Vec::new(),
                None,
                false,
                Some("h157-before-reopen"),
                overrides(None),
                |_| {},
                |_| {},
            )
            .await
            .unwrap();
        let requests = transport.0.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0]["reasoning"]["effort"], "low");
    }
    let db = Arc::new(Database::open(&database).unwrap());
    let settings = Arc::new(SettingsService::new(
        db.clone(),
        discovery("high", true),
        secrets,
    ));
    assert_eq!(
        settings.reasoning_effort().unwrap(),
        deepagent_models::ReasoningEffort::Low
    );
    settings
        .harness_models()
        .fetch_models("deepseek")
        .await
        .unwrap();
    // Editing the model's initial level changes its configuration, while the
    // user's explicit route selection remains authoritative after reopening.
    configure_thinking(&settings, "high", true);
    let selected = settings.harness_models().selected_model().unwrap().unwrap();
    assert_eq!(selected.default_reasoning_effort.as_deref(), Some("high"));
    assert_eq!(selected.reasoning_effort.as_deref(), Some("low"));
    assert!(serde_json::to_value(settings.load().unwrap().unwrap())
        .unwrap()
        .get("reasoning_effort_explicit")
        .is_none());
    let transport = Arc::new(Recorder::default());
    let chat = ChatService::new(db, settings, transport.clone(), &workspace);
    chat.run_in_session_with_overrides(
        "Continue briefly without tools.",
        Some(&session_id),
        None,
        None,
        Vec::new(),
        None,
        false,
        Some("h157-after-reopen"),
        overrides(None),
        |_| {},
        |_| {},
    )
    .await
    .unwrap();
    let requests = transport.0.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0]["reasoning"]["effort"], "low");
    assert!(requests[0]["input"]
        .to_string()
        .contains("h157 persisted reply"));
}

#[tokio::test]
async fn h157_kernel_rejects_explicit_unsupported_effort_before_model_transport() {
    let root = tempfile::tempdir().unwrap();
    let db = Arc::new(Database::open_in_memory().unwrap());
    let settings = Arc::new(SettingsService::new(
        db.clone(),
        discovery("low", false),
        Arc::new(MemorySecretStore::new()),
    ));
    crate::registry_fixture::configure(&settings, "synthetic-h157")
        .await
        .unwrap();
    configure_thinking(&settings, "low", false);
    let transport = Arc::new(Recorder::default());
    let chat = ChatService::new(db, settings, transport.clone(), root.path());
    let error = chat
        .run_in_session_with_overrides(
            "Never dispatch this unsupported effort.",
            None,
            None,
            None,
            Vec::new(),
            None,
            false,
            Some("h157-invalid-override"),
            overrides(Some("high")),
            |_| {},
            |_| {},
        )
        .await
        .unwrap_err();
    assert!(error
        .to_string()
        .contains("does not support reasoning effort"));
    assert!(transport.0.lock().unwrap().is_empty());
}
