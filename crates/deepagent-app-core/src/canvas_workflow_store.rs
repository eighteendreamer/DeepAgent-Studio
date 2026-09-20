//! Canvas workflow persistence.
//!
//! Graph JSON is opaque here on purpose: the canvas UI owns node shapes, while
//! the backend owns *where* it is stored, so a workflow survives a rebuild the
//! same way provider config does. Nothing canvas-related belongs in
//! `localStorage` any more.

use std::sync::Arc;

use deepagent_core::clock::{Clock, SystemClock};
use deepagent_core::error::{CoreError, Result};
use deepagent_persistence::document_store::DocumentStore;
use deepagent_persistence::Database;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Document collection for canvas workflow graphs.
pub const CANVAS_WORKFLOW_COLLECTION: &str = "canvas_workflows";

/// Largest accepted graph, in bytes of serialized JSON.
const MAX_GRAPH_BYTES: usize = 16 * 1024 * 1024;

/// The two canvas modes with their own current-workflow slot.
pub const CANVAS_MODES: [&str; 2] = ["creative", "professional"];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CanvasWorkflowDoc {
    /// Stable id; `current:<mode>:<workspace>` for the live canvas slot.
    pub id: String,
    pub name: String,
    pub mode: String,
    #[serde(default)]
    pub workspace_id: Option<String>,
    pub graph: Value,
    pub updated_at: i64,
    /// True for the live canvas slot rather than a saved workflow.
    #[serde(default)]
    pub current: bool,
}

/// Library listing entry (graph excluded to keep the payload small).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CanvasWorkflowMeta {
    pub id: String,
    pub name: String,
    pub mode: String,
    pub updated_at: i64,
}

/// The document-store id of a mode's live canvas.
pub fn current_workflow_id(mode: &str, workspace_id: Option<&str>) -> Result<String> {
    let mode = normalize_mode(mode)?;
    let workspace = match workspace_id {
        Some(value) if !value.trim().is_empty() => value.trim().to_string(),
        _ => "_default".to_string(),
    };
    validate_token(&workspace, "workspace id")?;
    Ok(format!("current:{mode}:{workspace}"))
}

fn normalize_mode(mode: &str) -> Result<String> {
    let trimmed = mode.trim().to_ascii_lowercase();
    if CANVAS_MODES.contains(&trimmed.as_str()) {
        Ok(trimmed)
    } else {
        Err(CoreError::invalid(format!(
            "unknown canvas mode `{mode}` (expected creative or professional)"
        )))
    }
}

/// Ids end up in document keys, so keep them to a boring ASCII alphabet.
fn validate_token(value: &str, location: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 120
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b':'))
    {
        return Err(CoreError::invalid(format!(
            "{location} must be 1..=120 ASCII letters, digits, underscore, hyphen or colon"
        )));
    }
    Ok(())
}

fn check_graph_size(graph: &Value) -> Result<()> {
    let bytes = serde_json::to_string(graph)?.len();
    if bytes > MAX_GRAPH_BYTES {
        return Err(CoreError::invalid(format!(
            "canvas graph too large: {bytes} bytes > {MAX_GRAPH_BYTES} limit"
        )));
    }
    Ok(())
}

fn decode(doc: &deepagent_persistence::document_store::Document) -> Result<CanvasWorkflowDoc> {
    serde_json::from_str(&doc.body)
        .map_err(|error| CoreError::invalid(format!("decode canvas workflow: {error}")))
}

/// Persistence service for canvas graphs.
pub struct CanvasWorkflowStore {
    db: Arc<Database>,
}

impl CanvasWorkflowStore {
    pub fn new(db: Arc<Database>) -> Self {
        Self { db }
    }

    fn now(&self) -> i64 {
        SystemClock.now().as_millis()
    }

    /// Persist the live canvas for one mode (and optionally one workspace).
    pub fn write_current(
        &self,
        mode: &str,
        workspace_id: Option<&str>,
        graph: &Value,
    ) -> Result<CanvasWorkflowMeta> {
        check_graph_size(graph)?;
        let id = current_workflow_id(mode, workspace_id)?;
        let meta = CanvasWorkflowMeta {
            id: id.clone(),
            name: format!("current-{mode}"),
            mode: mode.trim().to_ascii_lowercase(),
            updated_at: self.now(),
        };
        let document = CanvasWorkflowDoc {
            id: meta.id.clone(),
            name: meta.name.clone(),
            mode: meta.mode.clone(),
            workspace_id: workspace_id.map(str::to_string),
            graph: graph.clone(),
            updated_at: meta.updated_at,
            current: true,
        };
        self.put(&document)?;
        Ok(meta)
    }

    /// Read the live canvas graph, if any.
    pub fn read_current(&self, mode: &str, workspace_id: Option<&str>) -> Result<Option<Value>> {
        let id = current_workflow_id(mode, workspace_id)?;
        let doc = DocumentStore::new(&self.db).get(CANVAS_WORKFLOW_COLLECTION, &id)?;
        let Some(doc) = doc else { return Ok(None) };
        Ok(Some(decode(&doc)?.graph))
    }

    /// Save or overwrite a named workflow in the library.
    pub fn save_workflow(
        &self,
        id: Option<&str>,
        name: &str,
        mode: &str,
        workspace_id: Option<&str>,
        graph: &Value,
    ) -> Result<CanvasWorkflowMeta> {
        check_graph_size(graph)?;
        let mode = normalize_mode(mode)?;
        let name = name.trim();
        if name.is_empty() {
            return Err(CoreError::invalid("canvas workflow name is required"));
        }
        let id = match id.map(str::trim) {
            Some(existing) if !existing.is_empty() => {
                validate_token(existing, "workflow id")?;
                existing.to_string()
            }
            _ => format!("wf-{}", uuid::Uuid::new_v4()),
        };
        let meta = CanvasWorkflowMeta {
            id: id.clone(),
            name: name.to_string(),
            mode,
            updated_at: self.now(),
        };
        self.put(&CanvasWorkflowDoc {
            id,
            name: meta.name.clone(),
            mode: meta.mode.clone(),
            workspace_id: workspace_id.map(str::to_string),
            graph: graph.clone(),
            updated_at: meta.updated_at,
            current: false,
        })?;
        Ok(meta)
    }

    /// Library entries, newest first, excluding the live canvas slots.
    pub fn list_workflows(&self) -> Result<Vec<CanvasWorkflowMeta>> {
        let mut items: Vec<CanvasWorkflowMeta> = DocumentStore::new(&self.db)
            .list(CANVAS_WORKFLOW_COLLECTION)?
            .iter()
            .filter_map(|doc| decode(doc).ok())
            .filter(|document| !document.current)
            .map(|document| CanvasWorkflowMeta {
                id: document.id,
                name: document.name,
                mode: document.mode,
                updated_at: document.updated_at,
            })
            .collect();
        items.sort_by_key(|item| std::cmp::Reverse(item.updated_at));
        Ok(items)
    }

    pub fn load_workflow(&self, id: &str) -> Result<Option<CanvasWorkflowDoc>> {
        validate_token(id, "workflow id")?;
        let doc = DocumentStore::new(&self.db).get(CANVAS_WORKFLOW_COLLECTION, id)?;
        let Some(doc) = doc else { return Ok(None) };
        Ok(Some(decode(&doc)?))
    }

    pub fn delete_workflow(&self, id: &str) -> Result<bool> {
        validate_token(id, "workflow id")?;
        DocumentStore::new(&self.db).delete(CANVAS_WORKFLOW_COLLECTION, id)
    }

    fn put(&self, document: &CanvasWorkflowDoc) -> Result<()> {
        let body = serde_json::to_string(document)?;
        DocumentStore::new(&self.db).put(
            CANVAS_WORKFLOW_COLLECTION,
            &document.id,
            &body,
            None,
            SystemClock.now(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn store() -> CanvasWorkflowStore {
        CanvasWorkflowStore::new(Arc::new(Database::open_in_memory().expect("db")))
    }

    #[test]
    fn current_canvas_round_trips_per_mode_and_workspace() {
        let store = store();
        assert!(store
            .read_current("creative", None)
            .expect("read")
            .is_none());
        store
            .write_current(
                "creative",
                None,
                &json!({ "nodes": [{ "id": "a" }], "edges": [] }),
            )
            .expect("write creative");
        store
            .write_current("professional", None, &json!({ "nodes": [], "edges": [] }))
            .expect("write professional");
        let creative = store
            .read_current("creative", None)
            .expect("read")
            .expect("present");
        assert_eq!(creative["nodes"].as_array().expect("nodes").len(), 1);
        assert!(store
            .read_current("creative", Some("other-workspace"))
            .expect("read other")
            .is_none());

        store
            .write_current("creative", None, &json!({ "nodes": [], "edges": [] }))
            .expect("overwrite");
        assert_eq!(
            store
                .read_current("creative", None)
                .expect("read")
                .expect("present")["nodes"]
                .as_array()
                .expect("nodes")
                .len(),
            0
        );
    }

    #[test]
    fn named_workflows_list_load_and_delete() {
        let store = store();
        let saved = store
            .save_workflow(
                None,
                "海报流水线",
                "creative",
                None,
                &json!({ "nodes": [{ "id": "n1" }] }),
            )
            .expect("save");
        assert!(saved.id.starts_with("wf-"));
        let listed = store.list_workflows().expect("list");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].name, "海报流水线");

        let loaded = store
            .load_workflow(&saved.id)
            .expect("load")
            .expect("present");
        assert_eq!(loaded.mode, "creative");
        assert!(!loaded.current);

        store
            .save_workflow(
                Some(&saved.id),
                "改名后的流水线",
                "creative",
                None,
                &json!({ "nodes": [] }),
            )
            .expect("update");
        assert_eq!(store.list_workflows().expect("list").len(), 1);
        assert_eq!(
            store.list_workflows().expect("list")[0].name,
            "改名后的流水线"
        );
        assert!(store.delete_workflow(&saved.id).expect("delete"));
        assert!(!store.delete_workflow(&saved.id).expect("second delete"));
        assert!(store.list_workflows().expect("list").is_empty());
    }

    #[test]
    fn library_list_excludes_live_canvas_slots() {
        let store = store();
        store
            .write_current("creative", None, &json!({ "nodes": [] }))
            .expect("current");
        assert!(store.list_workflows().expect("list").is_empty());
    }

    #[test]
    fn rejects_unknown_mode_bad_id_and_oversized_graph() {
        let store = store();
        let mode_error = store
            .read_current("sideways", None)
            .expect_err("unknown mode");
        assert!(mode_error.to_string().contains("sideways"));
        let oversized = json!({ "blob": "x".repeat(MAX_GRAPH_BYTES + 10) });
        let error = store
            .write_current("creative", None, &oversized)
            .expect_err("oversized graph");
        assert!(error.to_string().contains("limit"));
        assert!(store
            .save_workflow(
                Some("bad id with spaces"),
                "n",
                "creative",
                None,
                &json!({})
            )
            .is_err());
    }
}
