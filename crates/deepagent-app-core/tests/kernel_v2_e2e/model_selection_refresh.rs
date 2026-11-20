//! H-223: the shared settings owner feeds real preparation, wire and replay.
use super::*;
use deepagent_persistence::run_store::RunStore;

const MODELS: &str = r#"{"data":[{"id":"deepseek-v4-flash"},{"id":"deepseek-v4-pro"}]}"#;
const INPUT: &str = "Acknowledge the synthetic selection control.";

fn overrides(model: Option<&str>) -> HarnessRunOverrides {
    HarnessRunOverrides {
        model: model.map(str::to_owned),
        permission_profile: Some("read_only".into()),
        sandbox_backend: Some("direct".into()),
        ..Default::default()
    }
}

fn check_events(chat: &ChatService, id: &str, expected: &str, snapshot: &serde_json::Value) {
    let events = chat.run_events(id, None).unwrap();
    let requests: Vec<_> = events
        .iter()
        .filter(|e| e.event_type == "model_request_started")
        .collect();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].data["model"], expected);
    assert_eq!(
        events
            .iter()
            .find(|e| e.event_type == "capability.snapshot")
            .unwrap()
            .data,
        *snapshot
    );
    let terminals: Vec<_> = events
        .iter()
        .filter(|e| e.event_type == "run_terminal")
        .collect();
    assert_eq!(terminals.len(), 1);
    assert_eq!(terminals[0].data["kind"], "succeeded");
}

async fn prepared_run(
    chat: &ChatService,
    id: &str,
    selected: Option<&str>,
    expected: &str,
) -> serde_json::Value {
    let config = chat.read_execution_config(&overrides(selected)).unwrap();
    assert_eq!(config["model"], expected);
    let session = chat.create_session(None).unwrap();
    let prepared = chat
        .admit_prepared_input(id, &session, INPUT, overrides(selected))
        .unwrap();
    let snapshot = prepared.capability_snapshot().clone();
    assert_eq!(snapshot["context"]["executionConfig"]["model"], expected);
    chat.run_prepared(prepared, INPUT, |_| {}, |_| {})
        .await
        .unwrap();
    check_events(chat, id, expected, &snapshot);
    snapshot
}

#[tokio::test]
async fn h223_refresh_selection_override_and_reopen_match_real_wire_and_durable_snapshot() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("state.sqlite");
    let db = Arc::new(Database::open(&path).unwrap());
    let discovery = Arc::new(MockTransport::with_get_json(MODELS));
    let secrets = Arc::new(MemorySecretStore::new());
    let settings = Arc::new(SettingsService::new(
        db.clone(),
        discovery.clone(),
        secrets.clone(),
    ));
    crate::registry_fixture::configure(&settings, "h223-synthetic-control")
        .await
        .unwrap();
    activate_selection_model(&db, &settings, KNOWN_OVERRIDE);
    settings
        .harness_models()
        .select_model("deepseek", KNOWN_OVERRIDE)
        .unwrap();

    settings
        .harness_models()
        .fetch_models("deepseek")
        .await
        .unwrap(); // Same service used by desktop startup.
    let model = Arc::new(SelectionModel::default());
    let chat = ChatService::new(db.clone(), settings.clone(), model.clone(), root.path());
    let first = prepared_run(&chat, "h223-selected", None, KNOWN_OVERRIDE).await;
    let second = prepared_run(&chat, "h223-override", Some(DEFAULT_MODEL), DEFAULT_MODEL).await;
    assert!(
        chat.read_execution_config(&overrides(None))
            .unwrap()
            .get("fallbackModel")
            .is_none(),
        "overload fallback was removed; the snapshot must not publish a fallback model"
    );
    drop(chat);
    drop(settings);
    drop(db); // Close every database owner, not just open a second connection.

    let reopened = Arc::new(Database::open(&path).unwrap());
    let settings = Arc::new(SettingsService::new(
        reopened.clone(),
        discovery,
        secrets.clone(),
    ));
    let selected = settings.harness_models().selected_model().unwrap().unwrap();
    assert_eq!(selected.model_name, KNOWN_OVERRIDE);
    assert_eq!(selected.provider_id, "deepseek");
    let store = RunStore::new(&reopened);
    for (id, snapshot) in [("h223-selected", first), ("h223-override", second)] {
        assert_eq!(
            store.get(id).unwrap().unwrap().terminal_kind.as_deref(),
            Some("succeeded")
        );
        let events = store.events_after(id, None).unwrap();
        assert_eq!(
            events
                .iter()
                .filter(|e| e.event_type == "run_terminal")
                .count(),
            1
        );
        assert_eq!(
            events
                .iter()
                .find(|e| e.event_type == "capability.snapshot")
                .unwrap()
                .data,
            snapshot
        );
    }
    let chat = ChatService::new(
        reopened.clone(),
        settings.clone(),
        model.clone(),
        root.path(),
    );
    // Discovery adds available models; it does not erase saved model settings
    // or silently retarget a selection when a supplier omits an existing id.
    let before = settings.load().unwrap();
    let removed = SettingsService::new(
        reopened,
        Arc::new(MockTransport::with_get_json(
            r#"{"data":[{"id":"deepseek-v4-flash"}]}"#,
        )),
        secrets,
    );
    removed
        .harness_models()
        .fetch_models("deepseek")
        .await
        .unwrap();
    assert_eq!(settings.load().unwrap(), before);
    prepared_run(&chat, "h223-reopened", None, KNOWN_OVERRIDE).await;
    assert_eq!(
        *model.0.lock().unwrap(),
        [KNOWN_OVERRIDE, DEFAULT_MODEL, KNOWN_OVERRIDE]
    );
}

#[tokio::test]
async fn h223_refresh_does_not_retarget_an_admitted_run_but_next_admission_uses_latest_choice() {
    let root = tempfile::tempdir().unwrap();
    let db = Arc::new(Database::open(root.path().join("state.sqlite")).unwrap());
    let discovery = Arc::new(MockTransport::with_get_json(MODELS));
    let secrets = Arc::new(MemorySecretStore::new());
    let settings = Arc::new(SettingsService::new(db.clone(), discovery, secrets));
    crate::registry_fixture::configure(&settings, "h223-synthetic-control")
        .await
        .unwrap();
    activate_selection_model(&db, &settings, KNOWN_OVERRIDE);
    settings
        .harness_models()
        .select_model("deepseek", KNOWN_OVERRIDE)
        .unwrap();
    let model = Arc::new(SelectionModel::default());
    let chat = ChatService::new(db, settings.clone(), model.clone(), root.path());
    let session = chat.create_session(None).unwrap();
    let prepared = chat
        .admit_prepared_input("h223-frozen", &session, INPUT, overrides(None))
        .unwrap();
    let snapshot = prepared.capability_snapshot().clone();
    assert_eq!(
        snapshot["context"]["executionConfig"]["model"],
        KNOWN_OVERRIDE
    );
    settings
        .harness_models()
        .select_model("deepseek", DEFAULT_MODEL)
        .unwrap();
    settings
        .harness_models()
        .fetch_models("deepseek")
        .await
        .unwrap();
    assert_eq!(
        chat.read_execution_config(&overrides(None)).unwrap()["model"],
        DEFAULT_MODEL
    );
    chat.run_prepared(prepared, INPUT, |_| {}, |_| {})
        .await
        .unwrap();
    check_events(&chat, "h223-frozen", KNOWN_OVERRIDE, &snapshot);
    prepared_run(&chat, "h223-next", None, DEFAULT_MODEL).await;
    assert_eq!(*model.0.lock().unwrap(), [KNOWN_OVERRIDE, DEFAULT_MODEL]);
}
