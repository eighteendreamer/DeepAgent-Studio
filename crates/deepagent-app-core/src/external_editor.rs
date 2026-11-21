//! Opt-in external editing. Saved snapshots enter TransferCoordinator; this
//! service observes files and receipts and owns no parallel transfer machinery.
use crate::{
    file_operations::WorkbenchOperation,
    file_transfer_service::{TransferCoordinator, TransferState},
};
use deepagent_ssh::{
    ConditionalUploadRequest, FileConflictPolicy, FileTransferDirection, FileTransferRequest,
    SshService,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::sync::Mutex;

const MAX_BYTES: u64 = 32 * 1024 * 1024;
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EditorSession {
    pub id: String,
    pub connection_id: String,
    pub remote_path: String,
    pub local_path: String,
    pub auto_upload: bool,
    pub state: String,
    pub error: Option<String>,
    pub task_id: Option<String>,
    #[serde(skip)]
    generation: u64,
    #[serde(skip)]
    remote_hash: Option<String>,
    #[serde(skip)]
    uploaded_hash: String,
    #[serde(skip)]
    pending_hash: Option<String>,
    #[serde(skip)]
    snapshot: Option<PathBuf>,
    #[serde(skip)]
    revision: u64,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EditorOpenRequest {
    pub connection_id: String,
    pub remote_path: String,
    pub executable: String,
    #[serde(default)]
    pub arguments: Vec<String>,
    #[serde(default)]
    pub auto_upload: bool,
}
#[derive(Clone)]
pub struct ExternalEditorService {
    inner: Arc<Inner>,
}
struct Inner {
    root: PathBuf,
    ssh: Arc<SshService>,
    queue: TransferCoordinator,
    sessions: Mutex<HashMap<String, EditorSession>>,
    opening: tokio::sync::Semaphore,
}
impl ExternalEditorService {
    pub fn new(root: PathBuf, ssh: Arc<SshService>, queue: TransferCoordinator) -> Self {
        Self {
            inner: Arc::new(Inner {
                root,
                ssh,
                queue,
                sessions: Mutex::new(HashMap::new()),
                opening: tokio::sync::Semaphore::new(1),
            }),
        }
    }
    pub async fn open(&self, request: EditorOpenRequest) -> Result<EditorSession, String> {
        let _opening = self
            .inner
            .opening
            .acquire()
            .await
            .map_err(|e| e.to_string())?;
        if self.inner.sessions.lock().await.len() >= 16 {
            return Err("close an external edit session before opening another".into());
        }
        if !Path::new(&request.executable).is_absolute()
            || !std::fs::metadata(&request.executable).is_ok_and(|m| m.is_file())
            || request.arguments.len() > 32
            || request
                .arguments
                .iter()
                .any(|a| a.len() > 4096 || a.contains('\0'))
        {
            return Err("select an absolute editor executable and bounded arguments".into());
        }
        let cancel = tokio_util::sync::CancellationToken::new();
        let (generation, remote_hash) = self
            .inner
            .ssh
            .file_content_version(&request.connection_id, &request.remote_path, cancel.clone())
            .await
            .map_err(|e| e.to_string())?;
        let expected = remote_hash.as_ref().ok_or("remote edit file was deleted")?;
        let id = uuid::Uuid::new_v4().to_string();
        let directory = self.inner.root.join(&id);
        tokio::fs::create_dir_all(&directory)
            .await
            .map_err(|e| e.to_string())?;
        let name = request
            .remote_path
            .rsplit('/')
            .next()
            .filter(|s| !s.is_empty() && !s.contains(['\\', ':']))
            .ok_or("remote filename cannot be represented locally")?;
        let local = directory.join(name);
        let receipt = self
            .inner
            .ssh
            .transfer_file(
                &request.connection_id,
                FileTransferRequest {
                    direction: FileTransferDirection::Download,
                    local_path: local.to_string_lossy().into(),
                    remote_path: request.remote_path.clone(),
                    conflict: FileConflictPolicy::Ask,
                },
                cancel.clone(),
                Arc::new(|_| {}),
            )
            .await
            .map_err(|e| e.to_string())?;
        if &receipt.sha256 != expected {
            return Err(format!(
                "remote file changed during download; local recovery file: {}",
                local.display()
            ));
        }
        let mut command = std::process::Command::new(&request.executable);
        command
            .args(&request.arguments)
            .arg(&local)
            .current_dir(&directory)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
        }
        let mut child = tokio::process::Command::from(command)
            .spawn()
            .map_err(|e| format!("open editor: {e}; local file: {}", local.display()))?;
        // Reap without coupling the editor lifetime to auto upload or queue state.
        tokio::spawn(async move {
            let _ = child.wait().await;
        });
        let session = EditorSession {
            id: id.clone(),
            connection_id: request.connection_id,
            remote_path: request.remote_path,
            local_path: local.to_string_lossy().into(),
            auto_upload: request.auto_upload,
            state: "watching".into(),
            error: None,
            task_id: None,
            generation,
            remote_hash,
            uploaded_hash: receipt.sha256,
            pending_hash: None,
            snapshot: None,
            revision: 1,
        };
        self.inner
            .sessions
            .lock()
            .await
            .insert(id.clone(), session.clone());
        let weak = Arc::downgrade(&self.inner);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                let Some(inner) = weak.upgrade() else { break };
                if !inner.sessions.lock().await.contains_key(&id) {
                    break;
                }
                if let Err(error) = inner.tick(&id).await {
                    if let Some(s) = inner.sessions.lock().await.get_mut(&id) {
                        s.auto_upload = false;
                        s.state = "paused".into();
                        s.error = Some(error);
                    }
                }
            }
        });
        Ok(session)
    }
    pub async fn list(&self) -> Vec<EditorSession> {
        self.inner.sessions.lock().await.values().cloned().collect()
    }
    pub async fn resolve(
        &self,
        id: &str,
        action: &str,
        path: Option<String>,
    ) -> Result<EditorSession, String> {
        let mut session = {
            let mut sessions = self.inner.sessions.lock().await;
            let session = sessions.get_mut(id).ok_or("edit session not found")?;
            if session.task_id.is_some() || session.state == "resolving" {
                return Err(
                    "wait for the current edit operation or cancel its upload in the queue".into(),
                );
            }
            session.revision += 1;
            session.auto_upload = false;
            session.state = "resolving".into();
            session.clone()
        };
        let resolution = async {
            match action {
                "pause" => {
                    session.auto_upload = false;
                    session.state = "paused".into();
                }
                "close" => {
                    let mut closed = self
                        .inner
                        .sessions
                        .lock()
                        .await
                        .remove(id)
                        .ok_or("edit session closed")?;
                    closed.auto_upload = false;
                    closed.state = "closed_local_retained".into();
                    return Ok(closed);
                }
                "resume" => {
                    session.auto_upload = true;
                    session.state = "watching".into();
                    session.error = None;
                }
                "overwrite" | "save_as" => {
                    let target = if action == "save_as" {
                        path.ok_or("Save As requires an absolute remote path")?
                    } else {
                        session.remote_path.clone()
                    };
                    let (generation, hash) = self
                        .inner
                        .ssh
                        .file_content_version(
                            &session.connection_id,
                            &target,
                            tokio_util::sync::CancellationToken::new(),
                        )
                        .await
                        .map_err(|e| e.to_string())?;
                    if action == "save_as" && hash.is_some() {
                        return Err("Save As target already exists; select another path".into());
                    }
                    session.remote_path = target;
                    session.generation = generation;
                    session.remote_hash = hash;
                    session.uploaded_hash.clear();
                    session.auto_upload = true;
                    session.error = None;
                    session.state = "watching".into();
                }
                "reload" => {
                    let fresh = Path::new(&session.local_path)
                        .with_extension(format!("remote-{}", uuid::Uuid::new_v4()));
                    let receipt = self
                        .inner
                        .ssh
                        .transfer_file(
                            &session.connection_id,
                            FileTransferRequest {
                                direction: FileTransferDirection::Download,
                                local_path: fresh.to_string_lossy().into(),
                                remote_path: session.remote_path.clone(),
                                conflict: FileConflictPolicy::Ask,
                            },
                            tokio_util::sync::CancellationToken::new(),
                            Arc::new(|_| {}),
                        )
                        .await
                        .map_err(|e| e.to_string())?;
                    // Preserve unsent edits. The user may copy/reopen the downloaded version.
                    session.auto_upload = false;
                    session.state = "paused".into();
                    session.error = Some(format!(
                        "downloaded remote version to {}; local edits retained at {}",
                        fresh.display(),
                        session.local_path
                    ));
                    let (generation, hash) = self
                        .inner
                        .ssh
                        .file_content_version(
                            &session.connection_id,
                            &session.remote_path,
                            tokio_util::sync::CancellationToken::new(),
                        )
                        .await
                        .map_err(|e| e.to_string())?;
                    if hash.as_ref() != Some(&receipt.sha256) {
                        return Err("remote changed while reloading; review downloaded file".into());
                    }
                    let backup = Path::new(&session.local_path)
                        .with_extension(format!("local-backup-{}", uuid::Uuid::new_v4()));
                    tokio::fs::rename(&session.local_path, &backup)
                        .await
                        .map_err(|e| e.to_string())?;
                    if let Err(error) = tokio::fs::rename(&fresh, &session.local_path).await {
                        let _ = tokio::fs::rename(&backup, &session.local_path).await;
                        return Err(format!(
                            "reload publication failed: {error}; backup: {}",
                            backup.display()
                        ));
                    }
                    session.uploaded_hash = receipt.sha256;
                    session.error = Some(format!(
                        "reloaded remote; original local edits retained at {}",
                        backup.display()
                    ));
                    session.generation = generation;
                    session.remote_hash = hash;
                }
                _ => return Err("unknown edit action".into()),
            }
            Ok(session.clone())
        }
        .await;
        let mut sessions = self.inner.sessions.lock().await;
        if let Some(current) = sessions
            .get_mut(id)
            .filter(|current| current.revision == session.revision)
        {
            if resolution.is_err() {
                session.auto_upload = false;
                session.state = "paused".into();
                session.error = resolution.as_ref().err().cloned();
            }
            *current = session;
        }
        resolution
    }
}
impl Inner {
    async fn tick(&self, id: &str) -> Result<(), String> {
        let Some(session) = self.sessions.lock().await.get(id).cloned() else {
            return Ok(());
        };
        if let Some(task_id) = &session.task_id {
            let tasks = self.queue.snapshot().await.map_err(|e| e.to_string())?;
            let task = tasks.iter().find(|t| &t.id == task_id);
            if task.is_some_and(|task| !task.state.terminal()) {
                return Ok(());
            }
            let mut sessions = self.sessions.lock().await;
            let Some(s) = sessions.get_mut(id) else {
                return Ok(());
            };
            s.task_id = None;
            let pending_hash = s.pending_hash.take();
            let Some(task) = task else {
                s.auto_upload = false;
                s.state = "paused".into();
                s.error = Some(
                    "upload receipt unavailable; reconcile the remote file before resuming".into(),
                );
                // Leave the snapshot and edited file on disk for recovery.
                s.snapshot = None;
                return Ok(());
            };
            if let (TransferState::Completed, Some(hash)) = (task.state, pending_hash) {
                s.uploaded_hash = hash.clone();
                s.remote_hash = Some(hash);
                s.state = "watching".into();
            } else {
                s.auto_upload = false;
                s.state = "conflict".into();
                s.error = Some(
                    task.error
                        .as_ref()
                        .map(|e| e.to_string())
                        .unwrap_or("upload stopped; review destination".into()),
                );
            }
            if let Some(snapshot) = s.snapshot.take() {
                let _ = tokio::fs::remove_file(snapshot).await;
            }
            return Ok(());
        }
        if !session.auto_upload {
            return Ok(());
        }
        let metadata = tokio::fs::symlink_metadata(&session.local_path)
            .await
            .map_err(|e| e.to_string())?;
        if !metadata.is_file()
            || crate::file_workspace::local_link(&metadata)
            || metadata.len() > MAX_BYTES
        {
            return Err("editor file must remain a regular file no larger than 32 MiB".into());
        }
        use tokio::io::AsyncReadExt;
        let file = tokio::fs::File::open(&session.local_path)
            .await
            .map_err(|e| e.to_string())?;
        let mut data = Vec::new();
        file.take(MAX_BYTES + 1)
            .read_to_end(&mut data)
            .await
            .map_err(|e| e.to_string())?;
        if data.len() as u64 > MAX_BYTES {
            return Err("editor content exceeds 32 MiB".into());
        }
        let hash = format!("{:x}", Sha256::digest(&data));
        if hash == session.uploaded_hash {
            return Ok(());
        }
        let after = tokio::fs::symlink_metadata(&session.local_path)
            .await
            .map_err(|e| e.to_string())?;
        if !after.is_file()
            || crate::file_workspace::local_link(&after)
            || after.len() != metadata.len()
            || after.modified().ok() != metadata.modified().ok()
        {
            return Ok(());
        }
        let snapshot = self
            .root
            .join(id)
            .join(format!(".upload-{}", uuid::Uuid::new_v4()));
        tokio::fs::write(&snapshot, data)
            .await
            .map_err(|e| e.to_string())?;
        let mut sessions = self.sessions.lock().await;
        let Some(s) = sessions
            .get_mut(id)
            .filter(|s| s.auto_upload && s.revision == session.revision && s.task_id.is_none())
        else {
            let _ = tokio::fs::remove_file(snapshot).await;
            return Ok(());
        };
        let admitted = self
            .queue
            .submit_operation(WorkbenchOperation::EditUpload {
                request: ConditionalUploadRequest {
                    connection_id: s.connection_id.clone(),
                    connection_generation: s.generation,
                    local_path: snapshot.to_string_lossy().into(),
                    remote_path: s.remote_path.clone(),
                    expected_sha256: s.remote_hash.clone(),
                },
            })
            .await;
        let task = match admitted {
            Ok(task) => task,
            Err(error) => {
                let _ = tokio::fs::remove_file(&snapshot).await;
                return Err(error.to_string());
            }
        };
        s.task_id = Some(task.id);
        s.pending_hash = Some(hash);
        s.snapshot = Some(snapshot);
        s.state = "uploading".into();
        Ok(())
    }
}
