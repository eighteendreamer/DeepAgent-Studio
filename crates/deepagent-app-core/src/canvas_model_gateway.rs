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

use std::collections::{BTreeSet, HashMap};
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::canvas_provider_usage::parse_usage;
use deepagent_core::error::Result as CoreResult;
use deepagent_runtime::workflow::canvas::{CanvasUsageReport, CanvasUsageSink};
use deepagent_runtime::workflow::{
    CanvasArtifactResponse, CanvasAudioInputPreparation, CanvasAudioRequest, CanvasAudioResponse,
    CanvasCompletionRequest, CanvasCompletionResponse, CanvasEmbeddingRequest,
    CanvasEmbeddingResponse, CanvasImagePreparation, CanvasImageRequest, CanvasImageResponse,
    CanvasJobPhase, CanvasJobProgress, CanvasModelBridge, CanvasPromptPolicy, CanvasRouteOutcome,
    CanvasRouteRequest, CanvasVideoPreparation, CanvasVideoRequest, CanvasVideoResponse,
};

use crate::canvas_node_contract::canvas_node_contract;
use crate::canvas_provider_service::{
    endpoint_url, CanvasModelConfig, CanvasProtocol, CanvasProviderService, CanvasRequestKind,
    CanvasScenario, ResolvedCanvasModel, ANTHROPIC_VERSION,
};
use crate::http_runtime::PublicHttpClient;
use deepagent_builtins::http_request::{HttpRequest, HttpRequestClient};
use deepagent_tools::{
    Permission, PermissionSet, RiskLevel, Tool, ToolDescriptor, ToolExecutionContext, ToolOutput,
};

mod video_download_limit;
use video_download_limit::VideoDownloadLimit;

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
    ResponseBodyTooLarge,
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
            Self::ResponseBodyTooLarge => "ResponseBodyTooLarge",
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

#[derive(Clone, Copy)]
struct VideoCancellation<'a> {
    flag: Option<&'a Arc<AtomicBool>>,
    message: &'a str,
}

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
    VideoTimelineEdit,
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
            Self::VideoTimelineEdit => "video_timeline_edit",
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
            Self::VideoTimelineEdit,
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
                | Self::VideoTimelineEdit
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
            if facts.node_kind == "video-gen"
                && matches!(
                    operation,
                    CanvasOperation::VideoEdit | CanvasOperation::VideoExtend
                )
            {
                return Err(CanvasError::new(
                    CanvasErrorCode::UnsupportedCapability,
                    format!(
                        "video operation `{}` is not connected to a provider request adapter",
                        operation.as_str()
                    ),
                ));
            }
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
            [single] => {
                // Same validator as the explicit path: an input rule must not be
                // bypassable by leaving the operation on auto.
                validate_operation_inputs(*single, facts)?;
                Ok(RouteDecision {
                    operation: *single,
                    reason: "artifact_facts",
                })
            }
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

/// Which media type an operation works on, when it works on media at all.
fn expected_media_kind(operation: CanvasOperation) -> Option<CanvasInputKind> {
    match operation {
        CanvasOperation::ImageGenerate
        | CanvasOperation::ImageEdit
        | CanvasOperation::ImageRemoveBackground
        | CanvasOperation::ImageUpscale
        | CanvasOperation::ImageRepaint
        | CanvasOperation::ImageCrop
        | CanvasOperation::ImageCompare
        | CanvasOperation::VisionDescribe => Some(CanvasInputKind::Image),
        CanvasOperation::VideoGenerate
        | CanvasOperation::VideoEdit
        | CanvasOperation::VideoExtend
        | CanvasOperation::VideoCompose => Some(CanvasInputKind::Video),
        _ => None,
    }
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
    // A media node may not be fed both media types. An image alone is still a
    // valid video input (the first frame); image + video together is a mistake.
    let is_media_operation = expected_media_kind(operation).is_some();
    if is_media_operation && has_image(facts) && has_video(facts) {
        return Err(CanvasError::new(
            CanvasErrorCode::OperationInputConflict,
            format!(
                "operation `{}` cannot mix image and video inputs",
                operation.as_str()
            ),
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

/// Speech payload. `reference` points at the audio to transcribe and the two
/// options are handed to the provider verbatim: the kernel never invents a
/// voice id, so an unset `voice` surfaces the provider's own error.
#[derive(Debug, Clone, Default)]
pub struct CanvasAudioInput {
    /// `artifact://`, `data:` or `http(s)` reference to the audio.
    pub reference: Option<String>,
    /// Provider voice id for synthesis.
    pub voice: Option<String>,
    /// Requested container, e.g. `mp3` or `wav`.
    pub format: Option<String>,
}

/// A resolved canvas model call.
#[derive(Debug, Clone)]
pub struct CanvasModelRequest {
    pub operation: CanvasOperation,
    pub prompt: String,
    pub system_prompt: Option<String>,
    /// Sampling control supplied by the canvas node. `None` means the
    /// provider/model default; an explicit value must be preserved or rejected
    /// by the protocol adapter, never silently dropped.
    pub temperature: Option<f32>,
    /// Maximum generated tokens supplied by the canvas node. `None` means the
    /// adapter's existing default policy; an explicit value is forwarded to
    /// the provider-specific field.
    pub max_tokens: Option<u32>,
    pub images: Vec<CanvasImageInput>,
    pub texts: Vec<String>,
    pub size: Option<String>,
    pub audio: CanvasAudioInput,
    pub timeout_ms: u64,
}

impl CanvasModelRequest {
    pub fn text(prompt: impl Into<String>) -> Self {
        Self {
            operation: CanvasOperation::TextGenerate,
            prompt: prompt.into(),
            system_prompt: None,
            temperature: None,
            max_tokens: None,
            images: Vec::new(),
            texts: Vec::new(),
            size: None,
            audio: CanvasAudioInput::default(),
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
        usage: Option<deepagent_models::Usage>,
        raw_usage: Option<Value>,
    },
    Image {
        mime: String,
        bytes: Vec<u8>,
        provider_id: String,
        model_id: String,
    },
    /// Provider returned a temporary URL.  The URL is kept inside the
    /// run-scoped gateway until the kernel-approved fetch tool consumes it.
    ImageUrl {
        url: String,
        provider_id: String,
        model_id: String,
    },
    Embedding {
        /// First vector retained for existing single-vector gateway callers.
        vector: Vec<f32>,
        /// Full batch, in input order. Workflow events expose only its count.
        vectors: Vec<Vec<f32>>,
        dimensions: usize,
        provider_id: String,
        model_id: String,
        usage: Option<deepagent_models::Usage>,
        raw_usage: Option<Value>,
    },
    Audio {
        mime: String,
        bytes: Vec<u8>,
        provider_id: String,
        model_id: String,
    },
}

/// State of an asynchronous provider media job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CanvasJobStatus {
    Queued,
    Running,
    Succeeded,
    Failed,
}

/// One poll result of a submitted media job.
#[derive(Debug, Clone)]
pub struct CanvasMediaJob {
    pub request_id: String,
    pub status: CanvasJobStatus,
    /// Temporary provider-hosted URL; the gateway downloads it into the
    /// artifact store because vendors expire these links quickly.
    pub video_url: Option<String>,
    pub reason: Option<String>,
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
    /// Immutable namespace captured at workflow admission, never active-project state.
    workspace_id: Option<String>,
    pending_artifacts: Arc<Mutex<HashMap<String, PendingArtifact>>>,
    public_http: Arc<PublicHttpClient>,
    video_download_limit: u64,
}

#[derive(Debug, Clone)]
struct VideoDownloadPlan {
    url: String,
    provider_id: String,
    model_id: String,
    task_id: String,
    resume_fingerprint: String,
}
struct PendingArtifact {
    url: String,
    origin: String,
    output: PendingArtifactOutput,
}

#[derive(Debug, Clone)]
enum PendingArtifactOutput {
    Image {
        provider_id: String,
        model_id: String,
        operation: String,
    },
    AudioInput,
    Video {
        provider_id: String,
        model_id: String,
        task_id: String,
        resume_fingerprint: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ArtifactFetchKind {
    Image,
    AudioInput,
    Video,
}

impl PendingArtifactOutput {
    fn kind(&self) -> ArtifactFetchKind {
        match self {
            Self::Image { .. } => ArtifactFetchKind::Image,
            Self::AudioInput => ArtifactFetchKind::AudioInput,
            Self::Video { .. } => ArtifactFetchKind::Video,
        }
    }
}

/// Kernel tool for provider-controlled media second hops. Its arguments contain
/// only a one-shot handle and a redacted origin; the URL stays in the gateway.
pub struct CanvasArtifactFetchTool {
    gateway: Arc<CanvasModelGateway>,
    kind: ArtifactFetchKind,
    enabled: bool,
}

/// Compatibility name for the existing provider-image fetch tool.
pub type CanvasImageFetchTool = CanvasArtifactFetchTool;

const CANVAS_IMAGE_FETCH_TOOL: &str = "canvas_image_fetch";
const CANVAS_AUDIO_FETCH_TOOL: &str = "canvas_audio_fetch";
const CANVAS_VIDEO_FETCH_TOOL: &str = "canvas_video_fetch";
/// Local policy for newly supported remote audio inputs, not a provider limit.
const MAX_REMOTE_AUDIO_BYTES: usize = 32 * 1024 * 1024;
/// Local default for a provider-generated video, independent of user imports
/// and provider capabilities. Trusted Rust hosts can configure a different limit.
pub const DEFAULT_VIDEO_DOWNLOAD_LIMIT: u64 = 32 * 1024 * 1024;
/// Error bodies are diagnostic input, not model output. Keep their memory and
/// provider-send budget independent from successful media payload limits.
const MAX_PROVIDER_ERROR_BODY_BYTES: usize = 64 * 1024;

impl CanvasArtifactFetchTool {
    fn parse(&self, arguments: &Value) -> CoreResult<(String, String)> {
        if !self.enabled {
            return Err(deepagent_core::error::CoreError::invalid(
                "NetworkDenied: selected sandbox has no HTTP broker; host fallback is forbidden",
            ));
        }
        let request_id = arguments
            .get("request_id")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                deepagent_core::error::CoreError::invalid("artifact fetch request_id is required")
            })?;
        let origin = arguments
            .get("origin")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                deepagent_core::error::CoreError::invalid("artifact fetch origin is required")
            })?;
        if self
            .gateway
            .pending_artifact_origin(request_id, self.kind)
            .as_deref()
            != Some(origin)
        {
            return Err(deepagent_core::error::CoreError::invalid(
                "artifact fetch handle is unknown or origin does not match",
            ));
        }
        Ok((request_id.to_string(), origin.to_string()))
    }
}

#[async_trait]
impl Tool for CanvasArtifactFetchTool {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: match self.kind {
                ArtifactFetchKind::Image => CANVAS_IMAGE_FETCH_TOOL,
                ArtifactFetchKind::AudioInput => CANVAS_AUDIO_FETCH_TOOL,
                ArtifactFetchKind::Video => CANVAS_VIDEO_FETCH_TOOL,
            }
            .to_string(),
            description: match self.kind {
                ArtifactFetchKind::Image => {
                    "Fetch one provider image artifact through the approved public network broker."
                }
                ArtifactFetchKind::AudioInput => {
                    "Fetch one remote audio input through the approved public network broker."
                }
                ArtifactFetchKind::Video => {
                    "Fetch one provider video artifact through the approved public network broker."
                }
            }
            .to_string(),
            parameters: json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "request_id": {"type": "string", "minLength": 1, "maxLength": 256},
                    "origin": {"type": "string", "minLength": 1, "maxLength": 512}
                },
                "required": ["request_id", "origin"]
            }),
            risk: RiskLevel::High,
            required_permissions: PermissionSet::from_iter_perms([Permission::Network]),
        }
    }

    fn validate_input(&self, arguments: Value) -> CoreResult<Value> {
        let (request_id, origin) = self.parse(&arguments)?;
        Ok(json!({"request_id": request_id, "origin": origin}))
    }

    async fn invoke(&self, arguments: Value) -> CoreResult<ToolOutput> {
        self.invoke_with_context(arguments, ToolExecutionContext::default())
            .await
    }

    async fn invoke_with_context(
        &self,
        arguments: Value,
        context: ToolExecutionContext,
    ) -> CoreResult<ToolOutput> {
        let (request_id, origin) = self.parse(&arguments)?;
        let response = self
            .gateway
            .fetch_artifact_handle(&request_id, &origin, self.kind, context)
            .await?;
        Ok(ToolOutput::success(response))
    }
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
            workspace_id: None,
            pending_artifacts: Arc::new(Mutex::new(HashMap::new())),
            public_http: Arc::new(PublicHttpClient::default()),
            video_download_limit: DEFAULT_VIDEO_DOWNLOAD_LIMIT,
        }
    }

    /// Create a gateway whose deferred image URLs live only for one workflow
    /// run. The provider, artifact and HTTP policy remain the same trusted
    /// services; only one-shot request state is isolated.
    pub(crate) fn scoped_for_run(
        self: &Arc<Self>,
        workspace_id: Option<&str>,
    ) -> CoreResult<Arc<Self>> {
        Ok(Arc::new(Self {
            providers: self.providers.clone(),
            http: self.http.clone(),
            artifacts: self.artifacts.clone(),
            workspace_id: crate::canvas_scope::normalize_workspace_id(workspace_id)?,
            pending_artifacts: Arc::new(Mutex::new(HashMap::new())),
            public_http: self.public_http.clone(),
            video_download_limit: self.video_download_limit,
        }))
    }

    #[must_use]
    pub fn with_artifacts(
        mut self,
        artifacts: std::sync::Arc<crate::canvas_artifact_service::CanvasArtifactService>,
    ) -> Self {
        self.artifacts = Some(artifacts);
        self
    }

    /// Set the byte ceiling for provider video downloads before sharing this
    /// gateway. Each run inherits the immutable policy; local video imports are
    /// unaffected. Zero/unindexable limits are rejected rather than unlimited.
    pub fn with_video_download_limit(mut self, max_bytes: u64) -> CoreResult<Self> {
        if max_bytes == 0 || max_bytes > i64::MAX as u64 {
            return Err(deepagent_core::error::CoreError::invalid(
                "video download limit must be positive and fit the artifact index",
            ));
        }
        self.video_download_limit = max_bytes;
        Ok(self)
    }

    /// Test-only route substitution for a synthetic public hostname. The
    /// production gateway always uses the system resolver and transport.
    #[cfg(test)]
    pub(crate) fn with_public_fixture_route(mut self, address: std::net::SocketAddr) -> Self {
        self.public_http = Arc::new(crate::http_runtime::public_fixture_client(address));
        self
    }

    fn stage_image_url(
        &self,
        url: String,
        provider_id: String,
        model_id: String,
        operation: String,
    ) -> CoreResult<CanvasImagePreparation> {
        let (request_id, origin) = self.stage_artifact_url(
            url,
            PendingArtifactOutput::Image {
                provider_id,
                model_id,
                operation,
            },
        )?;
        Ok(CanvasImagePreparation::Deferred { request_id, origin })
    }

    fn stage_artifact_url(
        &self,
        url: String,
        output: PendingArtifactOutput,
    ) -> CoreResult<(String, String)> {
        let parsed = reqwest::Url::parse(&url).map_err(|error| {
            deepagent_core::error::CoreError::invalid(format!("artifact URL is invalid: {error}"))
        })?;
        let origin = parsed.origin().ascii_serialization();
        let prefix = match output.kind() {
            ArtifactFetchKind::Image => "canvas_image",
            ArtifactFetchKind::AudioInput => "canvas_audio",
            ArtifactFetchKind::Video => "canvas_video",
        };
        let request_id = format!("{prefix}_{}", deepagent_core::id::EventId::new());
        self.pending_artifacts
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(
                request_id.clone(),
                PendingArtifact {
                    url,
                    origin: origin.clone(),
                    output,
                },
            );
        Ok((request_id, origin))
    }

    fn pending_artifact_origin(&self, request_id: &str, kind: ArtifactFetchKind) -> Option<String> {
        self.pending_artifacts
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(request_id)
            .filter(|pending| pending.output.kind() == kind)
            .map(|pending| pending.origin.clone())
    }

    #[cfg(test)]
    fn pending_image_origin(&self, request_id: &str) -> Option<String> {
        self.pending_artifact_origin(request_id, ArtifactFetchKind::Image)
    }

    #[cfg(test)]
    async fn fetch_image_handle(
        &self,
        request_id: &str,
        origin: &str,
        context: ToolExecutionContext,
    ) -> CoreResult<CanvasImageResponse> {
        Ok(serde_json::from_value(
            self.fetch_artifact_handle(request_id, origin, ArtifactFetchKind::Image, context)
                .await?,
        )?)
    }

    async fn fetch_artifact_handle(
        &self,
        request_id: &str,
        origin: &str,
        kind: ArtifactFetchKind,
        context: ToolExecutionContext,
    ) -> CoreResult<Value> {
        let pending = {
            let mut handles = self
                .pending_artifacts
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let pending = handles.get(request_id).ok_or_else(|| {
                deepagent_core::error::CoreError::invalid(
                    "artifact fetch handle is unknown or already consumed",
                )
            })?;
            if pending.origin != origin || pending.output.kind() != kind {
                return Err(deepagent_core::error::CoreError::invalid(
                    "artifact fetch origin or kind does not match its one-shot handle",
                ));
            }
            handles.remove(request_id).ok_or_else(|| {
                deepagent_core::error::CoreError::invalid(
                    "artifact fetch handle is unknown or already consumed",
                )
            })?
        };
        // One deadline covers DNS, every redirect, headers and the entire body.
        // A consumed handle is never restored, even when the wire outcome is unknown.
        tokio::select! {
            biased;
            _ = context.cancelled() => Err(deepagent_core::error::CoreError::other("media artifact fetch cancelled; staged artifact discarded; no automatic replay")),
            result = tokio::time::timeout(context.timeout(), self.download_artifact(pending)) => {
                result.map_err(|_| deepagent_core::error::CoreError::other("media artifact fetch deadline exceeded; outcome may be unknown; no automatic replay"))?
            }
        }
    }

    async fn download_artifact(&self, pending: PendingArtifact) -> CoreResult<Value> {
        use deepagent_core::error::CoreError;
        use deepagent_persistence::artifact_store::ArtifactKind;
        let request = HttpRequest {
            url: pending.url.clone(),
            method: "GET".to_string(),
            body: String::new(),
            headers: Default::default(),
            timeout_secs: 120,
            retry_count: 0,
        };
        self.public_http
            .validate(&request)
            .map_err(|error| CoreError::other(error.to_string()))?;
        let mut response = self.public_http.open_response(&request).await?;
        if !response.status().is_success() {
            return Err(CoreError::other(format!(
                "media artifact fetch failed with HTTP {}",
                response.status()
            )));
        }
        let kind = pending.output.kind();
        let mut video_limit = (kind == ArtifactFetchKind::Video)
            .then(|| VideoDownloadLimit::new(self.video_download_limit));
        if let Some(limit) = &video_limit {
            limit.check_declared(response.content_length())?;
        }
        let audio = kind == ArtifactFetchKind::AudioInput;
        if audio
            && response
                .content_length()
                .is_some_and(|size| size > MAX_REMOTE_AUDIO_BYTES as u64)
        {
            return Err(CoreError::invalid(
                "remote audio exceeds the 32 MiB download budget",
            ));
        }
        let mut prefix = Vec::new();
        // Resolve remote audio from actual bytes, not a filename or untrusted
        // Content-Type. At most the first transport chunks are buffered.
        if audio {
            while prefix.len() < 12 {
                let Some(chunk) = response.chunk().await.map_err(|error| {
                    CoreError::other(format!("read media artifact: {}", error.without_url()))
                })?
                else {
                    break;
                };
                if chunk.len() > MAX_REMOTE_AUDIO_BYTES - prefix.len() {
                    return Err(CoreError::invalid(
                        "remote audio exceeds the 32 MiB download budget",
                    ));
                }
                prefix.extend_from_slice(&chunk);
            }
        }
        let mime = if audio {
            sniff_audio_mime(&prefix).ok_or_else(|| {
                CoreError::invalid("remote audio has no supported audio container signature")
            })?
        } else if kind == ArtifactFetchKind::Video {
            response
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .map(|value| value.split(';').next().unwrap_or(value).trim().to_string())
                .filter(|value| value.starts_with("video/"))
                .unwrap_or_else(|| mime_from_url(&pending.url))
        } else {
            response
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .map(str::to_string)
                .unwrap_or_else(|| mime_from_url(&pending.url))
        };
        let mut stream = self
            .artifact_store("store a downloaded media artifact")?
            .begin_stream(
                match kind {
                    ArtifactFetchKind::AudioInput => ArtifactKind::Audio,
                    ArtifactFetchKind::Image => ArtifactKind::Image,
                    ArtifactFetchKind::Video => ArtifactKind::Video,
                },
                Some(&mime),
                None,
                self.workspace_id.as_deref(),
            )?;
        stream.write_chunk(&prefix)?;
        let mut size = prefix.len();
        while let Some(chunk) = response.chunk().await.map_err(|error| {
            CoreError::other(format!("read media artifact: {}", error.without_url()))
        })? {
            if let Some(limit) = &mut video_limit {
                limit.accept(chunk.len())?;
            }
            if audio && chunk.len() > MAX_REMOTE_AUDIO_BYTES - size {
                return Err(CoreError::invalid(
                    "remote audio exceeds the 32 MiB download budget",
                ));
            }
            if audio {
                size += chunk.len();
            }
            stream.write_chunk(&chunk)?;
        }
        // Let the enclosing cancellation/deadline guard observe readiness before publication.
        tokio::task::yield_now().await;
        let stored = stream.finish()?;
        let mime = stored.media_type.unwrap_or(mime);
        Ok(match pending.output {
            PendingArtifactOutput::Image {
                provider_id,
                model_id,
                operation,
            } => serde_json::to_value(CanvasImageResponse {
                artifact_uri: stored.uri,
                mime,
                provider_id,
                model_id,
                operation,
            })?,
            PendingArtifactOutput::AudioInput => serde_json::to_value(CanvasArtifactResponse {
                artifact_uri: stored.uri,
                mime,
            })?,
            PendingArtifactOutput::Video {
                provider_id,
                model_id,
                task_id,
                resume_fingerprint,
            } => serde_json::to_value(CanvasVideoResponse {
                artifact_uri: stored.uri,
                mime,
                provider_id,
                model_id,
                task_id,
                resume_fingerprint,
            })?,
        })
    }

    /// Preserve the existing image tool's name, schema and response contract.
    pub fn image_fetch_tool(self: &Arc<Self>) -> CanvasImageFetchTool {
        CanvasArtifactFetchTool {
            gateway: self.clone(),
            kind: ArtifactFetchKind::Image,
            enabled: true,
        }
    }

    /// Bind the same resolved backend capability as the existing typed HTTP tool.
    pub fn audio_fetch_tool(self: &Arc<Self>, enabled: bool) -> CanvasArtifactFetchTool {
        CanvasArtifactFetchTool {
            gateway: self.clone(),
            kind: ArtifactFetchKind::AudioInput,
            enabled,
        }
    }

    /// Video provider assets use the same resolved backend capability and
    /// public-network approval boundary as image and audio artifacts.
    pub fn video_fetch_tool(self: &Arc<Self>, enabled: bool) -> CanvasArtifactFetchTool {
        CanvasArtifactFetchTool {
            gateway: self.clone(),
            kind: ArtifactFetchKind::Video,
            enabled,
        }
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

    /// Bytes, container type and upload filename behind `audio.reference`.
    ///
    /// `artifact://` reads the managed blob, `data:` decodes inline bytes and
    /// Remote references must first become artifacts through the approved tool
    /// pipeline. Provider selection must never create an implicit host GET.
    async fn audio_input(
        &self,
        request: &CanvasModelRequest,
        workspace_id: Option<&str>,
    ) -> CanvasResult<(Vec<u8>, String, String)> {
        let reference = request
            .audio
            .reference
            .as_deref()
            .unwrap_or_default()
            .trim()
            .to_string();
        if reference.is_empty() {
            return Err(CanvasError::new(
                CanvasErrorCode::MissingReferenceInput,
                "speech_transcribe requires one audio artifact".to_string(),
            ));
        }
        let (bytes, mime) =
            if let Some(id) = crate::canvas_artifact_service::artifact_id_from_uri(&reference) {
                let store = self
                    .artifact_store("read an audio input")
                    .map_err(|e| CanvasError::new(CanvasErrorCode::InvalidInput, e.to_string()))?;
                let record = store
                    .record(id, workspace_id)
                    .map_err(|e| CanvasError::new(CanvasErrorCode::InvalidInput, e.to_string()))?
                    .ok_or_else(|| {
                        CanvasError::new(
                            CanvasErrorCode::InvalidInput,
                            format!("audio artifact `{id}` is missing from the store"),
                        )
                    })?;
                let bytes = store
                    .read_bytes(id, workspace_id)
                    .map_err(|e| CanvasError::new(CanvasErrorCode::InvalidInput, e.to_string()))?
                    .ok_or_else(|| {
                        CanvasError::new(
                            CanvasErrorCode::InvalidInput,
                            format!("audio artifact `{id}` has no stored bytes"),
                        )
                    })?;
                let mime = record.media_type.unwrap_or_else(|| "audio/wav".to_string());
                (bytes, mime)
            } else if let Some((head, payload)) = reference.split_once(";base64,") {
                use base64::Engine as _;
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(payload.trim())
                    .map_err(|e| {
                        CanvasError::new(
                            CanvasErrorCode::InvalidInput,
                            format!("decode inline audio: {e}"),
                        )
                    })?;
                let mime = head.strip_prefix("data:").unwrap_or(head).to_string();
                (bytes, mime)
            } else {
                return Err(CanvasError::invalid(
                "NetworkDenied: remote audio requires the approved workflow artifact tool pipeline",
            ));
            };
        if bytes.is_empty() {
            return Err(CanvasError::new(
                CanvasErrorCode::MissingReferenceInput,
                "audio input contains no bytes".to_string(),
            ));
        }
        // 魔数优先：供应商会按文件内容校验，声明与实际容器不一致时必须以字节为准。
        let mime = sniff_audio_mime(&bytes).unwrap_or(mime);
        let extension = mime.rsplit('/').next().unwrap_or("bin");
        Ok((bytes, mime.clone(), format!("audio.{extension}")))
    }

    /// Turn node-stored image references into what a provider can fetch:
    /// `artifact://id` becomes inline bytes, `data:` / `http(s)` pass through.
    fn resolve_image_inputs(&self, urls: &[String]) -> CoreResult<Vec<CanvasImageInput>> {
        urls.iter()
            .map(|url| {
                let resolved = self
                    .artifact_store("send a reference image")?
                    .resolve_for_provider(url, self.workspace_id.as_deref())?
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
        self.execute_on_observed(provider_id, model_id, request, None)
            .await
    }

    async fn execute_on_observed(
        &self,
        provider_id: &str,
        model_id: &str,
        request: &CanvasModelRequest,
        usage: Option<Arc<dyn CanvasUsageSink>>,
    ) -> CanvasResult<CanvasModelOutput> {
        let resolved = self
            .providers
            .resolve_model(provider_id, model_id)
            .map_err(|e| CanvasError::new(CanvasErrorCode::InvalidInput, e.to_string()))?;
        // An explicit model is not swapped out, so exceeding its declared limit
        // is the user's error to see rather than a silent reroute.
        reference_limit_error(&resolved.model, request.images.len())?;
        self.call(
            &resolved,
            request,
            usage.as_deref(),
            self.workspace_id.as_deref(),
        )
        .await
    }

    /// Run a request using the scenario candidate list, falling through only on
    /// transient failures. The operation is never recomputed while switching.
    pub async fn execute(
        &self,
        request: &CanvasModelRequest,
        workspace_id: Option<&str>,
    ) -> CanvasResult<CanvasModelOutput> {
        self.execute_observed(request, workspace_id, None).await
    }

    async fn execute_observed(
        &self,
        request: &CanvasModelRequest,
        workspace_id: Option<&str>,
        usage: Option<Arc<dyn CanvasUsageSink>>,
    ) -> CanvasResult<CanvasModelOutput> {
        let workspace_id = crate::canvas_scope::normalize_workspace_id(workspace_id)
            .map_err(|error| CanvasError::new(CanvasErrorCode::InvalidInput, error.to_string()))?;
        let workspace_id = workspace_id.as_deref();
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
        // 参数能力过滤（方案第十三节）：声明了上限又装不下的模型直接不参与。
        let images = request.images.len();
        let viable: Vec<ResolvedCanvasModel> = candidates
            .into_iter()
            .filter(|model| reference_limit_error(&model.model, images).is_ok())
            .collect();
        let candidates = if viable.is_empty() {
            return Err(CanvasError::new(
                CanvasErrorCode::TooManyReferences,
                format!(
                    "{images} reference image(s) exceed the declared limit of every candidate model"
                ),
            ));
        } else {
            viable
        };
        let mut last_error = None;
        for candidate in candidates {
            match self
                .call(&candidate, request, usage.as_deref(), workspace_id)
                .await
            {
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
        usage_sink: Option<&dyn CanvasUsageSink>,
        workspace_id: Option<&str>,
    ) -> CanvasResult<CanvasModelOutput> {
        let api_key = provider_api_key(candidate)?;
        let kind = request_kind(request.operation)?;
        let endpoint = endpoint_url(
            candidate.provider.protocol,
            &candidate.provider.base_url,
            kind,
            Some(&candidate.model.id),
        )
        .map_err(|e| CanvasError::new(CanvasErrorCode::UnsupportedCapability, e.to_string()))?;

        let (content_type, body) = match kind {
            CanvasRequestKind::SpeechTranscribe => {
                let (bytes, mime, filename) = self.audio_input(request, workspace_id).await?;
                (
                    format!("multipart/form-data; boundary={AUDIO_FORM_BOUNDARY}"),
                    transcription_form(&candidate.model.id, &bytes, &filename, &mime)?,
                )
            }
            _ => (
                "application/json".to_string(),
                build_request_body(
                    candidate.provider.protocol,
                    kind,
                    &candidate.model.id,
                    request,
                )?
                .to_string()
                .into_bytes(),
            ),
        };

        let timeout = Duration::from_millis(request.timeout_ms.max(1_000));
        let mut req = self
            .http
            .post(&endpoint)
            .timeout(timeout)
            .header(reqwest::header::CONTENT_TYPE, content_type);
        req = apply_auth_headers(req, candidate.provider.protocol, &api_key);
        if let Some(sink) = usage_sink {
            sink.before_call().map_err(|error| {
                CanvasError::new(CanvasErrorCode::InvalidInput, error.to_string())
            })?;
        }
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
        if !status.is_success() {
            let body = read_bounded_provider_body(
                resp,
                MAX_PROVIDER_ERROR_BODY_BYTES,
                VideoCancellation {
                    flag: None,
                    message: "provider error response read interrupted",
                },
                "provider error response",
            )
            .await;
            let value = body
                .as_ref()
                .ok()
                .and_then(|bytes| serde_json::from_slice::<Value>(bytes).ok());
            record_response_usage(candidate, kind, value.as_ref(), usage_sink)?;
            return Err(classify_status(
                status.as_u16(),
                &String::from_utf8_lossy(&body?),
            ));
        }
        // 语音合成回二进制，其余回 JSON，所以先按字节取再分流。
        let bytes = resp.bytes().await.map_err(|e| {
            CanvasError::new(
                CanvasErrorCode::TransientFailure,
                format!(
                    "reading {} response: {e}",
                    candidate.provider.protocol.as_str()
                ),
            )
        })?;
        let value = serde_json::from_slice::<Value>(&bytes);
        record_response_usage(candidate, kind, value.as_ref().ok(), usage_sink)?;
        if kind == CanvasRequestKind::SpeechSynthesize {
            if bytes.is_empty() {
                return Err(CanvasError::new(
                    CanvasErrorCode::ProviderFailure,
                    "speech synthesis returned an empty body".to_string(),
                ));
            }
            return Ok(CanvasModelOutput::Audio {
                mime: sniff_audio_mime(&bytes)
                    .or_else(|| request.audio.format.as_ref().map(|f| format!("audio/{f}")))
                    .unwrap_or_else(|| "application/octet-stream".to_string()),
                bytes: bytes.to_vec(),
                provider_id: candidate.provider.id.clone(),
                model_id: candidate.model.id.clone(),
            });
        }
        let value: Value = value.map_err(|e| {
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

    /// Send a small JSON control-plane request and return the parsed body.
    ///
    /// Media jobs need repeated plain JSON round-trips (submit, then poll);
    /// `call` handles the payload shapes that differ per capability.
    async fn post_json(
        &self,
        candidate: &ResolvedCanvasModel,
        endpoint: &str,
        api_key: &str,
        body: Value,
        timeout_ms: u64,
        cancellation: VideoCancellation<'_>,
    ) -> CanvasResult<Value> {
        let mut req = self
            .http
            .post(endpoint)
            .timeout(Duration::from_millis(timeout_ms.max(1_000)))
            .header(reqwest::header::CONTENT_TYPE, "application/json");
        req = apply_auth_headers(req, candidate.provider.protocol, api_key);
        let resp = await_video_operation(
            async {
                req.body(body.to_string()).send().await.map_err(|e| {
                    CanvasError::new(
                        CanvasErrorCode::TransientFailure,
                        format!(
                            "{} {endpoint} failed: {e}",
                            candidate.provider.protocol.as_str()
                        ),
                    )
                })
            },
            cancellation.flag,
            cancellation.message,
        )
        .await?;
        let status = resp.status();
        if !status.is_success() {
            let raw = read_bounded_provider_body(
                resp,
                MAX_PROVIDER_ERROR_BODY_BYTES,
                cancellation,
                "provider error response",
            )
            .await?;
            let raw = String::from_utf8_lossy(&raw);
            return Err(classify_status(status.as_u16(), &raw));
        }
        let raw = await_video_operation(
            async {
                resp.text().await.map_err(|e| {
                    CanvasError::new(
                        CanvasErrorCode::TransientFailure,
                        format!("reading {endpoint} response: {e}"),
                    )
                })
            },
            cancellation.flag,
            cancellation.message,
        )
        .await?;
        let value: Value = serde_json::from_str(&raw).map_err(|e| {
            CanvasError::new(
                CanvasErrorCode::ProviderFailure,
                format!("{endpoint} returned non-JSON body: {e}"),
            )
        })?;
        if let Some(err) = provider_error_message(&value) {
            return Err(CanvasError::new(CanvasErrorCode::ProviderFailure, err));
        }
        Ok(value)
    }

    /// Submit an asynchronous media job and return the provider task id.
    async fn submit_media_job(
        &self,
        candidate: &ResolvedCanvasModel,
        request: &CanvasModelRequest,
        cancel: Option<&Arc<AtomicBool>>,
    ) -> CanvasResult<String> {
        let api_key = provider_api_key(candidate)?;
        let endpoint = endpoint_url(
            candidate.provider.protocol,
            &candidate.provider.base_url,
            CanvasRequestKind::VideoSubmit,
            Some(&candidate.model.id),
        )
        .map_err(|e| CanvasError::new(CanvasErrorCode::UnsupportedCapability, e.to_string()))?;
        let mut body = json!({
            "model": candidate.model.id,
            "prompt": request.prompt,
        });
        if let Some(image) = request.images.first() {
            body["image"] = json!(image.data_url);
        }
        if let Some(size) = request
            .size
            .as_ref()
            .filter(|value| !value.trim().is_empty())
        {
            body["image_size"] = json!(size);
        }
        let value = self
            .post_json(
                candidate,
                &endpoint,
                &api_key,
                body,
                request.timeout_ms,
                VideoCancellation {
                    flag: cancel,
                    message: "Cancelled: video submission was interrupted; provider acceptance may be unknown",
                },
            )
            .await?;
        job_field(&value, "requestId").ok_or_else(|| {
            CanvasError::new(
                CanvasErrorCode::ProviderFailure,
                "video submit returned no request id".to_string(),
            )
        })
    }

    /// Ask the provider once about a submitted media job.
    async fn poll_media_job(
        &self,
        candidate: &ResolvedCanvasModel,
        request_id: &str,
        timeout_ms: u64,
        cancel: Option<&Arc<AtomicBool>>,
    ) -> CanvasResult<CanvasMediaJob> {
        let api_key = provider_api_key(candidate)?;
        let endpoint = endpoint_url(
            candidate.provider.protocol,
            &candidate.provider.base_url,
            CanvasRequestKind::VideoStatus,
            Some(&candidate.model.id),
        )
        .map_err(|e| CanvasError::new(CanvasErrorCode::UnsupportedCapability, e.to_string()))?;
        let value = self
            .post_json(
                candidate,
                &endpoint,
                &api_key,
                json!({ "requestId": request_id }),
                timeout_ms,
                VideoCancellation {
                    flag: cancel,
                    message: "Cancelled: video status wait interrupted; rerun with the same task id to keep polling",
                },
            )
            .await?;
        Ok(CanvasMediaJob {
            request_id: request_id.to_string(),
            status: parse_job_status(value.get("status").and_then(Value::as_str)),
            video_url: value
                .pointer("/results/videos/0/url")
                .and_then(Value::as_str)
                .map(str::to_string),
            reason: value
                .get("reason")
                .and_then(Value::as_str)
                .map(str::to_string),
        })
    }

    /// List the provider's real model catalog.
    ///
    /// Measured: OpenAI-compatible hosts answer `GET {base}/models` and Gemini
    /// gateways answer `GET {base}/v1beta/models`; a protocol with no catalog
    /// endpoint surfaces `UnsupportedCapability` rather than a fake preset list.
    pub async fn discover_models(&self, provider_id: &str) -> CanvasResult<Vec<String>> {
        let (provider, api_key) = self
            .providers
            .provider_credentials(provider_id)
            .map_err(|e| CanvasError::new(CanvasErrorCode::InvalidInput, e.to_string()))?;
        let api_key = api_key.filter(|k| !k.trim().is_empty()).ok_or_else(|| {
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
        if !status.is_success() {
            let raw = read_bounded_provider_body(
                resp,
                MAX_PROVIDER_ERROR_BODY_BYTES,
                VideoCancellation {
                    flag: None,
                    message: "model catalog error response read interrupted",
                },
                "model catalog error response",
            )
            .await?;
            let raw = String::from_utf8_lossy(&raw);
            return Err(classify_status(status.as_u16(), &raw));
        }
        let raw = resp.text().await.map_err(|e| {
            CanvasError::new(
                CanvasErrorCode::TransientFailure,
                format!("reading model catalog: {e}"),
            )
        })?;
        let value: Value = serde_json::from_str(&raw).map_err(|e| {
            CanvasError::new(
                CanvasErrorCode::ProviderFailure,
                format!("model catalog is not valid JSON: {e}"),
            )
        })?;
        let ids = model_ids_from_catalog(&value);
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
            audio: Default::default(),
            operation: match scenario {
                CanvasScenario::Embedding => CanvasOperation::Embedding,
                _ => CanvasOperation::TextGenerate,
            },
            prompt: "ping".to_string(),
            system_prompt: None,
            temperature: None,
            max_tokens: None,
            images: Vec::new(),
            texts: vec!["ping".to_string()],
            size: None,
            timeout_ms: 30_000,
        };
        let result = self
            .call(&resolved, &request, None, self.workspace_id.as_deref())
            .await;
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

#[cfg(test)]
#[path = "canvas_model_gateway/audio_input_tests.rs"]
mod audio_input_tests;

#[cfg(test)]
#[path = "canvas_model_gateway/error_body_tests.rs"]
mod error_body_tests;

#[cfg(test)]
#[path = "canvas_model_gateway/video_download_tests.rs"]
mod video_download_tests;

// Media workflows retain the real AppCore/Kernel/provider consumers while
// sharing the cfg(test)-only public DNS/local transport seam with the broker.
#[cfg(test)]
#[path = "../tests/workflow_audit/media.rs"]
mod workflow_media_audit;

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
        CanvasOperation::VideoGenerate => CanvasRequestKind::VideoSubmit,
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

/// Provider model catalog → usable model ids.
///
/// OpenAI-compatible hosts answer with `{"data":[{"id":...}]}`, a Gemini
/// gateway with `{"models":[{"name":"models/x"}]}`. Any other id is kept
/// exactly as the provider spelled it, because the id is what the request body
/// must echo back.
fn model_ids_from_catalog(value: &Value) -> Vec<String> {
    let items = value
        .get("data")
        .and_then(Value::as_array)
        .or_else(|| value.get("models").and_then(Value::as_array))
        .or_else(|| value.as_array())
        .cloned()
        .unwrap_or_default();
    let mut ids: Vec<String> = Vec::new();
    for item in items {
        let raw = item
            .get("id")
            .and_then(Value::as_str)
            .or_else(|| item.get("name").and_then(Value::as_str));
        let Some(raw) = raw else { continue };
        let raw = raw.trim();
        let id = raw.strip_prefix("models/").unwrap_or(raw).to_string();
        if !id.is_empty() && !ids.contains(&id) {
            ids.push(id);
        }
    }
    ids
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

/// Preserve one accounting observation per physical response, even if its body
/// was malformed or deliberately not consumed. Missing usage stays unknown.
fn record_response_usage(
    candidate: &ResolvedCanvasModel,
    kind: CanvasRequestKind,
    value: Option<&Value>,
    sink: Option<&dyn CanvasUsageSink>,
) -> CanvasResult<()> {
    if !matches!(
        kind,
        CanvasRequestKind::Chat | CanvasRequestKind::Embeddings
    ) {
        return Ok(());
    }
    if let Some(sink) = sink {
        let (usage, raw_usage) = value
            .map(|value| parse_usage(candidate.provider.protocol, kind, value))
            .unwrap_or((None, None));
        sink.record(CanvasUsageReport {
            provider_id: Some(candidate.provider.id.clone()),
            model_id: candidate.model.id.clone(),
            usage,
            raw_usage,
        });
        sink.checkpoint()
            .map_err(|error| CanvasError::new(CanvasErrorCode::InvalidInput, error.to_string()))?;
    }
    Ok(())
}

/// The api key of a resolved candidate, or the stable error saying it is gone.
pub(crate) fn provider_api_key(candidate: &ResolvedCanvasModel) -> CanvasResult<String> {
    candidate
        .api_key
        .clone()
        .filter(|key| !key.trim().is_empty())
        .ok_or_else(|| {
            CanvasError::new(
                CanvasErrorCode::SecretMissing,
                format!(
                    "provider `{}` has no stored api key",
                    candidate.provider.name
                ),
            )
        })
}

/// Reject input beyond the reference-image ceiling a model declares.
///
/// An undeclared limit is not a guess: the request goes out and the provider
/// answers for it.
fn reference_limit_error(model: &CanvasModelConfig, images: usize) -> CanvasResult<()> {
    let Some(limit) = model.max_reference_images else {
        return Ok(());
    };
    if images <= limit as usize {
        return Ok(());
    }
    Err(CanvasError::new(
        CanvasErrorCode::TooManyReferences,
        format!(
            "model `{}` accepts at most {limit} reference image(s), the node has {images}",
            model.id
        ),
    ))
}

/// Read one string field of a media job payload.
///
/// The vendor documents the submit response as `requestId` while the rest of
/// its payloads use `request_id`; both spellings are accepted here and nowhere
/// else, so no field-name guessing leaks into routing or the node protocol.
fn job_field(value: &Value, key: &str) -> Option<String> {
    // requestId -> request_id
    let snake: String = key
        .chars()
        .flat_map(|ch| {
            if ch.is_ascii_uppercase() {
                vec!['_', ch.to_ascii_lowercase()]
            } else {
                vec![ch]
            }
        })
        .collect();
    for name in [key, snake.as_str()] {
        if let Some(found) = value.get(name).and_then(Value::as_str) {
            let trimmed = found.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    None
}

/// Map a provider status word onto our job states. An unrecognised word stays
/// non-terminal so the poll loop keeps working until the deadline says so.
fn parse_job_status(raw: Option<&str>) -> CanvasJobStatus {
    match raw.unwrap_or_default().trim().to_ascii_lowercase().as_str() {
        "succeed" | "succeeded" | "success" | "completed" | "finished" => {
            CanvasJobStatus::Succeeded
        }
        "failed" | "error" => CanvasJobStatus::Failed,
        "inqueue" | "queued" | "pending" | "created" | "waiting" => CanvasJobStatus::Queued,
        other => {
            if !other.is_empty() {
                tracing::debug!("unknown media job status `{other}`; treating as running");
            }
            CanvasJobStatus::Running
        }
    }
}

/// Back off between polls: first immediately, then 1s, 2s, capped at 5s.
fn poll_delay(round: u32) -> Duration {
    match round {
        0 => Duration::ZERO,
        1 => Duration::from_secs(1),
        2 => Duration::from_secs(2),
        _ => Duration::from_secs(5),
    }
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
    validate_generation_options(protocol, kind, request)?;
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
            let mut body = json!({ "model": model_id, "messages": messages, "stream": false });
            if let Some(temperature) = request.temperature {
                body["temperature"] = json!(temperature);
            }
            if let Some(max_tokens) = request.max_tokens {
                body["max_tokens"] = json!(max_tokens);
            }
            body
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
            if let Some(max_tokens) = request.max_tokens {
                body["max_tokens"] = json!(max_tokens);
            }
            if let Some(temperature) = request.temperature {
                body["temperature"] = json!(temperature);
            }
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
            let mut generation_config = serde_json::Map::new();
            if let Some(temperature) = request.temperature {
                generation_config.insert("temperature".to_string(), json!(temperature));
            }
            if let Some(max_tokens) = request.max_tokens {
                generation_config.insert("maxOutputTokens".to_string(), json!(max_tokens));
            }
            if !generation_config.is_empty() {
                body["generationConfig"] = Value::Object(generation_config);
            }
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
        (CanvasProtocol::OpenAi, CanvasRequestKind::SpeechSynthesize) => {
            if request.prompt.trim().is_empty() {
                return Err(CanvasError::new(
                    CanvasErrorCode::MissingReferenceInput,
                    "text_to_speech requires text to speak".to_string(),
                ));
            }
            let mut body = json!({
                "model": model_id,
                "input": request.prompt,
            });
            // 音色由供应商定义，缺省时把供应商自己的报错原样交回，而不是挑一个默认值。
            if let Some(voice) = request
                .audio
                .voice
                .as_ref()
                .filter(|v| !v.trim().is_empty())
            {
                body["voice"] = json!(voice);
            }
            if let Some(format) = request
                .audio
                .format
                .as_ref()
                .filter(|f| !f.trim().is_empty())
            {
                body["response_format"] = json!(format);
            }
            body
        }
        (_, other) => {
            let label = match other {
                CanvasRequestKind::Chat => "chat",
                CanvasRequestKind::Embeddings => "embeddings",
                CanvasRequestKind::ImageGenerate => "image_generate",
                CanvasRequestKind::ImageEdit => "image_edit",
                CanvasRequestKind::SpeechTranscribe => "speech_transcribe",
                CanvasRequestKind::SpeechSynthesize => "speech_synthesize",
                CanvasRequestKind::VideoSubmit => "video_submit",
                CanvasRequestKind::VideoStatus => "video_status",
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

/// Validate the shared generation controls before any provider transport. The
/// numeric bounds are the common canvas contract; provider-specific serializers
/// retain the distinction between omitted and explicit values and map the
/// fields to their native names.
fn validate_generation_options(
    protocol: CanvasProtocol,
    kind: CanvasRequestKind,
    request: &CanvasModelRequest,
) -> CanvasResult<()> {
    if let Some(temperature) = request.temperature {
        if !temperature.is_finite() || !(0.0..=2.0).contains(&temperature) {
            return Err(CanvasError::invalid(format!(
                "InvalidGenerationOptions: temperature must be finite and between 0 and 2, got {temperature}"
            )));
        }
    }
    if let Some(max_tokens) = request.max_tokens {
        if max_tokens == 0 || max_tokens > 393_216 {
            return Err(CanvasError::invalid(format!(
                "InvalidGenerationOptions: max_tokens must be between 1 and 393216, got {max_tokens}"
            )));
        }
    }
    if (request.temperature.is_some() || request.max_tokens.is_some())
        && !matches!(
            (protocol, kind),
            (CanvasProtocol::OpenAi, CanvasRequestKind::Chat)
                | (CanvasProtocol::Anthropic, CanvasRequestKind::Chat)
                | (CanvasProtocol::Gemini, CanvasRequestKind::Chat)
                | (CanvasProtocol::Gemini, CanvasRequestKind::ImageGenerate)
        )
    {
        return Err(CanvasError::new(
            CanvasErrorCode::UnsupportedCapability,
            format!(
                "provider protocol `{}` does not support generation controls for `{kind:?}`",
                protocol.as_str()
            ),
        ));
    }
    Ok(())
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

/// Bind a provider-side task id to the exact effective request and account
/// configuration that created it. Only the digest crosses the event/UI
/// boundary; provider credentials and prompts remain inside the gateway.
fn video_input_fingerprint(
    candidate: &ResolvedCanvasModel,
    request: &CanvasVideoRequest,
    image: Option<&CanvasImageInput>,
) -> String {
    let parts = [
        "deepagent-video-resume-v1",
        candidate.provider.id.as_str(),
        candidate.model.id.as_str(),
        candidate.provider.protocol.as_str(),
        candidate.provider.base_url.trim().trim_end_matches('/'),
        candidate.provider.credential_version.as_str(),
        request.prompt.as_str(),
        request.size.as_deref().unwrap_or_default(),
        image
            .map(|input| input.data_url.as_str())
            .unwrap_or_default(),
    ];
    let parts: Vec<&[u8]> = parts.iter().map(|part| part.as_bytes()).collect();
    let key = candidate
        .api_key
        .as_deref()
        .filter(|key| !key.is_empty())
        .unwrap_or(candidate.provider.credential_version.as_str());
    let mac = hmac_sha256_framed_parts(key.as_bytes(), &parts);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(mac)
}

fn video_resume_fingerprint(input_fingerprint: &str, task_id: &str) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(hmac_sha256_framed_parts(
        input_fingerprint.as_bytes(),
        &[input_fingerprint.as_bytes(), task_id.as_bytes()],
    ))
}

#[cfg(test)]
fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    const BLOCK_SIZE: usize = 64;
    let mut normalized_key = [0u8; BLOCK_SIZE];
    if key.len() > BLOCK_SIZE {
        normalized_key[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        normalized_key[..key.len()].copy_from_slice(key);
    }

    let mut inner = Sha256::new();
    for byte in &normalized_key {
        inner.update([byte ^ 0x36]);
    }
    inner.update(message);
    hmac_sha256_finish(&normalized_key, inner.finalize())
}

fn hmac_sha256_framed_parts(key: &[u8], parts: &[&[u8]]) -> [u8; 32] {
    const BLOCK_SIZE: usize = 64;
    let mut normalized_key = [0u8; BLOCK_SIZE];
    if key.len() > BLOCK_SIZE {
        normalized_key[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        normalized_key[..key.len()].copy_from_slice(key);
    }

    let mut inner = Sha256::new();
    for byte in &normalized_key {
        inner.update([byte ^ 0x36]);
    }
    for part in parts {
        inner.update((part.len() as u64).to_be_bytes());
        inner.update(part);
    }
    hmac_sha256_finish(&normalized_key, inner.finalize())
}

fn hmac_sha256_finish(
    normalized_key: &[u8; 64],
    inner_digest: sha2::digest::Output<Sha256>,
) -> [u8; 32] {
    let mut outer = Sha256::new();
    for byte in normalized_key {
        outer.update([byte ^ 0x5c]);
    }
    outer.update(inner_digest);
    outer.finalize().into()
}

/// Parse all returned vectors rather than silently keeping the first input.
/// OpenAI-compatible endpoints may return indexed items out of order. Legacy
/// compatible endpoints without any index retain their response order.
fn parse_embedding_vectors(value: &Value) -> CanvasResult<Vec<Vec<f32>>> {
    let invalid = |reason: &str| {
        CanvasError::new(
            CanvasErrorCode::ProviderFailure,
            format!("invalid embedding response: {reason}"),
        )
    };
    let items = value
        .get("data")
        .and_then(Value::as_array)
        .filter(|items| !items.is_empty())
        .ok_or_else(|| invalid("no vectors"))?;
    let indexed = items.iter().any(|item| item.get("index").is_some());
    let mut vectors = vec![None; items.len()];
    let mut dimensions = None;
    for (position, item) in items.iter().enumerate() {
        let index = if indexed {
            item.get("index")
                .and_then(Value::as_u64)
                .and_then(|index| usize::try_from(index).ok())
                .filter(|&index| index < items.len())
                .ok_or_else(|| invalid("missing or out-of-range index"))?
        } else {
            position
        };
        let values = item
            .get("embedding")
            .and_then(Value::as_array)
            .filter(|values| !values.is_empty())
            .ok_or_else(|| invalid("empty or non-array vector"))?;
        if dimensions.is_some_and(|dimensions| dimensions != values.len()) {
            return Err(invalid("inconsistent vector dimensions"));
        }
        dimensions = Some(values.len());
        let vector = values
            .iter()
            .map(|value| {
                value
                    .as_f64()
                    .map(|number| number as f32)
                    .filter(|number| number.is_finite())
                    .ok_or_else(|| invalid("non-numeric or non-finite vector element"))
            })
            .collect::<CanvasResult<Vec<_>>>()?;
        if vectors[index].replace(vector).is_some() {
            return Err(invalid("duplicate vector index"));
        }
    }
    vectors
        .into_iter()
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| invalid("incomplete vector batch"))
}

async fn parse_output(
    candidate: &ResolvedCanvasModel,
    kind: CanvasRequestKind,
    value: &Value,
) -> CanvasResult<CanvasModelOutput> {
    let provider_id = candidate.provider.id.clone();
    let model_id = candidate.model.id.clone();
    let (usage, raw_usage) = parse_usage(candidate.provider.protocol, kind, value);
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
                usage,
                raw_usage,
            })
        }
        CanvasRequestKind::Embeddings => {
            let vectors = parse_embedding_vectors(value)?;
            let vector = vectors[0].clone();
            let dimensions = vector.len();
            Ok(CanvasModelOutput::Embedding {
                vector,
                vectors,
                dimensions,
                provider_id,
                model_id,
                usage,
                raw_usage,
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
            Ok(CanvasModelOutput::ImageUrl {
                url: url.to_string(),
                provider_id,
                model_id,
            })
        }
        CanvasRequestKind::Models => Err(CanvasError::new(
            CanvasErrorCode::ProviderFailure,
            "model catalogs are parsed by discover_models".to_string(),
        )),
        CanvasRequestKind::SpeechTranscribe => {
            let text = value
                .get("text")
                .and_then(Value::as_str)
                .map(str::to_string)
                .ok_or_else(|| {
                    CanvasError::new(
                        CanvasErrorCode::ProviderFailure,
                        "transcription response contained no text field".to_string(),
                    )
                })?;
            Ok(CanvasModelOutput::Text {
                text,
                reasoning: None,
                provider_id,
                model_id,
                usage: None,
                raw_usage: None,
            })
        }
        // 合成响应是二进制，call 在 JSON 解析前就返回了；走到这里说明接线错了。
        CanvasRequestKind::SpeechSynthesize => Err(CanvasError::new(
            CanvasErrorCode::ProviderFailure,
            "speech synthesis payloads are binary and handled before JSON parsing".to_string(),
        )),
        // 视频是异步作业：提交与轮询各有专门通道，不经一次性响应解析。
        CanvasRequestKind::VideoSubmit | CanvasRequestKind::VideoStatus => Err(CanvasError::new(
            CanvasErrorCode::ProviderFailure,
            "media jobs are parsed by submit_media_job and poll_media_job".to_string(),
        )),
    }
}

/// Await one video transport step while honoring the run's existing legacy
/// cancellation bridge. Dropping the request/body future closes the local HTTP
/// wait; it does not claim that a provider-side job or side effect was undone.
async fn await_video_operation<F, T>(
    future: F,
    cancel: Option<&Arc<AtomicBool>>,
    cancellation_message: &str,
) -> CanvasResult<T>
where
    F: Future<Output = CanvasResult<T>>,
{
    let Some(cancel) = cancel else {
        return future.await;
    };
    if cancel.load(Ordering::Acquire) {
        return Err(CanvasError::new(
            CanvasErrorCode::TransientFailure,
            cancellation_message,
        ));
    }
    tokio::select! {
        biased;
        _ = deepagent_runtime::cancellation::wait_for_legacy_flag(cancel.clone()) => {
            Err(CanvasError::new(
                CanvasErrorCode::TransientFailure,
                cancellation_message,
            ))
        }
        result = future => result,
    }
}

/// Read a provider response with a hard byte budget before turning it into an
/// error string or JSON value. The `Content-Length` check is only a fast path;
/// chunked responses are counted and dropped at the first over-budget chunk.
async fn read_bounded_provider_body(
    mut response: reqwest::Response,
    limit: usize,
    cancellation: VideoCancellation<'_>,
    label: &'static str,
) -> CanvasResult<Vec<u8>> {
    await_video_operation(
        async {
            let status = response.status().as_u16();
            let too_large = || {
                CanvasError::new(
                    CanvasErrorCode::ResponseBodyTooLarge,
                    format!("HTTP {status}: {label} exceeds {limit} byte limit (body omitted)"),
                )
            };
            if response
                .content_length()
                .is_some_and(|length| length > limit as u64)
            {
                return Err(too_large());
            }
            let mut body = Vec::with_capacity(limit);
            while let Some(chunk) = response.chunk().await.map_err(|error| {
                CanvasError::new(
                    CanvasErrorCode::TransientFailure,
                    format!("reading HTTP {status} {label}: {}", error.without_url()),
                )
            })? {
                if chunk.len() > limit.saturating_sub(body.len()) {
                    return Err(too_large());
                }
                body.extend_from_slice(&chunk);
            }
            Ok(body)
        },
        cancellation.flag,
        cancellation.message,
    )
    .await
}

async fn fetch_video_artifact(
    url: &str,
    artifacts: &crate::canvas_artifact_service::CanvasArtifactService,
    cancel: Option<&Arc<AtomicBool>>,
    task_id: &str,
    workspace_id: Option<&str>,
    max_bytes: u64,
) -> CanvasResult<crate::canvas_artifact_service::CanvasArtifactDto> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(120))
        .build()
        .map_err(|error| {
            CanvasError::new(
                CanvasErrorCode::TransientFailure,
                format!("create video download client: {error}"),
            )
        })?;
    let cancellation_message =
        format!("Cancelled: video task `{task_id}` interrupted while downloading; rerun to resume");
    let response = await_video_operation(
        async {
            client.get(url).send().await.map_err(|error| {
                CanvasError::new(
                    CanvasErrorCode::TransientFailure,
                    format!("download generated video: {error}"),
                )
            })
        },
        cancel,
        &cancellation_message,
    )
    .await?;
    let status = response.status();
    if !status.is_success() {
        return Err(classify_status(
            status.as_u16(),
            "video artifact download failed",
        ));
    }
    let mut limit = VideoDownloadLimit::new(max_bytes);
    limit
        .check_declared(response.content_length())
        .map_err(|error| {
            CanvasError::new(CanvasErrorCode::ResponseBodyTooLarge, error.to_string())
        })?;
    let media_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.split(';').next().unwrap_or(value).trim().to_string())
        .filter(|value| value.starts_with("video/"))
        .or_else(|| {
            let inferred = mime_from_url(url);
            (inferred != "application/octet-stream").then_some(inferred)
        });
    let mut stream = artifacts
        .begin_stream(
            deepagent_persistence::artifact_store::ArtifactKind::Video,
            media_type.as_deref(),
            None,
            workspace_id,
        )
        .map_err(|error| CanvasError::new(CanvasErrorCode::ProviderFailure, error.to_string()))?;
    let mut response = response;
    loop {
        let chunk = await_video_operation(
            async {
                response.chunk().await.map_err(|error| {
                    CanvasError::new(
                        CanvasErrorCode::TransientFailure,
                        format!("read generated video stream for task `{task_id}`: {error}"),
                    )
                })
            },
            cancel,
            &cancellation_message,
        )
        .await?;
        let Some(chunk) = chunk else {
            break;
        };
        limit.accept(chunk.len()).map_err(|error| {
            CanvasError::new(CanvasErrorCode::ResponseBodyTooLarge, error.to_string())
        })?;
        stream.write_chunk(&chunk).map_err(|error| {
            CanvasError::new(
                CanvasErrorCode::ProviderFailure,
                format!("store generated video stream for task `{task_id}`: {error}"),
            )
        })?;
    }
    if cancel.is_some_and(|flag| flag.load(Ordering::Acquire)) {
        return Err(CanvasError::new(
            CanvasErrorCode::TransientFailure,
            cancellation_message,
        ));
    }
    stream.finish().map_err(|error| {
        CanvasError::new(
            CanvasErrorCode::ProviderFailure,
            format!("finalize generated video artifact for task `{task_id}`: {error}"),
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

/// Boundary for the hand-written `multipart/form-data` transcription body.
///
/// `reqwest`'s `multipart` feature is not enabled anywhere in this workspace,
/// and the transcription request only ever carries two fields, so the bytes
/// are assembled here rather than pulling in another feature.
const AUDIO_FORM_BOUNDARY: &str = "----deepagentCanvasAudioBoundary";

/// `multipart/form-data` payload for `/audio/transcriptions` (`model` + `file`).
fn transcription_form(
    model_id: &str,
    bytes: &[u8],
    filename: &str,
    mime: &str,
) -> CanvasResult<Vec<u8>> {
    // 模型 id 来自用户录入的供应商配置，任何引号或换行都会改写表单结构。
    if model_id.contains(['"', '\r', '\n']) {
        return Err(CanvasError::new(
            CanvasErrorCode::InvalidInput,
            "model id cannot be sent as form data".to_string(),
        ));
    }
    let mut body = Vec::with_capacity(bytes.len() + 256);
    body.extend_from_slice(
        format!(
            "--{AUDIO_FORM_BOUNDARY}\r\nContent-Disposition: form-data; name=\"model\"\r\n\r\n{model_id}\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(
        format!(
            "--{AUDIO_FORM_BOUNDARY}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\nContent-Type: {mime}\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(bytes);
    body.extend_from_slice(format!("\r\n--{AUDIO_FORM_BOUNDARY}--\r\n").as_bytes());
    Ok(body)
}

/// Detect a video container from its magic bytes.
#[cfg(test)]
fn sniff_video_mime(bytes: &[u8]) -> Option<String> {
    let mime = match bytes {
        // ISO-BMFF: 4-byte box size, then the `ftyp` fourcc at offset 4.
        [_, _, _, _, b'f', b't', b'y', b'p', b'q', b't', b' ', b' ', ..] => "video/quicktime",
        [_, _, _, _, b'f', b't', b'y', b'p', ..] => "video/mp4",
        [0x1a, 0x45, 0xdf, 0xa3, ..] => "video/webm",
        _ => return None,
    };
    Some(mime.to_string())
}

/// Sniff the audio container from magic bytes; declared types lie.
fn sniff_audio_mime(bytes: &[u8]) -> Option<String> {
    let mime = match bytes {
        [b'I', b'D', b'3', ..] => "audio/mpeg",
        [0xff, 0xf2 | 0xf3 | 0xfb, ..] => "audio/mpeg",
        [b'R', b'I', b'F', b'F', _, _, _, _, b'W', b'A', b'V', b'E', ..] => "audio/wav",
        [b'O', b'g', b'g', b'S', ..] => "audio/ogg",
        [b'f', b'L', b'a', b'C', ..] => "audio/flac",
        [_, _, _, _, b'f', b't', b'y', b'p', ..] => "audio/mp4",
        _ => return None,
    };
    Some(mime.to_string())
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

/// Split a `providerId::modelId` reference for compatibility callers.
///
/// This lossy helper is retained for callers that only need to inspect a
/// valid pair. Gateway execution must use [`parse_model_ref`] instead, since a
/// non-empty malformed reference is not equivalent to automatic routing.
pub fn decode_model_ref(model_ref: &str) -> Option<(&str, &str)> {
    parse_model_ref(model_ref).ok().flatten()
}

/// Parse the canvas model route without conflating automatic routing and an
/// invalid explicit identity. Empty input is the only automatic route; every
/// non-empty value must contain exactly one non-empty `providerId::modelId`
/// pair. Provider/model existence is checked later against the same provider
/// snapshot by `execute_on`.
fn parse_model_ref(model_ref: &str) -> CoreResult<Option<(&str, &str)>> {
    let trimmed = model_ref.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    let mut parts = trimmed.split(MODEL_REF_SEPARATOR);
    let provider_id = parts.next().unwrap_or_default().trim();
    let model_id = parts.next().unwrap_or_default().trim();
    let valid = !provider_id.is_empty() && !model_id.is_empty() && parts.next().is_none();
    if !valid {
        return Err(deepagent_core::error::CoreError::invalid(format!(
            "InvalidModelReference: non-empty canvas model reference `{trimmed}` must use providerId::modelId"
        )));
    }
    Ok(Some((provider_id, model_id)))
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
    let policy = if node_kind.trim().is_empty() {
        CanvasPromptPolicy::Professional
    } else {
        CanvasPromptPolicy::CreativeProfile
    };
    assemble_system_prompt_with_policy(node_kind, policy, node_system_prompt, skill_ids)
}

fn assemble_system_prompt_with_policy(
    node_kind: &str,
    policy: CanvasPromptPolicy,
    node_system_prompt: Option<&str>,
    skill_ids: &[String],
) -> CoreResult<Option<String>> {
    if matches!(policy, CanvasPromptPolicy::Professional) {
        if !skill_ids.is_empty() {
            return Err(deepagent_core::error::CoreError::invalid(
                "UnsupportedOperation: professional model nodes cannot attach creative skills",
            ));
        }
        return Ok(node_system_prompt
            .map(str::to_string)
            .filter(|value| !value.trim().is_empty()));
    }
    if node_kind.trim().is_empty() {
        return Err(deepagent_core::error::CoreError::invalid(
            "InvalidConfiguration: creative model nodes require a node kind",
        ));
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

impl CanvasModelGateway {
    async fn complete_observed(
        &self,
        request: CanvasCompletionRequest,
        usage_sink: Option<Arc<dyn CanvasUsageSink>>,
    ) -> CoreResult<CanvasCompletionResponse> {
        // Keep the pre-policy wire compatibility: an omitted policy plus an
        // empty node kind is the legacy professional request shape. New
        // professional nodes set the policy explicitly in the runtime seam.
        let prompt_policy = if request.node_kind.trim().is_empty()
            && matches!(request.prompt_policy, CanvasPromptPolicy::CreativeProfile)
        {
            CanvasPromptPolicy::Professional
        } else {
            request.prompt_policy
        };
        let system_prompt = assemble_system_prompt_with_policy(
            &request.node_kind,
            prompt_policy,
            request.system_prompt.as_deref(),
            &request.skill_ids,
        )?;
        let model_request = CanvasModelRequest {
            audio: Default::default(),
            operation: CanvasOperation::TextGenerate,
            prompt: request.prompt.clone(),
            system_prompt,
            temperature: request.temperature,
            max_tokens: request.max_tokens,
            images: self.resolve_image_inputs(&request.images)?,
            texts: Vec::new(),
            size: None,
            timeout_ms: 120_000,
        };
        validate_request(&model_request).map_err(to_core_error)?;
        let output = match parse_model_ref(&request.model_ref)? {
            Some((provider_id, model_id)) => self
                .execute_on_observed(provider_id, model_id, &model_request, usage_sink)
                .await
                .map_err(to_core_error)?,
            None => self
                .execute_observed(&model_request, self.workspace_id.as_deref(), usage_sink)
                .await
                .map_err(to_core_error)?,
        };
        match output {
            CanvasModelOutput::Text {
                text,
                reasoning,
                provider_id,
                model_id,
                usage,
                raw_usage,
            } => Ok(CanvasCompletionResponse {
                text,
                reasoning,
                provider_id,
                model_id,
                usage,
                raw_usage,
            }),
            other => Err(deepagent_core::error::CoreError::other(format!(
                "text node received a non-text model output: {other:?}"
            ))),
        }
    }
    async fn run_video_job(&self, request: CanvasVideoRequest) -> CoreResult<VideoDownloadPlan> {
        use std::time::Instant;

        if request.prompt.trim().is_empty() && request.image_url.is_none() {
            return Err(deepagent_core::error::CoreError::invalid(
                "MissingReferenceInput: video node needs a prompt or a first frame",
            ));
        }
        let candidate = match parse_model_ref(&request.model_ref)? {
            Some((provider_id, model_id)) => self.providers.resolve_model(provider_id, model_id)?,
            None => self
                .providers
                .candidates_for_scenario(CanvasScenario::VideoGeneration, None)?
                .into_iter()
                .next()
                .ok_or_else(|| {
                    deepagent_core::error::CoreError::invalid(
                        "NoCandidateModel: no enabled model serves video generation",
                    )
                })?,
        };
        let images = match request
            .image_url
            .as_deref()
            .filter(|value| !value.trim().is_empty())
        {
            Some(reference) => self.resolve_image_inputs(&[reference.to_string()])?,
            None => Vec::new(),
        };
        let input_fingerprint = video_input_fingerprint(&candidate, &request, images.first());
        let model_request = CanvasModelRequest {
            operation: CanvasOperation::VideoGenerate,
            prompt: request.prompt.clone(),
            system_prompt: None,
            temperature: None,
            max_tokens: None,
            images,
            texts: Vec::new(),
            size: request.size.clone(),
            audio: CanvasAudioInput::default(),
            timeout_ms: 120_000,
        };
        // Provider task ids are scoped to the account that created them. Resume
        // only when the saved fingerprint still matches the effective inputs
        // and current provider credentials/configuration.
        let saved_task_id = request
            .resume_task_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let resumable_task_id = saved_task_id.filter(|task_id| {
            let current = video_resume_fingerprint(&input_fingerprint, task_id);
            request.resume_fingerprint.as_deref() == Some(current.as_str())
        });
        if saved_task_id.is_some() && resumable_task_id.is_none() {
            tracing::info!(
                provider_id = %candidate.provider.id,
                model_id = %candidate.model.id,
                has_saved_fingerprint = request.resume_fingerprint.is_some(),
                "saved video task does not match current inputs/provider; submitting a new task"
            );
        }
        let task_id = match resumable_task_id {
            Some(existing) => existing.trim().to_string(),
            None => self
                .submit_media_job(&candidate, &model_request, request.cancel.as_ref())
                .await
                .map_err(to_core_error)?,
        };
        let resume_fingerprint = video_resume_fingerprint(&input_fingerprint, &task_id);
        if let Some(sink) = request.on_progress.as_ref() {
            // Publish identity before the first potentially long status wait.
            sink.emit(CanvasJobProgress {
                job_id: task_id.clone(),
                phase: CanvasJobPhase::Queued,
                resume_fingerprint: resume_fingerprint.clone(),
            });
        }
        let timeout_ms = request.timeout_ms.max(1_000);
        let deadline = Instant::now() + Duration::from_millis(timeout_ms);
        let mut round = 0u32;
        let mut reported: Option<CanvasJobPhase> = Some(CanvasJobPhase::Queued);
        let job = loop {
            if request
                .cancel
                .as_ref()
                .is_some_and(|flag| flag.load(Ordering::SeqCst))
            {
                return Err(deepagent_core::error::CoreError::other(format!(
                    "Cancelled: video job `{task_id}` interrupted; rerun the node with the same task id to keep polling"
                )));
            }
            let job = self
                .poll_media_job(&candidate, &task_id, 60_000, request.cancel.as_ref())
                .await
                .map_err(to_core_error)?;
            if matches!(
                job.status,
                CanvasJobStatus::Succeeded | CanvasJobStatus::Failed
            ) {
                break job;
            }
            let phase = if job.status == CanvasJobStatus::Queued {
                CanvasJobPhase::Queued
            } else {
                CanvasJobPhase::Running
            };
            if reported != Some(phase) {
                if let Some(sink) = request.on_progress.as_ref() {
                    sink.emit(CanvasJobProgress {
                        job_id: task_id.clone(),
                        phase,
                        resume_fingerprint: resume_fingerprint.clone(),
                    });
                }
                reported = Some(phase);
            }
            if Instant::now() >= deadline {
                return Err(deepagent_core::error::CoreError::other(format!(
                    "TransientFailure: video job `{task_id}` did not finish within {timeout_ms} ms"
                )));
            }
            await_video_operation(
                async {
                    tokio::time::sleep(poll_delay(round)).await;
                    Ok(())
                },
                request.cancel.as_ref(),
                &format!(
                    "Cancelled: video job `{task_id}` polling wait interrupted; rerun with the same task id to keep polling"
                ),
            )
            .await
            .map_err(to_core_error)?;
            round += 1;
        };
        if job.status == CanvasJobStatus::Failed {
            return Err(deepagent_core::error::CoreError::other(format!(
                "ProviderFailure: video job `{task_id}` failed: {}",
                job.reason
                    .unwrap_or_else(|| "no reason reported".to_string())
            )));
        }
        let url = job.video_url.ok_or_else(|| {
            deepagent_core::error::CoreError::other(format!(
                "ProviderFailure: video job `{task_id}` succeeded without a video url"
            ))
        })?;
        Ok(VideoDownloadPlan {
            url,
            provider_id: candidate.provider.id.clone(),
            model_id: candidate.model.id.clone(),
            task_id,
            resume_fingerprint,
        })
    }
}

#[async_trait]
impl CanvasModelBridge for CanvasModelGateway {
    async fn complete(
        &self,
        request: CanvasCompletionRequest,
    ) -> CoreResult<CanvasCompletionResponse> {
        self.complete_observed(request, None).await
    }

    async fn complete_with_usage(
        &self,
        request: CanvasCompletionRequest,
        usage: Arc<dyn CanvasUsageSink>,
    ) -> CoreResult<CanvasCompletionResponse> {
        self.complete_observed(request, Some(usage)).await
    }

    async fn prepare_image(
        &self,
        request: CanvasImageRequest,
    ) -> CoreResult<CanvasImagePreparation> {
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
            audio: Default::default(),
            operation,
            prompt: request.prompt.clone(),
            system_prompt: None,
            temperature: None,
            max_tokens: None,
            images: self.resolve_image_inputs(&request.reference_images)?,
            texts: Vec::new(),
            size: request.size.clone(),
            timeout_ms: 300_000,
        };
        validate_request(&model_request).map_err(to_core_error)?;
        let output = match parse_model_ref(&request.model_ref)? {
            Some((provider_id, model_id)) => self
                .execute_on(provider_id, model_id, &model_request)
                .await
                .map_err(to_core_error)?,
            None => self
                .execute(&model_request, self.workspace_id.as_deref())
                .await
                .map_err(to_core_error)?,
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
                        self.workspace_id.as_deref(),
                    )?;
                Ok(CanvasImagePreparation::Ready(CanvasImageResponse {
                    artifact_uri: stored.uri,
                    mime,
                    provider_id,
                    model_id: model_id.clone(),
                    operation: if operation == CanvasOperation::ImageEdit {
                        "edit".to_string()
                    } else {
                        "generate".to_string()
                    },
                }))
            }
            CanvasModelOutput::ImageUrl {
                url,
                provider_id,
                model_id,
            } => self.stage_image_url(
                url,
                provider_id,
                model_id,
                if operation == CanvasOperation::ImageEdit {
                    "edit".to_string()
                } else {
                    "generate".to_string()
                },
            ),
            other => Err(deepagent_core::error::CoreError::other(format!(
                "image node received a non-image model output: {other:?}"
            ))),
        }
    }

    async fn generate_image(&self, request: CanvasImageRequest) -> CoreResult<CanvasImageResponse> {
        match CanvasModelBridge::prepare_image(self, request).await? {
            CanvasImagePreparation::Ready(response) => Ok(response),
            CanvasImagePreparation::Deferred { request_id, origin } => {
                self.pending_artifacts
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .remove(&request_id);
                Err(deepagent_core::error::CoreError::invalid(format!(
                    "provider image URL at {origin} requires the approved workflow tool pipeline"
                )))
            }
        }
    }

    fn prepare_audio_input(&self, reference: &str) -> CoreResult<CanvasAudioInputPreparation> {
        let reference = reference.trim();
        if crate::canvas_artifact_service::artifact_id_from_uri(reference).is_some()
            || reference.starts_with("data:")
        {
            return Ok(CanvasAudioInputPreparation::Ready(reference.to_string()));
        }
        let (request_id, origin) =
            self.stage_artifact_url(reference.to_string(), PendingArtifactOutput::AudioInput)?;
        Ok(CanvasAudioInputPreparation::Deferred { request_id, origin })
    }

    async fn run_audio(&self, request: CanvasAudioRequest) -> CoreResult<CanvasAudioResponse> {
        let operation = match request.operation.trim() {
            "speech_synthesize" => CanvasOperation::SpeechSynthesize,
            "speech_transcribe" => CanvasOperation::SpeechTranscribe,
            other => {
                return Err(deepagent_core::error::CoreError::invalid(format!(
                    "UnsupportedOperation: audio node cannot run `{other}`"
                )))
            }
        };
        let model_request = CanvasModelRequest {
            operation,
            prompt: request.text.clone(),
            system_prompt: None,
            temperature: None,
            max_tokens: None,
            images: Vec::new(),
            texts: Vec::new(),
            size: None,
            audio: CanvasAudioInput {
                reference: request.audio_reference.clone(),
                voice: request.voice.clone(),
                format: request.format.clone(),
            },
            timeout_ms: if request.timeout_ms == 0 {
                300_000
            } else {
                request.timeout_ms
            },
        };
        validate_request(&model_request).map_err(to_core_error)?;
        let output = match parse_model_ref(&request.model_ref)? {
            Some((provider_id, model_id)) => self
                .execute_on(provider_id, model_id, &model_request)
                .await
                .map_err(to_core_error)?,
            None => self
                .execute(&model_request, self.workspace_id.as_deref())
                .await
                .map_err(to_core_error)?,
        };
        match (operation, output) {
            (
                CanvasOperation::SpeechSynthesize,
                CanvasModelOutput::Audio {
                    mime,
                    bytes,
                    provider_id,
                    model_id,
                },
            ) => {
                let stored = self
                    .artifact_store("store a synthesized voice")?
                    .import_bytes(
                        deepagent_persistence::artifact_store::ArtifactKind::Audio,
                        Some(&mime),
                        &bytes,
                        self.workspace_id.as_deref(),
                    )?;
                Ok(CanvasAudioResponse {
                    operation: "speech_synthesize".to_string(),
                    audio_url: Some(stored.uri),
                    text: None,
                    provider_id,
                    model_id,
                })
            }
            (
                CanvasOperation::SpeechTranscribe,
                CanvasModelOutput::Text {
                    text,
                    provider_id,
                    model_id,
                    ..
                },
            ) => Ok(CanvasAudioResponse {
                operation: "speech_transcribe".to_string(),
                audio_url: None,
                text: Some(text),
                provider_id,
                model_id,
            }),
            (_, other) => Err(deepagent_core::error::CoreError::other(format!(
                "audio node received an unexpected model output: {other:?}"
            ))),
        }
    }

    async fn generate_video(&self, request: CanvasVideoRequest) -> CoreResult<CanvasVideoResponse> {
        let plan = self.run_video_job(request.clone()).await?;
        let artifacts = self.artifact_store("store a generated video")?;
        let stored = fetch_video_artifact(
            &plan.url,
            artifacts,
            request.cancel.as_ref(),
            &plan.task_id,
            self.workspace_id.as_deref(),
            self.video_download_limit,
        )
        .await
        .map_err(to_core_error)?;
        let mime = stored
            .media_type
            .clone()
            .unwrap_or_else(|| mime_from_url(&plan.url));
        Ok(CanvasVideoResponse {
            artifact_uri: stored.uri,
            mime,
            provider_id: plan.provider_id,
            model_id: plan.model_id,
            task_id: plan.task_id,
            resume_fingerprint: plan.resume_fingerprint,
        })
    }

    async fn prepare_video(
        &self,
        request: CanvasVideoRequest,
    ) -> CoreResult<CanvasVideoPreparation> {
        let plan = self.run_video_job(request).await?;
        let (request_id, origin) = self.stage_artifact_url(
            plan.url,
            PendingArtifactOutput::Video {
                provider_id: plan.provider_id,
                model_id: plan.model_id,
                task_id: plan.task_id,
                resume_fingerprint: plan.resume_fingerprint,
            },
        )?;
        Ok(CanvasVideoPreparation::Deferred { request_id, origin })
    }
    fn inspect_input_kinds(&self, references: &[String]) -> CoreResult<Vec<String>> {
        references
            .iter()
            .map(|reference| {
                let trimmed = reference.trim();
                if let Some(id) = crate::canvas_artifact_service::artifact_id_from_uri(trimmed) {
                    let kind = self
                        .artifact_store("inspect an input reference")?
                        .record(id, self.workspace_id.as_deref())?
                        .map(|record| match record.kind {
                            deepagent_persistence::artifact_store::ArtifactKind::Image => "Image",
                            deepagent_persistence::artifact_store::ArtifactKind::Video => "Video",
                            deepagent_persistence::artifact_store::ArtifactKind::Audio => "Audio",
                            _ => "Document",
                        })
                        .ok_or_else(|| {
                            deepagent_core::error::CoreError::invalid(
                                "artifact is unavailable in requested workspace",
                            )
                        })?;
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
        self.embed_observed(request, None).await
    }

    async fn embed_with_usage(
        &self,
        request: CanvasEmbeddingRequest,
        usage: Arc<dyn CanvasUsageSink>,
    ) -> CoreResult<CanvasEmbeddingResponse> {
        self.embed_observed(request, Some(usage)).await
    }
}

impl CanvasModelGateway {
    async fn embed_observed(
        &self,
        request: CanvasEmbeddingRequest,
        usage_sink: Option<Arc<dyn CanvasUsageSink>>,
    ) -> CoreResult<CanvasEmbeddingResponse> {
        let model_request = CanvasModelRequest {
            audio: Default::default(),
            operation: CanvasOperation::Embedding,
            prompt: request.texts.first().cloned().unwrap_or_default(),
            system_prompt: None,
            temperature: None,
            max_tokens: None,
            images: Vec::new(),
            texts: request.texts.clone(),
            size: None,
            timeout_ms: 120_000,
        };
        let output = match parse_model_ref(&request.model_ref)? {
            Some((provider_id, model_id)) => self
                .execute_on_observed(provider_id, model_id, &model_request, usage_sink)
                .await
                .map_err(to_core_error)?,
            None => self
                .execute_observed(&model_request, self.workspace_id.as_deref(), usage_sink)
                .await
                .map_err(to_core_error)?,
        };
        match output {
            CanvasModelOutput::Embedding {
                vectors,
                dimensions,
                provider_id,
                model_id,
                usage,
                raw_usage,
                ..
            } => {
                let expected_count = request
                    .texts
                    .iter()
                    .filter(|text| !text.trim().is_empty())
                    .count();
                if vectors.len() != expected_count {
                    return Err(deepagent_core::error::CoreError::other(
                        "embedding provider returned a vector count different from the text input count",
                    ));
                }
                Ok(CanvasEmbeddingResponse {
                    dimensions,
                    count: vectors.len(),
                    provider_id,
                    model_id,
                    vectors,
                    usage,
                    raw_usage,
                })
            }
            other => Err(deepagent_core::error::CoreError::other(format!(
                "embedding node received a non-vector model output: {other:?}"
            ))),
        }
    }
}

impl deepagent_runtime::workflow::CanvasArtifactValidator for CanvasModelGateway {
    fn validate_video_artifact(&self, reference: &str) -> CoreResult<()> {
        self.artifact_store("validate a video-edit artifact")?
            .validate_video_reference(reference, self.workspace_id.as_deref())
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
                .chain(
                    request
                        .audio
                        .reference
                        .iter()
                        .filter(|reference| !reference.trim().is_empty())
                        .map(|_| CanvasInputKind::Audio),
                )
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
        CanvasModelConfig, CanvasProviderInput, CanvasProviderService, CanvasScenarioBinding,
    };
    use deepagent_persistence::Database;
    use deepagent_runtime::workflow::{CanvasArtifactValidator, CanvasJobProgressSink};
    use std::sync::Arc;

    #[tokio::test]
    async fn h252_deferred_image_fetch_preserves_scope_and_isolates_one_shot_handles() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            while !request.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
                let mut buffer = [0; 1024];
                let count = socket.read(&mut buffer).await.unwrap();
                assert!(count > 0);
                request.extend_from_slice(&buffer[..count]);
            }
            assert!(String::from_utf8_lossy(&request).starts_with("GET /image.png "));
            socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: 5\r\nConnection: close\r\n\r\nimage").await.unwrap();
        });
        let root = tempfile::tempdir().unwrap();
        let db = Arc::new(Database::open_in_memory().unwrap());
        let artifacts = Arc::new(
            crate::canvas_artifact_service::CanvasArtifactService::new(root.path(), db.clone())
                .unwrap(),
        );
        let providers = Arc::new(CanvasProviderService::new(
            db,
            Arc::new(crate::MemorySecretStore::new()),
        ));
        let mut base = CanvasModelGateway::new(providers).with_artifacts(artifacts.clone());
        base.public_http = Arc::new(crate::http_runtime::public_fixture_client(address));
        let base = Arc::new(base);
        let owner = base.scoped_for_run(Some("ws-a")).unwrap();
        let foreign = base.scoped_for_run(Some("ws-b")).unwrap();
        let CanvasImagePreparation::Deferred { request_id, origin } = owner
            .stage_image_url(
                format!("http://public.fixture.invalid:{}/image.png", address.port()),
                "synthetic-provider".into(),
                "synthetic-image".into(),
                "generate".into(),
            )
            .unwrap()
        else {
            panic!("expected deferred image");
        };
        assert!(foreign
            .fetch_image_handle(&request_id, &origin, ToolExecutionContext::default())
            .await
            .is_err());
        assert!(owner.pending_image_origin(&request_id).is_some());
        let fetched = tokio::time::timeout(
            Duration::from_secs(5),
            owner.fetch_image_handle(&request_id, &origin, ToolExecutionContext::default()),
        )
        .await;
        if !matches!(&fetched, Ok(Ok(_))) {
            server.abort();
        }
        let joined = server.await;
        let output = fetched.unwrap().unwrap();
        joined.unwrap();
        let id = output.artifact_uri.trim_start_matches("artifact://");
        assert_eq!(
            artifacts
                .record(id, Some("ws-a"))
                .unwrap()
                .unwrap()
                .workspace_id
                .as_deref(),
            Some("ws-a")
        );
        assert_eq!(
            artifacts.read_bytes(id, Some("ws-a")).unwrap().unwrap(),
            b"image"
        );
        assert!(artifacts.record(id, None).unwrap().is_none());
        assert!(artifacts.record(id, Some("ws-b")).unwrap().is_none());
        assert!(owner
            .fetch_image_handle(&request_id, &origin, ToolExecutionContext::default())
            .await
            .is_err());
    }

    #[tokio::test]
    async fn h303_deferred_video_fetch_preserves_scope_kind_and_one_shot() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            while !request.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
                let mut buffer = [0; 1024];
                let count = socket.read(&mut buffer).await.unwrap();
                assert!(count > 0);
                request.extend_from_slice(&buffer[..count]);
            }
            assert!(String::from_utf8_lossy(&request).starts_with("GET /video.mp4 "));
            socket
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: video/mp4\r\nContent-Length: 5\r\nConnection: close\r\n\r\nvideo",
                )
                .await
                .unwrap();
        });
        let root = tempfile::tempdir().unwrap();
        let db = Arc::new(Database::open_in_memory().unwrap());
        let artifacts = Arc::new(
            crate::canvas_artifact_service::CanvasArtifactService::new(root.path(), db.clone())
                .unwrap(),
        );
        let providers = Arc::new(CanvasProviderService::new(
            db,
            Arc::new(crate::MemorySecretStore::new()),
        ));
        let mut base = CanvasModelGateway::new(providers).with_artifacts(artifacts.clone());
        base.public_http = Arc::new(crate::http_runtime::public_fixture_client(address));
        let base = Arc::new(base);
        let owner = base.scoped_for_run(Some("ws-a")).unwrap();
        let foreign = base.scoped_for_run(Some("ws-b")).unwrap();
        let (request_id, origin) = owner
            .stage_artifact_url(
                format!("http://public.fixture.invalid:{}/video.mp4", address.port()),
                PendingArtifactOutput::Video {
                    provider_id: "synthetic-provider".into(),
                    model_id: "synthetic-video".into(),
                    task_id: "synthetic-task".into(),
                    resume_fingerprint: "synthetic-fingerprint".into(),
                },
            )
            .unwrap();

        let no_broker = owner
            .video_fetch_tool(false)
            .invoke(json!({"request_id": request_id.clone(), "origin": origin.clone()}))
            .await;
        assert!(no_broker
            .expect_err("isolated backends must not fall back to the host")
            .to_string()
            .contains("NetworkDenied"));
        assert!(foreign
            .fetch_artifact_handle(
                &request_id,
                &origin,
                ArtifactFetchKind::Video,
                ToolExecutionContext::default(),
            )
            .await
            .is_err());
        assert!(owner
            .fetch_artifact_handle(
                &request_id,
                &origin,
                ArtifactFetchKind::Image,
                ToolExecutionContext::default(),
            )
            .await
            .is_err());
        assert!(owner
            .pending_artifact_origin(&request_id, ArtifactFetchKind::Video)
            .is_some());

        let fetched = tokio::time::timeout(
            Duration::from_secs(5),
            owner.fetch_artifact_handle(
                &request_id,
                &origin,
                ArtifactFetchKind::Video,
                ToolExecutionContext::default(),
            ),
        )
        .await;
        if !matches!(&fetched, Ok(Ok(_))) {
            server.abort();
        }
        let output: CanvasVideoResponse =
            serde_json::from_value(fetched.unwrap().unwrap()).unwrap();
        server.await.unwrap();
        let id = output.artifact_uri.trim_start_matches("artifact://");
        let record = artifacts.record(id, Some("ws-a")).unwrap().unwrap();
        assert_eq!(
            record.kind,
            deepagent_persistence::artifact_store::ArtifactKind::Video
        );
        assert_eq!(
            artifacts.read_bytes(id, Some("ws-a")).unwrap().unwrap(),
            b"video"
        );
        assert!(artifacts.record(id, None).unwrap().is_none());
        assert!(artifacts.record(id, Some("ws-b")).unwrap().is_none());
        assert!(owner
            .fetch_artifact_handle(
                &request_id,
                &origin,
                ArtifactFetchKind::Video,
                ToolExecutionContext::default(),
            )
            .await
            .is_err());
    }

    #[tokio::test]
    async fn h252_run_scoped_gateways_isolate_all_artifact_input_paths() {
        use crate::canvas_artifact_service::CanvasArtifactService;
        use deepagent_persistence::artifact_store::ArtifactKind;
        let root = tempfile::tempdir().unwrap();
        let db = Arc::new(Database::open_in_memory().unwrap());
        let artifacts = Arc::new(CanvasArtifactService::new(root.path(), db.clone()).unwrap());
        let providers = Arc::new(CanvasProviderService::new(
            db,
            Arc::new(crate::MemorySecretStore::new()),
        ));
        let base = Arc::new(CanvasModelGateway::new(providers).with_artifacts(artifacts.clone()));
        for owner in [None, Some("ws-a"), Some("ws-b")] {
            let image = artifacts
                .import_bytes(ArtifactKind::Image, Some("image/png"), b"image", owner)
                .unwrap();
            let audio = artifacts
                .import_bytes(ArtifactKind::Audio, Some("audio/mpeg"), b"ID3audio", owner)
                .unwrap();
            let video = artifacts
                .import_bytes(ArtifactKind::Video, Some("video/mp4"), b"video", owner)
                .unwrap();
            for caller in [None, Some("ws-a"), Some("ws-b")] {
                let gateway = base.scoped_for_run(caller).unwrap();
                let own = caller == owner;
                for item in [&image, &audio, &video] {
                    assert_eq!(
                        gateway
                            .inspect_input_kinds(std::slice::from_ref(&item.uri))
                            .is_ok(),
                        own
                    );
                }
                assert_eq!(
                    gateway
                        .resolve_image_inputs(std::slice::from_ref(&image.uri))
                        .is_ok(),
                    own
                );
                assert_eq!(gateway.validate_video_artifact(&video.uri).is_ok(), own);
                let mut request =
                    CanvasModelRequest::text("").with_operation(CanvasOperation::SpeechTranscribe);
                request.audio.reference = Some(audio.uri.clone());
                let result = gateway
                    .audio_input(&request, gateway.workspace_id.as_deref())
                    .await;
                assert_eq!(result.is_ok(), own);
                if own {
                    assert_eq!(result.unwrap().0, b"ID3audio");
                }
            }
        }
        assert_eq!(
            base.workspace_id, None,
            "creating scoped runs cannot mutate their shared parent"
        );
        assert!(base.scoped_for_run(Some("bad/path")).is_err());
    }

    #[test]
    fn embedding_batch_keeps_all_vectors_in_input_order_and_legacy_scalar() {
        assert_eq!(
            parse_embedding_vectors(&json!({"data": [
                {"index": 1, "embedding": [3.0, 4.0]},
                {"index": 0, "embedding": [1.0, 2.0]},
            ]}))
            .unwrap(),
            vec![vec![1.0, 2.0], vec![3.0, 4.0]]
        );
        assert_eq!(
            parse_embedding_vectors(&json!({"data": [
                {"embedding": [0.5, -0.25, 1.0]},
            ]}))
            .unwrap(),
            vec![vec![0.5, -0.25, 1.0]]
        );
    }

    #[test]
    fn embedding_batch_rejects_invalid_items_without_silent_truncation() {
        for data in [
            json!([]),
            json!([{"embedding": []}]),
            json!([{"embedding": [1.0, "invalid"]}]),
            json!([{"embedding": [1e100]}]),
            json!([{"embedding": [1.0]}, {"embedding": [1.0, 2.0]}]),
            json!([{"index": 0, "embedding": [1.0]}, {"index": 0, "embedding": [2.0]}]),
            json!([{"index": 2, "embedding": [1.0]}]),
            json!([{"index": 0, "embedding": [1.0]}, {"embedding": [2.0]}]),
        ] {
            assert!(parse_embedding_vectors(&json!({"data": data})).is_err());
        }
    }
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

    fn video_candidate() -> ResolvedCanvasModel {
        ResolvedCanvasModel {
            provider: crate::canvas_provider_service::CanvasProviderConfig {
                id: "provider-1".to_string(),
                name: "Video provider".to_string(),
                protocol: CanvasProtocol::OpenAi,
                base_url: "https://video.example/v1".to_string(),
                credential_version: "credential-generation-1".to_string(),
                credential_ref: None,
                enabled: true,
                logo: None,
                models: vec![],
            },
            model: CanvasModelConfig {
                id: "video-model-1".to_string(),
                name: "Video model".to_string(),
                description: String::new(),
                enabled: true,
                scenarios: vec![CanvasScenario::VideoGeneration],
                priority: 0,
                max_reference_images: None,
            },
            api_key: Some("sk-video-test-secret".to_string()),
        }
    }

    #[test]
    fn video_resume_identity_covers_effective_inputs_and_provider_generation() {
        let candidate = video_candidate();
        let request = CanvasVideoRequest {
            model_ref: "provider-1::video-model-1".to_string(),
            prompt: "a red kite over the sea".to_string(),
            image_url: Some("artifact://frame-1".to_string()),
            size: Some("720p".to_string()),
            resume_task_id: None,
            resume_fingerprint: None,
            timeout_ms: 60_000,
            cancel: None,
            on_progress: None,
        };
        let image = CanvasImageInput {
            data_url: "data:image/png;base64,AAAA".to_string(),
        };
        let fingerprint = video_input_fingerprint(&candidate, &request, Some(&image));
        assert_eq!(
            video_input_fingerprint(&candidate, &request, Some(&image)),
            fingerprint,
            "unchanged inputs must produce a stable identity"
        );
        assert!(!fingerprint.contains("sk-video-test-secret"));

        let mut changed_request = request.clone();
        changed_request.prompt.push_str(" with a yellow tail");
        assert_ne!(
            video_input_fingerprint(&candidate, &changed_request, Some(&image)),
            fingerprint,
            "prompt changes must invalidate a saved task"
        );
        changed_request = request.clone();
        changed_request.size = Some("1080p".to_string());
        assert_ne!(
            video_input_fingerprint(&candidate, &changed_request, Some(&image)),
            fingerprint,
            "size changes must invalidate a saved task"
        );

        let changed_image = CanvasImageInput {
            data_url: "data:image/png;base64,BBBB".to_string(),
        };
        assert_ne!(
            video_input_fingerprint(&candidate, &request, Some(&changed_image)),
            fingerprint,
            "reference image changes must invalidate a saved task"
        );

        let mut changed_candidate = candidate.clone();
        changed_candidate.model.id = "video-model-2".to_string();
        assert_ne!(
            video_input_fingerprint(&changed_candidate, &request, Some(&image)),
            fingerprint,
            "model changes must invalidate a saved task"
        );
        changed_candidate = candidate.clone();
        changed_candidate.provider.protocol = CanvasProtocol::Gemini;
        assert_ne!(
            video_input_fingerprint(&changed_candidate, &request, Some(&image)),
            fingerprint,
            "protocol changes must invalidate a saved task"
        );
        changed_candidate = candidate.clone();
        changed_candidate.provider.base_url = "https://other.example/v1".to_string();
        assert_ne!(
            video_input_fingerprint(&changed_candidate, &request, Some(&image)),
            fingerprint,
            "endpoint changes must invalidate a saved task"
        );
        changed_candidate = candidate.clone();
        changed_candidate.provider.credential_version = "credential-generation-2".to_string();
        assert_ne!(
            video_input_fingerprint(&changed_candidate, &request, Some(&image)),
            fingerprint,
            "credential changes must invalidate a saved task"
        );
        changed_candidate.api_key = Some("sk-another-key".to_string());
        assert_ne!(
            video_input_fingerprint(&changed_candidate, &request, Some(&image)),
            fingerprint,
            "the persisted identity must be keyed by the current credential"
        );
        assert_ne!(
            video_resume_fingerprint(&fingerprint, "job-1"),
            video_resume_fingerprint(&fingerprint, "job-2"),
            "task ids must be part of the persisted resume binding"
        );
    }

    #[tokio::test]
    async fn provider_image_handle_is_origin_bound_and_one_shot() {
        let db = Arc::new(Database::open_in_memory().expect("db"));
        let providers = Arc::new(CanvasProviderService::new(
            db,
            Arc::new(crate::secret_store::MemorySecretStore::default()),
        ));
        let gateway = CanvasModelGateway::new(providers);
        let preparation = gateway
            .stage_image_url(
                "http://127.0.0.1:9/private.png".to_string(),
                "provider".to_string(),
                "image-model".to_string(),
                "generate".to_string(),
            )
            .expect("defer image URL");
        let CanvasImagePreparation::Deferred { request_id, origin } = preparation else {
            panic!("provider URL must be deferred");
        };

        let wrong = gateway
            .fetch_image_handle(
                &request_id,
                "http://other.example",
                ToolExecutionContext::default(),
            )
            .await;
        assert!(wrong.is_err());
        assert_eq!(
            gateway.pending_image_origin(&request_id).as_deref(),
            Some(origin.as_str())
        );

        let first = gateway
            .fetch_image_handle(&request_id, &origin, ToolExecutionContext::default())
            .await;
        assert!(first.is_err(), "loopback must be rejected by the broker");
        assert!(gateway.pending_image_origin(&request_id).is_none());

        let second = gateway
            .fetch_image_handle(&request_id, &origin, ToolExecutionContext::default())
            .await;
        assert!(
            second.is_err(),
            "a provider image handle must not be replayable"
        );
    }

    #[test]
    fn hmac_sha256_matches_rfc_4231_test_case() {
        let mac = hmac_sha256(b"key", b"The quick brown fox jumps over the lazy dog");
        let hex = mac
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        assert_eq!(
            hex,
            "f7bc83f430538424b13298e6aa6fb143ef4d59a14946175997479dbc2d1a3cd8"
        );
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
    fn text_plus_image_still_routes_to_video_generation() {
        // 方案第十九节：文本 + 图片 → VideoGenerate（首帧图生视频是合法输入）。
        let resolved = DeterministicRouteResolver::resolve(&facts(
            "video-gen",
            &[CanvasOperation::VideoGenerate],
            None,
            &[CanvasInputKind::Text, CanvasInputKind::Image],
            true,
        ))
        .expect("first frame is a video input");
        assert_eq!(resolved.operation, CanvasOperation::VideoGenerate);
    }

    #[test]
    fn legacy_video_edit_and_extend_operations_fail_as_unsupported_without_rerouting() {
        for operation in [CanvasOperation::VideoEdit, CanvasOperation::VideoExtend] {
            let error = DeterministicRouteResolver::resolve(&facts(
                "video-gen",
                &[CanvasOperation::VideoGenerate],
                Some(operation),
                &[CanvasInputKind::Video],
                true,
            ))
            .expect_err("unsupported legacy operation");
            assert_eq!(error.code, CanvasErrorCode::UnsupportedCapability);
            assert!(error.message.contains(operation.as_str()));
        }
    }

    #[test]
    fn mixed_image_and_video_input_is_a_type_conflict() {
        let err = DeterministicRouteResolver::resolve(&facts(
            "image-edit",
            &[CanvasOperation::ImageEdit],
            Some(CanvasOperation::ImageEdit),
            &[CanvasInputKind::Image, CanvasInputKind::Video],
            true,
        ))
        .expect_err("media types cannot mix");
        assert_eq!(err.code, CanvasErrorCode::OperationInputConflict);
        assert!(
            err.message.contains("cannot mix image and video"),
            "reported {err}"
        );
    }

    #[test]
    fn auto_routing_cannot_bypass_the_media_conflict() {
        // 自动路由挑出 image_edit 后仍要过同一套输入校验。
        let err = DeterministicRouteResolver::resolve(&facts(
            "image-gen",
            &[CanvasOperation::ImageGenerate, CanvasOperation::ImageEdit],
            None,
            &[CanvasInputKind::Image, CanvasInputKind::Video],
            true,
        ))
        .expect_err("auto must not smuggle a mixed input through");
        assert_eq!(err.code, CanvasErrorCode::OperationInputConflict);
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
            audio: Default::default(),
            operation: CanvasOperation::Embedding,
            prompt: "hello".to_string(),
            system_prompt: None,
            temperature: None,
            max_tokens: None,
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
    fn generation_controls_map_to_each_chat_protocol_and_omitted_values_stay_omitted() {
        let mut request = CanvasModelRequest::text("hello");
        let openai_default = build_request_body(
            CanvasProtocol::OpenAi,
            CanvasRequestKind::Chat,
            "deepseek-chat",
            &request,
        )
        .expect("openai default body");
        assert!(openai_default.get("temperature").is_none());
        assert!(openai_default.get("max_tokens").is_none());

        request.temperature = Some(0.2);
        request.max_tokens = Some(17);
        let openai = build_request_body(
            CanvasProtocol::OpenAi,
            CanvasRequestKind::Chat,
            "deepseek-chat",
            &request,
        )
        .expect("openai body");
        assert!((openai["temperature"].as_f64().expect("temperature") - 0.2).abs() < 1e-6);
        assert_eq!(openai["max_tokens"], json!(17));

        let anthropic = build_request_body(
            CanvasProtocol::Anthropic,
            CanvasRequestKind::Chat,
            "claude-sonnet",
            &request,
        )
        .expect("anthropic body");
        assert!((anthropic["temperature"].as_f64().expect("temperature") - 0.2).abs() < 1e-6);
        assert_eq!(anthropic["max_tokens"], json!(17));

        let gemini = build_request_body(
            CanvasProtocol::Gemini,
            CanvasRequestKind::Chat,
            "gemini-2.5-flash",
            &request,
        )
        .expect("gemini body");
        assert!(
            (gemini["generationConfig"]["temperature"]
                .as_f64()
                .expect("temperature")
                - 0.2)
                .abs()
                < 1e-6
        );
        assert_eq!(gemini["generationConfig"]["maxOutputTokens"], json!(17));

        let anthropic_default = build_request_body(
            CanvasProtocol::Anthropic,
            CanvasRequestKind::Chat,
            "claude-sonnet",
            &CanvasModelRequest::text("hello"),
        )
        .expect("anthropic default body");
        assert_eq!(anthropic_default["max_tokens"], json!(4096));
        assert!(anthropic_default.get("temperature").is_none());
    }

    #[test]
    fn unsupported_generation_controls_fail_before_they_can_be_dropped() {
        let mut request = CanvasModelRequest::text("hello");
        request.temperature = Some(0.2);
        let error = build_request_body(
            CanvasProtocol::OpenAi,
            CanvasRequestKind::Embeddings,
            "text-embedding",
            &request,
        )
        .expect_err("embedding adapters must not silently drop temperature");
        assert_eq!(error.code, CanvasErrorCode::UnsupportedCapability);
    }

    #[test]
    fn invalid_generation_controls_are_rejected_before_serialization() {
        for temperature in [-0.1, 2.1, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let mut request = CanvasModelRequest::text("hello");
            request.temperature = Some(temperature);
            let error = build_request_body(
                CanvasProtocol::OpenAi,
                CanvasRequestKind::Chat,
                "deepseek-chat",
                &request,
            )
            .expect_err("invalid temperature");
            assert_eq!(error.code, CanvasErrorCode::InvalidInput);
        }

        for max_tokens in [0, 393_217] {
            let mut request = CanvasModelRequest::text("hello");
            request.max_tokens = Some(max_tokens);
            let error = build_request_body(
                CanvasProtocol::OpenAi,
                CanvasRequestKind::Chat,
                "deepseek-chat",
                &request,
            )
            .expect_err("invalid max_tokens");
            assert_eq!(error.code, CanvasErrorCode::InvalidInput);
        }
    }

    #[test]
    fn image_edit_body_requires_reference_images() {
        let request = CanvasModelRequest {
            audio: Default::default(),
            operation: CanvasOperation::ImageEdit,
            prompt: "change the sky".to_string(),
            system_prompt: None,
            temperature: None,
            max_tokens: None,
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

    /// 方案第十二节：每家协议只声明实际可用的能力，其余必须显式失败，
    /// 而且不能是瞬时错误——否则候选循环会把它当成换模型的时机静默绕行。
    #[test]
    fn each_protocol_declares_only_the_capabilities_it_implements() {
        use crate::canvas_provider_service::endpoint_url;

        let declared = [
            CanvasRequestKind::Chat,
            CanvasRequestKind::Embeddings,
            CanvasRequestKind::ImageGenerate,
            CanvasRequestKind::ImageEdit,
            CanvasRequestKind::SpeechTranscribe,
            CanvasRequestKind::SpeechSynthesize,
            CanvasRequestKind::VideoSubmit,
            CanvasRequestKind::VideoStatus,
            CanvasRequestKind::Models,
        ];
        let supported = |protocol: CanvasProtocol, kind: CanvasRequestKind| match protocol {
            CanvasProtocol::OpenAi => true,
            // Messages only; vision rides the same messages call.
            CanvasProtocol::Anthropic => kind == CanvasRequestKind::Chat,
            CanvasProtocol::Gemini => matches!(
                kind,
                CanvasRequestKind::Chat
                    | CanvasRequestKind::Embeddings
                    | CanvasRequestKind::ImageGenerate
                    | CanvasRequestKind::Models
            ),
        };

        for protocol in [
            CanvasProtocol::OpenAi,
            CanvasProtocol::Anthropic,
            CanvasProtocol::Gemini,
        ] {
            for kind in declared {
                let outcome = endpoint_url(
                    protocol,
                    "https://example.test/v1",
                    kind,
                    Some("Qwen/Qwen3-VL-Embedding-8B"),
                );
                let label = format!("{protocol:?}/{kind:?}");
                if supported(protocol, kind) {
                    let url =
                        outcome.unwrap_or_else(|error| panic!("{label} must map a url: {error}"));
                    assert!(
                        !url.contains("Qwen%2FQwen3") || matches!(protocol, CanvasProtocol::Gemini),
                        "{label} leaked a model id into the route: {url}"
                    );
                } else {
                    let error =
                        outcome.expect_err(&format!("{label} must be UnsupportedCapability")[..]);
                    assert!(
                        error.to_string().contains("UnsupportedCapability"),
                        "{label} reported a vague error: {error}"
                    );
                }
            }
        }
    }

    #[test]
    fn capability_errors_never_advance_the_candidate_list() {
        let request = CanvasModelRequest::text("hi");
        for kind in [
            CanvasRequestKind::Embeddings,
            CanvasRequestKind::ImageGenerate,
            CanvasRequestKind::SpeechSynthesize,
            CanvasRequestKind::VideoSubmit,
        ] {
            let err =
                build_request_body(CanvasProtocol::Anthropic, kind, "deepseek-flash", &request)
                    .expect_err("anthropic implements none of these");
            assert_eq!(err.code, CanvasErrorCode::UnsupportedCapability, "{kind:?}");
            assert!(!err.code.is_transient(), "{kind:?} must not reroute");
        }
    }

    #[test]
    fn model_catalog_endpoints_and_shapes_match_the_measured_gateways() {
        use crate::canvas_provider_service::endpoint_url;

        assert_eq!(
            endpoint_url(
                CanvasProtocol::Gemini,
                "http://127.0.0.1:8045",
                CanvasRequestKind::Models,
                None
            )
            .expect("gemini serves a catalog"),
            "http://127.0.0.1:8045/v1beta/models"
        );
        // 实测：这台 Anthropic 代理的 /v1/models 返回 404，所以只能显式报不支持。
        let anthropic = endpoint_url(
            CanvasProtocol::Anthropic,
            "https://api.deepseek.com/anthropic",
            CanvasRequestKind::Models,
            None,
        );
        assert!(
            anthropic.is_err(),
            "anthropic must not pretend to list models"
        );

        let openai =
            json!({ "data": [{ "id": "gpt-image-2" }, { "id": " Qwen/Qwen3-VL-Embedding-8B " }] });
        assert_eq!(
            model_ids_from_catalog(&openai),
            vec!["gpt-image-2", "Qwen/Qwen3-VL-Embedding-8B"],
            "a slash inside a model id stays inside the id"
        );
        let gemini =
            json!({ "models": [{ "name": "models/claude" }, { "name": "models/claude" }] });
        assert_eq!(model_ids_from_catalog(&gemini), vec!["claude"]);
        assert_eq!(
            model_ids_from_catalog(&json!([])),
            Vec::<String>::new(),
            "a bare array is still a catalog"
        );
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
                credential_version: String::new(),
                credential_ref: None,
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
                max_reference_images: None,
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
                credential_version: String::new(),
                credential_ref: None,
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
                max_reference_images: None,
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
    fn declared_reference_limit_governs_and_undeclared_does_not_guess() {
        let model = |limit: Option<u32>| CanvasModelConfig {
            id: "img-model".to_string(),
            name: "img".to_string(),
            description: String::new(),
            enabled: true,
            scenarios: vec![CanvasScenario::ImageGeneration],
            priority: 0,
            max_reference_images: limit,
        };
        assert!(reference_limit_error(&model(None), 40).is_ok());
        assert!(reference_limit_error(&model(Some(2)), 2).is_ok());
        let err = reference_limit_error(&model(Some(1)), 2).expect_err("over the ceiling");
        assert_eq!(err.code, CanvasErrorCode::TooManyReferences);
        assert!(err.message.contains("at most 1"), "reported {err}");
    }

    #[test]
    fn scenario_candidates_without_room_for_the_references_fail_closed() {
        // 候选全都不够装时不许偷偷截断输入，也不许发请求。
        let db = Arc::new(Database::open_in_memory().expect("db"));
        let providers = Arc::new(CanvasProviderService::new(
            db,
            Arc::new(crate::secret_store::MemorySecretStore::default()),
        ));
        providers
            .save_provider(CanvasProviderInput {
                name: "Limited".to_string(),
                protocol: "openai".to_string(),
                base_url: "http://127.0.0.1:1/v1".to_string(),
                enabled: true,
                models: vec![CanvasModelConfig {
                    id: "one-image-only".to_string(),
                    name: "one".to_string(),
                    description: String::new(),
                    enabled: true,
                    scenarios: vec![CanvasScenario::ImageGeneration],
                    priority: 0,
                    max_reference_images: Some(1),
                }],
                api_key: Some("sk-local".to_string()),
                ..Default::default()
            })
            .expect("provider");
        let gateway = CanvasModelGateway::new(providers);
        let request = CanvasModelRequest {
            operation: CanvasOperation::ImageEdit,
            prompt: "把两张图合成".to_string(),
            system_prompt: None,
            temperature: None,
            max_tokens: None,
            images: vec![
                CanvasImageInput {
                    data_url: "artifact://a".to_string(),
                },
                CanvasImageInput {
                    data_url: "artifact://b".to_string(),
                },
            ],
            texts: Vec::new(),
            size: None,
            audio: Default::default(),
            timeout_ms: 1_000,
        };
        let err = tokio::runtime::Runtime::new()
            .expect("runtime")
            .block_on(gateway.execute(&request, None))
            .expect_err("no candidate declares room");
        assert_eq!(err.code, CanvasErrorCode::TooManyReferences);
    }

    #[test]
    fn status_codes_split_transient_and_auth_failures() {
        assert!(classify_status(429, "rate limited").is_transient());
        assert!(classify_status(503, "unavailable").is_transient());
        assert!(!classify_status(401, "bad key").is_transient());
        assert!(!classify_status(400, "bad args").is_transient());
    }

    #[tokio::test]
    async fn provider_error_body_budget_handles_chunked_and_small_responses() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        async fn serve(response: &'static [u8], chunked: bool) -> reqwest::Response {
            let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
            let address = listener.local_addr().expect("address");
            let server = tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await.expect("accept");
                let mut request = Vec::new();
                let mut buffer = [0u8; 512];
                while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                    let count = socket.read(&mut buffer).await.expect("request");
                    assert_ne!(count, 0, "request ended before headers");
                    request.extend_from_slice(&buffer[..count]);
                }
                if chunked {
                    socket
                        .write_all(
                            b"HTTP/1.1 500 Internal Server Error\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n",
                        )
                        .await
                        .expect("headers");
                    for chunk in response.chunks(3) {
                        socket
                            .write_all(format!("{:x}\r\n", chunk.len()).as_bytes())
                            .await
                            .expect("chunk header");
                        socket.write_all(chunk).await.expect("chunk");
                        socket.write_all(b"\r\n").await.expect("chunk end");
                    }
                    socket.write_all(b"0\r\n\r\n").await.expect("trailers");
                } else {
                    socket
                        .write_all(
                            format!(
                                "HTTP/1.1 500 Internal Server Error\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                                response.len()
                            )
                            .as_bytes(),
                        )
                        .await
                        .expect("headers");
                    socket.write_all(response).await.expect("body");
                }
            });
            let response = reqwest::Client::new()
                .get(format!("http://{address}/error"))
                .send()
                .await
                .expect("response");
            server.await.expect("server task");
            response
        }

        let small = serve(b"bad args", false).await;
        let body = read_bounded_provider_body(
            small,
            MAX_PROVIDER_ERROR_BODY_BYTES,
            VideoCancellation {
                flag: None,
                message: "cancelled",
            },
            "provider error response",
        )
        .await
        .expect("small error body");
        assert_eq!(body, b"bad args");

        let chunked = serve(b"abcdef", true).await;
        let error = read_bounded_provider_body(
            chunked,
            5,
            VideoCancellation {
                flag: None,
                message: "cancelled",
            },
            "provider error response",
        )
        .await
        .expect_err("chunked body must stop at the first over-budget chunk");
        assert_eq!(error.code, CanvasErrorCode::ResponseBodyTooLarge);
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
            audio: Default::default(),
            operation: CanvasOperation::ImageGenerate,
            prompt: "a cube".to_string(),
            system_prompt: None,
            temperature: None,
            max_tokens: None,
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
    fn model_ref_parser_only_allows_empty_auto_route_or_one_explicit_pair() {
        assert!(parse_model_ref("").unwrap().is_none());
        assert!(parse_model_ref("  \t").unwrap().is_none());
        assert_eq!(
            parse_model_ref(" provider-1 :: model-1 ")
                .expect("valid explicit identity")
                .unwrap(),
            ("provider-1", "model-1")
        );
        for invalid in [
            "deepseek-chat",
            "provider-1::",
            "::model-1",
            "provider-1::::model-1",
            "provider-1::model-1::fallback",
        ] {
            let error = parse_model_ref(invalid).expect_err("invalid identity must not auto-route");
            assert!(error.to_string().contains("InvalidModelReference"));
        }
        assert_eq!(decode_model_ref("deepseek-chat"), None);
    }

    #[test]
    fn audit_invalid_explicit_model_ref_is_rejected_before_provider_transport() {
        let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
        let (base_url, captured_body) = spawn_single_chat_responder("routed");
        let (gateway, _) = local_text_gateway(base_url, "other-text-model");

        let result = rt.block_on(gateway.complete(CanvasCompletionRequest {
            model_ref: "deepseek-chat".to_string(),
            node_kind: String::new(),
            prompt_policy: CanvasPromptPolicy::Professional,
            skill_ids: Vec::new(),
            system_prompt: None,
            prompt: "audit".to_string(),
            temperature: None,
            max_tokens: None,
            images: Vec::new(),
        }));
        let wire_model = captured_body
            .lock()
            .expect("lock")
            .as_ref()
            .and_then(|body| body["model"].as_str().map(str::to_string));
        eprintln!(
            "canvas model identity audit: requested=deepseek-chat decoded={:?} wire_model={wire_model:?} result={result:?}",
            decode_model_ref("deepseek-chat")
        );

        assert!(
            result.is_err(),
            "a non-empty invalid model reference must be rejected, not routed automatically"
        );
        assert_eq!(
            wire_model, None,
            "invalid explicit model identity must not reach provider transport"
        );
    }

    #[test]
    fn audit_canvas_completion_honors_node_temperature_and_output_limit() {
        let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
        let (base_url, captured_body) = spawn_single_chat_responder("configured");
        let (gateway, provider_id) = local_text_gateway(base_url, "deepseek-flash");

        let response = rt
            .block_on(gateway.complete(CanvasCompletionRequest {
                model_ref: format!("{provider_id}::deepseek-flash"),
                node_kind: String::new(),
                prompt_policy: CanvasPromptPolicy::Professional,
                skill_ids: Vec::new(),
                system_prompt: None,
                prompt: "audit controls".to_string(),
                temperature: Some(0.2),
                max_tokens: Some(17),
                images: Vec::new(),
            }))
            .expect("the controlled provider returns a valid completion");
        let body = captured_body
            .lock()
            .expect("lock")
            .clone()
            .expect("provider request body");
        eprintln!(
            "canvas generation controls audit: response_model={} body={body}",
            response.model_id
        );

        let temperature = body
            .get("temperature")
            .and_then(Value::as_f64)
            .expect("temperature");
        assert!(
            (temperature - 0.2).abs() < 1e-6,
            "the configured node temperature must reach the provider: {temperature}"
        );
        assert_eq!(
            body.get("max_tokens").and_then(Value::as_u64),
            Some(17),
            "the configured node output limit must reach the provider"
        );
    }

    #[test]
    fn invalid_canvas_generation_controls_never_reach_provider_transport() {
        let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
        let (base_url, captured_body) = spawn_single_chat_responder("must not be sent");
        let (gateway, provider_id) = local_text_gateway(base_url, "deepseek-flash");

        let result = rt.block_on(gateway.complete(CanvasCompletionRequest {
            model_ref: format!("{provider_id}::deepseek-flash"),
            node_kind: String::new(),
            prompt_policy: CanvasPromptPolicy::Professional,
            skill_ids: Vec::new(),
            system_prompt: None,
            prompt: "invalid controls".to_string(),
            temperature: Some(f32::NAN),
            max_tokens: Some(17),
            images: Vec::new(),
        }));

        assert!(result.is_err(), "invalid controls must fail closed");
        assert_eq!(
            captured_body.lock().expect("lock").as_ref(),
            None,
            "validation must happen before provider transport"
        );
    }

    #[tokio::test]
    async fn audit_professional_ai_nodes_reach_provider_or_fail_closed() {
        use deepagent_runtime::agent::{Agent, AgentDecision};

        let cases = [
            (
                "llm",
                vec![
                    ("llmModel", Value::String("MODEL_REF".to_string())),
                    ("llmPrompt", Value::String("classify this".to_string())),
                ],
                "answer",
                true,
            ),
            (
                "agent-v2",
                vec![
                    ("agentV2Model", Value::String("MODEL_REF".to_string())),
                    ("agentV2Task", Value::String("complete this".to_string())),
                ],
                "answer",
                false,
            ),
            (
                "question-classifier",
                vec![
                    ("classifierModel", Value::String("MODEL_REF".to_string())),
                    (
                        "classifierInput",
                        Value::String("classify this".to_string()),
                    ),
                    (
                        "classifierClasses",
                        json!([{ "name": "one", "description": "first" }]),
                    ),
                ],
                "1",
                true,
            ),
            (
                "parameter-extractor",
                vec![
                    ("extractorModel", Value::String("MODEL_REF".to_string())),
                    ("extractorInput", Value::String("name=Eighteen".to_string())),
                    (
                        "extractorParams",
                        json!([{ "name": "name", "type": "string", "required": true }]),
                    ),
                ],
                r#"{"name":"Eighteen"}"#,
                true,
            ),
        ];
        let mut outcomes = Vec::new();
        for (kind, entries, response_text, enabled) in cases {
            let (base_url, captured_body) = spawn_single_chat_responder(response_text);
            let (gateway, provider_id) = local_text_gateway(base_url, "deepseek-flash");
            let model_ref = format!("{provider_id}::deepseek-flash");
            let mut config = serde_json::Map::new();
            for (key, value) in entries {
                config.insert(
                    key.to_string(),
                    if value.as_str() == Some("MODEL_REF") {
                        Value::String(model_ref.clone())
                    } else {
                        value
                    },
                );
            }
            let mut agent = workflow_agent_with_canvas_node(kind, config, gateway);
            assert!(matches!(
                agent.think(0, &[]).await.expect("start node"),
                AgentDecision::Continue
            ));
            let result = agent.think(1, &[]).await;
            let provider_called = captured_body.lock().expect("lock").is_some();
            outcomes.push((kind, result.map(|_| ()), provider_called, enabled));
        }
        eprintln!("professional AI node audit: {outcomes:?}");

        for (kind, result, provider_called, enabled) in outcomes {
            if enabled {
                assert!(result.is_ok(), "enabled node {kind} failed: {result:?}");
                assert!(
                    provider_called,
                    "enabled node {kind} completed without calling its configured provider"
                );
            } else {
                assert!(result.is_err(), "reserved node {kind} must fail closed");
                assert!(
                    !provider_called,
                    "reserved node {kind} reached its provider"
                );
                assert!(
                    result
                        .expect_err("reserved node should return an error")
                        .to_string()
                        .contains("reserved"),
                    "reserved node {kind} should explain its capability state"
                );
            }
        }
    }

    #[test]
    fn audit_canvas_completion_preserves_provider_usage_and_cache_details() {
        let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
        let provider_payload = deepseek_usage_completion_payload();
        let (base_url, _) = spawn_single_chat_json_responder(provider_payload.clone());
        let (gateway, provider_id) = local_text_gateway(base_url, "deepseek-flash");

        let response = rt
            .block_on(gateway.complete(CanvasCompletionRequest {
                model_ref: format!("{provider_id}::deepseek-flash"),
                node_kind: String::new(),
                prompt_policy: CanvasPromptPolicy::Professional,
                skill_ids: Vec::new(),
                system_prompt: None,
                prompt: "audit usage".to_string(),
                temperature: None,
                max_tokens: None,
                images: Vec::new(),
            }))
            .expect("the controlled provider returns a valid completion");
        let projected = serde_json::to_value(&response).expect("serializable bridge response");
        eprintln!(
            "canvas usage audit: provider_usage={} bridge_response={projected}",
            provider_payload["usage"]
        );

        assert_eq!(response.text, "answer");
        assert_eq!(response.reasoning.as_deref(), Some("reasoning"));
        assert_eq!(
            projected
                .pointer("/usage/prompt_tokens")
                .and_then(Value::as_u64),
            Some(100),
            "provider prompt usage must cross the canvas bridge"
        );
        assert_eq!(
            projected
                .pointer("/usage/prompt_cache_hit_tokens")
                .and_then(Value::as_u64),
            Some(80),
            "DeepSeek cache-hit usage must remain observable"
        );
        assert_eq!(
            projected
                .pointer("/usage/prompt_cache_miss_tokens")
                .and_then(Value::as_u64),
            Some(20),
            "DeepSeek cache-miss usage must remain observable"
        );
        assert_eq!(
            projected
                .pointer("/usage/reasoning_tokens")
                .and_then(Value::as_u64),
            Some(12),
            "DeepSeek reasoning-token usage must remain observable"
        );
    }

    #[tokio::test]
    async fn audit_canvas_provider_usage_reaches_workflow_run_accounting() {
        use std::collections::{HashMap, HashSet};
        use std::sync::Mutex;

        use deepagent_core::clock::SystemClock;
        use deepagent_runtime::agent::{Agent, AgentDecision};
        use deepagent_runtime::NullEventSink;
        use deepagent_session::Session;

        use crate::run_finalizer::{AppRunFinalizer, AppRunFinalizerRequest};
        use crate::session_memory::SessionMemoryPasses;
        use crate::tool_manifest::DiscoveredToolSet;

        let (base_url, _) = spawn_single_chat_json_responder(deepseek_usage_completion_payload());
        let (gateway, provider_id) = local_text_gateway(base_url, "deepseek-flash");
        let mut llm_config = serde_json::Map::new();
        llm_config.insert(
            "model".to_string(),
            Value::String(format!("{provider_id}::deepseek-flash")),
        );
        llm_config.insert(
            "prompt".to_string(),
            Value::String("audit usage".to_string()),
        );
        let mut agent = workflow_agent_with_canvas_node("text-gen", llm_config, gateway);

        assert!(matches!(
            agent.think(0, &[]).await.expect("start node"),
            AgentDecision::Continue
        ));
        assert!(matches!(
            agent.think(1, &[]).await.expect("text model node"),
            AgentDecision::Continue
        ));
        let run_usage = agent.cumulative_usage();
        let model_usage = agent.model_usage();

        let db = Arc::new(Database::open_in_memory().expect("ledger database"));
        let cost = Arc::new(crate::cost_service::CostService::new(db.clone()));
        let mut session =
            Session::create(&db, &SystemClock, Some("canvas usage audit")).expect("ledger session");
        let session_id = session.id().to_string();
        let discovered: DiscoveredToolSet = Arc::new(Mutex::new(HashSet::new()));
        let before = HashSet::new();
        AppRunFinalizer::new(
            db.clone(),
            Some(cost.clone()),
            None,
            Arc::new(Mutex::new(HashMap::new())),
            SessionMemoryPasses::default(),
        )
        .finalize_after_kernel(
            &mut session,
            AppRunFinalizerRequest {
                model_usage: Some(&model_usage),
                booked_calls: None,
                multi_model_label: "workflow",
                session_id: &session_id,
                run_id: "run_canvas_usage_audit",
                discovered_before_run: &before,
                discovered_tools: &discovered,
                usage: run_usage,
                side_usage: None,
                model_name: "deepseek-flash",
                sink: &NullEventSink,
                run_succeeded: false,
                capture_client: None,
                capture_model: None,
            },
        )
        .expect("production finalizer accepts the workflow accounting value");
        let ledger = cost.summary(&session_id).expect("cost summary");
        eprintln!(
            "canvas workflow usage audit: cumulative_usage={run_usage:?} ledger=({}, {}, {}, {})",
            ledger.input_tokens,
            ledger.output_tokens,
            ledger.cache_hit_tokens,
            ledger.cache_miss_tokens
        );

        let usage = run_usage.expect("WorkflowAgent exposes a cumulative usage value");
        assert_eq!(usage.billed_prompt_tokens, 100);
        assert_eq!(usage.completion_tokens, 20);
        assert_eq!(usage.reasoning_tokens, 12);
        assert_eq!(usage.billed_cache_hit_tokens, 80);
        assert_eq!(usage.billed_cache_miss_tokens, 20);
        assert_eq!(ledger.input_tokens, 100);
        assert_eq!(ledger.output_tokens, 20);
        assert_eq!(ledger.cache_hit_tokens, 80);
        assert_eq!(ledger.cache_miss_tokens, 20);
    }

    #[tokio::test]
    async fn audit_run_level_workflow_model_preserves_deepseek_usage_buckets() {
        use deepagent_models::{MockTransport, ModelClient, ModelConfig};
        use deepagent_runtime::agent::{Agent, AgentDecision};

        let transport = Arc::new(MockTransport::new([
            json!({
                "choices": [{
                    "delta": {
                        "content": "answer",
                        "reasoning_content": "reasoning"
                    },
                    "finish_reason": null
                }]
            })
            .to_string(),
            json!({
                "choices": [{ "delta": {}, "finish_reason": "stop" }],
                "usage": deepseek_usage_completion_payload()["usage"]
            })
            .to_string(),
            "[DONE]".to_string(),
        ]));
        let client = Arc::new(ModelClient::new(
            transport,
            ModelConfig::deepseek_chat("audit-key"),
        ));
        let mut config = serde_json::Map::new();
        config.insert(
            "llmPrompt".to_string(),
            Value::String("audit direct usage".to_string()),
        );
        let mut agent = workflow_agent_with_node("llm", config)
            .with_model(client, "deepseek-flash".to_string());

        assert!(matches!(
            agent.think(0, &[]).await.expect("start node"),
            AgentDecision::Continue
        ));
        assert!(matches!(
            agent.think(1, &[]).await.expect("run-level model node"),
            AgentDecision::Continue
        ));
        let usage = agent
            .cumulative_usage()
            .expect("WorkflowAgent exposes cumulative usage");
        eprintln!("run-level workflow usage audit: cumulative_usage={usage:?}");

        assert_eq!(usage.prompt_tokens, 100, "latest input gauge is retained");
        assert_eq!(usage.completion_tokens, 20, "output is retained");
        assert_eq!(usage.total_tokens, 120, "latest total gauge is retained");
        assert_eq!(
            usage.billed_prompt_tokens, 100,
            "the existing cost ledger reads billed input, not the latest gauge"
        );
        assert_eq!(usage.reasoning_tokens, 12);
        assert_eq!(usage.prompt_cache_hit_tokens, 80);
        assert_eq!(usage.prompt_cache_miss_tokens, 20);
        assert_eq!(usage.billed_cache_hit_tokens, 80);
        assert_eq!(usage.billed_cache_miss_tokens, 20);
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
                        max_reference_images: None,
                    },
                    CanvasModelConfig {
                        id: "gpt-image-2".to_string(),
                        name: "gpt-image-2".to_string(),
                        description: String::new(),
                        enabled: true,
                        scenarios: vec![CanvasScenario::ImageGeneration],
                        priority: 0,
                        max_reference_images: None,
                    },
                    CanvasModelConfig {
                        id: "Qwen/Qwen3-VL-Embedding-8B".to_string(),
                        name: "qwen-embed".to_string(),
                        description: String::new(),
                        enabled: true,
                        scenarios: vec![CanvasScenario::Embedding],
                        priority: 0,
                        max_reference_images: None,
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
            audio: Default::default(),
            operation: CanvasOperation::Embedding,
            prompt: "ping".to_string(),
            system_prompt: None,
            temperature: None,
            max_tokens: None,
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
                .read_bytes(&artifact_id, None)
                .expect("read")
                .expect("stored")
                .as_slice(),
            &[0x89, b'P', b'N', b'G', 0, 1]
        );
        let reused = artifacts
            .resolve_for_provider(&image.artifact_uri, None)
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

    /// 候选循环必须只在瞬时失败时换下一个模型。已有的
    /// `status_codes_split_transient_and_auth_failures` 只断言了
    /// 状态码分类，这里驱动真实的 `execute` 路径。
    #[test]
    fn only_transient_failures_fall_through_to_the_next_candidate() {
        use std::io::{Read, Write};
        let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let seen: Arc<std::sync::Mutex<Vec<String>>> = Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = seen.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut buf = [0u8; 2048];
                let mut text = String::new();
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
                let body = &text[head_end + 4..];
                let model = serde_json::from_str::<Value>(body)
                    .ok()
                    .and_then(|value| value["model"].as_str().map(str::to_string))
                    .unwrap_or_default();
                let (status, payload) = if model.contains("flaky") {
                    (
                        "503 Service Unavailable",
                        "{\"error\":{\"message\":\"overloaded\"}}".to_string(),
                    )
                } else if model.contains("denied") {
                    (
                        "401 Unauthorized",
                        "{\"error\":{\"message\":\"bad key\"}}".to_string(),
                    )
                } else {
                    (
                        "200 OK",
                        format!(
                            "{{\"choices\":[{{\"message\":{{\"content\":\"pong:{model}\"}}}}]}}"
                        ),
                    )
                };
                let response = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
                    payload.len()
                );
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.flush();
                sink.lock().expect("lock").push(model);
            }
        });
        let requested = || seen.lock().expect("lock").clone();

        let db = Arc::new(Database::open_in_memory().expect("db"));
        let providers = Arc::new(CanvasProviderService::new(
            db,
            Arc::new(crate::secret_store::MemorySecretStore::default()),
        ));
        let text_model = |id: &str| CanvasModelConfig {
            id: id.to_string(),
            name: id.to_string(),
            description: String::new(),
            enabled: true,
            scenarios: vec![CanvasScenario::Text],
            priority: 0,
            max_reference_images: None,
        };
        let created = providers
            .save_provider(CanvasProviderInput {
                name: "Local".to_string(),
                protocol: "openai".to_string(),
                base_url: format!("http://127.0.0.1:{port}/v1"),
                enabled: true,
                models: vec![
                    text_model("flaky-first"),
                    text_model("denied-first"),
                    text_model("healthy-second"),
                ],
                api_key: Some("sk-local".to_string()),
                ..Default::default()
            })
            .expect("provider");
        let bind = |models: &[&str]| {
            providers
                .save_bindings(
                    None,
                    models
                        .iter()
                        .map(|id| CanvasScenarioBinding {
                            scenario: "text".to_string(),
                            provider_id: created.id.clone(),
                            model_id: (*id).to_string(),
                            enabled: true,
                        })
                        .collect(),
                )
                .expect("bindings")
        };
        let gateway = CanvasModelGateway::new(providers.clone());

        bind(&["flaky-first", "healthy-second"]);
        let output = rt
            .block_on(gateway.execute(&CanvasModelRequest::text("ping"), None))
            .expect("a transient failure must move to the next candidate");
        match output {
            CanvasModelOutput::Text { text, model_id, .. } => {
                assert_eq!(text, "pong:healthy-second");
                assert_eq!(model_id, "healthy-second");
            }
            other => panic!("unexpected {other:?}"),
        }
        assert_eq!(requested(), vec!["flaky-first", "healthy-second"]);

        seen.lock().expect("lock").clear();
        bind(&["denied-first", "healthy-second"]);
        let error = rt
            .block_on(gateway.execute(&CanvasModelRequest::text("ping"), None))
            .expect_err("an auth failure must stop the chain");
        assert_eq!(error.code, CanvasErrorCode::SecretMissing);
        assert_eq!(
            requested(),
            vec!["denied-first"],
            "a non-transient failure must not leak the request to another model"
        );
    }

    /// 语音两端必须真的走线：合成回二进制并存成音频制品，转写把制品字节按
    /// multipart 上传并回文本；节点侧永远只拿到引用。
    #[test]
    fn audio_endpoints_round_trip_through_the_artifact_store() {
        use std::io::{Read, Write};
        let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let voice_bytes: Vec<u8> = b"ID3\x03\x00fake-voice-payload".to_vec();
        let received: Arc<std::sync::Mutex<Vec<(String, String, String)>>> =
            Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = received.clone();
        let spoken = voice_bytes.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut buf = [0u8; 4096];
                let mut text = String::new();
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
                let (content_type, payload) = if request_line.contains("/audio/speech") {
                    ("audio/mpeg", String::from_utf8_lossy(&spoken).to_string())
                } else {
                    ("application/json", "{\"text\":\"你好世界\"}".to_string())
                };
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
                    payload.len()
                );
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.flush();
                sink.lock().expect("lock").push((request_line, head, body));
            }
        });

        let db = Arc::new(Database::open_in_memory().expect("db"));
        let artifact_root =
            std::env::temp_dir().join(format!("deepagent-gateway-audio-{}", std::process::id()));
        let artifacts = Arc::new(
            crate::canvas_artifact_service::CanvasArtifactService::new(&artifact_root, db.clone())
                .expect("artifact service"),
        );
        let providers = Arc::new(CanvasProviderService::new(
            db,
            Arc::new(crate::secret_store::MemorySecretStore::default()),
        ));
        let config = |id: &str, scenario| CanvasModelConfig {
            id: id.to_string(),
            name: id.to_string(),
            description: String::new(),
            enabled: true,
            scenarios: vec![scenario],
            priority: 0,
            max_reference_images: None,
        };
        let created = providers
            .save_provider(CanvasProviderInput {
                name: "Local".to_string(),
                protocol: "openai".to_string(),
                base_url: format!("http://127.0.0.1:{port}/v1"),
                enabled: true,
                models: vec![
                    config("CosyVoice2-0.5B", CanvasScenario::TextToSpeech),
                    config("SenseVoiceSmall", CanvasScenario::SpeechToText),
                ],
                api_key: Some("sk-local".to_string()),
                ..Default::default()
            })
            .expect("provider");
        let gateway = CanvasModelGateway::new(providers).with_artifacts(artifacts.clone());

        let spoken = rt
            .block_on(gateway.run_audio(CanvasAudioRequest {
                model_ref: format!("{}::CosyVoice2-0.5B", created.id),
                operation: "speech_synthesize".to_string(),
                text: "你好".to_string(),
                audio_reference: None,
                voice: Some("voice-42".to_string()),
                format: Some("mp3".to_string()),
                timeout_ms: 5_000,
            }))
            .expect("synthesize");
        let uri = spoken.audio_url.clone().expect("artifact reference");
        assert!(uri.starts_with("artifact://"), "synthesize gave {uri}");
        assert!(spoken.text.is_none());
        let id = uri.trim_start_matches("artifact://");
        let record = artifacts.record(id, None).expect("record").expect("row");
        assert_eq!(
            record.kind,
            deepagent_persistence::artifact_store::ArtifactKind::Audio
        );
        assert_eq!(record.media_type.as_deref(), Some("audio/mpeg"));
        assert_eq!(
            artifacts
                .read_bytes(id, None)
                .expect("read")
                .expect("bytes"),
            voice_bytes
        );

        let heard = rt
            .block_on(gateway.run_audio(CanvasAudioRequest {
                model_ref: format!("{}::SenseVoiceSmall", created.id),
                operation: "speech_transcribe".to_string(),
                text: String::new(),
                audio_reference: Some(uri.clone()),
                voice: None,
                format: None,
                timeout_ms: 5_000,
            }))
            .expect("transcribe");
        assert_eq!(heard.text.as_deref(), Some("你好世界"));
        assert!(heard.audio_url.is_none());

        let captured = received.lock().expect("lock").clone();
        assert_eq!(captured.len(), 2, "both speech endpoints must be reached");
        let (speech_line, speech_head, speech_body) = &captured[0];
        assert!(
            speech_line.contains("POST /v1/audio/speech"),
            "speech line was {speech_line}"
        );
        assert!(speech_head.contains("content-type: application/json"));
        assert!(speech_body.contains("\"input\":\"你好\""));
        assert!(speech_body.contains("voice-42"));
        assert!(speech_body.contains("mp3"));
        let (asr_line, asr_head, asr_body) = &captured[1];
        assert!(
            asr_line.contains("POST /v1/audio/transcriptions"),
            "transcription line was {asr_line}"
        );
        assert!(
            asr_head.contains("multipart/form-data; boundary="),
            "transcription head was {asr_head}"
        );
        assert!(asr_body.contains("name=\"file\""));
        assert!(asr_body.contains("SenseVoiceSmall"));
        // 制品字节必须真的进表单，而不是把 artifact:// 引用发给供应商。
        assert!(
            asr_body.contains("fake-voice-payload"),
            "audio bytes missing from the upload"
        );
        let _ = std::fs::remove_dir_all(&artifact_root);
    }

    #[test]
    #[ignore = "audit reproducer: speech synthesis output must obey an artifact byte budget"]
    fn audit_speech_synthesis_output_rejects_oversized_artifact() {
        use std::io::{Read, Write};
        let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let configured_input_limit = 32 * 1024 * 1024usize;
        let audio = {
            let mut bytes = Vec::with_capacity(configured_input_limit + 1);
            bytes.extend_from_slice(b"ID3\x03\x00");
            bytes.resize(configured_input_limit + 1, 0x41);
            bytes
        };
        let response_audio = audio.clone();
        std::thread::spawn(move || {
            if let Some(Ok(mut stream)) = listener.incoming().next() {
                let mut request = [0u8; 4096];
                let _ = stream.read(&mut request);
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: audio/mpeg\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    response_audio.len()
                );
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(&response_audio);
                let _ = stream.flush();
            }
        });

        let db = Arc::new(Database::open_in_memory().expect("db"));
        let artifact_root = std::env::temp_dir().join(format!(
            "deepagent-gateway-audio-limit-{}",
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
                models: vec![CanvasModelConfig {
                    id: "local-voice".to_string(),
                    name: "local-voice".to_string(),
                    description: String::new(),
                    enabled: true,
                    scenarios: vec![CanvasScenario::TextToSpeech],
                    priority: 0,
                    max_reference_images: None,
                }],
                api_key: Some("sk-local".to_string()),
                ..Default::default()
            })
            .expect("provider");
        let gateway = CanvasModelGateway::new(providers).with_artifacts(artifacts.clone());
        let spoken = rt
            .block_on(gateway.run_audio(CanvasAudioRequest {
                model_ref: format!("{}::local-voice", created.id),
                operation: "speech_synthesize".to_string(),
                text: "audit".to_string(),
                audio_reference: None,
                voice: None,
                format: Some("mp3".to_string()),
                timeout_ms: 5_000,
            }))
            .expect("current implementation accepts the oversized response");
        let uri = spoken.audio_url.expect("artifact reference");
        let id = uri.trim_start_matches("artifact://");
        let artifact = artifacts
            .read_bytes(id, None)
            .expect("read")
            .expect("bytes");
        eprintln!(
            "speech output audit: configured_input_limit={configured_input_limit}, response_bytes={}, artifact_bytes={}",
            audio.len(),
            artifact.len()
        );
        assert!(
            artifact.len() <= configured_input_limit,
            "speech synthesis output exceeded the configured comparable media budget"
        );
        let _ = std::fs::remove_dir_all(&artifact_root);
    }

    #[tokio::test]
    async fn audit_speech_transcription_remote_reference_bypasses_network_policy() {
        super::audio_input_tests::assert_unprepared_reference_denied().await;
    }

    /// 视频作业整链走线：提交拿 requestId、轮询到 Succeed、把一小时内失效的
    /// 供应商 URL 当场下载成 Video 制品；带 resume 任务 id 时不再重复提交。
    #[test]
    fn video_job_polls_until_success_and_stores_the_download() {
        use std::io::{Read, Write};
        use std::sync::atomic::{AtomicUsize, Ordering};
        let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let mp4: Vec<u8> = b"\x00\x00\x00\x14ftypmp42mp4-payload".to_vec();
        let received: Arc<std::sync::Mutex<Vec<(String, String)>>> =
            Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = received.clone();
        let thread_polls = Arc::new(AtomicUsize::new(0));
        let payload = mp4.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut buf = [0u8; 4096];
                let mut text = String::new();
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
                let request_line = text.lines().next().unwrap_or_default().to_string();
                let body = text[head_end + 4..].to_string();
                let response = if request_line.contains("GET /v1/clip.mp4") {
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: video/mp4\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        payload.len()
                    ) + &String::from_utf8_lossy(&payload)
                } else if request_line.contains("/video/submit") {
                    let payload = "{\"requestId\":\"job-77\"}".to_string();
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
                        payload.len()
                    )
                } else {
                    let payload = if thread_polls.fetch_add(1, Ordering::SeqCst) == 0 {
                        "{\"status\":\"InQueue\"}".to_string()
                    } else {
                        "{\"status\":\"Succeed\",\"results\":{\"videos\":[{\"url\":\"http://127.0.0.1:__PORT__/v1/clip.mp4\"}]}}"
                            .replace("__PORT__", &port.to_string())
                    };
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
                        payload.len()
                    )
                };
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.flush();
                sink.lock().expect("lock").push((request_line, body));
            }
        });

        let db = Arc::new(Database::open_in_memory().expect("db"));
        let artifact_root =
            std::env::temp_dir().join(format!("deepagent-gateway-video-{}", std::process::id()));
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
                models: vec![CanvasModelConfig {
                    id: "Wan2.2-T2V-A14B".to_string(),
                    name: "wan".to_string(),
                    description: String::new(),
                    enabled: true,
                    scenarios: vec![CanvasScenario::VideoGeneration],
                    priority: 0,
                    max_reference_images: None,
                }],
                api_key: Some("sk-local".to_string()),
                ..Default::default()
            })
            .expect("provider");
        let gateway = CanvasModelGateway::new(providers).with_artifacts(artifacts.clone());
        let progress: Arc<std::sync::Mutex<Vec<String>>> =
            Arc::new(std::sync::Mutex::new(Vec::new()));
        let progress_sink = progress.clone();
        let base = CanvasVideoRequest {
            model_ref: format!("{}::Wan2.2-T2V-A14B", created.id),
            prompt: "一只橘猫在雨里走路".to_string(),
            image_url: None,
            size: Some("720p".to_string()),
            resume_task_id: None,
            resume_fingerprint: None,
            timeout_ms: 20_000,
            cancel: None,
            on_progress: Some(CanvasJobProgressSink::new(move |report| {
                progress_sink.lock().expect("lock").push(format!(
                    "{}@{}",
                    report.job_id,
                    report.phase.as_str()
                ));
            })),
        };

        let done = rt
            .block_on(gateway.generate_video(base.clone()))
            .expect("video job");
        assert_eq!(done.task_id, "job-77");
        assert_eq!(
            progress.lock().expect("lock").clone(),
            vec!["job-77@queued".to_string()],
            "a minutes-long job must announce itself before it finishes"
        );
        assert_eq!(done.mime, "video/mp4");
        assert!(
            done.artifact_uri.starts_with("artifact://"),
            "node saw {:?}",
            done.artifact_uri
        );
        let id = done.artifact_uri.trim_start_matches("artifact://");
        let record = artifacts.record(id, None).expect("record").expect("row");
        assert_eq!(
            record.kind,
            deepagent_persistence::artifact_store::ArtifactKind::Video
        );
        assert_eq!(record.media_type.as_deref(), Some("video/mp4"));
        assert_eq!(
            artifacts
                .read_bytes(id, None)
                .expect("read")
                .expect("bytes"),
            mp4,
            "the expiring provider url must be downloaded into the store"
        );

        let captured = received.lock().expect("lock").clone();
        let lines: Vec<&str> = captured.iter().map(|(line, _)| line.as_str()).collect();
        assert!(
            lines
                .iter()
                .any(|line| line.contains("POST /v1/video/submit")),
            "submit missing from {lines:?}"
        );
        assert_eq!(
            lines
                .iter()
                .filter(|line| line.contains("POST /v1/video/status"))
                .count(),
            2,
            "queued status must be polled again, got {lines:?}"
        );
        let submit_body = &captured
            .iter()
            .find(|(line, _)| line.contains("/video/submit"))
            .expect("submit")
            .1;
        assert!(submit_body.contains("Wan2.2-T2V-A14B"));
        assert!(submit_body.contains("720p"));
        let status_body = &captured
            .iter()
            .find(|(line, _)| line.contains("/video/status"))
            .expect("status")
            .1;
        assert!(
            status_body.contains("job-77"),
            "poll must carry the submitted task id, got {status_body}"
        );

        // 恢复：带着任务 id 再跑一次时不得再提交新作业。
        let submits_before = captured_submits(&received);
        let resumed = rt
            .block_on(gateway.generate_video(CanvasVideoRequest {
                resume_task_id: Some(done.task_id.clone()),
                resume_fingerprint: Some(done.resume_fingerprint.clone()),
                ..base
            }))
            .expect("resumed job");
        assert_eq!(resumed.task_id, done.task_id);
        assert_eq!(
            captured_submits(&received),
            submits_before,
            "a resumed job must not submit a second generation"
        );
        let _ = std::fs::remove_dir_all(&artifact_root);
    }

    #[test]
    fn generated_video_download_streams_beyond_the_legacy_artifact_limit() {
        use std::io::{Read, Write};
        let total_bytes = 64 * 1024 * 1024 + 1;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let address = listener.local_addr().expect("address");
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept download");
            let mut request = Vec::new();
            let mut buffer = [0u8; 1024];
            while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                let count = stream.read(&mut buffer).expect("read request");
                assert_ne!(count, 0, "request ended before headers");
                request.extend_from_slice(&buffer[..count]);
            }
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: {total_bytes}\r\nConnection: close\r\n\r\n"
            )
            .expect("write headers");
            let mut chunk = vec![b'x'; 64 * 1024];
            chunk[..12].copy_from_slice(b"\x00\x00\x00\x14ftypmp42");
            let mut remaining = total_bytes;
            while remaining > 0 {
                let count = remaining.min(chunk.len());
                stream.write_all(&chunk[..count]).expect("write body chunk");
                remaining -= count;
            }
        });

        let root = std::env::temp_dir().join(format!(
            "deepagent-gateway-large-video-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let db = Arc::new(Database::open_in_memory().expect("db"));
        let artifacts = crate::canvas_artifact_service::CanvasArtifactService::new(&root, db)
            .expect("artifact service");
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        let stored = runtime
            .block_on(fetch_video_artifact(
                &format!("http://{address}/video.mp4"),
                &artifacts,
                None,
                "large-job",
                None,
                total_bytes as u64,
            ))
            .expect("download large video");
        server.join().expect("server thread");
        assert_eq!(stored.byte_size, total_bytes as i64);
        assert_eq!(stored.media_type.as_deref(), Some("video/mp4"));
        let record = artifacts
            .record(&stored.id, None)
            .expect("record")
            .expect("row");
        assert_eq!(record.byte_size, total_bytes as i64);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn generated_video_download_cancellation_drops_staging_and_http_wait() {
        use std::sync::atomic::AtomicBool;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;
        use tokio::sync::Notify;

        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let response_started = Arc::new(Notify::new());
        let release_server = Arc::new(Notify::new());
        let started = response_started.clone();
        let release = release_server.clone();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("accept download");
            let mut request = Vec::new();
            let mut buffer = [0u8; 1024];
            while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                let count = socket.read(&mut buffer).await.expect("read request");
                assert_ne!(count, 0, "request ended before headers");
                request.extend_from_slice(&buffer[..count]);
            }
            socket
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: video/mp4\r\nContent-Length: 1024\r\nConnection: close\r\n\r\n",
                )
                .await
                .expect("write response headers");
            started.notify_one();
            release.notified().await;
        });

        let root = tempfile::tempdir().expect("artifact root");
        let db = Arc::new(Database::open_in_memory().expect("db"));
        let artifacts = Arc::new(
            crate::canvas_artifact_service::CanvasArtifactService::new(root.path(), db)
                .expect("artifact service"),
        );
        let cancel = Arc::new(AtomicBool::new(false));
        let task_artifacts = artifacts.clone();
        let task_cancel = cancel.clone();
        let download = tokio::spawn(async move {
            fetch_video_artifact(
                &format!("http://{address}/video.mp4"),
                task_artifacts.as_ref(),
                Some(&task_cancel),
                "cancel-download",
                None,
                DEFAULT_VIDEO_DOWNLOAD_LIMIT,
            )
            .await
        });

        tokio::time::timeout(Duration::from_secs(5), response_started.notified())
            .await
            .expect("download response must start");
        cancel.store(true, Ordering::SeqCst);
        let error = tokio::time::timeout(Duration::from_millis(300), download)
            .await
            .expect("cancellation must stop a pending body read")
            .expect("download task")
            .expect_err("cancelled download must not publish an artifact");
        release_server.notify_one();
        server.await.expect("server task");

        assert!(error.to_string().contains("Cancelled:"));
        assert_eq!(
            std::fs::read_dir(root.path())
                .expect("read artifact root")
                .count(),
            0,
            "cancellation must remove the unfinished .part file"
        );
    }

    #[test]
    fn media_containers_come_from_their_magic_bytes() {
        assert_eq!(
            sniff_video_mime(b"\x00\x00\x00\x14ftypmp42mp4-payload".as_slice()).as_deref(),
            Some("video/mp4")
        );
        assert_eq!(
            sniff_video_mime(b"\x00\x00\x00\x14ftypqt  quicktime".as_slice()).as_deref(),
            Some("video/quicktime")
        );
        assert_eq!(
            sniff_video_mime(b"\x1a\x45\xdf\xa3\x01\x00webm".as_slice()).as_deref(),
            Some("video/webm")
        );
        assert_eq!(sniff_video_mime(b"not-a-container".as_slice()), None);
        assert_eq!(
            sniff_audio_mime(b"ID3\x03\x00mp3-payload".as_slice()).as_deref(),
            Some("audio/mpeg")
        );
        assert_eq!(
            sniff_audio_mime(b"RIFF\x00\x00\x00\x00WAVEfmt ".as_slice()).as_deref(),
            Some("audio/wav")
        );
        assert_eq!(sniff_audio_mime(b"\x89PNG\x00\x00".as_slice()), None);
    }

    fn captured_submits(received: &Arc<std::sync::Mutex<Vec<(String, String)>>>) -> usize {
        received
            .lock()
            .expect("lock")
            .iter()
            .filter(|(line, _)| line.contains("/video/submit"))
            .count()
    }

    pub(super) fn deepseek_usage_completion_payload() -> Value {
        json!({
            "choices": [{
                "message": {
                    "content": "answer",
                    "reasoning_content": "reasoning"
                }
            }],
            "usage": {
                "prompt_tokens": 100,
                "completion_tokens": 20,
                "total_tokens": 120,
                "prompt_tokens_details": { "cached_tokens": 80 },
                "prompt_cache_hit_tokens": 80,
                "prompt_cache_miss_tokens": 20,
                "completion_tokens_details": { "reasoning_tokens": 12 }
            }
        })
    }

    fn spawn_single_chat_responder(
        answer: &'static str,
    ) -> (String, Arc<std::sync::Mutex<Option<Value>>>) {
        spawn_single_chat_json_responder(
            json!({ "choices": [{ "message": { "content": answer } }] }),
        )
    }

    fn spawn_single_chat_json_responder(
        payload: Value,
    ) -> (String, Arc<std::sync::Mutex<Option<Value>>>) {
        use std::io::{Read, Write};

        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let captured = Arc::new(std::sync::Mutex::new(None));
        let sink = captured.clone();
        std::thread::spawn(move || {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let mut buf = [0u8; 2048];
            let mut text = String::new();
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
            let body = text.get(head_end + 4..).unwrap_or_default();
            *sink.lock().expect("lock") = serde_json::from_str(body).ok();

            let payload = payload.to_string();
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
                payload.len()
            );
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.flush();
        });
        (format!("http://127.0.0.1:{port}/v1"), captured)
    }

    pub(super) fn local_text_gateway(
        base_url: String,
        model_id: &str,
    ) -> (CanvasModelGateway, String) {
        let db = Arc::new(Database::open_in_memory().expect("db"));
        let providers = Arc::new(CanvasProviderService::new(
            db,
            Arc::new(crate::secret_store::MemorySecretStore::default()),
        ));
        let created = providers
            .save_provider(CanvasProviderInput {
                name: "Canvas text audit".to_string(),
                protocol: "openai".to_string(),
                base_url,
                enabled: true,
                models: vec![CanvasModelConfig {
                    id: model_id.to_string(),
                    name: model_id.to_string(),
                    description: String::new(),
                    enabled: true,
                    scenarios: vec![CanvasScenario::Text],
                    priority: 0,
                    max_reference_images: None,
                }],
                api_key: Some("sk-audit".to_string()),
                ..Default::default()
            })
            .expect("provider");
        (CanvasModelGateway::new(providers), created.id)
    }

    fn workflow_agent_with_canvas_node(
        kind: &str,
        config: serde_json::Map<String, Value>,
        gateway: CanvasModelGateway,
    ) -> deepagent_runtime::workflow::WorkflowAgent {
        workflow_agent_with_node(kind, config).with_canvas_bridge(Arc::new(gateway))
    }

    fn workflow_agent_with_node(
        kind: &str,
        config: serde_json::Map<String, Value>,
    ) -> deepagent_runtime::workflow::WorkflowAgent {
        use deepagent_runtime::workflow::{
            compile, NodeEventPublisher, WorkflowAgent, WorkflowDefinition, WorkflowEdgeSpec,
            WorkflowNodeSpec,
        };

        let definition = WorkflowDefinition {
            version: 1,
            nodes: vec![
                WorkflowNodeSpec {
                    id: "start".to_string(),
                    kind: "start".to_string(),
                    config: serde_json::Map::new(),
                },
                WorkflowNodeSpec {
                    id: "model".to_string(),
                    kind: kind.to_string(),
                    config,
                },
                WorkflowNodeSpec {
                    id: "end".to_string(),
                    kind: "end".to_string(),
                    config: serde_json::Map::new(),
                },
            ],
            edges: vec![
                WorkflowEdgeSpec {
                    id: "start-model".to_string(),
                    source: "start".to_string(),
                    target: "model".to_string(),
                    source_handle: None,
                    target_handle: None,
                },
                WorkflowEdgeSpec {
                    id: "model-end".to_string(),
                    source: "model".to_string(),
                    target: "end".to_string(),
                    source_handle: (kind == "question-classifier").then(|| "class-0".to_string()),
                    target_handle: None,
                },
            ],
        };
        WorkflowAgent::new(
            compile(definition).expect("valid audit workflow"),
            serde_json::Map::new(),
            None,
            NodeEventPublisher::default(),
        )
        .expect("valid workflow inputs")
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
