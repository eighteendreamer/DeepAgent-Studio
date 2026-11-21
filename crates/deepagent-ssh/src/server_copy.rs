//! Streaming endpoint copies reuse transfer staging, verification and publication.
use crate::directory_walk::RemoteFileSession;
use crate::file_transfer::{
    bounded, digest_remote, operation, publish_remote, pump, remote_metadata,
    TransferProgressCallback,
};
use crate::{
    DirectoryWalkLimits, FileConflictPolicy, FileTransferError, FileTransferProgress,
    FileTransferReceipt, RemoteFileKind, RemoteFileMetadata, SshService,
};
use russh_sftp::{
    client::{error::Error as SftpError, SftpSession},
    protocol::{FileAttributes, OpenFlags, StatusCode},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use tokio::io::AsyncWriteExt;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServerCopyMethod {
    Relay,
    Scp,
    Rsync,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ServerCopySpec {
    pub source_connection_id: String,
    pub destination_connection_id: String,
    pub source_path: String,
    pub destination_path: String,
    pub conflict: FileConflictPolicy,
    pub method: ServerCopyMethod,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerCopyEntry {
    pub relative_path: String,
    pub source: RemoteFileMetadata,
    pub destination: Option<RemoteFileMetadata>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerCopyPreview {
    pub version: u8,
    pub spec: ServerCopySpec,
    pub source_generation: String,
    pub destination_generation: String,
    pub entries: Vec<ServerCopyEntry>,
    pub total_files: u32,
    pub total_bytes: String,
    pub conflicts: u32,
    pub fingerprint: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ServerCopyRequest {
    pub version: u8,
    pub spec: ServerCopySpec,
    pub source_generation: String,
    pub destination_generation: String,
    pub fingerprint: String,
}
impl From<&ServerCopyPreview> for ServerCopyRequest {
    fn from(p: &ServerCopyPreview) -> Self {
        Self {
            version: p.version,
            spec: p.spec.clone(),
            source_generation: p.source_generation.clone(),
            destination_generation: p.destination_generation.clone(),
            fingerprint: p.fingerprint.clone(),
        }
    }
}
impl ServerCopyRequest {
    pub fn validate(&self) -> Result<(u64, u64), FileTransferError> {
        validate(&self.spec)?;
        if self.version != 1
            || self.fingerprint.len() != 64
            || !self
                .fingerprint
                .bytes()
                .all(|b| matches!(b,b'0'..=b'9'|b'a'..=b'f'))
        {
            return Err(fail("invalid", "invalid copy confirmation"));
        }
        Ok((
            generation(&self.source_generation)?,
            generation(&self.destination_generation)?,
        ))
    }
}
impl SshService {
    async fn validate_copy_identity(&self, spec: &ServerCopySpec) -> Result<(), FileTransferError> {
        let connections = self.list_connections().await;
        let source = connections
            .iter()
            .find(|c| c.id == spec.source_connection_id);
        let destination = connections
            .iter()
            .find(|c| c.id == spec.destination_connection_id);
        if let (Some(source), Some(destination)) = (source, destination) {
            if source.host.eq_ignore_ascii_case(&destination.host)
                && source.port == destination.port
                && (spec.source_path == spec.destination_path
                    || spec
                        .destination_path
                        .starts_with(&format!("{}/", spec.source_path)))
            {
                return Err(fail("invalid", "copy destination is inside the same server source (including connection aliases)"));
            }
        }
        Ok(())
    }
    pub async fn preview_server_copy(
        &self,
        spec: ServerCopySpec,
        cancel: CancellationToken,
    ) -> Result<ServerCopyPreview, FileTransferError> {
        validate(&spec)?;
        self.validate_copy_identity(&spec).await?;
        let source = RemoteFileSession::open(self, &spec.source_connection_id, &cancel).await?;
        let dest = RemoteFileSession::open(self, &spec.destination_connection_id, &cancel).await?;
        let result = read_plan(&source, &dest, spec.clone(), &cancel).await;
        dest.finish(source.finish(result))
    }
    pub async fn execute_server_copy(
        &self,
        request: ServerCopyRequest,
        cancel: CancellationToken,
        progress: TransferProgressCallback,
    ) -> Result<FileTransferReceipt, FileTransferError> {
        let (sg, dg) = request.validate()?;
        self.validate_copy_identity(&request.spec).await?;
        let spec = &request.spec;
        let source =
            RemoteFileSession::open_generation(self, &spec.source_connection_id, Some(sg), &cancel)
                .await?;
        let dest = RemoteFileSession::open_generation(
            self,
            &spec.destination_connection_id,
            Some(dg),
            &cancel,
        )
        .await?;
        let plan = read_plan(&source, &dest, spec.clone(), &cancel).await?;
        if plan.fingerprint != request.fingerprint {
            return Err(fail("conflict", "copy endpoints changed; preview again"));
        }
        if plan.conflicts > 0 && spec.conflict == FileConflictPolicy::Ask {
            return Err(fail("conflict", "choose an explicit file conflict policy"));
        }
        let direct = if spec.method != ServerCopyMethod::Relay {
            Some(self.require_direct_copy(&request, &cancel).await?)
        } else {
            None
        };
        let (so, ss) =
            operation(&cancel, self.inner.file_session(&spec.source_connection_id)).await?;
        let (do_, ds) = operation(
            &cancel,
            self.inner.file_session(&spec.destination_connection_id),
        )
        .await?;
        if so.transport_generation() != Some(sg) || do_.transport_generation() != Some(dg) {
            return Err(fail("conflict", "copy connection generation changed"));
        }
        let (mut bytes, mut files, mut skipped) = (0u64, 0u32, 0u32);
        let mut warnings = vec![];
        let total = plan
            .total_bytes
            .parse::<u64>()
            .map_err(|_| fail("limit", "copy size exceeds u64"))?;
        let result = async {
            for entry in &plan.entries {
                let target = target_path(spec, &entry.relative_path)?;
                crate::directory_transfer::remote_chain(&ds, parent(&target), &cancel).await?;
                let current = optional_metadata(&dest, &target, &cancel).await?;
                if current != entry.destination {
                    return Err(fail("conflict", &format!("copy target changed: {target}")));
                }
                if entry.source.kind == RemoteFileKind::Directory {
                    if current.is_none() {
                        bounded("create copy directory", ds.create_dir(&target)).await?;
                    }
                    continue;
                }
                if current.is_some() && spec.conflict == FileConflictPolicy::Skip {
                    skipped += 1;
                    continue;
                }
                let target = if current.is_some() && spec.conflict == FileConflictPolicy::KeepBoth {
                    crate::conflict_target::remote_target(&ds, &target, false, &cancel).await?
                } else {
                    target
                };
                source
                    .require_chain(parent(&entry.source.path), &cancel)
                    .await?;
                if facts(source.metadata(&entry.source.path, &cancel).await?) != entry.source {
                    return Err(fail("conflict", "copy source changed"));
                }
                let size = entry
                    .source
                    .size_bytes
                    .as_ref()
                    .and_then(|size| size.parse::<u64>().ok())
                    .ok_or_else(|| fail("invalid", "copy source size missing"))?;
                let prior = bytes;
                let report = progress.clone();
                let child: TransferProgressCallback = Arc::new(move |p| {
                    report(FileTransferProgress {
                        bytes: prior + p.bytes,
                        total: Some(total),
                    })
                });
                let receipt = copy_file(
                    self,
                    &request,
                    &ss,
                    &ds,
                    entry,
                    &target,
                    current.is_some() && spec.conflict == FileConflictPolicy::Replace,
                    size,
                    &cancel,
                    &child,
                    direct.as_ref(),
                )
                .await?;
                bytes += size;
                files += 1;
                warnings.extend(receipt.cleanup_warnings);
                progress(FileTransferProgress {
                    bytes,
                    total: Some(total),
                });
            }
            Ok(FileTransferReceipt {
                bytes: bytes.to_string(),
                sha256: String::new(),
                skipped: files == 0 && skipped > 0,
                publish_strategy: format!(
                    "{:?}: {files} files, {skipped} skipped; directories merged",
                    spec.method
                ),
                cleanup_warnings: warnings,
                destination_path: Some(spec.destination_path.clone()),
            })
        }
        .await;
        let _ = tokio::time::timeout(std::time::Duration::from_secs(3), ss.close()).await;
        let _ = tokio::time::timeout(std::time::Duration::from_secs(3), ds.close()).await;
        dest.finish(source.finish(result)).map_err(|mut error|{error.cleanup_warnings.push(format!("copy preserved {files} published files ({bytes} bytes); review destination before retry"));error})
    }
}
async fn read_plan(
    source: &RemoteFileSession<'_>,
    dest: &RemoteFileSession<'_>,
    spec: ServerCopySpec,
    cancel: &CancellationToken,
) -> Result<ServerCopyPreview, FileTransferError> {
    source
        .require_chain(parent(&spec.source_path), cancel)
        .await?;
    dest.require_chain(parent(&spec.destination_path), cancel)
        .await?;
    let root = facts(source.metadata(&spec.source_path, cancel).await?);
    let mut source_entries = vec![(String::new(), root.clone())];
    if root.kind == RemoteFileKind::Directory {
        source_entries.extend(
            source
                .walk(
                    &spec.source_path,
                    DirectoryWalkLimits {
                        max_entries: 9999,
                        max_depth: 64,
                    },
                    cancel,
                )
                .await?
                .entries
                .into_iter()
                .map(|e| (e.relative_path, facts(e.metadata))),
        );
    }
    let mut plan = ServerCopyPreview {
        version: 1,
        spec: spec.clone(),
        source_generation: source.connection_generation()?,
        destination_generation: dest.connection_generation()?,
        entries: vec![],
        total_files: 0,
        total_bytes: "0".into(),
        conflicts: 0,
        fingerprint: String::new(),
    };
    let mut bytes = 0u64;
    for (relative_path, metadata) in source_entries {
        if !matches!(
            metadata.kind,
            RemoteFileKind::File | RemoteFileKind::Directory
        ) {
            return Err(fail(
                "unsupported",
                "copy does not follow links or copy special files",
            ));
        }
        let target = target_path(&spec, &relative_path)?;
        if let Some(existing) = optional_metadata(dest, parent(&target), cancel).await? {
            if existing.kind != RemoteFileKind::Directory {
                return Err(fail("invalid", "destination ancestor is not a directory"));
            }
            dest.require_chain(parent(&target), cancel).await?;
        }
        let existing = optional_metadata(dest, &target, cancel).await?;
        if let Some(e) = &existing {
            if e.kind != metadata.kind {
                return Err(fail("conflict", "copy file/directory/link type conflict"));
            }
            if metadata.kind == RemoteFileKind::File {
                plan.conflicts += 1;
            }
        }
        if metadata.kind == RemoteFileKind::File {
            let size = metadata
                .size_bytes
                .as_ref()
                .and_then(|n| n.parse::<u64>().ok())
                .ok_or_else(|| fail("invalid", "copy file size missing"))?;
            bytes = bytes
                .checked_add(size)
                .ok_or_else(|| fail("limit", "copy total exceeds u64"))?;
            plan.total_files += 1;
        }
        plan.entries.push(ServerCopyEntry {
            relative_path,
            source: metadata,
            destination: existing,
        });
    }
    if facts(source.metadata(&spec.source_path, cancel).await?) != root {
        return Err(fail("conflict", "source changed during copy preview"));
    }
    plan.total_bytes = bytes.to_string();
    plan.fingerprint = format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&plan).map_err(|_| fail("invalid", "encode copy plan"))?)
    );
    Ok(plan)
}
#[allow(clippy::too_many_arguments)]
async fn copy_file(
    ssh: &SshService,
    request: &ServerCopyRequest,
    source: &SftpSession,
    dest: &SftpSession,
    entry: &ServerCopyEntry,
    target: &str,
    replace: bool,
    size: u64,
    cancel: &CancellationToken,
    progress: &TransferProgressCallback,
    direct: Option<&crate::direct_copy::DirectCopyCapability>,
) -> Result<FileTransferReceipt, FileTransferError> {
    let stage = format!(
        "{}/.deepagent-{}.part",
        parent(target).trim_end_matches('/'),
        uuid::Uuid::new_v4()
    );
    let mut output = bounded(
        "create copy staging",
        dest.open_with_flags_and_attributes(
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
                "copy staging creation unconfirmed; inspect before cleanup: {stage}"
            ));
        }
        error
    })?;
    let result = async {
        let digest = if let Some(direct) = direct {
            bounded("close direct staging", output.shutdown()).await?;
            ssh.execute_direct_copy_file(request, direct, &entry.source.path, &stage, cancel)
                .await?;
            digest_remote(source, &entry.source.path, size, cancel).await?
        } else {
            let mut input = operation(cancel, source.open(&entry.source.path)).await?;
            let digest = pump(&mut input, &mut output, size, cancel, progress).await?;
            bounded("close copy source", input.shutdown()).await?;
            bounded("close copy staging", output.shutdown()).await?;
            digest
        };
        if digest_remote(dest, &stage, size, cancel).await? != digest
            || digest_remote(source, &entry.source.path, size, cancel).await? != digest
        {
            return Err(fail("integrity", "copy source or staged content changed"));
        }
        crate::directory_transfer::remote_chain(source, parent(&entry.source.path), cancel).await?;
        crate::directory_transfer::remote_chain(dest, parent(target), cancel).await?;
        let current = remote_metadata(source, &entry.source.path)
            .await?
            .ok_or_else(|| fail("conflict", "source removed during copy"))?;
        if facts(crate::files::metadata(
            "",
            &entry.source.path,
            false,
            current,
        )) != entry.source
        {
            return Err(fail("conflict", "source changed during copy"));
        }
        let existing = remote_metadata(dest, target)
            .await?
            .map(|attrs| facts(crate::files::metadata("", target, false, attrs)));
        if (replace && existing != entry.destination) || (!replace && existing.is_some()) {
            return Err(fail("conflict", "target changed before copy publication"));
        }
        if cancel.is_cancelled() {
            return Err(fail("cancelled", "copy cancelled before publication"));
        }
        let (strategy, warnings) = publish_remote(dest, &stage, target, replace).await?;
        Ok(FileTransferReceipt {
            bytes: size.to_string(),
            sha256: digest,
            skipped: false,
            publish_strategy: strategy,
            cleanup_warnings: warnings,
            destination_path: Some(target.into()),
        })
    }
    .await;
    drop(output);
    if result.is_err()
        && !matches!(
            tokio::time::timeout(std::time::Duration::from_secs(3), dest.remove_file(&stage)).await,
            Ok(Ok(()))
        )
    {
        return result.map_err(|mut e| {
            e.cleanup_warnings
                .push(format!("copy staging cleanup required: {stage}"));
            e
        });
    }
    result
}
async fn optional_metadata(
    reader: &RemoteFileSession<'_>,
    path: &str,
    cancel: &CancellationToken,
) -> Result<Option<RemoteFileMetadata>, FileTransferError> {
    let result = tokio::select! {_=cancel.cancelled()=>return Err(fail("cancelled","copy preview cancelled")),result=tokio::time::timeout(std::time::Duration::from_secs(30),reader.protocol().lstat(path))=>result};
    match result {
        Ok(Ok(attrs)) => Ok(Some(facts(crate::files::metadata(
            "",
            path,
            false,
            attrs.attrs,
        )))),
        Ok(Err(SftpError::Status(s))) if s.status_code == StatusCode::NoSuchFile => Ok(None),
        Ok(Err(error)) => Err(fail("io", &crate::file_transfer::error_detail(error))),
        Err(_) => Err(fail("timeout", "copy metadata timed out")),
    }
}
fn validate(spec: &ServerCopySpec) -> Result<(), FileTransferError> {
    for path in [&spec.source_path, &spec.destination_path] {
        crate::files::validate_absolute_path(path).map_err(|e| fail("invalid", &e.to_string()))?;
        if path == "/" {
            return Err(fail("invalid", "select a path below filesystem root"));
        }
    }
    for id in [&spec.source_connection_id, &spec.destination_connection_id] {
        if id.is_empty() || id.len() > 256 || id.contains('\0') {
            return Err(fail("invalid", "invalid copy endpoint"));
        }
    }
    if spec.source_connection_id == spec.destination_connection_id
        && (spec.source_path == spec.destination_path
            || spec
                .destination_path
                .starts_with(&format!("{}/", spec.source_path)))
    {
        return Err(fail("invalid", "copy destination is inside source"));
    }
    Ok(())
}
fn target_path(spec: &ServerCopySpec, relative: &str) -> Result<String, FileTransferError> {
    let p = if relative.is_empty() {
        spec.destination_path.clone()
    } else {
        format!("{}/{}", spec.destination_path, relative)
    };
    crate::files::validate_absolute_path(&p).map_err(|e| fail("invalid", &e.to_string()))?;
    Ok(p)
}
fn parent(p: &str) -> &str {
    p.rsplit_once('/')
        .map(|(p, _)| if p.is_empty() { "/" } else { p })
        .unwrap_or("/")
}
fn facts(mut m: RemoteFileMetadata) -> RemoteFileMetadata {
    m.connection_id = String::new();
    m.accessed_ms = None;
    m
}
fn generation(t: &str) -> Result<u64, FileTransferError> {
    t.parse::<u64>()
        .ok()
        .filter(|v| *v != 0 && v.to_string() == t)
        .ok_or_else(|| fail("invalid", "invalid copy generation"))
}
fn fail(code: &str, message: &str) -> FileTransferError {
    FileTransferError::new(code, message)
}
