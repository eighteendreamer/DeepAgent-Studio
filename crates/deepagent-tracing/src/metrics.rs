//! Lightweight in-process metrics registry.
//!
//! These power the Agent Timeline panel (开发提示词.md §18): token usage,
//! cache hits, tool latency, retries, cost. Kept deliberately simple (atomic
//! counters + a snapshot) so the runtime can record metrics on the hot path
//! without locks, and the UI can poll a consistent snapshot.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

/// A cloneable handle to the metrics registry. Cloning shares the same
/// underlying counters (via [`Arc`]).
#[derive(Clone, Default)]
pub struct Metrics {
    inner: Arc<MetricsInner>,
}

#[derive(Default)]
struct MetricsInner {
    counters: Mutex<BTreeMap<String, Arc<AtomicU64>>>,
}

impl Metrics {
    /// Create a fresh, empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Increment a named counter by `delta`, creating it if needed.
    pub fn incr(&self, name: &str, delta: u64) {
        let counter = self.counter(name);
        counter.fetch_add(delta, Ordering::Relaxed);
    }

    /// Read the current value of a counter (0 if it does not exist).
    pub fn get(&self, name: &str) -> u64 {
        let counters = self.inner.counters.lock().expect("metrics poisoned");
        counters
            .get(name)
            .map(|c| c.load(Ordering::Relaxed))
            .unwrap_or(0)
    }

    /// Take a consistent snapshot of all counters for display / export.
    pub fn snapshot(&self) -> MetricsSnapshot {
        let counters = self.inner.counters.lock().expect("metrics poisoned");
        let map = counters
            .iter()
            .map(|(k, v)| (k.clone(), v.load(Ordering::Relaxed)))
            .collect();
        MetricsSnapshot { counters: map }
    }

    fn counter(&self, name: &str) -> Arc<AtomicU64> {
        let mut counters = self.inner.counters.lock().expect("metrics poisoned");
        counters
            .entry(name.to_string())
            .or_insert_with(|| Arc::new(AtomicU64::new(0)))
            .clone()
    }
}

/// Well-known counter names used across the runtime.
pub mod names {
    /// Total tool calls executed.
    pub const TOOL_CALLS: &str = "runtime.tool_calls";
    /// Tool calls that returned an error.
    pub const TOOL_FAILURES: &str = "runtime.tool_failures";
    /// Prompt-cache hits.
    pub const CACHE_HITS: &str = "context.cache_hits";
    /// Prompt-cache misses.
    pub const CACHE_MISSES: &str = "context.cache_misses";
    /// Input tokens consumed.
    pub const TOKENS_IN: &str = "model.tokens_in";
    /// Output tokens produced.
    pub const TOKENS_OUT: &str = "model.tokens_out";
    /// Verification retries triggered.
    pub const RETRIES: &str = "verification.retries";
    /// Context compaction passes.
    pub const COMPACTIONS: &str = "context.compactions";
}

/// A point-in-time copy of all counters.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MetricsSnapshot {
    /// counter name -> value
    pub counters: BTreeMap<String, u64>,
}

impl MetricsSnapshot {
    /// Cache hit rate in `[0.0, 1.0]`, or `None` if no cache activity yet.
    pub fn cache_hit_rate(&self) -> Option<f64> {
        let hits = self.counters.get(names::CACHE_HITS).copied().unwrap_or(0);
        let misses = self.counters.get(names::CACHE_MISSES).copied().unwrap_or(0);
        let total = hits + misses;
        if total == 0 {
            None
        } else {
            Some(hits as f64 / total as f64)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counters_accumulate() {
        let m = Metrics::new();
        m.incr(names::TOOL_CALLS, 1);
        m.incr(names::TOOL_CALLS, 2);
        assert_eq!(m.get(names::TOOL_CALLS), 3);
    }

    #[test]
    fn clones_share_state() {
        let m = Metrics::new();
        let m2 = m.clone();
        m2.incr(names::RETRIES, 5);
        assert_eq!(m.get(names::RETRIES), 5);
    }

    #[test]
    fn snapshot_and_hit_rate() {
        let m = Metrics::new();
        assert_eq!(m.snapshot().cache_hit_rate(), None);
        m.incr(names::CACHE_HITS, 3);
        m.incr(names::CACHE_MISSES, 1);
        let snap = m.snapshot();
        assert_eq!(snap.cache_hit_rate(), Some(0.75));
        assert_eq!(snap.counters.get(names::CACHE_HITS), Some(&3));
    }
}
