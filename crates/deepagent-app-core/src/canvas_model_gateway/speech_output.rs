//! One bounded speech reader, targeting either the existing artifact stream or
//! the legacy byte-returning API. No provider dispatch or separate store here.
use super::*;
use crate::canvas_artifact_service::{CanvasArtifactDto, CanvasArtifactStream};
use deepagent_core::CoreError;
use deepagent_persistence::artifact_store::ArtifactKind;

#[derive(Clone, Copy)]
pub(super) enum SpeechOutputTarget {
    Bytes,
    Artifact,
}

#[derive(Debug)]
pub(super) enum GatewayOutput {
    Model(CanvasModelOutput),
    SpeechArtifact {
        stored: CanvasArtifactDto,
        provider_id: String,
        model_id: String,
    },
}

impl GatewayOutput {
    pub(super) fn into_model(self) -> CanvasResult<CanvasModelOutput> {
        match self {
            Self::Model(output) => Ok(output),
            Self::SpeechArtifact { .. } => Err(CanvasError::new(
                CanvasErrorCode::ProviderFailure,
                "internal gateway output target does not match byte-returning API",
            )),
        }
    }
}

#[derive(Clone, Hash, PartialEq, Eq)]
struct SpeechLimitKey {
    provider_id: String,
    model_id: String,
    format: Option<String>,
}

#[derive(Clone)]
pub(super) struct SpeechOutputPolicy {
    default: u64,
    overrides: HashMap<SpeechLimitKey, u64>,
}

impl Default for SpeechOutputPolicy {
    fn default() -> Self {
        Self {
            default: DEFAULT_SPEECH_OUTPUT_LIMIT,
            overrides: HashMap::new(),
        }
    }
}

fn validate_limit(limit: u64) -> CoreResult<u64> {
    if limit == 0 || limit > i64::MAX as u64 || usize::try_from(limit).is_err() {
        return Err(CoreError::invalid(
            "speech output limit must be positive and fit the artifact index and host addressing",
        ));
    }
    Ok(limit)
}

fn normalized_format(format: Option<&str>) -> Option<String> {
    format
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_ascii_lowercase)
}

impl SpeechOutputPolicy {
    pub(super) fn set_default(&mut self, limit: u64) -> CoreResult<()> {
        self.default = validate_limit(limit)?;
        Ok(())
    }

    pub(super) fn set_for(
        &mut self,
        provider_id: &str,
        model_id: &str,
        format: Option<&str>,
        limit: u64,
    ) -> CoreResult<()> {
        let limit = validate_limit(limit)?;
        if [provider_id, model_id]
            .iter()
            .any(|key| key.is_empty() || key.trim() != *key)
            || format.is_some_and(|value| value.trim().is_empty())
        {
            return Err(CoreError::invalid(
                "speech limit scope requires nonempty provider/model/format identifiers",
            ));
        }
        self.overrides.insert(
            SpeechLimitKey {
                provider_id: provider_id.into(),
                model_id: model_id.into(),
                format: normalized_format(format),
            },
            limit,
        );
        Ok(())
    }

    pub(super) fn for_request(
        &self,
        candidate: &ResolvedCanvasModel,
        request: &CanvasModelRequest,
    ) -> u64 {
        let mut key = SpeechLimitKey {
            provider_id: candidate.provider.id.clone(),
            model_id: candidate.model.id.clone(),
            format: normalized_format(request.audio.format.as_deref()),
        };
        if let Some(limit) = self.overrides.get(&key) {
            return *limit;
        }
        key.format = None;
        self.overrides.get(&key).copied().unwrap_or(self.default)
    }
}

enum SpeechSink {
    Bytes { bytes: Vec<u8>, limit: usize },
    Artifact(Box<CanvasArtifactStream>),
}

impl SpeechSink {
    fn write(&mut self, chunk: &[u8]) -> CanvasResult<()> {
        match self {
            Self::Artifact(stream) => stream.write_chunk(chunk).map_err(speech_store_error),
            Self::Bytes { bytes, limit } => {
                let required = bytes.len().checked_add(chunk.len()).ok_or_else(|| {
                    CanvasError::new(
                        CanvasErrorCode::ResponseBodyTooLarge,
                        "SpeechArtifactTooLarge: byte count overflow",
                    )
                })?;
                if required > *limit {
                    return Err(CanvasError::new(
                        CanvasErrorCode::ResponseBodyTooLarge,
                        "SpeechArtifactTooLarge: byte sink limit exceeded",
                    ));
                }
                if required > bytes.capacity() {
                    let capacity = bytes
                        .capacity()
                        .saturating_mul(2)
                        .max(4096)
                        .max(required)
                        .min(*limit);
                    bytes
                        .try_reserve_exact(capacity - bytes.len())
                        .map_err(|_| {
                            CanvasError::new(
                                CanvasErrorCode::ProviderFailure,
                                "bounded speech byte sink allocation failed",
                            )
                        })?;
                }
                bytes.extend_from_slice(chunk);
                Ok(())
            }
        }
    }

    fn finish(self, mime: String, candidate: &ResolvedCanvasModel) -> CanvasResult<GatewayOutput> {
        match self {
            Self::Bytes { bytes, .. } => Ok(GatewayOutput::Model(CanvasModelOutput::Audio {
                mime,
                bytes,
                provider_id: candidate.provider.id.clone(),
                model_id: candidate.model.id.clone(),
            })),
            Self::Artifact(stream) => Ok(GatewayOutput::SpeechArtifact {
                stored: (*stream).finish().map_err(speech_store_error)?,
                provider_id: candidate.provider.id.clone(),
                model_id: candidate.model.id.clone(),
            }),
        }
    }
}

fn speech_store_error(error: CoreError) -> CanvasError {
    CanvasError::new(
        CanvasErrorCode::ProviderFailure,
        format!("store speech output: {error}"),
    )
}

fn speech_mime(head: &[u8], format: Option<&str>, declared: Option<&str>) -> CanvasResult<String> {
    if let Some(mime) = sniff_audio_mime(head) {
        return Ok(mime);
    }
    // Raw PCM/Opus have no reliable container magic. Keep these existing UI
    // options only when explicitly requested and the response declares that
    // raw format (or opaque binary). This does not validate codec parameters.
    let requested = normalized_format(format);
    let declared = declared
        .unwrap_or_default()
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    match requested.as_deref() {
        Some("pcm")
            if matches!(
                declared.as_str(),
                "audio/pcm" | "audio/l16" | "audio/l24" | "application/octet-stream"
            ) =>
        {
            Ok("audio/pcm".into())
        }
        Some("opus") if matches!(declared.as_str(), "audio/opus" | "application/octet-stream") => {
            Ok("audio/opus".into())
        }
        _ => Err(CanvasError::new(
            CanvasErrorCode::ProviderFailure,
            "speech output has no supported audio container signature or explicit raw format",
        )),
    }
}

impl CanvasModelGateway {
    fn speech_sink(
        &self,
        target: SpeechOutputTarget,
        mime: &str,
        workspace_id: Option<&str>,
        limit: u64,
    ) -> CanvasResult<SpeechSink> {
        match target {
            SpeechOutputTarget::Bytes => Ok(SpeechSink::Bytes {
                bytes: Vec::new(),
                limit: limit as usize,
            }),
            SpeechOutputTarget::Artifact => Ok(SpeechSink::Artifact(Box::new(
                self.artifact_store("store a synthesized voice")
                    .map_err(speech_store_error)?
                    .begin_stream(ArtifactKind::Audio, Some(mime), None, workspace_id)
                    .map_err(speech_store_error)?,
            ))),
        }
    }

    pub(super) async fn consume_speech_response(
        &self,
        mut response: reqwest::Response,
        candidate: &ResolvedCanvasModel,
        request: &CanvasModelRequest,
        workspace_id: Option<&str>,
        target: SpeechOutputTarget,
    ) -> CanvasResult<GatewayOutput> {
        let max_bytes = self.speech_output_policy.for_request(candidate, request);
        let mut limit = MediaByteLimit::new(MediaLimitKind::SpeechOutput, max_bytes);
        limit
            .check_declared(response.content_length())
            .map_err(to_canvas_response_size_error)?;
        let declared = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let mut head = Vec::with_capacity(12);
        let mut sink = None;
        let mut mime = None;
        while let Some(chunk) = response.chunk().await.map_err(|error| {
            // The provider has already received the synthesis request. Do not
            // replay on another candidate after an incomplete body outcome.
            CanvasError::new(
                CanvasErrorCode::ProviderFailure,
                format!(
                    "speech response incomplete; remote outcome unknown; not replayed: {}",
                    error.without_url()
                ),
            )
        })? {
            limit
                .accept(chunk.len())
                .map_err(to_canvas_response_size_error)?;
            if let Some(sink) = &mut sink {
                SpeechSink::write(sink, &chunk)?;
                continue;
            }
            let needed = (12 - head.len()).min(chunk.len());
            head.extend_from_slice(&chunk[..needed]);
            if head.len() < 12 {
                continue;
            }
            let actual = speech_mime(&head, request.audio.format.as_deref(), declared.as_deref())?;
            let mut output = self.speech_sink(target, &actual, workspace_id, max_bytes)?;
            output.write(&head)?;
            output.write(&chunk[needed..])?;
            sink = Some(output);
            mime = Some(actual);
        }
        if sink.is_none() {
            if head.is_empty() {
                return Err(CanvasError::new(
                    CanvasErrorCode::ProviderFailure,
                    "speech synthesis returned an empty body",
                ));
            }
            let actual = speech_mime(&head, request.audio.format.as_deref(), declared.as_deref())?;
            let mut output = self.speech_sink(target, &actual, workspace_id, max_bytes)?;
            output.write(&head)?;
            sink = Some(output);
            mime = Some(actual);
        }
        // Give the existing outer cancellation/deadline owner a poll before
        // the synchronous atomic publication. Dropping the sink cleans staging.
        tokio::task::yield_now().await;
        match (sink, mime) {
            (Some(output), Some(mime)) => output.finish(mime, candidate),
            _ => Err(CanvasError::new(
                CanvasErrorCode::ProviderFailure,
                "speech output sink is unavailable",
            )),
        }
    }
}
