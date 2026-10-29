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
use sha2::{Digest, Sha256};

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
    /// Fresh write identity prevents ABA even for equal graphs/timestamps.
    /// Old documents remain readable without rewriting them on read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<String>,
}

/// Versioned live-slot read contract. Absence is an explicit create-only lease,
/// never permission to overwrite a document that appeared after this read.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CanvasWorkflowState {
    pub schema_version: u8,
    pub revision: Option<String>,
    pub graph: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CanvasWorkflowWriteReceipt {
    pub schema_version: u8,
    pub revision: String,
    pub updated_at: i64,
}

/// The IPC preserves conflict identity; callers must not parse display text or
/// adopt current_revision as permission to retry their stale graph.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CanvasWorkflowWriteError {
    pub schema_version: u8,
    pub code: &'static str,
    pub message: String,
    pub current_revision: Option<String>,
}

impl std::fmt::Display for CanvasWorkflowWriteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for CanvasWorkflowWriteError {}

impl From<CoreError> for CanvasWorkflowWriteError {
    fn from(error: CoreError) -> Self {
        Self {
            schema_version: 1,
            code: "write_failed",
            message: error.to_string(),
            current_revision: None,
        }
    }
}

fn revision(body: &str) -> String {
    format!("{:x}", Sha256::digest(body.as_bytes()))
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
        expected_revision: Option<&str>,
    ) -> Result<CanvasWorkflowWriteReceipt, CanvasWorkflowWriteError> {
        check_graph_size(graph)?;
        let id = current_workflow_id(mode, workspace_id)?;
        let store = DocumentStore::new(&self.db);
        let previous = store.get(CANVAS_WORKFLOW_COLLECTION, &id)?;
        let current_revision = previous.as_ref().map(|doc| revision(&doc.body));
        if current_revision.as_deref() != expected_revision {
            return Err(Self::conflict(&id, current_revision));
        }
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
            revision: Some(uuid::Uuid::new_v4().to_string()),
        };
        let body = serde_json::to_string(&document).map_err(CoreError::from)?;
        let now = SystemClock.now();
        let changed = match previous {
            Some(previous) => store.replace_body_if_unchanged(
                CANVAS_WORKFLOW_COLLECTION,
                &id,
                &previous.body,
                &body,
                now,
            )?,
            None => store.insert_if_absent(CANVAS_WORKFLOW_COLLECTION, &id, &body, now)?,
        };
        if !changed {
            let current = store.get(CANVAS_WORKFLOW_COLLECTION, &id)?;
            return Err(Self::conflict(&id, current.map(|doc| revision(&doc.body))));
        }
        tracing::debug!(document_id = %id, "canvas snapshot committed with revision check");
        Ok(CanvasWorkflowWriteReceipt {
            schema_version: 1,
            revision: revision(&body),
            updated_at: meta.updated_at,
        })
    }

    fn conflict(id: &str, current_revision: Option<String>) -> CanvasWorkflowWriteError {
        tracing::warn!(document_id = id, "stale canvas snapshot rejected");
        CanvasWorkflowWriteError {
            schema_version: 1,
            code: "revision_conflict",
            message:
                "Canvas changed in another window; preserve local edits and reload before saving"
                    .into(),
            current_revision,
        }
    }

    /// Read the live canvas graph, if any.
    pub fn read_current(
        &self,
        mode: &str,
        workspace_id: Option<&str>,
    ) -> Result<CanvasWorkflowState> {
        let id = current_workflow_id(mode, workspace_id)?;
        let doc = DocumentStore::new(&self.db).get(CANVAS_WORKFLOW_COLLECTION, &id)?;
        Ok(CanvasWorkflowState {
            schema_version: 1,
            revision: doc.as_ref().map(|doc| revision(&doc.body)),
            graph: doc
                .as_ref()
                .map(|doc| decode(doc).map(|doc| doc.graph))
                .transpose()?,
        })
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
            revision: None,
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
            .graph
            .is_none());
        let saved = store
            .write_current(
                "creative",
                None,
                &json!({ "nodes": [{ "id": "a" }], "edges": [] }),
                None,
            )
            .expect("write creative");
        store
            .write_current(
                "professional",
                None,
                &json!({ "nodes": [], "edges": [] }),
                None,
            )
            .expect("write professional");
        let creative = store
            .read_current("creative", None)
            .expect("read")
            .graph
            .expect("present");
        assert_eq!(creative["nodes"].as_array().expect("nodes").len(), 1);
        assert!(store
            .read_current("creative", Some("other-workspace"))
            .expect("read other")
            .graph
            .is_none());

        store
            .write_current(
                "creative",
                None,
                &json!({ "nodes": [], "edges": [] }),
                Some(&saved.revision),
            )
            .expect("overwrite");
        assert_eq!(
            store
                .read_current("creative", None)
                .expect("read")
                .graph
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
            .write_current("creative", None, &json!({ "nodes": [] }), None)
            .expect("current");
        assert!(store.list_workflows().expect("list").is_empty());
    }

    #[test]
    #[ignore = "audit H-251: named canvas workflows must enforce workspace scope on list/load/delete"]
    fn audit_named_workflows_require_workspace_scope() {
        let store = store();
        let workspace_a = store
            .save_workflow(
                None,
                "workspace-a-secret",
                "creative",
                Some("workspace-a"),
                &json!({"nodes":[{"id":"a-only"}]}),
            )
            .expect("save workspace A");
        let workspace_b = store
            .save_workflow(
                None,
                "workspace-b-secret",
                "creative",
                Some("workspace-b"),
                &json!({"nodes":[{"id":"b-only"}]}),
            )
            .expect("save workspace B");

        let listed = store.list_workflows().expect("list workflows");
        let loaded_b = store
            .load_workflow(&workspace_b.id)
            .expect("load workspace B")
            .expect("workspace B exists");
        let deleted_b = store
            .delete_workflow(&workspace_b.id)
            .expect("delete workspace B");
        let list_exposes_b = listed.iter().any(|item| item.id == workspace_b.id);
        let load_exposes_b = loaded_b.workspace_id.as_deref() == Some("workspace-b");
        eprintln!(
            "canvas workflow scope audit: listed_ids={:?}, list_exposes_b={}, loaded_workspace={:?}, load_exposes_b={}, deleted_other_workspace={}, retained_a_id={}",
            listed.iter().map(|item| item.id.as_str()).collect::<Vec<_>>(),
            list_exposes_b,
            loaded_b.workspace_id,
            load_exposes_b,
            deleted_b,
            store
                .load_workflow(&workspace_a.id)
                .expect("load workspace A")
                .is_some(),
        );
        assert!(
            !list_exposes_b && !load_exposes_b && !deleted_b,
            "workspace-scoped canvas APIs exposed or deleted another workspace's workflow"
        );
    }

    #[test]
    #[ignore = "audit H-255: named workflow ids must not collide with reserved current canvas slots"]
    fn audit_named_workflow_cannot_overwrite_or_delete_current_slot() {
        let store = store();
        let current_id = current_workflow_id("creative", Some("workspace-a")).expect("current id");
        store
            .write_current(
                "creative",
                Some("workspace-a"),
                &json!({"nodes":[{"id":"live-only"}]}),
                None,
            )
            .expect("write live canvas");

        let overwrite = store.save_workflow(
            Some(&current_id),
            "named-collision",
            "creative",
            Some("workspace-a"),
            &json!({"nodes":[{"id":"named-overwrite"}]}),
        );
        let live_after_overwrite = store
            .read_current("creative", Some("workspace-a"))
            .expect("read live canvas")
            .graph
            .map(|graph| graph["nodes"].clone());
        let delete_result = store
            .delete_workflow(&current_id)
            .expect("delete collision id");
        let live_after_delete = store
            .read_current("creative", Some("workspace-a"))
            .expect("read live canvas after delete")
            .graph;
        eprintln!(
            "canvas workflow id collision audit: overwrite_ok={}, live_after_overwrite={:?}, delete_ok={}, live_after_delete_present={}",
            overwrite.is_ok(),
            live_after_overwrite,
            delete_result,
            live_after_delete.is_some(),
        );
        assert!(
            overwrite.is_err()
                && live_after_overwrite == Some(json!([{"id":"live-only"}]))
                && !delete_result
                && live_after_delete.is_some(),
            "named workflow APIs must reject reserved current canvas ids"
        );
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
            .write_current("creative", None, &oversized, None)
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

    #[test]
    fn audit_current_canvas_rejects_stale_window_snapshot() {
        let root = tempfile::tempdir().expect("temp root");
        let db_path = root.path().join("canvas.sqlite");
        let first_db = Arc::new(Database::open(&db_path).expect("first database"));
        let second_db = Arc::new(Database::open(&db_path).expect("second database"));
        let first = CanvasWorkflowStore::new(first_db);
        let second = CanvasWorkflowStore::new(second_db);

        let seed = first
            .write_current("creative", None, &json!({"nodes":[{"id":"base"}]}), None)
            .expect("seed");
        let stale_snapshot = second
            .read_current("creative", None)
            .expect("read stale snapshot");

        first
            .write_current(
                "creative",
                None,
                &json!({"nodes":[{"id":"base"},{"id":"first"}]}),
                Some(&seed.revision),
            )
            .expect("newer write");

        let mut stale_update = stale_snapshot.graph.expect("snapshot exists");
        stale_update["nodes"] = json!([{"id":"base"},{"id":"second"}]);
        let stale_result = second.write_current(
            "creative",
            None,
            &stale_update,
            stale_snapshot.revision.as_deref(),
        );
        let final_graph = second
            .read_current("creative", None)
            .expect("read final graph")
            .graph
            .expect("final graph exists");
        eprintln!(
            "stale_accepted={} final_nodes={:?}",
            stale_result.is_ok(),
            final_graph["nodes"]
        );
        assert!(
            stale_result.is_err(),
            "a stale canvas snapshot was accepted and overwrote a newer window update"
        );
        assert_eq!(stale_result.unwrap_err().code, "revision_conflict");
        assert_eq!(final_graph["nodes"], json!([{"id":"base"},{"id":"first"}]));
    }

    #[test]
    fn absent_read_is_create_only_and_equal_graph_writes_get_new_revisions() {
        let store = store();
        let absent = store.read_current("creative", None).unwrap();
        assert_eq!(
            serde_json::to_value(&absent).unwrap(),
            json!({"schemaVersion":1,"revision":null,"graph":null})
        );
        let graph = json!({"nodes":[{"id":"a"}]});
        let first = store.write_current("creative", None, &graph, None).unwrap();
        let conflict = store
            .write_current("creative", None, &json!({"nodes":[]}), None)
            .unwrap_err();
        assert_eq!(conflict.code, "revision_conflict");
        let wire = serde_json::to_value(&conflict).unwrap();
        assert_eq!(wire["schemaVersion"], 1);
        assert_eq!(wire["currentRevision"], first.revision);
        assert!(
            wire.get("graph").is_none(),
            "conflict must not echo graph content"
        );
        let second = store
            .write_current("creative", None, &graph, Some(&first.revision))
            .unwrap();
        assert_ne!(first.revision, second.revision);
        assert!(store
            .write_current("creative", None, &graph, Some(&first.revision))
            .is_err());
        assert_eq!(
            store.read_current("creative", None).unwrap().graph,
            Some(graph)
        );
    }

    #[test]
    fn legacy_documents_get_stable_read_tokens_without_mutation_and_upgrade_on_write() {
        let store = store();
        let id = current_workflow_id("creative", None).unwrap();
        let legacy = json!({"id":id,"name":"old","mode":"creative","graph":{"nodes":[{"id":"legacy"}]},"updatedAt":1,"current":true}).to_string();
        let documents = DocumentStore::new(&store.db);
        documents
            .put(
                CANVAS_WORKFLOW_COLLECTION,
                &id,
                &legacy,
                None,
                SystemClock.now(),
            )
            .unwrap();
        let first = store.read_current("creative", None).unwrap();
        assert_eq!(first, store.read_current("creative", None).unwrap());
        assert_eq!(
            documents
                .get(CANVAS_WORKFLOW_COLLECTION, &id)
                .unwrap()
                .unwrap()
                .body,
            legacy
        );
        assert!(store
            .write_current("creative", None, &json!({}), None)
            .is_err());
        let saved = store
            .write_current(
                "creative",
                None,
                first.graph.as_ref().unwrap(),
                first.revision.as_deref(),
            )
            .unwrap();
        assert_ne!(first.revision.as_ref(), Some(&saved.revision));
        assert!(store
            .load_workflow(&id)
            .unwrap()
            .unwrap()
            .revision
            .is_some());
    }

    #[test]
    fn concurrent_windows_have_one_winner_and_reopen_preserves_its_revision() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("canvas.sqlite");
        let seed = {
            let store = CanvasWorkflowStore::new(Arc::new(Database::open(&path).unwrap()));
            store
                .write_current("creative", Some("ws"), &json!({"nodes":[]}), None)
                .unwrap()
        };
        let first = CanvasWorkflowStore::new(Arc::new(Database::open(&path).unwrap()));
        let second = CanvasWorkflowStore::new(Arc::new(Database::open(&path).unwrap()));
        let gate = std::sync::Barrier::new(2);
        let write = |store: &CanvasWorkflowStore, id| {
            gate.wait();
            store.write_current(
                "creative",
                Some("ws"),
                &json!({"nodes":[{"id":id}]}),
                Some(&seed.revision),
            )
        };
        let results = std::thread::scope(|scope| {
            let one = scope.spawn(|| write(&first, "a"));
            let two = scope.spawn(|| write(&second, "b"));
            [one.join().unwrap(), two.join().unwrap()]
        });
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        let winner = results
            .iter()
            .find_map(|result| result.as_ref().ok())
            .unwrap();
        let graph = first.read_current("creative", Some("ws")).unwrap().graph;
        drop(first);
        drop(second);
        let reopened = CanvasWorkflowStore::new(Arc::new(Database::open(&path).unwrap()));
        let state = reopened.read_current("creative", Some("ws")).unwrap();
        assert_eq!(state.revision.as_ref(), Some(&winner.revision));
        assert_eq!(state.graph, graph);
        assert!(reopened
            .write_current("creative", Some("ws"), &json!({}), Some(&seed.revision))
            .is_err());
        for (mode, workspace) in [("professional", Some("ws")), ("creative", Some("other"))] {
            assert!(reopened
                .write_current(mode, workspace, &json!({}), None)
                .is_ok());
        }
    }

    #[test]
    fn failed_database_update_retains_graph_and_revision_for_retry() {
        let store = store();
        let initial = json!({"nodes":[{"id":"kept"}]});
        let saved = store
            .write_current("creative", None, &initial, None)
            .unwrap();
        store.db.with_conn(|connection| {
            connection.execute_batch("CREATE TRIGGER reject_canvas BEFORE UPDATE ON documents BEGIN SELECT RAISE(ABORT, 'synthetic disk failure'); END;")
                .map_err(|error| CoreError::Persistence(error.to_string()))
        }).unwrap();
        let error = store
            .write_current(
                "creative",
                None,
                &json!({"nodes":[]}),
                Some(&saved.revision),
            )
            .unwrap_err();
        assert_eq!(error.code, "write_failed");
        assert!(error.message.contains("synthetic disk failure"));
        let unchanged = store.read_current("creative", None).unwrap();
        assert_eq!(unchanged.revision, Some(saved.revision.clone()));
        assert_eq!(unchanged.graph, Some(initial));
        store
            .db
            .with_conn(|connection| {
                connection
                    .execute_batch("DROP TRIGGER reject_canvas")
                    .map_err(|error| CoreError::Persistence(error.to_string()))
            })
            .unwrap();
        assert!(store
            .write_current(
                "creative",
                None,
                &json!({"nodes":[]}),
                Some(&saved.revision)
            )
            .is_ok());
    }
}
