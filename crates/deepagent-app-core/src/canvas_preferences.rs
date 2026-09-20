//! Canvas preferences: small per-workspace JSON documents in the kernel database.
//!
//! Snippets, the creative library and the workspace output directories used to
//! live in `localStorage`, which meant they vanished with the webview profile
//! and could not be shared with the CLI or a second window. They are not
//! secrets, but they are canvas state, so they belong in the same database as
//! provider config and workflow graphs.

use std::sync::Arc;

use deepagent_core::clock::{Clock, SystemClock};
use deepagent_core::error::{CoreError, Result};
use deepagent_persistence::document_store::DocumentStore;
use deepagent_persistence::Database;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Document collection for canvas preferences.
pub const CANVAS_PREFERENCES_COLLECTION: &str = "canvas_preferences";

/// The only preference documents the canvas may store. An allowlist keeps the
/// collection from becoming a second unmanaged key-value dump.
pub const CANVAS_PREFERENCE_KEYS: [&str; 3] =
    ["creative-library", "workflow-snippets", "workspace-dirs"];

/// Largest accepted preference payload, in bytes of serialized JSON.
const MAX_PREFERENCE_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CanvasPreferenceDoc {
    /// One of [`CANVAS_PREFERENCE_KEYS`].
    pub key: String,
    pub value: Value,
    pub updated_at: i64,
}

/// Validate a preference key against the allowlist.
pub fn check_preference_key(key: &str) -> Result<&'static str> {
    let trimmed = key.trim();
    CANVAS_PREFERENCE_KEYS
        .iter()
        .find(|allowed| **allowed == trimmed)
        .copied()
        .ok_or_else(|| {
            CoreError::invalid(format!(
                "unknown canvas preference `{trimmed}` (allowed: {})",
                CANVAS_PREFERENCE_KEYS.join(", ")
            ))
        })
}

/// Persistence for canvas preference documents.
pub struct CanvasPreferencesStore {
    db: Arc<Database>,
}

impl CanvasPreferencesStore {
    pub fn new(db: Arc<Database>) -> Self {
        Self { db }
    }

    /// Read one preference document, or `None` when never written.
    pub fn read(&self, key: &str) -> Result<Option<Value>> {
        let key = check_preference_key(key)?;
        let doc = DocumentStore::new(&self.db).get(CANVAS_PREFERENCES_COLLECTION, key)?;
        let Some(doc) = doc else { return Ok(None) };
        let decoded: CanvasPreferenceDoc = serde_json::from_str(&doc.body).map_err(|error| {
            CoreError::invalid(format!("decode canvas preference `{key}`: {error}"))
        })?;
        Ok(Some(decoded.value))
    }

    /// Replace one preference document and report its new timestamp.
    pub fn write(&self, key: &str, value: &Value) -> Result<i64> {
        let key = check_preference_key(key)?;
        let now = SystemClock.now();
        let document = CanvasPreferenceDoc {
            key: key.to_string(),
            value: value.clone(),
            updated_at: now.as_millis(),
        };
        let body = serde_json::to_string(&document).map_err(|error| {
            CoreError::invalid(format!("encode canvas preference `{key}`: {error}"))
        })?;
        if body.len() > MAX_PREFERENCE_BYTES {
            return Err(CoreError::invalid(format!(
                "canvas preference `{key}` is {} bytes > {MAX_PREFERENCE_BYTES} limit",
                body.len()
            )));
        }
        DocumentStore::new(&self.db).put(CANVAS_PREFERENCES_COLLECTION, key, &body, None, now)?;
        Ok(now.as_millis())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn store() -> CanvasPreferencesStore {
        CanvasPreferencesStore::new(Arc::new(Database::open_in_memory().expect("in-memory db")))
    }

    #[test]
    fn each_key_round_trips_independently() {
        let store = store();
        assert!(store.read("creative-library").expect("read").is_none());
        store
            .write(
                "creative-library",
                &json!([{"id": "a1", "imageUrl": "artifact://art_1"}]),
            )
            .expect("write library");
        store
            .write("workflow-snippets", &json!({"snippets": [], "version": 1}))
            .expect("write snippets");
        let library = store
            .read("creative-library")
            .expect("read")
            .expect("present");
        assert_eq!(library.as_array().expect("array").len(), 1);
        assert_eq!(
            store
                .read("workflow-snippets")
                .expect("read snippets")
                .expect("present")["version"],
            json!(1)
        );
    }

    #[test]
    fn later_writes_replace_the_document() {
        let store = store();
        store
            .write("workspace-dirs", &json!({"imageDir": "D:/out"}))
            .expect("first write");
        let second = store
            .write(
                "workspace-dirs",
                &json!({"imageDir": "E:/out", "videoDir": ""}),
            )
            .expect("second write");
        let value = store
            .read("workspace-dirs")
            .expect("read")
            .expect("present");
        assert_eq!(value["imageDir"], json!("E:/out"));
        assert!(second > 0);
    }

    #[test]
    fn rejects_unknown_keys() {
        let store = store();
        let error = store.read("secrets").expect_err("unknown read key");
        assert!(error.to_string().contains("unknown canvas preference"));
        assert!(store.write("api-keys", &json!({"sk": "x"})).is_err());
    }

    #[test]
    fn rejects_oversized_documents() {
        let store = store();
        let error = store
            .write(
                "workflow-snippets",
                &json!({"blob": "y".repeat(MAX_PREFERENCE_BYTES + 8)}),
            )
            .expect_err("oversized document");
        assert!(error.to_string().contains("limit"));
    }
}
