//! The Five-Layer Context Pipeline (开发提示词.md §4; 开发计划.md Phase 3 §1).
//!
//! Assembles the context window from five conceptual layers:
//!
//! | Layer | Content              | PromptSource     |
//! | ----- | -------------------- | ---------------- |
//! | L1    | Recent conversation  | `Context`        |
//! | L2    | Task summary         | `Context` (high) |
//! | L3    | Memory injection     | `Memory`         |
//! | L4    | Workspace context    | `WorkspaceRules` |
//! | L5    | Semantic retrieval   | `Context`        |
//!
//! Plus the always-present system core, safety rules, tool rules, and the user
//! goal. The pipeline turns each populated layer into a [`PromptFragment`],
//! then fits everything to a [`PromptBudget`] so the result respects the model's
//! context window — dropping the lowest-value layers first while never dropping
//! the mandatory system/safety/user-goal fragments.
//!
//! This crate stays dependency-light: callers (the runtime) supply each layer's
//! *content* as a string. Layer 3 content comes from `deepagent-memory`
//! retrieval; Layer 4 from `deepagent-workspace`'s snapshot. The pipeline does
//! not depend on those crates, keeping the dependency graph acyclic.

use crate::budget::{BudgetOutcome, PromptBudget};
use crate::prompt::{PromptFragment, PromptSource};
use crate::tokenizer::TokenCounter;

/// Relative priority assigned to each layer within its [`PromptSource`].
/// Higher survives budget pressure longer.
mod prio {
    pub const SYSTEM: u8 = u8::MAX;
    pub const SAFETY: u8 = u8::MAX;
    pub const USER_GOAL: u8 = u8::MAX;
    pub const TOOL_RULES: u8 = 200;
    pub const TASK_SUMMARY: u8 = 180; // L2 — most valuable non-mandatory
    pub const WORKSPACE: u8 = 150; // L4
    pub const RECENT_CONVO: u8 = 130; // L1
    pub const SEMANTIC: u8 = 90; // L5
    pub const MEMORY: u8 = 70; // L3 — dropped first
}

/// Builder for a five-layer context. Unset layers are simply omitted.
#[derive(Debug, Default, Clone)]
pub struct ContextPipeline {
    system_core: Option<String>,
    safety_rules: Option<String>,
    tool_rules: Option<String>,
    // L1..L5
    recent_conversation: Option<String>,
    task_summary: Option<String>,
    memory: Option<String>,
    workspace: Option<String>,
    semantic_retrieval: Option<String>,
    // The user's goal for this turn.
    user_goal: Option<String>,
}

impl ContextPipeline {
    /// New empty pipeline.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the immutable system core (mandatory).
    pub fn system_core(mut self, s: impl Into<String>) -> Self {
        self.system_core = Some(s.into());
        self
    }

    /// Set the safety rules (mandatory).
    pub fn safety_rules(mut self, s: impl Into<String>) -> Self {
        self.safety_rules = Some(s.into());
        self
    }

    /// Set the tool-usage rules.
    pub fn tool_rules(mut self, s: impl Into<String>) -> Self {
        self.tool_rules = Some(s.into());
        self
    }

    /// L1 — recent conversation window.
    pub fn recent_conversation(mut self, s: impl Into<String>) -> Self {
        self.recent_conversation = Some(s.into());
        self
    }

    /// L2 — structured task summary.
    pub fn task_summary(mut self, s: impl Into<String>) -> Self {
        self.task_summary = Some(s.into());
        self
    }

    /// L3 — injected long-term memory.
    pub fn memory(mut self, s: impl Into<String>) -> Self {
        self.memory = Some(s.into());
        self
    }

    /// L4 — workspace context (from a `WorkspaceSnapshot`).
    pub fn workspace(mut self, s: impl Into<String>) -> Self {
        self.workspace = Some(s.into());
        self
    }

    /// L5 — semantic retrieval results.
    pub fn semantic_retrieval(mut self, s: impl Into<String>) -> Self {
        self.semantic_retrieval = Some(s.into());
        self
    }

    /// The user's goal for this turn (mandatory).
    pub fn user_goal(mut self, s: impl Into<String>) -> Self {
        self.user_goal = Some(s.into());
        self
    }

    /// Collect the populated layers into prompt fragments.
    pub fn fragments(&self) -> Vec<PromptFragment> {
        let mut frags = Vec::new();
        let mut push = |src: PromptSource, prio: u8, content: &Option<String>| {
            if let Some(c) = content {
                if !c.trim().is_empty() {
                    frags.push(PromptFragment::new(src, prio, c.clone()));
                }
            }
        };

        push(PromptSource::SystemCore, prio::SYSTEM, &self.system_core);
        push(PromptSource::SafetyRules, prio::SAFETY, &self.safety_rules);
        push(PromptSource::ToolRules, prio::TOOL_RULES, &self.tool_rules);
        push(
            PromptSource::Context,
            prio::TASK_SUMMARY,
            &self.task_summary,
        );
        push(
            PromptSource::WorkspaceRules,
            prio::WORKSPACE,
            &self.workspace,
        );
        push(
            PromptSource::Context,
            prio::RECENT_CONVO,
            &self.recent_conversation,
        );
        push(
            PromptSource::Context,
            prio::SEMANTIC,
            &self.semantic_retrieval,
        );
        push(PromptSource::Memory, prio::MEMORY, &self.memory);
        push(PromptSource::UserGoal, prio::USER_GOAL, &self.user_goal);

        frags
    }

    /// Compile and fit the context to `budget`.
    pub fn compile(&self, budget: &PromptBudget, counter: &dyn TokenCounter) -> BudgetOutcome {
        let frags = self.fragments();
        budget.fit(&frags, counter)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tokenizer::HeuristicTokenizer;

    fn full_pipeline() -> ContextPipeline {
        ContextPipeline::new()
            .system_core("You are DeepAgent.")
            .safety_rules("Be safe.")
            .tool_rules("Use tools wisely.")
            .recent_conversation("user: hi\nassistant: hello")
            .task_summary("Goal: build feature X. Done: scaffolding.")
            .memory("Past: user prefers Rust.")
            .workspace("# Workspace\nProject type: Rust")
            .semantic_retrieval("relevant snippet from payment.rs")
            .user_goal("Fix the payment timeout bug.")
    }

    #[test]
    fn assembles_all_layers_when_budget_is_large() {
        let counter = HeuristicTokenizer::new();
        let budget = PromptBudget::new(100_000, 1000, 1000);
        let out = full_pipeline().compile(&budget, &counter);
        assert_eq!(out.dropped_fragments, 0);
        // System core comes first, user goal last.
        assert_eq!(
            out.prompt.fragments.first().unwrap().source,
            PromptSource::SystemCore
        );
        assert_eq!(
            out.prompt.fragments.last().unwrap().source,
            PromptSource::UserGoal
        );
    }

    #[test]
    fn empty_layers_are_omitted() {
        let counter = HeuristicTokenizer::new();
        let budget = PromptBudget::new(100_000, 0, 0);
        let out = ContextPipeline::new()
            .system_core("sys")
            .user_goal("goal")
            .memory("   ") // blank -> omitted
            .compile(&budget, &counter);
        assert_eq!(out.prompt.fragments.len(), 2);
    }

    #[test]
    fn memory_dropped_before_task_summary_under_pressure() {
        let counter = HeuristicTokenizer::new();
        // Small budget: mandatory + a little room. Memory (lowest prio) should
        // go before the task summary (higher prio).
        let budget = PromptBudget::new(40, 2, 2); // allowance 36
        let pipeline = ContextPipeline::new()
            .system_core("sys")
            .user_goal("goal")
            .task_summary("T ".repeat(15).trim().to_string()) // ~15 tokens
            .memory("M ".repeat(40).trim().to_string()); // ~40 tokens, too big
        let out = pipeline.compile(&budget, &counter);
        let has_task = out
            .prompt
            .fragments
            .iter()
            .any(|f| f.content.starts_with('T'));
        let has_memory = out
            .prompt
            .fragments
            .iter()
            .any(|f| f.content.starts_with('M'));
        assert!(has_task, "task summary should survive");
        assert!(!has_memory, "memory should be dropped first");
    }

    #[test]
    fn mandatory_layers_always_present() {
        let counter = HeuristicTokenizer::new();
        let budget = PromptBudget::new(8, 2, 2); // tiny
        let out = full_pipeline().compile(&budget, &counter);
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
    }
}
