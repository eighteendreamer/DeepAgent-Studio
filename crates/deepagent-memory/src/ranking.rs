//! Memory ranking (开发计划.md Phase 5 §3): importance, recency, decay.
//!
//! Retrieval ranks candidate memories by a composite score that blends:
//! - **importance** — the static value assigned at write time,
//! - **recency** — how recently the memory was accessed,
//! - **frequency** — how often it has been accessed,
//!
//! all subject to exponential **decay** over time so stale memories naturally
//! fall out of the working set.

use deepagent_core::clock::Timestamp;

use crate::MemoryItem;

/// Tunable weights / half-life for the ranking function.
#[derive(Debug, Clone, Copy)]
pub struct RankingParams {
    /// Weight of the static importance term.
    pub importance_weight: f32,
    /// Weight of the recency (decay) term.
    pub recency_weight: f32,
    /// Weight of the access-frequency term.
    pub frequency_weight: f32,
    /// Half-life (in milliseconds) for recency decay. After this much idle
    /// time, the recency contribution halves.
    pub half_life_ms: f64,
}

impl Default for RankingParams {
    fn default() -> Self {
        Self {
            importance_weight: 0.5,
            recency_weight: 0.35,
            frequency_weight: 0.15,
            // ~7 days half-life by default.
            half_life_ms: 7.0 * 24.0 * 60.0 * 60.0 * 1000.0,
        }
    }
}

impl RankingParams {
    /// Exponential decay factor in `[0.0, 1.0]` for an item last accessed at
    /// `last_accessed`, evaluated at `now`. 1.0 = just accessed.
    pub fn decay(&self, last_accessed: Timestamp, now: Timestamp) -> f32 {
        let elapsed = now.millis_since(last_accessed).max(0) as f64;
        // 0.5 ^ (elapsed / half_life)
        let factor = 0.5_f64.powf(elapsed / self.half_life_ms);
        factor as f32
    }

    /// Composite score for `item` at time `now`. Higher = more relevant.
    pub fn score(&self, item: &MemoryItem, now: Timestamp) -> f32 {
        let recency = self.decay(item.last_accessed, now);
        // Diminishing-returns frequency term: count / (count + 1) in [0,1).
        let freq = item.access_count as f32 / (item.access_count as f32 + 1.0);
        self.importance_weight * item.importance
            + self.recency_weight * recency
            + self.frequency_weight * freq
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MemoryTier;

    #[test]
    fn fresh_memory_has_full_recency() {
        let p = RankingParams::default();
        let t = Timestamp::from_millis(1000);
        assert!((p.decay(t, t) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn decay_halves_at_half_life() {
        let p = RankingParams::default();
        let start = Timestamp::from_millis(0);
        let later = Timestamp::from_millis(p.half_life_ms as i64);
        let d = p.decay(start, later);
        assert!((d - 0.5).abs() < 0.01, "expected ~0.5, got {d}");
    }

    #[test]
    fn higher_importance_scores_higher() {
        let p = RankingParams::default();
        let now = Timestamp::from_millis(1000);
        let low = MemoryItem::new(MemoryTier::Semantic, "a", 0.1, now);
        let high = MemoryItem::new(MemoryTier::Semantic, "b", 0.9, now);
        assert!(p.score(&high, now) > p.score(&low, now));
    }

    #[test]
    fn stale_memory_scores_lower_than_fresh() {
        let p = RankingParams::default();
        let created = Timestamp::from_millis(0);
        let fresh = MemoryItem::new(MemoryTier::Episodic, "fresh", 0.5, created);

        let mut stale = MemoryItem::new(MemoryTier::Episodic, "stale", 0.5, created);
        // not accessed for 30 days
        let now = Timestamp::from_millis(30 * 24 * 60 * 60 * 1000);
        // fresh accessed "now"
        let mut fresh = fresh;
        fresh.touch(now);
        let _ = &mut stale; // stale keeps old last_accessed
        assert!(p.score(&fresh, now) > p.score(&stale, now));
    }
}
