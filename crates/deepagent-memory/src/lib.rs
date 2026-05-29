//! # deepagent-memory
//!
//! Multi-tier long-term memory (开发提示词.md §4 Layer 3, 开发计划.md Phase 5).
//!
//! Claude-mem-style memory is only the baseline; the blueprint calls for a
//! *Multi-Tier Memory System*:
//!
//! | Tier               | Purpose                |
//! |--------------------|------------------------|
//! | Semantic Memory    | knowledge              |
//! | Episodic Memory    | past tasks             |
//! | Procedural Memory  | user habits            |
//! | Workspace Memory   | project structure      |
//! | Failure Memory     | past failures          |
//!
//! This crate defines the [`MemoryTier`] taxonomy, the [`MemoryItem`] record,
//! the [`ranking`] model (importance / recency / decay), an in-memory keyword
//! [`store::MemoryStore`], a vector-based [`semantic::SemanticMemoryStore`]
//! (embeddings + cosine similarity), and a [`repository::MemoryRepository`] that
//! persists memory across sessions via the document store.

pub mod bm25;
pub mod chunking;
pub mod contextual_retrieval;
pub mod contextualize;
pub mod embedding;
pub mod fusion;
pub mod hybrid;
pub mod observation;
pub mod ranking;
pub mod repository;
pub mod semantic;
pub mod store;

pub use bm25::{Bm25Index, Bm25Params};
pub use chunking::{chunk_markdown, Chunk, ChunkConfig};
pub use contextual_retrieval::{ChunkReranker, ContextualRetriever, RetrievedChunk, ScoreReranker};
pub use contextualize::{
    contextualize_chunks, ContextualizedChunk, Contextualizer, HeadingContextualizer,
};
pub use embedding::{cosine_similarity, Embedder, HashingEmbedder};
pub use fusion::reciprocal_rank_fusion;
pub use hybrid::{to_l5_block, HybridConfig, HybridRetriever, Reranker, SignalReranker};
pub use observation::{Observation, ObservationType};
pub use repository::{MemoryRepository, MEMORY_COLLECTION};
pub use semantic::SemanticMemoryStore;

use serde::{Deserialize, Serialize};

use deepagent_core::clock::Timestamp;

/// The category of a memory, per the blueprint's multi-tier model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryTier {
    /// General knowledge / facts.
    Semantic,
    /// Records of past tasks / interactions.
    Episodic,
    /// Learned user habits & preferences.
    Procedural,
    /// Project / workspace structure.
    Workspace,
    /// Past failures (so they are not repeated).
    Failure,
}

/// A unique id for a memory item.
pub type MemoryId = deepagent_core::id::EventId;

/// A single stored memory.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MemoryItem {
    /// Unique id.
    pub id: MemoryId,
    /// Which tier this belongs to.
    pub tier: MemoryTier,
    /// The memory content (natural language).
    pub content: String,
    /// Baseline importance in `[0.0, 1.0]` assigned at write time.
    pub importance: f32,
    /// When the memory was created.
    pub created_at: Timestamp,
    /// When the memory was last accessed (drives recency/decay).
    pub last_accessed: Timestamp,
    /// How many times it has been retrieved.
    pub access_count: u32,
    /// Optional free-form tags for filtering.
    #[serde(default)]
    pub tags: Vec<String>,
}

impl MemoryItem {
    /// Create a new memory item at time `now`.
    pub fn new(
        tier: MemoryTier,
        content: impl Into<String>,
        importance: f32,
        now: Timestamp,
    ) -> Self {
        Self {
            id: MemoryId::new(),
            tier,
            content: content.into(),
            importance: importance.clamp(0.0, 1.0),
            created_at: now,
            last_accessed: now,
            access_count: 0,
            tags: Vec::new(),
        }
    }

    /// Attach tags (builder style).
    pub fn with_tags(mut self, tags: impl IntoIterator<Item = String>) -> Self {
        self.tags = tags.into_iter().collect();
        self
    }

    /// Record an access at `now` (bumps recency and access count).
    pub fn touch(&mut self, now: Timestamp) {
        self.last_accessed = now;
        self.access_count = self.access_count.saturating_add(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn importance_is_clamped() {
        let m = MemoryItem::new(MemoryTier::Semantic, "x", 5.0, Timestamp::from_millis(0));
        assert_eq!(m.importance, 1.0);
        let m2 = MemoryItem::new(MemoryTier::Semantic, "x", -1.0, Timestamp::from_millis(0));
        assert_eq!(m2.importance, 0.0);
    }

    #[test]
    fn touch_updates_recency() {
        let mut m = MemoryItem::new(MemoryTier::Episodic, "x", 0.5, Timestamp::from_millis(0));
        m.touch(Timestamp::from_millis(100));
        assert_eq!(m.access_count, 1);
        assert_eq!(m.last_accessed.as_millis(), 100);
    }
}
