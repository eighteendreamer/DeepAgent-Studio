//! Model discovery & catalog.
//!
//! The system hard-codes the DeepSeek base URL (`https://api.deepseek.com`,
//! OpenAI-compatible). At project initialization the user supplies **only an API
//! key**; the system then calls `GET /models`, auto-selects the latest models,
//! and produces a [`ModelCatalog`] — the model configuration that is the key to
//! all subsequent usability (which model the runtime, planner, sub-agents, etc.
//! actually call).
//!
//! Discovery is transport-agnostic (works through any [`HttpTransport`]), so it
//! is fully testable offline via the mock transport's canned `get_json`.

use std::cmp::Reverse;
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
}

/// The raw `GET /models` response envelope.
#[derive(Debug, Clone, Deserialize)]
struct ModelsResponse {
    #[serde(default)]
    data: Vec<ModelInfo>,
}

/// The capability role a model is assigned to in the catalog.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelRole {
    /// Default chat / tool-use model (fast, general).
    Chat,
    /// Reasoning model (Thinking Mode, deep tasks).
    Reasoner,
}

/// The persisted model configuration: which discovered models fill each role.
/// This is what the rest of the system reads to decide which model to call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelCatalog {
    /// The base URL (always DeepSeek; stored for transparency / future override).
    pub base_url: String,
    /// All models discovered from the provider.
    pub available: Vec<ModelInfo>,
    /// The selected default chat model id.
    pub chat_model: String,
    /// The selected reasoning model id (falls back to chat if none found).
    pub reasoner_model: String,
}

impl ModelCatalog {
    /// Resolve the model id for a given role.
    pub fn model_for(&self, role: ModelRole) -> &str {
        match role {
            ModelRole::Chat => &self.chat_model,
            ModelRole::Reasoner => &self.reasoner_model,
        }
    }

    /// Whether a model id exists in the discovered set.
    pub fn has_model(&self, id: &str) -> bool {
        self.available.iter().any(|m| m.id == id)
    }

    /// Build a catalog by auto-selecting from a discovered model list.
    ///
    /// Selection heuristic (no network): score discovered model ids by the
    /// role they most likely serve, ignore deprecated compatibility aliases,
    /// and fall back to the first usable model when the provider names do not
    /// expose a clear chat/reasoner split.
    pub fn auto_select(base_url: impl Into<String>, available: Vec<ModelInfo>) -> Result<Self> {
        let available: Vec<ModelInfo> = available
            .into_iter()
            .filter(|m| !is_deprecated_model_id(&m.id))
            .collect();
        if available.is_empty() {
            return Err(CoreError::other(
                "model discovery returned no usable DeepSeek models",
            ));
        }

        let chat = select_role_model(&available, ModelRole::Chat)
            .or_else(|| available.first())
            .map(|m| m.id.clone())
            .ok_or_else(|| {
                CoreError::other("model discovery returned no usable DeepSeek models")
            })?;
        let reasoner_model = select_role_model(&available, ModelRole::Reasoner)
            .map(|m| m.id.clone())
            .unwrap_or_else(|| chat.clone());

        Ok(Self {
            base_url: base_url.into(),
            available,
            chat_model: chat,
            reasoner_model,
        })
    }
}

fn select_role_model(models: &[ModelInfo], role: ModelRole) -> Option<&ModelInfo> {
    models
        .iter()
        .enumerate()
        .max_by_key(|(idx, model)| {
            let score = role_score(&model.id, role, model);
            (score, Reverse(*idx))
        })
        .map(|(_, model)| model)
}

fn role_score(model_id: &str, role: ModelRole, model: &ModelInfo) -> i64 {
    let lower = model_id.to_ascii_lowercase();
    let tokens = model_id_tokens(model_id);
    let mut score = 0i64;

    match role {
        ModelRole::Chat => {
            score += token_bonus(&tokens, &["chat"], 100);
            score += token_bonus(&tokens, &["flash"], 80);
            score += token_bonus(&tokens, &["mini", "small", "lite"], 50);
            score += token_bonus(&tokens, &["base", "general", "default"], 30);
            score -= token_bonus(&tokens, &["reason", "reasoner", "think", "thinking"], 120);
            score -= token_bonus(&tokens, &["pro"], 60);
        }
        ModelRole::Reasoner => {
            score += token_bonus(&tokens, &["reason", "reasoner"], 120);
            score += token_bonus(&tokens, &["think", "thinking"], 100);
            score += token_bonus(&tokens, &["pro"], 90);
            score += token_bonus(&tokens, &["deep"], 20);
            score += token_bonus(&tokens, &["r1", "r2", "o1", "o3"], 40);
            score -= token_bonus(&tokens, &["chat"], 110);
            score -= token_bonus(&tokens, &["flash"], 60);
            score -= token_bonus(&tokens, &["mini", "small", "lite"], 40);
        }
    }

    if let Some(context_window) = model.context_window {
        score += (context_window / 100_000).min(10) as i64;
    }
    if let Some(max_output_tokens) = model.max_output_tokens {
        score += (max_output_tokens / 50_000).min(5) as i64;
    }

    if lower.contains("deepseek") {
        score += 3;
    }

    score
}

fn token_bonus(tokens: &[String], needles: &[&str], weight: i64) -> i64 {
    if needles
        .iter()
        .any(|needle| tokens.iter().any(|token| token == needle))
    {
        weight
    } else {
        0
    }
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

fn is_deprecated_model_id(id: &str) -> bool {
    matches!(id, "deepseek-chat" | "deepseek-reasoner")
}

/// Discovers models from a provider and builds an auto-selected catalog.
pub struct ModelDiscovery {
    transport: Arc<dyn HttpTransport>,
    base_url: String,
}

impl ModelDiscovery {
    /// Build a discovery client against the hard-coded DeepSeek base URL.
    pub fn deepseek(transport: Arc<dyn HttpTransport>) -> Self {
        Self {
            transport,
            base_url: DEEPSEEK_BASE_URL.to_string(),
        }
    }

    /// Build against a custom base URL (advanced / testing).
    pub fn with_base_url(transport: Arc<dyn HttpTransport>, base_url: impl Into<String>) -> Self {
        Self {
            transport,
            base_url: base_url.into(),
        }
    }

    /// The fully-qualified `/models` endpoint.
    pub fn models_endpoint(&self) -> String {
        format!("{}{}", self.base_url.trim_end_matches('/'), MODELS_PATH)
    }

    /// Query `GET /models` with the user's `api_key` and return the raw list.
    pub async fn list_models(&self, api_key: &str) -> Result<Vec<ModelInfo>> {
        let body = self
            .transport
            .get_json(&self.models_endpoint(), api_key)
            .await?;
        let parsed: ModelsResponse = serde_json::from_str(&body)
            .map_err(|e| CoreError::Serialization(format!("bad /models response: {e}")))?;
        Ok(parsed.data)
    }

    /// Discover models and auto-select the catalog — the one call the project
    /// init flow makes after the user enters their API key.
    pub async fn discover(&self, api_key: &str) -> Result<ModelCatalog> {
        let models = self.list_models(api_key).await?;
        ModelCatalog::auto_select(self.base_url.clone(), models)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::MockTransport;

    fn models(ids: &[&str]) -> Vec<ModelInfo> {
        ids.iter()
            .map(|id| ModelInfo {
                id: (*id).to_string(),
                object: "model".into(),
                owned_by: "deepseek".into(),
                context_window: None,
                max_output_tokens: None,
            })
            .collect()
    }

    #[test]
    fn base_url_is_deepseek() {
        assert_eq!(DEEPSEEK_BASE_URL, "https://api.deepseek.com");
    }

    #[test]
    fn auto_select_prefers_known_ids() {
        let cat = ModelCatalog::auto_select(
            DEEPSEEK_BASE_URL,
            models(&["deepseek-v4-flash", "deepseek-v4-pro"]),
        )
        .unwrap();
        assert_eq!(cat.chat_model, "deepseek-v4-flash");
        assert_eq!(cat.reasoner_model, "deepseek-v4-pro");
        assert_eq!(cat.model_for(ModelRole::Reasoner), "deepseek-v4-pro");
    }

    #[test]
    fn auto_select_falls_back_by_substring() {
        let cat = ModelCatalog::auto_select(
            DEEPSEEK_BASE_URL,
            models(&["provider-v4-flash-latest", "provider-v4-pro-latest"]),
        )
        .unwrap();
        assert_eq!(cat.chat_model, "provider-v4-flash-latest");
        assert_eq!(cat.reasoner_model, "provider-v4-pro-latest");
    }

    #[test]
    fn auto_select_uses_role_hints_without_exact_ids() {
        let cat = ModelCatalog::auto_select(
            DEEPSEEK_BASE_URL,
            models(&[
                "deepseek-umbrella",
                "deepseek-brainy-pro",
                "deepseek-fast-flash",
            ]),
        )
        .unwrap();
        assert_eq!(cat.chat_model, "deepseek-fast-flash");
        assert_eq!(cat.reasoner_model, "deepseek-brainy-pro");
    }

    #[test]
    fn deprecated_aliases_are_ignored() {
        assert!(ModelCatalog::auto_select(
            DEEPSEEK_BASE_URL,
            models(&["deepseek-chat", "deepseek-reasoner"])
        )
        .is_err());
    }

    #[test]
    fn single_model_reuses_for_both_roles() {
        let cat =
            ModelCatalog::auto_select(DEEPSEEK_BASE_URL, models(&["deepseek-vision"])).unwrap();
        assert_eq!(cat.chat_model, "deepseek-vision");
        assert_eq!(cat.reasoner_model, "deepseek-vision");
    }

    #[test]
    fn empty_discovery_errors() {
        assert!(ModelCatalog::auto_select(DEEPSEEK_BASE_URL, vec![]).is_err());
    }

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
        let cat = discovery.discover("sk-test").await.unwrap();
        assert_eq!(cat.chat_model, "deepseek-v4-flash");
        assert_eq!(cat.reasoner_model, "deepseek-v4-pro");
        assert_eq!(cat.available.len(), 2);
    }

    #[tokio::test]
    async fn discover_bad_json_errors() {
        let transport = Arc::new(MockTransport::with_get_json("not json"));
        let discovery = ModelDiscovery::deepseek(transport);
        assert!(discovery.discover("sk-test").await.is_err());
    }
}
