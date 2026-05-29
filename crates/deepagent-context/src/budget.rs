//! The Prompt Budget / Token Economy system (开发提示词.md §7).
//!
//! > Agent 最大问题：Context 爆炸。必须：动态裁剪。
//!
//! [`PromptBudget`] partitions a model's context window into reserves (output,
//! tools) and a remaining allowance for the prompt itself. The [`PromptBudget::fit`]
//! routine greedily keeps the highest-value fragments (mandatory first, then by
//! source order + priority) until the allowance is exhausted, dropping the rest.

use serde::{Deserialize, Serialize};

use crate::prompt::{render, CompiledPrompt, PromptFragment};
use crate::tokenizer::TokenCounter;

/// A token budget for a single model invocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptBudget {
    /// Total context window of the model (in tokens).
    pub total_budget: usize,
    /// Tokens reserved for the model's output.
    pub reserved_for_output: usize,
    /// Tokens reserved for tool schemas / results.
    pub reserved_for_tools: usize,
}

impl PromptBudget {
    /// Construct a budget.
    pub const fn new(
        total_budget: usize,
        reserved_for_output: usize,
        reserved_for_tools: usize,
    ) -> Self {
        Self {
            total_budget,
            reserved_for_output,
            reserved_for_tools,
        }
    }

    /// Tokens available for the prompt after subtracting reserves (saturating
    /// at 0 so an over-committed budget yields no allowance rather than panic).
    pub const fn prompt_allowance(&self) -> usize {
        self.total_budget
            .saturating_sub(self.reserved_for_output)
            .saturating_sub(self.reserved_for_tools)
    }

    /// Fit `fragments` (already in arbitrary order) into the allowance.
    ///
    /// Algorithm:
    /// 1. Sort by keep-priority: mandatory fragments first, then by source
    ///    order, then descending priority.
    /// 2. Greedily accumulate while the running token total stays within
    ///    allowance. Mandatory fragments are always kept (even if they alone
    ///    exceed the allowance — the caller is told via [`BudgetOutcome`]).
    /// 3. Re-render the kept set in canonical prompt order.
    pub fn fit(&self, fragments: &[PromptFragment], counter: &dyn TokenCounter) -> BudgetOutcome {
        let allowance = self.prompt_allowance();

        // Rank for *keeping* decisions.
        let mut ranked: Vec<(usize, &PromptFragment, usize)> = fragments
            .iter()
            .enumerate()
            .map(|(i, f)| (i, f, counter.count(&f.content)))
            .collect();
        ranked.sort_by(|(ia, a, _), (ib, b, _)| {
            b.source
                .is_mandatory()
                .cmp(&a.source.is_mandatory()) // mandatory first
                .then(a.source.cmp(&b.source))
                .then(b.priority.cmp(&a.priority))
                .then(ia.cmp(ib))
        });

        let mut used = 0usize;
        let mut kept_indices: Vec<usize> = Vec::new();
        let mut dropped = 0usize;
        let mut overflowed = false;

        for (idx, frag, cost) in &ranked {
            if frag.source.is_mandatory() {
                used += cost;
                kept_indices.push(*idx);
                if used > allowance {
                    overflowed = true;
                }
            } else if used + cost <= allowance {
                used += cost;
                kept_indices.push(*idx);
            } else {
                dropped += 1;
            }
        }

        // Re-render kept fragments in canonical (source) order.
        kept_indices.sort_unstable();
        let mut kept: Vec<PromptFragment> =
            kept_indices.iter().map(|i| fragments[*i].clone()).collect();
        kept.sort_by(|a, b| a.source.cmp(&b.source).then(b.priority.cmp(&a.priority)));

        let prompt = render(kept, counter);
        BudgetOutcome {
            prompt,
            dropped_fragments: dropped,
            allowance,
            overflowed,
        }
    }
}

/// The result of fitting fragments to a budget.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BudgetOutcome {
    /// The compiled prompt after budgeting.
    pub prompt: CompiledPrompt,
    /// How many non-mandatory fragments were dropped.
    pub dropped_fragments: usize,
    /// The prompt allowance that was applied.
    pub allowance: usize,
    /// True if mandatory fragments alone exceeded the allowance (the caller
    /// should consider compaction or a larger-context model).
    pub overflowed: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prompt::PromptSource;
    use crate::tokenizer::HeuristicTokenizer;

    fn frag(source: PromptSource, prio: u8, content: &str) -> PromptFragment {
        PromptFragment::new(source, prio, content)
    }

    #[test]
    fn allowance_subtracts_reserves() {
        let b = PromptBudget::new(1000, 200, 100);
        assert_eq!(b.prompt_allowance(), 700);
    }

    #[test]
    fn allowance_saturates() {
        let b = PromptBudget::new(100, 200, 100);
        assert_eq!(b.prompt_allowance(), 0);
    }

    #[test]
    fn keeps_everything_when_budget_is_large() {
        let counter = HeuristicTokenizer::new();
        let b = PromptBudget::new(100_000, 1000, 1000);
        let frags = vec![
            PromptFragment::system_core("system"),
            frag(PromptSource::Memory, 5, "some memory"),
            PromptFragment::user_goal("the goal"),
        ];
        let out = b.fit(&frags, &counter);
        assert_eq!(out.dropped_fragments, 0);
        assert!(!out.overflowed);
        assert_eq!(out.prompt.fragments.len(), 3);
    }

    #[test]
    fn drops_low_priority_under_pressure() {
        let counter = HeuristicTokenizer::new();
        // Tiny allowance: only mandatory + maybe one fragment survive.
        let b = PromptBudget::new(20, 5, 5); // allowance = 10 tokens
        let big_memory = "x ".repeat(200); // ~100 tokens
        let frags = vec![
            PromptFragment::system_core("sys"),
            frag(PromptSource::Memory, 1, &big_memory),
            PromptFragment::user_goal("goal"),
        ];
        let out = b.fit(&frags, &counter);
        // Mandatory ones kept; the big memory dropped.
        assert!(out
            .prompt
            .fragments
            .iter()
            .any(|f| f.source == PromptSource::SystemCore));
        assert!(out
            .prompt
            .fragments
            .iter()
            .any(|f| f.source == PromptSource::UserGoal));
        assert!(!out
            .prompt
            .fragments
            .iter()
            .any(|f| f.source == PromptSource::Memory));
        assert_eq!(out.dropped_fragments, 1);
    }

    #[test]
    fn mandatory_overflow_is_flagged() {
        let counter = HeuristicTokenizer::new();
        let b = PromptBudget::new(10, 2, 2); // allowance = 6
        let huge_goal = "word ".repeat(100);
        let frags = vec![PromptFragment::user_goal(&huge_goal)];
        let out = b.fit(&frags, &counter);
        assert!(out.overflowed);
        // Still kept because mandatory.
        assert_eq!(out.prompt.fragments.len(), 1);
    }
}
