//! Bounded IPC uploads into the existing artifact service. Unfinished streams
//! are RAII-cleaned on abort, idle expiration, and orderly host shutdown.
use crate::canvas_artifact_service::{CanvasArtifactService, CanvasArtifactStream};
use crate::canvas_scope::normalize_workspace_id;
use deepagent_core::error::{CoreError, Result};
use deepagent_persistence::artifact_store::ArtifactKind;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const MAX_CHUNK: usize = 256 * 1024;
const MAX_UPLOADS: usize = 8;
const MAX_UPLOAD_BYTES: u64 = 8 * 1024 * 1024 * 1024;
const IDLE_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum ArtifactUploadAction {
    Begin {
        kind: String,
        mime: String,
        name: String,
        size: u64,
    },
    BeginStreaming {
        kind: String,
        mime: String,
        name: String,
    },
    Write {
        id: String,
        offset: u64,
        bytes: Vec<u8>,
    },
    Finish {
        id: String,
    },
    Abort {
        id: String,
    },
}
struct Upload {
    stream: CanvasArtifactStream,
    workspace_id: Option<String>,
    caller_id: String,
    expected: Option<u64>,
    written: u64,
    touched: Instant,
}
pub struct CanvasArtifactUploads {
    artifacts: Arc<CanvasArtifactService>,
    uploads: Mutex<HashMap<String, Upload>>,
}
impl CanvasArtifactUploads {
    pub fn new(artifacts: Arc<CanvasArtifactService>) -> Self {
        Self {
            artifacts,
            uploads: Mutex::new(HashMap::new()),
        }
    }
    /// The host supplies caller identity; workspace is an explicit namespace,
    /// never a lookup of mutable active-project state. None is the default only.
    pub fn apply(
        &self,
        action: ArtifactUploadAction,
        workspace_id: Option<&str>,
        caller_id: &str,
    ) -> Result<Value> {
        let workspace_id = normalize_workspace_id(workspace_id)?;
        if caller_id.is_empty() {
            return Err(CoreError::invalid("upload caller identity is required"));
        }
        let mut uploads = self
            .uploads
            .lock()
            .map_err(|_| CoreError::invalid("artifact upload lock poisoned"))?;
        uploads.retain(|_, upload| upload.touched.elapsed() < IDLE_TIMEOUT);
        // Check before every mutation, including malformed chunks and abort.
        // A foreign request must neither consume the stream nor refresh its TTL.
        if let Some(id) = match &action {
            ArtifactUploadAction::Write { id, .. }
            | ArtifactUploadAction::Finish { id }
            | ArtifactUploadAction::Abort { id } => Some(id),
            _ => None,
        } {
            if !uploads.get(id).is_some_and(|upload| {
                upload.workspace_id == workspace_id && upload.caller_id == caller_id
            }) {
                tracing::warn!(
                    upload_id = id,
                    "canvas upload unavailable in requested context"
                );
                return Err(upload_unavailable(id));
            }
        }
        match action {
            ArtifactUploadAction::Begin {
                kind,
                mime,
                name,
                size,
            } => {
                if uploads.len() >= MAX_UPLOADS || size == 0 || size > MAX_UPLOAD_BYTES {
                    return Err(CoreError::invalid(
                        "empty upload or too many active uploads",
                    ));
                }
                let kind = match kind.as_str() {
                    "video" if mime.starts_with("video/") => ArtifactKind::Video,
                    "audio" if mime.starts_with("audio/") => ArtifactKind::Audio,
                    "image" if mime.starts_with("image/") => ArtifactKind::Image,
                    _ => return Err(CoreError::invalid("unsupported upload media kind/mime")),
                };
                if name.len() > 255 || mime.len() > 128 {
                    return Err(CoreError::invalid("upload metadata too long"));
                }
                let stream = self.artifacts.begin_stream(
                    kind,
                    Some(&mime),
                    Some(&name),
                    workspace_id.as_deref(),
                )?;
                let id = format!("upload_{}", deepagent_core::id::EventId::new());
                uploads.insert(
                    id.clone(),
                    Upload {
                        stream,
                        workspace_id,
                        caller_id: caller_id.to_string(),
                        expected: Some(size),
                        written: 0,
                        touched: Instant::now(),
                    },
                );
                Ok(json!(id))
            }
            ArtifactUploadAction::BeginStreaming { kind, mime, name } => {
                if uploads.len() >= MAX_UPLOADS {
                    return Err(CoreError::invalid("too many active uploads"));
                }
                let kind = match kind.as_str() {
                    "video" if mime.starts_with("video/") => ArtifactKind::Video,
                    "audio" if mime.starts_with("audio/") => ArtifactKind::Audio,
                    _ => return Err(CoreError::invalid("unsupported upload media kind/mime")),
                };
                if name.len() > 255 || mime.len() > 128 {
                    return Err(CoreError::invalid("upload metadata too long"));
                }
                let stream = self.artifacts.begin_stream(
                    kind,
                    Some(&mime),
                    Some(&name),
                    workspace_id.as_deref(),
                )?;
                let id = format!("upload_{}", deepagent_core::id::EventId::new());
                uploads.insert(
                    id.clone(),
                    Upload {
                        stream,
                        workspace_id,
                        caller_id: caller_id.to_string(),
                        expected: None,
                        written: 0,
                        touched: Instant::now(),
                    },
                );
                Ok(json!(id))
            }
            ArtifactUploadAction::Write { id, offset, bytes } => {
                let upload = uploads
                    .get_mut(&id)
                    .ok_or_else(|| upload_unavailable(&id))?;
                if bytes.is_empty()
                    || bytes.len() > MAX_CHUNK
                    || offset != upload.written
                    || upload.written.saturating_add(bytes.len() as u64) > MAX_UPLOAD_BYTES
                    || upload.expected.is_some_and(|expected| {
                        upload.written.saturating_add(bytes.len() as u64) > expected
                    })
                {
                    uploads.remove(&id);
                    return Err(CoreError::invalid("invalid upload size or chunk offset"));
                }
                if let Err(error) = upload.stream.write_chunk(&bytes) {
                    uploads.remove(&id);
                    return Err(error);
                }
                upload.written += bytes.len() as u64;
                upload.touched = Instant::now();
                Ok(json!(upload.written))
            }
            ArtifactUploadAction::Finish { id } => {
                let upload = uploads.remove(&id).ok_or_else(|| upload_unavailable(&id))?;
                if upload
                    .expected
                    .is_some_and(|expected| expected != upload.written)
                    || upload.written == 0
                {
                    return Err(CoreError::invalid("incomplete artifact upload"));
                }
                let artifact = upload.stream.finish()?;
                serde_json::to_value(artifact)
                    .map_err(|error| CoreError::invalid(error.to_string()))
            }
            ArtifactUploadAction::Abort { id } => {
                uploads.remove(&id);
                Ok(Value::Null)
            }
        }
    }
}

fn upload_unavailable(id: &str) -> CoreError {
    CoreError::ArtifactUploadUnavailable {
        upload_id: id.to_string(),
    }
}

#[cfg(test)]
mod ownership_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canvas_artifact_service::CanvasArtifactService;
    use deepagent_persistence::Database;
    use std::sync::Arc;

    fn uploads() -> (tempfile::TempDir, CanvasArtifactUploads) {
        let dir = tempfile::tempdir().expect("temp dir");
        let db = Arc::new(Database::open_in_memory().expect("database"));
        let service =
            Arc::new(CanvasArtifactService::new(dir.path(), db).expect("artifact service"));
        (dir, CanvasArtifactUploads::new(service))
    }

    #[test]
    fn upload_is_published_only_after_all_ordered_chunks_arrive() {
        let (_dir, uploads) = uploads();
        let id = uploads
            .apply(
                ArtifactUploadAction::Begin {
                    kind: "video".into(),
                    mime: "video/mp4".into(),
                    name: "test.mp4".into(),
                    size: 5,
                },
                None,
                "test-window",
            )
            .expect("begin")
            .as_str()
            .expect("id")
            .to_string();
        uploads
            .apply(
                ArtifactUploadAction::Write {
                    id: id.clone(),
                    offset: 0,
                    bytes: vec![1, 2],
                },
                None,
                "test-window",
            )
            .expect("first chunk");
        uploads
            .apply(
                ArtifactUploadAction::Write {
                    id: id.clone(),
                    offset: 2,
                    bytes: vec![3, 4, 5],
                },
                None,
                "test-window",
            )
            .expect("second chunk");
        let result = uploads
            .apply(ArtifactUploadAction::Finish { id }, None, "test-window")
            .expect("finish");
        assert_eq!(result["byteSize"], 5);
        assert!(result["uri"]
            .as_str()
            .expect("uri")
            .starts_with("artifact://"));
    }

    #[test]
    fn invalid_offset_discards_staging_and_oversized_upload_is_rejected() {
        let (dir, uploads) = uploads();
        assert!(uploads
            .apply(
                ArtifactUploadAction::Begin {
                    kind: "video".into(),
                    mime: "video/mp4".into(),
                    name: "huge.mp4".into(),
                    size: MAX_UPLOAD_BYTES + 1,
                },
                None,
                "test-window"
            )
            .is_err());
        let id = uploads
            .apply(
                ArtifactUploadAction::Begin {
                    kind: "video".into(),
                    mime: "video/mp4".into(),
                    name: "test.mp4".into(),
                    size: 3,
                },
                None,
                "test-window",
            )
            .expect("begin")
            .as_str()
            .expect("id")
            .to_string();
        assert!(uploads
            .apply(
                ArtifactUploadAction::Write {
                    id,
                    offset: 1,
                    bytes: vec![1],
                },
                None,
                "test-window"
            )
            .is_err());
        assert_eq!(
            std::fs::read_dir(dir.path()).expect("list files").count(),
            0
        );
    }

    #[test]
    fn streaming_upload_accepts_unknown_length_and_enforces_chunk_order() {
        let (_dir, uploads) = uploads();
        let id = uploads
            .apply(
                ArtifactUploadAction::BeginStreaming {
                    kind: "video".into(),
                    mime: "video/webm;codecs=vp9".into(),
                    name: "trimmed.webm".into(),
                },
                None,
                "test-window",
            )
            .expect("begin streaming")
            .as_str()
            .expect("id")
            .to_string();
        uploads
            .apply(
                ArtifactUploadAction::Write {
                    id: id.clone(),
                    offset: 0,
                    bytes: vec![1, 2],
                },
                None,
                "test-window",
            )
            .expect("first chunk");
        uploads
            .apply(
                ArtifactUploadAction::Write {
                    id: id.clone(),
                    offset: 2,
                    bytes: vec![3],
                },
                None,
                "test-window",
            )
            .expect("second chunk");
        let result = uploads
            .apply(ArtifactUploadAction::Finish { id }, None, "test-window")
            .expect("finish");
        assert_eq!(result["byteSize"], 3);
        assert_eq!(result["mediaType"], "video/webm;codecs=vp9");
    }

    #[test]
    fn empty_streaming_upload_is_not_published() {
        let (dir, uploads) = uploads();
        let id = uploads
            .apply(
                ArtifactUploadAction::BeginStreaming {
                    kind: "video".into(),
                    mime: "video/webm".into(),
                    name: "empty.webm".into(),
                },
                None,
                "test-window",
            )
            .expect("begin streaming")
            .as_str()
            .expect("id")
            .to_string();
        assert!(uploads
            .apply(ArtifactUploadAction::Finish { id }, None, "test-window")
            .is_err());
        assert_eq!(
            std::fs::read_dir(dir.path()).expect("list files").count(),
            0
        );
    }

    #[test]
    fn audit_chunked_upload_preserves_workspace_ownership() {
        let dir = tempfile::tempdir().expect("temp dir");
        let db = Arc::new(Database::open_in_memory().expect("database"));
        let service =
            Arc::new(CanvasArtifactService::new(dir.path(), db).expect("artifact service"));
        let uploads = CanvasArtifactUploads::new(service.clone());
        let id = uploads
            .apply(
                ArtifactUploadAction::Begin {
                    kind: "video".into(),
                    mime: "video/mp4".into(),
                    name: "workspace-owned.mp4".into(),
                    size: 4,
                },
                Some("audit-workspace"),
                "test-window",
            )
            .expect("begin")
            .as_str()
            .expect("upload id")
            .to_string();
        uploads
            .apply(
                ArtifactUploadAction::Write {
                    id: id.clone(),
                    offset: 0,
                    bytes: vec![1, 2, 3, 4],
                },
                Some("audit-workspace"),
                "test-window",
            )
            .expect("write");
        let dto = uploads
            .apply(
                ArtifactUploadAction::Finish { id },
                Some("audit-workspace"),
                "test-window",
            )
            .expect("finish");
        let artifact_id = dto["id"].as_str().expect("artifact id");
        let record = service
            .record(artifact_id, Some("audit-workspace"))
            .expect("record")
            .expect("published record");
        eprintln!(
            "canvas chunked upload ownership audit: dto_workspace_id={:?}, record_workspace_id={:?}, byte_size={}",
            dto["workspaceId"],
            record.workspace_id,
            record.byte_size,
        );
        assert_eq!(record.workspace_id.as_deref(), Some("audit-workspace"));
        assert_eq!(dto["workspaceId"], "audit-workspace");
    }
}
