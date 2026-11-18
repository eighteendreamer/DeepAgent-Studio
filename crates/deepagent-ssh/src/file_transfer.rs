//! One SFTP stream and its staging file. Queue ownership stays in AppCore.
use crate::SshService;
use russh_sftp::{
    client::{error::Error as SftpError, SftpSession},
    protocol::{FileAttributes, OpenFlags, StatusCode},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{future::Future, path::PathBuf, sync::Arc, time::Duration};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileTransferDirection {
    Upload,
    Download,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum FileConflictPolicy {
    #[default]
    Ask,
    Skip,
    Replace,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FileTransferRequest {
    pub direction: FileTransferDirection,
    pub local_path: String,
    pub remote_path: String,
    #[serde(default)]
    pub conflict: FileConflictPolicy,
}
#[derive(Debug, Clone, Copy)]
pub struct FileTransferProgress {
    pub bytes: u64,
    pub total: Option<u64>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileTransferReceipt {
    pub bytes: String,
    pub sha256: String,
    pub skipped: bool,
    pub publish_strategy: String,
    pub cleanup_warnings: Vec<String>,
}
#[derive(Debug, Clone, thiserror::Error, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[error("{code}: {message}")]
pub struct FileTransferError {
    pub code: String,
    pub message: String,
    pub cleanup_warnings: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relative_path: Option<String>,
}
impl FileTransferError {
    pub fn new(code: &str, message: &str) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            cleanup_warnings: vec![],
            relative_path: None,
        }
    }
}
type Result<T> = std::result::Result<T, FileTransferError>;
pub type TransferProgressCallback = Arc<dyn Fn(FileTransferProgress) + Send + Sync>;
fn fail(code: &str) -> FileTransferError {
    FileTransferError::new(
        code,
        match code {
            "cancelled" => "transfer stopped before publishing",
            "conflict" => "destination exists; choose a conflict policy",
            "integrity" => "source or staged content did not match verification",
            "invalid" => "expected absolute regular-file paths",
            _ => "file transfer operation failed; retry explicitly",
        },
    )
}
pub(super) async fn bounded<T, E: std::fmt::Display>(
    context: &str,
    work: impl Future<Output = std::result::Result<T, E>>,
) -> Result<T> {
    match tokio::time::timeout(Duration::from_secs(30), work).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(error)) => Err(FileTransferError::new("io", &format!("{context}: {error}"))),
        Err(_) => Err(FileTransferError::new(
            "timeout",
            &format!("{context}: timed out"),
        )),
    }
}
pub(super) async fn operation<T, E: std::fmt::Display>(
    cancel: &CancellationToken,
    work: impl Future<Output = std::result::Result<T, E>>,
) -> Result<T> {
    tokio::select! {
        biased;
        _ = cancel.cancelled() => Err(fail("cancelled")),
        result = bounded("transfer IO", work) => result,
    }
}
pub(super) async fn remote_metadata(
    sftp: &SftpSession,
    path: &str,
) -> Result<Option<FileAttributes>> {
    match tokio::time::timeout(Duration::from_secs(30), sftp.symlink_metadata(path)).await {
        Ok(Ok(meta)) => Ok(Some(meta)),
        Ok(Err(SftpError::Status(s))) if s.status_code == StatusCode::NoSuchFile => Ok(None),
        _ => Err(fail("io")),
    }
}
fn regular(meta: &FileAttributes) -> Result<u64> {
    if meta.file_type().is_file() {
        meta.size.ok_or_else(|| fail("integrity"))
    } else {
        Err(fail("invalid"))
    }
}
async fn pump<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
    reader: &mut R,
    writer: &mut W,
    total: u64,
    cancel: &CancellationToken,
    progress: &TransferProgressCallback,
) -> Result<String> {
    let mut buffer = vec![0; 64 * 1024];
    let mut bytes = 0u64;
    let mut digest = Sha256::new();
    progress(FileTransferProgress {
        bytes,
        total: Some(total),
    });
    loop {
        let count = operation(cancel, reader.read(&mut buffer)).await?;
        if count == 0 {
            break;
        }
        bytes = bytes
            .checked_add(count as u64)
            .ok_or_else(|| fail("integrity"))?;
        if bytes > total {
            return Err(fail("integrity"));
        }
        operation(cancel, writer.write_all(&buffer[..count])).await?;
        digest.update(&buffer[..count]);
        progress(FileTransferProgress {
            bytes,
            total: Some(total),
        });
    }
    if bytes != total {
        return Err(fail("integrity"));
    }
    operation(cancel, writer.flush()).await?;
    Ok(format!("{:x}", digest.finalize()))
}
async fn digest_remote(
    sftp: &SftpSession,
    path: &str,
    size: u64,
    cancel: &CancellationToken,
) -> Result<String> {
    let mut file = operation(cancel, sftp.open(path)).await?;
    let mut sink = tokio::io::sink();
    let progress: TransferProgressCallback = Arc::new(|_| {});
    let digest = pump(&mut file, &mut sink, size, cancel, &progress).await?;
    operation(cancel, file.shutdown()).await?;
    Ok(digest)
}

impl SshService {
    pub async fn transfer_file(
        &self,
        connection_id: &str,
        request: FileTransferRequest,
        cancel: CancellationToken,
        progress: TransferProgressCallback,
    ) -> Result<FileTransferReceipt> {
        let local = PathBuf::from(&request.local_path);
        crate::files::validate_path(&request.remote_path).map_err(|_| fail("invalid"))?;
        if !local.is_absolute()
            || !request.remote_path.starts_with('/')
            || request.remote_path.ends_with('/')
            || request
                .remote_path
                .rsplit('/')
                .next()
                .is_some_and(|n| n == "." || n == "..")
        {
            return Err(fail("invalid"));
        }
        let (_, sftp) = operation(&cancel, self.inner.file_session(connection_id)).await?;
        let result = match request.direction {
            FileTransferDirection::Upload => {
                upload(&sftp, &request, &local, &cancel, &progress, None).await
            }
            FileTransferDirection::Download => {
                download(&sftp, &request, &local, &cancel, &progress, None).await
            }
        };
        let _ = tokio::time::timeout(Duration::from_secs(3), sftp.close()).await;
        result
    }
}
fn skipped() -> FileTransferReceipt {
    FileTransferReceipt {
        bytes: "0".into(),
        sha256: String::new(),
        skipped: true,
        publish_strategy: "skipped".into(),
        cleanup_warnings: vec![],
    }
}

pub(super) async fn upload(
    sftp: &SftpSession,
    request: &FileTransferRequest,
    local: &PathBuf,
    cancel: &CancellationToken,
    progress: &TransferProgressCallback,
    expected_size: Option<u64>,
) -> Result<FileTransferReceipt> {
    let meta = operation(cancel, tokio::fs::symlink_metadata(local)).await?;
    if !meta.is_file() || crate::directory_plan::is_link(&meta) {
        return Err(fail("invalid"));
    }
    if expected_size.is_some_and(|size| size != meta.len()) {
        return Err(fail("integrity"));
    }
    let existing = remote_metadata(sftp, &request.remote_path).await?;
    if let Some(existing) = &existing {
        match request.conflict {
            FileConflictPolicy::Ask => return Err(fail("conflict")),
            FileConflictPolicy::Skip => return Ok(skipped()),
            FileConflictPolicy::Replace => {
                regular(existing)?;
            }
        }
    }
    let parent = request.remote_path.rsplit_once('/').unwrap().0;
    let stage = format!("{parent}/.deepagent-{}.part", uuid::Uuid::new_v4());
    if cancel.is_cancelled() {
        return Err(fail("cancelled"));
    }
    // Do not race cancellation against CREATE: the server may already own a file
    // while its acknowledgement is in flight. Await ownership, then clean up.
    let mut remote = bounded(
        "create remote staging file",
        sftp.open_with_flags_and_attributes(
            &stage,
            OpenFlags::CREATE | OpenFlags::EXCLUDE | OpenFlags::WRITE,
            FileAttributes {
                permissions: Some(0o600),
                ..FileAttributes::empty()
            },
        ),
    )
    .await
    .map_err(|mut error| {
        if error.code == "timeout" {
            error.cleanup_warnings.push(format!(
                "staging creation unconfirmed; inspect before cleanup: {stage}"
            ));
        }
        error
    })?;
    let result = async {
        let mut source = operation(cancel, tokio::fs::File::open(local)).await?;
        let digest = pump(&mut source, &mut remote, meta.len(), cancel, progress).await?;
        operation(cancel, remote.shutdown()).await?;
        let after = operation(cancel, source.metadata()).await?;
        if after.len() != meta.len() || after.modified().ok() != meta.modified().ok() {
            return Err(fail("integrity"));
        }
        if digest_remote(sftp, &stage, meta.len(), cancel).await? != digest {
            return Err(fail("integrity"));
        }
        if cancel.is_cancelled() {
            return Err(fail("cancelled"));
        }
        let (strategy, warnings) =
            publish_remote(sftp, &stage, &request.remote_path, existing.is_some()).await?;
        Ok(FileTransferReceipt {
            bytes: meta.len().to_string(),
            sha256: digest,
            skipped: false,
            publish_strategy: strategy,
            cleanup_warnings: warnings,
        })
    }
    .await;
    drop(remote);
    if let Err(mut error) = result {
        let cleanup = tokio::time::timeout(Duration::from_secs(3), sftp.remove_file(&stage)).await;
        if !matches!(cleanup, Ok(Ok(()))) {
            error
                .cleanup_warnings
                .push(format!("staging cleanup required: {stage}"));
            return Err(error);
        }
        return Err(error);
    }
    result
}

/// Publication/rollback is non-cancellable. SFTP v3 rename cannot promise atomic replacement.
async fn publish_remote(
    sftp: &SftpSession,
    stage: &str,
    target: &str,
    replace: bool,
) -> Result<(String, Vec<String>)> {
    let backup = format!("{}.deepagent-{}.backup", target, uuid::Uuid::new_v4());
    if replace {
        bounded("move original to backup", sftp.rename(target, &backup))
            .await
            .map_err(|mut error| {
                error.cleanup_warnings.push(format!(
                    "backup move not confirmed; inspect target {target} and backup {backup}"
                ));
                error.code = "publish".into();
                error
            })?;
    }
    let publish = tokio::time::timeout(Duration::from_secs(30), sftp.rename(stage, target)).await;
    if !matches!(publish, Ok(Ok(()))) {
        let mut error = fail("publish");
        error.cleanup_warnings.push(format!(
            "publication failed or unconfirmed; inspect destination: {target}"
        ));
        if replace
            && !matches!(
                tokio::time::timeout(Duration::from_secs(3), sftp.rename(&backup, target)).await,
                Ok(Ok(()))
            )
        {
            error.cleanup_warnings.push(format!(
                "original preserved at backup; manual recovery required: {backup}"
            ));
        }
        return Err(error);
    }
    let mut warnings = vec![];
    if replace
        && !matches!(
            tokio::time::timeout(Duration::from_secs(3), sftp.remove_file(&backup)).await,
            Ok(Ok(()))
        )
    {
        warnings.push(format!("published; backup cleanup required: {backup}"));
    }
    Ok((
        if replace { "backup_replace" } else { "created" }.into(),
        warnings,
    ))
}

pub(super) async fn download(
    sftp: &SftpSession,
    request: &FileTransferRequest,
    local: &PathBuf,
    cancel: &CancellationToken,
    progress: &TransferProgressCallback,
    expected_size: Option<u64>,
) -> Result<FileTransferReceipt> {
    let meta = remote_metadata(sftp, &request.remote_path)
        .await?
        .ok_or_else(|| fail("not_found"))?;
    let total = regular(&meta)?;
    if expected_size.is_some_and(|size| size != total) {
        return Err(fail("integrity"));
    }
    match tokio::fs::symlink_metadata(local).await {
        Ok(existing) => {
            if !existing.is_file() || crate::directory_plan::is_link(&existing) {
                return Err(fail("invalid"));
            }
            match request.conflict {
                FileConflictPolicy::Ask => return Err(fail("conflict")),
                FileConflictPolicy::Skip => return Ok(skipped()),
                FileConflictPolicy::Replace => {}
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err(fail("io")),
    }
    let parent = local.parent().ok_or_else(|| fail("invalid"))?.to_owned();
    let stage = tokio::task::spawn_blocking(move || {
        tempfile::Builder::new()
            .prefix(".deepagent-")
            .tempfile_in(parent)
    })
    .await
    .map_err(|_| fail("io"))?
    .map_err(|_| fail("io"))?;
    let mut destination =
        tokio::fs::File::from_std(stage.as_file().try_clone().map_err(|_| fail("io"))?);
    let mut source = operation(cancel, sftp.open(&request.remote_path)).await?;
    let digest = pump(&mut source, &mut destination, total, cancel, progress).await?;
    operation(cancel, source.shutdown()).await?;
    operation(cancel, destination.sync_all()).await?;
    drop(destination);
    let after = remote_metadata(sftp, &request.remote_path)
        .await?
        .ok_or_else(|| fail("integrity"))?;
    // Metadata alone cannot detect same-size writes within coarse mtime precision.
    // Re-read through SFTP, without remote exec or a server-specific hash extension.
    if digest_remote(sftp, &request.remote_path, total, cancel).await? != digest {
        return Err(fail("integrity"));
    }
    let verified = remote_metadata(sftp, &request.remote_path)
        .await?
        .ok_or_else(|| fail("integrity"))?;
    if after.size != meta.size
        || after.mtime != meta.mtime
        || verified.size != meta.size
        || verified.mtime != meta.mtime
    {
        return Err(fail("integrity"));
    }
    if cancel.is_cancelled() {
        return Err(fail("cancelled"));
    }
    let target = local.clone();
    let replace = request.conflict == FileConflictPolicy::Replace;
    tokio::task::spawn_blocking(move || {
        let result = if replace {
            stage.persist(target)
        } else {
            stage.persist_noclobber(target)
        };
        match result {
            Ok(_) => Ok(()),
            Err(error) => {
                let code = if error.error.kind() == std::io::ErrorKind::AlreadyExists {
                    "conflict"
                } else {
                    "publish"
                };
                let mut failure =
                    FileTransferError::new(code, &format!("publish local file: {}", error.error));
                let path = error.file.path().display().to_string();
                if let Err(cleanup) = error.file.close() {
                    failure
                        .cleanup_warnings
                        .push(format!("local staging cleanup required: {path}: {cleanup}"));
                }
                Err(failure)
            }
        }
    })
    .await
    .map_err(|_| fail("publish"))??;
    Ok(FileTransferReceipt {
        bytes: total.to_string(),
        sha256: digest,
        skipped: false,
        publish_strategy: if replace { "atomic_replace" } else { "created" }.into(),
        cleanup_warnings: vec![],
    })
}
