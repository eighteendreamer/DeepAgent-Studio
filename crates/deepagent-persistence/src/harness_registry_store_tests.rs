//! Harness provider/model store behaviour (plan §3, §8 — 第 1 步 CRUD + 激活).
//!
//! Exercises `Database` methods on the harness registry tables. The canvas
//! registry is not involved.

use crate::harness_registry::{HarnessModel, HarnessProvider};
use crate::Database;

fn provider(id: &str, protocol: &str) -> HarnessProvider {
    HarnessProvider {
        id: id.into(),
        channel: format!("{id}-channel"),
        logo: String::new(),
        website_url: String::new(),
        model_url: format!("https://{id}.test/v1"),
        protocol: protocol.into(),
        api_key_ref: format!("{id}_api_key"),
        balance_kind: "auto".into(),
        enabled: true,
        revision: 0,
    }
}

#[test]
fn chat_selection_round_trips_without_affecting_model_activation() {
    let db = Database::open_in_memory().unwrap();
    assert_eq!(db.harness_chat_selection().unwrap(), None);
    db.set_harness_chat_selection("alpha", "m-one").unwrap();
    assert_eq!(
        db.harness_chat_selection().unwrap(),
        Some(("alpha".into(), "m-one".into()))
    );
    db.set_harness_chat_selection("beta", "m-two").unwrap();
    assert_eq!(
        db.harness_chat_selection().unwrap(),
        Some(("beta".into(), "m-two".into()))
    );
}

fn model(provider_id: &str, name: &str) -> HarnessModel {
    HarnessModel {
        provider_id: provider_id.into(),
        model_name: name.into(),
        display_name: name.into(),
        protocol: None,
        context_window: 128_000,
        output_limit: 8_192,
        capabilities: "{}".into(),
        active: false,
    }
}

#[test]
fn saving_a_provider_round_trips_and_bumps_its_revision() {
    let db = Database::open_in_memory().unwrap();

    db.save_harness_provider(&provider("alpha", "responses"))
        .unwrap();
    db.save_harness_provider(&provider("alpha", "chat_completions"))
        .unwrap();

    let listed = db.list_harness_providers().unwrap();
    let saved = listed
        .iter()
        .find(|provider| provider.id == "alpha")
        .unwrap();
    assert_eq!(saved.protocol, "chat_completions");
    assert_eq!(saved.revision, 2);
}

#[test]
fn unknown_protocol_is_rejected_before_storage() {
    let db = Database::open_in_memory().unwrap();

    let error = db
        .save_harness_provider(&provider("beta", "gemini"))
        .expect_err("gemini is not a harness protocol");

    assert!(error.to_string().contains("gemini"), "{error}");
    assert_eq!(db.list_harness_providers().unwrap().len(), 2);
}

#[test]
fn saved_models_start_inactive_and_only_activated_ones_are_listed() {
    let db = Database::open_in_memory().unwrap();
    db.save_harness_provider(&provider("alpha", "responses"))
        .unwrap();
    db.save_harness_models("alpha", &[model("alpha", "m-one"), model("alpha", "m-two")])
        .unwrap();

    assert!(db.active_harness_models().unwrap().is_empty());

    assert!(db.set_harness_model_active("alpha", "m-two", true).unwrap());

    let active = db.active_harness_models().unwrap();
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].0.id, "alpha");
    assert_eq!(active[0].1.model_name, "m-two");
}

#[test]
fn refetching_models_keeps_existing_activation() {
    let db = Database::open_in_memory().unwrap();
    db.save_harness_provider(&provider("alpha", "responses"))
        .unwrap();
    db.save_harness_models("alpha", &[model("alpha", "m-one")])
        .unwrap();
    db.set_harness_model_active("alpha", "m-one", true).unwrap();

    db.save_harness_models("alpha", &[model("alpha", "m-one"), model("alpha", "m-new")])
        .unwrap();

    let active: Vec<String> = db
        .active_harness_models()
        .unwrap()
        .into_iter()
        .map(|(_, model)| model.model_name)
        .collect();
    assert_eq!(active, vec!["m-one".to_string()]);
}

#[test]
fn disabled_provider_hides_its_active_models() {
    let db = Database::open_in_memory().unwrap();
    db.save_harness_provider(&provider("alpha", "responses"))
        .unwrap();
    db.save_harness_models("alpha", &[model("alpha", "m-one")])
        .unwrap();
    db.set_harness_model_active("alpha", "m-one", true).unwrap();

    let mut disabled = provider("alpha", "responses");
    disabled.enabled = false;
    db.save_harness_provider(&disabled).unwrap();

    assert!(db.active_harness_models().unwrap().is_empty());
}

#[test]
fn listing_one_providers_models_includes_inactive_ones_and_no_other_provider() {
    let db = Database::open_in_memory().unwrap();
    db.save_harness_provider(&provider("alpha", "responses"))
        .unwrap();
    db.save_harness_provider(&provider("beta", "responses"))
        .unwrap();
    db.save_harness_models("alpha", &[model("alpha", "a-one"), model("alpha", "a-two")])
        .unwrap();
    db.save_harness_models("beta", &[model("beta", "b-one")])
        .unwrap();
    db.set_harness_model_active("alpha", "a-two", true).unwrap();

    let listed = db.list_harness_models("alpha").unwrap();

    let names: Vec<(&str, bool)> = listed
        .iter()
        .map(|item| (item.model_name.as_str(), item.active))
        .collect();
    assert_eq!(names, vec![("a-one", false), ("a-two", true)]);
}

#[test]
fn removing_a_provider_removes_its_models() {
    let db = Database::open_in_memory().unwrap();
    db.save_harness_provider(&provider("alpha", "responses"))
        .unwrap();
    db.save_harness_models("alpha", &[model("alpha", "m-one")])
        .unwrap();
    db.set_harness_model_active("alpha", "m-one", true).unwrap();

    assert!(db.remove_harness_provider("alpha").unwrap());

    assert_eq!(db.list_harness_providers().unwrap().len(), 2);
    assert!(db.active_harness_models().unwrap().is_empty());
}
