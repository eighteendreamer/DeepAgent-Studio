//! `memory_write` / `memory_recall` — cross-session multi-tier memory (D3).
//!
//! The item-level tier stores (`semantic`/`episodic`/`procedural`/`workspace`/
//! `failure`) were in-memory blueprint primitives with no persistence. These
//! tools give the agent a durable, cross-session memory: write a fact/habit/past
//! failure, recall the most relevant ones later (ranked by importance × recency,
//! NOT a second semantic index — the chunk-level knowledge base remains the sole
//! retrieval track per the knowledge-base design).
//!
//! Like the knowledge/task tools, the store is a pluggable [`MemoryBackend`] the
//! host wires up, keeping `deepagent-builtins` runtime/persistence-agnostic.

use async_trait::async_trait;

use deepagent_core::error::Result;
use deepagent_tools::permission::{Permission, PermissionSet, RiskLevel};
use deepagent_tools::{Tool, ToolDescriptor, ToolOutput};

/// The `memory_write` tool name.
pub const MEMORY_WRITE_TOOL_NAME: &str = "memory_write";
/// The `memory_recall` tool name.
pub const MEMORY_RECALL_TOOL_NAME: &str = "memory_recall";

/// The tiers a memory may belong to (mirrors `deepagent_memory::MemoryTier`).
pub const MEMORY_TIERS: [&str; 5] = ["semantic", "episodic", "procedural", "workspace", "failure"];

/// A new memory to persist.
#[derive(Debug, Clone, PartialEq)]
pub struct MemoryWriteDraft {
    /// One of [`MEMORY_TIERS`].
    pub tier: String,
    /// Natural-language memory content.
    pub content: String,
    /// Baseline importance in `[0,1]` (defaults applied by the backend).
    pub importance: Option<f32>,
    /// Optional tags for filtering.
    pub tags: Vec<String>,
}

/// A recalled memory.
#[derive(Debug, Clone, PartialEq)]
pub struct MemoryRecallHit {
    /// Stable id.
    pub id: String,
    /// Tier.
    pub tier: String,
    /// Content.
    pub content: String,
    /// Baseline importance.
    pub importance: f32,
    /// Tags.
    pub tags: Vec<String>,
}

/// Durable store behind the memory tools.
#[async_trait]
pub trait MemoryBackend: Send + Sync {
    /// Persist a memory, returning its id.
    async fn write(&self, draft: MemoryWriteDraft) -> Result<String>;

    /// Recall up to `limit` memories, optionally restricted to one tier,
    /// ranked most-relevant first (importance × recency × frequency).
    async fn recall(&self, tier: Option<String>, limit: usize) -> Result<Vec<MemoryRecallHit>>;
}

/// Delegate through a shared handle, so `Arc<dyn MemoryBackend>` (the type the
/// runtime wires) is itself a [`MemoryBackend`] and can back the tools.
#[async_trait]
impl<T: MemoryBackend + ?Sized> MemoryBackend for std::sync::Arc<T> {
    async fn write(&self, draft: MemoryWriteDraft) -> Result<String> {
        (**self).write(draft).await
    }

    async fn recall(&self, tier: Option<String>, limit: usize) -> Result<Vec<MemoryRecallHit>> {
        (**self).recall(tier, limit).await
    }
}

/// A backend reporting memory is unavailable (headless default).
pub struct UnavailableMemoryBackend;

#[async_trait]
impl MemoryBackend for UnavailableMemoryBackend {
    async fn write(&self, _draft: MemoryWriteDraft) -> Result<String> {
        Err(deepagent_core::error::CoreError::other(
            "long-term memory is not configured in this environment",
        ))
    }

    async fn recall(&self, _tier: Option<String>, _limit: usize) -> Result<Vec<MemoryRecallHit>> {
        Err(deepagent_core::error::CoreError::other(
            "long-term memory is not configured in this environment",
        ))
    }
}

fn tier_arg(args: &serde_json::Value) -> Option<String> {
    args.get("tier")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|tier| !tier.is_empty())
        .map(str::to_string)
}

fn tags_arg(args: &serde_json::Value) -> Vec<String> {
    args.get("tags")
        .and_then(serde_json::Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(|value| value.as_str())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// The `memory_recall` tool over a [`MemoryBackend`].
pub struct MemoryRecallTool<B: MemoryBackend> {
    backend: B,
}

impl<B: MemoryBackend> MemoryRecallTool<B> {
    /// Build the tool over `backend`.
    pub fn new(backend: B) -> Self {
        Self { backend }
    }
}

#[async_trait]
impl<B: MemoryBackend> Tool for MemoryRecallTool<B> {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: MEMORY_RECALL_TOOL_NAME.into(),
            description: "Recall long-term memories saved in earlier sessions — learned user \
                habits, prior task outcomes, and past failures to avoid repeating. Optionally \
                restrict to one tier. Results are ranked by importance and recency."
                .into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "tier": {
                        "type": "string",
                        "description": "Restrict recall to one tier; omit for all tiers.",
                        "enum": MEMORY_TIERS
                    },
                    "limit": {
                        "type": "number",
                        "description": "Max memories to return (default 8, max 50)."
                    }
                }
            }),
            risk: RiskLevel::Safe,
            required_permissions: PermissionSet::read_only(),
        }
    }

    async fn invoke(&self, args: serde_json::Value) -> Result<ToolOutput> {
        let tier = tier_arg(&args);
        let limit = args
            .get("limit")
            .and_then(serde_json::Value::as_u64)
            .map(|n| n.clamp(1, 50) as usize)
            .unwrap_or(8);
        match self.backend.recall(tier, limit).await {
            Ok(hits) => Ok(ToolOutput::success(serde_json::json!({
                "count": hits.len(),
                "memories": hits
                    .into_iter()
                    .map(|hit| serde_json::json!({
                        "id": hit.id,
                        "tier": hit.tier,
                        "content": hit.content,
                        "importance": hit.importance,
                        "tags": hit.tags,
                    }))
                    .collect::<Vec<_>>(),
            }))),
            Err(error) => Ok(ToolOutput::failure(format!(
                "memory_recall failed: {error}"
            ))),
        }
    }
}

/// The `memory_write` tool over a [`MemoryBackend`].
pub struct MemoryWriteTool<B: MemoryBackend> {
    backend: B,
}

impl<B: MemoryBackend> MemoryWriteTool<B> {
    /// Build the tool over `backend`.
    pub fn new(backend: B) -> Self {
        Self { backend }
    }
}

#[async_trait]
impl<B: MemoryBackend> Tool for MemoryWriteTool<B> {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: MEMORY_WRITE_TOOL_NAME.into(),
            description: "Save a durable memory that should survive across sessions: a learned \
                user habit/preference (procedural), a notable past task outcome (episodic), a \
                stable fact (semantic), project structure (workspace), or a failure to avoid \
                repeating (failure). Write a concise, self-contained note. For reusable \
                how-to/pitfall knowledge tied to the codebase, prefer knowledge_write instead."
                .into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "tier": {
                        "type": "string",
                        "description": "Which memory tier this belongs to.",
                        "enum": MEMORY_TIERS
                    },
                    "content": {
                        "type": "string",
                        "description": "The memory content (natural language, self-contained)."
                    },
                    "importance": {
                        "type": "number",
                        "description": "Baseline importance 0.0-1.0 (default 0.5)."
                    },
                    "tags": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "Optional tags for filtering."
                    }
                },
                "required": ["tier", "content"]
            }),
            risk: RiskLevel::Low,
            required_permissions: PermissionSet::from_iter_perms([Permission::WorkspaceWrite]),
        }
    }

    async fn invoke(&self, args: serde_json::Value) -> Result<ToolOutput> {
        let Some(tier) = tier_arg(&args) else {
            return Ok(ToolOutput::failure("missing 'tier'"));
        };
        if !MEMORY_TIERS.contains(&tier.as_str()) {
            return Ok(ToolOutput::failure(format!(
                "unknown tier '{tier}'; expected one of {MEMORY_TIERS:?}"
            )));
        }
        let Some(content) = args
            .get("content")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|content| !content.is_empty())
        else {
            return Ok(ToolOutput::failure("missing non-empty 'content'"));
        };
        let importance = args
            .get("importance")
            .and_then(serde_json::Value::as_f64)
            .map(|value| value as f32);
        let draft = MemoryWriteDraft {
            tier,
            content: content.to_string(),
            importance,
            tags: tags_arg(&args),
        };
        match self.backend.write(draft).await {
            Ok(id) => Ok(ToolOutput::success(
                serde_json::json!({ "id": id, "saved": true }),
            )),
            Err(error) => Ok(ToolOutput::failure(format!("memory_write failed: {error}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct StubBackend {
        written: Mutex<Vec<MemoryWriteDraft>>,
    }

    #[async_trait]
    impl MemoryBackend for StubBackend {
        async fn write(&self, draft: MemoryWriteDraft) -> Result<String> {
            self.written.lock().unwrap().push(draft);
            Ok("mem_1".to_string())
        }

        async fn recall(&self, tier: Option<String>, limit: usize) -> Result<Vec<MemoryRecallHit>> {
            Ok(vec![MemoryRecallHit {
                id: "mem_1".into(),
                tier: tier.unwrap_or_else(|| "episodic".into()),
                content: format!("recalled up to {limit}"),
                importance: 0.5,
                tags: vec![],
            }])
        }
    }

    #[tokio::test]
    async fn write_persists_and_returns_id() {
        let tool = MemoryWriteTool::new(StubBackend::default());
        let out = tool
            .invoke(serde_json::json!({
                "tier": "failure",
                "content": "npm build fails on node 22 without --openssl-legacy",
                "importance": 0.8,
                "tags": ["build", "node"]
            }))
            .await
            .unwrap();
        assert!(out.ok);
        assert_eq!(out.value["id"], "mem_1");
    }

    #[tokio::test]
    async fn write_rejects_unknown_tier() {
        let tool = MemoryWriteTool::new(StubBackend::default());
        let out = tool
            .invoke(serde_json::json!({ "tier": "bogus", "content": "x" }))
            .await
            .unwrap();
        assert!(!out.ok);
    }

    #[tokio::test]
    async fn write_requires_content() {
        let tool = MemoryWriteTool::new(StubBackend::default());
        let out = tool
            .invoke(serde_json::json!({ "tier": "semantic", "content": "  " }))
            .await
            .unwrap();
        assert!(!out.ok);
    }

    #[tokio::test]
    async fn recall_returns_memories() {
        let tool = MemoryRecallTool::new(StubBackend::default());
        let out = tool
            .invoke(serde_json::json!({ "tier": "procedural", "limit": 3 }))
            .await
            .unwrap();
        assert!(out.ok);
        assert_eq!(out.value["count"], 1);
        assert_eq!(out.value["memories"][0]["tier"], "procedural");
    }

    #[tokio::test]
    async fn unavailable_backend_reports_failure() {
        let write = MemoryWriteTool::new(UnavailableMemoryBackend);
        let out = write
            .invoke(serde_json::json!({ "tier": "semantic", "content": "x" }))
            .await
            .unwrap();
        assert!(!out.ok);
    }
}
