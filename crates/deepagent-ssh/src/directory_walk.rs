//! Bounded, read-only directory facts for transfer planning in the SSH adapter.
use crate::file_transfer::operation;
use crate::{FileTransferError, RemoteFileKind, RemoteFileMetadata, SshService};
use russh_sftp::{
    client::{error::Error as SftpError, RawSftpSession},
    protocol::StatusCode,
};
use serde::{Deserialize, Serialize};
use std::{collections::HashSet, time::Duration};
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, Copy)]
pub struct DirectoryWalkLimits {
    pub max_entries: u32,
    pub max_depth: u16,
}

impl Default for DirectoryWalkLimits {
    fn default() -> Self {
        Self {
            max_entries: 10_000,
            max_depth: 64,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteDirectoryEntry {
    pub relative_path: String,
    pub metadata: RemoteFileMetadata,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteDirectoryTree {
    pub root: String,
    pub entries: Vec<RemoteDirectoryEntry>,
}

impl SshService {
    pub async fn walk_directory(
        &self,
        connection_id: &str,
        root: &str,
        limits: DirectoryWalkLimits,
        cancel: CancellationToken,
    ) -> Result<RemoteDirectoryTree, FileTransferError> {
        if cancel.is_cancelled() {
            return Err(failure("cancelled", "directory planning cancelled"));
        }
        if root.is_empty() {
            return Err(failure("invalid", "selected directory path is empty"));
        }
        let root = root.trim_end_matches('/');
        let root = if root.is_empty() { "/" } else { root };
        validate_root(root)?;
        if limits.max_entries == 0 || limits.max_entries > 100_000 || limits.max_depth > 128 {
            return Err(failure("invalid", "directory limits exceed local bounds"));
        }
        let (owner, client) = operation(&cancel, self.inner.file_client(connection_id)).await?;
        let channel = operation(&cancel, client.get_channel()).await?;
        if let Err(error) = operation(&cancel, channel.request_subsystem(true, "sftp")).await {
            let _ = channel.close().await;
            return Err(error);
        }
        let session = RawSftpSession::new(crate::sftp_packet_stream::packet_stream(
            channel.into_stream(),
            crate::sftp_packet_stream::MAX_RECEIVE_PACKET_BYTES,
        ));
        let result = async {
            operation(&cancel, session.init()).await?;
            let attrs = operation(&cancel, session.lstat(root)).await?.attrs;
            if attrs.file_type() != russh_sftp::protocol::FileType::Dir {
                return Err(failure(
                    "invalid",
                    "selected root must be a directory, without following its final link",
                ));
            }
            let root = canonical(&session, root, &cancel).await?;
            let entries = Walker {
                session: &session,
                cancel: &cancel,
                connection_id,
                root: &root,
                limits,
                entries: Vec::new(),
                seen: HashSet::new(),
                metadata_bytes: 0,
            }
            .enumerate()
            .await?;
            Ok(RemoteDirectoryTree { root, entries })
        }
        .await;
        if let Err(error) = session.close_session() {
            return match result {
                Ok(_) => Err(failure("io", &format!("close directory channel: {error}"))),
                Err(mut failure) => {
                    failure
                        .cleanup_warnings
                        .push(format!("close directory channel: {error}"));
                    Err(failure)
                }
            };
        }
        if result.is_ok() {
            owner.touch_keepalive();
        }
        result
    }
}

fn failure(code: &str, message: &str) -> FileTransferError {
    FileTransferError::new(code, message)
}

fn validate_root(root: &str) -> Result<(), FileTransferError> {
    crate::files::validate_path(root)
        .map_err(|_| failure("invalid", "invalid remote directory path"))?;
    if !root.starts_with('/')
        || (root != "/"
            && root[1..]
                .split('/')
                .any(|part| part.is_empty() || part == "." || part == ".."))
    {
        return Err(failure(
            "invalid",
            "expected an absolute POSIX path without dot components",
        ));
    }
    Ok(())
}

async fn canonical(
    session: &RawSftpSession,
    path: &str,
    cancel: &CancellationToken,
) -> Result<String, FileTransferError> {
    let response = operation(cancel, session.realpath(path)).await?;
    if response.files.len() != 1 {
        return Err(failure("invalid", "expected one canonical directory path"));
    }
    let path = response.files.into_iter().next().unwrap().filename;
    validate_root(&path)?;
    Ok(path)
}

struct Walker<'a> {
    session: &'a RawSftpSession,
    cancel: &'a CancellationToken,
    connection_id: &'a str,
    root: &'a str,
    limits: DirectoryWalkLimits,
    entries: Vec<RemoteDirectoryEntry>,
    seen: HashSet<String>,
    metadata_bytes: usize,
}

impl Walker<'_> {
    async fn enumerate(mut self) -> Result<Vec<RemoteDirectoryEntry>, FileTransferError> {
        let mut pending = vec![(self.root.to_string(), 0)];
        while let Some((path, depth)) = pending.pop() {
            if canonical(self.session, &path, self.cancel).await? != path {
                return Err(failure(
                    "invalid",
                    "directory identity changed during planning",
                ));
            }
            // Recheck type before OPENDIR rather than trusting READDIR attributes.
            if operation(self.cancel, self.session.lstat(path.as_str()))
                .await?
                .attrs
                .file_type()
                != russh_sftp::protocol::FileType::Dir
            {
                return Err(failure("invalid", "directory type changed during planning"));
            }
            self.read(&path, depth, &mut pending).await?;
        }
        self.entries
            .sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
        Ok(self.entries)
    }

    async fn read(
        &mut self,
        path: &str,
        depth: u16,
        pending: &mut Vec<(String, u16)>,
    ) -> Result<(), FileTransferError> {
        let handle = operation(self.cancel, self.session.opendir(path))
            .await?
            .handle;
        let result = self.read_pages(path, depth, &handle, pending).await;
        let cleanup =
            tokio::time::timeout(Duration::from_millis(500), self.session.close(handle)).await;
        let warning = match cleanup {
            Ok(Ok(_)) => None,
            Ok(Err(error)) => Some(format!("close directory handle: {error}")),
            Err(_) => Some("close directory handle timed out; channel retired".into()),
        };
        match (result, warning) {
            (Ok(()), None) => Ok(()),
            (Ok(()), Some(warning)) => Err(failure("io", &warning)),
            (Err(mut error), warning) => {
                error.cleanup_warnings.extend(warning);
                Err(error)
            }
        }
    }

    async fn read_pages(
        &mut self,
        path: &str,
        depth: u16,
        handle: &str,
        pending: &mut Vec<(String, u16)>,
    ) -> Result<(), FileTransferError> {
        let mut pages = 0;
        loop {
            // Each successful page must make bounded progress, including dot entries.
            pages += 1;
            if pages > self.limits.max_entries + 3 {
                return Err(failure("limit", "directory page limit reached"));
            }
            // Preserve EOF as protocol control, not a generic IO error.
            let page = operation(self.cancel, async {
                Ok::<_, std::convert::Infallible>(self.session.readdir(handle).await)
            })
            .await?;
            let page = match page {
                Ok(page) if !page.files.is_empty() => page,
                Err(SftpError::Status(status)) if status.status_code == StatusCode::Eof => {
                    return Ok(())
                }
                Ok(_) => {
                    return Err(failure(
                        "invalid",
                        "server returned an empty directory page without EOF",
                    ))
                }
                Err(error) => return Err(failure("io", &format!("read directory page: {error}"))),
            };
            for item in page.files {
                if item.filename == "." || item.filename == ".." {
                    continue;
                }
                if item.filename.is_empty()
                    || item.filename.contains('/')
                    || item.filename.contains('\0')
                {
                    return Err(failure("invalid", "unsafe remote directory entry name"));
                }
                if self.entries.len() >= self.limits.max_entries as usize {
                    return Err(failure("limit", "directory entry limit reached"));
                }
                let full = format!("{}/{}", path.trim_end_matches('/'), item.filename);
                validate_root(&full)?;
                if !self.seen.insert(full.clone()) {
                    return Err(failure("invalid", "duplicate directory entry"));
                }
                let attrs = operation(self.cancel, self.session.lstat(full.as_str()))
                    .await?
                    .attrs;
                let metadata = crate::files::metadata(self.connection_id, &full, false, attrs);
                let relative_path = full
                    .strip_prefix(self.root.trim_end_matches('/'))
                    .and_then(|suffix| suffix.strip_prefix('/'))
                    .ok_or_else(|| failure("invalid", "directory entry escaped selected root"))?
                    .to_string();
                self.metadata_bytes += full.len()
                    + relative_path.len()
                    + metadata.user.as_ref().map_or(0, String::len)
                    + metadata.group.as_ref().map_or(0, String::len)
                    + 512;
                if self.metadata_bytes > 8 * 1024 * 1024 {
                    return Err(failure(
                        "limit",
                        "directory metadata exceeds 8 MiB local budget",
                    ));
                }
                if metadata.kind == RemoteFileKind::Directory {
                    if depth >= self.limits.max_depth {
                        return Err(failure("limit", "directory depth limit reached"));
                    }
                    pending.push((full, depth + 1));
                }
                self.entries.push(RemoteDirectoryEntry {
                    relative_path,
                    metadata,
                });
            }
        }
    }
}
