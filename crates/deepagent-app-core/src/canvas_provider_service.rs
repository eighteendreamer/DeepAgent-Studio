//! Infinite-canvas model provider configuration.
//!
//! Single source of truth for canvas providers / models / scenario bindings.
//! Non-secret config lives in [`DocumentStore`] under the `canvas` collection
//! (the same SQLite database the main-window DeepSeek settings use); API keys
//! live in [`SecretStore`] as AES-GCM ciphertext and never leave the backend —
//! every outbound DTO only carries a masked presence flag.
//!
//! Provider wire details are isolated here: [`endpoint_url`] is the one place
//! that maps `(protocol, request kind, base_url)` to a concrete endpoint. The
//! model id always travels in the request body (or, for Gemini, in one
//! percent-encoded path segment), so a model name containing `/` can never be
//! reinterpreted as extra routing.

use std::sync::Arc;

use deepagent_core::clock::{Clock, SystemClock, Timestamp};
use deepagent_core::error::{CoreError, Result};
use deepagent_persistence::document_store::DocumentStore;
use deepagent_persistence::Database;
use serde::{Deserialize, Serialize};

use crate::secret_store::SecretStore;

/// Document-store collection for all canvas configuration documents.
pub const CANVAS_COLLECTION: &str = "canvas";
/// Document id holding the full provider + model set.
pub const CANVAS_PROVIDERS_ID: &str = "providers";
/// Anthropic protocol version sent on `messages` requests.
pub const ANTHROPIC_VERSION: &str = "2023-06-01";

/// Wire protocol a canvas provider endpoint speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CanvasProtocol {
    /// OpenAI-compatible `/chat/completions`, `/embeddings`, `/images/*`, `/audio/*`.
    OpenAi,
    /// Anthropic Messages API.
    Anthropic,
    /// Google Gemini `generateContent` family.
    Gemini,
}

impl CanvasProtocol {
    /// Parse a frontend protocol label. Legacy `deepseek` / `custom` labels map
    /// onto the OpenAI-compatible protocol because both speak that wire format.
    pub fn from_label(label: &str) -> Self {
        match label.trim().to_ascii_lowercase().as_str() {
            "anthropic" => Self::Anthropic,
            "gemini" => Self::Gemini,
            _ => Self::OpenAi,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::OpenAi => "openai",
            Self::Anthropic => "anthropic",
            Self::Gemini => "gemini",
        }
    }
}

/// Model scenario a canvas model is allowed to serve. A scenario only gates
/// whether a model may be selected; it never picks an operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CanvasScenario {
    Text,
    ImageGeneration,
    VideoGeneration,
    SpeechToText,
    TextToSpeech,
    Embedding,
}

impl CanvasScenario {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::ImageGeneration => "image_generation",
            Self::VideoGeneration => "video_generation",
            Self::SpeechToText => "speech_to_text",
            Self::TextToSpeech => "text_to_speech",
            Self::Embedding => "embedding",
        }
    }

    pub fn from_label(label: &str) -> Option<Self> {
        match label.trim().to_ascii_lowercase().as_str() {
            "text" => Some(Self::Text),
            "image_generation" | "image-gen" | "image" => Some(Self::ImageGeneration),
            "video_generation" | "video-gen" | "video" => Some(Self::VideoGeneration),
            "speech_to_text" | "stt" => Some(Self::SpeechToText),
            "text_to_speech" | "tts" => Some(Self::TextToSpeech),
            "embedding" | "embeddings" | "vector" => Some(Self::Embedding),
            _ => None,
        }
    }
}

/// The kind of upstream call being made; drives endpoint selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CanvasRequestKind {
    Chat,
    Embeddings,
    ImageGenerate,
    ImageEdit,
    SpeechTranscribe,
    SpeechSynthesize,
    /// Provider model catalog listing (`GET {base}/models`).
    Models,
}

/// Persisted model entry inside a provider.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CanvasModelConfig {
    /// Provider-scoped model id sent to the provider (may contain `/`).
    pub id: String,
    /// Display name.
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Scenarios this model is bound to. Empty means "not selectable".
    #[serde(default)]
    pub scenarios: Vec<CanvasScenario>,
    /// Tie-break inside a scenario candidate list (higher wins).
    #[serde(default)]
    pub priority: i32,
}

/// Persisted provider entry with its nested model list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CanvasProviderConfig {
    /// Stable opaque id, generated on first save when absent.
    #[serde(default)]
    pub id: String,
    pub name: String,
    pub protocol: CanvasProtocol,
    /// Base URL, e.g. `https://api.siliconflow.cn/v1`. No endpoint suffix.
    pub base_url: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Optional custom icon (data URL, size-capped).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logo: Option<String>,
    #[serde(default)]
    pub models: Vec<CanvasModelConfig>,
}

/// The whole persisted provider set (one document).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CanvasProviderState {
    #[serde(default)]
    pub providers: Vec<CanvasProviderConfig>,
}

/// UI-facing provider DTO. `api_key` is intentionally absent: the secret never
/// crosses the backend boundary, only `api_key_set` / `api_key_masked`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CanvasProviderDto {
    pub id: String,
    pub name: String,
    pub protocol: String,
    pub base_url: String,
    pub enabled: bool,
    pub api_key_set: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key_masked: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logo: Option<String>,
    pub models: Vec<CanvasModelDto>,
}

/// UI-facing model DTO.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CanvasModelDto {
    pub id: String,
    pub name: String,
    pub description: String,
    pub enabled: bool,
    pub scenarios: Vec<String>,
    pub priority: i32,
}

/// Scenario → provider/model binding row. The list order of
/// [`CanvasScenarioBinding`]s for one scenario is the candidate fallback order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CanvasScenarioBinding {
    pub scenario: String,
    pub provider_id: String,
    pub model_id: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CanvasBindingState {
    #[serde(default)]
    pub bindings: Vec<CanvasScenarioBinding>,
}

/// Input for `save_provider`. `api_key` is write-only: `Some(text)` stores it,
/// `None` leaves the existing secret untouched.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CanvasProviderInput {
    #[serde(default)]
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub protocol: String,
    pub base_url: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub models: Vec<CanvasModelConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logo: Option<String>,
    #[serde(default)]
    pub api_key: Option<String>,
}

fn default_true() -> bool {
    true
}

impl CanvasModelConfig {
    fn to_dto(&self) -> CanvasModelDto {
        CanvasModelDto {
            id: self.id.clone(),
            name: self.name.clone(),
            description: self.description.clone(),
            enabled: self.enabled,
            scenarios: self
                .scenarios
                .iter()
                .map(|s| s.as_str().to_string())
                .collect(),
            priority: self.priority,
        }
    }
}

impl CanvasProviderConfig {
    fn to_dto(&self, api_key: Option<&str>) -> CanvasProviderDto {
        CanvasProviderDto {
            id: self.id.clone(),
            name: self.name.clone(),
            protocol: self.protocol.as_str().to_string(),
            base_url: self.base_url.clone(),
            enabled: self.enabled,
            api_key_set: api_key.is_some(),
            api_key_masked: api_key.map(mask_secret),
            logo: self.logo.clone(),
            models: self.models.iter().map(|m| m.to_dto()).collect(),
        }
    }
}

/// Mask a secret for display: keep a short head and tail, never the middle.
pub fn mask_secret(secret: &str) -> String {
    let chars: Vec<char> = secret.chars().collect();
    if chars.len() <= 10 {
        return "*".repeat(chars.len());
    }
    let head: String = chars[..3].iter().collect();
    let tail: String = chars[chars.len() - 4..].iter().collect();
    format!("{head}…{tail}")
}

/// Normalize a base URL: trim whitespace and trailing slashes.
pub fn normalize_base_url(base_url: &str) -> String {
    base_url.trim().trim_end_matches('/').to_string()
}

/// Map `(protocol, kind, base_url)` to the concrete endpoint.
///
/// The model name is never an input here, which is what guarantees a model id
/// such as `Qwen/Qwen3-VL-Embedding-8B` cannot add path segments.
pub fn endpoint_url(
    protocol: CanvasProtocol,
    base_url: &str,
    kind: CanvasRequestKind,
    model_id: Option<&str>,
) -> Result<String> {
    let base = normalize_base_url(base_url);
    if base.is_empty() {
        return Err(CoreError::invalid("provider base_url is empty"));
    }
    if !(base.starts_with("http://") || base.starts_with("https://")) {
        return Err(CoreError::invalid(format!(
            "provider base_url must start with http:// or https://, got `{base}`"
        )));
    }
    let url = match protocol {
        CanvasProtocol::OpenAi => match kind {
            CanvasRequestKind::Chat => format!("{base}/chat/completions"),
            CanvasRequestKind::Embeddings => format!("{base}/embeddings"),
            CanvasRequestKind::ImageGenerate => format!("{base}/images/generations"),
            CanvasRequestKind::ImageEdit => format!("{base}/images/edits"),
            CanvasRequestKind::SpeechTranscribe => format!("{base}/audio/transcriptions"),
            CanvasRequestKind::SpeechSynthesize => format!("{base}/audio/speech"),
            CanvasRequestKind::Models => format!("{base}/models"),
        },
        CanvasProtocol::Anthropic => match kind {
            CanvasRequestKind::Chat => format!("{base}/v1/messages"),
            other => {
                return Err(CoreError::invalid(format!(
                    "anthropic protocol does not support {} (UnsupportedCapability)",
                    kind_label(other)
                )))
            }
        },
        CanvasProtocol::Gemini => match kind {
            CanvasRequestKind::Chat => {
                let model = gemini_path_segment(model_id)?;
                format!("{base}/v1beta/models/{model}:generateContent")
            }
            CanvasRequestKind::Embeddings => {
                let model = gemini_path_segment(model_id)?;
                format!("{base}/v1beta/models/{model}:embedContent")
            }
            CanvasRequestKind::ImageGenerate => {
                let model = gemini_path_segment(model_id)?;
                format!("{base}/v1beta/models/{model}:generateContent")
            }
            other => {
                return Err(CoreError::invalid(format!(
                    "gemini protocol does not support {} (UnsupportedCapability)",
                    kind_label(other)
                )))
            }
        },
    };
    Ok(url)
}

fn kind_label(kind: CanvasRequestKind) -> &'static str {
    match kind {
        CanvasRequestKind::Chat => "chat",
        CanvasRequestKind::Embeddings => "embeddings",
        CanvasRequestKind::ImageGenerate => "image_generate",
        CanvasRequestKind::ImageEdit => "image_edit",
        CanvasRequestKind::SpeechTranscribe => "speech_transcribe",
        CanvasRequestKind::SpeechSynthesize => "speech_synthesize",
        CanvasRequestKind::Models => "models",
    }
}

/// Encode a Gemini model id as exactly one path segment. A stray `models/`
/// prefix is stripped and any remaining separator is percent-encoded, so the
/// model id can never open a new route.
fn gemini_path_segment(model_id: Option<&str>) -> Result<String> {
    use percent_encoding::{utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};

    const SEGMENT: &AsciiSet = &NON_ALPHANUMERIC
        .remove(b'-')
        .remove(b'_')
        .remove(b'.')
        .remove(b':');

    let raw = model_id
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| CoreError::invalid("gemini request requires a model id"))?;
    let stripped = raw
        .strip_prefix("models/")
        .unwrap_or(raw)
        .trim_start_matches('/');
    if stripped.is_empty() {
        return Err(CoreError::invalid("gemini model id is empty"));
    }
    Ok(utf8_percent_encode(stripped, SEGMENT).to_string())
}

/// Secret-store key for a canvas provider api key.
pub fn provider_secret_key(provider_id: &str) -> String {
    format!("canvas:provider:{provider_id}:api_key")
}

/// Trim and default one model entry. The display name falls back to the model
/// id so a blank name never renders as an empty option.
fn normalize_model(model: CanvasModelConfig) -> Result<CanvasModelConfig> {
    let id = model.id.trim().to_string();
    if id.is_empty() {
        return Err(CoreError::invalid("model id is required"));
    }
    let name = if model.name.trim().is_empty() {
        id.clone()
    } else {
        model.name.trim().to_string()
    };
    Ok(CanvasModelConfig {
        id,
        name,
        description: model.description.trim().to_string(),
        enabled: model.enabled,
        scenarios: model.scenarios,
        priority: model.priority,
    })
}

/// Largest accepted custom provider icon, counted in data-URL characters.
const MAX_LOGO_CHARS: usize = 512 * 1024;

fn normalize_logo(logo: Option<&str>) -> Result<Option<String>> {
    let Some(logo) = logo.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    if logo.len() > MAX_LOGO_CHARS {
        return Err(CoreError::invalid(format!(
            "provider icon is too large ({} characters, limit {MAX_LOGO_CHARS})",
            logo.len()
        )));
    }
    Ok(Some(logo.to_string()))
}

/// Validate + normalize provider input before persistence.
fn normalize_provider_input(
    input: CanvasProviderInput,
    existing_id: Option<String>,
) -> Result<CanvasProviderConfig> {
    let name = input.name.trim().to_string();
    if name.is_empty() {
        return Err(CoreError::invalid("provider name is required"));
    }
    let base_url = normalize_base_url(&input.base_url);
    if base_url.is_empty() {
        return Err(CoreError::invalid("provider base_url is required"));
    }
    if !(base_url.starts_with("http://") || base_url.starts_with("https://")) {
        return Err(CoreError::invalid(format!(
            "provider base_url must start with http:// or https://, got `{base_url}`"
        )));
    }
    let id = {
        let requested = input.id.trim().to_string();
        if requested.is_empty() {
            existing_id.unwrap_or_else(|| format!("cvp-{}", uuid::Uuid::new_v4()))
        } else {
            requested
        }
    };

    let mut models = Vec::with_capacity(input.models.len());
    let mut seen: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for model in input.models {
        let model = normalize_model(model)?;
        if !seen.insert(model.id.clone()) {
            return Err(CoreError::invalid(format!(
                "duplicate model id `{}` in provider `{name}`",
                model.id
            )));
        }
        models.push(model);
    }

    Ok(CanvasProviderConfig {
        id,
        name,
        protocol: CanvasProtocol::from_label(&input.protocol),
        base_url,
        enabled: input.enabled,
        logo: normalize_logo(input.logo.as_deref())?,
        models,
    })
}

/// Application service owning canvas provider persistence and secrets.
pub struct CanvasProviderService {
    db: Arc<Database>,
    secrets: Arc<dyn SecretStore>,
}

impl CanvasProviderService {
    /// Build over the shared application database and encrypted secret store.
    pub fn new(db: Arc<Database>, secrets: Arc<dyn SecretStore>) -> Self {
        Self { db, secrets }
    }

    fn clock_now(&self) -> Timestamp {
        SystemClock.now()
    }

    /// Read the raw provider state (empty when never configured).
    fn read_state(&self) -> Result<CanvasProviderState> {
        let doc = DocumentStore::new(&self.db).get(CANVAS_COLLECTION, CANVAS_PROVIDERS_ID)?;
        let Some(doc) = doc else {
            return Ok(CanvasProviderState::default());
        };
        serde_json::from_str(&doc.body)
            .map_err(|e| CoreError::invalid(format!("decode canvas provider state: {e}")))
    }

    fn write_state(&self, state: &CanvasProviderState) -> Result<()> {
        let body = serde_json::to_string(state)
            .map_err(|e| CoreError::Other(format!("encode canvas provider state: {e}")))?;
        DocumentStore::new(&self.db).put(
            CANVAS_COLLECTION,
            CANVAS_PROVIDERS_ID,
            &body,
            None,
            self.clock_now(),
        )
    }

    /// All providers with their models, plus secret presence (never the value).
    pub fn list_providers(&self) -> Result<Vec<CanvasProviderDto>> {
        let state = self.read_state()?;
        let mut out = Vec::with_capacity(state.providers.len());
        for provider in state.providers {
            let key = self.provider_api_key(&provider.id)?;
            out.push(provider.to_dto(key.as_deref()));
        }
        Ok(out)
    }

    /// Insert or replace a provider. Existing models are preserved when the
    /// incoming model list is empty and the provider already had models.
    pub fn save_provider(&self, input: CanvasProviderInput) -> Result<CanvasProviderDto> {
        let api_key = input
            .api_key
            .as_deref()
            .map(str::trim)
            .filter(|k| !k.is_empty())
            .map(ToString::to_string);
        let mut state = self.read_state()?;
        let existing = state
            .providers
            .iter()
            .find(|p| !input.id.trim().is_empty() && p.id == input.id.trim())
            .cloned();
        let mut normalized =
            normalize_provider_input(input, existing.as_ref().map(|p| p.id.clone()))?;
        if normalized.models.is_empty() {
            if let Some(previous) = &existing {
                normalized.models = previous.models.clone();
            }
        }
        if let Some(index) = state.providers.iter().position(|p| p.id == normalized.id) {
            state.providers[index] = normalized.clone();
        } else {
            if state
                .providers
                .iter()
                .any(|p| p.name.eq_ignore_ascii_case(&normalized.name))
            {
                return Err(CoreError::invalid(format!(
                    "a provider named `{}` already exists",
                    normalized.name
                )));
            }
            state.providers.push(normalized.clone());
        }
        self.write_state(&state)?;
        if let Some(key) = api_key {
            self.secrets
                .set(&provider_secret_key(&normalized.id), &key)?;
        }
        let stored = self.provider_api_key(&normalized.id)?;
        Ok(normalized.to_dto(stored.as_deref()))
    }

    /// Remove a provider and its stored secret.
    pub fn remove_provider(&self, provider_id: &str) -> Result<bool> {
        let mut state = self.read_state()?;
        let before = state.providers.len();
        state.providers.retain(|p| p.id != provider_id);
        if state.providers.len() == before {
            return Ok(false);
        }
        self.write_state(&state)?;
        if let Err(error) = self.secrets.delete(&provider_secret_key(provider_id)) {
            tracing::warn!(
                provider_id,
                error = %error,
                "canvas provider removed but secret cleanup failed"
            );
        }
        Ok(true)
    }

    /// Replace the stored api key for a provider.
    pub fn set_provider_api_key(&self, provider_id: &str, api_key: &str) -> Result<()> {
        let api_key = api_key.trim();
        if api_key.is_empty() {
            return Err(CoreError::invalid("api key is empty"));
        }
        self.ensure_provider(provider_id)?;
        self.secrets.set(&provider_secret_key(provider_id), api_key)
    }

    /// Delete the stored api key for a provider.
    pub fn clear_provider_api_key(&self, provider_id: &str) -> Result<()> {
        self.secrets.delete(&provider_secret_key(provider_id))
    }

    /// Raw api key for backend-internal use only (adapters / routing).
    pub fn provider_api_key(&self, provider_id: &str) -> Result<Option<String>> {
        let value = self.secrets.get(&provider_secret_key(provider_id))?;
        Ok(value.filter(|v| !v.trim().is_empty()))
    }

    fn ensure_provider(&self, provider_id: &str) -> Result<CanvasProviderConfig> {
        self.read_state()?
            .providers
            .into_iter()
            .find(|p| p.id == provider_id)
            .ok_or_else(|| CoreError::not_found(format!("canvas provider `{provider_id}`")))
    }

    /// Add or replace one model under a provider.
    pub fn save_model(
        &self,
        provider_id: &str,
        model: CanvasModelConfig,
    ) -> Result<CanvasProviderDto> {
        let mut state = self.read_state()?;
        let Some(provider) = state.providers.iter_mut().find(|p| p.id == provider_id) else {
            return Err(CoreError::not_found(format!(
                "canvas provider `{provider_id}`"
            )));
        };
        let entry = normalize_model(model)?;
        let model_id = entry.id.clone();
        match provider.models.iter_mut().find(|m| m.id == model_id) {
            Some(slot) => *slot = entry,
            None => provider.models.push(entry),
        }
        self.write_state(&state)?;
        let key = self.provider_api_key(provider_id)?;
        let provider = self
            .read_state()?
            .providers
            .into_iter()
            .find(|p| p.id == provider_id)
            .ok_or_else(|| CoreError::not_found(format!("canvas provider `{provider_id}`")))?;
        Ok(provider.to_dto(key.as_deref()))
    }

    /// Remove one model from a provider.
    pub fn remove_model(&self, provider_id: &str, model_id: &str) -> Result<bool> {
        let mut state = self.read_state()?;
        let Some(provider) = state.providers.iter_mut().find(|p| p.id == provider_id) else {
            return Ok(false);
        };
        let before = provider.models.len();
        provider.models.retain(|m| m.id != model_id);
        if provider.models.len() == before {
            return Ok(false);
        }
        self.write_state(&state)?;
        Ok(true)
    }

    /// Read scenario bindings for a workspace (global when `workspace_id` is
    /// `None`).
    pub fn bindings(&self, workspace_id: Option<&str>) -> Result<Vec<CanvasScenarioBinding>> {
        let id = binding_doc_id(workspace_id);
        let doc = DocumentStore::new(&self.db).get(CANVAS_COLLECTION, &id)?;
        let Some(doc) = doc else {
            return Ok(Vec::new());
        };
        let state: CanvasBindingState = serde_json::from_str(&doc.body)
            .map_err(|e| CoreError::invalid(format!("decode canvas bindings: {e}")))?;
        Ok(state.bindings)
    }

    /// Persist scenario bindings for a workspace.
    pub fn save_bindings(
        &self,
        workspace_id: Option<&str>,
        bindings: Vec<CanvasScenarioBinding>,
    ) -> Result<Vec<CanvasScenarioBinding>> {
        for binding in &bindings {
            let scenario = CanvasScenario::from_label(&binding.scenario).ok_or_else(|| {
                CoreError::invalid(format!("unknown canvas scenario `{}`", binding.scenario))
            })?;
            let provider = self
                .read_state()?
                .providers
                .into_iter()
                .find(|p| p.id == binding.provider_id)
                .ok_or_else(|| {
                    CoreError::not_found(format!("canvas provider `{}`", binding.provider_id))
                })?;
            let model = provider
                .models
                .iter()
                .find(|m| m.id == binding.model_id)
                .ok_or_else(|| {
                    CoreError::not_found(format!("canvas model `{}`", binding.model_id))
                })?;
            if !model.scenarios.contains(&scenario) {
                return Err(CoreError::invalid(format!(
                    "model `{}` is not declared for scenario `{}`",
                    model.id,
                    scenario.as_str()
                )));
            }
        }
        let body = serde_json::to_string(&CanvasBindingState {
            bindings: bindings.clone(),
        })
        .map_err(|e| CoreError::Other(format!("encode canvas bindings: {e}")))?;
        DocumentStore::new(&self.db).put(
            CANVAS_COLLECTION,
            &binding_doc_id(workspace_id),
            &body,
            None,
            self.clock_now(),
        )?;
        Ok(bindings)
    }

    /// Resolve the enabled provider + model behind a `(provider_id, model_id)`
    /// pair, with its api key attached for backend-internal use.
    pub fn resolve_model(&self, provider_id: &str, model_id: &str) -> Result<ResolvedCanvasModel> {
        let state = self.read_state()?;
        let provider = state
            .providers
            .into_iter()
            .find(|p| p.id == provider_id)
            .ok_or_else(|| CoreError::not_found(format!("canvas provider `{provider_id}`")))?;
        if !provider.enabled {
            return Err(CoreError::invalid(format!(
                "canvas provider `{}` is disabled",
                provider.name
            )));
        }
        let model = provider
            .models
            .iter()
            .find(|m| m.id == model_id)
            .cloned()
            .ok_or_else(|| {
                CoreError::not_found(format!("canvas model `{model_id}` in `{provider_id}`"))
            })?;
        if !model.enabled {
            return Err(CoreError::invalid(format!(
                "canvas model `{model_id}` is disabled"
            )));
        }
        let api_key = self.provider_api_key(provider_id)?;
        Ok(ResolvedCanvasModel {
            provider,
            model,
            api_key,
        })
    }

    /// Candidate models for a scenario, ordered by the workspace bindings first
    /// and then by any other enabled model that declared the scenario.
    pub fn candidates_for_scenario(
        &self,
        scenario: CanvasScenario,
        workspace_id: Option<&str>,
    ) -> Result<Vec<ResolvedCanvasModel>> {
        let state = self.read_state()?;
        let bindings = self.bindings(workspace_id)?;
        let mut ordered: Vec<ResolvedCanvasModel> = Vec::new();
        for binding in bindings
            .iter()
            .filter(|b| b.enabled && CanvasScenario::from_label(&b.scenario) == Some(scenario))
        {
            if let Some(found) = state
                .providers
                .iter()
                .find(|p| p.id == binding.provider_id && p.enabled)
                .and_then(|p| {
                    p.models
                        .iter()
                        .find(|m| m.id == binding.model_id && m.enabled)
                })
                .and_then(|_| {
                    self.resolve_model(&binding.provider_id, &binding.model_id)
                        .ok()
                })
            {
                if !ordered
                    .iter()
                    .any(|r| r.model.id == found.model.id && r.provider.id == found.provider.id)
                {
                    ordered.push(found);
                }
            }
        }
        let mut extras: Vec<ResolvedCanvasModel> = Vec::new();
        for provider in state.providers.iter().filter(|p| p.enabled) {
            for model in provider
                .models
                .iter()
                .filter(|m| m.enabled && m.scenarios.contains(&scenario))
            {
                if ordered
                    .iter()
                    .any(|r| r.provider.id == provider.id && r.model.id == model.id)
                {
                    continue;
                }
                let api_key = self.provider_api_key(&provider.id)?;
                extras.push(ResolvedCanvasModel {
                    provider: provider.clone(),
                    model: model.clone(),
                    api_key,
                });
            }
        }
        extras.sort_by(|a, b| {
            b.model
                .priority
                .cmp(&a.model.priority)
                .then_with(|| a.provider.name.cmp(&b.provider.name))
        });
        ordered.extend(extras);
        Ok(ordered)
    }

    /// Direct access to the provider record (used by the gateway / adapters).
    pub fn provider(&self, provider_id: &str) -> Result<CanvasProviderConfig> {
        self.ensure_provider(provider_id)
    }
}

/// A provider + model pair resolved for one call, secret attached.
#[derive(Debug, Clone)]
pub struct ResolvedCanvasModel {
    pub provider: CanvasProviderConfig,
    pub model: CanvasModelConfig,
    pub api_key: Option<String>,
}

/// Document id for a workspace's binding list.
pub fn binding_doc_id(workspace_id: Option<&str>) -> String {
    match workspace_id {
        Some(id) if !id.trim().is_empty() => format!("bindings:{}", id.trim()),
        _ => "bindings:_default".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> Arc<Database> {
        Arc::new(Database::open_in_memory().expect("in-memory db"))
    }

    fn service() -> CanvasProviderService {
        CanvasProviderService::new(
            db(),
            Arc::new(crate::secret_store::MemorySecretStore::default()),
        )
    }

    fn model(id: &str, scenarios: Vec<CanvasScenario>) -> CanvasModelConfig {
        CanvasModelConfig {
            id: id.to_string(),
            name: id.to_string(),
            description: String::new(),
            enabled: true,
            scenarios,
            priority: 0,
        }
    }

    #[test]
    fn embedding_endpoint_never_uses_model_as_route() {
        let url = endpoint_url(
            CanvasProtocol::OpenAi,
            "https://api.siliconflow.cn/v1",
            CanvasRequestKind::Embeddings,
            Some("Qwen/Qwen3-VL-Embedding-8B"),
        )
        .expect("endpoint");
        assert_eq!(url, "https://api.siliconflow.cn/v1/embeddings");
        assert!(!url.contains("Qwen"));
    }

    #[test]
    fn openai_image_generate_and_edit_endpoints_are_distinct() {
        let gen = endpoint_url(
            CanvasProtocol::OpenAi,
            "https://toloveu.asia/v1/",
            CanvasRequestKind::ImageGenerate,
            Some("gpt-image-2"),
        )
        .unwrap();
        let edit = endpoint_url(
            CanvasProtocol::OpenAi,
            "https://toloveu.asia/v1",
            CanvasRequestKind::ImageEdit,
            Some("gpt-image-2"),
        )
        .unwrap();
        assert_eq!(gen, "https://toloveu.asia/v1/images/generations");
        assert_eq!(edit, "https://toloveu.asia/v1/images/edits");
    }

    #[test]
    fn anthropic_chat_uses_messages_and_rejects_media() {
        let url = endpoint_url(
            CanvasProtocol::Anthropic,
            "https://api.deepseek.com/anthropic",
            CanvasRequestKind::Chat,
            Some("deepseek-flash"),
        )
        .unwrap();
        assert_eq!(url, "https://api.deepseek.com/anthropic/v1/messages");
        let err = endpoint_url(
            CanvasProtocol::Anthropic,
            "https://api.deepseek.com/anthropic",
            CanvasRequestKind::Embeddings,
            Some("deepseek-flash"),
        )
        .expect_err("embeddings unsupported");
        assert!(err.to_string().contains("UnsupportedCapability"));
    }

    #[test]
    fn gemini_model_id_is_encoded_into_one_path_segment() {
        let url = endpoint_url(
            CanvasProtocol::Gemini,
            "http://127.0.0.1:8045",
            CanvasRequestKind::Chat,
            Some("models/gemini-3.8-flash-low"),
        )
        .unwrap();
        assert_eq!(
            url,
            "http://127.0.0.1:8045/v1beta/models/gemini-3.8-flash-low:generateContent"
        );
        let with_slash = endpoint_url(
            CanvasProtocol::Gemini,
            "http://127.0.0.1:8045",
            CanvasRequestKind::Chat,
            Some("evil/../../admin"),
        )
        .unwrap();
        assert!(!with_slash.contains("admin") || with_slash.contains("%2F"));
        assert_eq!(
            with_slash,
            "http://127.0.0.1:8045/v1beta/models/evil%2F..%2F..%2Fadmin:generateContent"
        );
    }

    #[test]
    fn provider_round_trips_through_document_store_with_secret_in_secret_store() {
        let svc = service();
        let dto = svc
            .save_provider(CanvasProviderInput {
                name: "SiliconFlow".to_string(),
                protocol: "openai".to_string(),
                base_url: "  https://api.siliconflow.cn/v1/ ".to_string(),
                enabled: true,
                models: vec![model(
                    "Qwen/Qwen3-VL-Embedding-8B",
                    vec![CanvasScenario::Embedding],
                )],
                api_key: None,
                ..Default::default()
            })
            .expect("save provider");
        assert_eq!(dto.base_url, "https://api.siliconflow.cn/v1");
        assert!(!dto.api_key_set);

        svc.set_provider_api_key(&dto.id, "sk-canvas-local-test-9f2c41d7b6e04a51")
            .expect("set key");
        let listed = svc.list_providers().expect("list");
        assert_eq!(listed.len(), 1);
        assert!(listed[0].api_key_set);
        let masked = listed[0].api_key_masked.clone().expect("masked");
        assert!(masked.contains('…'));
        assert!(!masked.contains("9f2c41d7b6e0"));

        let raw = DocumentStore::new(&svc.db)
            .get(CANVAS_COLLECTION, CANVAS_PROVIDERS_ID)
            .unwrap()
            .expect("doc")
            .body;
        assert!(!raw.contains("sk-canvas-local-test"));
    }

    #[test]
    fn resolve_and_candidates_filter_disabled_and_unbound() {
        let svc = service();
        let provider = svc
            .save_provider(CanvasProviderInput {
                name: "Toloveu".to_string(),
                protocol: "openai".to_string(),
                base_url: "https://toloveu.asia/v1".to_string(),
                enabled: true,
                models: vec![
                    model("gpt-5.6-sol", vec![CanvasScenario::Text]),
                    model("gpt-image-2", vec![CanvasScenario::ImageGeneration]),
                ],
                api_key: Some("sk-test".to_string()),
                ..Default::default()
            })
            .expect("save");
        let resolved = svc
            .resolve_model(&provider.id, "gpt-5.6-sol")
            .expect("resolve");
        assert_eq!(resolved.api_key.as_deref(), Some("sk-test"));
        let text_only: Vec<String> = svc
            .candidates_for_scenario(CanvasScenario::Text, None)
            .unwrap()
            .into_iter()
            .map(|c| c.model.id)
            .collect();
        assert_eq!(text_only, vec!["gpt-5.6-sol".to_string()]);
        assert!(svc.resolve_model(&provider.id, "missing").is_err());
    }

    #[test]
    fn bindings_reject_model_without_scenario_and_round_trip() {
        let svc = service();
        let provider = svc
            .save_provider(CanvasProviderInput {
                name: "DeepSeek Anthropic".to_string(),
                protocol: "anthropic".to_string(),
                base_url: "https://api.deepseek.com/anthropic".to_string(),
                enabled: true,
                models: vec![model("deepseek-flash", vec![CanvasScenario::Text])],
                api_key: None,
                ..Default::default()
            })
            .expect("save");
        let ok = svc
            .save_bindings(
                Some("ws-1"),
                vec![CanvasScenarioBinding {
                    scenario: "text".to_string(),
                    provider_id: provider.id.clone(),
                    model_id: "deepseek-flash".to_string(),
                    enabled: true,
                }],
            )
            .expect("save bindings");
        assert_eq!(ok.len(), 1);
        assert_eq!(svc.bindings(Some("ws-1")).unwrap().len(), 1);
        assert!(svc.bindings(Some("ws-2")).unwrap().is_empty());
        let err = svc
            .save_bindings(
                Some("ws-1"),
                vec![CanvasScenarioBinding {
                    scenario: "embedding".to_string(),
                    provider_id: provider.id.clone(),
                    model_id: "deepseek-flash".to_string(),
                    enabled: true,
                }],
            )
            .expect_err("scenario mismatch");
        assert!(err.to_string().contains("not declared for scenario"));
    }

    #[test]
    fn save_provider_without_models_preserves_existing_models() {
        let svc = service();
        let created = svc
            .save_provider(CanvasProviderInput {
                name: "Gemini Local".to_string(),
                protocol: "gemini".to_string(),
                base_url: "http://127.0.0.1:8045".to_string(),
                enabled: true,
                models: vec![model("gemini-3.8-flash-low", vec![CanvasScenario::Text])],
                api_key: None,
                ..Default::default()
            })
            .expect("create");
        let renamed = svc
            .save_provider(CanvasProviderInput {
                id: created.id.clone(),
                name: "Gemini 本地网关".to_string(),
                protocol: "gemini".to_string(),
                base_url: "http://127.0.0.1:8045".to_string(),
                enabled: false,
                models: vec![],
                logo: None,
                api_key: None,
            })
            .expect("rename");
        assert_eq!(renamed.models.len(), 1);
        assert!(!renamed.enabled);
        assert_eq!(renamed.id, created.id);
    }

    #[test]
    fn remove_provider_drops_secret_and_model_crud_works() {
        let svc = service();
        let provider = svc
            .save_provider(CanvasProviderInput {
                name: "OAI".to_string(),
                protocol: "openai".to_string(),
                base_url: "https://toloveu.asia/v1".to_string(),
                enabled: true,
                models: vec![],
                logo: None,
                api_key: Some("sk-abcdef123456".to_string()),
                ..Default::default()
            })
            .expect("save");
        let after_model = svc
            .save_model(
                &provider.id,
                model("gpt-5.6-sol", vec![CanvasScenario::Text]),
            )
            .expect("add model");
        assert_eq!(after_model.models.len(), 1);
        assert!(svc.remove_model(&provider.id, "gpt-5.6-sol").unwrap());
        assert!(!svc.remove_model(&provider.id, "gpt-5.6-sol").unwrap());
        assert!(svc.remove_provider(&provider.id).unwrap());
        assert!(svc.list_providers().unwrap().is_empty());
        assert!(svc.provider_api_key(&provider.id).unwrap().is_none());
    }

    #[test]
    fn mask_secret_keeps_only_short_edges() {
        assert_eq!(mask_secret("sk-1234567890abcdef"), "sk-…cdef");
        assert_eq!(mask_secret("short"), "*****");
    }
}
