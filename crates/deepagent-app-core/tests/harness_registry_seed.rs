//! Onboarding and legacy settings publish the same single active harness route.
use std::sync::Arc;

use deepagent_app_core::{MemorySecretStore, SecretStore, SettingsService};
use deepagent_core::clock::Timestamp;
use deepagent_models::MockTransport;
use deepagent_persistence::{document_store::DocumentStore, Database};

fn service(db: Arc<Database>) -> SettingsService {
    SettingsService::new(
        db,
        Arc::new(MockTransport::with_get_json(
            r#"{"data":[{"id":"deepseek-flash"},{"id":"deepseek-v4-pro"}]}"#,
        )),
        Arc::new(MemorySecretStore::new()),
    )
}

fn legacy_settings(db: &Database) {
    DocumentStore::new(db)
        .put(
            "settings",
            "app",
            &serde_json::json!({
                "catalog": {"base_url":"https://api.deepseek.com", "available":[
                    {"id":"legacy-chat", "context_window":64000, "max_output_tokens":8192,
                        "input_modalities":["text"]},
                    {"id":"legacy-reasoner"}
                ], "chat_model":"legacy-chat", "reasoner_model":"legacy-reasoner"},
                "discovered_at":1, "sandbox_mode":"workspace_write"
            })
            .to_string(),
            None,
            Timestamp::from_millis(1),
        )
        .unwrap();
}

#[tokio::test]
async fn first_sign_in_publishes_one_active_model_with_the_original_key_reference() {
    let db = Arc::new(Database::open_in_memory().unwrap());
    let secrets = Arc::new(MemorySecretStore::new());
    let settings = SettingsService::new(
        db.clone(),
        Arc::new(MockTransport::with_get_json(
            r#"{"data":[{"id":"deepseek-flash"},{"id":"deepseek-v4-pro"}]}"#,
        )),
        secrets.clone(),
    );

    let view = settings
        .initialize("synthetic-onboarding-key")
        .await
        .unwrap();
    let routes = db.active_harness_models().unwrap();
    assert_eq!(routes.len(), 1);
    assert_eq!(routes[0].0.id, "deepseek");
    assert_eq!(routes[0].0.api_key_ref, "deepseek_api_key");
    assert_eq!(routes[0].1.model_name, view.chat_model);
    assert_eq!(
        secrets.get(&routes[0].0.api_key_ref).unwrap().as_deref(),
        Some("synthetic-onboarding-key")
    );
    assert!(secrets.get("harness_provider:deepseek").unwrap().is_none());
}

#[test]
fn legacy_load_imports_only_the_chat_selection_and_preserves_its_capacities() {
    let db = Arc::new(Database::open_in_memory().unwrap());
    legacy_settings(&db);
    service(db.clone()).load().unwrap();

    let routes = db.active_harness_models().unwrap();
    assert_eq!(routes.len(), 1);
    assert_eq!(routes[0].1.model_name, "legacy-chat");
    assert_eq!(routes[0].1.context_window, 64000);
    assert_eq!(routes[0].1.output_limit, 8192);
    let capabilities: serde_json::Value = serde_json::from_str(&routes[0].1.capabilities).unwrap();
    assert_eq!(
        capabilities["input_modalities"],
        serde_json::json!(["text"])
    );
    assert_eq!(db.list_harness_models("deepseek").unwrap().len(), 1);
}

#[test]
fn repeated_legacy_load_does_not_reactivate_or_recreate_user_removed_routes() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("seed.sqlite");
    {
        let db = Arc::new(Database::open(&path).unwrap());
        legacy_settings(&db);
        // Keep an encrypted-record-shaped legacy key across cold reopen. The
        // database's compatibility importer must not resurrect a deleted route.
        db.with_conn(|conn| {
            conn.execute(
                "INSERT INTO secret_records (name, ciphertext, nonce, key_version, updated_at)
                 VALUES ('deepseek_api_key', x'00', x'00', 1, 1)",
                [],
            )
            .map(|_| ())
            .map_err(|error| deepagent_core::error::CoreError::Persistence(error.to_string()))
        })
        .unwrap();
        let settings = service(db.clone());
        settings.load().unwrap();
        assert!(db
            .set_harness_model_active("deepseek", "legacy-chat", false)
            .unwrap());
        settings.load().unwrap();
        assert!(db.active_harness_models().unwrap().is_empty());
        assert!(db.remove_harness_provider("deepseek").unwrap());
        settings.load().unwrap();
        assert!(db.list_harness_providers().unwrap().is_empty());
    }
    let db = Arc::new(Database::open(&path).unwrap());
    service(db.clone()).load().unwrap();
    assert!(db.list_harness_providers().unwrap().is_empty());
}

#[test]
fn registry_seed_failure_cannot_publish_half_a_route() {
    let db = Arc::new(Database::open_in_memory().unwrap());
    legacy_settings(&db);
    db.with_conn(|conn| {
        conn.execute_batch(
            "CREATE TRIGGER reject_seed BEFORE INSERT ON harness_models
            BEGIN SELECT RAISE(ABORT, 'synthetic seed write failure'); END;",
        )
        .map_err(|error| deepagent_core::error::CoreError::Persistence(error.to_string()))
    })
    .unwrap();
    assert!(service(db.clone()).load().is_err());
    assert!(db.list_harness_providers().unwrap().is_empty());
    db.with_conn(|conn| {
        conn.execute_batch("DROP TRIGGER reject_seed;")
            .map_err(|error| deepagent_core::error::CoreError::Persistence(error.to_string()))
    })
    .unwrap();
    service(db.clone()).load().unwrap();
    assert_eq!(db.active_harness_models().unwrap().len(), 1);
}

#[test]
fn uninitialized_settings_do_not_create_a_default_route() {
    let db = Arc::new(Database::open_in_memory().unwrap());
    assert!(service(db.clone()).load().unwrap().is_none());
    assert!(db.list_harness_providers().unwrap().is_empty());
}

#[tokio::test]
async fn sign_in_seed_failure_keeps_credentials_unavailable_until_successful_recovery() {
    let db = Arc::new(Database::open_in_memory().unwrap());
    db.with_conn(|conn| {
        conn.execute_batch(
            "CREATE TRIGGER reject_seed BEFORE INSERT ON harness_models
             BEGIN SELECT RAISE(ABORT, 'synthetic seed failure'); END;",
        )
        .map_err(|error| deepagent_core::error::CoreError::Persistence(error.to_string()))
    })
    .unwrap();
    let settings = service(db.clone());
    assert!(settings
        .initialize("synthetic-failed-sign-in")
        .await
        .is_err());
    assert!(
        settings.api_key().is_err(),
        "a partially published sign-in must fail closed"
    );
    assert!(db.active_harness_models().unwrap().is_empty());
    db.with_conn(|conn| {
        conn.execute_batch("DROP TRIGGER reject_seed;")
            .map_err(|error| deepagent_core::error::CoreError::Persistence(error.to_string()))
    })
    .unwrap();
    settings
        .initialize("synthetic-recovered-sign-in")
        .await
        .unwrap();
    assert_eq!(
        settings.api_key().unwrap().as_deref(),
        Some("synthetic-recovered-sign-in")
    );
    assert_eq!(db.active_harness_models().unwrap().len(), 1);
}

#[test]
fn legacy_seed_preserves_existing_provider_and_model_edits() {
    use deepagent_persistence::harness_registry::{HarnessModel, HarnessProvider};
    let db = Arc::new(Database::open_in_memory().unwrap());
    legacy_settings(&db);
    let provider = HarnessProvider {
        id: "deepseek".into(),
        channel: "Edited route".into(),
        logo: "edited".into(),
        model_url: "https://edited.test/v1".into(),
        protocol: "messages".into(),
        api_key_ref: "edited-secret-ref".into(),
        balance_kind: "none".into(),
        enabled: false,
        revision: 0,
    };
    db.save_harness_provider(&provider).unwrap();
    let model = HarnessModel {
        provider_id: "deepseek".into(),
        model_name: "legacy-chat".into(),
        display_name: "Edited model".into(),
        protocol: Some("responses".into()),
        context_window: 32000,
        output_limit: 4096,
        capabilities: "{\"supports_tools\":false}".into(),
        active: false,
    };
    db.save_harness_models("deepseek", &[model.clone()])
        .unwrap();
    let saved = db.list_harness_providers().unwrap();
    service(db.clone()).load().unwrap();
    assert_eq!(db.list_harness_providers().unwrap(), saved);
    assert_eq!(db.list_harness_models("deepseek").unwrap(), [model]);
    assert!(db.active_harness_models().unwrap().is_empty());
}
