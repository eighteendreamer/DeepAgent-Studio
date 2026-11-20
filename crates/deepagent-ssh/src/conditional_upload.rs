//! External edits publish through the same staging and verification path as upload.
use crate::{FileTransferError, FileTransferReceipt, SshService};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConditionalUploadRequest {
    pub connection_id: String,
    pub connection_generation: u64,
    pub local_path: String,
    pub remote_path: String,
    /// None explicitly expects a missing remote file (Save As).
    pub expected_sha256: Option<String>,
}
impl ConditionalUploadRequest {
    pub fn validate(&self) -> Result<(), FileTransferError> {
        crate::files::validate_absolute_path(&self.remote_path)
            .map_err(|e| FileTransferError::new("invalid", &e.to_string()))?;
        if self.connection_id.is_empty()
            || self.connection_id.len() > 256
            || self.connection_generation == 0
            || !std::path::Path::new(&self.local_path).is_absolute()
            || self
                .expected_sha256
                .as_ref()
                .is_some_and(|h| h.len() != 64 || !h.bytes().all(|b| b.is_ascii_hexdigit()))
        {
            return Err(FileTransferError::new(
                "invalid",
                "invalid conditional upload",
            ));
        }
        Ok(())
    }
}
impl SshService {
    pub async fn file_content_version(
        &self,
        id: &str,
        path: &str,
        cancel: CancellationToken,
    ) -> Result<(u64, Option<String>), FileTransferError> {
        crate::files::validate_absolute_path(path)
            .map_err(|e| FileTransferError::new("invalid", &e.to_string()))?;
        let (owner, sftp) =
            crate::file_transfer::operation(&cancel, self.inner.file_session(id)).await?;
        let result = content_hash(&sftp, path, &cancel).await;
        let _ = tokio::time::timeout(std::time::Duration::from_secs(3), sftp.close()).await;
        Ok((
            owner
                .transport_generation()
                .ok_or_else(|| FileTransferError::new("conflict", "connection unavailable"))?,
            result?,
        ))
    }
    pub async fn conditional_upload(
        &self,
        request: ConditionalUploadRequest,
        cancel: CancellationToken,
        progress: crate::file_transfer::TransferProgressCallback,
    ) -> Result<FileTransferReceipt, FileTransferError> {
        request.validate()?;
        let (owner, sftp) = crate::file_transfer::operation(
            &cancel,
            self.inner.file_session(&request.connection_id),
        )
        .await?;
        if owner.transport_generation() != Some(request.connection_generation) {
            return Err(FileTransferError::new(
                "conflict",
                "edit connection changed; reload remote version",
            ));
        }
        let transfer = crate::FileTransferRequest {
            direction: crate::FileTransferDirection::Upload,
            local_path: request.local_path.clone(),
            remote_path: request.remote_path.clone(),
            conflict: crate::FileConflictPolicy::Replace,
        };
        let result = crate::file_transfer::upload_guarded(
            &sftp,
            &transfer,
            &request.local_path.into(),
            &cancel,
            &progress,
            None,
            Some(&request.expected_sha256),
        )
        .await;
        let _ = tokio::time::timeout(std::time::Duration::from_secs(3), sftp.close()).await;
        result
    }
}
pub(super) async fn content_hash(
    sftp: &russh_sftp::client::SftpSession,
    path: &str,
    cancel: &CancellationToken,
) -> Result<Option<String>, FileTransferError> {
    crate::directory_transfer::remote_chain(
        sftp,
        path.rsplit_once('/')
            .map(|(p, _)| if p.is_empty() { "/" } else { p })
            .unwrap_or("/"),
        cancel,
    )
    .await?;
    match crate::file_transfer::remote_metadata(sftp, path).await? {
        None => Ok(None),
        Some(meta) if meta.file_type().is_file() => {
            let size = meta
                .size
                .filter(|n| *n <= 32 * 1024 * 1024)
                .ok_or_else(|| {
                    FileTransferError::new("limit", "external edit limited to 32 MiB")
                })?;
            Ok(Some(
                crate::file_transfer::digest_remote(sftp, path, size, cancel).await?,
            ))
        }
        _ => Err(FileTransferError::new(
            "conflict",
            "remote edit target is no longer a regular file",
        )),
    }
}
