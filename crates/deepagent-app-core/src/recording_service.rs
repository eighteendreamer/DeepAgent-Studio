//! Microphone recording for the desktop recording panel (office-agent Phase 2).
//!
//! Owns the [`RecordingSessionDto`] lifecycle (idle → recording ⇄ paused →
//! done) and writes captured audio to `recordings/<ts>_<name>.wav` under the
//! app data dir. The capture itself is delegated to an injected
//! [`AudioRecorder`] so the state machine is unit-tested without audio
//! hardware; the real `cpal`-backed recorder lives behind the `audio` feature.
//!
//! Recordings are local-only — nothing is uploaded.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use deepagent_core::error::{CoreError, Result};

use crate::dto::RecordingSessionDto;

/// Abstracts the audio capture backend so the service is testable without a
/// microphone. The real implementation is [`CpalRecorder`] (feature `audio`).
pub trait AudioRecorder: Send + Sync {
    /// List human-readable input device names.
    fn list_input_devices(&self) -> Result<Vec<String>>;
    /// Start capturing to `output_path` (16kHz-ish mono WAV).
    fn start(&self, session_id: &str, output_path: &Path) -> Result<()>;
    /// Pause capture (samples dropped until resumed).
    fn pause(&self, session_id: &str) -> Result<()>;
    /// Resume capture.
    fn resume(&self, session_id: &str) -> Result<()>;
    /// Stop capture and finalize the WAV file.
    fn stop(&self, session_id: &str) -> Result<()>;
}

/// An [`AudioRecorder`] that always errors — used when the `audio` feature is
/// off so the service still compiles and runs (capture is unavailable).
pub struct UnavailableRecorder;

impl AudioRecorder for UnavailableRecorder {
    fn list_input_devices(&self) -> Result<Vec<String>> {
        Ok(Vec::new())
    }
    fn start(&self, _session_id: &str, _output_path: &Path) -> Result<()> {
        Err(CoreError::Other(
            "audio capture is not enabled in this build".to_string(),
        ))
    }
    fn pause(&self, _session_id: &str) -> Result<()> {
        Err(CoreError::Other("audio capture is not enabled".to_string()))
    }
    fn resume(&self, _session_id: &str) -> Result<()> {
        Err(CoreError::Other("audio capture is not enabled".to_string()))
    }
    fn stop(&self, _session_id: &str) -> Result<()> {
        Err(CoreError::Other("audio capture is not enabled".to_string()))
    }
}

/// Manages recording sessions and their on-disk WAV artifacts.
pub struct RecordingService {
    recordings_dir: PathBuf,
    recorder: std::sync::Arc<dyn AudioRecorder>,
    sessions: Mutex<HashMap<String, Session>>,
    counter: AtomicU64,
}

/// Internal per-session state (DTO + capture start instant).
struct Session {
    dto: RecordingSessionDto,
    started_instant: Option<std::time::Instant>,
    accumulated_ms: u64,
}

impl RecordingService {
    /// Build over `recordings_dir` (created on first use) with a recorder.
    pub fn new(
        recordings_dir: impl Into<PathBuf>,
        recorder: std::sync::Arc<dyn AudioRecorder>,
    ) -> Self {
        Self {
            recordings_dir: recordings_dir.into(),
            recorder,
            sessions: Mutex::new(HashMap::new()),
            counter: AtomicU64::new(0),
        }
    }

    /// List available input device names.
    pub fn list_input_devices(&self) -> Result<Vec<String>> {
        self.recorder.list_input_devices()
    }

    /// The directory recordings (and exported artifacts) are written to.
    pub fn recordings_dir(&self) -> &std::path::Path {
        &self.recordings_dir
    }

    /// Start a new recording. `name` is sanitized into the file name. Returns
    /// the new session DTO (`status == "recording"`).
    pub fn start_recording(&self, name: &str) -> Result<RecordingSessionDto> {
        std::fs::create_dir_all(&self.recordings_dir)
            .map_err(|e| CoreError::Other(format!("create recordings dir: {e}")))?;

        let id = self.new_id();
        let stamp = file_stamp();
        let safe = sanitize(name);
        let file = format!("{stamp}_{safe}.wav");
        let path = self.recordings_dir.join(&file);

        self.recorder.start(&id, &path)?;

        let dto = RecordingSessionDto {
            id: id.clone(),
            status: "recording".to_string(),
            started_at: now_ms(),
            duration_ms: 0,
            audio_path: Some(path.to_string_lossy().into_owned()),
            transcript_path: None,
            error: None,
        };
        let mut map = self.sessions.lock().map_err(lock_err)?;
        map.insert(
            id,
            Session {
                dto: dto.clone(),
                started_instant: Some(std::time::Instant::now()),
                accumulated_ms: 0,
            },
        );
        Ok(dto)
    }

    /// Pause an in-progress recording.
    pub fn pause_recording(&self, session_id: &str) -> Result<RecordingSessionDto> {
        self.transition(session_id, "recording", "paused", |svc, s| {
            svc.recorder.pause(session_id)?;
            // Fold the active interval into the accumulated total.
            if let Some(start) = s.started_instant.take() {
                s.accumulated_ms += start.elapsed().as_millis() as u64;
            }
            Ok(())
        })
    }

    /// Resume a paused recording.
    pub fn resume_recording(&self, session_id: &str) -> Result<RecordingSessionDto> {
        self.transition(session_id, "paused", "recording", |svc, s| {
            svc.recorder.resume(session_id)?;
            s.started_instant = Some(std::time::Instant::now());
            Ok(())
        })
    }

    /// Stop recording and finalize the WAV. Sets `duration_ms` and leaves the
    /// session ready for transcription (`status == "done"`).
    pub fn stop_recording(&self, session_id: &str) -> Result<RecordingSessionDto> {
        let mut map = self.sessions.lock().map_err(lock_err)?;
        let s = map
            .get_mut(session_id)
            .ok_or_else(|| CoreError::Other(format!("unknown recording '{session_id}'")))?;
        if s.dto.status != "recording" && s.dto.status != "paused" {
            return Err(CoreError::Other(format!(
                "cannot stop a recording in state '{}'",
                s.dto.status
            )));
        }
        self.recorder.stop(session_id)?;
        if let Some(start) = s.started_instant.take() {
            s.accumulated_ms += start.elapsed().as_millis() as u64;
        }
        s.dto.duration_ms = s.accumulated_ms;
        s.dto.status = "done".to_string();
        Ok(s.dto.clone())
    }

    /// Mark a session as failed with a message.
    pub fn mark_error(&self, session_id: &str, message: &str) -> Result<()> {
        let mut map = self.sessions.lock().map_err(lock_err)?;
        if let Some(s) = map.get_mut(session_id) {
            s.dto.status = "error".to_string();
            s.dto.error = Some(message.to_string());
        }
        Ok(())
    }

    /// Set the transcript path + status once transcription completes.
    pub fn set_transcript(&self, session_id: &str, transcript_path: &str) -> Result<()> {
        let mut map = self.sessions.lock().map_err(lock_err)?;
        if let Some(s) = map.get_mut(session_id) {
            s.dto.transcript_path = Some(transcript_path.to_string());
        }
        Ok(())
    }

    /// Get a session DTO by id.
    pub fn session(&self, session_id: &str) -> Result<Option<RecordingSessionDto>> {
        let map = self.sessions.lock().map_err(lock_err)?;
        Ok(map.get(session_id).map(|s| s.dto.clone()))
    }

    /// The audio path for a session (used by transcription).
    pub fn audio_path(&self, session_id: &str) -> Result<Option<String>> {
        let map = self.sessions.lock().map_err(lock_err)?;
        Ok(map.get(session_id).and_then(|s| s.dto.audio_path.clone()))
    }

    fn transition(
        &self,
        session_id: &str,
        from: &str,
        to: &str,
        f: impl FnOnce(&Self, &mut Session) -> Result<()>,
    ) -> Result<RecordingSessionDto> {
        let mut map = self.sessions.lock().map_err(lock_err)?;
        let s = map
            .get_mut(session_id)
            .ok_or_else(|| CoreError::Other(format!("unknown recording '{session_id}'")))?;
        if s.dto.status != from {
            return Err(CoreError::Other(format!(
                "cannot go {from}→{to} from state '{}'",
                s.dto.status
            )));
        }
        f(self, s)?;
        s.dto.status = to.to_string();
        Ok(s.dto.clone())
    }

    fn new_id(&self) -> String {
        let n = self.counter.fetch_add(1, Ordering::Relaxed);
        format!("rec-{}-{}", now_ms(), n)
    }
}

fn lock_err<T>(_e: T) -> CoreError {
    CoreError::Other("recording service lock poisoned".to_string())
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// A filesystem-safe timestamp `YYYYMMDD_HHMMSS`-ish using Unix seconds (no
/// extra deps; uniqueness is ensured by the session counter in the file's
/// sibling name when needed).
fn file_stamp() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("{secs}")
}

/// Sanitize a user-provided recording name into a safe file component.
fn sanitize(name: &str) -> String {
    let trimmed = name.trim();
    let base = if trimmed.is_empty() {
        "recording"
    } else {
        trimmed
    };
    let cleaned: String = base
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    cleaned.trim_matches('-').to_string()
}

// ---- cpal-backed recorder (behind the `audio` feature) --------------------

#[cfg(feature = "audio")]
pub use cpal_impl::CpalRecorder;

#[cfg(feature = "audio")]
mod cpal_impl {
    use super::*;
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    use std::sync::atomic::AtomicBool;
    use std::sync::mpsc::{self, Sender};
    use std::sync::Arc;

    enum Ctl {
        Pause,
        Resume,
        Stop,
    }

    struct Handle {
        tx: Sender<Ctl>,
    }

    /// Real microphone recorder using cpal + hound. Each session owns a
    /// dedicated thread that holds the (non-Send) cpal stream and writes mono
    /// i16 samples to a WAV file at the device's sample rate.
    pub struct CpalRecorder {
        handles: Mutex<HashMap<String, Handle>>,
    }

    impl Default for CpalRecorder {
        fn default() -> Self {
            Self {
                handles: Mutex::new(HashMap::new()),
            }
        }
    }

    impl CpalRecorder {
        pub fn new() -> Self {
            Self::default()
        }
    }

    impl AudioRecorder for CpalRecorder {
        fn list_input_devices(&self) -> Result<Vec<String>> {
            let host = cpal::default_host();
            let devices = host
                .input_devices()
                .map_err(|e| CoreError::Other(format!("enumerate input devices: {e}")))?;
            Ok(devices.filter_map(|d| d.name().ok()).collect())
        }

        fn start(&self, session_id: &str, output_path: &Path) -> Result<()> {
            let path = output_path.to_path_buf();
            let (tx, rx) = mpsc::channel::<Ctl>();
            let (ready_tx, ready_rx) = mpsc::channel::<Result<()>>();

            std::thread::spawn(move || {
                let paused = Arc::new(AtomicBool::new(false));
                let build = build_stream(&path, paused.clone());
                let (stream, writer) = match build {
                    Ok(v) => {
                        let _ = ready_tx.send(Ok(()));
                        v
                    }
                    Err(e) => {
                        let _ = ready_tx.send(Err(e));
                        return;
                    }
                };
                if let Err(e) = stream.play() {
                    let _ = writer.lock().map(|mut w| w.take());
                    let _ = ready_tx.send(Err(CoreError::Other(format!("stream play: {e}"))));
                    return;
                }
                // Block on control messages; drop the stream + finalize on Stop.
                loop {
                    match rx.recv() {
                        Ok(Ctl::Pause) => paused.store(true, Ordering::SeqCst),
                        Ok(Ctl::Resume) => paused.store(false, Ordering::SeqCst),
                        Ok(Ctl::Stop) | Err(_) => break,
                    }
                }
                drop(stream);
                let final_writer = writer.lock().ok().and_then(|mut g| g.take());
                if let Some(w) = final_writer {
                    let _ = w.finalize();
                }
            });

            // Wait for the thread to confirm the stream built/started.
            match ready_rx.recv() {
                Ok(Ok(())) => {
                    let mut map = self.handles.lock().map_err(lock_err)?;
                    map.insert(session_id.to_string(), Handle { tx });
                    Ok(())
                }
                Ok(Err(e)) => Err(e),
                Err(_) => Err(CoreError::Other("recorder thread exited early".to_string())),
            }
        }

        fn pause(&self, session_id: &str) -> Result<()> {
            self.send(session_id, Ctl::Pause)
        }
        fn resume(&self, session_id: &str) -> Result<()> {
            self.send(session_id, Ctl::Resume)
        }
        fn stop(&self, session_id: &str) -> Result<()> {
            let handle = {
                let mut map = self.handles.lock().map_err(lock_err)?;
                map.remove(session_id)
            };
            if let Some(h) = handle {
                let _ = h.tx.send(Ctl::Stop);
            }
            Ok(())
        }
    }

    impl CpalRecorder {
        fn send(&self, session_id: &str, ctl: Ctl) -> Result<()> {
            let map = self.handles.lock().map_err(lock_err)?;
            if let Some(h) = map.get(session_id) {
                h.tx.send(ctl)
                    .map_err(|e| CoreError::Other(format!("control send: {e}")))?;
            }
            Ok(())
        }
    }

    type SharedWriter = Arc<Mutex<Option<hound::WavWriter<std::io::BufWriter<std::fs::File>>>>>;

    /// Build a cpal input stream on the default device that downmixes to mono
    /// i16 and writes to a hound WAV at the device sample rate. Returns the
    /// stream (kept alive by the caller's thread) + the shared writer.
    fn build_stream(path: &Path, paused: Arc<AtomicBool>) -> Result<(cpal::Stream, SharedWriter)> {
        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .ok_or_else(|| CoreError::Other("no default input device (microphone)".to_string()))?;
        let config = device
            .default_input_config()
            .map_err(|e| CoreError::Other(format!("default input config: {e}")))?;
        let channels = config.channels() as usize;
        let sample_rate = config.sample_rate().0;

        let spec = hound::WavSpec {
            channels: 1,
            sample_rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let writer = hound::WavWriter::create(path, spec)
            .map_err(|e| CoreError::Other(format!("create wav: {e}")))?;
        let writer: SharedWriter = Arc::new(Mutex::new(Some(writer)));

        let err_fn = |e| eprintln!("[recording] stream error: {e}");
        let stream = match config.sample_format() {
            cpal::SampleFormat::F32 => {
                let w = writer.clone();
                let p = paused.clone();
                device.build_input_stream(
                    &config.into(),
                    move |data: &[f32], _: &_| {
                        if p.load(Ordering::SeqCst) {
                            return;
                        }
                        if let Ok(mut guard) = w.lock() {
                            if let Some(wr) = guard.as_mut() {
                                for frame in data.chunks(channels) {
                                    let avg =
                                        frame.iter().copied().sum::<f32>() / channels.max(1) as f32;
                                    let s = (avg.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
                                    let _ = wr.write_sample(s);
                                }
                            }
                        }
                    },
                    err_fn,
                    None,
                )
            }
            cpal::SampleFormat::I16 => {
                let w = writer.clone();
                let p = paused.clone();
                device.build_input_stream(
                    &config.into(),
                    move |data: &[i16], _: &_| {
                        if p.load(Ordering::SeqCst) {
                            return;
                        }
                        if let Ok(mut guard) = w.lock() {
                            if let Some(wr) = guard.as_mut() {
                                for frame in data.chunks(channels) {
                                    let sum: i32 = frame.iter().map(|&s| s as i32).sum();
                                    let avg = (sum / channels.max(1) as i32) as i16;
                                    let _ = wr.write_sample(avg);
                                }
                            }
                        }
                    },
                    err_fn,
                    None,
                )
            }
            cpal::SampleFormat::U16 => {
                let w = writer.clone();
                let p = paused.clone();
                device.build_input_stream(
                    &config.into(),
                    move |data: &[u16], _: &_| {
                        if p.load(Ordering::SeqCst) {
                            return;
                        }
                        if let Ok(mut guard) = w.lock() {
                            if let Some(wr) = guard.as_mut() {
                                for frame in data.chunks(channels) {
                                    let sum: i32 = frame.iter().map(|&s| s as i32 - 32768).sum();
                                    let avg = (sum / channels.max(1) as i32) as i16;
                                    let _ = wr.write_sample(avg);
                                }
                            }
                        }
                    },
                    err_fn,
                    None,
                )
            }
            other => {
                return Err(CoreError::Other(format!(
                    "unsupported sample format: {other:?}"
                )))
            }
        }
        .map_err(|e| CoreError::Other(format!("build input stream: {e}")))?;

        Ok((stream, writer))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    /// A recorder that records control calls without touching audio hardware.
    #[derive(Default)]
    struct MockRecorder {
        calls: Mutex<Vec<String>>,
    }

    impl AudioRecorder for MockRecorder {
        fn list_input_devices(&self) -> Result<Vec<String>> {
            Ok(vec!["Mock Mic".to_string()])
        }
        fn start(&self, session_id: &str, _output_path: &Path) -> Result<()> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("start:{session_id}"));
            Ok(())
        }
        fn pause(&self, session_id: &str) -> Result<()> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("pause:{session_id}"));
            Ok(())
        }
        fn resume(&self, session_id: &str) -> Result<()> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("resume:{session_id}"));
            Ok(())
        }
        fn stop(&self, session_id: &str) -> Result<()> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("stop:{session_id}"));
            Ok(())
        }
    }

    fn service() -> (RecordingService, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let svc =
            RecordingService::new(dir.path().to_path_buf(), Arc::new(MockRecorder::default()));
        (svc, dir)
    }

    #[test]
    fn full_lifecycle_transitions() {
        let (svc, _d) = service();
        let started = svc.start_recording("Team Sync").unwrap();
        assert_eq!(started.status, "recording");
        assert!(started.audio_path.as_deref().unwrap().ends_with(".wav"));

        let paused = svc.pause_recording(&started.id).unwrap();
        assert_eq!(paused.status, "paused");

        let resumed = svc.resume_recording(&started.id).unwrap();
        assert_eq!(resumed.status, "recording");

        let done = svc.stop_recording(&started.id).unwrap();
        assert_eq!(done.status, "done");
    }

    #[test]
    fn cannot_pause_when_not_recording() {
        let (svc, _d) = service();
        let started = svc.start_recording("x").unwrap();
        svc.pause_recording(&started.id).unwrap();
        // Pausing an already-paused session is invalid.
        let err = svc.pause_recording(&started.id).unwrap_err();
        assert!(err.to_string().contains("cannot go"));
    }

    #[test]
    fn stop_requires_active_session() {
        let (svc, _d) = service();
        let err = svc.stop_recording("nope").unwrap_err();
        assert!(err.to_string().contains("unknown recording"));
    }

    #[test]
    fn sanitize_makes_safe_names() {
        assert_eq!(sanitize("Team Sync 2026/06"), "Team-Sync-2026-06");
        assert_eq!(sanitize("   "), "recording");
        assert_eq!(sanitize("a.b.c"), "a-b-c");
    }

    #[test]
    fn mark_error_sets_status() {
        let (svc, _d) = service();
        let s = svc.start_recording("x").unwrap();
        svc.mark_error(&s.id, "device lost").unwrap();
        let cur = svc.session(&s.id).unwrap().unwrap();
        assert_eq!(cur.status, "error");
        assert_eq!(cur.error.as_deref(), Some("device lost"));
    }
}
