//! Remote file facts, independent of the UI and OS path implementation.
//! SFTP projection preserves absent attributes instead of inventing zeros.
use crate::{SshError, SshResult};
use russh_sftp::protocol::{FileAttributes, FileType};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteFileKind {
    File,
    Directory,
    Symlink,
    Other,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteFileMetadata {
    pub connection_id: String,
    pub path: String,
    pub followed_symlinks: bool,
    pub kind: RemoteFileKind,
    /// Decimal text is exact for all SFTP u64 sizes in JavaScript consumers.
    pub size_bytes: Option<String>,
    /// Complete Unix mode, including type bits when the server supplies them.
    pub permissions: Option<u32>,
    pub uid: Option<u32>,
    pub gid: Option<u32>,
    pub user: Option<String>,
    pub group: Option<String>,
    pub accessed_ms: Option<u64>,
    pub modified_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteRealpath {
    pub connection_id: String,
    pub path: String,
    pub canonical_path: String,
}

pub(super) fn validate_path(path: &str) -> SshResult<()> {
    // Local resource bound, not a claim about every server's filesystem limit.
    if path.is_empty() || path.contains('\0') || path.len() > 16 * 1024 {
        return Err(SshError::InvalidConfig(
            "remote path must be nonempty, NUL-free and at most 16 KiB".into(),
        ));
    }
    Ok(())
}

pub(super) fn validate_absolute_path(path: &str) -> SshResult<()> {
    validate_path(path)?;
    if !path.starts_with('/')
        || (path != "/"
            && path[1..]
                .split('/')
                .any(|part| part.is_empty() || part == "." || part == ".."))
    {
        return Err(SshError::InvalidConfig(
            "expected an absolute POSIX path without dot components".into(),
        ));
    }
    Ok(())
}

/// Prefixes of an already validated absolute POSIX directory, including root.
pub(super) fn directory_ancestors(path: &str) -> impl Iterator<Item = &str> {
    std::iter::once("/")
        .chain(
            path.match_indices('/')
                .skip(1)
                .map(|(index, _)| &path[..index]),
        )
        .chain((path != "/").then_some(path))
}

/// Join one untrusted POSIX directory entry without interpreting host OS paths.
pub(super) fn directory_entry_path(parent: &str, name: &str) -> SshResult<String> {
    validate_path(parent)?;
    validate_path(name)?;
    if name == "." || name == ".." || name.contains('/') {
        return Err(SshError::InvalidConfig(
            "unsafe remote directory entry name".into(),
        ));
    }
    let path = if parent.ends_with('/') {
        format!("{parent}{name}")
    } else {
        format!("{parent}/{name}")
    };
    validate_path(&path)?;
    Ok(path)
}

pub(super) fn metadata(
    connection_id: &str,
    path: &str,
    followed_symlinks: bool,
    attrs: FileAttributes,
) -> RemoteFileMetadata {
    let kind = if attrs.permissions.map_or(true, |mode| mode & 0o170000 == 0) {
        RemoteFileKind::Unknown
    } else {
        match attrs.file_type() {
            FileType::File => RemoteFileKind::File,
            FileType::Dir => RemoteFileKind::Directory,
            FileType::Symlink => RemoteFileKind::Symlink,
            FileType::Other => RemoteFileKind::Other,
        }
    };
    RemoteFileMetadata {
        connection_id: connection_id.into(),
        path: path.into(),
        followed_symlinks,
        kind,
        size_bytes: attrs.size.map(|size| size.to_string()),
        permissions: attrs.permissions,
        uid: attrs.uid,
        gid: attrs.gid,
        user: attrs.user,
        group: attrs.group,
        accessed_ms: attrs.atime.map(|time| u64::from(time) * 1000),
        modified_ms: attrs.mtime.map(|time| u64::from(time) * 1000),
    }
}
