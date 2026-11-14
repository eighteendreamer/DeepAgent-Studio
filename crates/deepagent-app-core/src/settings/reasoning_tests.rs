//! H-157: persisted selection provenance is distinct from the model default.
use super::*;
use crate::MemorySecretStore;
use deepagent_models::{MockTransport, ModelConfig};

const CATALOG: &str = r#"{"data":[
 {"id":"deepseek-flash","effort":{"supported_levels":["low","high"],"default_level":"low"}},
 {"id":"deepseek-v4-pro","effort":{"supported_levels":["high"],"default_level":"high"}}
]}"#;

fn service(db: Arc<Database>, secrets: Arc<MemorySecretStore>) -> SettingsService {
    SettingsService::new(db, Arc::new(MockTransport::with_get_json(CATALOG)), secrets)
}

fn config(settings: &SettingsService) -> ModelConfig {
    crate::model_runtime::model_config(&settings.load().unwrap().unwrap(), ModelRole::Chat, None)
        .unwrap()
}

#[tokio::test]
async fn h157_new_selection_follows_model_default_and_manual_choice_survives_cold_reopen() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("settings.db");
    let secrets = Arc::new(MemorySecretStore::new());
    {
        let settings = service(Arc::new(Database::open(&path).unwrap()), secrets.clone());
        let view = settings.initialize("synthetic-h157").await.unwrap();
        assert!(!settings.load().unwrap().unwrap().reasoning_effort_explicit);
        assert_eq!(view.reasoning_effort, "low");
        assert_eq!(settings.reasoning_effort().unwrap(), ReasoningEffort::Low);
        let config = config(&settings);
        assert_eq!(config.configured_reasoning_effort(), None);
        assert_eq!(
            config
                .effective_reasoning_effort("deepseek-flash", None)
                .unwrap(),
            Some(ReasoningEffort::Low)
        );
        assert_eq!(
            config
                .effective_reasoning_effort("deepseek-v4-pro", None)
                .unwrap(),
            Some(ReasoningEffort::High)
        );
    }
    {
        let settings = service(Arc::new(Database::open(&path).unwrap()), secrets.clone());
        assert!(!settings.load().unwrap().unwrap().reasoning_effort_explicit);
        assert_eq!(settings.view().unwrap().unwrap().reasoning_effort, "low");
        settings.refresh_models().await.unwrap();
        assert!(!settings.load().unwrap().unwrap().reasoning_effort_explicit);
        assert_eq!(
            settings
                .set_reasoning_effort(ReasoningEffort::High)
                .unwrap()
                .reasoning_effort,
            "high"
        );
        assert!(settings.load().unwrap().unwrap().reasoning_effort_explicit);
    }
    let settings = service(Arc::new(Database::open(&path).unwrap()), secrets);
    assert!(settings.load().unwrap().unwrap().reasoning_effort_explicit);
    assert_eq!(
        config(&settings).configured_reasoning_effort(),
        Some(ReasoningEffort::High)
    );
    assert_eq!(settings.reasoning_effort().unwrap(), ReasoningEffort::High);
}

#[tokio::test]
async fn h157_legacy_document_without_provenance_preserves_saved_global_choice() {
    let db = Arc::new(Database::open_in_memory().unwrap());
    let secrets = Arc::new(MemorySecretStore::new());
    let settings = service(db.clone(), secrets.clone());
    settings.initialize("synthetic-h157").await.unwrap();
    let mut legacy = serde_json::to_value(settings.load().unwrap().unwrap()).unwrap();
    legacy
        .as_object_mut()
        .unwrap()
        .remove("reasoning_effort_explicit");
    legacy["reasoning_effort"] = "high".into();
    DocumentStore::new(&db)
        .put(
            SETTINGS_COLLECTION,
            SETTINGS_ID,
            &legacy.to_string(),
            None,
            Timestamp::from_millis(now_ms()),
        )
        .unwrap();
    let reader = service(db, secrets);
    assert!(reader.load().unwrap().unwrap().reasoning_effort_explicit);
    assert_eq!(reader.reasoning_effort().unwrap(), ReasoningEffort::High);
    assert_eq!(reader.view().unwrap().unwrap().reasoning_effort, "high");
}

#[tokio::test]
async fn h157_failed_manual_save_does_not_publish_provenance_or_effective_value() {
    let db = Arc::new(Database::open_in_memory().unwrap());
    let secrets = Arc::new(MemorySecretStore::new());
    let settings = service(db.clone(), secrets.clone());
    settings.initialize("synthetic-h157").await.unwrap();
    db.with_conn(|connection| {
        connection
            .execute_batch(
                "CREATE TRIGGER h157_reject BEFORE UPDATE ON documents
          WHEN OLD.collection = 'settings' AND OLD.id = 'app'
          BEGIN SELECT RAISE(ABORT, 'synthetic H157 write failure'); END;",
            )
            .map_err(|error| CoreError::Persistence(error.to_string()))?;
        Ok(())
    })
    .unwrap();
    assert!(settings
        .set_reasoning_effort(ReasoningEffort::High)
        .is_err());
    for reader in [&settings, &service(db, secrets)] {
        assert!(!reader.load().unwrap().unwrap().reasoning_effort_explicit);
        assert_eq!(reader.reasoning_effort().unwrap(), ReasoningEffort::Low);
    }
}

#[tokio::test]
async fn h157_responses_choices_override_model_defaults_without_sticking_after_clear() {
    let settings = service(
        Arc::new(Database::open_in_memory().unwrap()),
        Arc::new(MemorySecretStore::new()),
    );
    settings.initialize("synthetic-h157").await.unwrap();
    settings
        .set_responses_settings(ResponsesApiSettings {
            reasoning_effort: Some("high".into()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(
        config(&settings).configured_reasoning_effort(),
        Some(ReasoningEffort::High)
    );
    settings
        .set_responses_developer_json(r#"{"reasoning":{"effort":"none"}}"#)
        .unwrap();
    assert_eq!(settings.reasoning_effort().unwrap(), ReasoningEffort::None);
    assert_eq!(
        config(&settings).configured_reasoning_effort(),
        Some(ReasoningEffort::None)
    );
    settings.set_responses_developer_json("{}").unwrap();
    assert_eq!(settings.reasoning_effort().unwrap(), ReasoningEffort::High);
    settings
        .set_responses_settings(ResponsesApiSettings::default())
        .unwrap();
    assert_eq!(settings.reasoning_effort().unwrap(), ReasoningEffort::Low);
    assert_eq!(config(&settings).configured_reasoning_effort(), None);
    assert!(!settings.load().unwrap().unwrap().reasoning_effort_explicit);
}
