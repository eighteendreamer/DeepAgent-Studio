//! Bounded KeepBoth naming shared by files and directory roots.
use crate::{file_transfer::remote_metadata, FileTransferError};
use russh_sftp::client::SftpSession;
use std::{
    future::Future,
    path::{Path, PathBuf},
    time::Duration,
};
use tokio_util::sync::CancellationToken;

const MAX_CANDIDATES: u16 = 128;

async fn available_name<F, Fut>(
    name: &str,
    directory: bool,
    cancel: &CancellationToken,
    mut occupied: F,
) -> Result<String, FileTransferError>
where
    F: FnMut(String) -> Fut,
    Fut: Future<Output = Result<bool, FileTransferError>>,
{
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    for number in 0..MAX_CANDIDATES {
        if cancel.is_cancelled() {
            return Err(FileTransferError::new(
                "cancelled",
                "name selection cancelled",
            ));
        }
        let candidate = numbered_name(name, directory, number);
        let exists = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(FileTransferError::new("cancelled", "name selection cancelled")),
            result = tokio::time::timeout_at(deadline, occupied(candidate.clone())) =>
                result.map_err(|_| FileTransferError::new("timeout", "keep-both name selection timed out"))??,
        };
        if !exists {
            return Ok(candidate);
        }
    }
    Err(FileTransferError::new(
        "limit",
        "no free keep-both name within 128 candidates",
    ))
}

fn numbered_name(name: &str, directory: bool, number: u16) -> String {
    if number == 0 {
        return name.into();
    }
    if !directory {
        if let Some((stem, extension)) = name.rsplit_once('.') {
            if !stem.is_empty() && !extension.is_empty() {
                return format!("{stem} ({number}).{extension}");
            }
        }
    }
    format!("{name} ({number})")
}

pub(super) async fn remote_target(
    sftp: &SftpSession,
    original: &str,
    directory: bool,
    cancel: &CancellationToken,
) -> Result<String, FileTransferError> {
    let (parent, name) = original
        .rsplit_once('/')
        .filter(|(_, name)| !name.is_empty())
        .ok_or_else(|| FileTransferError::new("invalid", "keep-both requires a named target"))?;
    let name = available_name(name, directory, cancel, |name| async move {
        let path = format!("{parent}/{name}");
        crate::files::validate_path(&path).map_err(|_| {
            FileTransferError::new("invalid", "keep-both target exceeds path bounds")
        })?;
        Ok(remote_metadata(sftp, &path).await?.is_some())
    })
    .await?;
    Ok(format!("{parent}/{name}"))
}

pub(super) async fn local_target(
    original: &Path,
    directory: bool,
    cancel: &CancellationToken,
) -> Result<PathBuf, FileTransferError> {
    let parent = original
        .parent()
        .filter(|_| original.is_absolute())
        .ok_or_else(|| {
            FileTransferError::new("invalid", "keep-both requires an absolute target")
        })?;
    let name = original
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            FileTransferError::new("invalid", "keep-both requires a UTF-8 target name")
        })?;
    let name = available_name(name, directory, cancel, |name| async move {
        match tokio::fs::symlink_metadata(parent.join(name)).await {
            Ok(_) => Ok(true),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(_) => Err(FileTransferError::new(
                "io",
                "cannot inspect keep-both target",
            )),
        }
    })
    .await?;
    Ok(parent.join(name))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn name_selection_is_bounded_and_precancelled_work_never_probes() {
        let mut probes = 0;
        let error = available_name("file", false, &CancellationToken::new(), |_| {
            probes += 1;
            std::future::ready(Ok(true))
        })
        .await
        .unwrap_err();
        assert_eq!(error.code, "limit");
        assert_eq!(probes, 128);
        let cancel = CancellationToken::new();
        cancel.cancel();
        let mut cancelled_probes = 0;
        let error = available_name("file", false, &cancel, |_| {
            cancelled_probes += 1;
            std::future::ready(Ok(false))
        })
        .await
        .unwrap_err();
        assert_eq!(error.code, "cancelled");
        assert_eq!(cancelled_probes, 0);
    }
}
