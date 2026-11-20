//! Directory execution uses the same single-file staging and verification path.
use crate::directory_plan::is_link;
use crate::file_transfer::{
    bounded, download, operation, remote_metadata, upload, TransferProgressCallback,
};
use crate::{
    DirectoryTransferEntry, DirectoryTransferPlan, DirectoryWalkLimits, FileConflictPolicy,
    FileTransferDirection, FileTransferError, FileTransferRequest, RemoteFileKind, SshService,
};
use russh_sftp::client::SftpSession;
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio_util::sync::CancellationToken;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DirectoryConflictPolicy {
    #[default]
    Ask,
    Skip,
    Merge,
    KeepBoth,
    Replace,
}

/// A file awaiting an explicit choice inside the current directory attempt.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DirectoryFileConflict {
    pub relative_path: String,
    pub destination_path: String,
    pub direction: FileTransferDirection,
    pub source_size: String,
}

/// The selected destination root, before any directory or file is published.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DirectoryRootConflict {
    pub destination_path: String,
    pub direction: FileTransferDirection,
    pub total_files: u32,
    pub total_bytes: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FileConflictDecision {
    pub policy: FileConflictPolicy,
    pub apply_to_remaining_files: bool,
}

#[async_trait::async_trait]
pub trait DirectoryConflictResolver: Send + Sync {
    async fn resolve(
        &self,
        conflict: DirectoryFileConflict,
    ) -> Result<FileConflictDecision, FileTransferError>;

    async fn resolve_root(
        &self,
        _conflict: DirectoryRootConflict,
    ) -> Result<DirectoryConflictPolicy, FileTransferError> {
        Err(failure(
            "conflict",
            "directory root requires an explicit decision",
        ))
    }
}

/// Compatibility name for existing file-only hosts; root handling is opt-in.
pub use DirectoryConflictResolver as DirectoryFileConflictResolver;

/// Host interaction is optional; callers without a resolver retain fail-on-Ask behavior.
pub struct DirectoryTransferOptions {
    pub conflict: DirectoryConflictPolicy,
    pub limits: DirectoryWalkLimits,
    pub cancel: CancellationToken,
    pub progress: Arc<dyn Fn(DirectoryTransferProgress) + Send + Sync>,
    /// Legacy field name; the same resolver may also handle the selected root.
    pub file_conflicts: Option<Arc<dyn DirectoryConflictResolver>>,
}

#[derive(Debug, Clone)]
pub struct DirectoryTransferProgress {
    pub bytes: u64,
    pub total: u64,
    pub total_files: u32,
    pub completed_files: u32,
    pub skipped_files: u32,
    pub current_file: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectoryTransferReceipt {
    pub bytes: String,
    pub total_bytes: String,
    pub total_files: u32,
    pub completed_files: u32,
    pub skipped_files: u32,
    pub created_directories: u32,
    pub cleanup_warnings: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub destination_path: Option<String>,
    #[serde(default)]
    pub renamed_files: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publish_strategy: Option<String>,
}

impl SshService {
    pub async fn transfer_directory(
        &self,
        connection_id: &str,
        request: FileTransferRequest,
        conflict: DirectoryConflictPolicy,
        limits: DirectoryWalkLimits,
        cancel: CancellationToken,
        progress: Arc<dyn Fn(DirectoryTransferProgress) + Send + Sync>,
    ) -> Result<DirectoryTransferReceipt, FileTransferError> {
        self.transfer_directory_with_options(
            connection_id,
            request,
            DirectoryTransferOptions {
                conflict,
                limits,
                cancel,
                progress,
                file_conflicts: None,
            },
        )
        .await
    }

    pub async fn transfer_directory_with_options(
        &self,
        connection_id: &str,
        mut request: FileTransferRequest,
        options: DirectoryTransferOptions,
    ) -> Result<DirectoryTransferReceipt, FileTransferError> {
        let DirectoryTransferOptions {
            conflict,
            limits,
            cancel,
            progress,
            file_conflicts,
        } = options;
        // Never trust a caller-provided plan or reuse a prior attempt's snapshot.
        if conflict == DirectoryConflictPolicy::KeepBoth
            && request.direction == FileTransferDirection::Download
        {
            request.local_path =
                crate::conflict_target::local_target(Path::new(&request.local_path), true, &cancel)
                    .await?
                    .to_str()
                    .ok_or_else(|| failure("invalid", "target is not UTF-8"))?
                    .to_owned();
        }
        let mut plan = self
            .plan_directory_transfer(connection_id, request.clone(), limits, cancel.clone())
            .await?;
        let (owner, sftp) = operation(&cancel, self.inner.file_session(connection_id)).await?;
        let result = async {
            if conflict == DirectoryConflictPolicy::KeepBoth
                && request.direction == FileTransferDirection::Upload
            {
                plan.remote_root =
                    crate::conflict_target::remote_target(&sftp, &plan.remote_root, true, &cancel)
                        .await?;
                for entry in &plan.entries {
                    crate::directory_walk::validate_root(&format!(
                        "{}/{}",
                        plan.remote_root, entry.relative_path
                    ))?;
                }
            }
            let total = plan
                .total_bytes
                .parse()
                .map_err(|_| failure("integrity", "invalid planned total"))?;
            let total_files = plan.total_files;
            Execution {
                ssh: self,
                connection_id,
                limits,
                sftp: &sftp,
                request: &request,
                conflict,
                plan: &mut plan,
                cancel: &cancel,
                notify: progress,
                progress: DirectoryTransferProgress {
                    bytes: 0,
                    total,
                    total_files,
                    completed_files: 0,
                    skipped_files: 0,
                    current_file: None,
                },
                created_directories: 0,
                renamed_files: 0,
                warnings: Vec::new(),
                file_conflict: request.conflict,
                file_conflicts,
            }
            .run()
            .await
        }
        .await;
        let result = result.map_err(|mut error: FileTransferError| {
            error.destination_path = Some(match request.direction {
                FileTransferDirection::Upload => plan.remote_root.clone(),
                FileTransferDirection::Download => plan.local_root.clone(),
            });
            error
        });
        let close = tokio::time::timeout(Duration::from_secs(3), sftp.close()).await;
        owner.touch_keepalive();
        match (result, close) {
            (result, Ok(Ok(()))) => result,
            (Ok(mut receipt), _) => {
                receipt
                    .cleanup_warnings
                    .push("directory SFTP channel close was not confirmed".into());
                Ok(receipt)
            }
            (Err(mut error), _) => {
                error
                    .cleanup_warnings
                    .push("directory SFTP channel close was not confirmed".into());
                Err(error)
            }
        }
    }
}

fn failure(code: &str, message: &str) -> FileTransferError {
    FileTransferError::new(code, message)
}

pub(super) async fn local_directory(
    path: &Path,
    cancel: &CancellationToken,
) -> Result<(), FileTransferError> {
    let meta = operation(cancel, tokio::fs::symlink_metadata(path)).await?;
    if !meta.is_dir()
        || is_link(&meta)
        || operation(cancel, tokio::fs::canonicalize(path)).await? != path
    {
        return Err(failure(
            "invalid",
            "local directory identity changed or contains a link",
        ));
    }
    Ok(())
}

async fn remote_directory(
    sftp: &SftpSession,
    path: &str,
    cancel: &CancellationToken,
) -> Result<(), FileTransferError> {
    let attrs = operation(cancel, sftp.symlink_metadata(path)).await?;
    if !attrs.is_dir() || operation(cancel, sftp.canonicalize(path)).await? != path {
        return Err(failure(
            "invalid",
            "remote directory identity changed or contains a link",
        ));
    }
    Ok(())
}

/// Recheck every ancestor before path-based IO. SFTP has no portable directory-handle CAS.
pub(super) async fn remote_chain(
    sftp: &SftpSession,
    path: &str,
    cancel: &CancellationToken,
) -> Result<(), FileTransferError> {
    remote_directory(sftp, "/", cancel).await?;
    let mut current = String::new();
    for name in path.split('/').filter(|name| !name.is_empty()) {
        current.push('/');
        current.push_str(name);
        remote_directory(sftp, &current, cancel).await?;
    }
    Ok(())
}

pub(super) async fn local_chain(
    root: &Path,
    path: &Path,
    cancel: &CancellationToken,
) -> Result<(), FileTransferError> {
    local_directory(root, cancel).await?;
    let relative = path
        .strip_prefix(root)
        .map_err(|_| failure("invalid", "local path escaped selected root"))?;
    let mut current = root.to_owned();
    for component in relative.components() {
        current.push(component);
        local_directory(&current, cancel).await?;
    }
    Ok(())
}

struct Execution<'a> {
    ssh: &'a SshService,
    connection_id: &'a str,
    limits: DirectoryWalkLimits,
    sftp: &'a SftpSession,
    request: &'a FileTransferRequest,
    conflict: DirectoryConflictPolicy,
    plan: &'a mut DirectoryTransferPlan,
    cancel: &'a CancellationToken,
    notify: Arc<dyn Fn(DirectoryTransferProgress) + Send + Sync>,
    progress: DirectoryTransferProgress,
    created_directories: u32,
    renamed_files: u32,
    warnings: Vec<String>,
    file_conflict: FileConflictPolicy,
    file_conflicts: Option<Arc<dyn DirectoryFileConflictResolver>>,
}

impl Execution<'_> {
    fn emit(&self) {
        (self.notify)(self.progress.clone());
    }

    async fn validate_roots(&self) -> Result<(), FileTransferError> {
        let root = PathBuf::from(&self.plan.local_root);
        match self.request.direction {
            FileTransferDirection::Upload => {
                local_directory(&root, self.cancel).await?;
                let parent = self.plan.remote_root.rsplit_once('/').unwrap().0;
                remote_chain(
                    self.sftp,
                    if parent.is_empty() { "/" } else { parent },
                    self.cancel,
                )
                .await?;
            }
            FileTransferDirection::Download => {
                remote_chain(self.sftp, &self.plan.remote_root, self.cancel).await?;
                local_directory(root.parent().unwrap(), self.cancel).await?;
            }
        }
        Ok(())
    }

    async fn ensure_root(&mut self) -> Result<bool, FileTransferError> {
        let result = self.ensure_directory("", self.conflict).await;
        let Err(error) = result else { return result };
        if error.code != "conflict" || self.conflict != DirectoryConflictPolicy::Ask {
            return Err(error);
        }
        let Some(resolver) = &self.file_conflicts else {
            return Err(error);
        };
        let conflict = DirectoryRootConflict {
            direction: self.request.direction,
            destination_path: match self.request.direction {
                FileTransferDirection::Upload => self.plan.remote_root.clone(),
                FileTransferDirection::Download => self.plan.local_root.clone(),
            },
            total_files: self.plan.total_files,
            total_bytes: self.plan.total_bytes.clone(),
        };
        let policy = tokio::select! {
            biased;
            _ = self.cancel.cancelled() => return Err(failure("cancelled", "directory decision cancelled")),
            decision = resolver.resolve_root(conflict) => decision?,
        };
        if policy == DirectoryConflictPolicy::Ask {
            return Err(failure(
                "invalid",
                "a directory decision must select skip, merge, keep both or replace",
            ));
        }
        // A human wait grants no exemption from the original source/path checks.
        self.validate_roots().await?;
        if policy == DirectoryConflictPolicy::KeepBoth {
            match self.request.direction {
                FileTransferDirection::Upload => {
                    self.plan.remote_root = crate::conflict_target::remote_target(
                        self.sftp,
                        &self.plan.remote_root,
                        true,
                        self.cancel,
                    )
                    .await?;
                    for entry in &self.plan.entries {
                        crate::directory_walk::validate_root(&format!(
                            "{}/{}",
                            self.plan.remote_root, entry.relative_path,
                        ))?;
                    }
                }
                FileTransferDirection::Download => {
                    self.plan.local_root = crate::conflict_target::local_target(
                        Path::new(&self.plan.local_root),
                        true,
                        self.cancel,
                    )
                    .await?
                    .to_str()
                    .ok_or_else(|| failure("invalid", "target is not UTF-8"))?
                    .into();
                }
            }
        }
        self.conflict = policy;
        self.ensure_directory("", policy).await
    }

    async fn run(mut self) -> Result<DirectoryTransferReceipt, FileTransferError> {
        self.emit();
        self.validate_roots().await?;
        if self.ensure_root().await? {
            self.progress.skipped_files = self.plan.total_files;
            self.emit();
            return Ok(self.receipt());
        }
        if self.conflict == DirectoryConflictPolicy::Replace {
            // Keep the publication state machine off the caller's polling stack.
            return Box::pin(self.run_replace()).await;
        }
        self.entries().await?;
        Ok(self.receipt())
    }

    async fn run_replace(&mut self) -> Result<DirectoryTransferReceipt, FileTransferError> {
        let mut publication = crate::directory_publication::DirectoryPublication::prepare(
            self.ssh,
            self.connection_id,
            self.sftp,
            self.plan,
            self.limits,
            self.cancel,
        )
        .await?;
        publication.stage_plan(self.plan);
        self.created_directories += 1;
        let staged = self.entries().await;
        // Receipts, source validation and every error keep the user's final root.
        publication.restore_plan(self.plan);
        let result = async {
            staged?;
            self.validate_roots().await?;
            publication.publish(self.cancel).await
        }
        .await;
        match result {
            Ok((strategy, warnings)) => {
                self.warnings.extend(warnings);
                let mut receipt = self.receipt();
                receipt.publish_strategy = Some(strategy);
                Ok(receipt)
            }
            Err(error) => Err(publication.abort(error).await),
        }
    }

    async fn entries(&mut self) -> Result<(), FileTransferError> {
        let root = PathBuf::from(&self.plan.local_root);
        // Planning sorts POSIX relative paths, placing each parent before its descendants.
        for index in 0..self.plan.entries.len() {
            let entry = self.plan.entries[index].clone();
            self.progress.current_file = Some(entry.relative_path.clone());
            self.emit();
            let result = self.resolve_entry(&entry, &root).await;
            if let Err(mut error) = result {
                error.relative_path = Some(entry.relative_path.clone());
                error.cleanup_warnings.append(&mut self.warnings);
                return Err(error);
            }
        }
        self.progress.current_file = None;
        self.emit();
        Ok(())
    }

    fn receipt(&self) -> DirectoryTransferReceipt {
        DirectoryTransferReceipt {
            bytes: self.progress.bytes.to_string(),
            total_bytes: self.plan.total_bytes.clone(),
            total_files: self.plan.total_files,
            completed_files: self.progress.completed_files,
            skipped_files: self.progress.skipped_files,
            created_directories: self.created_directories,
            cleanup_warnings: self.warnings.clone(),
            destination_path: Some(match self.request.direction {
                FileTransferDirection::Upload => self.plan.remote_root.clone(),
                FileTransferDirection::Download => self.plan.local_root.clone(),
            }),
            renamed_files: self.renamed_files,
            publish_strategy: None,
        }
    }

    fn paths(&self, relative: &str) -> (PathBuf, String) {
        let mut local = PathBuf::from(&self.plan.local_root);
        for part in relative.split('/').filter(|part| !part.is_empty()) {
            local.push(part);
        }
        let remote = if relative.is_empty() {
            self.plan.remote_root.clone()
        } else {
            format!(
                "{}/{}",
                self.plan.remote_root.trim_end_matches('/'),
                relative
            )
        };
        (local, remote)
    }

    /// Returns true only for an explicitly skipped existing root.
    async fn ensure_directory(
        &mut self,
        relative: &str,
        policy: DirectoryConflictPolicy,
    ) -> Result<bool, FileTransferError> {
        let (local, remote) = self.paths(relative);
        let exists = match self.request.direction {
            FileTransferDirection::Upload => {
                let existing = remote_metadata(self.sftp, &remote).await?;
                if let Some(attrs) = &existing {
                    if !attrs.is_dir() {
                        return Err(failure(
                            "invalid",
                            "directory target is not a regular directory",
                        ));
                    }
                    remote_chain(self.sftp, &remote, self.cancel).await?;
                }
                existing.is_some()
            }
            FileTransferDirection::Download => {
                match operation(self.cancel, async {
                    match tokio::fs::symlink_metadata(&local).await {
                        Ok(meta) => Ok(Some(meta)),
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
                        Err(e) => Err(e),
                    }
                })
                .await?
                {
                    Some(meta) => {
                        if !meta.is_dir() || is_link(&meta) {
                            return Err(failure(
                                "invalid",
                                "directory target is a link or has changed type",
                            ));
                        }
                        local_directory(&local, self.cancel).await?;
                        true
                    }
                    None => false,
                }
            }
        };
        // Whole-directory Replace writes exclusively into its own sibling stage.
        if relative.is_empty() && policy == DirectoryConflictPolicy::Replace {
            return Ok(false);
        }
        if exists {
            return match policy {
                DirectoryConflictPolicy::Ask => Err(failure(
                    "conflict",
                    "directory exists; choose skip or explicit merge",
                )),
                DirectoryConflictPolicy::KeepBoth => Err(failure(
                    "conflict",
                    "keep-both directory target appeared before creation",
                )),
                DirectoryConflictPolicy::Skip => Ok(true),
                DirectoryConflictPolicy::Merge => Ok(false),
                DirectoryConflictPolicy::Replace => {
                    unreachable!("replace root is staged separately")
                }
            };
        }
        if self.cancel.is_cancelled() {
            return Err(failure("cancelled", "directory transfer cancelled"));
        }
        // Await the acknowledgement; cancellation must not abandon a pending MKDIR.
        let created = match self.request.direction {
            FileTransferDirection::Upload => {
                bounded("create remote directory", self.sftp.create_dir(&remote)).await
            }
            FileTransferDirection::Download => {
                bounded("create local directory", tokio::fs::create_dir(&local)).await
            }
        };
        if let Err(mut error) = created {
            if error.code == "timeout" {
                error.cleanup_warnings.push(format!(
                    "directory creation unconfirmed; inspect relative path: {relative}"
                ));
            }
            return Err(error);
        }
        self.created_directories += 1;
        Ok(false)
    }

    async fn resolve_entry(
        &mut self,
        entry: &DirectoryTransferEntry,
        root: &Path,
    ) -> Result<(), FileTransferError> {
        let result = self.entry(entry, root, self.file_conflict).await;
        let Err(error) = result else { return result };
        // A publication/cleanup failure is never an invitation to replay work.
        if error.code != "conflict"
            || !error.cleanup_warnings.is_empty()
            || entry.kind != RemoteFileKind::File
            || self.file_conflict != FileConflictPolicy::Ask
        {
            return Err(error);
        }
        let Some(resolver) = &self.file_conflicts else {
            return Err(error);
        };
        let (local, remote) = self.paths(&entry.relative_path);
        let conflict = DirectoryFileConflict {
            relative_path: entry.relative_path.clone(),
            destination_path: match self.request.direction {
                FileTransferDirection::Upload => remote,
                FileTransferDirection::Download => local.to_string_lossy().into_owned(),
            },
            direction: self.request.direction,
            source_size: entry.size_bytes.clone().unwrap_or_default(),
        };
        let decision = tokio::select! {
            biased;
            _ = self.cancel.cancelled() => return Err(failure("cancelled", "file decision cancelled")),
            decision = resolver.resolve(conflict) => decision?,
        };
        if decision.policy == FileConflictPolicy::Ask {
            return Err(failure(
                "invalid",
                "a file decision must select skip, keep both or replace",
            ));
        }
        if decision.apply_to_remaining_files {
            self.file_conflict = decision.policy;
        }
        // Recheck source, ancestors and destination after the human wait. Only this
        // entry is retried; previously published entries are never replayed.
        self.entry(entry, root, decision.policy).await
    }

    async fn entry(
        &mut self,
        entry: &DirectoryTransferEntry,
        root: &Path,
        policy: FileConflictPolicy,
    ) -> Result<(), FileTransferError> {
        let (local, remote) = self.paths(&entry.relative_path);
        local_chain(root, local.parent().unwrap(), self.cancel).await?;
        let parent = remote.rsplit_once('/').unwrap().0;
        remote_chain(self.sftp, parent, self.cancel).await?;
        if entry.kind == RemoteFileKind::Directory {
            // Recheck source type and identity before creating/merging its target.
            match self.request.direction {
                FileTransferDirection::Upload => local_directory(&local, self.cancel).await?,
                FileTransferDirection::Download => {
                    remote_directory(self.sftp, &remote, self.cancel).await?
                }
            }
            let policy = if self.conflict == DirectoryConflictPolicy::Merge {
                self.conflict
            } else {
                DirectoryConflictPolicy::Ask
            };
            self.ensure_directory(&entry.relative_path, policy).await?;
            return Ok(());
        }
        let planned = entry
            .size_bytes
            .as_deref()
            .and_then(|n| n.parse::<u64>().ok())
            .ok_or_else(|| failure("integrity", "planned file size is missing"))?;
        let size = match self.request.direction {
            FileTransferDirection::Upload => {
                let meta = operation(self.cancel, tokio::fs::symlink_metadata(&local)).await?;
                if !meta.is_file() || is_link(&meta) {
                    return Err(failure(
                        "invalid",
                        "source file changed type or became a link",
                    ));
                }
                meta.len()
            }
            FileTransferDirection::Download => {
                let meta = operation(self.cancel, self.sftp.symlink_metadata(&remote)).await?;
                if !meta.is_regular() {
                    return Err(failure(
                        "invalid",
                        "source file changed type or became a link",
                    ));
                }
                meta.size
                    .ok_or_else(|| failure("integrity", "source size is missing"))?
            }
        };
        if size != planned {
            return Err(failure("integrity", "source size changed after planning"));
        }
        let request = FileTransferRequest {
            direction: self.request.direction,
            local_path: local.to_str().unwrap().into(),
            remote_path: remote,
            conflict: policy,
        };
        let base = self.progress.clone();
        let notify = self.notify.clone();
        let progress: TransferProgressCallback = Arc::new(move |p| {
            let mut aggregate = base.clone();
            aggregate.bytes += p.bytes;
            notify(aggregate);
        });
        let receipt = match self.request.direction {
            FileTransferDirection::Upload => {
                upload(
                    self.sftp,
                    &request,
                    &local,
                    self.cancel,
                    &progress,
                    Some(planned),
                )
                .await?
            }
            FileTransferDirection::Download => {
                download(
                    self.sftp,
                    &request,
                    &local,
                    self.cancel,
                    &progress,
                    Some(planned),
                )
                .await?
            }
        };
        if receipt.skipped {
            self.progress.skipped_files += 1;
        } else {
            let expected = match self.request.direction {
                FileTransferDirection::Upload => request.remote_path.as_str(),
                FileTransferDirection::Download => request.local_path.as_str(),
            };
            if receipt
                .destination_path
                .as_deref()
                .is_some_and(|path| path != expected)
            {
                self.renamed_files += 1;
            }
            self.progress.bytes += planned;
            self.progress.completed_files += 1;
        }
        self.warnings.extend(receipt.cleanup_warnings);
        self.emit();
        // Keep the published file's progress and complete cleanup facts, but do
        // not spread unresolved cleanup across the remaining directory entries.
        if !self.warnings.is_empty() {
            return Err(failure(
                "cleanup_required",
                "file publication left unresolved cleanup; review the destination before retrying",
            ));
        }
        Ok(())
    }
}
