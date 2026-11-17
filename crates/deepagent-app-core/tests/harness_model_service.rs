//! Harness model service: provider + model management for the main settings
//! "模型" page (plan §3, §8). Canvas providers are not involved.

use std::sync::Arc;

use deepagent_app_core::harness_model_service::{
    HarnessModelService, HarnessModelView, HarnessProviderInput,
};
use deepagent_app_core::{MemorySecretStore, SecretStore};
use deepagent_models::transport::{HttpTransport, MockTransport};
use deepagent_persistence::harness_registry::HarnessProvider;
use deepagent_persistence::Database;

fn service_with_models_response(body: &str) -> (HarnessModelService, Arc<MemorySecretStore>) {
    let db = Arc::new(Database::open_in_memory().unwrap());
    let secrets = Arc::new(MemorySecretStore::new());
    let transport: Arc<dyn HttpTransport> = Arc::new(MockTransport::with_get_json(body));
    let service = HarnessModelService::new(db, secrets.clone(), transport);
    (service, secrets)
}

fn input(id: &str, api_key: Option<&str>) -> HarnessProviderInput {
    HarnessProviderInput {
        id: Some(id.into()),
        channel: format!("{id} channel"),
        logo: String::new(),
        model_url: "https://provider.test/v1".into(),
        protocol: "chat_completions".into(),
        balance_kind: "auto".into(),
        enabled: true,
        api_key: api_key.map(str::to_string),
    }
}

// V24 imports the existing key by reference rather than copying its plaintext.
fn legacy_provider_service() -> (HarnessModelService, Arc<MemorySecretStore>, Arc<Database>) {
    let db = Arc::new(Database::open_in_memory().unwrap());
    db.save_harness_provider(&HarnessProvider {
        id: "deepseek".into(),
        channel: "DeepSeek".into(),
        logo: String::new(),
        model_url: "https://api.deepseek.com".into(),
        protocol: "responses".into(),
        api_key_ref: "deepseek_api_key".into(),
        balance_kind: "official".into(),
        enabled: true,
        revision: 0,
    })
    .unwrap();
    let secrets = Arc::new(MemorySecretStore::new());
    secrets.set("deepseek_api_key", "test-legacy-key").unwrap();
    let transport = Arc::new(MockTransport::with_get_json(
        "{\"data\":[{\"id\":\"deepseek-flash\"}]}",
    ));
    (
        HarnessModelService::new(db.clone(), secrets.clone(), transport),
        secrets,
        db,
    )
}

#[test]
fn legacy_provider_metadata_edits_keep_the_imported_key_reference() {
    let (service, secrets, db) = legacy_provider_service();
    assert!(service.list_providers().unwrap()[0].has_api_key);

    for key in [None, Some("   ")] {
        let saved = service.save_provider(input("deepseek", key)).unwrap();
        assert!(
            saved.has_api_key,
            "metadata edits must retain the imported key"
        );
        assert_eq!(
            db.list_harness_providers().unwrap()[0].api_key_ref,
            "deepseek_api_key"
        );
        assert_eq!(
            futures_block_on(service.fetch_models("deepseek"))
                .unwrap()
                .len(),
            1
        );
    }
    assert_eq!(
        secrets.get("deepseek_api_key").unwrap().as_deref(),
        Some("test-legacy-key")
    );
    assert!(secrets.get("harness_provider:deepseek").unwrap().is_none());
}

#[test]
fn legacy_provider_key_rotation_updates_the_referenced_secret_without_copying_it() {
    let (service, secrets, db) = legacy_provider_service();

    let saved = service
        .save_provider(input("deepseek", Some("test-rotated-key")))
        .unwrap();

    assert!(saved.has_api_key);
    assert_eq!(
        db.list_harness_providers().unwrap()[0].api_key_ref,
        "deepseek_api_key"
    );
    assert_eq!(
        secrets.get("deepseek_api_key").unwrap().as_deref(),
        Some("test-rotated-key")
    );
    assert!(secrets.get("harness_provider:deepseek").unwrap().is_none());
}

#[test]
fn legacy_provider_removal_deletes_its_referenced_secret() {
    let (service, secrets, _db) = legacy_provider_service();
    secrets
        .set("another-provider-key", "test-unrelated-key")
        .unwrap();

    assert!(service.remove_provider("deepseek").unwrap());

    assert!(secrets.get("deepseek_api_key").unwrap().is_none());
    assert_eq!(
        secrets.get("another-provider-key").unwrap().as_deref(),
        Some("test-unrelated-key")
    );
    assert!(service.list_providers().unwrap().is_empty());
}

#[test]
fn saving_a_key_stores_it_as_a_secret_and_never_returns_it() {
    let (service, secrets) = service_with_models_response("{\"data\":[]}");

    let view = service
        .save_provider(input("alpha", Some("sk-secret-123")))
        .unwrap();

    assert!(view.has_api_key);
    assert_eq!(
        secrets.get("harness_provider:alpha").unwrap().as_deref(),
        Some("sk-secret-123")
    );
    let json = serde_json::to_string(&view).unwrap();
    assert!(!json.contains("sk-secret"), "view leaked the key: {json}");
}

#[test]
fn saving_without_a_key_keeps_the_existing_key() {
    let (service, secrets) = service_with_models_response("{\"data\":[]}");
    service
        .save_provider(input("alpha", Some("sk-first")))
        .unwrap();

    let view = service.save_provider(input("alpha", None)).unwrap();

    assert!(view.has_api_key);
    assert_eq!(
        secrets.get("harness_provider:alpha").unwrap().as_deref(),
        Some("sk-first")
    );
    assert_eq!(view.revision, 2);
}

#[test]
fn fetched_models_are_stored_inactive_under_their_provider() {
    let (service, _secrets) = service_with_models_response(
        "{\"object\":\"list\",\"data\":[{\"id\":\"m-a\"},{\"id\":\"m-b\"}]}",
    );
    service
        .save_provider(input("alpha", Some("sk-test")))
        .unwrap();

    let fetched = futures_block_on(service.fetch_models("alpha")).unwrap();

    let names: Vec<(String, bool)> = fetched
        .iter()
        .map(|model| (model.model_name.clone(), model.active))
        .collect();
    assert_eq!(
        names,
        vec![("m-a".to_string(), false), ("m-b".to_string(), false)]
    );
    assert_eq!(service.list_models("alpha").unwrap().len(), 2);
    assert!(service.active_models().unwrap().is_empty());
}

#[test]
fn model_views_expose_ui_fields_in_camel_case() {
    let (service, _secrets) = service_with_models_response("{\"data\":[{\"id\":\"m-a\"}]}");
    service
        .save_provider(input("alpha", Some("sk-test")))
        .unwrap();
    futures_block_on(service.fetch_models("alpha")).unwrap();
    service.set_model_active("alpha", "m-a", true).unwrap();

    let listed: Vec<HarnessModelView> = service.list_models("alpha").unwrap();

    assert_eq!(listed.len(), 1);
    let json = serde_json::to_value(&listed[0]).unwrap();
    assert_eq!(json["providerId"], "alpha");
    assert_eq!(json["modelName"], "m-a");
    assert_eq!(json["displayName"], "m-a");
    assert!(json["protocol"].is_null(), "null means inherit: {json}");
    assert_eq!(json["contextWindow"], 1_000_000);
    assert_eq!(json["outputLimit"], 131_072);
    assert_eq!(json["active"], true);
}

#[test]
fn fetching_without_a_saved_key_is_refused() {
    let (service, _secrets) = service_with_models_response("{\"data\":[]}");
    service.save_provider(input("alpha", None)).unwrap();

    let error =
        futures_block_on(service.fetch_models("alpha")).expect_err("no key means no request");

    assert!(error.to_string().contains("API key"), "{error}");
}

#[test]
fn activated_models_report_their_effective_protocol_and_endpoint() {
    let (service, _secrets) = service_with_models_response("{\"data\":[{\"id\":\"m-a\"}]}");
    service
        .save_provider(input("alpha", Some("sk-test")))
        .unwrap();
    futures_block_on(service.fetch_models("alpha")).unwrap();
    service.set_model_active("alpha", "m-a", true).unwrap();

    let active = service.active_models().unwrap();

    assert_eq!(active.len(), 1);
    assert_eq!(active[0].provider_id, "alpha");
    assert_eq!(active[0].model_name, "m-a");
    assert_eq!(
        active[0].protocol, "chat_completions",
        "inherits the provider protocol"
    );
    assert_eq!(active[0].model_url, "https://provider.test/v1");
}

#[test]
fn removing_a_provider_deletes_its_key_and_models() {
    let (service, secrets) = service_with_models_response("{\"data\":[{\"id\":\"m-a\"}]}");
    service
        .save_provider(input("alpha", Some("sk-test")))
        .unwrap();
    futures_block_on(service.fetch_models("alpha")).unwrap();

    assert!(service.remove_provider("alpha").unwrap());

    assert!(secrets.get("harness_provider:alpha").unwrap().is_none());
    assert!(service.list_providers().unwrap().is_empty());
    assert!(service.list_models("alpha").unwrap().is_empty());
}

#[test]
fn unknown_protocol_is_rejected() {
    let (service, _secrets) = service_with_models_response("{\"data\":[]}");
    let mut bad = input("alpha", Some("sk-test"));
    bad.protocol = "gemini".into();

    assert!(service.save_provider(bad).is_err());
}

fn futures_block_on<F: std::future::Future>(future: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(future)
}
