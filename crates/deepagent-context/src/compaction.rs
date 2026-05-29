//! Context compaction (开发计划.md Phase 3 §3).
//!
//! When the conversation grows past a threshold, older turns are compacted into
//! a compact, structured [`TaskSummary`] (开发提示词.md §4 Layer 2) rather than
//! being carried verbatim. This is the "summarize / memory unload" strategy.
//!
//! The summarization itself is pluggable via [`Summarizer`]: production uses a
//! model-backed summarizer, while the built-in [`HeuristicSummarizer`] produces
//! a deterministic extractive summary with no model call (used as a fallback
//! and in tests). The decision of *when* to compact lives in
//! [`CompactionPolicy`].

use serde::{Deserialize, Serialize};

use crate::tokenizer::TokenCounter;

/// A structured task summary — the durable, compacted memory of progress so far
/// (开发提示词.md §4 Layer 2: "不是简单 summarize，而是 Structured Summary").
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskSummary {
    /// The overall goal.
    pub goal: String,
    /// Steps completed so far.
    pub completed_steps: Vec<String>,
    /// Steps still pending.
    pub pending_steps: Vec<String>,
    /// Known failures / dead-ends to avoid repeating.
    pub known_failures: Vec<String>,
    /// Notable design decisions.
    pub design_decisions: Vec<String>,
}

impl TaskSummary {
    /// Render the summary as a compact prompt block for Layer 2 injection.
    pub fn to_context_block(&self) -> String {
        let mut out = String::from("# Task summary\n");
        out.push_str(&format!("Goal: {}\n", self.goal));
        let section = |title: &str, items: &[String]| -> String {
            if items.is_empty() {
                String::new()
            } else {
                let mut s = format!("\n{title}:\n");
                for i in items {
                    s.push_str(&format!("- {i}\n"));
                }
                s
            }
        };
        out.push_str(&section("Completed", &self.completed_steps));
        out.push_str(&section("Pending", &self.pending_steps));
        out.push_str(&section("Known failures", &self.known_failures));
        out.push_str(&section("Design decisions", &self.design_decisions));
        out.trim_end().to_string()
    }
}

/// When to trigger compaction.
#[derive(Debug, Clone, Copy)]
pub struct CompactionPolicy {
    /// Compact when the tracked content exceeds this many tokens.
    pub trigger_tokens: usize,
    /// Number of most-recent turns to always keep verbatim (never compacted).
    pub keep_recent_turns: usize,
}

impl Default for CompactionPolicy {
    fn default() -> Self {
        Self {
            trigger_tokens: 8_000,
            keep_recent_turns: 6,
        }
    }
}

impl CompactionPolicy {
    /// Whether content of `tokens` size should trigger compaction.
    pub fn should_compact(&self, tokens: usize) -> bool {
        tokens > self.trigger_tokens
    }
}

/// Produces a [`TaskSummary`] from a goal + a set of older conversation turns.
pub trait Summarizer {
    /// Summarize `older_turns` (each a rendered message string) under `goal`,
    /// folding into / refining the `prior` summary.
    fn summarize(&self, goal: &str, prior: &TaskSummary, older_turns: &[String]) -> TaskSummary;
}

/// A deterministic, model-free extractive summarizer.
///
/// It does not attempt semantic understanding; it extracts signal heuristically:
/// turns that look like completed actions vs. failures. To guarantee genuine
/// compression, each section is capped at [`HeuristicSummarizer::MAX_PER_SECTION`]
/// entries (keeping the most recent), so the summary never grows unbounded. This
/// guarantees the runtime can always compact (no network dependency) and gives
/// tests a stable oracle. A model-backed summarizer implements the same trait.
#[derive(Debug, Clone, Copy, Default)]
pub struct HeuristicSummarizer;

impl HeuristicSummarizer {
    /// Maximum entries retained per section.
    pub const MAX_PER_SECTION: usize = 8;
}

impl Summarizer for HeuristicSummarizer {
    fn summarize(&self, goal: &str, prior: &TaskSummary, older_turns: &[String]) -> TaskSummary {
        let mut summary = prior.clone();
        if summary.goal.is_empty() {
            summary.goal = goal.to_string();
        }
        for turn in older_turns {
            let lower = turn.to_lowercase();
            let line: String = turn
                .lines()
                .next()
                .unwrap_or(turn)
                .chars()
                .take(160)
                .collect();
            if lower.contains("error") || lower.contains("failed") || lower.contains("panic") {
                push_unique(&mut summary.known_failures, line);
            } else if lower.contains("decided")
                || lower.contains("chose")
                || lower.contains("because")
            {
                push_unique(&mut summary.design_decisions, line);
            } else if lower.contains("done")
                || lower.contains("completed")
                || lower.contains("added")
                || lower.contains("created")
                || lower.contains("fixed")
            {
                push_unique(&mut summary.completed_steps, line);
            }
        }
        // Cap each section to keep the summary compact (most recent kept).
        cap_recent(&mut summary.completed_steps, Self::MAX_PER_SECTION);
        cap_recent(&mut summary.pending_steps, Self::MAX_PER_SECTION);
        cap_recent(&mut summary.known_failures, Self::MAX_PER_SECTION);
        cap_recent(&mut summary.design_decisions, Self::MAX_PER_SECTION);
        summary
    }
}

fn push_unique(v: &mut Vec<String>, item: String) {
    if !v.contains(&item) {
        v.push(item);
    }
}

/// Keep only the most recent `max` items (drops from the front).
fn cap_recent(v: &mut Vec<String>, max: usize) {
    if v.len() > max {
        let drop = v.len() - max;
        v.drain(..drop);
    }
}

/// The compaction engine: decides whether to compact and applies a summarizer.
pub struct Compactor<S: Summarizer> {
    policy: CompactionPolicy,
    summarizer: S,
}

impl<S: Summarizer> Compactor<S> {
    /// Build a compactor.
    pub fn new(policy: CompactionPolicy, summarizer: S) -> Self {
        Self { policy, summarizer }
    }

    /// Given the full ordered list of conversation turns, compact the older
    /// ones into the summary if the policy triggers. Returns the (possibly
    /// updated) summary and the turns to keep verbatim.
    ///
    /// `counter` measures total token pressure to decide whether to act.
    pub fn maybe_compact(
        &self,
        goal: &str,
        prior: &TaskSummary,
        turns: &[String],
        counter: &dyn TokenCounter,
    ) -> CompactionResult {
        let total_tokens: usize = turns.iter().map(|t| counter.count(t)).sum();
        if !self.policy.should_compact(total_tokens) || turns.len() <= self.policy.keep_recent_turns
        {
            return CompactionResult {
                summary: prior.clone(),
                kept_turns: turns.to_vec(),
                compacted: false,
                tokens_before: total_tokens,
                tokens_after: total_tokens,
            };
        }

        let split = turns.len() - self.policy.keep_recent_turns;
        let (older, recent) = turns.split_at(split);
        let summary = self.summarizer.summarize(goal, prior, older);

        let summary_block = summary.to_context_block();
        let tokens_after =
            counter.count(&summary_block) + recent.iter().map(|t| counter.count(t)).sum::<usize>();

        CompactionResult {
            summary,
            kept_turns: recent.to_vec(),
            compacted: true,
            tokens_before: total_tokens,
            tokens_after,
        }
    }
}

/// Outcome of a compaction pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactionResult {
    /// The (possibly updated) structured summary.
    pub summary: TaskSummary,
    /// Turns retained verbatim (the recent window).
    pub kept_turns: Vec<String>,
    /// Whether compaction actually happened.
    pub compacted: bool,
    /// Token pressure before.
    pub tokens_before: usize,
    /// Estimated token pressure after.
    pub tokens_after: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tokenizer::HeuristicTokenizer;

    #[test]
    fn task_summary_renders_sections() {
        let s = TaskSummary {
            goal: "build X".into(),
            completed_steps: vec!["scaffolded".into()],
            pending_steps: vec!["write tests".into()],
            known_failures: vec![],
            design_decisions: vec!["use Rust".into()],
        };
        let block = s.to_context_block();
        assert!(block.contains("Goal: build X"));
        assert!(block.contains("Completed:"));
        assert!(block.contains("- scaffolded"));
        assert!(block.contains("Design decisions:"));
        // Empty section omitted.
        assert!(!block.contains("Known failures:"));
    }

    #[test]
    fn policy_triggers_above_threshold() {
        let p = CompactionPolicy {
            trigger_tokens: 100,
            keep_recent_turns: 2,
        };
        assert!(p.should_compact(101));
        assert!(!p.should_compact(100));
    }

    #[test]
    fn no_compaction_below_threshold() {
        let counter = HeuristicTokenizer::new();
        let compactor = Compactor::new(
            CompactionPolicy {
                trigger_tokens: 100_000,
                keep_recent_turns: 2,
            },
            HeuristicSummarizer,
        );
        let turns = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let res = compactor.maybe_compact("goal", &TaskSummary::default(), &turns, &counter);
        assert!(!res.compacted);
        assert_eq!(res.kept_turns.len(), 3);
    }

    #[test]
    fn compaction_summarizes_older_turns() {
        let counter = HeuristicTokenizer::new();
        let compactor = Compactor::new(
            CompactionPolicy {
                trigger_tokens: 1, // always trigger
                keep_recent_turns: 1,
            },
            HeuristicSummarizer,
        );
        let turns = vec![
            "Created the database module".to_string(),
            "Error: migration failed on startup".to_string(),
            "Chose SQLite because it is embedded".to_string(),
            "the most recent turn".to_string(),
        ];
        let res =
            compactor.maybe_compact("build storage", &TaskSummary::default(), &turns, &counter);
        assert!(res.compacted);
        assert_eq!(res.summary.goal, "build storage");
        assert!(res
            .summary
            .completed_steps
            .iter()
            .any(|s| s.contains("Created")));
        assert!(res
            .summary
            .known_failures
            .iter()
            .any(|s| s.contains("migration failed")));
        assert!(res
            .summary
            .design_decisions
            .iter()
            .any(|s| s.contains("Chose SQLite")));
        // Only the most recent turn is kept verbatim.
        assert_eq!(res.kept_turns, vec!["the most recent turn".to_string()]);
    }

    #[test]
    fn compaction_reduces_tokens() {
        let counter = HeuristicTokenizer::new();
        let compactor = Compactor::new(
            CompactionPolicy {
                trigger_tokens: 1,
                keep_recent_turns: 1,
            },
            HeuristicSummarizer,
        );
        // Many verbose older turns.
        let mut turns: Vec<String> = (0..20)
            .map(|i| format!("Added feature number {i} with a fairly long description line"))
            .collect();
        turns.push("recent".to_string());
        let res = compactor.maybe_compact("goal", &TaskSummary::default(), &turns, &counter);
        assert!(res.compacted);
        assert!(res.tokens_after < res.tokens_before);
    }

    #[test]
    fn prior_summary_is_refined_not_replaced() {
        let prior = TaskSummary {
            goal: "existing goal".into(),
            completed_steps: vec!["earlier step".into()],
            ..Default::default()
        };
        let s = HeuristicSummarizer;
        let refined = s.summarize("ignored", &prior, &["Created new thing".to_string()]);
        assert_eq!(refined.goal, "existing goal");
        assert!(refined.completed_steps.iter().any(|x| x == "earlier step"));
        assert!(refined
            .completed_steps
            .iter()
            .any(|x| x.contains("Created new thing")));
    }
}
