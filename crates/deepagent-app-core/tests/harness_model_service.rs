//! Harness model service: provider + model management for the main settings
//! "模型" page (plan §3, §8). Canvas providers are not involved.

use std::sync::Arc;

use deepagent_app_core::harness_model_service::{
    HarnessModelInput, HarnessModelService, HarnessModelView, HarnessProviderInput,
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

#[tokio::test]
async fn h025_messages_discovery_uses_versioned_base_without_resource_path_in_settings() {
    struct EndpointTransport(String);
    #[async_trait::async_trait]
    impl HttpTransport for EndpointTransport {
        async fn stream(
            &self,
            _: deepagent_models::TransportRequest,
            _: &mut dyn deepagent_models::transport::EventSink,
        ) -> deepagent_core::Result<()> {
            panic!("discovery must not invoke inference");
        }

        async fn get_json(&self, url: &str, key: &str) -> deepagent_core::Result<String> {
            assert_eq!(url, self.0);
            assert_eq!(key, "synthetic");
            Ok(r#"{"data":[{"id":"fixture"}]}"#.into())
        }

        async fn get_json_authenticated(
            &self,
            url: &str,
            key: &str,
            auth: deepagent_models::TransportAuth,
        ) -> deepagent_core::Result<String> {
            assert_eq!(auth, deepagent_models::TransportAuth::Messages);
            self.get_json(url, key).await
        }
    }
    for (base, expected) in [
        (
            "https://provider.test/anthropic",
            "https://provider.test/anthropic/v1/models",
        ),
        (
            "https://provider.test/anthropic/v1/",
            "https://provider.test/anthropic/v1/models",
        ),
        (
            "https://api.deepseek.com/anthropic",
            "https://api.deepseek.com/models",
        ),
        (
            "https://api.deepseek.com/anthropic/v1/",
            "https://api.deepseek.com/models",
        ),
    ] {
        let service = HarnessModelService::new(
            Arc::new(Database::open_in_memory().unwrap()),
            Arc::new(MemorySecretStore::new()),
            Arc::new(EndpointTransport(expected.into())),
        );
        let mut provider = input("messages", Some("synthetic"));
        provider.protocol = "messages".into();
        provider.model_url = base.into();
        service.save_provider(provider).unwrap();
        let models = service.fetch_models("messages").await.unwrap();
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].model_name, "fixture");
        assert_eq!(
            service
                .list_providers()
                .unwrap()
                .into_iter()
                .find(|provider| provider.id == "messages")
                .unwrap()
                .model_url,
            base
        );
    }
}

fn input(id: &str, api_key: Option<&str>) -> HarnessProviderInput {
    HarnessProviderInput {
        id: Some(id.into()),
        channel: format!("{id} channel"),
        logo: String::new(),
        website_url: None,
        model_url: "https://provider.test/v1".into(),
        protocol: "chat_completions".into(),
        balance_kind: "auto".into(),
        enabled: true,
        api_key: api_key.map(str::to_string),
    }
}

#[test]
fn website_and_uploaded_logo_round_trip_without_exposing_credentials() {
    let (service, _) = service_with_models_response("{}");
    let mut provider = input("custom", Some("synthetic-provider-key"));
    provider.website_url = Some("https://provider.test/".into());
    provider.logo = "data:image/png;base64,iVBORw0KGgo=".into();
    let saved = service.save_provider(provider).unwrap();
    assert_eq!(saved.website_url, "https://provider.test/");
    assert!(saved.logo.starts_with("data:image/png;base64,"));
    assert!(saved.has_api_key);
    assert!(!serde_json::to_string(&saved)
        .unwrap()
        .contains("synthetic-provider-key"));
    let saved = service.save_provider(input("custom", None)).unwrap();
    assert_eq!(
        saved.website_url, "https://provider.test/",
        "older clients preserve the website"
    );
}

#[test]
fn invalid_branding_is_rejected_before_writing_a_secret() {
    let (service, secrets) = service_with_models_response("{}");
    for website in [
        "javascript:alert(1)",
        "file:///test",
        "https://user:secret@provider.test/",
    ] {
        let mut provider = input("bad", Some("synthetic"));
        provider.website_url = Some(website.into());
        assert!(service.save_provider(provider).is_err());
    }
    for logo in [
        "https://remote.test/logo.svg".to_string(),
        "data:image/svg+xml;base64,PHN2Zy8+".into(),
        format!("data:image/png;base64,{}", "A".repeat(1_398_108)),
    ] {
        let mut provider = input("bad", Some("synthetic"));
        provider.logo = logo;
        assert!(service.save_provider(provider).is_err());
    }
    assert!(secrets.get("harness_provider:bad").unwrap().is_none());
}

#[tokio::test]
async fn balance_none_is_a_real_noop_and_does_not_require_a_secret() {
    let (service, secrets, db) = legacy_provider_service();
    db.save_harness_provider(&HarnessProvider {
        id: "no-balance".into(),
        channel: "No Balance".into(),
        logo: String::new(),
        website_url: String::new(),
        model_url: "https://provider.example/v1".into(),
        protocol: "responses".into(),
        api_key_ref: "harness_provider:no-balance".into(),
        balance_kind: "none".into(),
        enabled: true,
        revision: 0,
    })
    .unwrap();
    let result = service.fetch_balance("no-balance").await.unwrap();
    assert_eq!(result.provider_id, "no-balance");
    assert_eq!(result.balance_kind, "none");
    assert_eq!(result.amount.amount, None);
    assert!(!result.amount.unlimited);
    assert!(secrets
        .get("harness_provider:no-balance")
        .unwrap()
        .is_none());
}

// V24 imports the existing key by reference rather than copying its plaintext.
fn legacy_provider_service() -> (HarnessModelService, Arc<MemorySecretStore>, Arc<Database>) {
    let db = Arc::new(Database::open_in_memory().unwrap());
    db.save_harness_provider(&HarnessProvider {
        id: "deepseek".into(),
        channel: "DeepSeek".into(),
        logo: String::new(),
        website_url: String::new(),
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
            db.list_harness_providers()
                .unwrap()
                .into_iter()
                .find(|provider| provider.id == "deepseek")
                .unwrap()
                .api_key_ref,
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
        db.list_harness_providers()
            .unwrap()
            .into_iter()
            .find(|provider| provider.id == "deepseek")
            .unwrap()
            .api_key_ref,
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
    assert!(service
        .list_providers()
        .unwrap()
        .iter()
        .all(|provider| provider.id != "deepseek"));
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
fn messages_discovery_uses_128k_when_the_provider_omits_the_ceiling() {
    let (service, _) = service_with_models_response(
        r#"{"data":[{"id":"claude-opus-5-5"},{"id":"declared","max_output_tokens":131072}]}"#,
    );
    let mut provider = input("third-party", Some("synthetic"));
    provider.protocol = "messages".into();
    service.save_provider(provider).unwrap();

    let models = futures_block_on(service.fetch_models("third-party")).unwrap();
    let ceiling = |name| {
        models
            .iter()
            .find(|model| model.model_name == name)
            .unwrap()
            .output_limit
    };
    assert_eq!(ceiling("claude-opus-5-5"), 128_000);
    assert_eq!(ceiling("declared"), 131_072);
}

#[test]
fn fetching_models_preserves_saved_model_configuration() {
    let (service, _, db) = legacy_provider_service();
    let saved = deepagent_persistence::harness_registry::HarnessModel {
        provider_id: "deepseek".into(),
        model_name: "deepseek-flash".into(),
        display_name: "Configured model".into(),
        protocol: Some("messages".into()),
        context_window: 64_000,
        output_limit: 4_096,
        capabilities: serde_json::json!({"thinking_levels":["off","medium"],
            "thinking_disable":true,"input_modalities":["text"]})
        .to_string(),
        active: true,
    };
    db.save_harness_models("deepseek", std::slice::from_ref(&saved))
        .unwrap();
    futures_block_on(service.fetch_models("deepseek")).unwrap();
    assert_eq!(db.list_harness_models("deepseek").unwrap(), [saved]);
    assert_eq!(
        service.active_models().unwrap()[0].thinking_levels,
        ["off", "medium"]
    );
}

#[test]
fn fetching_new_models_retains_declared_capacity_and_effort_without_name_guessing() {
    let (service, _) = service_with_models_response(
        r#"{"data":[{
        "id":"unknown-model","context_window":64000,"max_output_tokens":4096,
        "input_modalities":["text"],
        "effort":{"supported_levels":["low","medium"],"default_level":"medium"}
    }]}"#,
    );
    service
        .save_provider(input("alpha", Some("test-key")))
        .unwrap();
    let fetched = futures_block_on(service.fetch_models("alpha")).unwrap();
    assert_eq!(fetched[0].context_window, 64_000);
    assert_eq!(fetched[0].output_limit, 4_096);
    assert!(!fetched[0].active);
    service
        .set_model_active("alpha", "unknown-model", true)
        .unwrap();
    let active = service.active_models().unwrap().remove(0);
    assert_eq!(active.thinking_levels, ["low", "medium"]);
}

#[test]
fn invalid_discovery_metadata_cannot_partially_replace_saved_models() {
    for invalid in [
        serde_json::json!({"id":""}),
        serde_json::json!({"id":"valid"}),
        serde_json::json!({"id":"invalid","context_window":0}),
        serde_json::json!({"id":"invalid","max_output_tokens":u64::MAX}),
        serde_json::json!({"id":"invalid","effort":{"supported_levels":["future"]}}),
    ] {
        let body = serde_json::json!({"data":[{"id":"valid"},invalid]}).to_string();
        let (service, _) = service_with_models_response(&body);
        service
            .save_provider(input("alpha", Some("test-key")))
            .unwrap();
        assert!(futures_block_on(service.fetch_models("alpha")).is_err());
        assert!(service.list_models("alpha").unwrap().is_empty());
    }
}

#[test]
fn model_editor_updates_registry_capabilities_and_preserves_activation_and_other_metadata() {
    let (service, _, db) = legacy_provider_service();
    futures_block_on(service.fetch_models("deepseek")).unwrap();
    service
        .set_model_active("deepseek", "deepseek-flash", true)
        .unwrap();
    let mut row = db.list_harness_models("deepseek").unwrap().remove(0);
    row.capabilities = r#"{"input_modalities":["text"],"supports_tools":false}"#.into();
    db.save_harness_models("deepseek", &[row]).unwrap();
    let input = HarnessModelInput {
        provider_id: "deepseek".into(),
        model_name: "deepseek-flash".into(),
        display_name: "Edited model".into(),
        protocol: Some("messages".into()),
        context_window: 64_000,
        output_limit: 4_096,
        reasoning_efforts: Some(
            serde_json::from_value(serde_json::json!({"off":null,"medium":"deep"})).unwrap(),
        ),
        default_reasoning_effort: None,
        thinking_format: None,
    };
    let saved = service.save_model(input.clone()).unwrap();
    assert!(saved.active);
    assert_eq!(saved.display_name, "Edited model");
    assert_eq!(saved.protocol.as_deref(), Some("messages"));
    assert_eq!(saved.context_window, 64_000);
    assert_eq!(saved.output_limit, 4_096);
    assert_eq!(saved.thinking_levels, ["off", "medium"]);
    assert_eq!(saved.thinking_distinct, ["off", "medium"]);
    assert!(saved.thinking_disable);
    let stored: serde_json::Value =
        serde_json::from_str(&db.list_harness_models("deepseek").unwrap()[0].capabilities).unwrap();
    assert_eq!(stored["input_modalities"], serde_json::json!(["text"]));
    assert_eq!(stored["supports_tools"], false);
    let route = deepagent_app_core::harness_model_resolver::resolve_active_model(
        &db,
        "deepseek",
        "deepseek-flash",
    )
    .unwrap();
    assert!(route
        .capability
        .resolve_reasoning_effort(Some(deepagent_models::ReasoningEffort::High))
        .is_err());
    futures_block_on(service.fetch_models("deepseek")).unwrap();
    assert_eq!(service.list_models("deepseek").unwrap()[0], saved);

    for invalid in [
        HarnessModelInput {
            context_window: 0,
            ..input.clone()
        },
        HarnessModelInput {
            output_limit: 0,
            ..input.clone()
        },
        HarnessModelInput {
            output_limit: u32::MAX as i64 + 1,
            ..input.clone()
        },
        HarnessModelInput {
            protocol: Some("unknown".into()),
            ..input.clone()
        },
    ] {
        assert!(service.save_model(invalid).is_err());
        assert_eq!(service.list_models("deepseek").unwrap()[0], saved);
    }
    assert!(service
        .save_model(HarnessModelInput {
            model_name: "missing".into(),
            ..input
        })
        .is_err());
    assert_eq!(service.list_models("deepseek").unwrap().len(), 1);
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
fn active_model_thinking_uses_the_registry_contract_before_legacy_effort() {
    let (service, _, db) = legacy_provider_service();
    let capability = serde_json::json!({
        "thinking_levels": ["off", "low", "medium", "high", "max"],
        "thinking_distinct": ["low", "high", "max"],
        "thinking_disable": true,
        "effort": {"supported_levels": ["high"]}
    });
    db.save_harness_models(
        "deepseek",
        &[deepagent_persistence::harness_registry::HarnessModel {
            provider_id: "deepseek".into(),
            model_name: "test-thinking".into(),
            display_name: "Test Thinking".into(),
            protocol: None,
            context_window: 100_000,
            output_limit: 8_192,
            capabilities: capability.to_string(),
            active: true,
        }],
    )
    .unwrap();
    for view in [
        service.active_models().unwrap().remove(0),
        service.select_model("deepseek", "test-thinking").unwrap(),
        service.selected_model().unwrap().unwrap(),
    ] {
        let json = serde_json::to_value(view).unwrap();
        assert_eq!(json["thinkingLevels"], capability["thinking_levels"]);
        assert_eq!(json["thinkingDistinct"], capability["thinking_levels"]);
        assert_eq!(json["thinkingDisable"], true);
    }
}

#[test]
fn models_without_reasoning_declarations_receive_explicit_user_controls() {
    let (service, _) = service_with_models_response("{\"data\":[{\"id\":\"unknown-model\"}]}");
    service
        .save_provider(input("alpha", Some("test-key")))
        .unwrap();
    futures_block_on(service.fetch_models("alpha")).unwrap();
    service
        .set_model_active("alpha", "unknown-model", true)
        .unwrap();
    let view = service.active_models().unwrap().remove(0);
    assert_eq!(view.thinking_levels, ["low", "medium", "high", "max"]);
    assert!(
        !view.thinking_disable,
        "missing metadata must not promise thinking can be disabled"
    );
    assert_eq!(view.reasoning_effort, None);
    assert_eq!(view.default_reasoning_effort.as_deref(), Some("high"));
}

#[test]
fn model_reasoning_input_rejects_invalid_declarations_before_any_write() {
    for efforts in [
        serde_json::json!({}),
        serde_json::Value::Null,
        serde_json::json!(true),
        serde_json::json!({"off":null}),
        serde_json::json!({"low":null}),
        serde_json::json!({"max":""}),
        serde_json::json!({"ultra":"ultra"}),
    ] {
        let input = serde_json::json!({"providerId":"alpha","modelName":"m-a","displayName":"M",
            "protocol":null,"contextWindow":64000,"outputLimit":4096,"reasoningEfforts":efforts});
        assert!(serde_json::from_value::<HarnessModelInput>(input).is_err());
    }
}

#[test]
fn declared_default_roundtrips_and_drives_the_runtime_without_an_explicit_selection() {
    let (service, _, db) = legacy_provider_service();
    futures_block_on(service.fetch_models("deepseek")).unwrap();
    service
        .set_model_active("deepseek", "deepseek-flash", true)
        .unwrap();
    let input = |default: serde_json::Value| {
        serde_json::from_value::<HarnessModelInput>(
            serde_json::json!({"providerId":"deepseek","modelName":"deepseek-flash",
            "displayName":"Declared","protocol":null,"contextWindow":64000,"outputLimit":4096,
            "reasoningEfforts":{"off":null,"minimal":"tiny","max":"ultra"},
            "defaultReasoningEffort":default}),
        )
        .unwrap()
    };
    for (default, expected) in [("max", "max"), ("off", "none")] {
        let saved = service.save_model(input(default.into())).unwrap();
        assert_eq!(
            serde_json::to_value(&saved).unwrap()["defaultReasoningEffort"],
            default
        );
        let reloaded = service.list_models("deepseek").unwrap().remove(0);
        assert_eq!(reloaded, saved);
        let active = service.select_model("deepseek", "deepseek-flash").unwrap();
        assert_eq!(active.reasoning_effort.as_deref(), Some(expected));
        assert_eq!(active.default_reasoning_effort.as_deref(), Some(expected));
        let route = deepagent_app_core::harness_model_resolver::resolve_active_model(
            &db,
            "deepseek",
            "deepseek-flash",
        )
        .unwrap();
        let effort = route
            .capability
            .resolve_reasoning_effort(None)
            .unwrap()
            .unwrap();
        assert_eq!(effort.label(), expected);
        let persisted = db.list_harness_models("deepseek").unwrap().remove(0);
        let metadata: serde_json::Value = serde_json::from_str(&persisted.capabilities).unwrap();
        assert_eq!(metadata["reasoning_default"], default);
        assert!(service.save_model(input("high".into())).is_err());
        assert_eq!(service.list_models("deepseek").unwrap()[0], saved);
    }
}

#[test]
fn explicit_empty_thinking_levels_override_legacy_and_invalid_metadata_is_reported() {
    let (service, _, db) = legacy_provider_service();
    let mut model = deepagent_persistence::harness_registry::HarnessModel {
        provider_id: "deepseek".into(),
        model_name: "test-thinking".into(),
        display_name: "Test Thinking".into(),
        protocol: None,
        context_window: 100_000,
        output_limit: 8_192,
        capabilities: serde_json::json!({"thinking_levels": [],
            "effort": {"supported_levels": ["high"]}})
        .to_string(),
        active: true,
    };
    db.save_harness_models("deepseek", &[model.clone()])
        .unwrap();
    assert!(service.active_models().unwrap()[0]
        .thinking_levels
        .is_empty());
    for invalid in [
        "[]",
        "{\"thinking_levels\":true}",
        "{\"thinking_levels\":[\"unknown\"]}",
        "{\"thinking_levels\":[\"off\",\"off\"]}",
        "{\"thinking_levels\":[\"off\"],\"thinking_distinct\":[\"high\"]}",
    ] {
        model.capabilities = invalid.into();
        db.save_harness_models("deepseek", &[model.clone()])
            .unwrap();
        assert!(
            service.active_models().is_err(),
            "invalid metadata must not become an empty capability"
        );
        let before = db.harness_chat_selection().unwrap();
        assert!(service.select_model("deepseek", "test-thinking").is_err());
        assert_eq!(
            db.harness_chat_selection().unwrap(),
            before,
            "an invalid capability must not change the selected model"
        );
    }
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
    assert!(service
        .list_providers()
        .unwrap()
        .iter()
        .all(|provider| provider.id != "alpha"));
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
