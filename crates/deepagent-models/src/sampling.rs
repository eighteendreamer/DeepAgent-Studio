//! DeepSeek Responses top_p, resolved at the existing provider boundary.
//! Rules checked against /api/create-response/ and /guides/responses_api/
//! on 2026-11-13. This is a documented effective value, not measured sampling.
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::ReasoningEffort;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SamplingReason {
    FlashThinkingFloor,
    FlashNonThinkingFixed,
    ProThinkingToolsFixed,
    ProThinkingNoToolsFixed,
    ProNonThinkingRequested,
    UnpublishedModel,
    UnvalidatedInput,
}

/// Safe, credential/content-free evidence. Never serialized into provider JSON.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EffectiveSamplingConfig {
    pub configured_top_p: Option<f32>,
    pub effective_top_p: Option<f32>,
    /// Preserve omission when no top_p was configured.
    pub wire_top_p: Option<f32>,
    pub thinking_enabled: bool,
    pub tools_enabled: bool,
    pub reason: SamplingReason,
}

pub fn resolve_responses_sampling(
    model: &str,
    configured_top_p: Option<f32>,
    effort: Option<ReasoningEffort>,
    has_supported_tools: bool,
    tool_choice: Option<&Value>,
) -> EffectiveSamplingConfig {
    let thinking_enabled = effort != Some(ReasoningEffort::None);
    let tools_enabled = has_supported_tools && tool_choice.and_then(Value::as_str) != Some("none");
    let requested = configured_top_p.unwrap_or(1.0);
    let (effective_top_p, reason) = if !(0.0..=1.0).contains(&requested) {
        // H-172 owns numeric bounds. Do not turn invalid public input into a
        // seemingly valid request or claim knowledge of the server's behavior.
        (None, SamplingReason::UnvalidatedInput)
    } else {
        match (model, thinking_enabled, tools_enabled) {
            ("deepseek-flash", true, _) => (
                Some(requested.max(0.95)),
                SamplingReason::FlashThinkingFloor,
            ),
            ("deepseek-flash", false, _) => (Some(1.0), SamplingReason::FlashNonThinkingFixed),
            ("deepseek-v4-pro", true, true) => (Some(0.95), SamplingReason::ProThinkingToolsFixed),
            ("deepseek-v4-pro", true, false) => {
                (Some(1.0), SamplingReason::ProThinkingNoToolsFixed)
            }
            ("deepseek-v4-pro", false, _) => {
                (Some(requested), SamplingReason::ProNonThinkingRequested)
            }
            _ => (None, SamplingReason::UnpublishedModel),
        }
    };
    EffectiveSamplingConfig {
        configured_top_p,
        effective_top_p,
        wire_top_p: configured_top_p.map(|value| effective_top_p.unwrap_or(value)),
        thinking_enabled,
        tools_enabled,
        reason,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn omission_unknown_ids_and_invalid_values_are_not_invented_or_repaired() {
        let omitted = resolve_responses_sampling("deepseek-v4-pro", None, None, true, None);
        assert_eq!(omitted.effective_top_p, Some(0.95));
        assert_eq!(omitted.wire_top_p, None);
        for model in ["custom-flash", "deepseek-v4-flash", "DEEPSEEK-FLASH"] {
            let resolved = resolve_responses_sampling(model, Some(0.2), None, true, None);
            assert_eq!(resolved.reason, SamplingReason::UnpublishedModel);
            assert_eq!(resolved.effective_top_p, None);
            assert_eq!(resolved.wire_top_p, Some(0.2));
        }
        for invalid in [-0.1, 1.1, f32::INFINITY, f32::NAN] {
            let resolved =
                resolve_responses_sampling("deepseek-flash", Some(invalid), None, false, None);
            assert_eq!(resolved.reason, SamplingReason::UnvalidatedInput);
            assert_eq!(resolved.effective_top_p, None);
            assert_eq!(resolved.wire_top_p.unwrap().to_bits(), invalid.to_bits());
        }
    }

    #[test]
    fn none_disables_tools_and_metadata_roundtrips_without_provider_fields() {
        let resolved = resolve_responses_sampling(
            "deepseek-v4-pro",
            Some(0.2),
            None,
            true,
            Some(&Value::String("none".into())),
        );
        assert!(!resolved.tools_enabled);
        assert_eq!(resolved.wire_top_p, Some(1.0));
        assert_eq!(resolved.reason, SamplingReason::ProThinkingNoToolsFixed);
        let json = serde_json::to_string(&resolved).unwrap();
        assert_eq!(
            serde_json::from_str::<EffectiveSamplingConfig>(&json).unwrap(),
            resolved
        );
    }
}
