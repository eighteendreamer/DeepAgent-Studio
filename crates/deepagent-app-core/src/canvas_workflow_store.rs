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

/// Live slots and named workflows share a collection, but never an ID namespace.
const CURRENT_WORKFLOW_PREFIX: &str = "current:";

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
    Ok(format!("{CURRENT_WORKFLOW_PREFIX}{mode}:{workspace}"))
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

/// The persistent key, not the mutable `current` flag, owns the live namespace.
/// Keep legacy named IDs (including colons) valid outside that namespace.
fn validate_named_workflow_id(id: &str) -> Result<()> {
    validate_token(id, "workflow id")?;
    if id.starts_with(CURRENT_WORKFLOW_PREFIX) {
        return Err(CoreError::invalid(
            "reserved_workflow_id: current canvas IDs are not named workflow IDs",
        ));
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

/// Named workflows stay in the original document collection. A deletion is a
/// conditional replacement with a retained graph, not an unauditable hard delete.
/// Keep the storage-only marker out of the public, live workflow DTO.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredNamedWorkflow {
    #[serde(flatten)]
    document: CanvasWorkflowDoc,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    deleted_at: Option<i64>,
}

/// None is the legacy/default namespace, never a wildcard. Explicit invalid
/// identities must not silently fall back to that namespace.
fn named_workspace(workspace_id: Option<&str>) -> Result<Option<String>> {
    workspace_id
        .map(|value| {
            let value = value.trim();
            validate_token(value, "workspace id")?;
            Ok(value.to_string())
        })
        .transpose()
}

fn scoped_named_workflow(
    doc: &deepagent_persistence::document_store::Document,
    workspace: &Option<String>,
) -> Result<Option<StoredNamedWorkflow>> {
    let stored: StoredNamedWorkflow = serde_json::from_str(&doc.body)
        // Do not surface serde's invalid-value excerpt: it can contain graph data.
        .map_err(|_| CoreError::invalid("invalid stored named workflow document"))?;
    if stored.document.id != doc.id {
        return Err(CoreError::invalid(
            "named workflow document identity mismatch",
        ));
    }
    let owner = named_workspace(stored.document.workspace_id.as_deref())?;
    Ok(
        (stored.deleted_at.is_none() && !stored.document.current && owner == *workspace)
            .then_some(stored),
    )
}

fn named_workflow_unavailable(id: &str) -> CoreError {
    tracing::warn!(
        document_id = id,
        "named workflow mutation rejected: unavailable or changed"
    );
    CoreError::invalid("workflow_unavailable: named workflow is unavailable or changed")
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
        let workspace = named_workspace(workspace_id)?;
        let name = name.trim();
        if name.is_empty() {
            return Err(CoreError::invalid("canvas workflow name is required"));
        }
        let id = match id.map(str::trim) {
            Some(existing) if !existing.is_empty() => {
                validate_named_workflow_id(existing)?;
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
        let store = DocumentStore::new(&self.db);
        let previous = store.get(CANVAS_WORKFLOW_COLLECTION, &id)?;
        if let Some(previous) = &previous {
            if scoped_named_workflow(previous, &workspace)?.is_none() {
                return Err(named_workflow_unavailable(&id));
            }
        }
        let body = serde_json::to_string(&StoredNamedWorkflow {
            document: CanvasWorkflowDoc {
                id: id.clone(),
                name: meta.name.clone(),
                mode: meta.mode.clone(),
                workspace_id: workspace,
                graph: graph.clone(),
                updated_at: meta.updated_at,
                current: false,
                revision: Some(uuid::Uuid::new_v4().to_string()),
            },
            deleted_at: None,
        })?;
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
            return Err(named_workflow_unavailable(&id));
        }
        tracing::debug!(document_id = %id, "named workflow committed in requested workspace");
        Ok(meta)
    }

    /// Entries in exactly one workspace, newest first. No global-list fallback.
    pub fn list_workflows(&self, workspace_id: Option<&str>) -> Result<Vec<CanvasWorkflowMeta>> {
        let workspace = named_workspace(workspace_id)?;
        let mut items = Vec::new();
        for doc in DocumentStore::new(&self.db).list(CANVAS_WORKFLOW_COLLECTION)? {
            // The actual key owns the live namespace, including old collisions.
            if validate_named_workflow_id(&doc.id).is_err() {
                continue;
            }
            if let Some(stored) = scoped_named_workflow(&doc, &workspace)? {
                let document = stored.document;
                items.push(CanvasWorkflowMeta {
                    id: document.id,
                    name: document.name,
                    mode: document.mode,
                    updated_at: document.updated_at,
                });
            }
        }
        items.sort_by_key(|item| std::cmp::Reverse(item.updated_at));
        Ok(items)
    }

    pub fn load_workflow(
        &self,
        id: &str,
        workspace_id: Option<&str>,
    ) -> Result<Option<CanvasWorkflowDoc>> {
        validate_named_workflow_id(id)?;
        let workspace = named_workspace(workspace_id)?;
        let doc = DocumentStore::new(&self.db).get(CANVAS_WORKFLOW_COLLECTION, id)?;
        let Some(doc) = doc else { return Ok(None) };
        Ok(scoped_named_workflow(&doc, &workspace)?.map(|stored| stored.document))
    }

    /// Retain a tombstone and the graph atomically, bound to the checked owner.
    /// Absent, deleted, and different-workspace IDs all return false.
    pub fn delete_workflow(&self, id: &str, workspace_id: Option<&str>) -> Result<bool> {
        validate_named_workflow_id(id)?;
        let workspace = named_workspace(workspace_id)?;
        let store = DocumentStore::new(&self.db);
        let Some(previous) = store.get(CANVAS_WORKFLOW_COLLECTION, id)? else {
            return Ok(false);
        };
        let Some(mut stored) = scoped_named_workflow(&previous, &workspace)? else {
            tracing::debug!(
                document_id = id,
                "named workflow delete: unavailable in requested workspace"
            );
            return Ok(false);
        };
        stored.deleted_at = Some(self.now());
        stored.document.revision = Some(uuid::Uuid::new_v4().to_string());
        let body = serde_json::to_string(&stored)?;
        if !store.replace_body_if_unchanged(
            CANVAS_WORKFLOW_COLLECTION,
            id,
            &previous.body,
            &body,
            SystemClock.now(),
        )? {
            return Err(named_workflow_unavailable(id));
        }
        tracing::info!(
            document_id = id,
            "named workflow deleted with retained graph and owner"
        );
        Ok(true)
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
        let listed = store.list_workflows(None).expect("list");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].name, "海报流水线");

        let loaded = store
            .load_workflow(&saved.id, None)
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
        assert_eq!(store.list_workflows(None).expect("list").len(), 1);
        assert_eq!(
            store.list_workflows(None).expect("list")[0].name,
            "改名后的流水线"
        );
        assert!(store.delete_workflow(&saved.id, None).expect("delete"));
        assert!(!store
            .delete_workflow(&saved.id, None)
            .expect("second delete"));
        assert!(store.list_workflows(None).expect("list").is_empty());
    }

    #[test]
    fn library_list_excludes_live_canvas_slots() {
        let store = store();
        store
            .write_current("creative", None, &json!({ "nodes": [] }), None)
            .expect("current");
        assert!(store.list_workflows(None).expect("list").is_empty());
    }

    #[test]
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

        let before_b = store
            .load_workflow(&workspace_b.id, Some("workspace-b"))
            .unwrap()
            .unwrap();
        let listed = store
            .list_workflows(Some("workspace-a"))
            .expect("list workflows");
        let loaded_b = store
            .load_workflow(&workspace_b.id, Some("workspace-a"))
            .expect("load workspace B from A");
        let deleted_b = store
            .delete_workflow(&workspace_b.id, Some("workspace-a"))
            .expect("delete workspace B from A");
        let overwrite_b = store.save_workflow(
            Some(&workspace_b.id),
            "attempted overwrite",
            "creative",
            Some("workspace-a"),
            &json!({"nodes":[{"id":"must-not-reach-b"}]}),
        );
        let list_exposes_b = listed.iter().any(|item| item.id == workspace_b.id);
        let load_exposes_b = loaded_b.is_some();
        eprintln!(
            "canvas workflow scope audit: listed_ids={:?}, list_exposes_b={}, loaded_workspace={:?}, load_exposes_b={}, deleted_other_workspace={}, retained_a_id={}",
            listed.iter().map(|item| item.id.as_str()).collect::<Vec<_>>(),
            list_exposes_b,
            loaded_b.as_ref().and_then(|doc| doc.workspace_id.as_deref()),
            load_exposes_b,
            deleted_b,
            store
                .load_workflow(&workspace_a.id, Some("workspace-a"))
                .expect("load workspace A")
                .is_some(),
        );
        assert!(
            !list_exposes_b && !load_exposes_b && !deleted_b && overwrite_b.is_err(),
            "workspace-scoped canvas APIs exposed or deleted another workspace's workflow"
        );
        assert_eq!(
            store
                .load_workflow(&workspace_b.id, Some("workspace-b"))
                .unwrap(),
            Some(before_b)
        );
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, workspace_a.id);
    }

    #[test]
    fn named_workflows_isolate_default_and_explicit_workspaces_for_all_operations() {
        let store = store();
        let scopes = [None, Some("workspace-a"), Some("workspace-b")];
        let saved: Vec<_> = scopes
            .iter()
            .map(|scope| {
                store
                    .save_workflow(None, "scoped", "creative", *scope, &json!({"owner":scope}))
                    .unwrap()
            })
            .collect();
        for (caller, scope) in scopes.iter().enumerate() {
            assert_eq!(
                store.list_workflows(*scope).unwrap(),
                vec![saved[caller].clone()]
            );
            for (owner, item) in saved.iter().enumerate() {
                if caller == owner {
                    assert!(store.load_workflow(&item.id, *scope).unwrap().is_some());
                    continue;
                }
                assert_eq!(store.load_workflow(&item.id, *scope).unwrap(), None);
                assert!(!store.delete_workflow(&item.id, *scope).unwrap());
                let error = store
                    .save_workflow(Some(&item.id), "forbidden", "creative", *scope, &json!({}))
                    .unwrap_err();
                assert!(error.to_string().contains("workflow_unavailable"));
                assert_eq!(
                    store
                        .load_workflow(&item.id, scopes[owner])
                        .unwrap()
                        .unwrap()
                        .graph,
                    json!({"owner":scopes[owner]})
                );
            }
            assert!(store.load_workflow("wf-unknown", *scope).unwrap().is_none());
            assert!(!store.delete_workflow("wf-unknown", *scope).unwrap());
        }
        for (scope, item) in scopes.iter().zip(saved) {
            store
                .save_workflow(
                    Some(&item.id),
                    "normal update",
                    "professional",
                    *scope,
                    &json!({"updated":true}),
                )
                .unwrap();
            assert_eq!(
                store
                    .load_workflow(&item.id, *scope)
                    .unwrap()
                    .unwrap()
                    .graph,
                json!({"updated":true})
            );
            assert!(store.delete_workflow(&item.id, *scope).unwrap());
            assert!(store.list_workflows(*scope).unwrap().is_empty());
        }
    }

    #[test]
    fn named_delete_retains_graph_owner_and_receipt_without_reassigning_the_id() {
        let store = store();
        let schema = store.db.schema_version().unwrap();
        let graph = json!({"nodes":[{"id":"recoverable"}]});
        let saved = store
            .save_workflow(None, "kept", "creative", Some("ws-a"), &graph)
            .unwrap();
        let documents = DocumentStore::new(&store.db);
        let before = documents
            .get(CANVAS_WORKFLOW_COLLECTION, &saved.id)
            .unwrap()
            .unwrap();
        assert!(store.delete_workflow(&saved.id, Some("ws-a")).unwrap());
        let after = documents
            .get(CANVAS_WORKFLOW_COLLECTION, &saved.id)
            .unwrap()
            .unwrap();
        let tombstone: StoredNamedWorkflow = serde_json::from_str(&after.body).unwrap();
        assert!(tombstone.deleted_at.is_some());
        assert_eq!(tombstone.document.graph, graph);
        assert_eq!(tombstone.document.workspace_id.as_deref(), Some("ws-a"));
        assert_ne!(
            tombstone.document.revision,
            decode(&before).unwrap().revision
        );
        for scope in [None, Some("ws-a"), Some("ws-b")] {
            assert!(store.load_workflow(&saved.id, scope).unwrap().is_none());
            assert!(!store.delete_workflow(&saved.id, scope).unwrap());
            assert!(store
                .save_workflow(Some(&saved.id), "reclaim", "creative", scope, &json!({}))
                .is_err());
            assert!(store.list_workflows(scope).unwrap().is_empty());
        }
        assert_eq!(
            documents
                .get(CANVAS_WORKFLOW_COLLECTION, &saved.id)
                .unwrap()
                .unwrap()
                .body,
            after.body
        );
        assert_eq!(store.db.schema_version().unwrap(), schema);
    }

    #[test]
    fn legacy_missing_workspace_is_default_only_and_normalized_owners_keep_their_scope() {
        let store = store();
        let documents = DocumentStore::new(&store.db);
        for (id, workspace) in [("wf-legacy", None), ("wf-spaced", Some(" ws-a "))] {
            let mut body = json!({"id":id,"name":"legacy","mode":"creative","graph":{"nodes":[]},"updatedAt":1});
            if let Some(workspace) = workspace {
                body["workspaceId"] = json!(workspace);
            }
            documents
                .put(
                    CANVAS_WORKFLOW_COLLECTION,
                    id,
                    &body.to_string(),
                    None,
                    SystemClock.now(),
                )
                .unwrap();
        }
        assert_eq!(store.list_workflows(None).unwrap()[0].id, "wf-legacy");
        assert_eq!(
            store.list_workflows(Some("ws-a")).unwrap()[0].id,
            "wf-spaced"
        );
        assert!(store
            .load_workflow("wf-legacy", Some("ws-a"))
            .unwrap()
            .is_none());
        assert!(store.load_workflow("wf-spaced", None).unwrap().is_none());
        let before = documents
            .get(CANVAS_WORKFLOW_COLLECTION, "wf-spaced")
            .unwrap()
            .unwrap();
        assert!(store
            .load_workflow("wf-spaced", Some(" ws-a "))
            .unwrap()
            .is_some());
        assert_eq!(
            documents
                .get(CANVAS_WORKFLOW_COLLECTION, "wf-spaced")
                .unwrap()
                .unwrap()
                .body,
            before.body,
            "reads must not silently migrate old rows"
        );
        store
            .save_workflow(
                Some("wf-spaced"),
                "updated",
                "creative",
                Some(" ws-a "),
                &json!({}),
            )
            .unwrap();
        assert_eq!(
            store
                .load_workflow("wf-spaced", Some("ws-a"))
                .unwrap()
                .unwrap()
                .workspace_id
                .as_deref(),
            Some("ws-a")
        );
        for invalid in ["", "  ", "bad/path", "工作区"] {
            assert!(store.list_workflows(Some(invalid)).is_err());
            assert!(store.load_workflow("wf-legacy", Some(invalid)).is_err());
            assert!(store.delete_workflow("wf-legacy", Some(invalid)).is_err());
            assert!(store
                .save_workflow(None, "invalid", "creative", Some(invalid), &json!({}))
                .is_err());
        }
        assert!(store.load_workflow("wf-legacy", None).unwrap().is_some());
    }

    #[test]
    fn failed_tombstone_commit_preserves_the_complete_original_document() {
        let store = store();
        let saved = store
            .save_workflow(
                None,
                "kept",
                "creative",
                Some("ws-a"),
                &json!({"nodes":[1]}),
            )
            .unwrap();
        let documents = DocumentStore::new(&store.db);
        let before = documents
            .get(CANVAS_WORKFLOW_COLLECTION, &saved.id)
            .unwrap()
            .unwrap();
        store.db.with_conn(|conn| {
            conn.execute_batch("CREATE TRIGGER reject_workflow_delete BEFORE UPDATE ON documents WHEN NEW.collection = 'canvas_workflows' BEGIN SELECT RAISE(ABORT, 'synthetic failed commit'); END;").unwrap();
            Ok(())
        }).unwrap();
        assert!(store.delete_workflow(&saved.id, Some("ws-a")).is_err());
        assert_eq!(
            documents
                .get(CANVAS_WORKFLOW_COLLECTION, &saved.id)
                .unwrap()
                .unwrap(),
            before
        );
        assert!(store
            .load_workflow(&saved.id, Some("ws-a"))
            .unwrap()
            .is_some());
        assert_eq!(store.list_workflows(Some("ws-a")).unwrap().len(), 1);
    }

    #[test]
    fn concurrent_named_creators_cannot_take_over_each_others_workspace() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("named-scopes.sqlite");
        let first = CanvasWorkflowStore::new(Arc::new(Database::open(&path).unwrap()));
        let second = CanvasWorkflowStore::new(Arc::new(Database::open(&path).unwrap()));
        for attempt in 0..12 {
            let id = format!("wf-race-{attempt}");
            let barrier = std::sync::Barrier::new(2);
            let create = |store: &CanvasWorkflowStore, owner| {
                barrier.wait();
                store.save_workflow(
                    Some(&id),
                    "racing",
                    "creative",
                    Some(owner),
                    &json!({"owner":owner}),
                )
            };
            let (a, b) = std::thread::scope(|threads| {
                let a = threads.spawn(|| create(&first, "ws-a"));
                let b = threads.spawn(|| create(&second, "ws-b"));
                (a.join().unwrap(), b.join().unwrap())
            });
            assert_ne!(
                a.is_ok(),
                b.is_ok(),
                "exactly one namespace owns a concurrently created id"
            );
            let (owner, other) = if a.is_ok() {
                ("ws-a", "ws-b")
            } else {
                ("ws-b", "ws-a")
            };
            assert_eq!(
                first
                    .load_workflow(&id, Some(owner))
                    .unwrap()
                    .unwrap()
                    .graph,
                json!({"owner":owner})
            );
            assert!(!second.delete_workflow(&id, Some(other)).unwrap());
            assert!(first.delete_workflow(&id, Some(owner)).unwrap());
            assert!(second
                .save_workflow(
                    Some(&id),
                    "reuse after delete",
                    "creative",
                    Some(other),
                    &json!({})
                )
                .is_err());
        }
    }

    #[test]
    fn concurrent_named_save_and_delete_preserve_the_acknowledged_graph_and_owner() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("named-mutations.sqlite");
        let db = Arc::new(Database::open(&path).unwrap());
        let first = CanvasWorkflowStore::new(db.clone());
        let second = CanvasWorkflowStore::new(Arc::new(Database::open(&path).unwrap()));
        for attempt in 0..12 {
            let saved = first
                .save_workflow(None, "before", "creative", Some("ws-a"), &json!({"v":0}))
                .unwrap();
            let barrier = std::sync::Barrier::new(2);
            let graph = json!({"v":attempt + 1});
            let (update, deletion) = std::thread::scope(|threads| {
                let update = threads.spawn(|| {
                    barrier.wait();
                    first.save_workflow(Some(&saved.id), "after", "creative", Some("ws-a"), &graph)
                });
                let deletion = threads.spawn(|| {
                    barrier.wait();
                    second.delete_workflow(&saved.id, Some("ws-a"))
                });
                (update.join().unwrap(), deletion.join().unwrap())
            });
            assert!(update.is_ok() || matches!(deletion, Ok(true)));
            let raw = DocumentStore::new(&db)
                .get(CANVAS_WORKFLOW_COLLECTION, &saved.id)
                .unwrap()
                .unwrap();
            let stored: StoredNamedWorkflow = serde_json::from_str(&raw.body).unwrap();
            assert_eq!(stored.document.workspace_id.as_deref(), Some("ws-a"));
            assert_eq!(stored.deleted_at.is_some(), matches!(deletion, Ok(true)));
            assert_eq!(
                stored.document.graph,
                if update.is_ok() {
                    graph
                } else {
                    json!({"v":0})
                }
            );
            assert!(second
                .load_workflow(&saved.id, Some("ws-b"))
                .unwrap()
                .is_none());
            assert!(!second.delete_workflow(&saved.id, Some("ws-b")).unwrap());
            assert!(second
                .save_workflow(
                    Some(&saved.id),
                    "takeover",
                    "creative",
                    Some("ws-b"),
                    &json!({})
                )
                .is_err());
        }
    }

    #[test]
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
        let before = store.read_current("creative", Some("workspace-a")).unwrap();

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
        let delete_result = store.delete_workflow(&current_id, None);
        let live_after_delete = store
            .read_current("creative", Some("workspace-a"))
            .expect("read live canvas after delete")
            .graph;
        eprintln!(
            "canvas workflow id collision audit: overwrite_ok={}, live_after_overwrite={:?}, delete_ok={:?}, live_after_delete_present={}",
            overwrite.is_ok(),
            live_after_overwrite,
            delete_result,
            live_after_delete.is_some(),
        );
        assert!(
            overwrite.is_err()
                && live_after_overwrite == Some(json!([{"id":"live-only"}]))
                && delete_result.is_err()
                && live_after_delete.is_some(),
            "named workflow APIs must reject reserved current canvas ids"
        );
        for error in [
            overwrite.unwrap_err(),
            delete_result.unwrap_err(),
            store.load_workflow(&current_id, None).unwrap_err(),
        ] {
            assert!(error.to_string().contains("reserved_workflow_id"));
        }
        assert_eq!(
            store.read_current("creative", Some("workspace-a")).unwrap(),
            before,
            "failed named operations must retain both graph and revision"
        );
        let document = DocumentStore::new(&store.db)
            .get(CANVAS_WORKFLOW_COLLECTION, &current_id)
            .unwrap()
            .unwrap();
        assert!(decode(&document).unwrap().current);
        assert!(store.list_workflows(None).unwrap().is_empty());
    }

    #[test]
    fn named_apis_cannot_preoccupy_any_live_namespace_key() {
        let store = store();
        let mut ids = vec![
            "current:".to_string(),
            "current:future:anything".to_string(),
        ];
        for mode in CANVAS_MODES {
            for workspace in [None, Some("ws-a"), Some("nested:ws-b")] {
                ids.push(current_workflow_id(mode, workspace).unwrap());
            }
        }
        for id in ids {
            for supplied in [id.clone(), format!("  {id}  ")] {
                let error = store
                    .save_workflow(Some(&supplied), "rejected", "creative", None, &json!({}))
                    .unwrap_err();
                assert!(error.to_string().contains("reserved_workflow_id"));
            }
            assert!(store.load_workflow(&id, None).is_err());
            assert!(store.delete_workflow(&id, None).is_err());
            assert!(DocumentStore::new(&store.db)
                .get(CANVAS_WORKFLOW_COLLECTION, &id)
                .unwrap()
                .is_none());
        }
        assert!(store.list_workflows(None).unwrap().is_empty());
        // Rejected preoccupation must not prevent a legitimate create-only write.
        for mode in CANVAS_MODES {
            store
                .write_current(mode, None, &json!({"nodes":[]}), None)
                .unwrap();
        }
    }

    #[test]
    fn named_ids_keep_legacy_spelling_and_normal_lifecycle() {
        let store = store();
        for id in [
            "wf-demo",
            "legacy:import-1",
            "current-copy",
            "CURRENT:creative:_default",
        ] {
            let saved = store
                .save_workflow(Some(id), "first", "creative", None, &json!({"nodes":[]}))
                .unwrap();
            assert_eq!(saved.id, id);
            store
                .save_workflow(Some(id), "updated", "creative", None, &json!({"nodes":[1]}))
                .unwrap();
            let loaded = store.load_workflow(id, None).unwrap().unwrap();
            assert_eq!(loaded.graph, json!({"nodes":[1]}));
            assert_eq!(loaded.name, "updated");
            assert!(!loaded.current);
            assert!(store
                .list_workflows(None)
                .unwrap()
                .iter()
                .any(|entry| entry.id == id));
            assert!(store.delete_workflow(id, None).unwrap());
            assert!(!store.delete_workflow(id, None).unwrap());
            assert!(store.load_workflow(id, None).unwrap().is_none());
        }
    }

    #[test]
    fn historical_collisions_are_not_library_entries_or_silently_rewritten() {
        let store = store();
        let documents = DocumentStore::new(&store.db);
        for (mode, payload_id) in [
            ("creative", "current:creative:_default"),
            ("professional", "wf-alias"),
        ] {
            let id = current_workflow_id(mode, None).unwrap();
            // Previous named overwrite changed current to false. Even a legacy
            // payload whose ID disagrees with its key must not expose the slot.
            let body = json!({"id":payload_id,"name":"old collision","mode":mode,
                "graph":{"nodes":[{"id":"retained"}]},"updatedAt":1,"current":false})
            .to_string();
            documents
                .put(
                    CANVAS_WORKFLOW_COLLECTION,
                    &id,
                    &body,
                    None,
                    SystemClock.now(),
                )
                .unwrap();
            let before = store.read_current(mode, None).unwrap();
            assert!(store.list_workflows(None).unwrap().is_empty());
            assert!(store.load_workflow(&id, None).is_err());
            assert!(store.delete_workflow(&id, None).is_err());
            assert!(store
                .save_workflow(Some(&id), "rejected", mode, None, &json!({}))
                .is_err());
            assert_eq!(
                documents
                    .get(CANVAS_WORKFLOW_COLLECTION, &id)
                    .unwrap()
                    .unwrap()
                    .body,
                body
            );
            assert_eq!(store.read_current(mode, None).unwrap(), before);
        }
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
        let written = documents
            .get(CANVAS_WORKFLOW_COLLECTION, &id)
            .unwrap()
            .unwrap();
        assert!(decode(&written).unwrap().revision.is_some());
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
