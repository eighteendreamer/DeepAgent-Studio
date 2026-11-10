//! Reciprocal Rank Fusion (RRF) — shared by the hybrid retrievers.
//!
//! RRF merges multiple ranked lists into one without needing comparable score
//! scales: each item accrues `1 / (k + rank)` from every list it appears in
//! (rank is 1-based). Items appearing high in multiple lists rise to the top.
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
    reciprocal_rank_fusion_many(&[primary, secondary], k, tiebreak)
}

/// Fuse any number of ranked lists while keeping ranks local to the original
/// list. Duplicate ids within one list contribute only their first rank.
pub fn reciprocal_rank_fusion_many<Id, F>(lists: &[&[Id]], k: f32, tiebreak: F) -> Vec<(Id, f32)>
where
    Id: Clone + Eq + Hash,
    F: Fn(&Id, &Id) -> std::cmp::Ordering,
{
    let ranked: Vec<Vec<_>> = lists
        .iter()
        .map(|list| {
            list.iter()
                .cloned()
                .enumerate()
                .map(|(i, id)| (id, i + 1))
                .collect()
        })
        .collect();
    let lists: Vec<_> = ranked.iter().map(Vec::as_slice).collect();
    reciprocal_rank_fusion_ranked(&lists, k, tiebreak)
}

/// Fuse lists with explicit original 1-based ranks. Filtering or deduplicating
/// an upstream list must not promote the ranks of the remaining entries.
pub fn reciprocal_rank_fusion_ranked<Id, F>(
    lists: &[&[(Id, usize)]],
    k: f32,
    tiebreak: F,
) -> Vec<(Id, f32)>
where
    Id: Clone + Eq + Hash,
    F: Fn(&Id, &Id) -> std::cmp::Ordering,
{
    let mut scores: HashMap<Id, f32> = HashMap::new();
    for list in lists {
        let mut seen = std::collections::HashSet::new();
        for (id, rank) in *list {
            if seen.insert(id) {
                *scores.entry(id.clone()).or_insert(0.0) += 1.0 / (k + *rank as f32);
            }
        }
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

    #[test]
    fn three_sources_keep_their_original_ranks_and_ignore_duplicates_per_source() {
        let fused = reciprocal_rank_fusion_many(&[&[1u32, 2, 2], &[3, 2], &[1, 3]], 60.0, ord);
        let scores: HashMap<_, _> = fused.into_iter().collect();
        assert_eq!(scores[&1], 2.0 / 61.0);
        assert_eq!(scores[&2], 1.0 / 62.0 + 1.0 / 62.0);
        assert_eq!(scores[&3], 1.0 / 61.0 + 1.0 / 62.0);
    }

    #[test]
    fn explicit_ranks_keep_gaps_after_filtering() {
        let fused = reciprocal_rank_fusion_ranked(&[&[(1u32, 1), (2, 3)]], 60.0, ord);
        assert_eq!(fused, [(1, 1.0 / 61.0), (2, 1.0 / 63.0)]);
    }
}
