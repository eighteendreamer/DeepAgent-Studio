//! Seed existing Responses settings on disk to exercise runtime compatibility.
use deepagent_app_core::settings::ResponsesApiSettings;
use deepagent_core::{clock::Timestamp, error::Result};
use deepagent_persistence::{document_store::DocumentStore, Database};

pub trait PersistedResponsesFixture {
    fn seed_responses_settings(&self, responses: ResponsesApiSettings) -> Result<()>;
    fn seed_responses_developer_json(&self, raw: &str) -> Result<()>;
}

impl PersistedResponsesFixture for Database {
    fn seed_responses_settings(&self, responses: ResponsesApiSettings) -> Result<()> {
        update(self, |stored| *stored = responses)
    }

    fn seed_responses_developer_json(&self, raw: &str) -> Result<()> {
        let developer = serde_json::from_str(raw)?;
        update(self, |stored| stored.developer = developer)
    }
}

fn update(db: &Database, edit: impl FnOnce(&mut ResponsesApiSettings)) -> Result<()> {
    let store = DocumentStore::new(db);
    let mut body: serde_json::Value = match store.get("settings", "app")? {
        Some(doc) => serde_json::from_str(&doc.body)?,
        None => serde_json::json!({}),
    };
    let mut responses = match body.get("responses") {
        Some(value) => serde_json::from_value(value.clone())?,
        None => ResponsesApiSettings::default(),
    };
    edit(&mut responses);
    body["responses"] = serde_json::to_value(responses)?;
    store.put("settings", "app", &body.to_string(), None, Timestamp::EPOCH)?;
    Ok(())
}
