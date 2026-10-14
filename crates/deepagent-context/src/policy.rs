//! Dynamic context budgeting policy.

use serde::{Deserialize, Serialize};

use deepagent_models::{ModelCapability, ThinkingDepth};

use crate::budget::PromptBudget;

/// Default Harness-style compaction headroom (65_536). Overridable via the
/// `autocompact_reserve_tokens` setting / `DEEPAGENT_AUTOCOMPACT_RESERVE_TOKENS`.
pub const AUTOCOMPACT_BUFFER_TOKENS: usize = 65_536;

/// One calculation shared by prompt assembly, pressure checks and UI snapshots.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContextBudget {
    pub context_window: usize,
    pub reserved_output: usize,
    pub reserved_tools: usize,
    pub headroom: usize,
    pub prompt_budget: usize,
    pub compaction_threshold: usize,
}

impl ContextBudget {
    pub fn new(window: usize, output: usize, tools: usize, headroom: usize) -> Self {
        let message_budget = window.saturating_sub(output);
        Self {
            context_window: window,
            reserved_output: output,
            reserved_tools: tools,
            headroom,
            prompt_budget: message_budget.saturating_sub(tools),
            compaction_threshold: ratio(window, 80).min(message_budget.saturating_sub(headroom)),
        }
    }
}

/// How much verbatim tail a compaction must preserve, measured in tokens rather
/// than entry counts. Aligned with Claude Code
/// `sessionMemoryCompact.ts::DEFAULT_SM_COMPACT_CONFIG` (minTokens 10_000 /
/// minTextBlockMessages 5 / maxTokens 40_000).
///
/// The two minimums exist because a tail of eight giant tool dumps keeps the
/// recent *volume* but not the recent *thread*: the model loses the narrative it
/// was building. `max_tokens` is the hard stop in the other direction: a bigger
/// retained tail means the compacted request still overflows, which is what makes
/// compaction loop.
///
/// This governs the reactive paths (the overflow pass and its speculative
/// prefire). The before-the-run proactive compaction keeps its own turn-count
/// knob, `CompactionPolicy::keep_recent_turns`; they are deliberately not
/// merged, because one reacts to a provider overflow and the other to a
/// pre-request token estimate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompactionKeepWindow {
    /// Least tail worth of tokens before the cut may stop.
    pub min_tokens: usize,
    /// Least number of tail entries carrying narrative text (not tool traffic).
    pub min_text_block_entries: usize,
    /// Grow no further once the tail reaches this size.
    pub max_tokens: usize,
}

/// Budgeting policy derived from a resolved model capability.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextPolicy {
    pub model_id: String,
    pub context_window: usize,
    pub max_output_tokens: usize,
    pub reserved_output_tokens: usize,
    pub reserved_tool_tokens: usize,
    pub prompt_budget: usize,
    pub auto_compact_at: usize,
    pub warning_at: usize,
    pub danger_at: usize,
}

impl ContextPolicy {
    /// Build policy from official/model capability and the active thinking
    /// depth. Thresholds are ratios of the real model budget, not fixed magic
    /// numbers scattered through the app.
    pub fn for_capability(capability: &ModelCapability, thinking_depth: ThinkingDepth) -> Self {
        Self::for_capability_with_output(capability, thinking_depth, None)
    }

    /// Build a policy using the output ceiling of the actual request when one
    /// is available. The provider context capacity remains the denominator.
    pub fn for_capability_with_output(
        capability: &ModelCapability,
        thinking_depth: ThinkingDepth,
        requested_output_tokens: Option<usize>,
    ) -> Self {
        let management_window = capability.context_window;
        let _ = thinking_depth; // Reasoning effort does not change the provider capacity.
        let reserved_output_tokens = requested_output_tokens
            .unwrap_or(capability.max_output_tokens)
            .min(capability.max_output_tokens)
            .min(management_window);
        let reserved_tool_tokens = (management_window / 16).clamp(8_000, 64_000);
        let budget = ContextBudget::new(
            management_window,
            reserved_output_tokens,
            reserved_tool_tokens,
            AUTOCOMPACT_BUFFER_TOKENS,
        );
        let prompt_budget = budget.prompt_budget;

        Self {
            model_id: capability.model_id.clone(),
            context_window: capability.context_window,
            max_output_tokens: capability.max_output_tokens,
            reserved_output_tokens,
            reserved_tool_tokens,
            prompt_budget,
            auto_compact_at: budget.compaction_threshold,
            warning_at: ratio(prompt_budget, 80),
            danger_at: ratio(prompt_budget, 92),
        }
    }

    /// Convert to the existing prompt-budget primitive.
    pub const fn prompt_budget(&self) -> PromptBudget {
        PromptBudget::new(
            self.context_window,
            self.reserved_output_tokens,
            self.reserved_tool_tokens,
        )
    }

    /// A compaction trigger suitable for conversation history token pressure.
    pub const fn compaction_trigger_tokens(&self) -> usize {
        self.auto_compact_at
    }

    /// The verbatim tail budget every compaction path must honour: the reactive
    /// committed pass, its speculative prefire, and both the `Message` and
    /// response-item views of each. One definition for all four; a tail chosen
    /// any other way makes the prefire note unreusable and the run pays twice.
    ///
    /// A constant rather than a per-model field on purpose: these are the tail
    /// *floor* numbers Claude Code runs with, and the provider window is already
    /// handled by [`ContextPolicy::autocompact_threshold_tokens`].
    pub const KEEP_WINDOW: CompactionKeepWindow = CompactionKeepWindow {
        min_tokens: 10_000,
        min_text_block_entries: 5,
        max_tokens: 40_000,
    };

    /// Input capacity after reserving the active request's output ceiling.
    pub fn effective_context_window(&self) -> usize {
        self.context_window
            .saturating_sub(self.reserved_output_tokens)
    }

    /// min(80% of capacity, capacity - active output - headroom).
    /// The legacy percentage override can only lower this threshold.
    pub fn autocompact_threshold_tokens(
        &self,
        reserve_tokens: Option<usize>,
        pct_override: Option<f32>,
    ) -> usize {
        let effective = self.effective_context_window();
        let buffer = reserve_tokens.unwrap_or(AUTOCOMPACT_BUFFER_TOKENS);
        let threshold = ContextBudget::new(
            self.context_window,
            self.reserved_output_tokens,
            self.reserved_tool_tokens,
            buffer,
        )
        .compaction_threshold;
        if let Some(pct) = pct_override {
            if pct > 0.0 && pct <= 100.0 {
                let percentage = (effective as f64 * (pct as f64 / 100.0)).floor() as usize;
                return percentage.min(threshold);
            }
        }
        threshold
    }
}

const fn ratio(value: usize, percent: usize) -> usize {
    value.saturating_mul(percent) / 100
}

#[cfg(test)]
mod tests {
    use super::*;
    use deepagent_models::{CapabilitySource, ModelCapability};

    fn cap() -> ModelCapability {
        ModelCapability {
            model_id: "deepseek-v4-flash".into(),
            context_window: 1_000_000,
            max_output_tokens: 384_000,
            supports_tools: true,
            supports_thinking: true,
            supports_json_output: true,
            capability_source: CapabilitySource::BundledOfficialSnapshot,
            fallback_reason: None,
        }
    }

    #[test]
    fn deepseek_policy_uses_provider_window_without_model_name_cap() {
        let policy = ContextPolicy::for_capability(&cap(), ThinkingDepth::Medium);
        assert_eq!(policy.context_window, 1_000_000);
        assert_eq!(policy.reserved_output_tokens, 384_000);
        assert_eq!(policy.reserved_tool_tokens, 62_500);
        assert_eq!(policy.prompt_budget, 553_500);
        assert_eq!(policy.auto_compact_at, 550_464);
    }

    #[test]
    fn output_budget_uses_request_ceiling_not_reasoning_effort() {
        let medium =
            ContextPolicy::for_capability_with_output(&cap(), ThinkingDepth::Medium, Some(256_000));
        let deep =
            ContextPolicy::for_capability_with_output(&cap(), ThinkingDepth::Deep, Some(256_000));
        assert_eq!(medium.reserved_output_tokens, deep.reserved_output_tokens);
        assert_eq!(medium.prompt_budget, deep.prompt_budget);
    }

    #[test]
    fn autocompact_threshold_reserves_actual_request_output() {
        let policy = ContextPolicy::for_capability(&cap(), ThinkingDepth::Medium);
        assert_eq!(policy.effective_context_window(), 616_000);
        // threshold = effective − 65,536 Harness-style headroom.
        assert_eq!(policy.autocompact_threshold_tokens(None, None), 550_464);
        // Custom reserve wins over the default buffer.
        assert_eq!(
            policy.autocompact_threshold_tokens(Some(50_000), None),
            566_000
        );
    }

    #[test]
    fn autocompact_pct_override_is_min_combined_with_default() {
        let policy = ContextPolicy::for_capability(&cap(), ThinkingDepth::Medium);
        // 50% of usable message capacity is lower than the normal trigger.
        assert_eq!(
            policy.autocompact_threshold_tokens(None, Some(50.0)),
            308_000
        );
        // An override cannot raise the normal trigger.
        assert_eq!(
            policy.autocompact_threshold_tokens(None, Some(99.0)),
            550_464
        );
        // Invalid override values are ignored.
        assert_eq!(
            policy.autocompact_threshold_tokens(None, Some(0.0)),
            550_464
        );
        assert_eq!(
            policy.autocompact_threshold_tokens(None, Some(150.0)),
            550_464
        );
    }

    #[test]
    fn small_window_threshold_never_underflows() {
        let mut small = cap();
        small.context_window = 16_000;
        small.max_output_tokens = 8_000;
        let policy = ContextPolicy::for_capability(&small, ThinkingDepth::Simple);
        // effective = 16_000 − 8_000 = 8_000; threshold saturates at 0.
        assert_eq!(policy.effective_context_window(), 8_000);
        assert_eq!(policy.autocompact_threshold_tokens(None, None), 0);
    }

    #[test]
    fn one_million_window_with_256k_output_matches_harness_pressure_budget() {
        let policy =
            ContextPolicy::for_capability_with_output(&cap(), ThinkingDepth::Medium, Some(256_000));
        assert_eq!(policy.autocompact_threshold_tokens(None, None), 678_464);
        assert_eq!(
            policy.prompt_budget,
            policy.prompt_budget().prompt_allowance()
        );
        let mut alias = cap();
        alias.model_id = "deepseek-flash".into();
        let alias_policy =
            ContextPolicy::for_capability_with_output(&alias, ThinkingDepth::Medium, Some(256_000));
        assert_eq!(alias_policy.auto_compact_at, policy.auto_compact_at);
    }
}
