//! Speech transcription + meeting-minutes generation (office-agent Phase 2).
//!
//! Transcription uses the managed `whisper-cli` sidecar. The model file and
//! engine binary are runtime assets resolved via [`RuntimeService`] (downloaded
//! on demand) — never bundled.
//!
//! Meeting minutes are produced by the system LLM via [`ChatService`], turning
//! a transcript into a structured document outline.

use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use deepagent_core::error::{CoreError, Result};
use deepagent_models::ThinkingDepth;

use crate::chat_service::ChatService;
use crate::dto::TranscriptSegmentDto;
use crate::runtime_service::RuntimeService;

/// The capability id a speech model provides (matches the runtime registry).
const SPEECH_MODEL_CAPABILITY: &str = "speech-model";
const SPEECH_ENGINE_CAPABILITY: &str = "speech-engine";
/// Default model id + on-disk file name (the `whisper-base` runtime entry).
const DEFAULT_MODEL_ID: &str = "whisper-base";
const DEFAULT_MODEL_FILE: &str = "ggml-base.bin";
const DEFAULT_TRANSCRIPTION_LANGUAGE: &str = "zh";
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x08000000;

/// System prompt steering the LLM to produce a structured meeting-minutes doc.
const MINUTES_SYSTEM_PROMPT: &str = "你是会议纪要助手。根据用户提供的录音转写，整理成结构化的中文会议纪要。\
严格输出以下小节（缺失内容写“（无）”）：会议主题、会议时间、参会人、会议摘要、关键决策、待办事项、风险问题、原始转写。\
不要编造未在转写中出现的事实。";

/// Abstracts the transcription engine so the service is testable and so the
/// heavy whisper.cpp build stays optional.
pub trait TranscriptionEngine: Send + Sync {
    /// Transcribe `wav_path` using the model at `model_path`.
    fn transcribe(
        &self,
        wav_path: &Path,
        model_path: &Path,
        engine_dir: Option<&Path>,
    ) -> Result<Vec<TranscriptSegmentDto>>;
}

/// Engine used when the `whisper` feature is off — always errors with guidance.
pub struct UnavailableEngine;

impl TranscriptionEngine for UnavailableEngine {
    fn transcribe(
        &self,
        _wav_path: &Path,
        _model_path: &Path,
        _engine_dir: Option<&Path>,
    ) -> Result<Vec<TranscriptSegmentDto>> {
        Err(CoreError::Other(
            "speech transcription engine is not enabled in this build".to_string(),
        ))
    }
}

/// `whisper.cpp` command-line sidecar engine.
pub struct WhisperSidecarEngine;

impl WhisperSidecarEngine {
    pub fn new() -> Self {
        Self
    }
}

impl Default for WhisperSidecarEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl TranscriptionEngine for WhisperSidecarEngine {
    fn transcribe(
        &self,
        wav_path: &Path,
        model_path: &Path,
        engine_dir: Option<&Path>,
    ) -> Result<Vec<TranscriptSegmentDto>> {
        let engine_dir = engine_dir.ok_or_else(|| {
            CoreError::Other(
                "speech engine not installed — download runtime 'whisper-cli' first".to_string(),
            )
        })?;
        let exe = find_whisper_cli(engine_dir).ok_or_else(|| {
            CoreError::Other(format!(
                "speech engine executable missing under {} — reinstall 'whisper-cli'",
                engine_dir.display()
            ))
        })?;
        let out_base = std::env::temp_dir().join(format!(
            "deepagent-whisper-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or_default()
        ));
        let mut cmd = Command::new(&exe);
        cmd.arg("-m")
            .arg(model_path)
            .arg("-f")
            .arg(wav_path)
            .arg("-oj")
            .arg("-ojf")
            .arg("-of")
            .arg(&out_base)
            .arg("-np")
            .arg("-l")
            .arg(DEFAULT_TRANSCRIPTION_LANGUAGE)
            .current_dir(exe.parent().unwrap_or(engine_dir));
        configure_hidden_process(&mut cmd);
        let output = cmd
            .output()
            .map_err(|e| CoreError::Other(format!("run whisper-cli: {e}")))?;
        if !output.status.success() {
            return Err(CoreError::Other(format!(
                "whisper-cli failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        let json_path = out_base.with_extension("json");
        let json = std::fs::read_to_string(&json_path)
            .map_err(|e| CoreError::Other(format!("read whisper json: {e}")))?;
        let _ = std::fs::remove_file(&json_path);
        parse_whisper_json(&json)
    }
}

fn configure_hidden_process(_cmd: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        _cmd.creation_flags(CREATE_NO_WINDOW);
    }
}

/// Transcription + meeting-minutes service.
pub struct SpeechService {
    engine: Arc<dyn TranscriptionEngine>,
    runtime: Arc<RuntimeService>,
    chat: Arc<ChatService>,
}

impl SpeechService {
    /// Build over a transcription engine, the runtime manager (to locate the
    /// model), and the chat service (to generate minutes).
    pub fn new(
        engine: Arc<dyn TranscriptionEngine>,
        runtime: Arc<RuntimeService>,
        chat: Arc<ChatService>,
    ) -> Self {
        Self {
            engine,
            runtime,
            chat,
        }
    }

    /// True when a speech model is installed (so transcription can run without
    /// first prompting a download).
    pub fn model_installed(&self) -> bool {
        self.runtime.resolve(SPEECH_MODEL_CAPABILITY).is_some()
    }

    /// The runtime id to install when the model is missing.
    pub fn required_model_id(&self) -> &'static str {
        DEFAULT_MODEL_ID
    }

    /// True when the local `whisper.cpp` sidecar engine is installed.
    pub fn engine_installed(&self) -> bool {
        self.runtime.resolve(SPEECH_ENGINE_CAPABILITY).is_some()
    }

    /// The runtime id to install when the sidecar engine is missing.
    pub fn required_engine_id(&self) -> &'static str {
        "whisper-cli"
    }

    /// Transcribe a WAV file to timestamped segments, writing the result next
    /// to the audio as `<stem>_转写.json`. Errors clearly when the model is not
    /// installed (so the UI can offer to download it) or the engine is absent.
    pub fn transcribe_file(&self, wav_path: &str) -> Result<Vec<TranscriptSegmentDto>> {
        let model_dir = self
            .runtime
            .resolve(SPEECH_MODEL_CAPABILITY)
            .ok_or_else(|| {
                CoreError::Other(format!(
                    "speech model not installed — download runtime '{DEFAULT_MODEL_ID}' first"
                ))
            })?;
        let model_path = model_dir.join(DEFAULT_MODEL_FILE);
        if !model_path.exists() {
            return Err(CoreError::Other(format!(
                "speech model file missing at {} — reinstall '{DEFAULT_MODEL_ID}'",
                model_path.display()
            )));
        }

        let engine_dir = self.runtime.resolve(SPEECH_ENGINE_CAPABILITY);
        let segments =
            self.engine
                .transcribe(Path::new(wav_path), &model_path, engine_dir.as_deref())?;

        // Persist alongside the audio: <stem>_转写.json
        let transcript_path = transcript_path_for(wav_path);
        let json = serde_json::to_string_pretty(&segments)
            .map_err(|e| CoreError::Other(format!("serialize transcript: {e}")))?;
        std::fs::write(&transcript_path, json)
            .map_err(|e| CoreError::Other(format!("write transcript: {e}")))?;

        Ok(segments)
    }

    /// Generate a structured Markdown meeting-minutes document from a transcript
    /// (plain text or the joined segment text) via the system LLM.
    pub async fn generate_meeting_minutes(&self, transcript: &str) -> Result<String> {
        if transcript.trim().is_empty() {
            return Err(CoreError::invalid("transcript is empty"));
        }
        let user_prompt =
            format!("以下是会议录音的转写内容，请整理成结构化会议纪要：\n\n{transcript}");
        self.chat
            .run_review_streaming(
                MINUTES_SYSTEM_PROMPT,
                &user_prompt,
                ThinkingDepth::Medium,
                3000,
                |_tok| {},
            )
            .await
    }
}

/// Compute the transcript JSON path next to a WAV file: `<stem>_转写.json`.
fn transcript_path_for(wav_path: &str) -> std::path::PathBuf {
    let p = Path::new(wav_path);
    let stem = p
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "recording".to_string());
    let parent = p.parent().map(|p| p.to_path_buf()).unwrap_or_default();
    parent.join(format!("{stem}_转写.json"))
}

fn find_whisper_cli(root: &Path) -> Option<std::path::PathBuf> {
    let candidates = [
        "whisper-cli.exe",
        "Release/whisper-cli.exe",
        "bin/whisper-cli.exe",
        "whisper-cli",
        "Release/whisper-cli",
        "bin/whisper-cli",
        "whisper-bin-ubuntu-x64/whisper-cli",
        "whisper-bin-ubuntu-arm64/whisper-cli",
        "deepagent-whisper-cli-macos-x64/whisper-cli",
        "deepagent-whisper-cli-macos-arm64/whisper-cli",
        "main.exe",
        "Release/main.exe",
        "main",
    ];
    candidates
        .iter()
        .map(|rel| root.join(rel))
        .find(|p| p.is_file())
}

fn parse_whisper_json(json: &str) -> Result<Vec<TranscriptSegmentDto>> {
    let value: serde_json::Value = serde_json::from_str(json)
        .map_err(|e| CoreError::Other(format!("parse whisper json: {e}")))?;
    let transcription = value
        .get("transcription")
        .and_then(|v| v.as_array())
        .ok_or_else(|| CoreError::Other("whisper json missing transcription array".to_string()))?;
    let mut out = Vec::with_capacity(transcription.len());
    for item in transcription {
        let text = item
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .trim()
            .to_string();
        if text.is_empty() {
            continue;
        }
        let offsets = item.get("offsets");
        let timestamps = item.get("timestamps");
        let start_ms = offsets
            .and_then(|o| o.get("from"))
            .and_then(json_number_to_u64)
            .or_else(|| {
                timestamps
                    .and_then(|t| t.get("from"))
                    .and_then(|v| v.as_str())
                    .and_then(parse_whisper_timestamp_ms)
            })
            .unwrap_or(0);
        let end_ms = offsets
            .and_then(|o| o.get("to"))
            .and_then(json_number_to_u64)
            .or_else(|| {
                timestamps
                    .and_then(|t| t.get("to"))
                    .and_then(|v| v.as_str())
                    .and_then(parse_whisper_timestamp_ms)
            })
            .unwrap_or(start_ms);
        out.push(TranscriptSegmentDto {
            start_ms,
            end_ms,
            text,
            speaker: None,
            confidence: None,
        });
    }
    Ok(out)
}

fn json_number_to_u64(v: &serde_json::Value) -> Option<u64> {
    v.as_u64()
        .or_else(|| v.as_i64().and_then(|n| u64::try_from(n).ok()))
}

fn parse_whisper_timestamp_ms(s: &str) -> Option<u64> {
    let clean = s.trim().replace(',', ".");
    let parts: Vec<&str> = clean.split(':').collect();
    if parts.len() != 3 {
        return None;
    }
    let hours: u64 = parts[0].parse().ok()?;
    let minutes: u64 = parts[1].parse().ok()?;
    let seconds: f64 = parts[2].parse().ok()?;
    Some((((hours * 60 + minutes) * 60) as f64 * 1000.0 + seconds * 1000.0).round() as u64)
}

/// Build the default speech engine over the managed whisper-cli sidecar.
pub fn default_engine() -> Arc<dyn TranscriptionEngine> {
    // 默认走托管 sidecar，避免 all-features 门禁被本机 whisper.cpp FFI 工具链绑死。
    Arc::new(WhisperSidecarEngine::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transcript_path_is_sibling_with_suffix() {
        let p = transcript_path_for("/tmp/recordings/123_meeting.wav");
        assert!(p.to_string_lossy().ends_with("_转写.json"));
        assert!(p.to_string_lossy().contains("123_meeting"));
    }

    #[test]
    fn unavailable_engine_errors() {
        let e = UnavailableEngine;
        let err = e
            .transcribe(Path::new("a.wav"), Path::new("m.bin"), None)
            .unwrap_err();
        assert!(err.to_string().contains("not enabled"));
    }

    #[test]
    fn parses_whisper_json_segments() {
        let json = r#"{
            "transcription": [
                {
                    "timestamps": { "from": "00:00:00,000", "to": "00:00:01,230" },
                    "offsets": { "from": 0, "to": 1230 },
                    "text": " hello "
                }
            ]
        }"#;
        let segments = parse_whisper_json(json).unwrap();
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].start_ms, 0);
        assert_eq!(segments[0].end_ms, 1230);
        assert_eq!(segments[0].text, "hello");
    }
}
