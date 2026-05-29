//! Semantic memory store (开发计划.md Phase 5).
//!
//! Combines vector similarity ([`Embedder`] + [`cosine_similarity`]) with the
//! importance/recency/decay [`ranking`](crate::ranking) model to retrieve
//! memories by *meaning* rather than exact keyword overlap. Each item's
//! embedding is computed on insert and cached alongside it.
//!
//! The final retrieval score blends semantic similarity with the composite
//! ranking score, so a highly-relevant-but-stale memory and a
//! marginally-relevant-but-fresh one are ordered sensibly.

use std::collections::HashMap;

use deepagent_core::clock::Timestamp;

use crate::embedding::{cosine_similarity, Embedder};
use crate::ranking::RankingParams;
use crate::store::ScoredMemory;
use crate::{MemoryId, MemoryItem, MemoryTier};

/// An in-memory semantic store. Holds items plus their cached embeddings.
pub struct SemanticMemoryStore<E: Embedder> {
    embedder: E,
    params: RankingParams,
    /// How strongly semantic similarity weighs vs. the ranking score, in [0,1].
    /// 1.0 = pure similarity; 0.0 = pure importance/recency.
    similarity_weight: f32,
    items: HashMap<MemoryId, MemoryItem>,
    embeddings: HashMap<MemoryId, Vec<f32>>,
}

impl<E: Embedder> SemanticMemoryStore<E> {
    /// Build with an embedder and default ranking params.
    pub fn new(embedder: E) -> Self {
        Self {
            embedder,
            params: RankingParams::default(),
            similarity_weight: 0.7,
            items: HashMap::new(),
            embeddings: HashMap::new(),
        }
    }

    /// Set the similarity vs. ranking blend weight (clamped to [0,1]).
    pub fn with_similarity_weight(mut self, w: f32) -> Self {
        self.similarity_weight = w.clamp(0.0, 1.0);
        self
    }

    /// Number of stored memories.
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Whether the store is empty.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Insert an item, computing and caching its embedding.
    pub fn insert(&mut self, item: MemoryItem) -> MemoryId {
        let id = item.id;
        let embedding = self.embedder.embed(&item.content);
        self.embeddings.insert(id, embedding);
        self.items.insert(id, item);
        id
    }

    /// Insert an item with a precomputed embedding (e.g. loaded from disk).
    pub fn insert_with_embedding(&mut self, item: MemoryItem, embedding: Vec<f32>) -> MemoryId {
        let id = item.id;
        self.embeddings.insert(id, embedding);
        self.items.insert(id, item);
        id
    }

    /// Get the cached embedding for an item.
    pub fn embedding_of(&self, id: MemoryId) -> Option<&[f32]> {
        self.embeddings.get(&id).map(|v| v.as_slice())
    }

    /// Get an item by id.
    pub fn get(&self, id: MemoryId) -> Option<&MemoryItem> {
        self.items.get(&id)
    }

    /// Semantically retrieve the top-`k` memories for `query`, optionally
    /// restricted to a tier. Retrieved items have their recency/access bumped.
    pub fn retrieve(
        &mut self,
        query: &str,
        tier: Option<MemoryTier>,
        k: usize,
        now: Timestamp,
    ) -> Vec<ScoredMemory> {
        if k == 0 {
            return Vec::new();
        }
        let query_vec = self.embedder.embed(query);

        let mut scored: Vec<(MemoryId, f32)> = self
            .items
            .values()
            .filter(|m| tier.map(|t| t == m.tier).unwrap_or(true))
            .filter_map(|m| {
                let emb = self.embeddings.get(&m.id)?;
                let sim = cosine_similarity(&query_vec, emb).max(0.0);
                // Drop clearly-irrelevant matches.
                if sim < 1e-4 {
                    return None;
                }
                let rank = self.params.score(m, now);
                let blended = self.similarity_weight * sim + (1.0 - self.similarity_weight) * rank;
                Some((m.id, blended))
            })
            .collect();

        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(k);

        let mut out = Vec::with_capacity(scored.len());
        for (id, score) in scored {
            if let Some(item) = self.items.get_mut(&id) {
                item.touch(now);
                out.push(ScoredMemory {
                    item: item.clone(),
                    score,
                });
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embedding::HashingEmbedder;

    fn at(ms: i64) -> Timestamp {
        Timestamp::from_millis(ms)
    }

    fn store() -> SemanticMemoryStore<HashingEmbedder> {
        SemanticMemoryStore::new(HashingEmbedder::default())
    }

    #[test]
    fn retrieves_semantically_closest() {
        let mut s = store();
        s.insert(MemoryItem::new(
            MemoryTier::Semantic,
            "the payment service retries on timeout",
            0.5,
            at(0),
        ));
        s.insert(MemoryItem::new(
            MemoryTier::Semantic,
            "the UI renders a blue button",
            0.5,
            at(0),
        ));

        let hits = s.retrieve("payment timeout handling", None, 1, at(1000));
        assert_eq!(hits.len(), 1);
        assert!(hits[0].item.content.contains("payment"));
    }

    #[test]
    fn tier_filter_applies() {
        let mut s = store();
        s.insert(MemoryItem::new(
            MemoryTier::Failure,
            "payment broke once",
            0.5,
            at(0),
        ));
        s.insert(MemoryItem::new(
            MemoryTier::Semantic,
            "payment docs",
            0.5,
            at(0),
        ));
        let hits = s.retrieve("payment", Some(MemoryTier::Failure), 5, at(1));
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].item.tier, MemoryTier::Failure);
    }

    #[test]
    fn retrieval_bumps_access() {
        let mut s = store();
        let id = s.insert(MemoryItem::new(
            MemoryTier::Episodic,
            "did the thing",
            0.5,
            at(0),
        ));
        let _ = s.retrieve("thing", None, 5, at(500));
        assert_eq!(s.get(id).unwrap().access_count, 1);
    }

    #[test]
    fn top_k_limits_results() {
        let mut s = store();
        for i in 0..10 {
            s.insert(MemoryItem::new(
                MemoryTier::Semantic,
                format!("shared concept item {i}"),
                0.5,
                at(0),
            ));
        }
        let hits = s.retrieve("shared concept", None, 3, at(1));
        assert_eq!(hits.len(), 3);
    }

    #[test]
    fn insert_with_embedding_is_used() {
        let mut s = store();
        let item = MemoryItem::new(MemoryTier::Semantic, "content", 0.5, at(0));
        let id = item.id;
        s.insert_with_embedding(item, vec![1.0; HashingEmbedder::default().dimensions()]);
        assert!(s.embedding_of(id).is_some());
    }
}
