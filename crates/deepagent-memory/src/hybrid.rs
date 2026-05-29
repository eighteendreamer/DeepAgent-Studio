//! Hybrid retrieval: **embedding + BM25 + rerank** (Anthropic's Contextual
//! Retrieval recipe; claude-mem's dense-vector + FTS5/BM25 fusion).
//!
//! Pipeline (开发计划.md Phase 5, aligned with claude-mem):
//!
//! ```text
//! query
//!   ├── dense:  embed(query) · cosine vs each item    -> ranked list A
//!   └── sparse: BM25(query) over item markdown        -> ranked list B
//!   fuse(A, B) via Reciprocal Rank Fusion             -> candidate set
//!   rerank(candidates) by composite signal            -> final top-k
//! ```
//!
//! **Why fuse?** Dense embeddings capture paraphrase/semantics; BM25 nails exact
//! keywords and identifiers. Reciprocal Rank Fusion (RRF) combines two ranked
//! lists without needing comparable score scales — it sums `1/(k + rank)` across
//! lists, which is robust and parameter-light.
//!
//! **Rerank** then re-orders the fused candidates with a richer signal that
//! folds in the memory's importance/recency/decay and a concept-overlap boost —
//! the lightweight stand-in for a cross-encoder reranker (which would implement
//! the [`Reranker`] trait).

use std::collections::HashMap;

use deepagent_core::clock::Timestamp;

use crate::bm25::{tokenize, Bm25Index};
use crate::embedding::{cosine_similarity, Embedder};
use crate::fusion::reciprocal_rank_fusion;
use crate::ranking::RankingParams;
use crate::store::ScoredMemory;
use crate::{MemoryId, MemoryItem, MemoryTier};

/// Tuning for the hybrid retriever.
#[derive(Debug, Clone, Copy)]
pub struct HybridConfig {
    /// RRF dampening constant (Cormack et al. use 60). Larger = flatter fusion.
    pub rrf_k: f32,
    /// How many candidates each retriever contributes before fusion.
    pub candidate_pool: usize,
    /// Weight of the fused retrieval score in the final rerank, in [0,1].
    /// The remainder weights the memory ranking (importance/recency/decay).
    pub retrieval_weight: f32,
    /// Extra additive boost per overlapping concept tag during rerank.
    pub concept_boost: f32,
}

impl Default for HybridConfig {
    fn default() -> Self {
        Self {
            rrf_k: 60.0,
            candidate_pool: 50,
            retrieval_weight: 0.75,
            concept_boost: 0.05,
        }
    }
}

/// A reranking stage applied to fused candidates.
///
/// The default [`SignalReranker`] uses memory ranking + concept overlap. A
/// model-backed cross-encoder reranker implements this same trait.
pub trait Reranker {
    /// Re-score `candidates` for `query`; return them re-ordered, best first.
    /// Each candidate carries its fused retrieval score in `[0,1]`.
    fn rerank(
        &self,
        query: &str,
        candidates: Vec<RerankCandidate>,
        now: Timestamp,
    ) -> Vec<ScoredMemory>;
}

/// A candidate entering the rerank stage.
#[derive(Debug, Clone)]
pub struct RerankCandidate {
    /// The memory item.
    pub item: MemoryItem,
    /// Normalized fused retrieval score in `[0,1]` (1 = top of fused list).
    pub fused_score: f32,
}

/// Render retrieved memories as the Context Pipeline's **Layer 5 (Semantic
/// Retrieval)** block — markdown ready to inject into a prompt. Empty input
/// yields an empty string (so the pipeline omits the layer).
pub fn to_l5_block(hits: &[ScoredMemory]) -> String {
    if hits.is_empty() {
        return String::new();
    }
    let mut out = String::from("# Relevant memory (retrieved)\n");
    for hit in hits {
        // The item content is already markdown (from Observation::to_markdown);
        // indent it under a bullet for the retrieval section.
        out.push_str(&format!("\n{}\n", hit.item.content));
    }
    out.trim_end().to_string()
}

/// The default reranker: blends fused retrieval score with the memory's
/// importance/recency/decay ranking and a concept-overlap boost.
#[derive(Debug, Clone, Copy)]
pub struct SignalReranker {
    params: RankingParams,
    retrieval_weight: f32,
    concept_boost: f32,
}

impl SignalReranker {
    /// Build from config.
    pub fn new(config: &HybridConfig) -> Self {
        Self {
            params: RankingParams::default(),
            retrieval_weight: config.retrieval_weight.clamp(0.0, 1.0),
            concept_boost: config.concept_boost,
        }
    }
}

impl Reranker for SignalReranker {
    fn rerank(
        &self,
        query: &str,
        candidates: Vec<RerankCandidate>,
        now: Timestamp,
    ) -> Vec<ScoredMemory> {
        let query_terms: Vec<String> = tokenize(query);
        let mut scored: Vec<ScoredMemory> = candidates
            .into_iter()
            .map(|c| {
                let ranking = self.params.score(&c.item, now);
                let concept_hits = c
                    .item
                    .tags
                    .iter()
                    .filter(|t| query_terms.iter().any(|q| q == &t.to_lowercase()))
                    .count() as f32;
                let score = self.retrieval_weight * c.fused_score
                    + (1.0 - self.retrieval_weight) * ranking
                    + self.concept_boost * concept_hits;
                ScoredMemory {
                    item: c.item,
                    score,
                }
            })
            .collect();
        scored.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        scored
    }
}

/// The hybrid retriever: holds items, their embeddings, and a BM25 index, and
/// runs the embedding + BM25 + rerank pipeline.
pub struct HybridRetriever<E: Embedder, R: Reranker> {
    embedder: E,
    reranker: R,
    config: HybridConfig,
    items: HashMap<MemoryId, MemoryItem>,
    embeddings: HashMap<MemoryId, Vec<f32>>,
    bm25: Bm25Index<MemoryId>,
}

impl<E: Embedder> HybridRetriever<E, SignalReranker> {
    /// Build with the default signal reranker.
    pub fn new(embedder: E) -> Self {
        let config = HybridConfig::default();
        let reranker = SignalReranker::new(&config);
        Self::with_config(embedder, reranker, config)
    }
}

impl<E: Embedder, R: Reranker> HybridRetriever<E, R> {
    /// Build with a custom reranker and config.
    pub fn with_config(embedder: E, reranker: R, config: HybridConfig) -> Self {
        Self {
            embedder,
            reranker,
            config,
            items: HashMap::new(),
            embeddings: HashMap::new(),
            bm25: Bm25Index::default(),
        }
    }

    /// Number of indexed memories.
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Whether empty.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Index a memory item across both dense and sparse indexes. The item's
    /// `content` (markdown) is what gets embedded and BM25-indexed.
    pub fn insert(&mut self, item: MemoryItem) -> MemoryId {
        let id = item.id;
        let embedding = self.embedder.embed(&item.content);
        self.bm25.add(id, &item.content);
        self.embeddings.insert(id, embedding);
        self.items.insert(id, item);
        id
    }

    /// Index with a precomputed embedding (e.g. loaded from persistence).
    pub fn insert_with_embedding(&mut self, item: MemoryItem, embedding: Vec<f32>) -> MemoryId {
        let id = item.id;
        self.bm25.add(id, &item.content);
        self.embeddings.insert(id, embedding);
        self.items.insert(id, item);
        id
    }

    /// Get an item by id.
    pub fn get(&self, id: MemoryId) -> Option<&MemoryItem> {
        self.items.get(&id)
    }

    /// Run the full hybrid pipeline and return the top-`k` reranked memories.
    /// Optionally restrict to a single [`MemoryTier`]. Retrieved items have
    /// their recency/access stats bumped (`now`).
    pub fn retrieve(
        &mut self,
        query: &str,
        tier: Option<MemoryTier>,
        k: usize,
        now: Timestamp,
    ) -> Vec<ScoredMemory> {
        if k == 0 || self.items.is_empty() {
            return Vec::new();
        }
        let pool = self.config.candidate_pool;

        // --- Dense retrieval (embeddings) ---------------------------------
        let query_vec = self.embedder.embed(query);
        let mut dense: Vec<(MemoryId, f32)> = self
            .items
            .values()
            .filter(|m| tier.map(|t| t == m.tier).unwrap_or(true))
            .filter_map(|m| {
                let emb = self.embeddings.get(&m.id)?;
                let sim = cosine_similarity(&query_vec, emb);
                if sim > 1e-4 {
                    Some((m.id, sim))
                } else {
                    None
                }
            })
            .collect();
        dense.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        dense.truncate(pool);

        // --- Sparse retrieval (BM25) --------------------------------------
        let sparse_hits = self.bm25.search(query, pool * 2);
        let sparse: Vec<MemoryId> = sparse_hits
            .into_iter()
            .filter(|h| {
                tier.map(|t| self.items.get(&h.id).map(|m| m.tier) == Some(t))
                    .unwrap_or(true)
            })
            .map(|h| h.id)
            .take(pool)
            .collect();

        // --- Reciprocal Rank Fusion ---------------------------------------
        let dense_ids: Vec<MemoryId> = dense.iter().map(|(id, _)| *id).collect();
        let fused = reciprocal_rank_fusion(&dense_ids, &sparse, self.config.rrf_k, |a, b| {
            a.to_string().cmp(&b.to_string())
        });
        if fused.is_empty() {
            return Vec::new();
        }

        // Normalize fused scores to [0,1] for the reranker.
        let max_fused = fused.first().map(|(_, s)| *s).unwrap_or(1.0).max(1e-6);
        let candidates: Vec<RerankCandidate> = fused
            .into_iter()
            .filter_map(|(id, s)| {
                self.items.get(&id).map(|item| RerankCandidate {
                    item: item.clone(),
                    fused_score: s / max_fused,
                })
            })
            .collect();

        // --- Rerank -------------------------------------------------------
        let mut ranked = self.reranker.rerank(query, candidates, now);
        ranked.truncate(k);

        // Record access on the returned items.
        for hit in &ranked {
            if let Some(item) = self.items.get_mut(&hit.item.id) {
                item.touch(now);
            }
        }
        ranked
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embedding::HashingEmbedder;
    use crate::observation::{Observation, ObservationType};

    fn at(ms: i64) -> Timestamp {
        Timestamp::from_millis(ms)
    }

    fn retriever() -> HybridRetriever<HashingEmbedder, SignalReranker> {
        HybridRetriever::new(HashingEmbedder::default())
    }

    fn obs_item(t: ObservationType, title: &str, narrative: &str, concepts: &[&str]) -> MemoryItem {
        Observation::new(t, title)
            .narrative(narrative)
            .concepts(concepts.iter().map(|s| s.to_string()))
            .into_memory_item(at(0))
    }

    #[test]
    fn fuses_dense_and_sparse_to_find_relevant() {
        let mut r = retriever();
        r.insert(obs_item(
            ObservationType::BugFix,
            "Payment timeout fix",
            "the payment service retries on timeout with backoff",
            &["payment", "timeout"],
        ));
        r.insert(obs_item(
            ObservationType::Feature,
            "Dashboard charts",
            "render charts and graphs on the dashboard",
            &["ui", "charts"],
        ));

        let hits = r.retrieve("how do we handle payment timeouts", None, 1, at(1000));
        assert_eq!(hits.len(), 1);
        assert!(hits[0].item.content.to_lowercase().contains("payment"));
    }

    #[test]
    fn bm25_catches_exact_identifier_embeddings_might_blur() {
        let mut r = retriever();
        // An exact rare token ("X9Z_TOKEN") is BM25's strength.
        r.insert(obs_item(
            ObservationType::Knowledge,
            "Config key X9Z_TOKEN controls retries",
            "the special configuration key governs retry counts",
            &["config"],
        ));
        r.insert(obs_item(
            ObservationType::Knowledge,
            "General retry guidance",
            "retries should use exponential backoff",
            &["retry"],
        ));
        let hits = r.retrieve("X9Z_TOKEN", None, 1, at(1000));
        assert_eq!(hits.len(), 1);
        assert!(hits[0].item.content.contains("X9Z_TOKEN"));
    }

    #[test]
    fn tier_filter_restricts() {
        let mut r = retriever();
        r.insert(obs_item(
            ObservationType::Failure,
            "payment broke",
            "payment outage",
            &[],
        ));
        r.insert(obs_item(
            ObservationType::Knowledge,
            "payment docs",
            "payment overview",
            &[],
        ));
        let hits = r.retrieve("payment", Some(MemoryTier::Failure), 5, at(1));
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].item.tier, MemoryTier::Failure);
    }

    #[test]
    fn concept_overlap_boosts_rerank() {
        let mut r = retriever();
        // Two items with similar text; one has the matching concept tag.
        r.insert(obs_item(
            ObservationType::Knowledge,
            "retry behavior notes one",
            "retry behavior and policies",
            &["payment"],
        ));
        r.insert(obs_item(
            ObservationType::Knowledge,
            "retry behavior notes two",
            "retry behavior and policies",
            &["unrelated"],
        ));
        let hits = r.retrieve("payment retry behavior", None, 2, at(1));
        assert_eq!(hits.len(), 2);
        // The payment-tagged item should rank first due to concept boost.
        assert!(hits[0].item.tags.contains(&"payment".to_string()));
    }

    #[test]
    fn top_k_and_access_recording() {
        let mut r = retriever();
        let mut ids = Vec::new();
        for i in 0..6 {
            let item = obs_item(
                ObservationType::Knowledge,
                &format!("shared concept item {i}"),
                "shared concept body text",
                &[],
            );
            ids.push(r.insert(item));
        }
        let hits = r.retrieve("shared concept", None, 3, at(500));
        assert_eq!(hits.len(), 3);
        // Returned items had their access recorded.
        for hit in &hits {
            assert_eq!(r.get(hit.item.id).unwrap().access_count, 1);
        }
    }

    #[test]
    fn empty_query_returns_nothing() {
        let mut r = retriever();
        r.insert(obs_item(ObservationType::Knowledge, "x", "y", &[]));
        assert!(r.retrieve("", None, 5, at(1)).is_empty());
    }
}
