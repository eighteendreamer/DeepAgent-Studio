use super::*;
use serde_json::json;

#[test]
fn settings_defaults_and_engine_order_are_stable() {
    let settings = SearchSettings::default();
    assert_eq!(settings.schema_version, 2);
    assert_eq!(settings.strategy, SearchStrategy::Fallback);
    assert_eq!(
        settings.engines.iter().map(|e| e.id).collect::<Vec<_>>(),
        [
            EngineId::Deepseek,
            EngineId::Anysearch,
            EngineId::Searxng,
            EngineId::Duckduckgo,
        ]
    );
    assert!(settings
        .engines
        .iter()
        .any(|e| e.id == EngineId::Deepseek && e.enabled));
    assert!(settings.engines[0].enabled);
    assert!(!settings.engines[1].enabled);
    assert!(settings.engines[2].enabled && settings.engines[3].enabled);
}

#[test]
fn old_settings_migrate_without_enabling_engines_the_user_disabled() {
    let previous = serde_json::from_value::<SearchSettings>(json!({
        "enabled":true,"provider":"duckduckgo","searxng_url":"https://search.example",
        "anysearch_enabled":true,"anysearch_base_url":"https://gateway.example",
        "anysearch_api_key_configured":false
    }))
    .unwrap();
    assert!(!previous
        .engines
        .iter()
        .any(|e| e.id == EngineId::Deepseek && e.enabled));
    assert!(!previous
        .engines
        .iter()
        .any(|e| e.id == EngineId::Searxng && e.enabled));
    assert!(previous
        .engines
        .iter()
        .any(|e| e.id == EngineId::Duckduckgo && e.enabled));
    assert!(previous
        .engines
        .iter()
        .any(|e| e.id == EngineId::Anysearch && e.enabled));
}

#[test]
fn settings_reject_wrong_version_and_duplicate_engine_ids() {
    let mut value = serde_json::to_value(SearchSettings::default()).unwrap();
    value["schema_version"] = json!(1);
    assert!(serde_json::from_value::<SearchSettings>(value).is_err());
    let mut value = serde_json::to_value(SearchSettings::default()).unwrap();
    value["engines"][0]["id"] = json!("anysearch");
    assert!(serde_json::from_value::<SearchSettings>(value).is_err());
}

#[test]
fn search_input_validates_and_converts_only_legacy_limit() {
    for (input, count) in [
        (json!({"query":"evidence"}), 10),
        (json!({"query":"evidence","count":4}), 4),
        (json!({"query":"evidence","limit":7}), 7),
        (
            json!({"query":"evidence","ori_question":null,"localization":"127.0.0.1","need_content":true}),
            10,
        ),
    ] {
        assert_eq!(SearchRequest::parse(input).unwrap().count, count);
    }
    for input in [
        json!({"query":"  "}),
        json!({"query":"x".repeat(129)}),
        json!({"query":"x","count":0}),
        json!({"query":"x","count":51}),
        json!({"query":"x","count":"3"}),
        json!({"query":"x","count":null}),
        json!({"query":"x","limit":null}),
        json!({"query":"x","need_content":"false"}),
        json!({"query":"x","count":3,"limit":4}),
        json!({"query":"x","localization":"en-US"}),
        json!({"query":"x","api_key":"secret"}),
        json!({"query":"x","model":"deepseek-flash"}),
    ] {
        assert!(SearchRequest::parse(input).is_err());
    }
}
