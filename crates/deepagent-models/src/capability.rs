//! Model capability resolution.
//!
//! DeepSeek's `/models` endpoint currently exposes the model ids but not the
//! context window / max-output fields. This module keeps that fact contained in
//! one place: callers still use dynamic model discovery for "what exists", then
//! resolve the discovered id through a small, versioned official-doc snapshot.

use serde::{Deserialize, Serialize};

use crate::discovery::ModelInfo;

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
    pub capability_source: CapabilitySource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback_reason: Option<String>,
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
        self.provider_metadata_capability(model_id, None, None)
            .or_else(|| self.official_snapshot_capability(model_id))
            .unwrap_or_else(|| self.fallback(model_id))
    }

    /// Resolve from a provider model entry. Future `/models` extensions can
    /// fill the optional fields on [`ModelInfo`] and automatically win.
    pub fn resolve_model(&self, model: &ModelInfo) -> ModelCapability {
        self.provider_metadata_capability(
            &model.id,
            model.context_window.map(|v| v as usize),
            model.max_output_tokens.map(|v| v as usize),
        )
        .or_else(|| self.official_snapshot_capability(&model.id))
        .unwrap_or_else(|| self.fallback(&model.id))
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
            model_id: model_id.to_string(),
            context_window,
            max_output_tokens,
            supports_tools: true,
            supports_thinking: is_deepseek_v4(model_id) || model_id.contains("reason"),
            supports_json_output: true,
            capability_source: CapabilitySource::ProviderMetadata,
            fallback_reason: None,
        })
    }

    fn official_snapshot_capability(&self, model_id: &str) -> Option<ModelCapability> {
        if !is_deepseek_v4(model_id) {
            return None;
        }
        Some(ModelCapability {
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

fn is_deepseek_v4(model_id: &str) -> bool {
    matches!(model_id, "deepseek-v4-flash" | "deepseek-v4-pro")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deepseek_v4_uses_official_snapshot() {
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
