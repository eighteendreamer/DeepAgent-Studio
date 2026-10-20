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

use base64::Engine;
use deepagent_core::clock::{Clock, SystemClock};
use deepagent_core::error::{CoreError, Result};
use deepagent_persistence::artifact_store::{ArtifactKind, ArtifactRecord, ArtifactStore};
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
        let root = root.as_ref().to_path_buf();
        std::fs::create_dir_all(&root)
            .map_err(|error| CoreError::Other(format!("create artifact root {root:?}: {error}")))?;
        Ok(Self { root, db })
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
            workspace_id: workspace_id.map(str::to_string),
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
    ) -> Result<CanvasArtifactDto> {
        self.import_file(
            ArtifactKind::Video,
            Some(media_type),
            Some(file_name),
            path,
            None,
        )
    }

    /// Index row for an artifact id.
    pub fn record(&self, id: &str) -> Result<Option<ArtifactRecord>> {
        ArtifactStore::validate_id(id)?;
        ArtifactStore::new(&self.db).get(id)
    }

    /// Validate a persisted reference as a readable video artifact.
    pub fn validate_video_reference(&self, reference: &str) -> Result<()> {
        self.video_file_path(reference).map(|_| ())
    }

    /// Resolve a validated video artifact to its managed file for native media tools.
    pub fn video_file_path(&self, reference: &str) -> Result<PathBuf> {
        let id = artifact_id_from_uri(reference.trim())
            .ok_or_else(|| CoreError::invalid("expected an artifact:// video reference"))?;
        let record = self
            .record(id)?
            .ok_or_else(|| CoreError::invalid(format!("unknown video artifact `{id}`")))?;
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
    }

    /// Bytes behind an artifact, for sending to a provider.
    pub fn read_bytes(&self, id: &str) -> Result<Option<Vec<u8>>> {
        let Some(record) = self.record(id)? else {
            return Ok(None);
        };
        let bytes = std::fs::read(&record.path).map_err(|error| {
            CoreError::Other(format!("read artifact {id} from {}: {error}", record.path))
        })?;
        Ok(Some(bytes))
    }

    /// Resolve any node-stored reference into something a provider accepts:
    /// `data:` / `http(s)` pass through, `artifact://id` becomes a data URL.
    pub fn resolve_for_provider(&self, reference: &str) -> Result<Option<String>> {
        let reference = reference.trim();
        if is_remote_or_inline_reference(reference) {
            return Ok(Some(reference.to_string()));
        }
        match artifact_id_from_uri(reference) {
            Some(id) => {
                let record = self
                    .record(id)?
                    .ok_or_else(|| CoreError::invalid(format!("unknown artifact `{id}`")))?;
                let bytes = self.read_bytes(id)?.unwrap_or_default();
                let mime = record
                    .media_type
                    .unwrap_or_else(|| "application/octet-stream".into());
                Ok(Some(format!(
                    "data:{mime};base64,{}",
                    base64::engine::general_purpose::STANDARD.encode(&bytes)
                )))
            }
            None => Ok(None),
        }
    }

    /// Resolve a node-stored reference for display. Artifact URIs come back with
    /// the managed path the webview can load; any other scheme returns `None`,
    /// meaning the caller keeps using the value it already has.
    pub fn display_target(&self, reference: &str) -> Result<Option<CanvasArtifactDto>> {
        let Some(id) = artifact_id_from_uri(reference.trim()) else {
            return Ok(None);
        };
        let record = self.record(id)?.ok_or_else(|| {
            CoreError::invalid(format!(
                "artifact `{id}` is referenced by the canvas but missing from the store"
            ))
        })?;
        Ok(Some(CanvasArtifactDto::from_record(&record)))
    }

    /// Indexed artifacts for one workspace, newest first.
    pub fn list(
        &self,
        workspace_id: &str,
        kind: Option<ArtifactKind>,
    ) -> Result<Vec<ArtifactRecord>> {
        ArtifactStore::new(&self.db).list_for_workspace(workspace_id, kind)
    }

    /// Drop the index row and the bytes together.
    pub fn delete(&self, id: &str) -> Result<bool> {
        let removed = self.record(id)?.is_some_and(|record| {
            if let Err(error) = std::fs::remove_file(&record.path) {
                if error.kind() != std::io::ErrorKind::NotFound {
                    tracing::warn!(
                        artifact_id = id,
                        error = %error,
                        "artifact bytes could not be removed"
                    );
                }
            }
            true
        });
        if !removed {
            return Ok(false);
        }
        ArtifactStore::new(&self.db).delete(id)
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
        staging.publish(&path)?;

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
        if let Err(error) = ArtifactStore::new(&self.db).put(&record) {
            let _ = std::fs::remove_file(&path);
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
        std::fs::rename(&self.path, final_path).map_err(|error| {
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
                .list("ws-1", Some(ArtifactKind::Image))
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
            .resolve_for_provider(&imported.uri)
            .expect("resolve")
            .expect("present");
        assert!(resolved.starts_with("data:image/png;base64,"));
        assert_eq!(
            service
                .read_bytes(&imported.id)
                .expect("read")
                .expect("bytes")
                .len(),
            7
        );
        // Remote and inline references pass through untouched, so graphs drawn
        // before artifacts existed still execute.
        assert_eq!(
            service
                .resolve_for_provider("https://cdn/x.png")
                .expect("remote")
                .as_deref(),
            Some("https://cdn/x.png")
        );
        assert!(service
            .resolve_for_provider("asset://localhost/x")
            .unwrap()
            .is_none());
        assert!(service
            .resolve_for_provider("artifact://art_missing")
            .expect_err("unknown id")
            .to_string()
            .contains("unknown artifact"));
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
            .display_target(&imported.uri)
            .expect("resolve")
            .expect("artifact target");
        assert_eq!(target.id, imported.id);
        assert!(Path::new(&target.path).exists());
        assert!(service
            .display_target("https://cdn.example.com/x.png")
            .expect("remote")
            .is_none());
        assert!(service
            .display_target("asset://localhost/C:/Users/x.png")
            .expect("asset")
            .is_none());
        service.delete(&imported.id).expect("delete");
        assert!(service
            .display_target(&imported.uri)
            .expect_err("dangling reference must not render blank")
            .to_string()
            .contains("missing from the store"));
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
                .read_bytes(&imported.id)
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
                .read_bytes(&imported.id)
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
        assert!(service.delete(&imported.id).expect("delete"));
        assert!(!Path::new(&imported.path).exists());
        assert!(service.record(&imported.id).expect("record").is_none());
        assert!(!service.delete(&imported.id).expect("second delete"));
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
            .validate_video_reference(&video.uri)
            .expect("stored video passes");

        let image = service
            .import_bytes(ArtifactKind::Image, Some("image/png"), b"image", None)
            .expect("store image");
        assert!(service
            .validate_video_reference(&image.uri)
            .expect_err("image must not pass as video")
            .to_string()
            .contains("expected video"));
        assert!(service
            .validate_video_reference("artifact://missing")
            .expect_err("missing artifact must fail")
            .to_string()
            .contains("unknown video artifact"));

        std::fs::remove_file(&video.path).expect("remove video bytes");
        assert!(service
            .validate_video_reference(&video.uri)
            .expect_err("missing bytes must fail")
            .to_string()
            .contains("unreadable"));
    }
}
