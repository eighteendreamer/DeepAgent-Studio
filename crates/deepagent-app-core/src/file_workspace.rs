//! File-panel facts. Transfer execution remains in TransferCoordinator.
use deepagent_ssh::{FileTransferError, SshService, SshServiceHandle};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum FileEndpoint {
    Local,
    Ssh {
        #[serde(rename = "connectionId")]
        connection_id: String,
    },
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FilePanelEntry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub is_symlink: bool,
    pub size_bytes: Option<String>,
    pub modified_ms: Option<u64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FilePanelListing {
    pub endpoint: FileEndpoint,
    pub path: String,
    pub parent: Option<String>,
    pub entries: Vec<FilePanelEntry>,
}

pub async fn list_endpoint(
    ssh: &SshService,
    endpoint: FileEndpoint,
    path: String,
) -> Result<FilePanelListing, FileTransferError> {
    match &endpoint {
        FileEndpoint::Local => tokio::task::spawn_blocking(move || list_local(&path))
            .await
            .map_err(|error| failure(&error.to_string()))?,
        FileEndpoint::Ssh { connection_id } => {
            let handle = SshServiceHandle::new(connection_id, connection_id, 80, 24);
            let listing = ssh
                .list_dir(&handle, &path)
                .await
                .map_err(|error| failure(&error.to_string()))?;
            let parent = if listing.canonical_path == "/" {
                None
            } else {
                Some(
                    listing
                        .canonical_path
                        .rsplit_once('/')
                        .map(|(parent, _)| if parent.is_empty() { "/" } else { parent })
                        .unwrap_or("/")
                        .into(),
                )
            };
            Ok(FilePanelListing {
                endpoint,
                path: listing.canonical_path,
                parent,
                entries: listing
                    .entries
                    .into_iter()
                    .map(|entry| FilePanelEntry {
                        name: entry.name,
                        path: entry.path,
                        is_dir: entry.is_dir,
                        is_symlink: entry.is_symlink,
                        size_bytes: entry.size.map(|size| size.to_string()),
                        modified_ms: entry.modified_ms,
                    })
                    .collect(),
            })
        }
    }
}

fn list_local(path: &str) -> Result<FilePanelListing, FileTransferError> {
    if path.contains('\0') || path.len() > 32 * 1024 || !Path::new(path).is_absolute() {
        return Err(failure("expected absolute local directory"));
    }
    let canonical = std::fs::canonicalize(path).map_err(|error| failure(&error.to_string()))?;
    let mut entries = Vec::new();
    for result in std::fs::read_dir(&canonical).map_err(|error| failure(&error.to_string()))? {
        if entries.len() == 10_000 {
            return Err(failure(
                "directory exceeds 10000 entries; choose a smaller directory",
            ));
        }
        let entry = result.map_err(|error| failure(&error.to_string()))?;
        let metadata =
            std::fs::symlink_metadata(entry.path()).map_err(|error| failure(&error.to_string()))?;
        let is_symlink = local_link(&metadata);
        entries.push(FilePanelEntry {
            name: entry.file_name().to_string_lossy().into(),
            path: display_path(&entry.path()),
            is_dir: metadata.is_dir() && !is_symlink,
            is_symlink,
            size_bytes: metadata.is_file().then(|| metadata.len().to_string()),
            modified_ms: metadata
                .modified()
                .ok()
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64),
        });
    }
    Ok(FilePanelListing {
        endpoint: FileEndpoint::Local,
        path: display_path(&canonical),
        parent: canonical.parent().map(display_path),
        entries,
    })
}

pub(crate) fn local_link(metadata: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}

fn display_path(path: &Path) -> String {
    let text = path.to_string_lossy();
    #[cfg(windows)]
    {
        if let Some(path) = text.strip_prefix("\\\\?\\UNC\\") {
            return format!("\\\\{path}");
        }
        text.strip_prefix("\\\\?\\").unwrap_or(&text).into()
    }
    #[cfg(not(windows))]
    {
        text.into()
    }
}

fn failure(message: &str) -> FileTransferError {
    FileTransferError::new("io", message)
}
