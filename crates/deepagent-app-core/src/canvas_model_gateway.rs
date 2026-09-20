//! Canvas model gateway: deterministic operation routing plus provider calls.
//!
//! This is the canvas-side counterpart of the chat `ResponsesBackend`. It owns
//! three things and nothing else:
//!
//! 1. [`DeterministicRouteResolver`] — decide the concrete [`CanvasOperation`]
//!    from the node contract, an explicit operation and the *resolved artifact
//!    kinds*. AI is never consulted when the inputs already determine it.
//! 2. Candidate selection — scenario bindings / priorities from
//!    [`CanvasProviderService`], with fallback restricted to transient failures.
//! 3. Protocol adapters — OpenAI-compatible, Anthropic Messages and Gemini
//!    `generateContent`. Third-party wire shapes stop here; nothing above this
//!    module sees a provider payload, and nothing below it sees a node name.

use std::collections::BTreeSet;
use std::time::Duration;

use async_trait::async_trait;
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use deepagent_core::error::Result as CoreResult;
use deepagent_runtime::workflow::{
    CanvasCompletionRequest, CanvasCompletionResponse, CanvasEmbeddingRequest,
    CanvasEmbeddingResponse, CanvasImageRequest, CanvasImageResponse, CanvasModelBridge,
    CanvasRouteOutcome, CanvasRouteRequest,
};

use crate::canvas_node_contract::canvas_node_contract;
use crate::canvas_provider_service::{
    endpoint_url, CanvasProtocol, CanvasProviderService, CanvasRequestKind, CanvasScenario,
    ResolvedCanvasModel, ANTHROPIC_VERSION,
};

/// Stable error codes surfaced to the UI, CLI and events.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CanvasErrorCode {
    RouteAmbiguous,
    OperationInputConflict,
    MissingReferenceInput,
    TooManyReferences,
    UnsupportedOperation,
    UnsupportedCapability,
    UnsupportedNode,
    NoCandidateModel,
    SecretMissing,
    InvalidInput,
    TransientFailure,
    ProviderFailure,
}

impl CanvasErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RouteAmbiguous => "RouteAmbiguous",
            Self::OperationInputConflict => "OperationInputConflict",
            Self::MissingReferenceInput => "MissingReferenceInput",
            Self::TooManyReferences => "TooManyReferences",
            Self::UnsupportedOperation => "UnsupportedOperation",
            Self::UnsupportedCapability => "UnsupportedCapability",
            Self::UnsupportedNode => "UnsupportedNode",
            Self::NoCandidateModel => "NoCandidateModel",
            Self::SecretMissing => "SecretMissing",
            Self::InvalidInput => "InvalidInput",
            Self::TransientFailure => "TransientFailure",
            Self::ProviderFailure => "ProviderFailure",
        }
    }

    /// Only timeouts, network errors, 429 and 5xx may advance the candidate list.
    pub fn is_transient(self) -> bool {
        matches!(self, Self::TransientFailure)
    }
}

/// Gateway failure carrying a stable code plus a human-readable reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanvasError {
    pub code: CanvasErrorCode,
    pub message: String,
}

impl CanvasError {
    pub fn new(code: CanvasErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new(CanvasErrorCode::InvalidInput, message)
    }

    pub fn is_transient(&self) -> bool {
        self.code.is_transient()
    }
}

impl std::fmt::Display for CanvasError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code.as_str(), self.message)
    }
}

impl std::error::Error for CanvasError {}

impl From<deepagent_core::error::CoreError> for CanvasError {
    fn from(value: deepagent_core::error::CoreError) -> Self {
        Self::new(CanvasErrorCode::InvalidInput, value.to_string())
    }
}

pub type CanvasResult<T> = Result<T, CanvasError>;

/// What a canvas node actually does. Declared per node contract; the route
/// resolver narrows `auto` to exactly one of these.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CanvasOperation {
    ArtifactImport,
    NormalizeIntent,
    TextGenerate,
    VisionDescribe,
    ImageGenerate,
    ImageEdit,
    ImageCrop,
    ImageRemoveBackground,
    ImageUpscale,
    ImageRepaint,
    ImageCompare,
    VideoGenerate,
    VideoEdit,
    VideoExtend,
    VideoCompose,
    CameraConstraint,
    LensConstraint,
    FocalLengthConstraint,
    ApertureConstraint,
    DirectorPlan,
    TemplateExpand,
    CharacterFaceCompose,
    CharacterBodyCompose,
    CharacterStyleCompose,
    StoryboardPlan,
    SpeechTranscribe,
    SpeechSynthesize,
    Embedding,
}

impl CanvasOperation {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ArtifactImport => "artifact_import",
            Self::NormalizeIntent => "normalize_intent",
            Self::TextGenerate => "text_generate",
            Self::VisionDescribe => "vision_describe",
            Self::ImageGenerate => "image_generate",
            Self::ImageEdit => "image_edit",
            Self::ImageCrop => "image_crop",
            Self::ImageRemoveBackground => "image_remove_background",
            Self::ImageUpscale => "image_upscale",
            Self::ImageRepaint => "image_repaint",
            Self::ImageCompare => "image_compare",
            Self::VideoGenerate => "video_generate",
            Self::VideoEdit => "video_edit",
            Self::VideoExtend => "video_extend",
            Self::VideoCompose => "video_compose",
            Self::CameraConstraint => "camera_constraint",
            Self::LensConstraint => "lens_constraint",
            Self::FocalLengthConstraint => "focal_length_constraint",
            Self::ApertureConstraint => "aperture_constraint",
            Self::DirectorPlan => "director_plan",
            Self::TemplateExpand => "template_expand",
            Self::CharacterFaceCompose => "character_face_compose",
            Self::CharacterBodyCompose => "character_body_compose",
            Self::CharacterStyleCompose => "character_style_compose",
            Self::StoryboardPlan => "storyboard_plan",
            Self::SpeechTranscribe => "speech_transcribe",
            Self::SpeechSynthesize => "speech_synthesize",
            Self::Embedding => "embedding",
        }
    }

    pub fn from_label(label: &str) -> Option<Self> {
        let normalized = label.trim().to_ascii_lowercase().replace('-', "_");
        [
            Self::ArtifactImport,
            Self::NormalizeIntent,
            Self::TextGenerate,
            Self::VisionDescribe,
            Self::ImageGenerate,
            Self::ImageEdit,
            Self::ImageCrop,
            Self::ImageRemoveBackground,
            Self::ImageUpscale,
            Self::ImageRepaint,
            Self::ImageCompare,
            Self::VideoGenerate,
            Self::VideoEdit,
            Self::VideoExtend,
            Self::VideoCompose,
            Self::CameraConstraint,
            Self::LensConstraint,
            Self::FocalLengthConstraint,
            Self::ApertureConstraint,
            Self::DirectorPlan,
            Self::TemplateExpand,
            Self::CharacterFaceCompose,
            Self::CharacterBodyCompose,
            Self::CharacterStyleCompose,
            Self::StoryboardPlan,
            Self::SpeechTranscribe,
            Self::SpeechSynthesize,
            Self::Embedding,
        ]
        .into_iter()
        .find(|op| op.as_str() == normalized)
    }

    /// Scenario gate: which model scenario may serve this operation.
    pub fn scenario(self) -> Option<CanvasScenario> {
        Some(match self {
            Self::TextGenerate
            | Self::VisionDescribe
            | Self::NormalizeIntent
            | Self::DirectorPlan
            | Self::TemplateExpand
            | Self::StoryboardPlan
            | Self::CharacterFaceCompose
            | Self::CharacterBodyCompose
            | Self::CharacterStyleCompose => CanvasScenario::Text,
            Self::ImageGenerate | Self::ImageEdit => CanvasScenario::ImageGeneration,
            Self::VideoGenerate | Self::VideoEdit | Self::VideoExtend => {
                CanvasScenario::VideoGeneration
            }
            Self::SpeechTranscribe => CanvasScenario::SpeechToText,
            Self::SpeechSynthesize => CanvasScenario::TextToSpeech,
            Self::Embedding => CanvasScenario::Embedding,
            _ => return None,
        })
    }

    /// Operations that run in-process (no model call).
    pub fn is_local(self) -> bool {
        matches!(
            self,
            Self::ArtifactImport
                | Self::ImageCrop
                | Self::VideoCompose
                | Self::CameraConstraint
                | Self::LensConstraint
                | Self::FocalLengthConstraint
                | Self::ApertureConstraint
        )
    }
}

/// Artifact kinds the resolver may observe on an input edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum CanvasInputKind {
    Empty,
    Text,
    Json,
    Image,
    Video,
    Audio,
    Document,
}

/// Deterministic routing inputs. `input_kinds` must come from the backend's
/// resolved artifacts — never from a node label or a file-name suffix.
#[derive(Debug, Clone, Default)]
pub struct RouteFacts {
    pub node_kind: String,
    pub allowed_operations: Vec<CanvasOperation>,
    /// `None` means the node asked for automatic routing.
    pub explicit_operation: Option<CanvasOperation>,
    pub input_kinds: Vec<CanvasInputKind>,
    pub has_prompt: bool,
}

/// The outcome of routing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteDecision {
    pub operation: CanvasOperation,
    pub reason: &'static str,
}

/// Operations a node kind may auto-route to. Anything outside the table must be
/// selected explicitly, otherwise routing would be a guess.
fn auto_route_candidates(node_kind: &str) -> &'static [CanvasOperation] {
    match node_kind {
        "text-gen" | "script-gen" => &[CanvasOperation::TextGenerate],
        "image-gen" => &[CanvasOperation::ImageGenerate, CanvasOperation::ImageEdit],
        "image-edit" => &[CanvasOperation::ImageEdit],
        "image-input" => &[CanvasOperation::ArtifactImport],
        "video-gen" => &[CanvasOperation::VideoGenerate],
        "storyboard-grid" => &[CanvasOperation::StoryboardPlan],
        "director" => &[CanvasOperation::DirectorPlan],
        _ => &[],
    }
}

/// Rule-based router. AI is not consulted here; ambiguity is an error the UI
/// resolves by asking the user to pick an operation.
pub struct DeterministicRouteResolver;

impl DeterministicRouteResolver {
    pub fn resolve(facts: &RouteFacts) -> CanvasResult<RouteDecision> {
        if facts.allowed_operations.is_empty() {
            return Err(CanvasError::new(
                CanvasErrorCode::UnsupportedNode,
                format!("node `{}` has no declared operations", facts.node_kind),
            ));
        }
        if let Some(operation) = facts.explicit_operation {
            if !facts.allowed_operations.contains(&operation) {
                return Err(CanvasError::new(
                    CanvasErrorCode::OperationInputConflict,
                    format!(
                        "operation `{}` is not allowed for node `{}`",
                        operation.as_str(),
                        facts.node_kind
                    ),
                ));
            }
            validate_operation_inputs(operation, facts)?;
            return Ok(RouteDecision {
                operation,
                reason: "explicit_operation",
            });
        }

        let candidates = auto_route_candidates(&facts.node_kind);
        let allowed: BTreeSet<CanvasOperation> = facts.allowed_operations.iter().copied().collect();
        let viable: Vec<CanvasOperation> = candidates
            .iter()
            .copied()
            .filter(|op| allowed.contains(op))
            .filter(|op| operation_matches_inputs(*op, facts))
            .collect();

        match viable.as_slice() {
            [single] => Ok(RouteDecision {
                operation: *single,
                reason: "artifact_facts",
            }),
            [] => Err(CanvasError::new(
                CanvasErrorCode::RouteAmbiguous,
                format!(
                    "cannot route node `{}` from {} input kind(s); select an operation explicitly",
                    facts.node_kind,
                    facts.input_kinds.len()
                ),
            )),
            many => Err(CanvasError::new(
                CanvasErrorCode::RouteAmbiguous,
                format!(
                    "node `{}` matches multiple operations: {}",
                    facts.node_kind,
                    many.iter()
                        .map(|op| op.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            )),
        }
    }
}

fn has_image(facts: &RouteFacts) -> bool {
    facts.input_kinds.contains(&CanvasInputKind::Image)
}

fn has_video(facts: &RouteFacts) -> bool {
    facts.input_kinds.contains(&CanvasInputKind::Video)
}

fn operation_matches_inputs(operation: CanvasOperation, facts: &RouteFacts) -> bool {
    match operation {
        CanvasOperation::ImageGenerate => !has_image(facts) && facts.has_prompt,
        CanvasOperation::ImageEdit => has_image(facts) && facts.has_prompt,
        CanvasOperation::TextGenerate => !has_image(facts) && facts.has_prompt,
        // A video input means the user either wants a new video or to change the
        // existing one; artifacts alone cannot tell those apart, so auto-routing
        // must stay unresolved rather than default to generation.
        CanvasOperation::VideoGenerate => facts.has_prompt && !has_video(facts),
        _ => false,
    }
}

fn validate_operation_inputs(operation: CanvasOperation, facts: &RouteFacts) -> CanvasResult<()> {
    let requires_image = matches!(
        operation,
        CanvasOperation::ImageEdit
            | CanvasOperation::ImageRemoveBackground
            | CanvasOperation::ImageUpscale
            | CanvasOperation::ImageRepaint
            | CanvasOperation::ImageCrop
            | CanvasOperation::ImageCompare
            | CanvasOperation::VisionDescribe
    );
    if requires_image && !has_image(facts) {
        return Err(CanvasError::new(
            CanvasErrorCode::MissingReferenceInput,
            format!(
                "operation `{}` requires an image artifact",
                operation.as_str()
            ),
        ));
    }
    if operation == CanvasOperation::ImageGenerate && has_image(facts) {
        return Err(CanvasError::new(
            CanvasErrorCode::OperationInputConflict,
            "image_generate conflicts with image input; use image_edit".to_string(),
        ));
    }
    if matches!(
        operation,
        CanvasOperation::ImageGenerate | CanvasOperation::TextGenerate
    ) && !facts.has_prompt
    {
        return Err(CanvasError::new(
            CanvasErrorCode::MissingReferenceInput,
            format!(
                "operation `{}` requires a non-empty prompt",
                operation.as_str()
            ),
        ));
    }
    if operation == CanvasOperation::Embedding
        && !facts
            .input_kinds
            .iter()
            .any(|k| matches!(k, CanvasInputKind::Text | CanvasInputKind::Document))
    {
        return Err(CanvasError::new(
            CanvasErrorCode::MissingReferenceInput,
            "embedding requires text input".to_string(),
        ));
    }
    if operation == CanvasOperation::SpeechTranscribe
        && !facts.input_kinds.contains(&CanvasInputKind::Audio)
    {
        return Err(CanvasError::new(
            CanvasErrorCode::MissingReferenceInput,
            "speech_transcribe requires exactly one audio artifact".to_string(),
        ));
    }
    Ok(())
}

/// One image supplied to a call. `data_url` keeps the gateway independent of
/// how the caller stored the artifact.
#[derive(Debug, Clone)]
pub struct CanvasImageInput {
    pub data_url: String,
}

/// A resolved canvas model call.
#[derive(Debug, Clone)]
pub struct CanvasModelRequest {
    pub operation: CanvasOperation,
    pub prompt: String,
    pub system_prompt: Option<String>,
    pub images: Vec<CanvasImageInput>,
    pub texts: Vec<String>,
    pub size: Option<String>,
    pub timeout_ms: u64,
}

impl CanvasModelRequest {
    pub fn text(prompt: impl Into<String>) -> Self {
        Self {
            operation: CanvasOperation::TextGenerate,
            prompt: prompt.into(),
            system_prompt: None,
            images: Vec::new(),
            texts: Vec::new(),
            size: None,
            timeout_ms: 60_000,
        }
    }

    pub fn with_operation(mut self, operation: CanvasOperation) -> Self {
        self.operation = operation;
        self
    }
}

/// Gateway response. Media is returned as bytes for the caller to store as an
/// artifact; only the reference belongs in events.
#[derive(Debug, Clone)]
pub enum CanvasModelOutput {
    Text {
        text: String,
        /// Provider-side reasoning (Anthropic thinking / Gemini thought) kept
        /// structured instead of being flattened into the answer text.
        reasoning: Option<String>,
        provider_id: String,
        model_id: String,
    },
    Image {
        mime: String,
        bytes: Vec<u8>,
        provider_id: String,
        model_id: String,
    },
    Embedding {
        vector: Vec<f32>,
        dimensions: usize,
        provider_id: String,
        model_id: String,
    },
    Audio {
        mime: String,
        bytes: Vec<u8>,
        provider_id: String,
        model_id: String,
    },
}

/// Result of a connectivity probe, safe to show in the UI.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CanvasConnectionTestResult {
    pub ok: bool,
    pub code: String,
    pub message: String,
    pub endpoint: String,
    pub model_id: String,
    pub latency_ms: u64,
}

/// Executes canvas model calls across providers.
pub struct CanvasModelGateway {
    providers: std::sync::Arc<CanvasProviderService>,
    http: reqwest::Client,
    /// Where generated media is stored. Without it, image calls have nowhere to
    /// put their bytes, so they fail instead of emitting inline base64.
    artifacts: Option<std::sync::Arc<crate::canvas_artifact_service::CanvasArtifactService>>,
}

impl CanvasModelGateway {
    pub fn new(providers: std::sync::Arc<CanvasProviderService>) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(300))
            .build()
            .unwrap_or_default();
        Self {
            providers,
            http,
            artifacts: None,
        }
    }

    #[must_use]
    pub fn with_artifacts(
        mut self,
        artifacts: std::sync::Arc<crate::canvas_artifact_service::CanvasArtifactService>,
    ) -> Self {
        self.artifacts = Some(artifacts);
        self
    }

    fn artifact_store(
        &self,
        what: &str,
    ) -> CoreResult<&crate::canvas_artifact_service::CanvasArtifactService> {
        let service = self.artifacts.as_ref().ok_or_else(|| {
            deepagent_core::error::CoreError::other(format!(
                "canvas artifact store is not wired; cannot {what}"
            ))
        })?;
        Ok(service.as_ref())
    }

    /// Turn node-stored image references into what a provider can fetch:
    /// `artifact://id` becomes inline bytes, `data:` / `http(s)` pass through.
    fn resolve_image_inputs(&self, urls: &[String]) -> CoreResult<Vec<CanvasImageInput>> {
        urls.iter()
            .map(|url| {
                let resolved = self
                    .artifact_store("send a reference image")?
                    .resolve_for_provider(url)?
                    .ok_or_else(|| {
                        deepagent_core::error::CoreError::invalid(format!(
                            "UnsupportedOperation: image reference `{url}` is not an artifact, data URL or http URL"
                        ))
                    })?;
                Ok(CanvasImageInput {
                    data_url: resolved,
                })
            })
            .collect()
    }

    /// Run a request against an explicit provider/model pair.
    pub async fn execute_on(
        &self,
        provider_id: &str,
        model_id: &str,
        request: &CanvasModelRequest,
    ) -> CanvasResult<CanvasModelOutput> {
        let resolved = self
            .providers
            .resolve_model(provider_id, model_id)
            .map_err(|e| CanvasError::new(CanvasErrorCode::InvalidInput, e.to_string()))?;
        self.call(&resolved, request).await
    }

    /// Run a request using the scenario candidate list, falling through only on
    /// transient failures. The operation is never recomputed while switching.
    pub async fn execute(
        &self,
        request: &CanvasModelRequest,
        workspace_id: Option<&str>,
    ) -> CanvasResult<CanvasModelOutput> {
        let scenario = request.operation.scenario().ok_or_else(|| {
            CanvasError::new(
                CanvasErrorCode::UnsupportedOperation,
                format!(
                    "operation `{}` has no model scenario and must run locally",
                    request.operation.as_str()
                ),
            )
        })?;
        let candidates = self
            .providers
            .candidates_for_scenario(scenario, workspace_id)
            .map_err(|e| CanvasError::new(CanvasErrorCode::InvalidInput, e.to_string()))?;
        if candidates.is_empty() {
            return Err(CanvasError::new(
                CanvasErrorCode::NoCandidateModel,
                format!(
                    "no enabled model is bound to scenario `{}`",
                    scenario.as_str()
                ),
            ));
        }
        let mut last_error = None;
        for candidate in candidates {
            match self.call(&candidate, request).await {
                Ok(output) => return Ok(output),
                Err(error) if error.is_transient() => {
                    tracing::warn!(
                        provider_id = %candidate.provider.id,
                        model_id = %candidate.model.id,
                        error = %error,
                        "canvas candidate failed transiently; trying next model"
                    );
                    last_error = Some(error);
                }
                Err(error) => return Err(error),
            }
        }
        Err(last_error.unwrap_or_else(|| {
            CanvasError::new(
                CanvasErrorCode::TransientFailure,
                "all canvas candidates failed".to_string(),
            )
        }))
    }

    async fn call(
        &self,
        candidate: &ResolvedCanvasModel,
        request: &CanvasModelRequest,
    ) -> CanvasResult<CanvasModelOutput> {
        let api_key = candidate
            .api_key
            .clone()
            .filter(|k| !k.trim().is_empty())
            .ok_or_else(|| {
                CanvasError::new(
                    CanvasErrorCode::SecretMissing,
                    format!(
                        "provider `{}` has no stored api key",
                        candidate.provider.name
                    ),
                )
            })?;
        let kind = request_kind(request.operation)?;
        let endpoint = endpoint_url(
            candidate.provider.protocol,
            &candidate.provider.base_url,
            kind,
            Some(&candidate.model.id),
        )
        .map_err(|e| CanvasError::new(CanvasErrorCode::UnsupportedCapability, e.to_string()))?;

        let body = build_request_body(
            candidate.provider.protocol,
            kind,
            &candidate.model.id,
            request,
        )
        .map(|v| v.to_string())?;

        let timeout = Duration::from_millis(request.timeout_ms.max(1_000));
        let mut req = self
            .http
            .post(&endpoint)
            .timeout(timeout)
            .header(reqwest::header::CONTENT_TYPE, "application/json");
        req = apply_auth_headers(req, candidate.provider.protocol, &api_key);
        let resp = req.body(body).send().await.map_err(|e| {
            CanvasError::new(
                CanvasErrorCode::TransientFailure,
                format!(
                    "{} request failed: {e}",
                    candidate.provider.protocol.as_str()
                ),
            )
        })?;
        let status = resp.status();
        let raw = resp.text().await.map_err(|e| {
            CanvasError::new(
                CanvasErrorCode::TransientFailure,
                format!(
                    "reading {} response: {e}",
                    candidate.provider.protocol.as_str()
                ),
            )
        })?;
        if !status.is_success() {
            return Err(classify_status(status.as_u16(), &raw));
        }
        let value: Value = serde_json::from_str(&raw).map_err(|e| {
            CanvasError::new(
                CanvasErrorCode::ProviderFailure,
                format!(
                    "{} returned non-JSON body: {e}",
                    candidate.provider.protocol.as_str()
                ),
            )
        })?;
        if let Some(err) = provider_error_message(&value) {
            return Err(CanvasError::new(CanvasErrorCode::ProviderFailure, err));
        }
        parse_output(candidate, kind, &value).await
    }

    /// List the provider's real model catalog through `GET {base}/models`.
    ///
    /// Only the OpenAI-compatible protocol exposes a catalog endpoint; other
    /// protocols surface `UnsupportedCapability` rather than a fake preset list.
    pub async fn discover_models(&self, provider_id: &str) -> CanvasResult<Vec<String>> {
        let provider = self
            .providers
            .provider(provider_id)
            .map_err(|e| CanvasError::new(CanvasErrorCode::InvalidInput, e.to_string()))?;
        let api_key = self
            .providers
            .provider_api_key(provider_id)
            .map_err(|e| CanvasError::new(CanvasErrorCode::InvalidInput, e.to_string()))?
            .filter(|k| !k.trim().is_empty())
            .ok_or_else(|| {
                CanvasError::new(
                    CanvasErrorCode::SecretMissing,
                    format!("provider `{}` has no stored api key", provider.name),
                )
            })?;
        let endpoint = endpoint_url(
            provider.protocol,
            &provider.base_url,
            CanvasRequestKind::Models,
            None,
        )
        .map_err(|e| CanvasError::new(CanvasErrorCode::UnsupportedCapability, e.to_string()))?;
        let request = apply_auth_headers(
            self.http.get(&endpoint).timeout(Duration::from_secs(30)),
            provider.protocol,
            &api_key,
        );
        let resp = request.send().await.map_err(|e| {
            CanvasError::new(
                CanvasErrorCode::TransientFailure,
                format!("model discovery failed: {e}"),
            )
        })?;
        let status = resp.status();
        let raw = resp.text().await.map_err(|e| {
            CanvasError::new(
                CanvasErrorCode::TransientFailure,
                format!("reading model catalog: {e}"),
            )
        })?;
        if !status.is_success() {
            return Err(classify_status(status.as_u16(), &raw));
        }
        let value: Value = serde_json::from_str(&raw).map_err(|e| {
            CanvasError::new(
                CanvasErrorCode::ProviderFailure,
                format!("model catalog is not valid JSON: {e}"),
            )
        })?;
        let items = value
            .get("data")
            .and_then(Value::as_array)
            .or_else(|| value.as_array())
            .cloned()
            .unwrap_or_default();
        let mut ids: Vec<String> = Vec::new();
        for item in items {
            if let Some(id) = item.get("id").and_then(Value::as_str) {
                let id = id.trim().to_string();
                if !id.is_empty() && !ids.contains(&id) {
                    ids.push(id);
                }
            }
        }
        Ok(ids)
    }

    /// Probe a saved provider/model with a minimal chat or embeddings call.
    /// The api key is read from the secret store, so the renderer never handles it.
    pub async fn test_connection(
        &self,
        provider_id: &str,
        model_id: &str,
        scenario: CanvasScenario,
    ) -> CanvasConnectionTestResult {
        let started = std::time::Instant::now();
        let resolved = match self.providers.resolve_model(provider_id, model_id) {
            Ok(resolved) => resolved,
            Err(error) => {
                return CanvasConnectionTestResult {
                    ok: false,
                    code: CanvasErrorCode::InvalidInput.as_str().to_string(),
                    message: error.to_string(),
                    endpoint: String::new(),
                    model_id: model_id.to_string(),
                    latency_ms: 0,
                }
            }
        };
        let kind = match scenario {
            CanvasScenario::Embedding => CanvasRequestKind::Embeddings,
            _ => CanvasRequestKind::Chat,
        };
        let endpoint_result = endpoint_url(
            resolved.provider.protocol,
            &resolved.provider.base_url,
            kind,
            Some(&resolved.model.id),
        );
        let endpoint = endpoint_result.unwrap_or_default();
        let request = CanvasModelRequest {
            operation: match scenario {
                CanvasScenario::Embedding => CanvasOperation::Embedding,
                _ => CanvasOperation::TextGenerate,
            },
            prompt: "ping".to_string(),
            system_prompt: None,
            images: Vec::new(),
            texts: vec!["ping".to_string()],
            size: None,
            timeout_ms: 30_000,
        };
        let result = self.call(&resolved, &request).await;
        let latency_ms = started.elapsed().as_millis() as u64;
        match result {
            Ok(_) => CanvasConnectionTestResult {
                ok: true,
                code: "Ok".to_string(),
                message: "连接成功".to_string(),
                endpoint,
                model_id: model_id.to_string(),
                latency_ms,
            },
            Err(error) => CanvasConnectionTestResult {
                ok: false,
                code: error.code.as_str().to_string(),
                message: error.message,
                endpoint,
                model_id: model_id.to_string(),
                latency_ms,
            },
        }
    }
}

/// Map an operation onto the endpoint family it must use. Generation and edit
/// never share an endpoint.
fn request_kind(operation: CanvasOperation) -> CanvasResult<CanvasRequestKind> {
    Ok(match operation {
        CanvasOperation::TextGenerate
        | CanvasOperation::VisionDescribe
        | CanvasOperation::NormalizeIntent
        | CanvasOperation::DirectorPlan
        | CanvasOperation::TemplateExpand
        | CanvasOperation::StoryboardPlan
        | CanvasOperation::CharacterFaceCompose
        | CanvasOperation::CharacterBodyCompose
        | CanvasOperation::CharacterStyleCompose => CanvasRequestKind::Chat,
        CanvasOperation::ImageGenerate => CanvasRequestKind::ImageGenerate,
        CanvasOperation::ImageEdit
        | CanvasOperation::ImageRemoveBackground
        | CanvasOperation::ImageUpscale
        | CanvasOperation::ImageRepaint => CanvasRequestKind::ImageEdit,
        CanvasOperation::Embedding => CanvasRequestKind::Embeddings,
        CanvasOperation::SpeechTranscribe => CanvasRequestKind::SpeechTranscribe,
        CanvasOperation::SpeechSynthesize => CanvasRequestKind::SpeechSynthesize,
        other => {
            return Err(CanvasError::new(
                CanvasErrorCode::UnsupportedOperation,
                format!(
                    "operation `{}` is not executable by the canvas gateway",
                    other.as_str()
                ),
            ))
        }
    })
}

fn apply_auth_headers(
    req: reqwest::RequestBuilder,
    protocol: CanvasProtocol,
    api_key: &str,
) -> reqwest::RequestBuilder {
    match protocol {
        CanvasProtocol::Anthropic => req
            .header("x-api-key", api_key)
            .header("anthropic-version", ANTHROPIC_VERSION),
        // Local Gemini gateways accept bearer auth; Google's native endpoint reads
        // the same value from `x-goog-api-key`. Both are sent to the configured
        // host only, so the key never travels anywhere the user did not set.
        CanvasProtocol::Gemini => req
            .header("Authorization", format!("Bearer {api_key}"))
            .header("x-goog-api-key", api_key),
        CanvasProtocol::OpenAi => req.header("Authorization", format!("Bearer {api_key}")),
    }
}

/// Classify a non-2xx provider response. 401/403/4xx argument errors must not
/// trigger candidate switching.
fn classify_status(status: u16, raw: &str) -> CanvasError {
    let snippet: String = raw.chars().take(400).collect();
    let message = format!("provider returned HTTP {status}: {snippet}");
    let code = match status {
        408 | 425 | 429 | 500..=599 => CanvasErrorCode::TransientFailure,
        401 | 403 => CanvasErrorCode::SecretMissing,
        _ => CanvasErrorCode::ProviderFailure,
    };
    CanvasError::new(code, message)
}

/// Providers may answer 200 with an embedded error object.
fn provider_error_message(value: &Value) -> Option<String> {
    value
        .get("error")
        .map(|err| match err {
            Value::String(text) => text.clone(),
            other => other
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("provider reported an error")
                .to_string(),
        })
        .or_else(|| {
            value
                .get("promptFeedback")
                .and_then(|pf| pf.get("blockReason"))
                .and_then(Value::as_str)
                .map(ToString::to_string)
        })
}

fn build_request_body(
    protocol: CanvasProtocol,
    kind: CanvasRequestKind,
    model_id: &str,
    request: &CanvasModelRequest,
) -> CanvasResult<Value> {
    Ok(match (protocol, kind) {
        (CanvasProtocol::OpenAi, CanvasRequestKind::Chat) => {
            let mut messages = Vec::new();
            if let Some(system) = request
                .system_prompt
                .as_ref()
                .filter(|s| !s.trim().is_empty())
            {
                messages.push(json!({ "role": "system", "content": system }));
            }
            messages.push(json!({
                "role": "user",
                "content": openai_user_content(request),
            }));
            json!({ "model": model_id, "messages": messages, "stream": false })
        }
        (CanvasProtocol::OpenAi, CanvasRequestKind::Embeddings) => json!({
            "model": model_id,
            "input": embedding_input(request),
        }),
        (CanvasProtocol::OpenAi, CanvasRequestKind::ImageGenerate) => json!({
            "model": model_id,
            "prompt": request.prompt,
            "n": 1,
            "size": request.size.clone().unwrap_or_else(|| "1024x1024".to_string()),
            "response_format": "b64_json",
        }),
        (CanvasProtocol::OpenAi, CanvasRequestKind::ImageEdit) => {
            if request.images.is_empty() {
                return Err(CanvasError::new(
                    CanvasErrorCode::MissingReferenceInput,
                    "image edit requires at least one image artifact".to_string(),
                ));
            }
            json!({
                "model": model_id,
                "prompt": request.prompt,
                "image": request.images.iter().map(|i| i.data_url.clone()).collect::<Vec<_>>(),
                "n": 1,
                "response_format": "b64_json",
            })
        }
        (CanvasProtocol::Anthropic, CanvasRequestKind::Chat) => {
            if !request.images.is_empty() && request.images.len() > MAX_ANTHROPIC_IMAGES {
                return Err(CanvasError::new(
                    CanvasErrorCode::TooManyReferences,
                    format!("anthropic accepts at most {MAX_ANTHROPIC_IMAGES} reference images"),
                ));
            }
            let mut body = json!({
                "model": model_id,
                "max_tokens": 4096,
                "messages": [{ "role": "user", "content": anthropic_user_content(request) }],
            });
            if let Some(system) = request
                .system_prompt
                .as_ref()
                .filter(|s| !s.trim().is_empty())
            {
                body["system"] = json!(system);
            }
            body
        }
        (CanvasProtocol::Gemini, CanvasRequestKind::Chat)
        | (CanvasProtocol::Gemini, CanvasRequestKind::ImageGenerate) => {
            let mut body = json!({
                "contents": [{
                    "role": "user",
                    "parts": gemini_parts(request),
                }],
            });
            if let Some(system) = request
                .system_prompt
                .as_ref()
                .filter(|s| !s.trim().is_empty())
            {
                body["systemInstruction"] = json!({ "parts": [{ "text": system }] });
            }
            body
        }
        (CanvasProtocol::Gemini, CanvasRequestKind::Embeddings) => json!({
            "model": model_id,
            "content": { "parts": [{ "text": request.prompt }] },
            "outputDimensionality": 1024,
        }),
        (_, other) => {
            let label = match other {
                CanvasRequestKind::Chat => "chat",
                CanvasRequestKind::Embeddings => "embeddings",
                CanvasRequestKind::ImageGenerate => "image_generate",
                CanvasRequestKind::ImageEdit => "image_edit",
                CanvasRequestKind::SpeechTranscribe => "speech_transcribe",
                CanvasRequestKind::SpeechSynthesize => "speech_synthesize",
                CanvasRequestKind::Models => "models",
            };
            return Err(CanvasError::new(
                CanvasErrorCode::UnsupportedCapability,
                format!(
                    "provider protocol `{}` does not support `{label}` (UnsupportedCapability)",
                    protocol.as_str()
                ),
            ));
        }
    })
}

const MAX_ANTHROPIC_IMAGES: usize = 20;

fn openai_user_content(request: &CanvasModelRequest) -> Value {
    if request.images.is_empty() {
        return json!(request.prompt);
    }
    let mut parts = vec![json!({ "type": "text", "text": request.prompt })];
    for image in &request.images {
        parts.push(json!({ "type": "image_url", "image_url": { "url": image.data_url } }));
    }
    Value::Array(parts)
}

fn anthropic_user_content(request: &CanvasModelRequest) -> Value {
    let mut parts = vec![json!({ "type": "text", "text": request.prompt })];
    for image in &request.images {
        if let Some((mime, data)) = split_data_url(&image.data_url) {
            parts.push(json!({
                "type": "image",
                "source": { "type": "base64", "media_type": mime, "data": data },
            }));
        }
    }
    Value::Array(parts)
}

fn gemini_parts(request: &CanvasModelRequest) -> Vec<Value> {
    let mut parts = vec![json!({ "text": request.prompt })];
    for image in &request.images {
        if let Some((mime, data)) = split_data_url(&image.data_url) {
            parts.push(json!({ "inline_data": { "mime_type": mime, "data": data } }));
        }
    }
    parts
}

/// Embedding input: text list when present, otherwise a single image payload.
/// The model id never appears here — it stays a body field only.
fn embedding_input(request: &CanvasModelRequest) -> Value {
    if !request.texts.is_empty() {
        let texts: Vec<String> = request
            .texts
            .iter()
            .filter(|t| !t.trim().is_empty())
            .cloned()
            .collect();
        if texts.len() == 1 {
            return json!(texts[0]);
        }
        return Value::Array(texts.into_iter().map(|t| json!(t)).collect());
    }
    if let Some(image) = request.images.first() {
        return json!({ "image": image.data_url });
    }
    json!(request.prompt)
}

fn split_data_url(value: &str) -> Option<(String, String)> {
    let rest = value.strip_prefix("data:")?;
    let (meta, data) = rest.split_once(',')?;
    let mime = meta
        .split(';')
        .next()
        .unwrap_or("application/octet-stream")
        .to_string();
    Some((mime, data.to_string()))
}

async fn parse_output(
    candidate: &ResolvedCanvasModel,
    kind: CanvasRequestKind,
    value: &Value,
) -> CanvasResult<CanvasModelOutput> {
    let provider_id = candidate.provider.id.clone();
    let model_id = candidate.model.id.clone();
    match kind {
        CanvasRequestKind::Chat => {
            let text = extract_text(candidate.provider.protocol, value).ok_or_else(|| {
                CanvasError::new(
                    CanvasErrorCode::ProviderFailure,
                    "provider response contained no text".to_string(),
                )
            })?;
            Ok(CanvasModelOutput::Text {
                text,
                reasoning: extract_reasoning(candidate.provider.protocol, value),
                provider_id,
                model_id,
            })
        }
        CanvasRequestKind::Embeddings => {
            let vector = value
                .get("data")
                .and_then(Value::as_array)
                .and_then(|items| items.first())
                .and_then(|item| item.get("embedding"))
                .and_then(Value::as_array)
                .map(|values| {
                    values
                        .iter()
                        .filter_map(|v| v.as_f64().map(|f| f as f32))
                        .collect::<Vec<f32>>()
                })
                .filter(|v| !v.is_empty())
                .ok_or_else(|| {
                    CanvasError::new(
                        CanvasErrorCode::ProviderFailure,
                        "embedding response contained no vector".to_string(),
                    )
                })?;
            let dimensions = vector.len();
            Ok(CanvasModelOutput::Embedding {
                vector,
                dimensions,
                provider_id,
                model_id,
            })
        }
        CanvasRequestKind::ImageGenerate | CanvasRequestKind::ImageEdit => {
            let first = value
                .get("data")
                .and_then(Value::as_array)
                .and_then(|items| items.first())
                .cloned()
                .or_else(|| {
                    // Gemini returns images inline in the content parts.
                    value
                        .get("candidates")
                        .and_then(Value::as_array)
                        .and_then(|c| c.first())
                        .and_then(|c| c.get("content"))
                        .and_then(|c| c.get("parts"))
                        .and_then(Value::as_array)
                        .and_then(|parts| {
                            parts.iter().find_map(|part| {
                                part.get("inlineData")
                                    .or_else(|| part.get("inline_data"))
                                    .cloned()
                            })
                        })
                })
                .ok_or_else(|| {
                    CanvasError::new(
                        CanvasErrorCode::ProviderFailure,
                        "image response contained no image".to_string(),
                    )
                })?;
            let inline = first
                .get("b64_json")
                .or_else(|| first.get("data"))
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty());
            if let Some(b64) = inline {
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(b64.trim())
                    .map_err(|e| {
                        CanvasError::new(
                            CanvasErrorCode::ProviderFailure,
                            format!("image payload is not valid base64: {e}"),
                        )
                    })?;
                return Ok(CanvasModelOutput::Image {
                    mime: sniff_image_mime(&bytes).unwrap_or_else(|| "image/png".to_string()),
                    bytes,
                    provider_id,
                    model_id,
                });
            }
            let url = first
                .get("url")
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| {
                    CanvasError::new(
                        CanvasErrorCode::ProviderFailure,
                        "image response contained neither b64_json nor url".to_string(),
                    )
                })?;
            let bytes = fetch_binary(url, None).await?;
            Ok(CanvasModelOutput::Image {
                mime: sniff_image_mime(&bytes).unwrap_or_else(|| mime_from_url(url)),
                bytes,
                provider_id,
                model_id,
            })
        }
        CanvasRequestKind::Models => Err(CanvasError::new(
            CanvasErrorCode::ProviderFailure,
            "model catalogs are parsed by discover_models".to_string(),
        )),
        CanvasRequestKind::SpeechTranscribe | CanvasRequestKind::SpeechSynthesize => {
            Err(CanvasError::new(
                CanvasErrorCode::UnsupportedCapability,
                format!(
                    "speech {} is not enabled for provider protocol `{}`",
                    if kind == CanvasRequestKind::SpeechTranscribe {
                        "transcription"
                    } else {
                        "synthesis"
                    },
                    candidate.provider.protocol.as_str()
                ),
            ))
        }
    }
}

/// Download a provider-hosted artifact (image responses frequently return a
/// temporary URL instead of inline bytes).
async fn fetch_binary(url: &str, api_key: Option<&str>) -> CanvasResult<Vec<u8>> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(120))
        .build()
        .map_err(|e| {
            CanvasError::new(
                CanvasErrorCode::TransientFailure,
                format!("create download client: {e}"),
            )
        })?;
    let mut req = client.get(url);
    if let Some(key) = api_key {
        req = req.header("Authorization", format!("Bearer {key}"));
    }
    let resp = req.send().await.map_err(|e| {
        CanvasError::new(
            CanvasErrorCode::TransientFailure,
            format!("download provider artifact: {e}"),
        )
    })?;
    let status = resp.status();
    if !status.is_success() {
        return Err(classify_status(status.as_u16(), "artifact download failed"));
    }
    resp.bytes().await.map(|b| b.to_vec()).map_err(|e| {
        CanvasError::new(
            CanvasErrorCode::TransientFailure,
            format!("read provider artifact bytes: {e}"),
        )
    })
}

/// Classify a node-stored media reference from its mime, not its config key.
fn classify_media_reference(value: &str) -> Option<&'static str> {
    let mime = if let Some(rest) = value.strip_prefix("data:") {
        rest.split(';').next().unwrap_or_default().to_string()
    } else if value.starts_with("http://") || value.starts_with("https://") {
        mime_from_url(value)
    } else {
        return None;
    };
    if mime.starts_with("image/") {
        Some("Image")
    } else if mime.starts_with("video/") {
        Some("Video")
    } else if mime.starts_with("audio/") {
        Some("Audio")
    } else if mime == "application/json" {
        Some("Json")
    } else if mime.starts_with("text/") {
        Some("Text")
    } else {
        None
    }
}

/// Detect an image type from its magic bytes.
///
/// Providers label payloads inconsistently, and the type reported here becomes
/// the stored artifact's extension plus the `data:` prefix sent back to other
/// providers, so the bytes win over any declared name.
fn sniff_image_mime(bytes: &[u8]) -> Option<String> {
    let mime = match bytes {
        [0x89, b'P', b'N', b'G', ..] => "image/png",
        [0xff, 0xd8, 0xff, ..] => "image/jpeg",
        [b'G', b'I', b'F', b'8', ..] => "image/gif",
        [b'R', b'I', b'F', b'F', _, _, _, _, b'W', b'E', b'B', b'P', ..] => "image/webp",
        _ => return None,
    };
    Some(mime.to_string())
}

fn mime_from_url(url: &str) -> String {
    let path = url.split_once("://").map(|(_, rest)| rest).unwrap_or(url);
    let tail = path.rsplit('/').next().unwrap_or_default();
    match tail
        .split_once('?')
        .map(|(a, _)| a)
        .unwrap_or(tail)
        .rsplit('.')
        .next()
    {
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("webp") => "image/webp",
        _ => "application/octet-stream",
    }
    .to_string()
}

/// Pull the provider's reasoning block out of a response, when it returns one.
fn extract_reasoning(protocol: CanvasProtocol, value: &Value) -> Option<String> {
    match protocol {
        CanvasProtocol::Anthropic => Some(
            value
                .get("content")?
                .as_array()?
                .iter()
                .filter(|part| part.get("type").and_then(Value::as_str) == Some("thinking"))
                .filter_map(|part| part.get("thinking").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("\n"),
        )
        .and_then(non_empty),
        CanvasProtocol::OpenAi => value
            .get("choices")?
            .as_array()?
            .first()?
            .get("message")?
            .get("reasoning_content")
            .and_then(Value::as_str)
            .filter(|text| !text.trim().is_empty())
            .map(ToString::to_string),
        CanvasProtocol::Gemini => None,
    }
}

/// Pull assistant text out of any supported protocol's response shape.
fn extract_text(protocol: CanvasProtocol, value: &Value) -> Option<String> {
    match protocol {
        CanvasProtocol::OpenAi => value
            .get("choices")?
            .as_array()?
            .first()?
            .get("message")?
            .get("content")
            .and_then(content_to_text),
        CanvasProtocol::Anthropic => Some(
            value
                .get("content")?
                .as_array()?
                .iter()
                .filter(|part| part.get("type").and_then(Value::as_str) == Some("text"))
                .filter_map(|part| part.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join(""),
        )
        .and_then(non_empty),
        CanvasProtocol::Gemini => Some(
            value
                .get("candidates")?
                .as_array()?
                .first()?
                .get("content")?
                .get("parts")?
                .as_array()?
                .iter()
                .filter_map(|part| part.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join(""),
        )
        .and_then(non_empty),
    }
}

/// Keep provider text only when it is not blank.
fn non_empty(text: String) -> Option<String> {
    (!text.trim().is_empty()).then_some(text)
}

/// OpenAI chat content is either a plain string or a part list.
fn content_to_text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Array(parts) => {
            let text: String = parts
                .iter()
                .filter_map(|part| {
                    part.get("text")
                        .and_then(Value::as_str)
                        .or_else(|| part.get("content").and_then(Value::as_str))
                })
                .collect::<Vec<_>>()
                .join("");
            (!text.is_empty()).then_some(text)
        }
        _ => None,
    }
}

/// The separator the canvas UI uses to address one model of one provider.
pub const MODEL_REF_SEPARATOR: &str = "::";

/// Split a `providerId::modelId` reference. A bare legacy name (no separator)
/// yields `None`, which means "route by scenario candidates".
pub fn decode_model_ref(model_ref: &str) -> Option<(&str, &str)> {
    let index = model_ref.find(MODEL_REF_SEPARATOR)?;
    if index == 0 {
        return None;
    }
    let provider_id = model_ref[..index].trim();
    let model_id = model_ref[index + MODEL_REF_SEPARATOR.len()..].trim();
    if provider_id.is_empty() || model_id.is_empty() {
        return None;
    }
    Some((provider_id, model_id))
}

/// Assemble the system prompt for a canvas node call.
///
/// A creative node owns only a profile reference, so the profile's system
/// prompt plus its allowed skill blocks are resolved here. A professional node
/// keeps its own system prompt unchanged, which preserves existing workflows.
pub fn assemble_system_prompt(
    node_kind: &str,
    node_system_prompt: Option<&str>,
    skill_ids: &[String],
) -> CoreResult<Option<String>> {
    if node_kind.trim().is_empty() {
        return Ok(node_system_prompt
            .map(str::to_string)
            .filter(|value| !value.trim().is_empty()));
    }
    let contract = canvas_node_contract(node_kind)?;
    let profile = deepagent_prompts::canvas_prompt::creative_profile(node_kind)?;
    let allowed: std::collections::BTreeSet<&str> = contract
        .allowed_skill_ids
        .iter()
        .map(String::as_str)
        .collect();
    let mut selected: Vec<String> = Vec::new();
    for skill_id in skill_ids {
        if !allowed.contains(skill_id.as_str()) {
            return Err(deepagent_core::error::CoreError::invalid(format!(
                "UnsupportedOperation: skill `{skill_id}` is not allowed for node `{node_kind}`"
            )));
        }
        if !selected.iter().any(|existing| existing == skill_id) {
            selected.push(skill_id.clone());
        }
    }
    let _ = node_system_prompt;
    let mut blocks = vec![profile.system_prompt.clone()];
    for skill_id in &selected {
        let skill = deepagent_prompts::canvas_prompt::canvas_skill(skill_id).ok_or_else(|| {
            deepagent_core::error::CoreError::invalid(format!("unknown canvas skill `{skill_id}`"))
        })?;
        blocks.push(skill.system_prompt.clone());
    }
    Ok(Some(blocks.join("\n\n")))
}

fn to_core_error(error: CanvasError) -> deepagent_core::error::CoreError {
    deepagent_core::error::CoreError::other(error.to_string())
}

#[async_trait]
impl CanvasModelBridge for CanvasModelGateway {
    async fn complete(
        &self,
        request: CanvasCompletionRequest,
    ) -> CoreResult<CanvasCompletionResponse> {
        let system_prompt = assemble_system_prompt(
            &request.node_kind,
            request.system_prompt.as_deref(),
            &request.skill_ids,
        )?;
        let model_request = CanvasModelRequest {
            operation: CanvasOperation::TextGenerate,
            prompt: request.prompt.clone(),
            system_prompt,
            images: self.resolve_image_inputs(&request.images)?,
            texts: Vec::new(),
            size: None,
            timeout_ms: 120_000,
        };
        validate_request(&model_request).map_err(to_core_error)?;
        let output = if let Some((provider_id, model_id)) = decode_model_ref(&request.model_ref) {
            self.execute_on(provider_id, model_id, &model_request)
                .await
                .map_err(to_core_error)?
        } else {
            self.execute(&model_request, None)
                .await
                .map_err(to_core_error)?
        };
        match output {
            CanvasModelOutput::Text {
                text,
                reasoning,
                provider_id,
                model_id,
            } => Ok(CanvasCompletionResponse {
                text,
                reasoning,
                provider_id,
                model_id,
            }),
            other => Err(deepagent_core::error::CoreError::other(format!(
                "text node received a non-text model output: {other:?}"
            ))),
        }
    }

    async fn generate_image(&self, request: CanvasImageRequest) -> CoreResult<CanvasImageResponse> {
        let has_reference = !request.reference_images.is_empty();
        let operation = match request.operation.trim() {
            "image_generate" => CanvasOperation::ImageGenerate,
            "image_edit" => CanvasOperation::ImageEdit,
            "" => {
                if has_reference {
                    CanvasOperation::ImageEdit
                } else {
                    CanvasOperation::ImageGenerate
                }
            }
            other => {
                return Err(deepagent_core::error::CoreError::invalid(format!(
                    "UnsupportedOperation: image node cannot run `{other}`"
                )))
            }
        };
        let model_request = CanvasModelRequest {
            operation,
            prompt: request.prompt.clone(),
            system_prompt: None,
            images: self.resolve_image_inputs(&request.reference_images)?,
            texts: Vec::new(),
            size: request.size.clone(),
            timeout_ms: 300_000,
        };
        validate_request(&model_request).map_err(to_core_error)?;
        let output = if let Some((provider_id, model_id)) = decode_model_ref(&request.model_ref) {
            self.execute_on(provider_id, model_id, &model_request)
                .await
                .map_err(to_core_error)?
        } else {
            self.execute(&model_request, None)
                .await
                .map_err(to_core_error)?
        };
        match output {
            CanvasModelOutput::Image {
                mime,
                bytes,
                provider_id,
                model_id,
            } => {
                let stored = self
                    .artifact_store("store a generated image")?
                    .import_bytes(
                        deepagent_persistence::artifact_store::ArtifactKind::Image,
                        Some(&mime),
                        &bytes,
                        None,
                    )?;
                Ok(CanvasImageResponse {
                    artifact_uri: stored.uri,
                    mime,
                    provider_id,
                    model_id: model_id.clone(),
                    operation: if operation == CanvasOperation::ImageEdit {
                        "edit".to_string()
                    } else {
                        "generate".to_string()
                    },
                })
            }
            other => Err(deepagent_core::error::CoreError::other(format!(
                "image node received a non-image model output: {other:?}"
            ))),
        }
    }

    fn inspect_input_kinds(&self, references: &[String]) -> CoreResult<Vec<String>> {
        references
            .iter()
            .map(|reference| {
                let trimmed = reference.trim();
                if let Some(id) = crate::canvas_artifact_service::artifact_id_from_uri(trimmed) {
                    let kind = self
                        .artifact_store("inspect an input reference")?
                        .record(id)?
                        .map(|record| match record.kind {
                            deepagent_persistence::artifact_store::ArtifactKind::Image => "Image",
                            deepagent_persistence::artifact_store::ArtifactKind::Video => "Video",
                            deepagent_persistence::artifact_store::ArtifactKind::Audio => "Audio",
                            _ => "Document",
                        })
                        .unwrap_or("Unknown");
                    return Ok(kind.to_string());
                }
                Ok(classify_media_reference(trimmed)
                    .unwrap_or("Unknown")
                    .to_string())
            })
            .collect()
    }

    fn route_operation(&self, request: CanvasRouteRequest) -> CoreResult<CanvasRouteOutcome> {
        let contract = canvas_node_contract(&request.node_kind)?;
        let input_kinds = request
            .input_kinds
            .iter()
            .filter_map(|kind| match kind.trim().to_ascii_lowercase().as_str() {
                "text" => Some(CanvasInputKind::Text),
                "json" => Some(CanvasInputKind::Json),
                "image" => Some(CanvasInputKind::Image),
                "video" => Some(CanvasInputKind::Video),
                "audio" => Some(CanvasInputKind::Audio),
                "document" => Some(CanvasInputKind::Document),
                "empty" => Some(CanvasInputKind::Empty),
                _ => None,
            })
            .collect::<Vec<_>>();
        let explicit_operation = match request.explicit_operation.as_deref() {
            None => None,
            Some(label) => Some(
                CanvasOperation::from_label(label)
                    .ok_or_else(|| {
                        CanvasError::new(
                            CanvasErrorCode::UnsupportedOperation,
                            format!(
                                "unknown operation `{label}` for node `{}`",
                                request.node_kind
                            ),
                        )
                    })
                    .map_err(to_core_error)?,
            ),
        };
        let decision = DeterministicRouteResolver::resolve(&RouteFacts {
            node_kind: request.node_kind.clone(),
            allowed_operations: contract.allowed_operations,
            explicit_operation,
            input_kinds,
            has_prompt: request.has_prompt,
        })
        .map_err(to_core_error)?;
        Ok(CanvasRouteOutcome {
            operation: decision.operation.as_str().to_string(),
            reason: decision.reason.to_string(),
        })
    }

    async fn embed(&self, request: CanvasEmbeddingRequest) -> CoreResult<CanvasEmbeddingResponse> {
        let model_request = CanvasModelRequest {
            operation: CanvasOperation::Embedding,
            prompt: request.texts.first().cloned().unwrap_or_default(),
            system_prompt: None,
            images: Vec::new(),
            texts: request.texts.clone(),
            size: None,
            timeout_ms: 120_000,
        };
        let output = if let Some((provider_id, model_id)) = decode_model_ref(&request.model_ref) {
            self.execute_on(provider_id, model_id, &model_request)
                .await
                .map_err(to_core_error)?
        } else {
            self.execute(&model_request, None)
                .await
                .map_err(to_core_error)?
        };
        match output {
            CanvasModelOutput::Embedding {
                vector,
                dimensions,
                provider_id,
                model_id,
            } => Ok(CanvasEmbeddingResponse {
                dimensions,
                count: 1,
                provider_id,
                model_id,
                vectors: vec![vector],
            }),
            other => Err(deepagent_core::error::CoreError::other(format!(
                "embedding node received a non-vector model output: {other:?}"
            ))),
        }
    }
}

/// Validate that the caller's request matches the routed operation before any
/// network call happens, so a mis-wired node fails fast.
pub fn validate_request(request: &CanvasModelRequest) -> CanvasResult<()> {
    validate_operation_inputs(
        request.operation,
        &RouteFacts {
            node_kind: String::new(),
            allowed_operations: vec![request.operation],
            explicit_operation: Some(request.operation),
            input_kinds: request
                .images
                .iter()
                .map(|_| CanvasInputKind::Image)
                .chain(request.texts.iter().map(|_| CanvasInputKind::Text))
                .collect(),
            has_prompt: !request.prompt.trim().is_empty(),
        },
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canvas_provider_service::{
        CanvasModelConfig, CanvasProviderInput, CanvasProviderService,
    };
    use deepagent_persistence::Database;
    use std::sync::Arc;

    fn facts(
        node_kind: &str,
        allowed: &[CanvasOperation],
        explicit: Option<CanvasOperation>,
        kinds: &[CanvasInputKind],
        prompt: bool,
    ) -> RouteFacts {
        RouteFacts {
            node_kind: node_kind.to_string(),
            allowed_operations: allowed.to_vec(),
            explicit_operation: explicit,
            input_kinds: kinds.to_vec(),
            has_prompt: prompt,
        }
    }

    #[test]
    fn image_gen_routes_generate_without_image_and_edit_with_image() {
        let allowed = vec![CanvasOperation::ImageGenerate, CanvasOperation::ImageEdit];
        let generated = DeterministicRouteResolver::resolve(&facts(
            "image-gen",
            &allowed,
            None,
            &[CanvasInputKind::Text],
            true,
        ))
        .expect("auto generate");
        assert_eq!(generated.operation, CanvasOperation::ImageGenerate);
        assert_eq!(generated.reason, "artifact_facts");

        let edited = DeterministicRouteResolver::resolve(&facts(
            "image-gen",
            &allowed,
            None,
            &[CanvasInputKind::Text, CanvasInputKind::Image],
            true,
        ))
        .expect("auto edit");
        assert_eq!(edited.operation, CanvasOperation::ImageEdit);
    }

    #[test]
    fn explicit_operations_conflict_or_missing_input() {
        let allowed = vec![CanvasOperation::ImageGenerate, CanvasOperation::ImageEdit];
        let conflict = DeterministicRouteResolver::resolve(&facts(
            "image-gen",
            &allowed,
            Some(CanvasOperation::ImageGenerate),
            &[CanvasInputKind::Image],
            true,
        ))
        .expect_err("generate + image conflicts");
        assert_eq!(conflict.code, CanvasErrorCode::OperationInputConflict);

        let missing = DeterministicRouteResolver::resolve(&facts(
            "image-edit",
            &[CanvasOperation::ImageEdit],
            Some(CanvasOperation::ImageEdit),
            &[],
            true,
        ))
        .expect_err("edit without image");
        assert_eq!(missing.code, CanvasErrorCode::MissingReferenceInput);
    }

    #[test]
    fn disallowed_explicit_operation_is_rejected_not_rerouted() {
        let err = DeterministicRouteResolver::resolve(&facts(
            "image-gen",
            &[CanvasOperation::ImageGenerate],
            Some(CanvasOperation::ImageEdit),
            &[CanvasInputKind::Image],
            true,
        ))
        .expect_err("not allowed");
        assert_eq!(err.code, CanvasErrorCode::OperationInputConflict);
    }

    #[test]
    fn image_edit_node_never_downgrades_to_generation() {
        let err = DeterministicRouteResolver::resolve(&facts(
            "image-edit",
            &[CanvasOperation::ImageEdit],
            None,
            &[CanvasInputKind::Text],
            true,
        ))
        .expect_err("no image means no route");
        assert_eq!(err.code, CanvasErrorCode::RouteAmbiguous);
    }

    #[test]
    fn unregistered_node_is_unsupported_not_passthrough() {
        let err = DeterministicRouteResolver::resolve(&facts("mystery", &[], None, &[], true))
            .expect_err("empty contract");
        assert_eq!(err.code, CanvasErrorCode::UnsupportedNode);
    }

    #[test]
    fn video_auto_routing_stays_ambiguous_until_explicit_operation() {
        let resolved = DeterministicRouteResolver::resolve(&facts(
            "video-gen",
            &[CanvasOperation::VideoGenerate, CanvasOperation::VideoEdit],
            None,
            &[CanvasInputKind::Video],
            true,
        ))
        .expect_err("video edit vs generate cannot be guessed from facts");
        assert_eq!(resolved.code, CanvasErrorCode::RouteAmbiguous);
    }

    #[test]
    fn speech_and_embedding_rules_are_enforced() {
        let missing_audio = DeterministicRouteResolver::resolve(&facts(
            "audio",
            &[
                CanvasOperation::SpeechTranscribe,
                CanvasOperation::SpeechSynthesize,
            ],
            Some(CanvasOperation::SpeechTranscribe),
            &[CanvasInputKind::Text],
            true,
        ))
        .expect_err("no audio");
        assert_eq!(missing_audio.code, CanvasErrorCode::MissingReferenceInput);

        let ok = DeterministicRouteResolver::resolve(&facts(
            "audio",
            &[CanvasOperation::SpeechTranscribe],
            Some(CanvasOperation::SpeechTranscribe),
            &[CanvasInputKind::Audio],
            false,
        ))
        .expect("audio transcribe");
        assert_eq!(ok.operation, CanvasOperation::SpeechTranscribe);
    }

    #[test]
    fn body_builders_put_the_model_only_in_the_body() {
        let request = CanvasModelRequest {
            operation: CanvasOperation::Embedding,
            prompt: "hello".to_string(),
            system_prompt: None,
            images: Vec::new(),
            texts: vec!["hello".to_string(), "world".to_string()],
            size: None,
            timeout_ms: 1000,
        };
        let body = build_request_body(
            CanvasProtocol::OpenAi,
            CanvasRequestKind::Embeddings,
            "Qwen/Qwen3-VL-Embedding-8B",
            &request,
        )
        .expect("body");
        assert_eq!(body["model"], "Qwen/Qwen3-VL-Embedding-8B");
        assert_eq!(body["input"].as_array().expect("inputs").len(), 2);
    }

    #[test]
    fn image_edit_body_requires_reference_images() {
        let request = CanvasModelRequest {
            operation: CanvasOperation::ImageEdit,
            prompt: "change the sky".to_string(),
            system_prompt: None,
            images: vec![CanvasImageInput {
                data_url: "data:image/png;base64,AAAA".to_string(),
            }],
            texts: Vec::new(),
            size: None,
            timeout_ms: 1000,
        };
        let body = build_request_body(
            CanvasProtocol::OpenAi,
            CanvasRequestKind::ImageEdit,
            "gpt-image-2",
            &request,
        )
        .expect("body");
        assert_eq!(body["model"], "gpt-image-2");
        assert_eq!(body["image"].as_array().expect("images").len(), 1);
    }

    #[test]
    fn unsupported_protocol_capability_is_explicit() {
        let request = CanvasModelRequest::text("hi");
        let err = build_request_body(
            CanvasProtocol::Anthropic,
            CanvasRequestKind::Embeddings,
            "deepseek-flash",
            &request,
        )
        .expect_err("anthropic has no embeddings");
        assert_eq!(err.code, CanvasErrorCode::UnsupportedCapability);
    }

    #[test]
    fn anthropic_thinking_blocks_stay_out_of_the_answer_text() {
        let value = json!({
            "content": [
                { "type": "thinking", "thinking": "用户只要 pong" },
                { "type": "text", "text": "pong" },
            ]
        });
        assert_eq!(
            extract_text(CanvasProtocol::Anthropic, &value).as_deref(),
            Some("pong")
        );
        assert_eq!(
            extract_reasoning(CanvasProtocol::Anthropic, &value).as_deref(),
            Some("用户只要 pong")
        );
    }

    #[test]
    fn empty_b64_payload_is_not_accepted_as_an_inline_image() {
        let resolved = ResolvedCanvasModel {
            provider: crate::canvas_provider_service::CanvasProviderConfig {
                id: "p".to_string(),
                name: "P".to_string(),
                protocol: CanvasProtocol::OpenAi,
                base_url: "https://x".to_string(),
                enabled: true,
                logo: None,
                models: vec![],
            },
            model: CanvasModelConfig {
                id: "gpt-image-2".to_string(),
                name: String::new(),
                description: String::new(),
                enabled: true,
                scenarios: vec![CanvasScenario::ImageGeneration],
                priority: 0,
            },
            api_key: Some("sk".to_string()),
        };
        let payload = json!({ "data": [{ "b64_json": "", "url": "" }] });
        let error = futures::executor::block_on(parse_output(
            &resolved,
            CanvasRequestKind::ImageGenerate,
            &payload,
        ))
        .expect_err("empty b64 and empty url must not produce an image");
        assert!(error.message.contains("b64_json"));
    }

    #[test]
    fn responses_from_each_protocol_are_parsed_to_text() {
        let openai = json!({ "choices": [{ "message": { "content": "你好" } }] });
        assert_eq!(
            extract_text(CanvasProtocol::OpenAi, &openai).as_deref(),
            Some("你好")
        );
        let anthropic = json!({ "content": [{ "type": "text", "text": "claude" }] });
        assert_eq!(
            extract_text(CanvasProtocol::Anthropic, &anthropic).as_deref(),
            Some("claude")
        );
        let gemini = json!({ "candidates": [{ "content": { "parts": [{ "text": "gem" }] } }] });
        assert_eq!(
            extract_text(CanvasProtocol::Gemini, &gemini).as_deref(),
            Some("gem")
        );
    }

    #[test]
    fn embedding_vector_and_image_outputs_are_parsed() {
        let resolved = ResolvedCanvasModel {
            provider: crate::canvas_provider_service::CanvasProviderConfig {
                id: "p1".to_string(),
                name: "P".to_string(),
                protocol: CanvasProtocol::OpenAi,
                base_url: "https://x".to_string(),
                enabled: true,
                logo: None,
                models: vec![],
            },
            model: CanvasModelConfig {
                id: "Qwen/Qwen3-VL-Embedding-8B".to_string(),
                name: String::new(),
                description: String::new(),
                enabled: true,
                scenarios: vec![CanvasScenario::Embedding],
                priority: 0,
            },
            api_key: Some("sk".to_string()),
        };
        let payload = json!({ "data": [{ "embedding": [0.5, -0.25, 1.0] }] });
        let out = futures::executor::block_on(parse_output(
            &resolved,
            CanvasRequestKind::Embeddings,
            &payload,
        ))
        .expect("vector");
        match out {
            CanvasModelOutput::Embedding {
                vector,
                dimensions,
                model_id,
                ..
            } => {
                assert_eq!(dimensions, 3);
                assert_eq!(vector.len(), 3);
                assert_eq!(model_id, "Qwen/Qwen3-VL-Embedding-8B");
            }
            other => panic!("unexpected {other:?}"),
        }

        let image_payload = json!({ "data": [{ "b64_json": "aGk=" }] });
        let out = futures::executor::block_on(parse_output(
            &resolved,
            CanvasRequestKind::ImageGenerate,
            &image_payload,
        ))
        .expect("image");
        match out {
            CanvasModelOutput::Image { bytes, .. } => assert_eq!(bytes, b"hi"),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn transient_status_switches_candidates_but_auth_errors_do_not() {
        assert!(classify_status(429, "rate limited").is_transient());
        assert!(classify_status(503, "unavailable").is_transient());
        assert!(!classify_status(401, "bad key").is_transient());
        assert!(!classify_status(400, "bad args").is_transient());
    }

    #[test]
    fn embedded_provider_error_on_200_is_reported() {
        let value = json!({ "error": { "message": "content policy" } });
        assert_eq!(
            provider_error_message(&value).as_deref(),
            Some("content policy")
        );
        assert!(provider_error_message(&json!({ "data": [] })).is_none());
    }

    #[test]
    fn image_operations_route_to_their_own_endpoints() {
        let request = CanvasModelRequest {
            operation: CanvasOperation::ImageGenerate,
            prompt: "a cube".to_string(),
            system_prompt: None,
            images: vec![CanvasImageInput {
                data_url: "data:image/png;base64,BB".to_string(),
            }],
            texts: Vec::new(),
            size: None,
            timeout_ms: 1000,
        };
        let error = validate_request(&request).expect_err("generate with an image conflicts");
        assert_eq!(error.code, CanvasErrorCode::OperationInputConflict);
    }

    #[test]
    fn creative_text_node_prompt_comes_from_its_profile() {
        let assembled = assemble_system_prompt("text-gen", None, &[])
            .expect("profile prompt")
            .expect("text-gen must have a system prompt");
        assert!(assembled.contains("你是通用文本创作节点"));
        assert!(!assembled.contains("通用图片提示词优化器"));
    }

    #[test]
    fn skill_blocks_are_allowlisted_and_appended() {
        let style: Vec<String> = vec!["image.style.extract.v1".to_string()];
        let assembled = assemble_system_prompt("character-style", None, &style)
            .expect("allowed skill")
            .expect("prompt present");
        assert!(assembled.contains("你是角色视觉风格节点"));
        assert!(assembled.contains("通用视觉风格提炼助手"));

        let error = assemble_system_prompt("text-gen", None, &style)
            .expect_err("text-gen does not allow that skill");
        assert!(error.to_string().contains("not allowed for node"));
    }

    #[test]
    fn professional_nodes_keep_their_own_system_prompt() {
        let kept = assemble_system_prompt("", Some("自定义专业节点提示词"), &[])
            .expect("professional prompt")
            .expect("present");
        assert_eq!(kept, "自定义专业节点提示词");
        assert!(assemble_system_prompt("", None, &[])
            .expect("no prompt")
            .is_none());
    }

    #[test]
    fn creative_nodes_cannot_override_their_profile_prompt() {
        let assembled = assemble_system_prompt("text-gen", Some("节点自己写的系统提示词"), &[])
            .expect("profile prompt")
            .expect("present");
        assert!(assembled.contains("你是通用文本创作节点"));
        assert!(!assembled.contains("节点自己写的系统提示词"));
    }

    #[test]
    fn data_url_splitting_extracts_mime_and_payload() {
        let (mime, data) = split_data_url("data:image/webp;base64,ABCD").expect("split");
        assert_eq!(mime, "image/webp");
        assert_eq!(data, "ABCD");
        assert!(split_data_url("https://example.com/a.png").is_none());
    }

    #[test]
    fn gateway_reports_no_candidate_model_when_scenario_is_unbound() {
        let db = Arc::new(Database::open_in_memory().expect("db"));
        let providers = Arc::new(CanvasProviderService::new(
            db,
            Arc::new(crate::secret_store::MemorySecretStore::default()),
        ));
        let gateway = CanvasModelGateway::new(providers);
        let err =
            futures::executor::block_on(gateway.execute(&CanvasModelRequest::text("hi"), None))
                .expect_err("no models configured");
        assert_eq!(err.code, CanvasErrorCode::NoCandidateModel);
    }

    #[test]
    fn gateway_calls_openai_compatible_endpoints_end_to_end() {
        use std::io::{Read, Write};
        let rt = tokio::runtime::Runtime::new().expect("tokio runtime");

        // A local responder proves endpoint shape, auth header and body wiring
        // without depending on a live vendor.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        // Bytes a provider would hand back; the PNG magic is what the gateway
        // must trust over any declared type.
        let provider_png =
            base64::engine::general_purpose::STANDARD.encode([0x89u8, b'P', b'N', b'G', 0, 1]);
        let received: Arc<std::sync::Mutex<Vec<(String, String, String)>>> =
            Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = received.clone();
        let image_payload = provider_png.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut buf = [0u8; 2048];
                let mut text = String::new();
                // Read until the headers plus the declared body length arrive.
                loop {
                    match stream.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => {
                            text.push_str(&String::from_utf8_lossy(&buf[..n]));
                            if body_complete(&text) {
                                break;
                            }
                        }
                        Err(_) => break,
                    }
                }
                let head_end = text.find("\r\n\r\n").unwrap_or(text.len());
                let head = text[..head_end].to_lowercase();
                let request_line = text.lines().next().unwrap_or_default().to_string();
                let body = text[head_end + 4..].to_string();
                let payload = if request_line.contains("/embeddings") {
                    "{\"data\":[{\"embedding\":[0.1,0.2,0.3]}]}".to_string()
                } else if request_line.contains("/images/generations") {
                    format!("{{\"data\":[{{\"b64_json\":\"{image_payload}\"}}]}}")
                } else {
                    "{\"choices\":[{\"message\":{\"content\":\"pong\"}}]}".to_string()
                };
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
                    payload.len()
                );
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.flush();
                sink.lock().expect("lock").push((request_line, head, body));
            }
        });

        let db = Arc::new(Database::open_in_memory().expect("db"));
        let artifact_root = std::env::temp_dir().join(format!(
            "deepagent-gateway-artifacts-{}",
            std::process::id()
        ));
        let artifacts = Arc::new(
            crate::canvas_artifact_service::CanvasArtifactService::new(&artifact_root, db.clone())
                .expect("artifact service"),
        );
        let providers = Arc::new(CanvasProviderService::new(
            db,
            Arc::new(crate::secret_store::MemorySecretStore::default()),
        ));
        let created = providers
            .save_provider(CanvasProviderInput {
                name: "Local".to_string(),
                protocol: "openai".to_string(),
                base_url: format!("http://127.0.0.1:{port}/v1"),
                enabled: true,
                models: vec![
                    CanvasModelConfig {
                        id: "gpt-5.6-sol".to_string(),
                        name: "gpt-5.6-sol".to_string(),
                        description: String::new(),
                        enabled: true,
                        scenarios: vec![CanvasScenario::Text],
                        priority: 0,
                    },
                    CanvasModelConfig {
                        id: "gpt-image-2".to_string(),
                        name: "gpt-image-2".to_string(),
                        description: String::new(),
                        enabled: true,
                        scenarios: vec![CanvasScenario::ImageGeneration],
                        priority: 0,
                    },
                    CanvasModelConfig {
                        id: "Qwen/Qwen3-VL-Embedding-8B".to_string(),
                        name: "qwen-embed".to_string(),
                        description: String::new(),
                        enabled: true,
                        scenarios: vec![CanvasScenario::Embedding],
                        priority: 0,
                    },
                ],
                api_key: Some("sk-local".to_string()),
                ..Default::default()
            })
            .expect("provider");

        let gateway = CanvasModelGateway::new(providers.clone()).with_artifacts(artifacts.clone());
        let out = rt
            .block_on(gateway.execute_on(
                &created.id,
                "gpt-5.6-sol",
                &CanvasModelRequest::text("ping"),
            ))
            .expect("chat call");
        match out {
            CanvasModelOutput::Text { text, .. } => assert_eq!(text, "pong"),
            other => panic!("unexpected {other:?}"),
        }

        let embed_req = CanvasModelRequest {
            operation: CanvasOperation::Embedding,
            prompt: "ping".to_string(),
            system_prompt: None,
            images: Vec::new(),
            texts: vec!["ping".to_string()],
            size: None,
            timeout_ms: 5000,
        };
        let out = rt
            .block_on(gateway.execute_on(&created.id, "Qwen/Qwen3-VL-Embedding-8B", &embed_req))
            .expect("embedding call");
        match out {
            CanvasModelOutput::Embedding {
                dimensions, vector, ..
            } => {
                assert_eq!(dimensions, 3);
                assert_eq!(vector.len(), 3);
            }
            other => panic!("unexpected {other:?}"),
        }

        // The bridge must persist generated bytes and hand the node a reference.
        let image = rt
            .block_on(gateway.generate_image(
                deepagent_runtime::workflow::canvas::CanvasImageRequest {
                    model_ref: format!("{}::gpt-image-2", created.id),
                    prompt: "画一只戴帽子的橘猫".to_string(),
                    reference_images: Vec::new(),
                    size: Some("1024x1024".to_string()),
                    operation: "image_generate".to_string(),
                },
            ))
            .expect("image call");
        assert_eq!(image.mime, "image/png");
        assert_eq!(image.operation, "generate");
        assert!(
            image.artifact_uri.starts_with("artifact://"),
            "node saw {:?}",
            image.artifact_uri
        );
        let artifact_id = image
            .artifact_uri
            .trim_start_matches("artifact://")
            .to_string();
        assert_eq!(
            artifacts
                .read_bytes(&artifact_id)
                .expect("read")
                .expect("stored")
                .as_slice(),
            &[0x89, b'P', b'N', b'G', 0, 1]
        );
        let reused = artifacts
            .resolve_for_provider(&image.artifact_uri)
            .expect("resolve")
            .expect("provider-ready reference");
        assert!(
            reused.starts_with("data:image/png;base64,"),
            "resolved reference was {reused}"
        );

        let kinds = gateway
            .inspect_input_kinds(&[
                image.artifact_uri.clone(),
                "data:video/mp4;base64,AA".to_string(),
                "asset://localhost/x".to_string(),
            ])
            .expect("inspect kinds");
        assert_eq!(kinds, vec!["Image", "Video", "Unknown"]);

        let captured = received.lock().expect("lock").clone();
        assert_eq!(captured.len(), 3, "all three calls must reach the endpoint");
        let (chat_line, chat_head, chat_body) = &captured[0];
        assert!(
            chat_line.contains("POST /v1/chat/completions"),
            "chat line was {chat_line}"
        );
        assert!(chat_head.contains("authorization: bearer sk-local"));
        assert!(chat_body.contains("gpt-5.6-sol"));
        let (embed_line, _, embed_body) = &captured[1];
        assert!(
            embed_line.contains("POST /v1/embeddings"),
            "embedding line was {embed_line}"
        );
        // The slash-bearing model id is a body field only; it must not extend the route.
        assert!(
            !embed_line.contains("Qwen"),
            "route leaked model id: {embed_line}"
        );
        assert!(embed_body.contains("Qwen/Qwen3-VL-Embedding-8B"));
        assert!(providers.provider_api_key(&created.id).unwrap().as_deref() == Some("sk-local"));
        let (image_line, _, image_body) = &captured[2];
        assert!(
            image_line.contains("POST /v1/images/generations"),
            "image line was {image_line}"
        );
        assert!(image_body.contains("gpt-image-2"));
        assert!(image_body.contains("1024x1024"));
        let _ = std::fs::remove_dir_all(&artifact_root);
    }

    /// True once the headers plus the declared body length have been received.
    fn body_complete(text: &str) -> bool {
        let Some(head_end) = text.find("\r\n\r\n") else {
            return false;
        };
        let head = &text[..head_end];
        let declared = head
            .lines()
            .filter_map(|line| line.split_once(':'))
            .find(|(name, _)| name.trim().eq_ignore_ascii_case("content-length"))
            .and_then(|(_, value)| value.trim().parse::<usize>().ok())
            .unwrap_or(0);
        text.len() >= head_end + 4 + declared
    }
}
