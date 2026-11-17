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
async fn h157_kernel_default_selection_survives_reopen_without_pinning_the_old_effective_value() {
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
        settings.initialize("synthetic-h157").await.unwrap();
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
    settings.refresh_models().await.unwrap();
    // Registry model edits, rather than automatic legacy discovery, publish the
    // new capability used by the next turn.
    model_selection::activate_selection_model(&db, &settings, "deepseek-flash");
    assert!(!settings.load().unwrap().unwrap().reasoning_effort_explicit);
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
    assert_eq!(requests[0]["reasoning"]["effort"], "high");
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
    settings.initialize("synthetic-h157").await.unwrap();
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
