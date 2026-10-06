//! Fusion tuning for the crate's one retrieval track.
//!
//! This module used to also hold a complete `HybridRetriever` (dense + sparse +
//! a signal reranker) with its own tests. Nothing in the product used it: the
//! knowledge vault retrieves through [`crate::ContextualRetriever`], and a second
//! unwired answer to "what is relevant" in the same crate is how a system grows
//! two behaviours and one of them silently rots. It is gone rather than kept
//! behind a feature flag.

/// Tuning for the fused (dense + BM25) candidate stage.
#[derive(Debug, Clone, Copy)]
pub struct HybridConfig {
    /// RRF dampening constant (Cormack et al. use 60). Larger = flatter fusion.
    pub rrf_k: f32,
    /// How many candidates each retriever contributes before fusion.
    pub candidate_pool: usize,
}

impl Default for HybridConfig {
    fn default() -> Self {
        Self {
            rrf_k: 60.0,
            candidate_pool: 50,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_the_recipe_the_retriever_was_tuned_for() {
        let config = HybridConfig::default();
        assert_eq!(config.rrf_k, 60.0);
        assert_eq!(config.candidate_pool, 50);
    }
}
