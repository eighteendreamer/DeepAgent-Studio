//! Dynamic context budgeting policy.

use serde::{Deserialize, Serialize};

use deepagent_models::{ModelCapability, ThinkingDepth};

use crate::budget::PromptBudget;

/// Tokens reserved for the compaction summary's own output when computing the
/// effective context window. Aligned with Claude Code
/// `autoCompact.ts::MAX_OUTPUT_TOKENS_FOR_SUMMARY` (20_000, sized from the
/// p99.99 of compact summary outputs).
pub const AUTOCOMPACT_SUMMARY_RESERVE_TOKENS: usize = 20_000;

/// Default buffer subtracted from the effective window to form the proactive
/// auto-compact threshold. Aligned with Claude Code
/// `autoCompact.ts::AUTOCOMPACT_BUFFER_TOKENS` (13_000). Overridable via the
/// `autocompact_reserve_tokens` setting / `DEEPAGENT_AUTOCOMPACT_RESERVE_TOKENS`.
pub const AUTOCOMPACT_BUFFER_TOKENS: usize = 13_000;

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
        let desired_output = match thinking_depth {
            ThinkingDepth::Simple => 64_000,
            ThinkingDepth::Medium => 96_000,
            ThinkingDepth::Deep => 160_000,
        };
        let reserved_output_tokens = desired_output
            .min(capability.max_output_tokens)
            .min(capability.context_window / 2)
            .max(4_000.min(capability.context_window));
        let reserved_tool_tokens = (capability.context_window / 16).clamp(8_000, 64_000);
        let prompt_budget = capability
            .context_window
            .saturating_sub(reserved_output_tokens)
            .saturating_sub(reserved_tool_tokens);

        Self {
            model_id: capability.model_id.clone(),
            context_window: capability.context_window,
            max_output_tokens: capability.max_output_tokens,
            reserved_output_tokens,
            reserved_tool_tokens,
            prompt_budget,
            auto_compact_at: ratio(prompt_budget, 70),
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

    /// The context window minus the output tokens reserved for the compaction
    /// summary. Aligned with Claude Code
    /// `autoCompact.ts::getEffectiveContextWindowSize`: reserve =
    /// `min(model max output, 20_000)`.
    pub fn effective_context_window(&self) -> usize {
        let reserved = self
            .max_output_tokens
            .min(AUTOCOMPACT_SUMMARY_RESERVE_TOKENS);
        self.context_window.saturating_sub(reserved)
    }

    /// Proactive auto-compact threshold in tokens, checked before every model
    /// request. Aligned with Claude Code `autoCompact.ts::getAutoCompactThreshold`:
    /// threshold = effective window − buffer (absolute subtraction, ~93% of the
    /// effective window at CC defaults). `reserve_tokens` overrides the 13k
    /// buffer; `pct_override` (0–100, CC `CLAUDE_AUTOCOMPACT_PCT_OVERRIDE`) is a
    /// testing knob taken as a percentage of the effective window and combined
    /// with the default threshold via `min`.
    pub fn autocompact_threshold_tokens(
        &self,
        reserve_tokens: Option<usize>,
        pct_override: Option<f32>,
    ) -> usize {
        let effective = self.effective_context_window();
        let buffer = reserve_tokens.unwrap_or(AUTOCOMPACT_BUFFER_TOKENS);
        let threshold = effective.saturating_sub(buffer);
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
    fn deepseek_v4_policy_uses_large_window_without_fixed_thresholds() {
        let policy = ContextPolicy::for_capability(&cap(), ThinkingDepth::Medium);
        assert_eq!(policy.context_window, 1_000_000);
        assert_eq!(policy.reserved_output_tokens, 96_000);
        assert_eq!(policy.reserved_tool_tokens, 62_500);
        assert_eq!(policy.prompt_budget, 841_500);
        assert_eq!(policy.auto_compact_at, 589_050);
    }

    #[test]
    fn deep_thinking_reserves_more_output_room() {
        let medium = ContextPolicy::for_capability(&cap(), ThinkingDepth::Medium);
        let deep = ContextPolicy::for_capability(&cap(), ThinkingDepth::Deep);
        assert!(deep.reserved_output_tokens > medium.reserved_output_tokens);
        assert!(deep.prompt_budget < medium.prompt_budget);
    }

    #[test]
    fn autocompact_threshold_matches_claude_code_formula() {
        let policy = ContextPolicy::for_capability(&cap(), ThinkingDepth::Medium);
        // effective = window − min(max_output, 20k) = 1_000_000 − 20_000.
        assert_eq!(policy.effective_context_window(), 980_000);
        // threshold = effective − 13k (CC AUTOCOMPACT_BUFFER_TOKENS).
        assert_eq!(policy.autocompact_threshold_tokens(None, None), 967_000);
        // Custom reserve wins over the default buffer.
        assert_eq!(
            policy.autocompact_threshold_tokens(Some(50_000), None),
            930_000
        );
    }

    #[test]
    fn autocompact_pct_override_is_min_combined_with_default() {
        let policy = ContextPolicy::for_capability(&cap(), ThinkingDepth::Medium);
        // 50% of effective (490_000) < default threshold → percentage wins.
        assert_eq!(
            policy.autocompact_threshold_tokens(None, Some(50.0)),
            490_000
        );
        // 99% of effective (970_200) > default threshold → default wins (min).
        assert_eq!(
            policy.autocompact_threshold_tokens(None, Some(99.0)),
            967_000
        );
        // Invalid override values are ignored.
        assert_eq!(
            policy.autocompact_threshold_tokens(None, Some(0.0)),
            967_000
        );
        assert_eq!(
            policy.autocompact_threshold_tokens(None, Some(150.0)),
            967_000
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
}
