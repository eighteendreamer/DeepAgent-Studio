//! Read-only directory transfer planning. Execution must revalidate these facts.
use crate::directory_walk::{validate_root, MAX_METADATA_BYTES};
use crate::file_transfer::operation;
use crate::{
    DirectoryWalkLimits, FileTransferDirection, FileTransferError, FileTransferRequest,
    RemoteFileKind, SshService,
};
use serde::{Deserialize, Serialize};
use std::{
    fs::Metadata,
    path::{Path, PathBuf},
};
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectoryTransferEntry {
    pub relative_path: String,
    pub kind: RemoteFileKind,
    pub size_bytes: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectoryTransferPlan {
    pub direction: FileTransferDirection,
    pub local_root: String,
    pub remote_root: String,
    pub entries: Vec<DirectoryTransferEntry>,
    pub total_bytes: String,
    pub total_files: u32,
}

impl SshService {
    pub async fn plan_directory_transfer(
        &self,
        connection_id: &str,
        request: FileTransferRequest,
        limits: DirectoryWalkLimits,
        cancel: CancellationToken,
    ) -> Result<DirectoryTransferPlan, FileTransferError> {
        if cancel.is_cancelled() {
            return Err(failure("cancelled", "directory planning cancelled"));
        }
        limits.validate()?;
        validate_root(&request.remote_path)?;
        let local = local_root(&request.local_path, request.direction, &cancel).await?;
        let mut builder = PlanBuilder {
            limits,
            entries: Vec::new(),
            bytes: 0,
            files: 0,
            metadata_bytes: 0,
        };
        let remote_root = match request.direction {
            FileTransferDirection::Upload => {
                builder.read_local(&local, &cancel).await?;
                request.remote_path
            }
            FileTransferDirection::Download => {
                let tree = self
                    .walk_directory(connection_id, &request.remote_path, limits, cancel)
                    .await?;
                for item in tree.entries {
                    builder.push(
                        item.relative_path,
                        item.metadata.kind,
                        item.metadata.size_bytes,
                    )?;
                }
                validate_download_names(&builder.entries)?;
                tree.root
            }
        };
        for entry in &builder.entries {
            validate_root(&format!(
                "{}/{}",
                remote_root.trim_end_matches('/'),
                entry.relative_path
            ))
            .map_err(|_| {
                failure(
                    "invalid",
                    &format!(
                        "remote destination exceeds path bounds: {}",
                        entry.relative_path
                    ),
                )
            })?;
        }
        builder
            .entries
            .sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
        Ok(DirectoryTransferPlan {
            direction: request.direction,
            local_root: path_text(&local)?.into(),
            remote_root,
            entries: builder.entries,
            total_bytes: builder.bytes.to_string(),
            total_files: builder.files,
        })
    }
}

fn failure(code: &str, message: &str) -> FileTransferError {
    FileTransferError::new(code, message)
}

fn path_text(path: &Path) -> Result<&str, FileTransferError> {
    path.to_str()
        .ok_or_else(|| failure("invalid", "local path is not representable as UTF-8"))
}

pub(super) fn is_link(metadata: &Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        // FILE_ATTRIBUTE_REPARSE_POINT includes junctions, not just symlinks.
        metadata.file_attributes()
            & windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT
            != 0
    }
    #[cfg(not(windows))]
    {
        metadata.is_symlink()
    }
}

async fn local_root(
    path: &str,
    direction: FileTransferDirection,
    cancel: &CancellationToken,
) -> Result<PathBuf, FileTransferError> {
    let path = Path::new(path);
    if !path.is_absolute() || path.file_name().is_none() {
        return Err(failure(
            "invalid",
            "select an absolute named local directory",
        ));
    }
    #[cfg(windows)]
    validate_windows_name(
        path.file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| failure("invalid", "selected local directory name is not UTF-8"))?,
    )?;
    let metadata = operation(cancel, async {
        match tokio::fs::symlink_metadata(path).await {
            Ok(metadata) => Ok(Some(metadata)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    })
    .await?;
    match metadata {
        Some(metadata) if is_link(&metadata) || !metadata.is_dir() => Err(failure(
            "invalid",
            "selected local root must be a regular directory, without links or reparse points",
        )),
        Some(_) => operation(cancel, tokio::fs::canonicalize(path)).await,
        None if direction == FileTransferDirection::Download => {
            let parent = operation(cancel, tokio::fs::canonicalize(path.parent().unwrap())).await?;
            Ok(parent.join(path.file_name().unwrap()))
        }
        None => Err(failure(
            "not_found",
            "selected upload directory does not exist",
        )),
    }
}

struct PlanBuilder {
    limits: DirectoryWalkLimits,
    entries: Vec<DirectoryTransferEntry>,
    bytes: u64,
    files: u32,
    metadata_bytes: usize,
}

impl PlanBuilder {
    fn push(
        &mut self,
        relative_path: String,
        kind: RemoteFileKind,
        size: Option<String>,
    ) -> Result<(), FileTransferError> {
        if self.entries.len() >= self.limits.max_entries as usize {
            return Err(failure("limit", "directory plan entry limit reached"));
        }
        if !matches!(kind, RemoteFileKind::Directory | RemoteFileKind::File) {
            return Err(failure(
                "invalid",
                &format!("unsupported directory entry (links are not followed): {relative_path}"),
            ));
        }
        if kind == RemoteFileKind::File {
            let size = size
                .as_deref()
                .and_then(|s| s.parse::<u64>().ok())
                .ok_or_else(|| {
                    failure(
                        "integrity",
                        &format!("file size is unavailable: {relative_path}"),
                    )
                })?;
            self.bytes = self.bytes.checked_add(size).ok_or_else(|| {
                failure(
                    "limit",
                    &format!("directory byte total exceeds u64: {relative_path}"),
                )
            })?;
            self.files += 1;
        }
        self.metadata_bytes += relative_path.len() * 2 + 512;
        if self.metadata_bytes > MAX_METADATA_BYTES {
            return Err(failure(
                "limit",
                "directory plan metadata exceeds 8 MiB local budget",
            ));
        }
        self.entries.push(DirectoryTransferEntry {
            relative_path,
            kind,
            size_bytes: size,
        });
        Ok(())
    }

    async fn read_local(
        &mut self,
        root: &Path,
        cancel: &CancellationToken,
    ) -> Result<(), FileTransferError> {
        for (relative, metadata) in local_entries(root, self.limits, cancel).await? {
            self.push(
                relative,
                local_kind(&metadata),
                metadata.is_file().then(|| metadata.len().to_string()),
            )?;
        }
        Ok(())
    }
}

pub(super) fn local_kind(metadata: &Metadata) -> RemoteFileKind {
    if is_link(metadata) {
        RemoteFileKind::Symlink
    } else if metadata.is_dir() {
        RemoteFileKind::Directory
    } else if metadata.is_file() {
        RemoteFileKind::File
    } else {
        RemoteFileKind::Other
    }
}

/// One bounded, non-following local traversal for planning and publication cleanup.
pub(super) async fn local_entries(
    root: &Path,
    limits: DirectoryWalkLimits,
    cancel: &CancellationToken,
) -> Result<Vec<(String, Metadata)>, FileTransferError> {
    limits.validate()?;
    let mut entries = Vec::new();
    let mut metadata_bytes = 0;
    let mut pending = vec![(root.to_owned(), String::new(), 0)];
    while let Some((path, relative, depth)) = pending.pop() {
        let metadata = operation(cancel, tokio::fs::symlink_metadata(&path)).await?;
        if is_link(&metadata)
            || !metadata.is_dir()
            || operation(cancel, tokio::fs::canonicalize(&path)).await? != path
        {
            return Err(failure(
                "invalid",
                &format!("local directory identity changed: {relative}"),
            ));
        }
        let mut directory = operation(cancel, tokio::fs::read_dir(&path)).await?;
        while let Some(item) = operation(cancel, directory.next_entry()).await? {
            let name = item.file_name();
            let name = name
                .to_str()
                .ok_or_else(|| failure("invalid", "local entry name is not UTF-8"))?;
            if name.contains('/') || name.contains('\0') || name.is_empty() {
                return Err(failure(
                    "invalid",
                    "local name cannot be represented as a POSIX component",
                ));
            }
            let relative_path = if relative.is_empty() {
                name.into()
            } else {
                format!("{relative}/{name}")
            };
            crate::files::validate_path(&relative_path)
                .map_err(|_| failure("invalid", "relative path exceeds local bounds"))?;
            let metadata = operation(cancel, tokio::fs::symlink_metadata(item.path())).await?;
            let kind = local_kind(&metadata);
            if kind == RemoteFileKind::Directory {
                if depth >= limits.max_depth {
                    return Err(failure(
                        "limit",
                        &format!("directory depth limit reached: {relative_path}"),
                    ));
                }
                pending.push((item.path(), relative_path.clone(), depth + 1));
            }
            if entries.len() >= limits.max_entries as usize {
                return Err(failure("limit", "directory entry limit reached"));
            }
            metadata_bytes += relative_path.len() * 2 + 512;
            if metadata_bytes > MAX_METADATA_BYTES {
                return Err(failure(
                    "limit",
                    "directory metadata exceeds 8 MiB local budget",
                ));
            }
            entries.push((relative_path, metadata));
        }
    }
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(entries)
}

fn validate_download_names(entries: &[DirectoryTransferEntry]) -> Result<(), FileTransferError> {
    #[cfg(windows)]
    {
        use windows_sys::Win32::Globalization::{
            CompareStringOrdinal, CSTR_EQUAL, CSTR_GREATER_THAN, CSTR_LESS_THAN,
        };
        for entry in entries {
            for name in entry.relative_path.split('/') {
                validate_windows_name(name).map_err(|mut error| {
                    error.message =
                        format!("{}; relative path: {}", error.message, entry.relative_path);
                    error
                })?;
            }
        }
        let mut names: Vec<_> = entries
            .iter()
            .map(|e| e.relative_path.encode_utf16().collect::<Vec<_>>())
            .collect();
        let compare_failed = std::cell::Cell::new(false);
        let compare = |a: &[u16], b: &[u16]| {
            // SAFETY: Nonempty UTF-16 vectors live across the call. Paths are bounded
            // to 16 KiB, so explicit lengths fit i32. No locale-dependent collation.
            match unsafe {
                CompareStringOrdinal(a.as_ptr(), a.len() as i32, b.as_ptr(), b.len() as i32, 1)
            } {
                CSTR_LESS_THAN => std::cmp::Ordering::Less,
                CSTR_EQUAL => std::cmp::Ordering::Equal,
                CSTR_GREATER_THAN => std::cmp::Ordering::Greater,
                _ => {
                    compare_failed.set(true);
                    a.cmp(b)
                }
            }
        };
        names.sort_unstable_by(|a, b| compare(a, b));
        let aliases = names
            .windows(2)
            .any(|pair| compare(&pair[0], &pair[1]).is_eq());
        if compare_failed.get() || aliases {
            return Err(failure(
                "invalid",
                "directory contains ambiguous Windows destination paths",
            ));
        }
    }
    #[cfg(not(windows))]
    let _ = entries;
    Ok(())
}

#[cfg(windows)]
fn validate_windows_name(name: &str) -> Result<(), FileTransferError> {
    let stem = name
        .split('.')
        .next()
        .unwrap()
        .trim_end_matches(' ')
        .to_ascii_uppercase();
    let reserved = matches!(
        stem.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
    ) || ["COM", "LPT"].iter().any(|prefix| {
        stem.strip_prefix(prefix).is_some_and(|suffix| {
            matches!(
                suffix,
                "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
            )
        })
    });
    if name.is_empty()
        || reserved
        || name.ends_with(['.', ' '])
        || name.encode_utf16().count() > 255
        || name.chars().any(|c| c < ' ' || "<>:/\\|?*\"".contains(c))
    {
        return Err(failure(
            "invalid",
            &format!("unsupported Windows destination name: {name}"),
        ));
    }
    Ok(())
}
