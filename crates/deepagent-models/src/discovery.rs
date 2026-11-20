//! Protocol-aware model listing. Discovery returns metadata only; activation
//! and selection belong to the SQLite harness registry.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use deepagent_core::error::{CoreError, Result};

use crate::transport::HttpTransport;

/// The hard-coded DeepSeek base URL (OpenAI-compatible endpoint).
pub const DEEPSEEK_BASE_URL: &str = "https://api.deepseek.com";

/// The `/models` path appended to the base URL.
pub const MODELS_PATH: &str = "/models";

/// One model entry as returned by the OpenAI-compatible `GET /models`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelInfo {
    /// Model id (e.g. `"deepseek-v4-flash"`, `"deepseek-v4-pro"`).
    pub id: String,
    /// Object type (usually `"model"`).
    #[serde(default)]
    pub object: String,
    /// Owner (e.g. `"deepseek"`).
    #[serde(default)]
    pub owned_by: String,
    /// Optional future provider metadata. DeepSeek's current `/models` response
    /// does not include this, but keeping the field here lets live metadata win
    /// automatically if the endpoint is expanded later.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
    /// Optional future provider metadata for max completion/output tokens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u64>,
    /// Provider-declared input types. Missing is unknown; an explicit empty
    /// list must not be replaced with a more permissive bundled declaration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_modalities: Option<Vec<String>>,
    /// Model-level Responses reasoning metadata returned by DeepSeek.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<ModelEffortMetadata>,
}

/// Provider-declared reasoning levels. Keep raw labels for forward-compatible
/// catalog persistence; the capability layer validates known canonical values.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelEffortMetadata {
    pub supported_levels: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_level: Option<String>,
}

/// The raw `GET /models` response envelope.
#[derive(Debug, Clone, Deserialize)]
struct ModelsResponse {
    #[serde(default)]
    data: Vec<ModelInfo>,
}

fn model_id_tokens(model_id: &str) -> Vec<String> {
    model_id
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|part| !part.is_empty())
        .map(|part| part.to_ascii_lowercase())
        .collect()
}

pub(crate) fn looks_like_chat_model_id(model_id: &str) -> bool {
    let tokens = model_id_tokens(model_id);
    has_any_token(
        &tokens,
        &[
            "chat", "flash", "mini", "small", "lite", "base", "general", "default",
        ],
    )
}

pub(crate) fn looks_like_reasoner_model_id(model_id: &str) -> bool {
    let tokens = model_id_tokens(model_id);
    has_any_token(
        &tokens,
        &[
            "reason", "reasoner", "think", "thinking", "pro", "deep", "r1", "r2", "o1", "o3",
        ],
    )
}

pub(crate) fn looks_like_high_context_model_id(model_id: &str) -> bool {
    let lower = model_id.to_ascii_lowercase();
    lower.contains("deepseek")
        && (looks_like_chat_model_id(model_id) || looks_like_reasoner_model_id(model_id))
}

fn has_any_token(tokens: &[String], needles: &[&str]) -> bool {
    needles
        .iter()
        .any(|needle| tokens.iter().any(|token| token == needle))
}

/// Lists models without assigning roles or selecting a default.
pub struct ModelDiscovery {
    transport: Arc<dyn HttpTransport>,
    base_url: String,
    auth: crate::transport::TransportAuth,
}

impl ModelDiscovery {
    /// Build a discovery client against the hard-coded DeepSeek base URL.
    pub fn deepseek(transport: Arc<dyn HttpTransport>) -> Self {
        Self {
            transport,
            base_url: DEEPSEEK_BASE_URL.to_string(),
            auth: crate::transport::TransportAuth::Bearer,
        }
    }

    /// Build against a custom base URL (advanced / testing).
    pub fn with_base_url(transport: Arc<dyn HttpTransport>, base_url: impl Into<String>) -> Self {
        Self {
            transport,
            base_url: base_url.into(),
            auth: crate::transport::TransportAuth::Bearer,
        }
    }

    /// Select the protocol's listing root without changing the configured inference root.
    pub fn with_wire_mode(mut self, wire_mode: crate::WireMode) -> Self {
        self.auth = crate::protocol::codec_for(wire_mode).auth();
        if wire_mode == crate::WireMode::Messages {
            // The official DeepSeek Messages route lists models at its origin,
            // as llm-deepseek::modelListingURL does. Other Anthropic-compatible
            // providers retain their deployment prefix and native `/v1` path.
            let deepseek_messages = url::Url::parse(&self.base_url).is_ok_and(|url| {
                url.origin().ascii_serialization() == DEEPSEEK_BASE_URL
                    && matches!(
                        url.path().trim_end_matches('/'),
                        "/anthropic" | "/anthropic/v1"
                    )
            });
            self.base_url = if deepseek_messages {
                DEEPSEEK_BASE_URL.to_owned()
            } else {
                crate::messages::api_root(&self.base_url)
            };
        }
        self
    }

    /// The fully-qualified `/models` endpoint.
    pub fn models_endpoint(&self) -> String {
        format!("{}{}", self.base_url.trim_end_matches('/'), MODELS_PATH)
    }

    /// Query `GET /models` with the user's `api_key` and return the raw list.
    pub async fn list_models(&self, api_key: &str) -> Result<Vec<ModelInfo>> {
        let body = self
            .transport
            .get_json_authenticated(&self.models_endpoint(), api_key, self.auth)
            .await?;
        let parsed: ModelsResponse = serde_json::from_str(&body)
            .map_err(|e| CoreError::Serialization(format!("bad /models response: {e}")))?;
        Ok(parsed.data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::MockTransport;

    #[tokio::test]
    async fn discover_through_transport() {
        let body = r#"{"object":"list","data":[
            {"id":"deepseek-v4-flash","object":"model","owned_by":"deepseek"},
            {"id":"deepseek-v4-pro","object":"model","owned_by":"deepseek"}
        ]}"#;
        let transport = Arc::new(MockTransport::with_get_json(body));
        let discovery = ModelDiscovery::deepseek(transport);
        assert_eq!(
            discovery.models_endpoint(),
            "https://api.deepseek.com/models"
        );
        let cat = discovery.list_models("sk-test").await.unwrap();
        assert_eq!(cat[0].id, "deepseek-v4-flash");
        assert_eq!(cat[1].id, "deepseek-v4-pro");
        assert_eq!(cat.len(), 2);
    }

    #[tokio::test]
    async fn published_capacity_metadata_survives_discovery_and_catalog_restore() {
        // Shape from DeepSeek's /models documentation (2026-10-22). Extra
        // modality/effort fields must not prevent capacity metadata parsing.
        let body = r#"{"object":"list","data":[{
            "id":"deepseek-flash","object":"model","owned_by":"deepseek",
            "name":"DeepSeek-V4.1-Flash",
            "context_window":1048576,"max_output_tokens":393216,
            "input_modalities":["text","image"],"output_modalities":["text"],
            "effort":{"supported_levels":["low","high","max"],"default_level":"high"},
            "api_capabilities":{"anthropic_messages":{"system_prompt_update":"in-history"}}
        }]}"#;
        let discovery = ModelDiscovery::deepseek(Arc::new(MockTransport::with_get_json(body)));
        let catalog = discovery
            .list_models("offline-audit-placeholder")
            .await
            .unwrap();
        let restored: Vec<ModelInfo> =
            serde_json::from_str(&serde_json::to_string(&catalog).unwrap()).unwrap();
        assert_eq!(restored, catalog);
        assert_eq!(restored[0].context_window, Some(1_048_576));
        assert_eq!(restored[0].max_output_tokens, Some(393_216));
        assert_eq!(
            restored[0].input_modalities,
            Some(vec!["text".into(), "image".into()])
        );
        assert_eq!(
            restored[0].effort,
            Some(ModelEffortMetadata {
                supported_levels: vec!["low".into(), "high".into(), "max".into()],
                default_level: Some("high".into()),
            })
        );
        let capability = crate::ModelCapabilityResolver::new().resolve_model(&restored[0]);
        assert_eq!(capability.context_window, 1_048_576);
        assert_eq!(capability.max_output_tokens, 393_216);
        assert_eq!(
            capability.capability_source,
            crate::CapabilitySource::ProviderMetadata
        );
    }

    #[tokio::test]
    async fn discover_bad_json_errors() {
        let transport = Arc::new(MockTransport::with_get_json("not json"));
        let discovery = ModelDiscovery::deepseek(transport);
        assert!(discovery.list_models("sk-test").await.is_err());
    }
}
