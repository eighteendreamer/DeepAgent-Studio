//! Cross-session multi-tier memory persistence (D3).
//!
//! Backs the `memory_write` / `memory_recall` tools. Memories are stored as
//! **knowledge-vault entries** (`kind: memory` + `tier` / `importance`
//! frontmatter), not as JSON rows in a private table.
//!
//! That convergence is deliberate. While memories lived in their own
//! `DocumentStore` collection, the retrieval track that injects `relevant
//! memories` into the model context (the chunk-level `ContextualRetriever`
//! over the vault) could never see them: `memory_write` wrote to a store only
//! `memory_recall` reads, so a remembered fact stayed invisible to every turn
//! unless the model happened to call the tool. One store makes a written
//! memory retrievable by the same path that already surfaces knowledge.
//!
//! Recall keeps the tiered ranking it had before (importance × recency ×
//! frequency from `deepagent_memory::ranking`); only the storage moved.

use std::sync::Arc;

use async_trait::async_trait;

use deepagent_builtins::{MemoryBackend, MemoryRecallHit, MemoryWriteDraft};
use deepagent_core::clock::{Clock, SystemClock, Timestamp};
use deepagent_core::error::{CoreError, Result};
use deepagent_memory::ranking::RankingParams;
use deepagent_memory::{MemoryItem, MemoryTier};

use crate::knowledge_service::{KnowledgeDraftDto, KnowledgeService};

/// Entry kind label every memory is written under.
const MEMORY_KIND: &str = "memory";
/// Default vault scope for a memory: memories are meant to survive across
/// projects, so they land in the user-global vault unless asked otherwise.
const DEFAULT_SCOPE: &str = "global";
/// Longest derived title; the vault dedups by normalized title, so a title cut
/// too short would silently merge unrelated memories.
const TITLE_MAX_CHARS: usize = 72;

fn parse_tier(tier: &str) -> Option<MemoryTier> {
    match tier.trim().to_lowercase().as_str() {
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

fn normalize_scope(scope: Option<&str>) -> Result<&'static str> {
    match scope.map(str::trim).filter(|s| !s.is_empty()) {
        None => Ok(DEFAULT_SCOPE),
        Some(s) if s.eq_ignore_ascii_case("global") => Ok("global"),
        Some(s) if s.eq_ignore_ascii_case("project") => Ok("project"),
        Some(s) => Err(CoreError::invalid(format!(
            "unknown memory scope '{s}' (expected 'global' or 'project')"
        ))),
    }
}

/// Derive a stable vault title from the memory content: its first non-empty
/// line, clamped to [`TITLE_MAX_CHARS`]. Stability matters — the vault updates
/// an entry in place when the normalized title matches, so re-writing the same
/// memory does not pile up duplicates. Shared with the one-release migration so
/// both write the same title for the same content.
pub(crate) fn memory_title(content: &str) -> String {
    let first_line = content
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("")
        .trim_end_matches(['。', '！', '？', '，', '；', '.', '!', '?', ',', ';']);
    let mut title: String = first_line.chars().take(TITLE_MAX_CHARS).collect();
    if title.is_empty() {
        title = "memory".to_string();
    }
    title
}

/// Persists and recalls item-level memories through the knowledge vault.
pub struct MemoryService {
    knowledge: Arc<KnowledgeService>,
}

impl MemoryService {
    /// Build over the shared knowledge service (one store, one retrieval track).
    pub fn new(knowledge: Arc<KnowledgeService>) -> Self {
        Self { knowledge }
    }
}

#[async_trait]
impl MemoryBackend for MemoryService {
    async fn write(&self, draft: MemoryWriteDraft) -> Result<String> {
        let tier = parse_tier(&draft.tier)
            .ok_or_else(|| CoreError::invalid(format!("unknown memory tier '{}'", draft.tier)))?;
        let scope = normalize_scope(draft.scope.as_deref())?;
        if draft.content.trim().is_empty() {
            return Err(CoreError::invalid("memory content is empty"));
        }
        let dto = self.knowledge.save(KnowledgeDraftDto {
            title: memory_title(&draft.content),
            body: draft.content.clone(),
            kind: Some(MEMORY_KIND.to_string()),
            tags: draft.tags.clone(),
            scope: Some(scope.to_string()),
            source_session: None,
            tier: Some(tier_str(tier).to_string()),
            importance: draft.importance,
        })?;
        Ok(dto.id)
    }

    async fn recall(&self, tier: Option<String>, limit: usize) -> Result<Vec<MemoryRecallHit>> {
        let want_tier = match tier.as_deref() {
            Some(tier) => Some(
                parse_tier(tier)
                    .ok_or_else(|| CoreError::invalid(format!("unknown memory tier '{tier}'")))?,
            ),
            None => None,
        };
        let now = SystemClock.now();
        let params = RankingParams::default();
        let mut scored: Vec<(f32, MemoryRecallHit)> = self
            .knowledge
            .list()
            .into_iter()
            .filter(|entry| entry.kind == MEMORY_KIND)
            .filter(|entry| {
                entry.tier.as_deref().map_or(true, |tier| {
                    want_tier.map_or(true, |want| tier == tier_str(want))
                })
            })
            .map(|entry| {
                let item = MemoryItem::new(
                    want_tier.unwrap_or(MemoryTier::Semantic),
                    entry.body.clone(),
                    entry.importance.unwrap_or(0.5),
                    Timestamp::from_millis(entry.updated_at.max(entry.created_at)),
                )
                .with_tags(entry.tags.clone());
                let score = params.score(&item, now);
                (
                    score,
                    MemoryRecallHit {
                        id: entry.id.clone(),
                        tier: entry
                            .tier
                            .clone()
                            .unwrap_or_else(|| tier_str(item.tier).to_string()),
                        content: entry.body.clone(),
                        importance: entry.importance.unwrap_or(0.5),
                        tags: entry.tags.clone(),
                    },
                )
            })
            .collect();
        scored.sort_by(|a, b| b.0.total_cmp(&a.0));
        scored.truncate(limit);
        Ok(scored.into_iter().map(|(_, hit)| hit).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::knowledge_service::KnowledgeService;

    fn service() -> Arc<MemoryService> {
        let project = std::env::temp_dir().join(format!("mem-proj-{}", uuid::Uuid::new_v4()));
        let global = std::env::temp_dir().join(format!("mem-glob-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&project).unwrap();
        std::fs::create_dir_all(&global).unwrap();
        // Keep the tempdirs alive for the test process lifetime.
        std::mem::forget((project.clone(), global.clone()));
        let knowledge =
            Arc::new(KnowledgeService::open(&project, &global).expect("open knowledge service"));
        Arc::new(MemoryService::new(knowledge))
    }

    fn draft(tier: &str, content: &str, importance: Option<f32>) -> MemoryWriteDraft {
        MemoryWriteDraft {
            tier: tier.to_string(),
            content: content.to_string(),
            importance,
            tags: vec![],
            scope: None,
        }
    }

    #[tokio::test]
    async fn write_then_recall_roundtrips_through_the_vault() {
        let svc = service();
        svc.write(draft(
            "failure",
            "cargo test hangs without --offline behind the proxy",
            Some(0.9),
        ))
        .await
        .unwrap();
        svc.write(draft(
            "procedural",
            "user prefers concise commit messages",
            Some(0.4),
        ))
        .await
        .unwrap();

        let all = svc.recall(None, 10).await.unwrap();
        assert_eq!(all.len(), 2);
        // Importance sorts first: the 0.9 failure outranks the 0.4 procedure.
        assert_eq!(all[0].tier, "failure");

        let procedural = svc.recall(Some("procedural".into()), 10).await.unwrap();
        assert_eq!(procedural.len(), 1);
        assert_eq!(procedural[0].tier, "procedural");
    }

    #[tokio::test]
    async fn write_rejects_unknown_tier_and_scope() {
        let svc = service();
        assert!(svc.write(draft("bogus", "x", None)).await.is_err());
        let err = svc
            .write(MemoryWriteDraft {
                tier: "semantic".into(),
                content: "x".into(),
                importance: None,
                tags: vec![],
                scope: Some("elsewhere".into()),
            })
            .await;
        assert!(err.is_err(), "unknown scope must not be silently coerced");
    }

    #[tokio::test]
    async fn recall_limit_is_respected() {
        let svc = service();
        for i in 0..5 {
            svc.write(draft("semantic", &format!("fact {i}"), Some(0.5)))
                .await
                .unwrap();
        }
        assert_eq!(svc.recall(None, 3).await.unwrap().len(), 3);
    }

    #[tokio::test]
    async fn written_memory_is_visible_to_the_knowledge_retrieval_track() {
        // The regression this convergence exists to fix: a memory written by
        // `memory_write` must be retrievable by the same search the background
        // `relevant memories` prefetch uses, not only by `memory_recall`.
        let knowledge = {
            let project = std::env::temp_dir().join(format!("mem-x-proj-{}", uuid::Uuid::new_v4()));
            let global = std::env::temp_dir().join(format!("mem-x-glob-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&project).unwrap();
            std::fs::create_dir_all(&global).unwrap();
            std::mem::forget((project.clone(), global.clone()));
            Arc::new(KnowledgeService::open(&project, &global).unwrap())
        };
        let svc = Arc::new(MemoryService::new(knowledge.clone()));
        svc.write(draft(
            "failure",
            "pnpm exec tsc reports a fake green when invoked through pnpm",
            Some(0.8),
        ))
        .await
        .unwrap();

        let hits = knowledge.search("fake green tsc", None, 5);
        assert!(
            hits.iter().any(|hit| hit.kind == MEMORY_KIND),
            "memory must surface through knowledge search, got {hits:?}"
        );
    }

    #[tokio::test]
    async fn rewriting_the_same_memory_updates_one_entry() {
        let svc = service();
        let first = svc
            .write(draft(
                "semantic",
                "the build needs -j1 on this machine",
                Some(0.6),
            ))
            .await
            .unwrap();
        let second = svc
            .write(draft(
                "semantic",
                "the build needs -j1 on this machine",
                Some(0.9),
            ))
            .await
            .unwrap();
        assert_eq!(first, second, "same title must reuse the vault entry");
        assert_eq!(
            svc.recall(Some("semantic".into()), 10).await.unwrap().len(),
            1
        );
    }
}
