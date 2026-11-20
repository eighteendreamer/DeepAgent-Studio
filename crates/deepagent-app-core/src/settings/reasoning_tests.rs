//! Reasoning selection is owned by the persisted provider/model route.
use super::*;
use crate::MemorySecretStore;
use deepagent_models::MockTransport;

fn service(db: Arc<Database>, secrets: Arc<MemorySecretStore>) -> SettingsService {
    SettingsService::new(
        db,
        Arc::new(MockTransport::with_get_json(
            r#"{"data":[{"id":"deepseek-flash"},{"id":"deepseek-v4-pro"}]}"#,
        )),
        secrets,
    )
}

#[tokio::test]
async fn selected_effort_matches_the_admitted_route_and_survives_reopen() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("settings.db");
    let secrets = Arc::new(MemorySecretStore::new());
    {
        let db = Arc::new(Database::open(&path).unwrap());
        let settings = service(db.clone(), secrets.clone());
        crate::registry_fixture::configure(&settings, "synthetic-effort")
            .await
            .unwrap();
        settings.set_reasoning_effort(ReasoningEffort::Low).unwrap();
        let selection = db.harness_chat_model_selection().unwrap().unwrap();
        assert_eq!(selection.reasoning_effort.as_deref(), Some("low"));
        let plan = crate::model_runtime::resolve_run_model(
            &db,
            &settings.load().unwrap().unwrap(),
            None,
            None,
            None,
        )
        .unwrap();
        assert_eq!(plan.reasoning_effort, Some(ReasoningEffort::Low));
        assert_eq!(settings.view().unwrap().unwrap().reasoning_effort, "low");
        settings
            .harness_models()
            .fetch_models("deepseek")
            .await
            .unwrap();
        assert_eq!(
            db.harness_chat_model_selection().unwrap().unwrap(),
            selection
        );
    }
    let reader = service(Arc::new(Database::open(&path).unwrap()), secrets);
    assert_eq!(reader.reasoning_effort().unwrap(), ReasoningEffort::Low);
    assert_eq!(reader.view().unwrap().unwrap().reasoning_effort, "low");
}

#[tokio::test]
async fn failed_selection_save_preserves_the_previous_effort() {
    let db = Arc::new(Database::open_in_memory().unwrap());
    let settings = service(db.clone(), Arc::new(MemorySecretStore::new()));
    crate::registry_fixture::configure(&settings, "synthetic-effort")
        .await
        .unwrap();
    let before = db.harness_chat_model_selection().unwrap().unwrap();
    db.with_conn(|conn| conn.execute_batch("CREATE TRIGGER reject_selection BEFORE UPDATE ON documents WHEN OLD.collection='harness_selection' AND OLD.id='chat' BEGIN SELECT RAISE(ABORT, 'synthetic selection failure'); END;").map_err(|error| CoreError::Persistence(error.to_string()))).unwrap();
    assert!(settings.set_reasoning_effort(ReasoningEffort::Low).is_err());
    assert_eq!(db.harness_chat_model_selection().unwrap().unwrap(), before);
}

#[test]
fn no_model_means_no_global_reasoning_fallback() {
    let settings = service(
        Arc::new(Database::open_in_memory().unwrap()),
        Arc::new(MemorySecretStore::new()),
    );
    assert!(settings
        .set_reasoning_effort(ReasoningEffort::High)
        .is_err());
    assert!(settings
        .harness_models()
        .selected_model()
        .unwrap()
        .is_none());
}
