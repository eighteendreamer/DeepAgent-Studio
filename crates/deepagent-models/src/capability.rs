//! Model capability resolution.
//!
//! Provider metadata wins when present. Missing metadata is resolved here,
//! through the existing official snapshot or conservative fallback; callers
//! must not independently infer image support from model-name substrings.

use serde::{Deserialize, Serialize};

use deepagent_core::error::{CoreError, Result};

use crate::discovery::{
    looks_like_high_context_model_id, looks_like_reasoner_model_id, ModelEffortMetadata, ModelInfo,
};
use crate::ReasoningEffort;

/// Where a model capability came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilitySource {
    /// Capability fields were supplied by the live provider response.
    ProviderMetadata,
    /// Capability fields came from the bundled official documentation snapshot.
    BundledOfficialSnapshot,
    /// Capability fields came from a user override.
    UserOverride,
    /// Capability was not known; conservative defaults were used.
    ConservativeFallback,
    /// Context and output capacities have different sources, detailed in fallback_reason.
    Mixed,
}

/// Resolved model capability used by context budgeting.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelCapability {
    pub model_id: String,
    pub context_window: usize,
    pub max_output_tokens: usize,
    pub supports_tools: bool,
    pub supports_thinking: bool,
    pub supports_json_output: bool,
    /// Effective input types, independently of context-capacity metadata.
    /// None means unknown, not implicit permission to send images.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_modalities: Option<Vec<String>>,
    /// Raw provider declaration, retained independently of capacity provenance.
    /// Missing means unknown; an empty or unrecognized list is not unrestricted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<ModelEffortMetadata>,
    /// Provenance of the existing context/output budgeting fields. Input
    /// modalities are independently merged from metadata or the exact snapshot.
    pub capability_source: CapabilitySource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback_reason: Option<String>,
}

impl ModelCapability {
    /// Resolve Responses effort without silently downgrading an explicit value.
    /// `none` is the independent thinking-off switch, not a supported-level entry.
    pub fn resolve_reasoning_effort(
        &self,
        requested: Option<ReasoningEffort>,
    ) -> Result<Option<ReasoningEffort>> {
        let Some(metadata) = self.effort.as_ref() else {
            return Ok(requested);
        };
        if requested == Some(ReasoningEffort::None) {
            return Ok(requested);
        }
        let invalid_metadata = || {
            CoreError::invalid(format!(
                "model '{}' has invalid reasoning effort metadata",
                self.model_id
            ))
        };
        if metadata.supported_levels.is_empty()
            || metadata
                .supported_levels
                .iter()
                .any(|level| ReasoningEffort::parse_responses(level) == Some(ReasoningEffort::None))
        {
            return Err(invalid_metadata());
        }
        let supports = |effort| {
            metadata
                .supported_levels
                .iter()
                .any(|level| ReasoningEffort::parse_responses(level) == Some(effort))
        };
        let default = metadata
            .default_level
            .as_deref()
            .map(|level| {
                ReasoningEffort::parse_responses(level)
                    .filter(|effort| *effort != ReasoningEffort::None && supports(*effort))
                    .ok_or_else(invalid_metadata)
            })
            .transpose()?;
        let effort = requested.or(default);
        if let Some(effort) = effort {
            if !supports(effort) {
                return Err(CoreError::invalid(format!(
                    "model '{}' does not support reasoning effort '{}'",
                    self.model_id,
                    effort.label()
                )));
            }
        }
        Ok(effort)
    }

    /// A requested upper bound cannot exceed either provider output capacity
    /// or the whole context window. This does not invent a request default.
    pub fn bound_output_tokens(&self, requested: usize) -> usize {
        requested
            .min(self.max_output_tokens)
            .min(self.context_window)
    }
}

/// Resolves provider model ids into budgeting capabilities.
#[derive(Debug, Clone)]
pub struct ModelCapabilityResolver {
    fallback_context_window: usize,
    fallback_max_output_tokens: usize,
}

impl Default for ModelCapabilityResolver {
    fn default() -> Self {
        Self {
            fallback_context_window: 128_000,
            fallback_max_output_tokens: 32_000,
        }
    }
}

impl ModelCapabilityResolver {
    pub fn new() -> Self {
        Self::default()
    }

    /// Resolve by id when only the selected model name is available.
    pub fn resolve_model_id(&self, model_id: impl AsRef<str>) -> ModelCapability {
        let model_id = model_id.as_ref();
        let mut capability = self
            .official_snapshot_capability(model_id)
            .unwrap_or_else(|| self.fallback(model_id));
        capability.input_modalities = official_input_modalities(model_id);
        capability
    }

    /// Resolve from a provider model entry. Future `/models` extensions can
    /// fill the optional fields on [`ModelInfo`] and automatically win.
    pub fn resolve_model(&self, model: &ModelInfo) -> ModelCapability {
        let mut capability = self.resolve_model_id(&model.id);
        let fallback_source = match capability.capability_source {
            CapabilitySource::BundledOfficialSnapshot => "bundled_official_snapshot",
            _ => "conservative_fallback",
        };
        // Preserve a supplied zero instead of treating it as absent. On narrow
        // platforms only cap unrepresentable values; never wrap them to a smaller
        // unrelated number. The raw u64 declaration remains in ModelCatalog.
        let platform_capacity = |value| usize::try_from(value).unwrap_or(usize::MAX);
        if let Some(context) = model.context_window {
            capability.context_window = platform_capacity(context);
            // Preserve the existing feature heuristic; this is not capacity provenance.
            capability.supports_thinking = looks_like_high_context_model_id(&model.id)
                || looks_like_reasoner_model_id(&model.id)
                || model.id.contains("reason");
        }
        if let Some(output) = model.max_output_tokens {
            capability.max_output_tokens = platform_capacity(output);
        }
        match (model.context_window, model.max_output_tokens) {
            (Some(_), Some(_)) => {
                capability.capability_source = CapabilitySource::ProviderMetadata;
                capability.fallback_reason = None;
            }
            (Some(_), None) | (None, Some(_)) => {
                let context_source = if model.context_window.is_some() {
                    "provider_metadata"
                } else {
                    fallback_source
                };
                let output_source = if model.max_output_tokens.is_some() {
                    "provider_metadata"
                } else {
                    fallback_source
                };
                capability.capability_source = CapabilitySource::Mixed;
                capability.fallback_reason = Some(format!(
                    "missing provider capacity field; context_window={context_source}; max_output_tokens={output_source}"
                ));
            }
            (None, None) => {}
        }
        capability.input_modalities = model
            .input_modalities
            .clone()
            .or_else(|| official_input_modalities(&model.id));
        capability.effort = model.effort.clone();
        if let Some(effort) = &model.effort {
            capability.supports_thinking = !effort.supported_levels.is_empty();
        }
        capability
    }

    fn official_snapshot_capability(&self, model_id: &str) -> Option<ModelCapability> {
        if !looks_like_high_context_model_id(model_id) {
            return None;
        }
        Some(ModelCapability {
            input_modalities: None,
            effort: None,
            model_id: model_id.to_string(),
            context_window: 1_000_000,
            max_output_tokens: 384_000,
            supports_tools: true,
            supports_thinking: true,
            supports_json_output: true,
            capability_source: CapabilitySource::BundledOfficialSnapshot,
            fallback_reason: None,
        })
    }

    fn fallback(&self, model_id: &str) -> ModelCapability {
        ModelCapability {
            input_modalities: None,
            effort: None,
            model_id: model_id.to_string(),
            context_window: self.fallback_context_window,
            max_output_tokens: self.fallback_max_output_tokens,
            supports_tools: true,
            supports_thinking: false,
            supports_json_output: true,
            capability_source: CapabilitySource::ConservativeFallback,
            fallback_reason: Some(
                "model capability is not published by /models and no official snapshot matched"
                    .to_string(),
            ),
        }
    }
}

/// Exact official declarations, checked 2026-11-06 against /api/list-models/
/// and /guides/vision/. Do not extend capacity-name heuristics to modalities.
fn official_input_modalities(model_id: &str) -> Option<Vec<String>> {
    let values: &[&str] = match model_id {
        "deepseek-flash" | "deepseek-v4-flash-vision-exp" => &["text", "image"],
        "deepseek-v4-pro" => &["text"],
        _ => return None,
    };
    Some(values.iter().map(|value| (*value).to_owned()).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deepseek_high_context_model_uses_official_snapshot() {
        let cap = ModelCapabilityResolver::new().resolve_model_id("deepseek-v4-flash");
        assert_eq!(cap.context_window, 1_000_000);
        assert_eq!(cap.max_output_tokens, 384_000);
        assert_eq!(
            cap.capability_source,
            CapabilitySource::BundledOfficialSnapshot
        );
    }

    #[test]
    fn provider_metadata_wins_when_present() {
        let model = ModelInfo {
            input_modalities: None,
            effort: None,
            id: "future-model".into(),
            object: "model".into(),
            owned_by: "deepseek".into(),
            context_window: Some(2_000_000),
            max_output_tokens: Some(500_000),
        };
        let cap = ModelCapabilityResolver::new().resolve_model(&model);
        assert_eq!(cap.context_window, 2_000_000);
        assert_eq!(cap.max_output_tokens, 500_000);
        assert_eq!(cap.capability_source, CapabilitySource::ProviderMetadata);
    }

    #[test]
    fn h296_capacity_fields_merge_independently_and_describe_mixed_sources() {
        for (id, window, output, base_source) in [
            (
                "deepseek-flash",
                1_000_000,
                384_000,
                "bundled_official_snapshot",
            ),
            (
                "audit-capacity-model",
                128_000,
                32_000,
                "conservative_fallback",
            ),
        ] {
            for context in [None, Some(2_000_000)] {
                for completion in [None, Some(8_192)] {
                    let model: ModelInfo = serde_json::from_value(serde_json::json!({
                        "id":id,"context_window":context,"max_output_tokens":completion,
                        "input_modalities":["text"],
                        "effort":{"supported_levels":["low"],"default_level":"low"}
                    }))
                    .unwrap();
                    let cap = ModelCapabilityResolver::new().resolve_model(&model);
                    assert_eq!(cap.context_window, context.unwrap_or(window) as usize);
                    assert_eq!(cap.max_output_tokens, completion.unwrap_or(output) as usize);
                    let source = match (context, completion) {
                        (None, None) => base_source,
                        (Some(_), Some(_)) => "provider_metadata",
                        _ => "mixed",
                    };
                    assert_eq!(serde_json::to_value(cap.capability_source).unwrap(), source);
                    if context.is_some() != completion.is_some() {
                        let reason = cap
                            .fallback_reason
                            .as_deref()
                            .expect("mixed field provenance");
                        assert!(reason.contains(base_source));
                        assert!(reason.contains("context_window"));
                        assert!(reason.contains("max_output_tokens"));
                        assert!(reason.contains("provider_metadata"));
                    }
                    assert_eq!(cap.input_modalities, model.input_modalities);
                    assert_eq!(cap.effort, model.effort);
                    let restored: ModelCapability =
                        serde_json::from_value(serde_json::to_value(&cap).unwrap()).unwrap();
                    assert_eq!(restored, cap);
                }
            }
        }
    }

    #[test]
    fn h296_present_zero_is_not_absence_and_combined_capacity_stays_bounded() {
        for (context, output) in [
            (None, Some(0)),
            (Some(0), None),
            (Some(4_096), Some(8_192)),
            (Some(u64::MAX), Some(u64::MAX)),
        ] {
            let model: ModelInfo = serde_json::from_value(serde_json::json!({
                "id":"audit-capacity-model","context_window":context,"max_output_tokens":output
            }))
            .unwrap();
            let cap = ModelCapabilityResolver::new().resolve_model(&model);
            let platform = |value| usize::try_from(value).unwrap_or(usize::MAX);
            assert_eq!(cap.context_window, platform(context.unwrap_or(128_000)));
            assert_eq!(cap.max_output_tokens, platform(output.unwrap_or(32_000)));
            assert_eq!(
                cap.bound_output_tokens(65_536),
                65_536.min(cap.context_window).min(cap.max_output_tokens)
            );
        }
    }

    #[test]
    fn reasoning_metadata_preserves_unknown_levels_and_normalizes_supported_aliases() {
        let model = ModelInfo {
            id: "limited".into(),
            object: "model".into(),
            owned_by: "deepseek".into(),
            context_window: None,
            max_output_tokens: None,
            input_modalities: None,
            effort: Some(crate::discovery::ModelEffortMetadata {
                supported_levels: vec!["minimal".into(), "xhigh".into(), "mystery".into()],
                default_level: Some("minimal".into()),
            }),
        };
        let capability = ModelCapabilityResolver::new().resolve_model(&model);
        assert_eq!(capability.effort, model.effort);
        assert_eq!(
            capability.resolve_reasoning_effort(None).unwrap(),
            Some(ReasoningEffort::Low)
        );
        assert_eq!(
            capability
                .resolve_reasoning_effort(Some(ReasoningEffort::High))
                .unwrap(),
            Some(ReasoningEffort::High)
        );
        assert!(capability
            .resolve_reasoning_effort(Some(ReasoningEffort::Max))
            .is_err());
    }

    #[test]
    fn reasoning_metadata_does_not_turn_invalid_or_empty_declarations_into_unknown_support() {
        for metadata in [
            ModelEffortMetadata {
                supported_levels: vec![],
                default_level: None,
            },
            ModelEffortMetadata {
                supported_levels: vec!["low".into()],
                default_level: Some("high".into()),
            },
            ModelEffortMetadata {
                supported_levels: vec!["future".into()],
                default_level: Some("future".into()),
            },
            ModelEffortMetadata {
                supported_levels: vec!["none".into()],
                default_level: None,
            },
        ] {
            let mut capability = ModelCapabilityResolver::new().resolve_model_id("limited");
            capability.effort = Some(metadata);
            let restored: ModelCapability =
                serde_json::from_value(serde_json::to_value(&capability).unwrap()).unwrap();
            assert_eq!(restored, capability);
            for request in [None, Some(ReasoningEffort::Low)] {
                assert!(restored.resolve_reasoning_effort(request).is_err());
            }
            assert_eq!(
                restored
                    .resolve_reasoning_effort(Some(ReasoningEffort::None))
                    .unwrap(),
                Some(ReasoningEffort::None)
            );
        }
    }

    #[test]
    fn missing_effort_metadata_is_compatible_but_missing_required_levels_is_not() {
        let capability = ModelCapabilityResolver::new().resolve_model_id("unknown");
        assert_eq!(capability.resolve_reasoning_effort(None).unwrap(), None);
        assert_eq!(
            capability
                .resolve_reasoning_effort(Some(ReasoningEffort::High))
                .unwrap(),
            Some(ReasoningEffort::High)
        );
        assert!(
            serde_json::from_value::<ModelEffortMetadata>(serde_json::json!({
                "default_level": "low"
            }))
            .is_err()
        );
        let mut capability = capability;
        capability.effort = Some(ModelEffortMetadata {
            supported_levels: vec!["low".into()],
            default_level: None,
        });
        assert_eq!(capability.resolve_reasoning_effort(None).unwrap(), None);
        assert!(capability
            .resolve_reasoning_effort(Some(ReasoningEffort::High))
            .is_err());
    }

    #[test]
    fn unknown_model_uses_conservative_fallback() {
        let cap = ModelCapabilityResolver::new().resolve_model_id("custom-model");
        assert_eq!(cap.context_window, 128_000);
        assert_eq!(cap.max_output_tokens, 32_000);
        assert_eq!(
            cap.capability_source,
            CapabilitySource::ConservativeFallback
        );
        assert!(cap.fallback_reason.is_some());
    }
}
