//! Model capability resolution.
//!
//! Provider metadata wins when present. Missing metadata is resolved here,
//! through the existing official snapshot or conservative fallback; callers
//! must not independently infer image support from model-name substrings.

use serde::{Deserialize, Serialize};

use crate::discovery::{looks_like_high_context_model_id, looks_like_reasoner_model_id, ModelInfo};

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
    /// Provenance of the existing context/output budgeting fields. Input
    /// modalities are independently merged from metadata or the exact snapshot.
    pub capability_source: CapabilitySource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback_reason: Option<String>,
}

impl ModelCapability {
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
            .provider_metadata_capability(model_id, None, None)
            .or_else(|| self.official_snapshot_capability(model_id))
            .unwrap_or_else(|| self.fallback(model_id));
        capability.input_modalities = official_input_modalities(model_id);
        capability
    }

    /// Resolve from a provider model entry. Future `/models` extensions can
    /// fill the optional fields on [`ModelInfo`] and automatically win.
    pub fn resolve_model(&self, model: &ModelInfo) -> ModelCapability {
        let mut capability = self
            .provider_metadata_capability(
                &model.id,
                model.context_window.map(|v| v as usize),
                model.max_output_tokens.map(|v| v as usize),
            )
            .or_else(|| self.official_snapshot_capability(&model.id))
            .unwrap_or_else(|| self.fallback(&model.id));
        capability.input_modalities = model
            .input_modalities
            .clone()
            .or_else(|| official_input_modalities(&model.id));
        capability
    }

    fn provider_metadata_capability(
        &self,
        model_id: &str,
        context_window: Option<usize>,
        max_output_tokens: Option<usize>,
    ) -> Option<ModelCapability> {
        let context_window = context_window?;
        let max_output_tokens = max_output_tokens.unwrap_or(self.fallback_max_output_tokens);
        Some(ModelCapability {
            input_modalities: None,
            model_id: model_id.to_string(),
            context_window,
            max_output_tokens,
            supports_tools: true,
            supports_thinking: looks_like_high_context_model_id(model_id)
                || looks_like_reasoner_model_id(model_id)
                || model_id.contains("reason"),
            supports_json_output: true,
            capability_source: CapabilitySource::ProviderMetadata,
            fallback_reason: None,
        })
    }

    fn official_snapshot_capability(&self, model_id: &str) -> Option<ModelCapability> {
        if !looks_like_high_context_model_id(model_id) {
            return None;
        }
        Some(ModelCapability {
            input_modalities: None,
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
