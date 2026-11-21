//! Whole-directory publication around the existing per-file executor.
//! SFTP v3 and portable filesystem paths have no directory CAS: snapshots detect
//! observable changes, but cannot exclude every concurrent rename/write race.
use crate::directory_plan::{local_entries, local_kind};
use crate::directory_transfer::{local_chain, local_directory, remote_chain};
use crate::file_transfer::{bounded, error_detail, operation, remote_metadata};
use crate::{
    DirectoryTransferPlan, DirectoryWalkLimits, FileTransferDirection, FileTransferError,
    RemoteFileKind, RemoteFileMetadata, SshService,
};
use russh_sftp::client::{error::Error as SftpError, SftpSession};
use std::{
    fs::Metadata,
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio_util::sync::CancellationToken;

type Result<T> = std::result::Result<T, FileTransferError>;

#[derive(Clone)]
enum Destination {
    Local(PathBuf),
    Remote(String),
}

impl Destination {
    fn text(&self) -> String {
        match self {
            Self::Local(path) => path.to_string_lossy().into_owned(),
            Self::Remote(path) => path.clone(),
        }
    }

    fn sibling(&self, suffix: &str) -> Self {
        match self {
            Self::Local(path) => {
                let mut name = path.file_name().unwrap().to_os_string();
                name.push(suffix);
                Self::Local(path.with_file_name(name))
            }
            Self::Remote(path) => Self::Remote(format!("{path}{suffix}")),
        }
    }

    fn child(&self, relative: &str) -> Self {
        match self {
            Self::Local(root) => {
                let mut path = root.clone();
                for part in relative.split('/') {
                    path.push(part);
                }
                Self::Local(path)
            }
            Self::Remote(root) => Self::Remote(format!("{root}/{relative}")),
        }
    }

    fn redirect(&self, plan: &mut DirectoryTransferPlan) {
        match self {
            Self::Local(_) => plan.local_root = self.text(),
            Self::Remote(_) => plan.remote_root = self.text(),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
struct Stamp {
    kind: RemoteFileKind,
    bytes: Option<String>,
    modified: Option<SystemTime>,
    created: Option<SystemTime>,
    permissions: Option<u32>,
    uid: Option<u32>,
    gid: Option<u32>,
}

impl Stamp {
    fn local(metadata: &Metadata) -> Self {
        #[cfg(unix)]
        let permissions = {
            use std::os::unix::fs::PermissionsExt;
            metadata.permissions().mode()
        };
        #[cfg(not(unix))]
        let permissions = u32::from(metadata.permissions().readonly());
        Self {
            kind: local_kind(metadata),
            bytes: Some(metadata.len().to_string()),
            modified: metadata.modified().ok(),
            created: metadata.created().ok(),
            permissions: Some(permissions),
            uid: None,
            gid: None,
        }
    }

    fn remote(metadata: RemoteFileMetadata) -> Self {
        Self {
            kind: metadata.kind,
            bytes: metadata.size_bytes,
            modified: metadata
                .modified_ms
                .map(|ms| UNIX_EPOCH + Duration::from_millis(ms)),
            created: None,
            permissions: metadata.permissions,
            uid: metadata.uid,
            gid: metadata.gid,
        }
    }

    fn unchanged_for_removal(&self, current: &Self) -> bool {
        if self.kind == RemoteFileKind::Directory {
            // Removing our children changes the directory's size/mtime.
            self.kind == current.kind
                && self.created == current.created
                && self.permissions == current.permissions
                && self.uid == current.uid
                && self.gid == current.gid
        } else {
            self == current
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
struct Snapshot {
    root: Stamp,
    entries: Vec<(String, Stamp)>,
}

struct RenameFailure {
    error: Box<FileTransferError>,
    uncertain: bool,
}

pub(super) struct DirectoryPublication<'a> {
    ssh: &'a SshService,
    connection_id: &'a str,
    sftp: &'a SftpSession,
    limits: DirectoryWalkLimits,
    target: Destination,
    stage: Destination,
    backup: Destination,
    original: Option<Snapshot>,
    stage_owned: bool,
    retain_stage: bool,
}

impl<'a> DirectoryPublication<'a> {
    pub(super) async fn prepare(
        ssh: &'a SshService,
        connection_id: &'a str,
        sftp: &'a SftpSession,
        plan: &DirectoryTransferPlan,
        limits: DirectoryWalkLimits,
        cancel: &CancellationToken,
    ) -> Result<Self> {
        let target = match plan.direction {
            FileTransferDirection::Upload if plan.remote_root == "/" => {
                return Err(FileTransferError::new(
                    "invalid",
                    "cannot replace the remote filesystem root",
                ));
            }
            FileTransferDirection::Upload => Destination::Remote(plan.remote_root.clone()),
            FileTransferDirection::Download => Destination::Local(PathBuf::from(&plan.local_root)),
        };
        let attempt = uuid::Uuid::new_v4();
        let stage = target.sibling(&format!(".deepagent-{attempt}.directory-stage"));
        let backup = target.sibling(&format!(".deepagent-{attempt}.directory-backup"));
        let mut publication = Self {
            ssh,
            connection_id,
            sftp,
            limits,
            target,
            stage,
            backup,
            original: None,
            stage_owned: false,
            retain_stage: false,
        };
        publication.original = publication.snapshot(&publication.target, cancel).await?;
        if publication
            .metadata(&publication.stage, cancel)
            .await?
            .is_some()
            || publication
                .metadata(&publication.backup, cancel)
                .await?
                .is_some()
        {
            return Err(FileTransferError::new(
                "conflict",
                "unique directory publication path is occupied",
            ));
        }
        if let Destination::Remote(path) = &publication.stage {
            crate::directory_walk::validate_root(path)?;
            for entry in &plan.entries {
                crate::directory_walk::validate_root(&format!("{path}/{}", entry.relative_path))?;
            }
        }
        if cancel.is_cancelled() {
            return Err(FileTransferError::new(
                "cancelled",
                "directory transfer cancelled",
            ));
        }
        // Await the creation acknowledgement before claiming cleanup ownership.
        let mut unconfirmed_creation = false;
        let created = match &publication.stage {
            Destination::Local(path) => {
                bounded("create directory stage", tokio::fs::create_dir(path)).await
            }
            Destination::Remote(path) => {
                bounded("create directory stage", async {
                    let result = sftp.create_dir(path.as_str()).await;
                    if let Err(error) = &result {
                        unconfirmed_creation = !matches!(error, SftpError::Status(_));
                    }
                    result
                })
                .await
            }
        };
        created.map_err(|mut error| {
            if unconfirmed_creation || error.code == "timeout" {
                error.cleanup_warnings.push(format!(
                    "staging creation unconfirmed; inspect {}",
                    publication.stage.text()
                ));
            }
            error
        })?;
        publication.stage_owned = true;
        Ok(publication)
    }

    pub(super) fn stage_plan(&self, plan: &mut DirectoryTransferPlan) {
        self.stage.redirect(plan);
    }

    pub(super) fn restore_plan(&self, plan: &mut DirectoryTransferPlan) {
        self.target.redirect(plan);
    }

    async fn metadata(
        &self,
        path: &Destination,
        cancel: &CancellationToken,
    ) -> Result<Option<Stamp>> {
        match path {
            Destination::Local(path) => {
                operation(cancel, async {
                    match tokio::fs::symlink_metadata(path).await {
                        Ok(metadata) => Ok(Some(Stamp::local(&metadata))),
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
                        Err(error) => Err(error),
                    }
                })
                .await
            }
            Destination::Remote(path) => Ok(remote_metadata(self.sftp, path).await?.map(|attrs| {
                Stamp::remote(crate::files::metadata(
                    self.connection_id,
                    path,
                    false,
                    attrs,
                ))
            })),
        }
    }

    async fn validate_parent(&self, path: &Destination, cancel: &CancellationToken) -> Result<()> {
        match path {
            Destination::Local(path) => local_directory(path.parent().unwrap(), cancel).await,
            Destination::Remote(path) => {
                let parent = path.rsplit_once('/').unwrap().0;
                remote_chain(
                    self.sftp,
                    if parent.is_empty() { "/" } else { parent },
                    cancel,
                )
                .await
            }
        }
    }

    async fn snapshot(
        &self,
        path: &Destination,
        cancel: &CancellationToken,
    ) -> Result<Option<Snapshot>> {
        self.validate_parent(path, cancel).await?;
        let Some(root) = self.metadata(path, cancel).await? else {
            return Ok(None);
        };
        if root.kind != RemoteFileKind::Directory {
            return Err(FileTransferError::new(
                "invalid",
                "directory publication target changed type or became a link",
            ));
        }
        let entries = match path {
            Destination::Local(path) => local_entries(path, self.limits, cancel)
                .await?
                .into_iter()
                .map(|(relative, metadata)| (relative, Stamp::local(&metadata)))
                .collect(),
            Destination::Remote(path) => {
                remote_chain(self.sftp, path, cancel).await?;
                let tree = self
                    .ssh
                    .walk_directory(self.connection_id, path, self.limits, cancel.clone())
                    .await?;
                if tree.root != *path {
                    return Err(FileTransferError::new(
                        "invalid",
                        "directory identity changed during publication",
                    ));
                }
                tree.entries
                    .into_iter()
                    .map(|entry| (entry.relative_path, Stamp::remote(entry.metadata)))
                    .collect()
            }
        };
        Ok(Some(Snapshot { root, entries }))
    }

    async fn rename(
        &self,
        from: &Destination,
        to: &Destination,
    ) -> std::result::Result<(), RenameFailure> {
        let result = tokio::time::timeout(Duration::from_secs(30), async {
            match (from, to) {
                (Destination::Local(from), Destination::Local(to)) => tokio::fs::rename(from, to)
                    .await
                    .map_err(|error| (error_detail(error), false)),
                (Destination::Remote(from), Destination::Remote(to)) => self
                    .sftp
                    .rename(from.as_str(), to.as_str())
                    .await
                    .map_err(|error| {
                        let uncertain = !matches!(error, SftpError::Status(_));
                        (error_detail(error), uncertain)
                    }),
                _ => unreachable!("one publication always stays on the same endpoint"),
            }
        })
        .await;
        match result {
            Ok(Ok(())) => Ok(()),
            Ok(Err((message, uncertain))) => Err(RenameFailure {
                error: Box::new(FileTransferError::new(
                    "publish",
                    &format!("directory rename: {message}"),
                )),
                uncertain,
            }),
            Err(_) => Err(RenameFailure {
                error: Box::new(FileTransferError::new(
                    "publish",
                    "directory rename timed out; outcome unconfirmed",
                )),
                uncertain: true,
            }),
        }
    }

    /// The critical section deliberately does not observe the caller's cancellation.
    pub(super) async fn publish(
        &mut self,
        cancel: &CancellationToken,
    ) -> Result<(String, Vec<String>)> {
        if self.snapshot(&self.target, cancel).await? != self.original {
            return Err(FileTransferError::new(
                "conflict",
                "directory target changed while staging",
            ));
        }
        if cancel.is_cancelled() {
            return Err(FileTransferError::new(
                "cancelled",
                "directory transfer cancelled before publication",
            ));
        }
        let critical = CancellationToken::new();
        if self.metadata(&self.backup, cancel).await?.is_some() {
            return Err(FileTransferError::new(
                "conflict",
                "directory backup path appeared before publication",
            ));
        }
        if cancel.is_cancelled() {
            return Err(FileTransferError::new(
                "cancelled",
                "directory transfer cancelled before publication",
            ));
        }
        if self.original.is_some() {
            if let Err(mut failure) = self.rename(&self.target, &self.backup).await {
                if failure.uncertain {
                    self.retain_stage = true;
                    failure.error.cleanup_warnings.push(format!(
                        "backup move unconfirmed; inspect target {}, backup {} and stage {}",
                        self.target.text(),
                        self.backup.text(),
                        self.stage.text()
                    ));
                }
                return Err(*failure.error);
            }
        }
        // An explicit failure may be rolled back only when the target is absent.
        let published = match self.metadata(&self.target, &critical).await {
            Ok(None) => self.rename(&self.stage, &self.target).await,
            Ok(Some(_)) => Err(RenameFailure {
                error: Box::new(FileTransferError::new(
                    "publish",
                    "directory target appeared before publication",
                )),
                uncertain: false,
            }),
            Err(error) => Err(RenameFailure {
                error: Box::new(error),
                uncertain: true,
            }),
        };
        if let Err(mut failure) = published {
            failure.error.code = "publish".into();
            if failure.uncertain {
                self.retain_stage = true;
                failure.error.cleanup_warnings.push(format!(
                    "publication unconfirmed; inspect destination {} and stage {}",
                    self.target.text(),
                    self.stage.text()
                ));
            }
            if self.original.is_some() {
                let recovery = async {
                    if failure.uncertain {
                        return Err(FileTransferError::new(
                            "publish",
                            "publication outcome is unconfirmed; automatic recovery withheld",
                        ));
                    }
                    if self.metadata(&self.target, &critical).await?.is_some() {
                        return Err(FileTransferError::new(
                            "conflict",
                            "destination is occupied; recovery must not overwrite it",
                        ));
                    }
                    self.rename(&self.backup, &self.target)
                        .await
                        .map_err(|failure| *failure.error)
                }
                .await;
                if let Err(error) = recovery {
                    failure.error.cleanup_warnings.push(format!(
                        "original backup requires manual recovery: {} ({})",
                        self.backup.text(),
                        error.message
                    ));
                }
            }
            return Err(*failure.error);
        }
        self.stage_owned = false;
        let mut warnings = Vec::new();
        if self.original.is_some() {
            if let Err(error) = self.cleanup(&self.backup, self.original.as_ref()).await {
                warnings.push(format!(
                    "directory published; backup cleanup required: {} ({})",
                    self.backup.text(),
                    error.message
                ));
            }
        }
        Ok((
            if self.original.is_some() {
                "directory_backup_replace"
            } else {
                "directory_created"
            }
            .into(),
            warnings,
        ))
    }

    pub(super) async fn abort(&self, mut error: FileTransferError) -> FileTransferError {
        if self.stage_owned && !self.retain_stage {
            if let Err(cleanup) = self.cleanup(&self.stage, None).await {
                error.cleanup_warnings.push(format!(
                    "directory staging cleanup required: {} ({})",
                    self.stage.text(),
                    cleanup.message
                ));
            }
        }
        error
    }

    async fn cleanup(&self, root: &Destination, expected: Option<&Snapshot>) -> Result<()> {
        bounded("directory cleanup", async {
            let cancel = CancellationToken::new();
            let snapshot = self.snapshot(root, &cancel).await?.ok_or_else(|| {
                FileTransferError::new("invalid", "owned directory disappeared before cleanup")
            })?;
            if expected.is_some_and(|expected| *expected != snapshot) {
                return Err(FileTransferError::new(
                    "conflict",
                    "backup changed; retained for manual inspection",
                ));
            }
            for (relative, stamp) in snapshot.entries.iter().rev() {
                let path = root.child(relative);
                self.remove(root, &path, stamp, &cancel).await?;
            }
            self.remove(root, root, &snapshot.root, &cancel).await
        })
        .await
    }

    async fn remove(
        &self,
        root: &Destination,
        path: &Destination,
        stamp: &Stamp,
        cancel: &CancellationToken,
    ) -> Result<()> {
        self.validate_parent(root, cancel).await?;
        match (root, path) {
            (Destination::Local(root), Destination::Local(path)) => {
                if root != path {
                    local_chain(root, path.parent().unwrap(), cancel).await?;
                } else {
                    local_directory(root, cancel).await?;
                }
            }
            (Destination::Remote(_), Destination::Remote(path)) => {
                let parent = path.rsplit_once('/').unwrap().0;
                remote_chain(
                    self.sftp,
                    if parent.is_empty() { "/" } else { parent },
                    cancel,
                )
                .await?;
            }
            _ => unreachable!(),
        }
        let current = self
            .metadata(path, cancel)
            .await?
            .ok_or_else(|| FileTransferError::new("conflict", "cleanup entry disappeared"))?;
        if !stamp.unchanged_for_removal(&current) {
            return Err(FileTransferError::new(
                "conflict",
                "cleanup entry changed; retained",
            ));
        }
        match path {
            Destination::Local(path) => {
                let metadata = operation(cancel, tokio::fs::symlink_metadata(path)).await?;
                #[cfg(windows)]
                let directory = {
                    use std::os::windows::fs::MetadataExt;
                    metadata.file_attributes()
                        & windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_DIRECTORY
                        != 0
                };
                #[cfg(not(windows))]
                let directory = metadata.is_dir();
                if directory {
                    bounded("remove owned directory", tokio::fs::remove_dir(path)).await
                } else {
                    bounded("remove owned entry", tokio::fs::remove_file(path)).await
                }
            }
            Destination::Remote(path) if current.kind == RemoteFileKind::Directory => {
                bounded(
                    "remove owned directory",
                    self.sftp.remove_dir(path.as_str()),
                )
                .await
            }
            Destination::Remote(path) => {
                bounded("remove owned entry", self.sftp.remove_file(path.as_str())).await
            }
        }
    }
}
