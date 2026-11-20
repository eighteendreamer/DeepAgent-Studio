//! Safe remote permission changes.
//!
//! Permission changes are planned and then executed against the same bounded
//! metadata snapshot.  The execution path never follows a directory symlink.

use crate::directory_walk::RemoteFileSession;
use crate::{
    DirectoryWalkLimits, FileTransferError, RemoteDirectoryEntry, RemoteFileKind,
    RemoteFileMetadata, SshService,
};
use russh_sftp::protocol::FileAttributes;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteChmodPreview {
    pub version: u8,
    pub connection_id: String,
    pub connection_generation: String,
    pub path: String,
    pub recursive: bool,
    pub mode: u32,
    pub root: RemoteFileMetadata,
    pub entries: Vec<RemoteDirectoryEntry>,
    pub total_entries: u32,
    pub total_files: u32,
    pub total_directories: u32,
    pub total_symlinks: u32,
    pub fingerprint: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RemoteChmodRequest {
    pub version: u8,
    pub connection_id: String,
    pub connection_generation: String,
    pub path: String,
    pub recursive: bool,
    pub mode: u32,
    pub fingerprint: String,
}

impl From<&RemoteChmodPreview> for RemoteChmodRequest {
    fn from(preview: &RemoteChmodPreview) -> Self {
        Self {
            version: preview.version,
            connection_id: preview.connection_id.clone(),
            connection_generation: preview.connection_generation.clone(),
            path: preview.path.clone(),
            recursive: preview.recursive,
            mode: preview.mode,
            fingerprint: preview.fingerprint.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteChmodProgress {
    pub total_entries: u32,
    pub changed_entries: u32,
    pub skipped_symlinks: u32,
    pub current_path: Option<String>,
}

pub type ChmodProgressCallback = Arc<dyn Fn(RemoteChmodProgress) + Send + Sync>;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteChmodReceipt {
    pub version: u8,
    pub connection_id: String,
    pub connection_generation: String,
    pub path: String,
    pub recursive: bool,
    pub mode: u32,
    pub total_entries: u32,
    pub changed_entries: u32,
    pub skipped_symlinks: u32,
}

impl SshService {
    pub async fn preview_chmod(
        &self,
        connection_id: &str,
        path: &str,
        mode: u32,
        recursive: bool,
        limits: DirectoryWalkLimits,
        cancel: CancellationToken,
    ) -> Result<RemoteChmodPreview, FileTransferError> {
        validate_mode(mode)?;
        limits.validate()?;
        if cancel.is_cancelled() {
            return Err(failure("cancelled", "chmod preview cancelled"));
        }
        let path = crate::files::validate_absolute_path(path)
            .map(|_| path)
            .map_err(|error| failure("invalid", &error.to_string()))?;
        if path == "/" {
            return Err(failure(
                "invalid",
                "select a path below the filesystem root",
            ));
        }
        let reader = RemoteFileSession::open(self, connection_id, &cancel).await?;
        let result = RemoteChmodPreview::read(
            &reader,
            connection_id,
            path,
            mode,
            recursive,
            limits,
            &cancel,
        )
        .await;
        reader.finish(result)
    }

    pub async fn execute_chmod(
        &self,
        request: RemoteChmodRequest,
        limits: DirectoryWalkLimits,
        cancel: CancellationToken,
        progress: ChmodProgressCallback,
    ) -> Result<RemoteChmodReceipt, FileTransferError> {
        let generation = request.validate()?;
        limits.validate()?;
        if request.version != 1
            || request.connection_id.is_empty()
            || request.fingerprint.len() != 64
            || !request
                .fingerprint
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(failure("invalid", "invalid chmod confirmation"));
        }
        let path = crate::files::validate_absolute_path(&request.path)
            .map(|_| request.path.as_str())
            .map_err(|error| failure("invalid", &error.to_string()))?;
        let reader = RemoteFileSession::open_generation(
            self,
            &request.connection_id,
            Some(generation),
            &cancel,
        )
        .await?;
        let result = async {
            let preview = RemoteChmodPreview::read(
                &reader,
                &request.connection_id,
                path,
                request.mode,
                request.recursive,
                limits,
                &cancel,
            )
            .await?;
            if preview.fingerprint != request.fingerprint {
                return Err(failure("conflict", "chmod facts changed; preview again"));
            }
            let mut receipt = RemoteChmodReceipt {
                version: 1,
                connection_id: request.connection_id.clone(),
                connection_generation: request.connection_generation.clone(),
                path: request.path.clone(),
                recursive: request.recursive,
                mode: request.mode,
                total_entries: preview.total_entries,
                changed_entries: 0,
                skipped_symlinks: 0,
            };
            progress(RemoteChmodProgress {
                total_entries: receipt.total_entries,
                changed_entries: 0,
                skipped_symlinks: 0,
                current_path: None,
            });
            let mut entries = std::iter::once(RemoteDirectoryEntry {
                relative_path: String::new(),
                metadata: preview.root,
            })
            .chain(preview.entries)
            .collect::<Vec<_>>();
            // A mode can remove traversal access; change descendants first.
            entries.sort_by(|a,b| b.metadata.path.matches('/').count().cmp(&a.metadata.path.matches('/').count()).then_with(||a.metadata.path.cmp(&b.metadata.path)));
            for entry in entries {
                if cancel.is_cancelled() {
                    return Err(failure("cancelled", "chmod cancelled"));
                }
                let expected = entry.metadata;
                let parent = parent(&expected.path);
                reader.require_chain(parent, &cancel).await?;
                progress(RemoteChmodProgress {
                    total_entries: receipt.total_entries,
                    changed_entries: receipt.changed_entries,
                    skipped_symlinks: receipt.skipped_symlinks,
                    current_path: Some(expected.path.clone()),
                });
                let current = reader.metadata(&expected.path, &cancel).await?;
                if comparison_metadata(current.clone()) != comparison_metadata(expected.clone()) {
                    return Err(failure("conflict", "chmod entry changed; preview again"));
                }
                if current.kind == RemoteFileKind::Symlink {
                    receipt.skipped_symlinks += 1;
                    continue;
                }
                let _current_permissions = current
                    .permissions
                    .ok_or_else(|| failure("invalid", "server did not provide permissions"))?;
                let mut attrs = FileAttributes::empty();
                if current.kind == RemoteFileKind::Directory { reader.require_directory(&expected.path, &cancel).await?; }
                attrs.permissions = Some(request.mode);
                crate::file_transfer::operation(&cancel, reader.protocol().setstat(&expected.path, attrs)).await
                    .map_err(|mut error| { error.cleanup_warnings.push(format!("chmod: {} acknowledged changes; this request may have changed {} before acknowledgement",receipt.changed_entries,expected.path)); error })?;
                receipt.changed_entries += 1;
                progress(RemoteChmodProgress {
                    total_entries: receipt.total_entries,
                    changed_entries: receipt.changed_entries,
                    skipped_symlinks: receipt.skipped_symlinks,
                    current_path: Some(expected.path),
                });
            }
            Ok(receipt)
        }
        .await;
        reader.finish(result)
    }
}

impl RemoteChmodPreview {
    async fn read(
        reader: &RemoteFileSession<'_>,
        connection_id: &str,
        path: &str,
        mode: u32,
        recursive: bool,
        limits: DirectoryWalkLimits,
        cancel: &CancellationToken,
    ) -> Result<Self, FileTransferError> {
        reader.require_chain(parent(path), cancel).await?;
        let root = reader.metadata(path, cancel).await?;
        if !matches!(root.kind, RemoteFileKind::File | RemoteFileKind::Directory) {
            return Err(failure(
                "invalid",
                "chmod requires a regular file or directory; selected links are not followed",
            ));
        }
        if root.permissions.is_none() {
            return Err(failure("invalid", "server did not provide permissions"));
        }
        let entries = if recursive && root.kind == RemoteFileKind::Directory {
            reader
                .walk(
                    path,
                    DirectoryWalkLimits {
                        max_entries: limits.max_entries - 1,
                        max_depth: limits.max_depth,
                    },
                    cancel,
                )
                .await?
                .entries
        } else {
            Vec::new()
        };
        reader.require_chain(parent(path), cancel).await?;
        if comparison_metadata(reader.metadata(path, cancel).await?)
            != comparison_metadata(root.clone())
        {
            return Err(failure("conflict", "chmod root changed during preview"));
        }
        if entries.iter().any(|entry| {
            !matches!(
                entry.metadata.kind,
                RemoteFileKind::File | RemoteFileKind::Directory | RemoteFileKind::Symlink
            )
        }) {
            return Err(failure(
                "unsupported",
                "chmod preview includes unknown file types",
            ));
        }
        let mut all = Vec::with_capacity(entries.len() + 1);
        all.push(comparison_metadata(root.clone()));
        all.extend(
            entries
                .iter()
                .map(|entry| comparison_metadata(entry.metadata.clone())),
        );
        let fingerprint_input = serde_json::to_vec(&(
            1_u8,
            connection_id,
            reader.connection_generation()?,
            path,
            mode,
            recursive,
            &all,
        ))
        .map_err(|error| failure("invalid", &format!("encode chmod facts: {error}")))?;
        let mut preview = Self {
            version: 1,
            connection_id: connection_id.into(),
            connection_generation: reader.connection_generation()?,
            path: path.into(),
            recursive,
            mode,
            root,
            entries,
            total_entries: 0,
            total_files: 0,
            total_directories: 0,
            total_symlinks: 0,
            fingerprint: format!("{:x}", Sha256::digest(fingerprint_input)),
        };
        for metadata in std::iter::once(&preview.root)
            .chain(preview.entries.iter().map(|entry| &entry.metadata))
        {
            preview.total_entries += 1;
            match metadata.kind {
                RemoteFileKind::File => preview.total_files += 1,
                RemoteFileKind::Directory => preview.total_directories += 1,
                RemoteFileKind::Symlink => preview.total_symlinks += 1,
                RemoteFileKind::Other | RemoteFileKind::Unknown => {}
            }
        }
        Ok(preview)
    }
}

impl RemoteChmodRequest {
    pub fn validate(&self) -> Result<u64, FileTransferError> {
        validate_mode(self.mode)?;
        crate::files::validate_absolute_path(&self.path)
            .map_err(|error| failure("invalid", &error.to_string()))?;
        if self.path == "/"
            || self.version != 1
            || self.connection_id.is_empty()
            || self.connection_id.len() > 256
            || self.connection_id.contains('\0')
            || self.fingerprint.len() != 64
            || !self
                .fingerprint
                .bytes()
                .all(|b| matches!(b,b'0'..=b'9'|b'a'..=b'f'))
        {
            return Err(failure("invalid", "invalid chmod confirmation"));
        }
        self.connection_generation
            .parse::<u64>()
            .ok()
            .filter(|value| *value != 0 && value.to_string() == self.connection_generation)
            .ok_or_else(|| failure("invalid", "invalid chmod connection generation"))
    }
}

fn parent(path: &str) -> &str {
    path.rsplit_once('/')
        .map(|(p, _)| if p.is_empty() { "/" } else { p })
        .unwrap_or("/")
}
fn comparison_metadata(mut metadata: RemoteFileMetadata) -> RemoteFileMetadata {
    metadata.accessed_ms = None;
    metadata
}

fn validate_mode(mode: u32) -> Result<(), FileTransferError> {
    if mode > 0o7777 {
        return Err(failure(
            "invalid",
            "chmod mode must be an octal permission value between 0000 and 7777",
        ));
    }
    Ok(())
}

fn failure(code: &str, message: &str) -> FileTransferError {
    FileTransferError::new(code, message)
}
