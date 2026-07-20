//! Dynamic context budgeting policy.

use serde::{Deserialize, Serialize};

use deepagent_models::{ModelCapability, ThinkingDepth};

use crate::budget::PromptBudget;

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
}
