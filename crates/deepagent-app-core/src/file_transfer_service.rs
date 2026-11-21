//! The desktop's single transfer queue. Reuses application SQLite documents
//! and CancellationTree; transport remains in the SSH adapter, never in UI.
use deepagent_core::{
    clock::{Clock, SystemClock},
    error::{CoreError, Result},
};
use deepagent_persistence::Database;
use deepagent_runtime::CancellationTree;
use deepagent_ssh::{
    DirectoryConflictPolicy, DirectoryFileConflict, DirectoryFileConflictResolver,
    DirectoryRootConflict, DirectoryTransferOptions, DirectoryTransferProgress,
    DirectoryTransferReceipt, DirectoryWalkLimits, FileConflictDecision, FileConflictPolicy,
    FileTransferError, FileTransferProgress, FileTransferReceipt, FileTransferRequest, SshService,
};
use serde::{Deserialize, Serialize};
use std::time::Instant;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, Weak},
};
use tokio::sync::{oneshot, watch, Semaphore};

#[path = "file_transfer_store.rs"]
mod storage;

const COLLECTION: &str = "remote_transfers_v1";
const LIMIT: usize = 128;
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransferKind {
    #[default]
    File,
    Directory,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TransferRequest {
    pub connection_id: String,
    pub transfer: FileTransferRequest,
    #[serde(default)]
    pub kind: TransferKind,
    #[serde(default)]
    pub directory_conflict: DirectoryConflictPolicy,
}
/// Existing transfers retain their original wire shape and persisted history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum FileOperationRequest {
    Transfer(TransferRequest),
    Workbench(crate::file_operations::WorkbenchOperation),
}

impl FileOperationRequest {
    pub fn connection_id(&self) -> &str {
        match self {
            Self::Transfer(request) => &request.connection_id,
            Self::Workbench(operation) => operation.connection_id(),
        }
    }

    pub fn as_transfer(&self) -> Option<&TransferRequest> {
        match self {
            Self::Transfer(request) => Some(request),
            Self::Workbench(_) => None,
        }
    }

    fn connections(&self) -> Vec<String> {
        match self {
            Self::Workbench(operation) => operation.connections(),
            _ => vec![self.connection_id().to_owned()],
        }
    }

    fn initial_file(&self) -> Option<String> {
        self.as_transfer()
            .filter(|request| request.kind == TransferKind::File)
            .map(|request| request.transfer.remote_path.clone())
    }

    fn same_work(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Transfer(left), Self::Transfer(right)) => {
                left.kind == right.kind
                    && left.connection_id == right.connection_id
                    && left.transfer.direction == right.transfer.direction
                    && left.transfer.local_path == right.transfer.local_path
                    && left.transfer.remote_path == right.transfer.remote_path
            }
            (Self::Workbench(left), Self::Workbench(right)) => left == right,
            _ => false,
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransferState {
    Queued,
    Running,
    Cancelling,
    Completed,
    Failed,
    Cancelled,
}
impl TransferState {
    pub fn terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PendingConflict<C> {
    pub version: u32,
    pub id: String,
    pub attempt: u32,
    pub conflict: C,
}
pub type PendingFileConflict = PendingConflict<DirectoryFileConflict>;
pub type PendingDirectoryConflict = PendingConflict<DirectoryRootConflict>;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConflictResponse<D> {
    pub version: u32,
    pub id: String,
    pub attempt: u32,
    pub decision: D,
}
pub type FileConflictResponse = ConflictResponse<FileConflictDecision>;
pub type DirectoryConflictResponse = ConflictResponse<DirectoryConflictPolicy>;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedConflict<C, D> {
    pub pending: PendingConflict<C>,
    pub decision: D,
}
pub type ResolvedFileConflict = ResolvedConflict<DirectoryFileConflict, FileConflictDecision>;
pub type ResolvedDirectoryConflict =
    ResolvedConflict<DirectoryRootConflict, DirectoryConflictPolicy>;
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransferTask {
    pub id: String,
    pub request: FileOperationRequest,
    pub attempt: u32,
    pub state: TransferState,
    pub bytes: String,
    pub total: Option<String>,
    pub revision: u64,
    pub updated_ms: i64,
    /// Single remote file path or current relative path within a directory task.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_file: Option<String>,
    /// Known file count for this task; single-file transfers are always 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_files: Option<u32>,
    #[serde(default)]
    pub completed_files: u32,
    #[serde(default)]
    pub skipped_files: u32,
    /// Average payload bytes per second since Running, excluding queue wait.
    /// Kept exact across IPC and cleared when the task reaches a terminal state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speed_bps: Option<String>,
    #[serde(skip)]
    started_at: Option<Instant>,
    pub receipt: Option<FileTransferReceipt>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub directory_receipt: Option<DirectoryTransferReceipt>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_receipt: Option<serde_json::Value>,
    pub error: Option<FileTransferError>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_file_conflict: Option<PendingFileConflict>,
    /// Bounded audit of the latest explicit decision in this attempt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_file_conflict: Option<ResolvedFileConflict>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_directory_conflict: Option<PendingDirectoryConflict>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_directory_conflict: Option<ResolvedDirectoryConflict>,
}
impl TransferTask {
    fn has_pending_conflict(&self) -> bool {
        self.pending_file_conflict.is_some() || self.pending_directory_conflict.is_some()
    }
    fn clear_pending_conflict(&mut self) -> bool {
        let file = self.pending_file_conflict.take().is_some();
        self.pending_directory_conflict.take().is_some() || file
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransferSnapshot {
    /// Opaque host-generation/revision cursor; never a JavaScript number.
    pub revision: String,
    pub tasks: Vec<TransferTask>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Document {
    version: u32,
    tasks: Vec<TransferTask>,
}
#[async_trait::async_trait]
pub trait TransferExecutor: Send + Sync {
    async fn execute(
        &self,
        connection_id: &str,
        request: FileTransferRequest,
        cancel: CancellationTree,
        progress: Arc<dyn Fn(FileTransferProgress) + Send + Sync>,
    ) -> std::result::Result<FileTransferReceipt, FileTransferError>;
    async fn execute_directory(
        &self,
        _connection_id: &str,
        _request: FileTransferRequest,
        _conflict: DirectoryConflictPolicy,
        _cancel: CancellationTree,
        _progress: Arc<dyn Fn(DirectoryTransferProgress) + Send + Sync>,
        _file_conflicts: Option<Arc<dyn DirectoryFileConflictResolver>>,
    ) -> std::result::Result<DirectoryTransferReceipt, FileTransferError> {
        Err(FileTransferError::new(
            "unsupported",
            "executor does not support directory transfers",
        ))
    }
    async fn execute_workbench(
        &self,
        _request: crate::file_operations::WorkbenchOperation,
        _cancel: CancellationTree,
        _progress: Arc<dyn Fn(FileTransferProgress) + Send + Sync>,
    ) -> std::result::Result<serde_json::Value, FileTransferError> {
        Err(FileTransferError::new(
            "unsupported",
            "executor does not support workbench operations",
        ))
    }
}
#[async_trait::async_trait]
impl TransferExecutor for SshService {
    async fn execute(
        &self,
        id: &str,
        request: FileTransferRequest,
        cancel: CancellationTree,
        progress: Arc<dyn Fn(FileTransferProgress) + Send + Sync>,
    ) -> std::result::Result<FileTransferReceipt, FileTransferError> {
        self.transfer_file(id, request, cancel.token(), progress)
            .await
    }
    async fn execute_directory(
        &self,
        id: &str,
        request: FileTransferRequest,
        conflict: DirectoryConflictPolicy,
        cancel: CancellationTree,
        progress: Arc<dyn Fn(DirectoryTransferProgress) + Send + Sync>,
        file_conflicts: Option<Arc<dyn DirectoryFileConflictResolver>>,
    ) -> std::result::Result<DirectoryTransferReceipt, FileTransferError> {
        self.transfer_directory_with_options(
            id,
            request,
            DirectoryTransferOptions {
                conflict,
                limits: DirectoryWalkLimits::default(),
                cancel: cancel.token(),
                progress,
                file_conflicts,
            },
        )
        .await
    }
    async fn execute_workbench(
        &self,
        request: crate::file_operations::WorkbenchOperation,
        cancel: CancellationTree,
        progress: Arc<dyn Fn(FileTransferProgress) + Send + Sync>,
    ) -> std::result::Result<serde_json::Value, FileTransferError> {
        use crate::file_operations::WorkbenchOperation::*;
        let value = match request {
            Chmod { request } => serde_json::to_value(
                self.execute_chmod(
                    request,
                    DirectoryWalkLimits::default(),
                    cancel.token(),
                    Arc::new(move |p| {
                        progress(FileTransferProgress {
                            bytes: u64::from(p.changed_entries),
                            total: Some(u64::from(p.total_entries)),
                        })
                    }),
                )
                .await?,
            ),
            Copy { request } => serde_json::to_value(
                self.execute_server_copy(request, cancel.token(), progress)
                    .await?,
            ),
            Extract { request } => return self.execute_archive(request, cancel.token()).await,
            EditUpload { request } => serde_json::to_value(
                self.conditional_upload(request, cancel.token(), progress)
                    .await?,
            ),
        };
        value.map_err(|e| FileTransferError::new("invalid", &e.to_string()))
    }
}
enum ExecutionReceipt {
    File(FileTransferReceipt),
    Directory(DirectoryTransferReceipt),
    Workbench(serde_json::Value),
}

struct ExecutionFailure {
    error: FileTransferError,
    receipt: Option<ExecutionReceipt>,
}

impl From<FileTransferError> for ExecutionFailure {
    fn from(error: FileTransferError) -> Self {
        Self {
            error,
            receipt: None,
        }
    }
}
enum ProgressUpdate {
    File(FileTransferProgress),
    Directory(DirectoryTransferProgress),
}
struct State {
    document: Document,
    storage: storage::QueueStorage,
    tokens: HashMap<String, CancellationTree>,
    lanes: HashMap<String, Weak<Semaphore>>,
    decision_waiters: HashMap<String, oneshot::Sender<ConflictDecision>>,
    recovery_required: bool,
}
struct Inner {
    db: Arc<Database>,
    executor: Arc<dyn TransferExecutor>,
    state: Mutex<State>,
    global: Arc<Semaphore>,
    root: CancellationTree,
    generation: String,
    changes: watch::Sender<u64>,
}
struct Owner(CancellationTree);
impl Drop for Owner {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

/// Holds no strong queue/owner reference while a user is deciding.
struct QueueConflictResolver {
    inner: Weak<Inner>,
    task_id: String,
    attempt: u32,
    cancel: CancellationTree,
}

enum ConflictRequest {
    File(DirectoryFileConflict),
    Directory(DirectoryRootConflict),
}

#[derive(Clone, Copy)]
enum ConflictDecision {
    File(FileConflictDecision),
    Directory(DirectoryConflictPolicy),
}

impl QueueConflictResolver {
    async fn wait(
        &self,
        conflict: ConflictRequest,
    ) -> std::result::Result<ConflictDecision, FileTransferError> {
        if self.cancel.is_cancelled() {
            return Err(FileTransferError::new(
                "cancelled",
                "conflict decision cancelled",
            ));
        }
        let receiver = self
            .inner
            .upgrade()
            .ok_or_else(|| FileTransferError::new("cancelled", "transfer host stopped"))?
            .begin_conflict(&self.task_id, self.attempt, conflict)
            .map_err(|error| {
                if self.cancel.is_cancelled() {
                    FileTransferError::new("cancelled", "conflict decision cancelled")
                } else {
                    FileTransferError::new("persistence", &error.to_string())
                }
            })?;
        tokio::select! {
            biased;
            _ = self.cancel.cancelled() => Err(FileTransferError::new("cancelled", "conflict decision cancelled")),
            decision = receiver => decision.map_err(|_| FileTransferError::new("cancelled", "conflict decision closed")),
        }
    }
}

#[async_trait::async_trait]
impl DirectoryFileConflictResolver for QueueConflictResolver {
    async fn resolve(
        &self,
        conflict: DirectoryFileConflict,
    ) -> std::result::Result<FileConflictDecision, FileTransferError> {
        match self.wait(ConflictRequest::File(conflict)).await? {
            ConflictDecision::File(decision) => Ok(decision),
            _ => Err(FileTransferError::new(
                "invalid",
                "file decision kind changed",
            )),
        }
    }
    async fn resolve_root(
        &self,
        conflict: DirectoryRootConflict,
    ) -> std::result::Result<DirectoryConflictPolicy, FileTransferError> {
        match self.wait(ConflictRequest::Directory(conflict)).await? {
            ConflictDecision::Directory(decision) => Ok(decision),
            _ => Err(FileTransferError::new(
                "invalid",
                "directory decision kind changed",
            )),
        }
    }
}
#[derive(Clone)]
pub struct TransferCoordinator {
    inner: Arc<Inner>,
    _owner: Arc<Owner>,
}
fn invalid(message: &str) -> CoreError {
    CoreError::invalid(message)
}
fn healthy(state: &State) -> Result<()> {
    if state.recovery_required {
        return Err(invalid(
            "transfer state could not be saved; reload host and review destination before retrying",
        ));
    }
    Ok(())
}

impl TransferCoordinator {
    pub fn new(db: Arc<Database>, executor: Arc<dyn TransferExecutor>) -> Result<Self> {
        let (mut document, storage) = storage::QueueStorage::load(&db)?;
        let mut changed = false;
        for task in &mut document.tasks {
            if task.pending_file_conflict.is_some() && task.pending_directory_conflict.is_some() {
                return Err(invalid("multiple pending decisions for one transfer"));
            }
            let interrupted = !task.state.terminal();
            let had_live_speed = task.speed_bps.take().is_some();
            let had_pending_file = task.clear_pending_conflict();
            if interrupted {
                task.state = TransferState::Failed;
                task.error = Some(FileTransferError::new(
                    "interrupted",
                    "host stopped; retry explicitly after reviewing destination",
                ));
            }
            if interrupted || had_live_speed || had_pending_file {
                task.revision += 1;
                changed = true;
            }
        }
        let root = CancellationTree::new();
        let owner = Arc::new(Owner(root.clone()));
        let service = Self {
            inner: Arc::new(Inner {
                db,
                executor,
                state: Mutex::new(State {
                    document,
                    storage,
                    tokens: HashMap::new(),
                    lanes: HashMap::new(),
                    decision_waiters: HashMap::new(),
                    recovery_required: false,
                }),
                global: Arc::new(Semaphore::new(4)),
                root,
                generation: uuid::Uuid::new_v4().to_string(),
                changes: watch::channel(0).0,
            }),
            _owner: owner,
        };
        if changed {
            let mut state = service
                .inner
                .state
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            service.inner.persist(&mut state)?;
        }
        Ok(service)
    }
    pub async fn snapshot(&self) -> Result<Vec<TransferTask>> {
        let state = self.inner.state.lock().unwrap_or_else(|p| p.into_inner());
        healthy(&state)?;
        Ok(state.document.tasks.clone())
    }
    /// Subscribe before reading the snapshot so mutations cannot fall in a gap.
    /// A dropped/expired wait owns no worker and never cancels a transfer.
    pub async fn watch(
        &self,
        after_revision: Option<&str>,
        max_wait: std::time::Duration,
    ) -> Result<TransferSnapshot> {
        let mut changes = self.inner.changes.subscribe();
        let wait = async {
            loop {
                changes.borrow_and_update();
                let snapshot = self.inner.current_snapshot()?;
                if after_revision != Some(snapshot.revision.as_str()) {
                    return Ok(snapshot);
                }
                changes
                    .changed()
                    .await
                    .map_err(|_| invalid("transfer host stopped"))?;
            }
        };
        match tokio::time::timeout(max_wait, wait).await {
            Ok(result) => result,
            Err(_) => self.inner.current_snapshot(),
        }
    }
    /// Native drops carry paths, not trusted file kinds. Resolve upload sources
    /// before admission; persisted tasks and retries keep the existing typed contract.
    pub async fn submit_detected_upload(
        &self,
        mut request: TransferRequest,
    ) -> Result<TransferTask> {
        let local = std::path::Path::new(&request.transfer.local_path);
        if request.transfer.direction != deepagent_ssh::FileTransferDirection::Upload
            || !local.is_absolute()
            || local.file_name().is_none()
        {
            return Err(invalid(
                "local kind detection requires an absolute upload source",
            ));
        }
        let metadata = tokio::fs::symlink_metadata(local)
            .await
            .map_err(|error| invalid(&format!("cannot inspect local upload source: {error}")))?;
        #[cfg(windows)]
        let is_link = {
            use std::os::windows::fs::MetadataExt;
            metadata.file_attributes()
                & windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT
                != 0
        };
        #[cfg(not(windows))]
        let is_link = metadata.is_symlink();
        request.kind = if is_link {
            return Err(invalid(
                "local upload source must not be a link or reparse point",
            ));
        } else if metadata.is_file() {
            TransferKind::File
        } else if metadata.is_dir() {
            TransferKind::Directory
        } else {
            return Err(invalid(
                "local upload source must be a regular file or directory",
            ));
        };
        self.submit(request).await
    }

    pub async fn submit(&self, mut request: TransferRequest) -> Result<TransferTask> {
        if request.connection_id.is_empty()
            || request.connection_id.len() > 256
            || !std::path::Path::new(&request.transfer.local_path).is_absolute()
            || !request.transfer.remote_path.starts_with('/')
            || request.transfer.remote_path.len() > 16 * 1024
            || request.transfer.remote_path.contains('\0')
        {
            return Err(invalid("invalid transfer endpoints"));
        }
        // Canonicalize the local parent (never POSIX remote paths using Path).
        let local = std::path::PathBuf::from(&request.transfer.local_path);
        let parent = local
            .parent()
            .ok_or_else(|| invalid("local filename required"))?;
        let canonical = tokio::fs::canonicalize(parent)
            .await
            .map_err(|_| invalid("local parent directory is unavailable"))?;
        request.transfer.local_path = canonical
            .join(
                local
                    .file_name()
                    .ok_or_else(|| invalid("local filename required"))?,
            )
            .to_string_lossy()
            .into();
        self.admit(FileOperationRequest::Transfer(request))
    }

    pub async fn submit_operation(
        &self,
        operation: crate::file_operations::WorkbenchOperation,
    ) -> Result<TransferTask> {
        operation.validate().map_err(|e| invalid(&e.to_string()))?;
        self.admit(FileOperationRequest::Workbench(operation))
    }

    fn admit(&self, request: FileOperationRequest) -> Result<TransferTask> {
        let task = {
            let mut state = self.inner.state.lock().unwrap_or_else(|p| p.into_inner());
            healthy(&state)?;
            if let Some(task) = state
                .document
                .tasks
                .iter()
                .find(|t| !t.state.terminal() && t.request.same_work(&request))
            {
                return Ok(task.clone());
            }
            if state.document.tasks.len() >= LIMIT {
                return Err(invalid("transfer history is full; clear finished tasks"));
            }
            let current_file = request.initial_file();
            let total_files = current_file.as_ref().map(|_| 1);
            let task = TransferTask {
                id: uuid::Uuid::new_v4().to_string(),
                request,
                attempt: 1,
                state: TransferState::Queued,
                bytes: "0".into(),
                total: None,
                revision: 1,
                updated_ms: SystemClock.now().as_millis(),
                current_file,
                total_files,
                completed_files: 0,
                skipped_files: 0,
                speed_bps: None,
                started_at: None,
                receipt: None,
                directory_receipt: None,
                operation_receipt: None,
                error: None,
                pending_file_conflict: None,
                last_file_conflict: None,
                pending_directory_conflict: None,
                last_directory_conflict: None,
            };
            let before = state.document.clone();
            state.document.tasks.push(task.clone());
            if let Err(e) = self.inner.persist(&mut state) {
                state.document = before;
                return Err(e);
            }
            task
        };
        self.launch(&task);
        Ok(task)
    }
    pub async fn cancel(&self, id: &str) -> Result<TransferTask> {
        let mut state = self.inner.state.lock().unwrap_or_else(|p| p.into_inner());
        healthy(&state)?;
        let index = state
            .document
            .tasks
            .iter()
            .position(|t| t.id == id)
            .ok_or_else(|| invalid("transfer not found"))?;
        if state.document.tasks[index].state.terminal() {
            return Ok(state.document.tasks[index].clone());
        }
        let before = state.document.clone();
        let queued = state.document.tasks[index].state == TransferState::Queued;
        state.document.tasks[index].state = if queued {
            TransferState::Cancelled
        } else {
            TransferState::Cancelling
        };
        state.document.tasks[index].revision += 1;
        state.document.tasks[index].clear_pending_conflict();
        state.document.tasks[index].speed_bps = None;
        if let Err(e) = self.inner.persist(&mut state) {
            state.document = before;
            return Err(e);
        }
        if let Some(token) = state.tokens.get(id) {
            token.cancel();
        }
        state.decision_waiters.remove(id);
        if queued {
            state.tokens.remove(id);
        }
        Ok(state.document.tasks[index].clone())
    }

    pub async fn resolve_file_conflict(
        &self,
        id: &str,
        response: FileConflictResponse,
    ) -> Result<TransferTask> {
        self.resolve_conflict(
            id,
            response.version,
            &response.id,
            response.attempt,
            ConflictDecision::File(response.decision),
        )
        .await
    }

    pub async fn resolve_directory_conflict(
        &self,
        id: &str,
        response: DirectoryConflictResponse,
    ) -> Result<TransferTask> {
        self.resolve_conflict(
            id,
            response.version,
            &response.id,
            response.attempt,
            ConflictDecision::Directory(response.decision),
        )
        .await
    }

    async fn resolve_conflict(
        &self,
        id: &str,
        version: u32,
        nonce: &str,
        attempt: u32,
        decision: ConflictDecision,
    ) -> Result<TransferTask> {
        let mut state = self.inner.state.lock().unwrap_or_else(|p| p.into_inner());
        healthy(&state)?;
        let index = state
            .document
            .tasks
            .iter()
            .position(|t| t.id == id)
            .ok_or_else(|| invalid("transfer not found"))?;
        let task = &state.document.tasks[index];
        let pending = match decision {
            ConflictDecision::File(choice) if choice.policy != FileConflictPolicy::Ask => task
                .pending_file_conflict
                .as_ref()
                .map(|p| (p.version, p.id.as_str(), p.attempt)),
            ConflictDecision::Directory(policy) if policy != DirectoryConflictPolicy::Ask => task
                .pending_directory_conflict
                .as_ref()
                .map(|p| (p.version, p.id.as_str(), p.attempt)),
            _ => None,
        }
        .ok_or_else(|| invalid("no pending conflict matching the decision kind"))?;
        if version != 1
            || version != pending.0
            || nonce != pending.1
            || attempt != task.attempt
            || pending.2 != task.attempt
            || task.state != TransferState::Running
            || !state
                .tokens
                .get(id)
                .is_some_and(|token| !token.is_cancelled())
            || !state
                .decision_waiters
                .get(id)
                .is_some_and(|sender| !sender.is_closed())
        {
            return Err(invalid("stale or invalid conflict response"));
        }
        let before = state.document.clone();
        let task = &mut state.document.tasks[index];
        let (kind, policy, remaining) = match decision {
            ConflictDecision::File(choice) => {
                task.last_file_conflict = Some(ResolvedFileConflict {
                    pending: task.pending_file_conflict.take().unwrap(),
                    decision: choice,
                });
                (
                    "file",
                    format!("{:?}", choice.policy),
                    choice.apply_to_remaining_files,
                )
            }
            ConflictDecision::Directory(policy) => {
                task.last_directory_conflict = Some(ResolvedDirectoryConflict {
                    pending: task.pending_directory_conflict.take().unwrap(),
                    decision: policy,
                });
                ("directory_root", format!("{policy:?}"), false)
            }
        };
        task.revision += 1;
        task.updated_ms = SystemClock.now().as_millis();
        // Durable consent precedes any resumed filesystem/network operation.
        if let Err(error) = self.inner.persist(&mut state) {
            state.document = before;
            return Err(error);
        }
        let sender = state.decision_waiters.remove(id).unwrap();
        tracing::info!(
            task_id = id,
            attempt,
            kind,
            policy,
            remaining_files = remaining,
            "remote conflict decision saved"
        );
        sender
            .send(decision)
            .map_err(|_| invalid("conflict decision wait ended"))?;
        Ok(state.document.tasks[index].clone())
    }
    pub async fn retry(&self, id: &str, conflict: FileConflictPolicy) -> Result<TransferTask> {
        self.retry_with_policies(id, conflict, None).await
    }
    pub async fn retry_with_policies(
        &self,
        id: &str,
        conflict: FileConflictPolicy,
        directory_conflict: Option<DirectoryConflictPolicy>,
    ) -> Result<TransferTask> {
        self.retry_operation(id, |request| {
            let FileOperationRequest::Transfer(request) = request else {
                return Err(invalid(
                    "workbench retry requires a new preview and confirmation",
                ));
            };
            if directory_conflict.is_some() && request.kind != TransferKind::Directory {
                return Err(invalid("directory policy requires a directory task"));
            }
            request.transfer.conflict = conflict;
            if let Some(policy) = directory_conflict {
                request.directory_conflict = policy;
            }
            Ok(())
        })
    }

    fn retry_operation(
        &self,
        id: &str,
        update: impl FnOnce(&mut FileOperationRequest) -> Result<()>,
    ) -> Result<TransferTask> {
        let task = {
            let mut state = self.inner.state.lock().unwrap_or_else(|p| p.into_inner());
            healthy(&state)?;
            let index = state
                .document
                .tasks
                .iter()
                .position(|t| t.id == id)
                .ok_or_else(|| invalid("transfer not found"))?;
            if !matches!(
                state.document.tasks[index].state,
                TransferState::Failed | TransferState::Cancelled
            ) {
                return Err(invalid("only failed/cancelled transfers can be retried"));
            }
            let mut request = state.document.tasks[index].request.clone();
            update(&mut request)?;
            let before = state.document.clone();
            let task = &mut state.document.tasks[index];
            task.attempt = task
                .attempt
                .checked_add(1)
                .ok_or_else(|| invalid("attempt limit reached"))?;
            task.request = request;
            task.state = TransferState::Queued;
            task.error = None;
            task.receipt = None;
            task.directory_receipt = None;
            task.operation_receipt = None;
            task.bytes = "0".into();
            task.total = None;
            task.completed_files = 0;
            task.skipped_files = 0;
            task.current_file = task.request.initial_file();
            task.total_files = task.current_file.as_ref().map(|_| 1);
            task.speed_bps = None;
            task.started_at = None;
            task.clear_pending_conflict();
            task.last_file_conflict = None;
            task.last_directory_conflict = None;
            task.revision += 1;
            let result = task.clone();
            if let Err(e) = self.inner.persist(&mut state) {
                state.document = before;
                return Err(e);
            }
            result
        };
        self.launch(&task);
        Ok(task)
    }
    pub async fn clear_finished(&self) -> Result<()> {
        let mut state = self.inner.state.lock().unwrap_or_else(|p| p.into_inner());
        healthy(&state)?;
        let before = state.document.clone();
        state.document.tasks.retain(|t| !t.state.terminal());
        if let Err(e) = self.inner.persist(&mut state) {
            state.document = before;
            return Err(e);
        }
        Ok(())
    }
    fn launch(&self, task: &TransferTask) {
        let (lanes, cancel) = {
            let mut state = self.inner.state.lock().unwrap_or_else(|p| p.into_inner());
            state.lanes.retain(|_, v| v.strong_count() > 0);
            // Both endpoints are locked in a stable order: A→B and B→A cannot
            // deadlock or publish concurrently with an upload on B.
            let lanes: Vec<_> = task
                .request
                .connections()
                .into_iter()
                .map(|id| {
                    state
                        .lanes
                        .get(&id)
                        .and_then(Weak::upgrade)
                        .unwrap_or_else(|| {
                            let lane = Arc::new(Semaphore::new(1));
                            state.lanes.insert(id, Arc::downgrade(&lane));
                            lane
                        })
                })
                .collect();
            let cancel = self.inner.root.child();
            state.tokens.insert(task.id.clone(), cancel.clone());
            (lanes, cancel)
        };
        let weak = Arc::downgrade(&self.inner);
        let task = task.clone();
        let global = self.inner.global.clone();
        let executor = self.inner.executor.clone();
        tokio::spawn(async move {
            let permits = async {
                let mut held = Vec::new();
                for lane in lanes {
                    held.push(lane.acquire_owned().await.ok()?);
                }
                let global = global.acquire_owned().await.ok()?;
                Some((held, global))
            };
            let _permits = tokio::select! {_ = cancel.cancelled()=>return,permits=permits=>match permits{Some(p)=>p,None=>return}};
            let Some(inner) = weak.upgrade() else { return };
            if let Err(error) =
                inner.transition(&task.id, task.attempt, TransferState::Running, None, None)
            {
                tracing::warn!(%error,"transfer could not start durably");
                return;
            }
            drop(inner);
            let progress_weak = weak.clone();
            let id = task.id.clone();
            let attempt = task.attempt;
            let progress = Arc::new(move |progress: ProgressUpdate| {
                if let Some(inner) = progress_weak.upgrade() {
                    inner.progress(&id, attempt, progress);
                }
            });
            let result = match task.request {
                FileOperationRequest::Workbench(request) => executor
                    .execute_workbench(
                        request,
                        cancel,
                        Arc::new(move |p| progress(ProgressUpdate::File(p))),
                    )
                    .await
                    .map(ExecutionReceipt::Workbench)
                    .map_err(ExecutionFailure::from),
                FileOperationRequest::Transfer(request) if request.kind == TransferKind::File => {
                    executor
                        .execute(
                            &request.connection_id,
                            request.transfer,
                            cancel,
                            Arc::new(move |p| progress(ProgressUpdate::File(p))),
                        )
                        .await
                        .map(ExecutionReceipt::File)
                        .map_err(ExecutionFailure::from)
                }
                FileOperationRequest::Transfer(request) => executor
                    .execute_directory(
                        &request.connection_id,
                        request.transfer,
                        request.directory_conflict,
                        cancel.clone(),
                        Arc::new(move |p| progress(ProgressUpdate::Directory(p))),
                        Some(Arc::new(QueueConflictResolver {
                            inner: weak.clone(),
                            task_id: task.id.clone(),
                            attempt: task.attempt,
                            cancel,
                        })),
                    )
                    .await
                    .map(ExecutionReceipt::Directory)
                    .map_err(ExecutionFailure::from),
            };
            if let Some(inner) = weak.upgrade() {
                let (state, receipt, error) = match result {
                    Ok(receipt) => (TransferState::Completed, Some(receipt), None),
                    Err(failure) => (
                        if failure.error.code == "cancelled" {
                            TransferState::Cancelled
                        } else {
                            TransferState::Failed
                        },
                        failure.receipt,
                        Some(failure.error),
                    ),
                };
                if let Err(error) = inner.transition(&task.id, task.attempt, state, receipt, error)
                {
                    tracing::warn!(%error,"transfer terminal persistence failed; host requires reconciliation");
                }
            }
        });
    }
}
impl Inner {
    fn begin_conflict(
        &self,
        id: &str,
        attempt: u32,
        conflict: ConflictRequest,
    ) -> Result<oneshot::Receiver<ConflictDecision>> {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        healthy(&state)?;
        let index = state
            .document
            .tasks
            .iter()
            .position(|t| t.id == id && t.attempt == attempt)
            .ok_or_else(|| invalid("stale transfer attempt"))?;
        let task = &state.document.tasks[index];
        if task.state != TransferState::Running
            || !task
                .request
                .as_transfer()
                .is_some_and(|request| request.kind == TransferKind::Directory)
            || task.has_pending_conflict()
            || !state
                .tokens
                .get(id)
                .is_some_and(|token| !token.is_cancelled())
        {
            return Err(invalid("transfer cannot wait for a conflict decision"));
        }
        let before = state.document.clone();
        let task = &mut state.document.tasks[index];
        let nonce = uuid::Uuid::new_v4().to_string();
        let kind = match conflict {
            ConflictRequest::File(conflict) => {
                task.pending_file_conflict = Some(PendingConflict {
                    version: 1,
                    id: nonce,
                    attempt,
                    conflict,
                });
                "file"
            }
            ConflictRequest::Directory(conflict) => {
                task.pending_directory_conflict = Some(PendingConflict {
                    version: 1,
                    id: nonce,
                    attempt,
                    conflict,
                });
                "directory_root"
            }
        };
        task.speed_bps = None;
        task.revision += 1;
        task.updated_ms = SystemClock.now().as_millis();
        if let Err(error) = self.persist(&mut state) {
            state.document = before;
            return Err(error);
        }
        let (sender, receiver) = oneshot::channel();
        state.decision_waiters.insert(id.into(), sender);
        tracing::info!(
            task_id = id,
            attempt,
            kind,
            "remote transfer awaiting conflict decision"
        );
        Ok(receiver)
    }

    fn progress(&self, id: &str, attempt: u32, update: ProgressUpdate) {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(task) = state
            .document
            .tasks
            .iter_mut()
            .find(|t| t.id == id && t.attempt == attempt && !t.state.terminal())
        {
            let (bytes, total) = match update {
                ProgressUpdate::File(p) => (p.bytes, p.total),
                ProgressUpdate::Directory(p) => {
                    task.current_file = p.current_file;
                    task.total_files = Some(p.total_files);
                    task.completed_files = p.completed_files;
                    task.skipped_files = p.skipped_files;
                    (p.bytes, Some(p.total))
                }
            };
            task.bytes = bytes.to_string();
            task.total = total.map(|v| v.to_string());
            let started = task.started_at.get_or_insert_with(Instant::now);
            task.speed_bps = Some(average_speed(bytes, started.elapsed()));
            task.revision += 1;
            task.updated_ms = SystemClock.now().as_millis();
            self.notify();
        }
    }
    fn notify(&self) {
        self.changes
            .send_modify(|revision| *revision = revision.wrapping_add(1));
    }
    fn current_snapshot(&self) -> Result<TransferSnapshot> {
        let state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        healthy(&state)?;
        Ok(TransferSnapshot {
            revision: format!("{}:{}", self.generation, *self.changes.borrow()),
            tasks: state.document.tasks.clone(),
        })
    }
    fn persist(&self, state: &mut State) -> Result<()> {
        state
            .storage
            .save(&self.db, &state.document.tasks, SystemClock.now())?;
        self.notify();
        Ok(())
    }
    fn transition(
        &self,
        id: &str,
        attempt: u32,
        next: TransferState,
        receipt: Option<ExecutionReceipt>,
        error: Option<FileTransferError>,
    ) -> Result<()> {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        let index = state
            .document
            .tasks
            .iter()
            .position(|t| t.id == id && t.attempt == attempt)
            .ok_or_else(|| invalid("stale transfer attempt"))?;
        let old = state.document.tasks[index].state;
        if next == TransferState::Running {
            healthy(&state)?;
        }
        if old.terminal() || next == TransferState::Running && old != TransferState::Queued {
            return Err(invalid("invalid transfer transition"));
        }
        let before = state.document.clone();
        let task = &mut state.document.tasks[index];
        task.state = next;
        match receipt {
            Some(ExecutionReceipt::File(receipt)) => {
                task.bytes = receipt.bytes.clone();
                task.completed_files = u32::from(!receipt.skipped);
                task.skipped_files = u32::from(receipt.skipped);
                task.receipt = Some(receipt);
            }
            Some(ExecutionReceipt::Directory(receipt)) => {
                task.bytes = receipt.bytes.clone();
                task.total = Some(receipt.total_bytes.clone());
                task.total_files = Some(receipt.total_files);
                task.completed_files = receipt.completed_files;
                task.skipped_files = receipt.skipped_files;
                task.directory_receipt = Some(receipt);
            }
            Some(ExecutionReceipt::Workbench(receipt)) => {
                if let Some(bytes) = receipt.get("bytes").and_then(|v| v.as_str()) {
                    task.bytes = bytes.into();
                }
                task.operation_receipt = Some(receipt);
            }
            None => {}
        }
        task.error = error;
        task.revision += 1;
        task.updated_ms = SystemClock.now().as_millis();
        if next == TransferState::Running {
            task.started_at = Some(Instant::now());
            task.speed_bps = None;
            task.completed_files = 0;
        }
        if next.terminal() {
            task.started_at = None;
            task.speed_bps = None;
            task.clear_pending_conflict();
        }
        if let Err(e) = self.persist(&mut state) {
            state.document = before;
            // Network outcome may already be committed. Never show a fake running
            // task or admit more work after losing its durable terminal record.
            state.recovery_required = true;
            self.notify();
            return Err(e);
        }
        if next.terminal() {
            state.tokens.remove(id);
            state.decision_waiters.remove(id);
        }
        Ok(())
    }
}

fn average_speed(bytes: u64, elapsed: std::time::Duration) -> String {
    // u128 handles u64::MAX * 1000 without saturation or floating-point loss.
    // The one millisecond floor bounds the first/submillisecond estimate.
    (u128::from(bytes) * 1_000 / elapsed.as_millis().max(1)).to_string()
}

#[cfg(test)]
mod progress_tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn cancelled_file_decision_reports_cancellation_before_registration() {
        let dir = tempfile::tempdir().unwrap();
        let queue = TransferCoordinator::new(
            Arc::new(Database::open_in_memory().unwrap()),
            Arc::new(SshService::new(dir.path().to_owned())),
        )
        .unwrap();
        let cancel = CancellationTree::new();
        cancel.cancel();
        let resolver = QueueConflictResolver {
            inner: Arc::downgrade(&queue.inner),
            task_id: "cancelled-before-registration".into(),
            attempt: 1,
            cancel,
        };
        let error = resolver
            .resolve(DirectoryFileConflict {
                relative_path: "b".into(),
                destination_path: "/tree/b".into(),
                direction: deepagent_ssh::FileTransferDirection::Upload,
                source_size: "5".into(),
            })
            .await
            .unwrap_err();
        assert_eq!(error.code, "cancelled");
        assert!(queue.snapshot().await.unwrap().is_empty());
    }

    #[test]
    fn average_speed_preserves_large_byte_counts_and_bounds_submillisecond_samples() {
        assert_eq!(
            average_speed(u64::MAX, Duration::from_secs(2)),
            "9223372036854775807"
        );
        assert_eq!(
            average_speed(u64::MAX, Duration::from_millis(1)),
            "18446744073709551615000"
        );
        assert_eq!(average_speed(3, Duration::ZERO), "3000");
        assert_eq!(average_speed(0, Duration::from_secs(1)), "0");
    }
}
