//! Cross-session multi-tier memory persistence (D3).
//!
//! Backs the `memory_write` / `memory_recall` tools. Item-level memories
//! (`deepagent_memory::MemoryItem`) are persisted as JSON in the shared
//! [`DocumentStore`] under the `memory_items` collection and recalled by the
//! importance × recency × frequency ranking in `deepagent_memory::ranking`.
//!
//! Deliberately NOT a second semantic index: the knowledge-base design keeps the
//! chunk-level `ContextualRetriever` as the sole embeddings/BM25 retrieval track
//! (design.md:200). This layer only adds durable, tiered recall (habits, past
//! task outcomes, failures) that the in-memory tier stores previously lacked.

use std::sync::Arc;

use async_trait::async_trait;

use deepagent_builtins::{MemoryBackend, MemoryRecallHit, MemoryWriteDraft};
use deepagent_core::clock::{Clock, SystemClock};
use deepagent_core::error::{CoreError, Result};
use deepagent_memory::ranking::RankingParams;
use deepagent_memory::{MemoryItem, MemoryTier};
use deepagent_persistence::document_store::DocumentStore;
use deepagent_persistence::Database;

/// DocumentStore collection holding item-level memories.
const MEMORY_COLLECTION: &str = "memory_items";

/// Persists and recalls item-level multi-tier memories over the shared DB.
pub struct MemoryService {
    db: Arc<Database>,
}

impl MemoryService {
    /// Build the service over the shared database.
    pub fn new(db: Arc<Database>) -> Self {
        Self { db }
    }
}

fn parse_tier(tier: &str) -> Option<MemoryTier> {
    match tier {
        "semantic" => Some(MemoryTier::Semantic),
        "episodic" => Some(MemoryTier::Episodic),
        "procedural" => Some(MemoryTier::Procedural),
        "workspace" => Some(MemoryTier::Workspace),
        "failure" => Some(MemoryTier::Failure),
        _ => None,
    }
}

fn tier_str(tier: MemoryTier) -> &'static str {
    match tier {
        MemoryTier::Semantic => "semantic",
        MemoryTier::Episodic => "episodic",
        MemoryTier::Procedural => "procedural",
        MemoryTier::Workspace => "workspace",
        MemoryTier::Failure => "failure",
    }
}

#[async_trait]
impl MemoryBackend for MemoryService {
    async fn write(&self, draft: MemoryWriteDraft) -> Result<String> {
        let tier = parse_tier(&draft.tier)
            .ok_or_else(|| CoreError::invalid(format!("unknown memory tier '{}'", draft.tier)))?;
        let now = SystemClock.now();
        let mut item = MemoryItem::new(tier, draft.content, draft.importance.unwrap_or(0.5), now);
        if !draft.tags.is_empty() {
            item = item.with_tags(draft.tags);
        }
        let id = item.id.to_string();
        let body = serde_json::to_string(&item)
            .map_err(|error| CoreError::Serialization(format!("serialize memory: {error}")))?;
        DocumentStore::new(&self.db).put(MEMORY_COLLECTION, &id, &body, None, now)?;
        Ok(id)
    }

    async fn recall(&self, tier: Option<String>, limit: usize) -> Result<Vec<MemoryRecallHit>> {
        let want_tier = match tier.as_deref() {
            Some(tier) => Some(
                parse_tier(tier)
                    .ok_or_else(|| CoreError::invalid(format!("unknown memory tier '{tier}'")))?,
            ),
            None => None,
        };
        let docs = DocumentStore::new(&self.db).list(MEMORY_COLLECTION)?;
        let now = SystemClock.now();
        let params = RankingParams::default();
        let mut scored: Vec<(f32, MemoryItem)> = docs
            .iter()
            .filter_map(|doc| serde_json::from_str::<MemoryItem>(&doc.body).ok())
            .filter(|item| want_tier.map_or(true, |tier| item.tier == tier))
            .map(|item| (params.score(&item, now), item))
            .collect();
        scored.sort_by(|a, b| b.0.total_cmp(&a.0));
        scored.truncate(limit);
        Ok(scored
            .into_iter()
            .map(|(_, item)| MemoryRecallHit {
                id: item.id.to_string(),
                tier: tier_str(item.tier).to_string(),
                content: item.content,
                importance: item.importance,
                tags: item.tags,
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn service() -> MemoryService {
        let dir = tempfile::tempdir().unwrap();
        let db = Arc::new(Database::open(dir.path().join("t.db")).unwrap());
        // Keep the tempdir alive for the test process lifetime.
        std::mem::forget(dir);
        MemoryService::new(db)
    }

    #[tokio::test]
    async fn write_then_recall_roundtrips_across_the_store() {
        let svc = service();
        svc.write(MemoryWriteDraft {
            tier: "failure".into(),
            content: "cargo test hangs without --offline behind the proxy".into(),
            importance: Some(0.9),
            tags: vec!["ci".into()],
        })
        .await
        .unwrap();
        svc.write(MemoryWriteDraft {
            tier: "procedural".into(),
            content: "user prefers concise commit messages".into(),
            importance: Some(0.4),
            tags: vec![],
        })
        .await
        .unwrap();

        // All tiers, ranked: the higher-importance failure sorts first.
        let all = svc.recall(None, 10).await.unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].tier, "failure");

        // Tier filter restricts results.
        let procedural = svc.recall(Some("procedural".into()), 10).await.unwrap();
        assert_eq!(procedural.len(), 1);
        assert_eq!(procedural[0].tier, "procedural");
    }

    #[tokio::test]
    async fn write_rejects_unknown_tier() {
        let svc = service();
        let err = svc
            .write(MemoryWriteDraft {
                tier: "bogus".into(),
                content: "x".into(),
                importance: None,
                tags: vec![],
            })
            .await;
        assert!(err.is_err());
    }

    #[tokio::test]
    async fn recall_limit_is_respected() {
        let svc = service();
        for i in 0..5 {
            svc.write(MemoryWriteDraft {
                tier: "semantic".into(),
                content: format!("fact {i}"),
                importance: Some(0.5),
                tags: vec![],
            })
            .await
            .unwrap();
        }
        assert_eq!(svc.recall(None, 3).await.unwrap().len(), 3);
    }
}
