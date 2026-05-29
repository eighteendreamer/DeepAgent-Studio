//! The Prompt Compiler AST (开发提示词.md §6).
//!
//! > Prompt 不是字符串。而是：AST。
//!
//! A prompt is assembled from typed [`PromptFragment`]s, each tagged with a
//! [`PromptSource`] and a `priority`. The compiler orders fragments by source
//! (so the system core always precedes the user goal) and, under budget
//! pressure, drops the lowest-priority fragments first.

use serde::{Deserialize, Serialize};

use crate::tokenizer::TokenCounter;

/// Where a prompt fragment originates. The ordering of this enum defines the
/// canonical assembly order of the compiled prompt:
///
/// ```text
/// System Core + Safety Rules + Workspace Rules + Agent Identity
///   + Tool Rules + Memory + Context + User Goal
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptSource {
    /// Immutable system core. Never dropped.
    SystemCore,
    /// Safety / guardrail rules. Never dropped.
    SafetyRules,
    /// Workspace-specific rules (steering files).
    WorkspaceRules,
    /// The agent's identity / role.
    AgentIdentity,
    /// Tool usage rules.
    ToolRules,
    /// Injected long-term memory.
    Memory,
    /// Retrieved / conversational context.
    Context,
    /// The user's goal for this turn. Never dropped.
    UserGoal,
}

impl PromptSource {
    /// Whether fragments from this source are mandatory (never dropped by the
    /// budgeter, even under pressure).
    pub const fn is_mandatory(&self) -> bool {
        matches!(
            self,
            PromptSource::SystemCore | PromptSource::SafetyRules | PromptSource::UserGoal
        )
    }
}

/// A single typed piece of a prompt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptFragment {
    /// Logical origin of the fragment (drives ordering & mandatory-ness).
    pub source: PromptSource,
    /// Higher priority fragments are kept longer under budget pressure.
    /// Range is arbitrary; only relative ordering matters.
    pub priority: u8,
    /// The textual content of the fragment.
    pub content: String,
}

impl PromptFragment {
    /// Build a fragment.
    pub fn new(source: PromptSource, priority: u8, content: impl Into<String>) -> Self {
        Self {
            source,
            priority,
            content: content.into(),
        }
    }

    /// A mandatory system-core fragment (max priority).
    pub fn system_core(content: impl Into<String>) -> Self {
        Self::new(PromptSource::SystemCore, u8::MAX, content)
    }

    /// A mandatory user-goal fragment (max priority).
    pub fn user_goal(content: impl Into<String>) -> Self {
        Self::new(PromptSource::UserGoal, u8::MAX, content)
    }
}

/// The result of compiling a set of fragments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompiledPrompt {
    /// The fragments that were kept, in canonical order.
    pub fragments: Vec<PromptFragment>,
    /// The fully rendered prompt text.
    pub text: String,
    /// Estimated token count of [`CompiledPrompt::text`].
    pub tokens: usize,
}

/// Compiles [`PromptFragment`]s into a single ordered prompt string.
#[derive(Debug, Default)]
pub struct PromptCompiler {
    fragments: Vec<PromptFragment>,
}

impl PromptCompiler {
    /// New empty compiler.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a fragment (builder style).
    pub fn with(mut self, fragment: PromptFragment) -> Self {
        self.fragments.push(fragment);
        self
    }

    /// Add a fragment in place.
    pub fn push(&mut self, fragment: PromptFragment) -> &mut Self {
        self.fragments.push(fragment);
        self
    }

    /// Borrow the current fragments.
    pub fn fragments(&self) -> &[PromptFragment] {
        &self.fragments
    }

    /// Order fragments canonically: primarily by [`PromptSource`] order, then
    /// by descending priority, preserving insertion order for ties.
    fn ordered(&self) -> Vec<PromptFragment> {
        let mut out: Vec<(usize, PromptFragment)> =
            self.fragments.iter().cloned().enumerate().collect();
        out.sort_by(|(ia, a), (ib, b)| {
            a.source
                .cmp(&b.source)
                .then(b.priority.cmp(&a.priority))
                .then(ia.cmp(ib))
        });
        out.into_iter().map(|(_, f)| f).collect()
    }

    /// Compile without any budget constraint.
    pub fn compile(&self, counter: &dyn TokenCounter) -> CompiledPrompt {
        let fragments = self.ordered();
        render(fragments, counter)
    }
}

/// Render an ordered fragment list into a [`CompiledPrompt`].
pub(crate) fn render(fragments: Vec<PromptFragment>, counter: &dyn TokenCounter) -> CompiledPrompt {
    let text = fragments
        .iter()
        .map(|f| f.content.as_str())
        .collect::<Vec<_>>()
        .join("\n\n");
    let tokens = counter.count(&text);
    CompiledPrompt {
        fragments,
        text,
        tokens,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tokenizer::HeuristicTokenizer;

    #[test]
    fn fragments_are_ordered_by_source() {
        let counter = HeuristicTokenizer::new();
        let compiled = PromptCompiler::new()
            .with(PromptFragment::user_goal("do the thing"))
            .with(PromptFragment::system_core("you are an agent"))
            .with(PromptFragment::new(PromptSource::ToolRules, 10, "tools"))
            .compile(&counter);

        let sources: Vec<_> = compiled.fragments.iter().map(|f| f.source).collect();
        assert_eq!(
            sources,
            vec![
                PromptSource::SystemCore,
                PromptSource::ToolRules,
                PromptSource::UserGoal
            ]
        );
    }

    #[test]
    fn higher_priority_first_within_source() {
        let counter = HeuristicTokenizer::new();
        let compiled = PromptCompiler::new()
            .with(PromptFragment::new(PromptSource::Memory, 1, "low"))
            .with(PromptFragment::new(PromptSource::Memory, 9, "high"))
            .compile(&counter);
        assert_eq!(compiled.fragments[0].content, "high");
    }

    #[test]
    fn mandatory_sources_flagged() {
        assert!(PromptSource::SystemCore.is_mandatory());
        assert!(PromptSource::UserGoal.is_mandatory());
        assert!(!PromptSource::Memory.is_mandatory());
    }

    #[test]
    fn compiled_text_joins_fragments() {
        let counter = HeuristicTokenizer::new();
        let compiled = PromptCompiler::new()
            .with(PromptFragment::system_core("A"))
            .with(PromptFragment::user_goal("B"))
            .compile(&counter);
        assert_eq!(compiled.text, "A\n\nB");
        assert!(compiled.tokens > 0);
    }
}
