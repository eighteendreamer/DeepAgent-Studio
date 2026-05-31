//! `knowledge_search` / `knowledge_write` — the knowledge base's active
//! channel.
//!
//! Beyond the host's *passive* injection (relevant knowledge auto-added to the
//! prompt each turn), these two tools let the model deliberately look deeper or
//! capture new knowledge:
//!
//! - `knowledge_search` — read-only retrieval over the knowledge base. Safe,
//!   concurrency-friendly, never needs approval. Sub-agents get this too.
//! - `knowledge_write` — persist a reusable note (pitfall / fix / command /
//!   config). Writes a file, so it is `Low` risk and needs `WorkspaceWrite`.
//!   The main agent gets this; sub-agents do not (they should not litter the
//!   vault).
//!
//! Like `task`/`ask_user_question`, the actual storage is delegated to a
//! pluggable [`KnowledgeBackend`] the host wires up (the desktop app backs it
//! with the real `KnowledgeService`; headless/tests use a stub).

use async_trait::async_trait;

use deepagent_core::error::Result;
use deepagent_tools::permission::{Permission, PermissionSet, RiskLevel};
use deepagent_tools::{Tool, ToolDescriptor, ToolOutput};

/// The tool name for read-only knowledge retrieval.
pub const KNOWLEDGE_SEARCH_TOOL_NAME: &str = "knowledge_search";
/// The tool name for persisting a knowledge entry.
pub const KNOWLEDGE_WRITE_TOOL_NAME: &str = "knowledge_write";

/// One hit returned by a [`KnowledgeBackend`] search (plain, transport-only).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct KnowledgeToolHit {
    /// Stable entry id (scope-qualified).
    pub id: String,
    /// Entry title.
    pub title: String,
    /// Entry kind label (pitfall/solution/command/config/note).
    pub kind: String,
    /// Vault scope label (project/global).
    pub scope: String,
    /// Relevance score in `[0,1]`.
    pub score: f32,
    /// Matching excerpt.
    pub excerpt: String,
}

/// A draft to persist via a [`KnowledgeBackend`] write (plain, transport-only).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnowledgeToolDraft {
    /// Entry title.
    pub title: String,
    /// Markdown body.
    pub content: String,
    /// Optional kind label; backend defaults unknown/empty to `note`.
    pub kind: Option<String>,
    /// Tags.
    pub tags: Vec<String>,
}

/// Bridges the knowledge tools to the host's knowledge base.
///
/// The default direction is host → tool: the desktop app implements this over
/// its `KnowledgeService`; headless contexts use [`UnavailableKnowledgeBackend`].
#[async_trait]
pub trait KnowledgeBackend: Send + Sync {
    /// Search the knowledge base. `kind` optionally filters by kind label.
    async fn search(
        &self,
        query: &str,
        kind: Option<String>,
        limit: usize,
    ) -> Result<Vec<KnowledgeToolHit>>;

    /// Persist a knowledge entry, returning its id.
    async fn write(&self, draft: KnowledgeToolDraft) -> Result<String>;
}

/// A backend that reports the knowledge base is unavailable (headless default).
#[derive(Debug, Default, Clone, Copy)]
pub struct UnavailableKnowledgeBackend;

#[async_trait]
impl KnowledgeBackend for UnavailableKnowledgeBackend {
    async fn search(
        &self,
        _query: &str,
        _kind: Option<String>,
        _limit: usize,
    ) -> Result<Vec<KnowledgeToolHit>> {
        Ok(Vec::new())
    }

    async fn write(&self, _draft: KnowledgeToolDraft) -> Result<String> {
        Err(deepagent_core::error::CoreError::other(
            "the knowledge base is not configured in this environment",
        ))
    }
}

/// Default cap on `knowledge_search` results.
const DEFAULT_SEARCH_LIMIT: usize = 5;
/// Hard cap on `knowledge_search` results.
const MAX_SEARCH_LIMIT: usize = 20;

/// The `knowledge_search` tool over a pluggable [`KnowledgeBackend`].
pub struct KnowledgeSearchTool<B: KnowledgeBackend> {
    backend: B,
}

impl<B: KnowledgeBackend> KnowledgeSearchTool<B> {
    /// Build the tool with a backend.
    pub fn new(backend: B) -> Self {
        Self { backend }
    }
}

#[async_trait]
impl<B: KnowledgeBackend> Tool for KnowledgeSearchTool<B> {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: KNOWLEDGE_SEARCH_TOOL_NAME.into(),
            description: "Search the knowledge base for accumulated, project-specific experience: \
                pitfalls already hit, fixes that worked, frequently used commands, and important \
                configs. Use this when you face an unfamiliar error, a recurring problem, or need \
                a project convention — check existing experience before guessing. Returns matching \
                entries with a relevance score; an empty result simply means nothing relevant has \
                been recorded yet."
                .into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "What to look up — an error message, symptom, command, config key, or topic."
                    },
                    "kind": {
                        "type": "string",
                        "description": "Optional filter: pitfall | solution | command | config | note.",
                        "enum": ["pitfall", "solution", "command", "config", "note"]
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Max number of entries to return (default 5).",
                        "minimum": 1,
                        "maximum": 20
                    }
                },
                "required": ["query"]
            }),
            // Read-only and concurrency-safe; never needs approval.
            risk: RiskLevel::Safe,
            required_permissions: PermissionSet::read_only(),
        }
    }

    async fn invoke(&self, args: serde_json::Value) -> Result<ToolOutput> {
        let Some(query) = args.get("query").and_then(|v| v.as_str()) else {
            return Ok(ToolOutput::failure("missing 'query'"));
        };
        if query.trim().is_empty() {
            return Ok(ToolOutput::failure("'query' must not be empty"));
        }
        let kind = args
            .get("kind")
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
            .map(|s| s.to_string());
        let limit = args
            .get("limit")
            .and_then(|v| v.as_u64())
            .map(|n| (n as usize).clamp(1, MAX_SEARCH_LIMIT))
            .unwrap_or(DEFAULT_SEARCH_LIMIT);

        let hits = self.backend.search(query, kind, limit).await?;
        // Empty result is success, not an error: the model learns the base has
        // nothing relevant yet (and should not retry endlessly).
        Ok(ToolOutput::success(serde_json::json!({
            "count": hits.len(),
            "hits": hits,
        })))
    }
}

/// The `knowledge_write` tool over a pluggable [`KnowledgeBackend`].
pub struct KnowledgeWriteTool<B: KnowledgeBackend> {
    backend: B,
}

impl<B: KnowledgeBackend> KnowledgeWriteTool<B> {
    /// Build the tool with a backend.
    pub fn new(backend: B) -> Self {
        Self { backend }
    }
}

#[async_trait]
impl<B: KnowledgeBackend> Tool for KnowledgeWriteTool<B> {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: KNOWLEDGE_WRITE_TOOL_NAME.into(),
            description: "Save a reusable piece of knowledge to the knowledge base so it is not \
                rediscovered the hard way next time. Use after you solve a non-obvious problem, \
                confirm a useful command, or learn an important config or pitfall. Write a clear, \
                self-contained note: a specific title and a body covering the symptom and the \
                resolution. Re-using an existing title updates that entry instead of duplicating."
                .into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "title": {
                        "type": "string",
                        "description": "A specific, searchable title for the entry."
                    },
                    "content": {
                        "type": "string",
                        "description": "The Markdown body: what the pitfall/fix/command/config is, and how to apply it."
                    },
                    "kind": {
                        "type": "string",
                        "description": "Category of the entry (defaults to note).",
                        "enum": ["pitfall", "solution", "command", "config", "note"]
                    },
                    "tags": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "Optional tags for filtering (e.g. windows, cargo, tauri)."
                    }
                },
                "required": ["title", "content"]
            }),
            // Writes a file in the vault: reversible local mutation.
            risk: RiskLevel::Low,
            required_permissions: PermissionSet::from_iter_perms([Permission::WorkspaceWrite]),
        }
    }

    async fn invoke(&self, args: serde_json::Value) -> Result<ToolOutput> {
        let Some(title) = args.get("title").and_then(|v| v.as_str()) else {
            return Ok(ToolOutput::failure("missing 'title'"));
        };
        if title.trim().is_empty() {
            return Ok(ToolOutput::failure("'title' must not be empty"));
        }
        let Some(content) = args.get("content").and_then(|v| v.as_str()) else {
            return Ok(ToolOutput::failure("missing 'content'"));
        };
        if content.trim().is_empty() {
            return Ok(ToolOutput::failure("'content' must not be empty"));
        }
        let kind = args
            .get("kind")
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
            .map(|s| s.to_string());
        let tags = args
            .get("tags")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|t| t.as_str())
                    .filter(|s| !s.trim().is_empty())
                    .map(|s| s.to_string())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        let draft = KnowledgeToolDraft {
            title: title.to_string(),
            content: content.to_string(),
            kind,
            tags,
        };
        match self.backend.write(draft).await {
            Ok(id) => Ok(ToolOutput::success(serde_json::json!({
                "saved": true,
                "id": id,
            }))),
            Err(e) => Ok(ToolOutput::failure(format!(
                "failed to save knowledge entry: {e}"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct StubBackend {
        entries: Mutex<Vec<KnowledgeToolDraft>>,
    }

    #[async_trait]
    impl KnowledgeBackend for StubBackend {
        async fn search(
            &self,
            query: &str,
            kind: Option<String>,
            limit: usize,
        ) -> Result<Vec<KnowledgeToolHit>> {
            let entries = self.entries.lock().unwrap();
            let hits = entries
                .iter()
                .filter(|d| d.content.contains(query) || d.title.contains(query))
                .filter(|d| match &kind {
                    Some(k) => d.kind.as_deref() == Some(k.as_str()),
                    None => true,
                })
                .take(limit)
                .enumerate()
                .map(|(i, d)| KnowledgeToolHit {
                    id: format!("project:entry-{i}"),
                    title: d.title.clone(),
                    kind: d.kind.clone().unwrap_or_else(|| "note".into()),
                    scope: "project".into(),
                    score: 0.9,
                    excerpt: d.content.clone(),
                })
                .collect();
            Ok(hits)
        }

        async fn write(&self, draft: KnowledgeToolDraft) -> Result<String> {
            let mut entries = self.entries.lock().unwrap();
            let id = format!("project:{}", entries.len());
            entries.push(draft);
            Ok(id)
        }
    }

    #[tokio::test]
    async fn search_returns_hits() {
        let backend = StubBackend::default();
        backend
            .write(KnowledgeToolDraft {
                title: "Redirect trick".into(),
                content: "use redirect to avoid the pipe interrupt".into(),
                kind: Some("pitfall".into()),
                tags: vec![],
            })
            .await
            .unwrap();
        let tool = KnowledgeSearchTool::new(backend);
        let out = tool
            .invoke(serde_json::json!({"query": "redirect"}))
            .await
            .unwrap();
        assert!(out.ok);
        assert_eq!(out.value["count"], 1);
        assert_eq!(out.value["hits"][0]["title"], "Redirect trick");
    }

    #[tokio::test]
    async fn search_empty_is_success_not_error() {
        let tool = KnowledgeSearchTool::new(StubBackend::default());
        let out = tool
            .invoke(serde_json::json!({"query": "nothing here"}))
            .await
            .unwrap();
        assert!(out.ok);
        assert_eq!(out.value["count"], 0);
        assert!(out.value["hits"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn search_kind_filter() {
        let backend = StubBackend::default();
        backend
            .write(KnowledgeToolDraft {
                title: "cmd".into(),
                content: "cargo test offline".into(),
                kind: Some("command".into()),
                tags: vec![],
            })
            .await
            .unwrap();
        backend
            .write(KnowledgeToolDraft {
                title: "note".into(),
                content: "cargo test offline general note".into(),
                kind: Some("note".into()),
                tags: vec![],
            })
            .await
            .unwrap();
        let tool = KnowledgeSearchTool::new(backend);
        let out = tool
            .invoke(serde_json::json!({"query": "cargo", "kind": "command"}))
            .await
            .unwrap();
        assert_eq!(out.value["count"], 1);
        assert_eq!(out.value["hits"][0]["kind"], "command");
    }

    #[tokio::test]
    async fn search_missing_query_fails() {
        let tool = KnowledgeSearchTool::new(StubBackend::default());
        let out = tool.invoke(serde_json::json!({})).await.unwrap();
        assert!(!out.ok);
    }

    #[tokio::test]
    async fn write_returns_id() {
        let tool = KnowledgeWriteTool::new(StubBackend::default());
        let out = tool
            .invoke(serde_json::json!({
                "title": "PowerShell pipe interrupt",
                "content": "Redirect to a file instead of piping.",
                "kind": "pitfall",
                "tags": ["windows", "powershell"]
            }))
            .await
            .unwrap();
        assert!(out.ok);
        assert_eq!(out.value["saved"], true);
        assert!(out.value["id"].as_str().unwrap().starts_with("project:"));
    }

    #[tokio::test]
    async fn write_missing_fields_fails() {
        let tool = KnowledgeWriteTool::new(StubBackend::default());
        assert!(
            !tool
                .invoke(serde_json::json!({"title": "x"}))
                .await
                .unwrap()
                .ok
        );
        assert!(
            !tool
                .invoke(serde_json::json!({"content": "y"}))
                .await
                .unwrap()
                .ok
        );
    }

    #[tokio::test]
    async fn write_unavailable_backend_reports_failure() {
        let tool = KnowledgeWriteTool::new(UnavailableKnowledgeBackend);
        let out = tool
            .invoke(serde_json::json!({"title": "x", "content": "y"}))
            .await
            .unwrap();
        assert!(!out.ok);
    }

    #[test]
    fn descriptors_have_correct_risk_and_permissions() {
        let search = KnowledgeSearchTool::new(UnavailableKnowledgeBackend);
        let sd = search.descriptor();
        assert_eq!(sd.name, KNOWLEDGE_SEARCH_TOOL_NAME);
        assert_eq!(sd.risk, RiskLevel::Safe);
        assert!(!sd.required_permissions.contains(Permission::WorkspaceWrite));

        let write = KnowledgeWriteTool::new(UnavailableKnowledgeBackend);
        let wd = write.descriptor();
        assert_eq!(wd.name, KNOWLEDGE_WRITE_TOOL_NAME);
        assert_eq!(wd.risk, RiskLevel::Low);
        assert!(wd.required_permissions.contains(Permission::WorkspaceWrite));
    }
}
