//! ModelResolver: one explicitly selected, activated harness model resolves to a
//! credential-free route. No fallback to any other model (plan §0.1-7, §8).

use deepagent_app_core::harness_model_resolver::{resolve_active_model, wire_mode_for_protocol};
use deepagent_models::WireMode;
use deepagent_persistence::harness_registry::{HarnessModel, HarnessProvider};
use deepagent_persistence::Database;

fn provider(id: &str, protocol: &str, enabled: bool) -> HarnessProvider {
    HarnessProvider {
        id: id.into(),
        channel: format!("{id} channel"),
        logo: String::new(),
        model_url: format!("https://{id}.test/v1"),
        protocol: protocol.into(),
        api_key_ref: format!("secret-{id}"),
        balance_kind: "auto".into(),
        enabled,
        revision: 0,
    }
}

fn model(provider_id: &str, name: &str, protocol: Option<&str>) -> HarnessModel {
    HarnessModel {
        provider_id: provider_id.into(),
        model_name: name.into(),
        display_name: name.into(),
        protocol: protocol.map(str::to_string),
        context_window: 1_000_000,
        output_limit: 131_072,
        capabilities: "{}".into(),
        active: false,
    }
}

/// One provider with the given models; each `(name, active)` pair is stored as is.
fn seeded(protocol: &str, enabled: bool, models: &[(&str, bool, Option<&str>)]) -> Database {
    let db = Database::open_in_memory().unwrap();
    db.save_harness_provider(&provider("alpha", protocol, enabled))
        .unwrap();
    let rows: Vec<HarnessModel> = models
        .iter()
        .map(|(name, _, override_protocol)| model("alpha", name, *override_protocol))
        .collect();
    db.save_harness_models("alpha", &rows).unwrap();
    for (name, active, _) in models {
        if *active {
            assert!(db.set_harness_model_active("alpha", name, true).unwrap());
        }
    }
    db
}

#[test]
fn active_model_resolves_to_provider_route_with_its_capacities() {
    let db = seeded("chat_completions", true, &[("m-one", true, None)]);
    let route = resolve_active_model(&db, "alpha", "m-one").unwrap();

    assert_eq!(route.provider_id, "alpha");
    assert_eq!(route.model_name, "m-one");
    assert_eq!(route.base_url, "https://alpha.test/v1");
    assert_eq!(route.wire_mode, WireMode::ChatCompletions);
    assert_eq!(route.context_window, 1_000_000);
    assert_eq!(route.output_limit, 131_072);
    assert_eq!(route.capability.context_window, 1_000_000);
    assert_eq!(route.capability.max_output_tokens, 131_072);
}

#[test]
fn model_protocol_overrides_the_provider_protocol() {
    let db = seeded(
        "responses",
        true,
        &[("m-one", true, Some("chat_completions"))],
    );

    let route = resolve_active_model(&db, "alpha", "m-one").unwrap();

    assert_eq!(route.wire_mode, WireMode::ChatCompletions);
}

#[test]
fn model_without_override_inherits_the_provider_protocol() {
    let db = seeded("responses", true, &[("m-one", true, None)]);

    let route = resolve_active_model(&db, "alpha", "m-one").unwrap();

    assert_eq!(route.wire_mode, WireMode::Responses);
}

#[test]
fn inactive_model_is_refused_even_when_another_model_is_active() {
    let db = seeded(
        "chat_completions",
        true,
        &[("active-one", true, None), ("idle-one", false, None)],
    );

    let error = resolve_active_model(&db, "alpha", "idle-one").unwrap_err();

    assert!(
        error.to_string().contains("not activated"),
        "unexpected error: {error}"
    );
}

#[test]
fn unknown_model_is_refused() {
    let db = seeded("chat_completions", true, &[("m-one", true, None)]);

    let error = resolve_active_model(&db, "alpha", "ghost").unwrap_err();

    assert!(error.to_string().contains("unknown model"), "{error}");
}

#[test]
fn disabled_provider_is_refused_even_with_an_active_model() {
    let db = seeded("chat_completions", false, &[("m-one", true, None)]);

    let error = resolve_active_model(&db, "alpha", "m-one").unwrap_err();

    assert!(error.to_string().contains("is disabled"), "{error}");
}

#[test]
fn unknown_provider_is_refused() {
    let db = seeded("chat_completions", true, &[("m-one", true, None)]);

    let error = resolve_active_model(&db, "beta", "m-one").unwrap_err();

    assert!(error.to_string().contains("provider `beta`"), "{error}");
}

#[test]
fn messages_protocol_is_refused_until_a_codec_exists() {
    let db = seeded("messages", true, &[("m-one", true, None)]);

    let error = resolve_active_model(&db, "alpha", "m-one").unwrap_err();

    assert!(error.to_string().contains("no runtime codec"), "{error}");
    assert!(wire_mode_for_protocol("messages").is_err());
}

#[test]
fn unknown_protocol_name_is_refused() {
    assert!(wire_mode_for_protocol("gemini").is_err());
    assert_eq!(
        wire_mode_for_protocol("responses").unwrap(),
        WireMode::Responses
    );
}

#[test]
fn registry_columns_own_identity_and_capacities_while_optional_metadata_is_preserved() {
    let db = seeded("responses", true, &[("m-one", true, None)]);
    let mut stored = model("alpha", "m-one", None);
    stored.capabilities = serde_json::json!({
        "model_id":"stale-id", "context_window":3, "max_output_tokens":1,
        "input_modalities":["text"], "supports_tools":false,
        "effort":{"supported_levels":["low"], "default_level":"low"}
    })
    .to_string();
    db.save_harness_models("alpha", &[stored]).unwrap();
    let route = resolve_active_model(&db, "alpha", "m-one").unwrap();
    assert_eq!(route.capability.model_id, "m-one");
    assert_eq!(route.capability.context_window, 1_000_000);
    assert_eq!(route.capability.max_output_tokens, 131_072);
    assert_eq!(route.capability.input_modalities, Some(vec!["text".into()]));
    assert!(!route.capability.supports_tools);
    assert_eq!(
        route.capability.effort.unwrap().default_level.as_deref(),
        Some("low")
    );
}

#[test]
fn malformed_capabilities_and_nonpositive_limits_fail_without_echoing_stored_values() {
    let db = seeded("responses", true, &[("m-one", true, None)]);
    for bad in [
        "[\"private-fixture\"]",
        "{",
        r#"{"supports_tools":"private-fixture"}"#,
    ] {
        let mut stored = model("alpha", "m-one", None);
        stored.capabilities = bad.into();
        db.save_harness_models("alpha", &[stored]).unwrap();
        let error = resolve_active_model(&db, "alpha", "m-one")
            .unwrap_err()
            .to_string();
        assert!(error.contains("capabilit"));
        assert!(!error.contains("private-fixture"));
    }
    for (context, output) in [(0, 100), (100, 0), (-1, 100), (100, -1)] {
        let mut stored = model("alpha", "m-one", None);
        stored.context_window = context;
        stored.output_limit = output;
        db.save_harness_models("alpha", &[stored]).unwrap();
        assert!(resolve_active_model(&db, "alpha", "m-one").is_err());
    }
}
