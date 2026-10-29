//! Canvas media artifacts: bytes on disk, index in the kernel database.
//!
//! The canvas used to carry every uploaded, cropped and generated image as a
//! `data:image/...;base64` string, which then leaked into the workflow graph,
//! the node events and the persisted session. Here a blob is stored once under
//! the managed artifacts root and everything else refers to it by
//! `artifact://<id>` — the same index [`deepagent_persistence::artifact_store`]
//! already uses for truncated tool output, so there is one media store.
//!
//! Node configs and events may contain the URI; they must never contain bytes,
//! the absolute path or a provider's signed URL.

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::canvas_scope::normalize_workspace_id;
use base64::Engine;
use deepagent_core::clock::{Clock, SystemClock};
use deepagent_core::error::{CoreError, Result};
use deepagent_persistence::artifact_store::{
    verify_file_for_deletion, ArtifactKind, ArtifactRecord, ArtifactStore,
};
use deepagent_persistence::fs_commit;
use deepagent_persistence::Database;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// URI scheme stored on canvas nodes instead of the bytes themselves.
pub const ARTIFACT_URI_PREFIX: &str = "artifact://";

const COPY_BUFFER_BYTES: usize = 64 * 1024;

/// Where the imported bytes come from. Exactly one of these must be set.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CanvasArtifactImport {
    /// `image` | `video` | `audio` | `document`.
    pub kind: String,
    #[serde(default)]
    pub data_url: Option<String>,
    #[serde(default)]
    pub local_path: Option<String>,
    #[serde(default)]
    pub file_name: Option<String>,
    #[serde(default)]
    pub media_type: Option<String>,
    #[serde(default)]
    pub workspace_id: Option<String>,
}

/// What the frontend gets back. `path` is resolved to a displayable URL at
/// render time and must not be persisted into the graph.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CanvasArtifactDto {
    pub id: String,
    pub uri: String,
    pub kind: String,
    pub media_type: Option<String>,
    pub byte_size: i64,
    pub path: String,
    pub digest: Option<String>,
    pub created_at: i64,
}

impl CanvasArtifactDto {
    fn from_record(record: &ArtifactRecord) -> Self {
        Self {
            id: record.id.clone(),
            uri: artifact_uri(&record.id),
            kind: record.kind.as_str().to_string(),
            media_type: record.media_type.clone(),
            byte_size: record.byte_size,
            path: record.path.clone(),
            digest: record.digest.clone(),
            created_at: record.created_at,
        }
    }
}

/// `artifact://<id>` for a stored artifact.
pub fn artifact_uri(id: &str) -> String {
    format!("{ARTIFACT_URI_PREFIX}{id}")
}

/// The artifact id inside an `artifact://` URI, if this is one.
pub fn artifact_id_from_uri(value: &str) -> Option<&str> {
    value
        .strip_prefix(ARTIFACT_URI_PREFIX)
        .map(str::trim)
        .filter(|id| !id.is_empty())
}

/// True for anything a provider can consume directly (`http(s)`, `data:`).
pub fn is_remote_or_inline_reference(value: &str) -> bool {
    let value = value.trim();
    value.starts_with("data:") || value.starts_with("http://") || value.starts_with("https://")
}

fn parse_kind(label: &str) -> Result<ArtifactKind> {
    ArtifactKind::from_label(label).ok_or_else(|| {
        CoreError::invalid(format!(
            "unknown artifact kind `{label}` (expected image, video, audio or document)"
        ))
    })
}

/// `data:<mime>;base64,<payload>` → mime + bytes.
fn decode_data_url(data_url: &str) -> Result<(Option<String>, Vec<u8>)> {
    let rest = data_url
        .trim()
        .strip_prefix("data:")
        .ok_or_else(|| CoreError::invalid("expected a data: URL"))?;
    let (header, payload) = rest.split_once(',').ok_or_else(|| {
        CoreError::invalid("data URL is missing the `,` that separates header and payload")
    })?;
    let mime = header.split(';').next().filter(|value| !value.is_empty());
    if !header.to_ascii_lowercase().contains("base64") {
        return Err(CoreError::invalid("only base64 data URLs can be imported"));
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(payload.trim())
        .map_err(|error| CoreError::invalid(format!("decode data URL payload: {error}")))?;
    Ok((mime.map(str::to_string), bytes))
}

fn extension_for(media_type: Option<&str>, file_name: Option<&str>, kind: ArtifactKind) -> String {
    if let Some(name) = file_name {
        if let Some((_, ext)) = name.rsplit_once('.') {
            let cleaned: String = ext
                .chars()
                .filter(|char| char.is_ascii_alphanumeric())
                .collect();
            if !cleaned.is_empty() && cleaned.len() <= 5 {
                return format!(".{}", cleaned.to_ascii_lowercase());
            }
        }
    }
    let from_mime = match media_type.unwrap_or("").to_ascii_lowercase().as_str() {
        "image/png" => ".png",
        "image/jpeg" | "image/jpg" => ".jpg",
        "image/webp" => ".webp",
        "image/gif" => ".gif",
        "image/svg+xml" => ".svg",
        "video/mp4" => ".mp4",
        "video/webm" => ".webm",
        "video/quicktime" => ".mov",
        "audio/mpeg" | "audio/mp3" => ".mp3",
        "audio/wav" | "audio/x-wav" => ".wav",
        "audio/ogg" => ".ogg",
        "application/json" => ".json",
        _ => "",
    };
    if !from_mime.is_empty() {
        return from_mime.to_string();
    }
    match kind {
        ArtifactKind::Image => ".png",
        ArtifactKind::Video => ".mp4",
        ArtifactKind::Audio => ".wav",
        ArtifactKind::Document | ArtifactKind::ToolResult => ".bin",
    }
    .to_string()
}

/// Stores artifact bytes under a managed root and keeps the index in SQLite.
pub struct CanvasArtifactService {
    root: PathBuf,
    db: Arc<Database>,
}

impl CanvasArtifactService {
    pub fn new(root: impl AsRef<Path>, db: Arc<Database>) -> Result<Self> {
        let root = if root.as_ref().is_absolute() {
            root.as_ref().to_path_buf()
        } else {
            std::env::current_dir()
                .map_err(|error| CoreError::other(format!("resolve artifact root: {error}")))?
                .join(root)
        };
        std::fs::create_dir_all(&root)
            .map_err(|error| CoreError::Other(format!("create artifact root {root:?}: {error}")))?;
        let service = Self { root, db };
        service.recover_deletions()?;
        Ok(service)
    }

    /// Import bytes, a `data:` URL or a local file as an artifact.
    pub fn import(&self, input: &CanvasArtifactImport) -> Result<CanvasArtifactDto> {
        let kind = parse_kind(&input.kind)?;
        let (declared_mime, bytes) = match (
            input.data_url.as_deref().map(str::trim),
            input.local_path.as_deref().map(str::trim),
        ) {
            (Some(data_url), None) => decode_data_url(data_url)?,
            (None, Some(path)) => {
                let mime = guess_media_type(Path::new(path), kind);
                let file_name = input
                    .file_name
                    .as_deref()
                    .or_else(|| Path::new(path).file_name().and_then(|name| name.to_str()));
                return self.import_reader(
                    kind,
                    input
                        .media_type
                        .as_deref()
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .or(mime.as_deref()),
                    file_name,
                    File::open(path)
                        .map_err(|error| CoreError::Other(format!("open {path}: {error}")))?,
                    input.workspace_id.as_deref(),
                );
            }
            (Some(_), Some(_)) => {
                return Err(CoreError::invalid(
                    "OperationInputConflict: provide either dataUrl or localPath, not both",
                ))
            }
            (None, None) => {
                return Err(CoreError::invalid(
                    "artifact import needs dataUrl or localPath to read bytes from",
                ))
            }
        };
        let declared = input
            .media_type
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        self.import_reader(
            kind,
            declared.as_deref().or(declared_mime.as_deref()),
            input.file_name.as_deref(),
            std::io::Cursor::new(bytes),
            input.workspace_id.as_deref(),
        )
    }

    /// Store bytes the caller already holds, e.g. a provider's generated image.
    pub fn import_bytes(
        &self,
        kind: ArtifactKind,
        media_type: Option<&str>,
        bytes: &[u8],
        workspace_id: Option<&str>,
    ) -> Result<CanvasArtifactDto> {
        self.import_reader(
            kind,
            media_type,
            None,
            std::io::Cursor::new(bytes),
            workspace_id,
        )
    }

    fn import_reader(
        &self,
        kind: ArtifactKind,
        media_type: Option<&str>,
        file_name: Option<&str>,
        mut reader: impl Read,
        workspace_id: Option<&str>,
    ) -> Result<CanvasArtifactDto> {
        let mut stream = self.begin_stream(kind, media_type, file_name, workspace_id)?;
        let mut buffer = [0u8; COPY_BUFFER_BYTES];
        loop {
            let count = reader.read(&mut buffer).map_err(|error| {
                CoreError::Other(format!("read artifact input stream: {error}"))
            })?;
            if count == 0 {
                break;
            }
            stream.write_chunk(&buffer[..count])?;
        }
        stream.finish()
    }

    /// Begin a bounded-memory write into the managed artifact area. The
    /// returned stream only becomes visible in the artifact index on finish.
    pub fn begin_stream(
        &self,
        kind: ArtifactKind,
        media_type: Option<&str>,
        file_name: Option<&str>,
        workspace_id: Option<&str>,
    ) -> Result<CanvasArtifactStream> {
        let workspace_id = normalize_workspace_id(workspace_id)?;
        if kind == ArtifactKind::ToolResult {
            return Err(CoreError::invalid("tool results are not canvas media"));
        }
        let id = format!("art_{}", deepagent_core::id::EventId::new());
        ArtifactStore::validate_id(&id)?;
        let staging_path = self.root.join(format!(".{id}.part"));
        Ok(CanvasArtifactStream {
            root: self.root.clone(),
            db: self.db.clone(),
            id,
            kind,
            media_type: media_type.map(str::to_string),
            file_name: file_name.map(str::to_string),
            workspace_id,
            staging: Some(ArtifactStaging::create(staging_path)?),
        })
    }

    /// Import a local file without holding the full file contents in memory.
    pub fn import_file(
        &self,
        kind: ArtifactKind,
        media_type: Option<&str>,
        file_name: Option<&str>,
        path: impl AsRef<Path>,
        workspace_id: Option<&str>,
    ) -> Result<CanvasArtifactDto> {
        let path = path.as_ref();
        let mime = media_type
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .or_else(|| guess_media_type(path, kind));
        let name = file_name.or_else(|| path.file_name().and_then(|name| name.to_str()));
        let file = File::open(path)
            .map_err(|error| CoreError::Other(format!("open {}: {error}", path.display())))?;
        self.import_reader(kind, mime.as_deref(), name, file, workspace_id)
    }

    /// Import a native media-tool output as a video artifact.
    pub fn import_video_file(
        &self,
        media_type: &str,
        file_name: &str,
        path: impl AsRef<Path>,
        workspace_id: Option<&str>,
    ) -> Result<CanvasArtifactDto> {
        self.import_file(
            ArtifactKind::Video,
            Some(media_type),
            Some(file_name),
            path,
            workspace_id,
        )
    }

    /// Index row for an artifact id.
    pub fn record(&self, id: &str, workspace_id: Option<&str>) -> Result<Option<ArtifactRecord>> {
        ArtifactStore::validate_id(id)?;
        let workspace = normalize_workspace_id(workspace_id)?;
        let state = ArtifactStore::new(&self.db).state(id)?;
        match state {
            Some(state) if Self::in_workspace(state.record(), &workspace)? => {
                state.require_active().map(Some)
            }
            _ => Ok(None),
        }
    }

    fn in_workspace(record: &ArtifactRecord, workspace: &Option<String>) -> Result<bool> {
        Ok(record.kind != ArtifactKind::ToolResult
            && record.run_id.is_none()
            && record.call_id.is_none()
            && normalize_workspace_id(record.workspace_id.as_deref())? == *workspace)
    }

    /// Keep materialization inside the same path gate used by deletion, then
    /// re-read ownership/state. A returned path is only valid at query time;
    /// it is not a lease over a later webview/native-tool open.
    fn with_record<T>(
        &self,
        id: &str,
        workspace_id: Option<&str>,
        use_record: impl FnOnce(&ArtifactRecord) -> Result<T>,
    ) -> Result<Option<T>> {
        let Some(expected) = self.record(id, workspace_id)? else {
            return Ok(None);
        };
        let _operation = fs_commit::lock_path_operation(Path::new(&expected.path))
            .map_err(|error| CoreError::other(format!("lock artifact {id}: {error}")))?;
        let Some(record) = self.record(id, workspace_id)? else {
            return Ok(None);
        };
        if record != expected {
            return Err(CoreError::other("artifact changed during materialization"));
        }
        use_record(&record).map(Some)
    }

    fn with_required_record<T>(
        &self,
        id: &str,
        workspace_id: Option<&str>,
        use_record: impl FnOnce(&ArtifactRecord) -> Result<T>,
    ) -> Result<T> {
        self.with_record(id, workspace_id, use_record)?
            .ok_or_else(|| {
                tracing::warn!(
                    artifact_id = id,
                    "canvas artifact unavailable in requested workspace"
                );
                CoreError::invalid("unknown artifact in requested workspace")
            })
    }

    /// Validate a persisted reference as a readable video artifact.
    pub fn validate_video_reference(
        &self,
        reference: &str,
        workspace_id: Option<&str>,
    ) -> Result<()> {
        self.video_file_path(reference, workspace_id).map(|_| ())
    }

    /// Resolve a validated video artifact to its managed file for native media tools.
    pub fn video_file_path(&self, reference: &str, workspace_id: Option<&str>) -> Result<PathBuf> {
        let id = artifact_id_from_uri(reference.trim())
            .ok_or_else(|| CoreError::invalid("expected an artifact:// video reference"))?;
        self.with_required_record(id, workspace_id, |record| {
            if record.kind != ArtifactKind::Video {
                return Err(CoreError::invalid(format!(
                    "artifact `{id}` is {}, expected video",
                    record.kind.as_str()
                )));
            }
            let path = PathBuf::from(&record.path);
            File::open(&path).map_err(|error| {
                CoreError::invalid(format!("video artifact `{id}` is unreadable: {error}"))
            })?;
            Ok(path)
        })
    }

    /// Bytes behind an artifact, for sending to a provider.
    pub fn read_bytes(&self, id: &str, workspace_id: Option<&str>) -> Result<Option<Vec<u8>>> {
        self.with_record(id, workspace_id, |record| {
            std::fs::read(&record.path).map_err(|error| {
                CoreError::Other(format!("read artifact {id} from {}: {error}", record.path))
            })
        })
    }

    /// Resolve any node-stored reference into something a provider accepts:
    /// `data:` / `http(s)` pass through, `artifact://id` becomes a data URL.
    pub fn resolve_for_provider(
        &self,
        reference: &str,
        workspace_id: Option<&str>,
    ) -> Result<Option<String>> {
        normalize_workspace_id(workspace_id)?;
        let reference = reference.trim();
        if is_remote_or_inline_reference(reference) {
            return Ok(Some(reference.to_string()));
        }
        match artifact_id_from_uri(reference) {
            Some(id) => self.with_required_record(id, workspace_id, |record| {
                let bytes = std::fs::read(&record.path)
                    .map_err(|error| CoreError::Other(format!("read artifact {id}: {error}")))?;
                let mime = record
                    .media_type
                    .as_deref()
                    .unwrap_or("application/octet-stream");
                Ok(Some(format!(
                    "data:{mime};base64,{}",
                    base64::engine::general_purpose::STANDARD.encode(&bytes)
                )))
            }),
            None => Ok(None),
        }
    }

    /// Resolve a node-stored reference for display. Artifact URIs come back with
    /// the managed path the webview can load; any other scheme returns `None`,
    /// meaning the caller keeps using the value it already has.
    pub fn display_target(
        &self,
        reference: &str,
        workspace_id: Option<&str>,
    ) -> Result<Option<CanvasArtifactDto>> {
        normalize_workspace_id(workspace_id)?;
        let Some(id) = artifact_id_from_uri(reference.trim()) else {
            return Ok(None);
        };
        self.with_required_record(id, workspace_id, |record| {
            Ok(Some(CanvasArtifactDto::from_record(record)))
        })
    }

    /// Indexed artifacts for one workspace, newest first.
    pub fn list(
        &self,
        workspace_id: Option<&str>,
        kind: Option<ArtifactKind>,
    ) -> Result<Vec<ArtifactRecord>> {
        let workspace = normalize_workspace_id(workspace_id)?;
        let mut records = Vec::new();
        for record in ArtifactStore::new(&self.db).list(kind)? {
            if Self::in_workspace(&record, &workspace)? {
                records.push(record);
            }
        }
        Ok(records)
    }

    /// Commit a deletion intent before removing bytes. A committed but
    /// unfinished operation is reported as `ArtifactPendingDelete` and can be
    /// retried by the same namespace or recovered when this service reopens.
    pub fn delete(&self, id: &str, workspace_id: Option<&str>) -> Result<bool> {
        ArtifactStore::validate_id(id)?;
        let workspace = normalize_workspace_id(workspace_id)?;
        let store = ArtifactStore::new(&self.db);
        let Some(state) = store.state(id)? else {
            return Ok(false);
        };
        if !Self::in_workspace(state.record(), &workspace)? {
            return Ok(false);
        }
        store.delete_file(state.record(), |record| self.verify_deletion(record))
    }

    fn owns_deletion_path(&self, record: &ArtifactRecord) -> Result<bool> {
        let path = Path::new(&record.path);
        let Some(parent) = path.parent() else {
            return Ok(false);
        };
        if !path.is_absolute() {
            return Ok(false);
        }
        let parent = fs_commit::canonicalize_with_missing_suffix(parent)
            .map_err(|error| CoreError::other(format!("resolve artifact parent: {error}")))?;
        let root = std::fs::canonicalize(&self.root)
            .map_err(|error| CoreError::other(format!("resolve artifact root: {error}")))?;
        Ok(parent == root)
    }

    fn verify_deletion(&self, record: &ArtifactRecord) -> Result<bool> {
        if !self.owns_deletion_path(record)? || !verify_file_for_deletion(record)? {
            return Err(CoreError::invalid(
                "artifact deletion refused unverified managed file",
            ));
        }
        Ok(true)
    }

    fn recover_deletions(&self) -> Result<()> {
        let store = ArtifactStore::new(&self.db);
        for record in store.pending_deletions()? {
            if record.kind == ArtifactKind::ToolResult
                || record.run_id.is_some()
                || record.call_id.is_some()
            {
                continue;
            }
            // Multiple media roots may share the database. Recovery only owns
            // this service's configured root, never the persisted path alone.
            let recovery = (|| {
                if !self.owns_deletion_path(&record)? {
                    return Ok(false);
                }
                store.delete_file(&record, |record| self.verify_deletion(record))
            })();
            if let Err(error) = recovery {
                tracing::warn!(artifact_id=%record.id, %error, "canvas artifact deletion remains pending after recovery");
            }
        }
        Ok(())
    }
}

pub struct CanvasArtifactStream {
    root: PathBuf,
    db: Arc<Database>,
    id: String,
    kind: ArtifactKind,
    media_type: Option<String>,
    file_name: Option<String>,
    workspace_id: Option<String>,
    staging: Option<ArtifactStaging>,
}

impl CanvasArtifactStream {
    pub fn write_chunk(&mut self, bytes: &[u8]) -> Result<()> {
        self.staging
            .as_mut()
            .ok_or_else(|| CoreError::other("artifact stream is already finalized"))?
            .write(bytes)
    }

    pub fn finish(mut self) -> Result<CanvasArtifactDto> {
        let mut staging = self
            .staging
            .take()
            .ok_or_else(|| CoreError::other("artifact stream is already finalized"))?;
        let (byte_size, digest, head) = staging.finish_writing()?;
        if byte_size == 0 {
            return Err(CoreError::invalid("artifact payload is empty"));
        }
        let byte_size = i64::try_from(byte_size)
            .map_err(|_| CoreError::invalid("artifact is too large to index"))?;
        let media_type = sniff_media_type(self.kind, &head)
            .or(self.media_type)
            .or_else(|| {
                guess_media_type(
                    Path::new(self.file_name.as_deref().unwrap_or("")),
                    self.kind,
                )
            })
            .or_else(|| default_media_type(self.kind));
        let path = self.root.join(format!(
            "{}{}",
            self.id,
            extension_for(media_type.as_deref(), self.file_name.as_deref(), self.kind)
        ));
        let _operation = fs_commit::lock_path_operation(&path)
            .map_err(|error| CoreError::other(format!("lock artifact publication: {error}")))?;

        let record = ArtifactRecord {
            id: self.id,
            kind: self.kind,
            run_id: None,
            call_id: None,
            workspace_id: self.workspace_id,
            path: path.to_string_lossy().to_string(),
            digest: Some(format!("sha256:{digest}")),
            byte_size,
            media_type,
            created_at: SystemClock.now().as_millis(),
        };
        let store = ArtifactStore::new(&self.db);
        if let Some(state) = store.state(&record.id)? {
            state.require_active()?;
            return Err(CoreError::invalid("artifact identity is already published"));
        }
        // Check the reserved identity before touching its final path, including
        // the crash window after an earlier unlink but before its receipt.
        staging.publish(&path)?;
        if let Err(error) = store.put_new(&record) {
            // These bytes have never been indexed; they are not a published
            // artifact retirement. The path gate and no-clobber move ensure
            // this rollback cannot remove an older pending publication.
            if let Err(cleanup) = std::fs::remove_file(&path) {
                tracing::warn!(artifact_id=%record.id, %cleanup, "unindexed artifact cleanup failed");
            }
            return Err(error);
        }
        tracing::debug!(
            artifact_id = %record.id,
            kind = record.kind.as_str(),
            bytes = record.byte_size,
            "canvas artifact imported"
        );
        Ok(CanvasArtifactDto::from_record(&record))
    }
}

struct ArtifactStaging {
    path: PathBuf,
    file: Option<File>,
    hasher: Sha256,
    byte_size: u64,
    head: Vec<u8>,
    published: bool,
}

impl ArtifactStaging {
    fn create(path: PathBuf) -> Result<Self> {
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|error| {
                CoreError::Other(format!(
                    "create artifact staging file {}: {error}",
                    path.display()
                ))
            })?;
        Ok(Self {
            path,
            file: Some(file),
            hasher: Sha256::new(),
            byte_size: 0,
            head: Vec::with_capacity(32),
            published: false,
        })
    }

    fn write(&mut self, bytes: &[u8]) -> Result<()> {
        let file = self
            .file
            .as_mut()
            .ok_or_else(|| CoreError::other("artifact staging stream is already closed"))?;
        file.write_all(bytes).map_err(|error| {
            CoreError::Other(format!(
                "write artifact staging file {}: {error}",
                self.path.display()
            ))
        })?;
        self.byte_size = self
            .byte_size
            .checked_add(bytes.len() as u64)
            .ok_or_else(|| CoreError::invalid("artifact size overflow"))?;
        self.hasher.update(bytes);
        let needed = 32usize.saturating_sub(self.head.len());
        self.head
            .extend_from_slice(&bytes[..bytes.len().min(needed)]);
        Ok(())
    }

    fn finish_writing(&mut self) -> Result<(u64, String, Vec<u8>)> {
        let mut file = self
            .file
            .take()
            .ok_or_else(|| CoreError::other("artifact staging stream is already closed"))?;
        file.flush()
            .and_then(|()| file.sync_all())
            .map_err(|error| {
                CoreError::Other(format!(
                    "flush artifact staging file {}: {error}",
                    self.path.display()
                ))
            })?;
        let digest = format!("{:x}", std::mem::take(&mut self.hasher).finalize());
        Ok((self.byte_size, digest, self.head.clone()))
    }

    fn publish(&mut self, final_path: &Path) -> Result<()> {
        fs_commit::rename_no_replace(&self.path, final_path).map_err(|error| {
            CoreError::Other(format!(
                "publish artifact {}: {error}",
                final_path.display()
            ))
        })?;
        self.published = true;
        Ok(())
    }
}

impl Drop for ArtifactStaging {
    fn drop(&mut self) {
        self.file.take();
        if !self.published {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

fn sniff_media_type(kind: ArtifactKind, head: &[u8]) -> Option<String> {
    let mime = match kind {
        ArtifactKind::Image if head.starts_with(b"\x89PNG\r\n\x1a\n") => "image/png",
        ArtifactKind::Image if head.starts_with(&[0xff, 0xd8, 0xff]) => "image/jpeg",
        ArtifactKind::Image if head.starts_with(b"GIF8") => "image/gif",
        ArtifactKind::Video if head.len() >= 12 && &head[4..12] == b"ftypqt  " => "video/quicktime",
        ArtifactKind::Video if head.len() >= 8 && &head[4..8] == b"ftyp" => "video/mp4",
        ArtifactKind::Video if head.starts_with(b"\x1aE\xdf\xa3") => "video/webm",
        ArtifactKind::Audio if head.starts_with(b"RIFF") && head.get(8..12) == Some(b"WAVE") => {
            "audio/wav"
        }
        _ => return None,
    };
    Some(mime.to_string())
}

fn guess_media_type(path: &Path, kind: ArtifactKind) -> Option<String> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    let mime = match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "mov" => "video/quicktime",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "ogg" => "audio/ogg",
        "json" => "application/json",
        _ => return None,
    };
    Some(mime.to_string()).filter(|_| kind != ArtifactKind::ToolResult)
}

fn default_media_type(kind: ArtifactKind) -> Option<String> {
    let mime = match kind {
        ArtifactKind::Image => "image/png",
        ArtifactKind::Video => "video/mp4",
        ArtifactKind::Audio => "audio/wav",
        ArtifactKind::Document | ArtifactKind::ToolResult => "application/octet-stream",
    };
    Some(mime.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::engine::general_purpose::STANDARD;
    use serde_json::json;

    struct TempRoot {
        dir: PathBuf,
    }

    impl TempRoot {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir()
                .join(format!("deepagent-artifacts-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            Self { dir }
        }
    }

    impl Drop for TempRoot {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn service(name: &str) -> (TempRoot, CanvasArtifactService) {
        let root = TempRoot::new(name);
        let service =
            CanvasArtifactService::new(&root.dir, Arc::new(Database::open_in_memory().unwrap()))
                .expect("service");
        (root, service)
    }

    fn png_data_url() -> String {
        // The smallest real PNG header plus payload; content does not matter,
        // only that bytes round-trip and the mime is parsed out of the header.
        format!(
            "data:image/png;base64,{}",
            STANDARD.encode([0x89u8, b'P', b'N', b'G', 1, 2, 3])
        )
    }

    #[test]
    fn imports_a_data_url_and_keeps_bytes_out_of_the_index() {
        let (_root, service) = service("import");
        let imported = service
            .import(&CanvasArtifactImport {
                kind: "image".to_string(),
                data_url: Some(png_data_url()),
                local_path: None,
                file_name: None,
                media_type: None,
                workspace_id: Some("ws-1".to_string()),
            })
            .expect("import");
        assert!(imported.uri.starts_with("artifact://"));
        assert_eq!(imported.kind, "image");
        assert_eq!(imported.media_type.as_deref(), Some("image/png"));
        assert_eq!(imported.byte_size, 7);
        assert!(!imported.path.contains("base64"));
        let body = serde_json::to_string(&imported).expect("serialize");
        assert!(
            !body.contains("data:image"),
            "dto leaked inline bytes: {body}"
        );
        assert_eq!(
            service
                .list(Some("ws-1"), Some(ArtifactKind::Image))
                .expect("list")
                .len(),
            1
        );
        assert!(Path::new(&imported.path).exists());
    }

    #[test]
    fn artifact_uri_resolves_to_provider_ready_bytes() {
        let (_root, service) = service("resolve");
        let imported = service
            .import(&CanvasArtifactImport {
                kind: "image".to_string(),
                data_url: Some(png_data_url()),
                local_path: None,
                file_name: None,
                media_type: None,
                workspace_id: None,
            })
            .expect("import");
        let resolved = service
            .resolve_for_provider(&imported.uri, None)
            .expect("resolve")
            .expect("present");
        assert!(resolved.starts_with("data:image/png;base64,"));
        assert_eq!(
            service
                .read_bytes(&imported.id, None)
                .expect("read")
                .expect("bytes")
                .len(),
            7
        );
        // Remote and inline references pass through untouched, so graphs drawn
        // before artifacts existed still execute.
        assert_eq!(
            service
                .resolve_for_provider("https://cdn/x.png", None)
                .expect("remote")
                .as_deref(),
            Some("https://cdn/x.png")
        );
        assert!(service
            .resolve_for_provider("asset://localhost/x", None)
            .unwrap()
            .is_none());
        assert!(service
            .resolve_for_provider("artifact://art_missing", None)
            .expect_err("unknown id")
            .to_string()
            .contains("unknown artifact"));
    }

    #[test]
    #[ignore = "audit H-250: canvas artifact reads must verify the stored digest before provider projection"]
    fn audit_replaced_canvas_artifact_is_rejected_before_provider_projection() {
        let (_root, service) = service("digest-integrity");
        let imported = service
            .import(&CanvasArtifactImport {
                kind: "image".to_string(),
                data_url: Some(png_data_url()),
                local_path: None,
                file_name: None,
                media_type: None,
                workspace_id: Some("audit-workspace".into()),
            })
            .expect("import artifact");
        let original = service
            .read_bytes(&imported.id, Some("audit-workspace"))
            .expect("read original")
            .expect("original bytes");
        let replacement = b"\x89PNG\r\n\x1a\naudit-replacement";
        std::fs::write(&imported.path, replacement).expect("replace managed artifact bytes");

        let projected = service
            .resolve_for_provider(&imported.uri, Some("audit-workspace"))
            .expect("digest mismatch should be a structured artifact error");
        let projected = projected.expect("artifact reference should resolve");
        let (_, projected_payload) = projected
            .split_once(',')
            .expect("provider projection must be a data URL");
        let projected_bytes = STANDARD
            .decode(projected_payload)
            .expect("decode projected data");
        eprintln!(
            "canvas artifact digest audit: original_bytes={}, replacement_bytes={}, projected_bytes={}, stored_digest={:?}",
            original.len(),
            replacement.len(),
            projected_bytes.len(),
            service.record(&imported.id, Some("audit-workspace")).expect("record").and_then(|record| record.digest),
        );
        assert_ne!(
            projected_bytes, replacement,
            "a replaced canvas artifact must not be projected to the provider under the old URI"
        );
    }

    #[test]
    fn audit_canvas_artifact_operations_require_workspace_scope() {
        let (_root, service) = service("workspace-scope");
        let a = service
            .import_bytes(
                ArtifactKind::Image,
                Some("image/png"),
                b"workspace-a-image",
                Some("workspace-a"),
            )
            .unwrap();
        let b = service
            .import_bytes(
                ArtifactKind::Image,
                Some("image/png"),
                b"workspace-b-image",
                Some("workspace-b"),
            )
            .unwrap();
        let caller = Some("workspace-a");
        let read_exposes_b = service.read_bytes(&b.id, caller).unwrap().is_some();
        let provider_exposes_b = service.resolve_for_provider(&b.uri, caller).is_ok();
        let display_exposes_b = service.display_target(&b.uri, caller).is_ok();
        let deleted_b = service.delete(&b.id, caller).unwrap();
        eprintln!("canvas artifact scope audit: read_exposes_b={read_exposes_b}, provider_exposes_b={provider_exposes_b}, display_exposes_b={display_exposes_b}, deleted_other_workspace={deleted_b}");
        assert!(!read_exposes_b && !provider_exposes_b && !display_exposes_b && !deleted_b);
        assert_eq!(
            service
                .read_bytes(&b.id, Some("workspace-b"))
                .unwrap()
                .unwrap(),
            b"workspace-b-image"
        );
        assert!(service.record(&a.id, caller).unwrap().is_some());
    }

    #[test]
    fn artifacts_require_exact_namespace_for_all_materialization_and_deletion() {
        let (_root, service) = service("scope-matrix");
        let scopes = [None, Some("ws-a"), Some("ws-b")];
        let items: Vec<_> = scopes
            .iter()
            .map(|scope| {
                service
                    .import_bytes(
                        ArtifactKind::Video,
                        Some("video/mp4"),
                        b"synthetic video",
                        *scope,
                    )
                    .unwrap()
            })
            .collect();
        for (caller, scope) in scopes.iter().enumerate() {
            assert_eq!(
                service
                    .list(*scope, None)
                    .unwrap()
                    .iter()
                    .map(|r| r.id.as_str())
                    .collect::<Vec<_>>(),
                vec![items[caller].id.as_str()]
            );
            for (owner, item) in items.iter().enumerate() {
                if caller == owner {
                    assert_eq!(
                        service.read_bytes(&item.id, *scope).unwrap().unwrap(),
                        b"synthetic video"
                    );
                    assert!(service
                        .resolve_for_provider(&item.uri, *scope)
                        .unwrap()
                        .unwrap()
                        .starts_with("data:video/mp4;base64,"));
                    assert_eq!(
                        service
                            .display_target(&item.uri, *scope)
                            .unwrap()
                            .unwrap()
                            .id,
                        item.id
                    );
                    service.validate_video_reference(&item.uri, *scope).unwrap();
                } else {
                    assert!(service.record(&item.id, *scope).unwrap().is_none());
                    assert!(service.read_bytes(&item.id, *scope).unwrap().is_none());
                    for (actual, missing) in [
                        (
                            service.resolve_for_provider(&item.uri, *scope).unwrap_err(),
                            service
                                .resolve_for_provider("artifact://art_missing", *scope)
                                .unwrap_err(),
                        ),
                        (
                            service.display_target(&item.uri, *scope).unwrap_err(),
                            service
                                .display_target("artifact://art_missing", *scope)
                                .unwrap_err(),
                        ),
                        (
                            service.video_file_path(&item.uri, *scope).unwrap_err(),
                            service
                                .video_file_path("artifact://art_missing", *scope)
                                .unwrap_err(),
                        ),
                    ] {
                        assert_eq!(actual.to_string(), missing.to_string());
                    }
                    assert!(!service.delete(&item.id, *scope).unwrap());
                    assert_eq!(std::fs::read(&item.path).unwrap(), b"synthetic video");
                }
            }
        }
        for (scope, item) in scopes.iter().zip(items) {
            assert!(service.delete(&item.id, *scope).unwrap());
            assert!(!service.delete(&item.id, *scope).unwrap());
            assert!(service.record(&item.id, *scope).unwrap().is_none());
        }
    }

    #[test]
    fn legacy_default_and_normalized_owners_do_not_expose_host_tool_results() {
        let (_root, service) = service("scope-legacy");
        let default = service
            .import_bytes(ArtifactKind::Image, Some("image/png"), b"default", None)
            .unwrap();
        let scoped = service
            .import_bytes(
                ArtifactKind::Image,
                Some("image/png"),
                b"scoped",
                Some(" ws-a "),
            )
            .unwrap();
        let mut legacy = ArtifactStore::new(&service.db)
            .get(&scoped.id)
            .unwrap()
            .unwrap();
        legacy.workspace_id = Some(" ws-a ".into());
        ArtifactStore::new(&service.db).put(&legacy).unwrap();
        assert_eq!(service.list(Some("ws-a"), None).unwrap()[0].id, scoped.id);
        assert!(service
            .record(&scoped.id, Some(" ws-a "))
            .unwrap()
            .is_some());
        assert_eq!(
            ArtifactStore::new(&service.db)
                .get(&scoped.id)
                .unwrap()
                .unwrap(),
            legacy
        );
        let mut host_result = legacy.clone();
        host_result.id = "host_result".into();
        host_result.workspace_id = None;
        host_result.kind = ArtifactKind::ToolResult;
        ArtifactStore::new(&service.db).put(&host_result).unwrap();
        assert!(service.record(&host_result.id, None).unwrap().is_none());
        assert!(!service.delete(&host_result.id, None).unwrap());
        assert_eq!(service.list(None, None).unwrap()[0].id, default.id);
        assert_eq!(service.list(None, None).unwrap().len(), 1);
        for invalid in ["", " ", "bad/path", "工作区"] {
            assert!(service.record(&default.id, Some(invalid)).is_err());
            assert!(service.list(Some(invalid), None).is_err());
            assert!(service.delete(&default.id, Some(invalid)).is_err());
            assert!(service
                .import_bytes(ArtifactKind::Image, None, b"invalid", Some(invalid))
                .is_err());
        }
        assert!(service
            .import_bytes(ArtifactKind::ToolResult, None, b"host-only", None)
            .is_err());
    }

    #[test]
    fn audit_artifact_delete_is_atomic_across_file_and_index() {
        let root = TempRoot::new("delete-atomicity");
        let db = Arc::new(Database::open_in_memory().expect("database"));
        let service = CanvasArtifactService::new(&root.dir, db.clone()).expect("service");
        let imported = service
            .import(&CanvasArtifactImport {
                kind: "image".to_string(),
                data_url: Some(png_data_url()),
                local_path: None,
                file_name: None,
                media_type: None,
                workspace_id: Some("delete-workspace".into()),
            })
            .expect("import artifact");
        db.with_conn(|conn| {
            conn.execute_batch(
                "CREATE TRIGGER audit_block_artifact_delete
                 BEFORE DELETE ON artifacts
                 BEGIN SELECT RAISE(ABORT, 'audit delete blocked'); END;",
            )
            .map_err(|error| CoreError::other(error.to_string()))?;
            Ok(())
        })
        .expect("install delete failure trigger");

        let delete_result = service.delete(&imported.id, Some("delete-workspace"));
        let file_exists = Path::new(&imported.path).exists();
        let row_exists = service
            .record(&imported.id, Some("delete-workspace"))
            .expect("read failed-delete record")
            .is_some();
        eprintln!(
            "canvas artifact delete atomicity audit: delete_error={}, file_exists={}, row_exists={}",
            delete_result.is_err(),
            file_exists,
            row_exists,
        );
        assert!(
            delete_result.is_err() && file_exists && row_exists,
            "a failed artifact index deletion must preserve both the row and its bytes"
        );
    }

    #[test]
    fn display_target_resolves_artifacts_and_passes_other_schemes() {
        let (_root, service) = service("display");
        let imported = service
            .import(&CanvasArtifactImport {
                kind: "image".to_string(),
                data_url: Some(png_data_url()),
                local_path: None,
                file_name: None,
                media_type: None,
                workspace_id: None,
            })
            .expect("import");
        let target = service
            .display_target(&imported.uri, None)
            .expect("resolve")
            .expect("artifact target");
        assert_eq!(target.id, imported.id);
        assert!(Path::new(&target.path).exists());
        assert!(service
            .display_target("https://cdn.example.com/x.png", None)
            .expect("remote")
            .is_none());
        assert!(service
            .display_target("asset://localhost/C:/Users/x.png", None)
            .expect("asset")
            .is_none());
        service.delete(&imported.id, None).expect("delete");
        assert!(service
            .display_target(&imported.uri, None)
            .expect_err("dangling reference must not render blank")
            .to_string()
            .contains("unknown artifact in requested workspace"));
    }

    #[test]
    fn h254_index_failure_preserves_all_materializations_and_allows_retry() {
        let (root, service) = service("delete-materializations");
        let video = service
            .import_bytes(
                ArtifactKind::Video,
                Some("video/mp4"),
                b"video",
                Some("ws-a"),
            )
            .unwrap();
        service.db.with_conn(|conn| {
            conn.execute_batch("CREATE TRIGGER h254_reject_delete BEFORE DELETE ON artifacts BEGIN SELECT RAISE(ABORT, 'synthetic delete failure'); END;")
                .map_err(|error| CoreError::other(error.to_string()))?;
            Ok(())
        }).unwrap();
        assert!(service.delete(&video.id, Some("ws-a")).is_err());
        assert_eq!(
            service
                .read_bytes(&video.id, Some("ws-a"))
                .unwrap()
                .unwrap(),
            b"video"
        );
        assert!(service
            .resolve_for_provider(&video.uri, Some("ws-a"))
            .unwrap()
            .is_some());
        assert!(service
            .display_target(&video.uri, Some("ws-a"))
            .unwrap()
            .is_some());
        assert!(service
            .video_file_path(&video.uri, Some("ws-a"))
            .unwrap()
            .is_file());
        assert!(!service.delete(&video.id, Some("ws-b")).unwrap());
        service
            .db
            .with_conn(|conn| {
                conn.execute_batch("DROP TRIGGER h254_reject_delete;")
                    .map_err(|error| CoreError::other(error.to_string()))?;
                Ok(())
            })
            .unwrap();
        assert!(service.delete(&video.id, Some("ws-a")).unwrap());
        assert!(!service.delete(&video.id, Some("ws-a")).unwrap());
        assert!(service.record(&video.id, Some("ws-a")).unwrap().is_none());
        assert!(!Path::new(&video.path).exists());
        drop(root);
    }

    #[cfg(windows)]
    #[test]
    fn h254_file_delete_failure_is_pending_scoped_and_recoverable_on_reopen() {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_SHARE_READ: u32 = 0x0000_0001;
        let root = tempfile::tempdir().unwrap();
        let database = root.path().join("state.db");
        let media_root = root.path().join("artifacts");
        let db = Arc::new(Database::open(&database).unwrap());
        let service = CanvasArtifactService::new(&media_root, db.clone()).unwrap();
        let video = service
            .import_bytes(
                ArtifactKind::Video,
                Some("video/mp4"),
                b"locked video",
                Some("ws-a"),
            )
            .unwrap();
        // Permit ordinary reads, but deny deletion while this real OS handle is live.
        let held = OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ)
            .open(&video.path)
            .unwrap();
        let error = service
            .delete(&video.id, Some("ws-a"))
            .expect_err("file deletion failure must not be reported as success");
        assert!(error.to_string().contains("artifact_pending_delete"));
        assert_eq!(std::fs::read(&video.path).unwrap(), b"locked video");
        assert!(service.list(Some("ws-a"), None).unwrap().is_empty());
        for caller in [None, Some("ws-b")] {
            assert!(!service.delete(&video.id, caller).unwrap());
            assert!(service.record(&video.id, caller).unwrap().is_none());
            for error in [
                service
                    .resolve_for_provider(&video.uri, caller)
                    .unwrap_err(),
                service.display_target(&video.uri, caller).unwrap_err(),
                service.video_file_path(&video.uri, caller).unwrap_err(),
            ] {
                assert!(error
                    .to_string()
                    .contains("unknown artifact in requested workspace"));
                assert!(!error.to_string().contains("artifact_pending_delete"));
            }
        }
        for result in [
            service.read_bytes(&video.id, Some("ws-a")).map(|_| ()),
            service
                .resolve_for_provider(&video.uri, Some("ws-a"))
                .map(|_| ()),
            service.display_target(&video.uri, Some("ws-a")).map(|_| ()),
            service
                .video_file_path(&video.uri, Some("ws-a"))
                .map(|_| ()),
        ] {
            assert!(result
                .unwrap_err()
                .to_string()
                .contains("artifact_pending_delete"));
        }
        drop(service);
        drop(db);
        drop(held);
        let unrelated = CanvasArtifactService::new(
            root.path().join("other-media"),
            Arc::new(Database::open(&database).unwrap()),
        )
        .unwrap();
        assert!(
            Path::new(&video.path).exists(),
            "another service root cannot recover this deletion"
        );
        drop(unrelated);
        let reopened =
            CanvasArtifactService::new(&media_root, Arc::new(Database::open(&database).unwrap()))
                .unwrap();
        assert!(reopened.record(&video.id, Some("ws-a")).unwrap().is_none());
        assert!(
            !Path::new(&video.path).exists(),
            "opening the service must finish its committed deletion intent"
        );
        assert!(!reopened.delete(&video.id, Some("ws-a")).unwrap());
    }

    #[test]
    fn h254_cleanup_refuses_an_external_path_even_with_matching_identity_and_digest() {
        let (root, service) = service("delete-external-path");
        let video = service
            .import_bytes(
                ArtifactKind::Video,
                Some("video/mp4"),
                b"synthetic video",
                None,
            )
            .unwrap();
        let outside = tempfile::tempdir().unwrap();
        let outside_path = outside
            .path()
            .join(Path::new(&video.path).file_name().unwrap());
        std::fs::rename(&video.path, &outside_path).unwrap();
        let mut record = service.record(&video.id, None).unwrap().unwrap();
        record.path = outside_path.to_string_lossy().into_owned();
        ArtifactStore::new(&service.db).put(&record).unwrap();
        assert!(service.delete(&video.id, None).is_err());
        assert_eq!(std::fs::read(&outside_path).unwrap(), b"synthetic video");
        assert_eq!(service.record(&video.id, None).unwrap(), Some(record));
        assert!(ArtifactStore::new(&service.db)
            .pending_deletions()
            .unwrap()
            .is_empty());
        drop(root);
    }

    #[test]
    fn h254_materialization_and_delete_share_the_operation_gate() {
        let (_root, service) = service("read-delete-gate");
        let video = service
            .import_bytes(
                ArtifactKind::Video,
                Some("video/mp4"),
                b"synthetic video",
                None,
            )
            .unwrap();
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let (deleted_tx, deleted_rx) = std::sync::mpsc::channel();
        std::thread::scope(|threads| {
            let service = &service;
            let id = &video.id;
            let reader = threads.spawn(move || {
                service
                    .with_record(id, None, |record| {
                        entered_tx.send(()).unwrap();
                        release_rx
                            .recv_timeout(std::time::Duration::from_secs(3))
                            .unwrap();
                        std::fs::read(&record.path)
                            .map_err(|error| CoreError::other(error.to_string()))
                    })
                    .unwrap()
            });
            entered_rx
                .recv_timeout(std::time::Duration::from_secs(3))
                .unwrap();
            let deletion =
                threads.spawn(move || deleted_tx.send(service.delete(id, None)).unwrap());
            assert!(matches!(
                deleted_rx.recv_timeout(std::time::Duration::from_millis(100)),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout)
            ));
            release_tx.send(()).unwrap();
            assert_eq!(reader.join().unwrap().unwrap(), b"synthetic video");
            assert!(deleted_rx
                .recv_timeout(std::time::Duration::from_secs(3))
                .unwrap()
                .unwrap());
            deletion.join().unwrap();
        });
        assert!(service.read_bytes(&video.id, None).unwrap().is_none());
    }

    #[test]
    fn h254_stream_publication_never_overwrites_an_existing_or_pending_identity() {
        let (_root, service) = service("pending-publication");
        let video = service
            .import_bytes(
                ArtifactKind::Video,
                Some("video/mp4"),
                b"original video",
                None,
            )
            .unwrap();
        for pending in [false, true] {
            if pending {
                service
                    .db
                    .with_conn(|conn| {
                        conn.execute_batch(
                            "CREATE TRIGGER h254_receipt BEFORE DELETE ON documents
                        WHEN OLD.collection='artifact_deletions_v1'
                        BEGIN SELECT RAISE(ABORT,'synthetic receipt failure'); END;",
                        )
                        .map_err(|error| CoreError::other(error.to_string()))
                    })
                    .unwrap();
                assert!(service.delete(&video.id, None).is_err());
            }
            let mut stream = service
                .begin_stream(ArtifactKind::Video, Some("video/mp4"), None, None)
                .unwrap();
            stream.id = video.id.clone();
            stream.write_chunk(b"replacement video").unwrap();
            assert!(stream.finish().is_err());
            if pending {
                assert!(!Path::new(&video.path).exists());
                assert!(service
                    .record(&video.id, None)
                    .unwrap_err()
                    .to_string()
                    .contains("artifact_pending_delete"));
            } else {
                assert_eq!(
                    service.read_bytes(&video.id, None).unwrap().unwrap(),
                    b"original video"
                );
            }
        }
    }

    #[test]
    fn imports_a_local_file_keeping_its_extension() {
        let (root, service) = service("local");
        let source = root.dir.join("source-cat.jpg");
        let payload = b"\xff\xd8\xff\xe0fake-jpeg";
        std::fs::write(&source, payload).expect("write source");
        let imported = service
            .import(&CanvasArtifactImport {
                kind: "image".to_string(),
                data_url: None,
                local_path: Some(source.to_string_lossy().to_string()),
                file_name: None,
                media_type: None,
                workspace_id: None,
            })
            .expect("import local");
        assert_eq!(imported.media_type.as_deref(), Some("image/jpeg"));
        assert!(imported.path.ends_with(".jpg"));
        assert_eq!(imported.byte_size as usize, payload.len());
        // The artifact owns its own copy: the graph must survive the source file
        // being moved or deleted by the user.
        std::fs::remove_file(&source).expect("remove source");
        assert_eq!(
            service
                .read_bytes(&imported.id, None)
                .expect("read")
                .expect("bytes")
                .len(),
            payload.len()
        );
    }

    #[test]
    fn artifact_stream_publishes_on_finish_and_discards_unfinished_staging() {
        let (root, service) = service("stream-lifecycle");
        {
            let mut abandoned = service
                .begin_stream(
                    ArtifactKind::Video,
                    Some("video/mp4"),
                    Some("clip.mp4"),
                    None,
                )
                .expect("begin abandoned stream");
            abandoned.write_chunk(b"partial").expect("write partial");
        }
        assert_eq!(std::fs::read_dir(&root.dir).unwrap().count(), 0);

        let mut stream = service
            .begin_stream(
                ArtifactKind::Video,
                Some("application/octet-stream"),
                None,
                None,
            )
            .expect("begin stream");
        stream
            .write_chunk(b"\x00\x00\x00\x14ftypmp42")
            .expect("write header");
        stream.write_chunk(b"media").expect("write body");
        let imported = stream.finish().expect("finish stream");
        assert_eq!(imported.media_type.as_deref(), Some("video/mp4"));
        assert_eq!(imported.byte_size, 17);
        assert_eq!(
            service
                .read_bytes(&imported.id, None)
                .expect("read artifact")
                .expect("stored bytes"),
            b"\x00\x00\x00\x14ftypmp42media"
        );
        assert_eq!(std::fs::read_dir(&root.dir).unwrap().count(), 1);
    }

    #[test]
    fn delete_removes_row_and_bytes_together() {
        let (_root, service) = service("delete");
        let imported = service
            .import(&CanvasArtifactImport {
                kind: "document".to_string(),
                data_url: Some(format!(
                    "data:application/json;base64,{}",
                    STANDARD.encode(json!({"a": 1}).to_string())
                )),
                local_path: None,
                file_name: None,
                media_type: None,
                workspace_id: None,
            })
            .expect("import json");
        assert!(service.delete(&imported.id, None).expect("delete"));
        assert!(!Path::new(&imported.path).exists());
        assert!(service
            .record(&imported.id, None)
            .expect("record")
            .is_none());
        assert!(!service.delete(&imported.id, None).expect("second delete"));
    }

    #[test]
    fn rejects_invalid_sources_and_streams_beyond_the_legacy_limit() {
        let (_root, service) = service("reject");
        let conflict = service
            .import(&CanvasArtifactImport {
                kind: "image".to_string(),
                data_url: Some(png_data_url()),
                local_path: Some("C:/x.png".to_string()),
                file_name: None,
                media_type: None,
                workspace_id: None,
            })
            .expect_err("both sources");
        assert!(conflict.to_string().contains("OperationInputConflict"));

        let missing = service
            .import(&CanvasArtifactImport {
                kind: "image".to_string(),
                data_url: None,
                local_path: None,
                file_name: None,
                media_type: None,
                workspace_id: None,
            })
            .expect_err("no source");
        assert!(missing.to_string().contains("needs dataUrl or localPath"));

        let empty = service
            .import(&CanvasArtifactImport {
                kind: "image".to_string(),
                data_url: Some("data:image/png;base64,".to_string()),
                local_path: None,
                file_name: None,
                media_type: None,
                workspace_id: None,
            })
            .expect_err("empty payload");
        assert!(empty.to_string().contains("empty"));

        let bad_kind = service
            .import(&CanvasArtifactImport {
                kind: "spaceship".to_string(),
                data_url: Some(png_data_url()),
                local_path: None,
                file_name: None,
                media_type: None,
                workspace_id: None,
            })
            .expect_err("unknown kind");
        assert!(bad_kind.to_string().contains("unknown artifact kind"));

        let beyond_legacy_limit = service
            .import_reader(
                ArtifactKind::Video,
                Some("video/mp4"),
                Some("large.mp4"),
                std::io::repeat(7).take(64 * 1024 * 1024 + 1),
                None,
            )
            .expect("stream larger than the former limit");
        assert_eq!(beyond_legacy_limit.byte_size, 64 * 1024 * 1024 + 1);
        assert!(Path::new(&beyond_legacy_limit.path).exists());
    }

    #[test]
    fn stored_files_land_under_the_managed_root_only() {
        let (root, service) = service("rooted");
        let imported = service
            .import(&CanvasArtifactImport {
                kind: "image".to_string(),
                data_url: Some(png_data_url()),
                local_path: None,
                file_name: Some("../../evil.png".to_string()),
                media_type: None,
                workspace_id: None,
            })
            .expect("import");
        assert!(Path::new(&imported.path).starts_with(&root.dir));
        assert!(!imported.path.contains(".."));
    }

    #[test]
    fn video_reference_validation_checks_index_kind_and_file() {
        let (_root, service) = service("validate-video");
        let video = service
            .import_bytes(ArtifactKind::Video, Some("video/webm"), b"video", None)
            .expect("store video");
        service
            .validate_video_reference(&video.uri, None)
            .expect("stored video passes");

        let image = service
            .import_bytes(ArtifactKind::Image, Some("image/png"), b"image", None)
            .expect("store image");
        assert!(service
            .validate_video_reference(&image.uri, None)
            .expect_err("image must not pass as video")
            .to_string()
            .contains("expected video"));
        assert!(service
            .validate_video_reference("artifact://missing", None)
            .expect_err("missing artifact must fail")
            .to_string()
            .contains("unknown artifact in requested workspace"));

        std::fs::remove_file(&video.path).expect("remove video bytes");
        assert!(service
            .validate_video_reference(&video.uri, None)
            .expect_err("missing bytes must fail")
            .to_string()
            .contains("unreadable"));
    }
}
