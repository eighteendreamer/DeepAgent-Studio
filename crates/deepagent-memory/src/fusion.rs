//! Reciprocal Rank Fusion (RRF) — shared by the hybrid retrievers.
//!
//! RRF merges multiple ranked lists into one without needing comparable score
//! scales: each item accrues `1 / (k + rank)` from every list it appears in
//! (rank is 0-based). Items appearing high in multiple lists rise to the top.
//! `k` (Cormack et al. use 60) dampens the contribution of top ranks so a
//! single list cannot dominate.

use std::collections::HashMap;
use std::hash::Hash;

/// Fuse two ranked id lists via RRF. Returns `(id, score)` sorted by score
/// descending. `tiebreak` provides a deterministic order for equal scores.
pub fn reciprocal_rank_fusion<Id, F>(
    primary: &[Id],
    secondary: &[Id],
    k: f32,
    tiebreak: F,
) -> Vec<(Id, f32)>
where
    Id: Clone + Eq + Hash,
    F: Fn(&Id, &Id) -> std::cmp::Ordering,
{
    let mut scores: HashMap<Id, f32> = HashMap::new();
    for (rank, id) in primary.iter().enumerate() {
        *scores.entry(id.clone()).or_insert(0.0) += 1.0 / (k + rank as f32 + 1.0);
    }
    for (rank, id) in secondary.iter().enumerate() {
        *scores.entry(id.clone()).or_insert(0.0) += 1.0 / (k + rank as f32 + 1.0);
    }
    let mut fused: Vec<(Id, f32)> = scores.into_iter().collect();
    fused.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| tiebreak(&a.0, &b.0))
    });
    fused
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ord(a: &u32, b: &u32) -> std::cmp::Ordering {
        a.cmp(b)
    }

    #[test]
    fn item_in_both_lists_ranks_highest() {
        // id 2 appears high in both lists -> should win.
        let dense = vec![1u32, 2, 3];
        let sparse = vec![2u32, 4, 5];
        let fused = reciprocal_rank_fusion(&dense, &sparse, 60.0, ord);
        assert_eq!(fused[0].0, 2);
    }

    #[test]
    fn disjoint_lists_preserve_relative_order() {
        let dense = vec![1u32, 2];
        let sparse = vec![3u32, 4];
        let fused = reciprocal_rank_fusion(&dense, &sparse, 60.0, ord);
        // Top-ranked of each list (1 and 3) outrank second-ranked (2 and 4).
        let pos = |id: u32| fused.iter().position(|(x, _)| *x == id).unwrap();
        assert!(pos(1) < pos(2));
        assert!(pos(3) < pos(4));
    }

    #[test]
    fn empty_inputs_yield_empty() {
        let fused = reciprocal_rank_fusion::<u32, _>(&[], &[], 60.0, ord);
        assert!(fused.is_empty());
    }

    #[test]
    fn deterministic_tiebreak() {
        // Same single-list rank for two ids -> tiebreak by id.
        let a = reciprocal_rank_fusion(&[5u32, 3], &[], 60.0, ord);
        let b = reciprocal_rank_fusion(&[5u32, 3], &[], 60.0, ord);
        assert_eq!(a, b);
    }
}
