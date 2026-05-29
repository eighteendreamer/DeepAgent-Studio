//! In-memory memory store with ranked retrieval.
//!
//! This is the baseline backend implementing the store API. It performs a
//! simple keyword overlap match for candidate selection, then ranks by the
//! composite [`ranking`](crate::ranking) score. A `sqlite-vec` semantic backend
//! (Phase 5) implements the same conceptual API with real embeddings.

use std::collections::HashMap;

use deepagent_core::clock::Timestamp;

use crate::ranking::RankingParams;
use crate::{MemoryId, MemoryItem, MemoryTier};

/// A retrieval hit: the item plus the score it was ranked with.
#[derive(Debug, Clone)]
pub struct ScoredMemory {
    /// The matched item.
    pub item: MemoryItem,
    /// Composite ranking score.
    pub score: f32,
}

/// A simple in-process multi-tier memory store.
#[derive(Debug, Default)]
pub struct MemoryStore {
    items: HashMap<MemoryId, MemoryItem>,
    params: RankingParams,
}

impl MemoryStore {
    /// New empty store with default ranking params.
    pub fn new() -> Self {
        Self::default()
    }

    /// New store with custom ranking parameters.
    pub fn with_params(params: RankingParams) -> Self {
        Self {
            items: HashMap::new(),
            params,
        }
    }

    /// Number of stored memories.
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Whether the store is empty.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Insert (or replace) a memory item; returns its id.
    pub fn insert(&mut self, item: MemoryItem) -> MemoryId {
        let id = item.id;
        self.items.insert(id, item);
        id
    }

    /// Get a memory by id without recording an access.
    pub fn get(&self, id: MemoryId) -> Option<&MemoryItem> {
        self.items.get(&id)
    }

    /// Retrieve the top-`k` memories matching `query`, optionally restricted to
    /// a single tier. Matches are ranked by composite score; retrieved items
    /// have their recency/access stats bumped (`now`).
    pub fn retrieve(
        &mut self,
        query: &str,
        tier: Option<MemoryTier>,
        k: usize,
        now: Timestamp,
    ) -> Vec<ScoredMemory> {
        let query_terms = tokenize(query);

        // Score all candidates that match the tier filter and share >=1 term.
        let mut scored: Vec<(MemoryId, f32)> = self
            .items
            .values()
            .filter(|m| tier.map(|t| t == m.tier).unwrap_or(true))
            .filter_map(|m| {
                let overlap = term_overlap(&query_terms, &tokenize(&m.content));
                if overlap == 0 && !query_terms.is_empty() {
                    return None;
                }
                let base = self.params.score(m, now);
                // Boost by query term overlap so relevance matters most.
                let relevance = 1.0 + overlap as f32;
                Some((m.id, base * relevance))
            })
            .collect();

        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(k);

        // Materialize results and record the access.
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

    /// Remove a memory by id, returning it if present.
    pub fn remove(&mut self, id: MemoryId) -> Option<MemoryItem> {
        self.items.remove(&id)
    }
}

fn tokenize(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_lowercase())
        .collect()
}

fn term_overlap(a: &[String], b: &[String]) -> usize {
    a.iter().filter(|t| b.contains(t)).count()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(ms: i64) -> Timestamp {
        Timestamp::from_millis(ms)
    }

    #[test]
    fn insert_and_get() {
        let mut s = MemoryStore::new();
        let item = MemoryItem::new(MemoryTier::Semantic, "rust ownership", 0.8, at(0));
        let id = s.insert(item);
        assert_eq!(s.len(), 1);
        assert!(s.get(id).is_some());
    }

    #[test]
    fn retrieve_matches_by_keyword() {
        let mut s = MemoryStore::new();
        s.insert(MemoryItem::new(
            MemoryTier::Semantic,
            "payment retry timeout handling",
            0.9,
            at(0),
        ));
        s.insert(MemoryItem::new(
            MemoryTier::Semantic,
            "frontend button styling",
            0.9,
            at(0),
        ));

        let hits = s.retrieve("fix payment timeout", None, 5, at(1000));
        assert_eq!(hits.len(), 1);
        assert!(hits[0].item.content.contains("payment"));
    }

    #[test]
    fn tier_filter_restricts_results() {
        let mut s = MemoryStore::new();
        s.insert(MemoryItem::new(
            MemoryTier::Failure,
            "payment failed once",
            0.5,
            at(0),
        ));
        s.insert(MemoryItem::new(
            MemoryTier::Semantic,
            "payment api docs",
            0.5,
            at(0),
        ));

        let hits = s.retrieve("payment", Some(MemoryTier::Failure), 5, at(1000));
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].item.tier, MemoryTier::Failure);
    }

    #[test]
    fn retrieval_records_access() {
        let mut s = MemoryStore::new();
        let id = s.insert(MemoryItem::new(
            MemoryTier::Episodic,
            "did a thing",
            0.5,
            at(0),
        ));
        let _ = s.retrieve("thing", None, 5, at(500));
        assert_eq!(s.get(id).unwrap().access_count, 1);
        assert_eq!(s.get(id).unwrap().last_accessed.as_millis(), 500);
    }

    #[test]
    fn top_k_limits_results() {
        let mut s = MemoryStore::new();
        for i in 0..10 {
            s.insert(MemoryItem::new(
                MemoryTier::Semantic,
                format!("shared term item{i}"),
                0.5,
                at(0),
            ));
        }
        let hits = s.retrieve("shared", None, 3, at(100));
        assert_eq!(hits.len(), 3);
    }
}
