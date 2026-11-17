//! The desktop's single transfer queue. Reuses application SQLite documents
//! and CancellationTree; transport remains in the SSH adapter, never in UI.
use deepagent_core::{
    clock::{Clock, SystemClock},
    error::{CoreError, Result},
};
use deepagent_persistence::{document_store::DocumentStore, Database};
use deepagent_runtime::CancellationTree;
use deepagent_ssh::{
    FileConflictPolicy, FileTransferError, FileTransferProgress, FileTransferReceipt,
    FileTransferRequest, SshService,
};
use serde::{Deserialize, Serialize};
use std::time::Instant;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, Weak},
};
use tokio::sync::{watch, Semaphore};

const COLLECTION: &str = "remote_transfers_v1";
const LIMIT: usize = 128;
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TransferRequest {
    pub connection_id: String,
    pub transfer: FileTransferRequest,
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
#[serde(rename_all = "camelCase")]
pub struct TransferTask {
    pub id: String,
    pub request: TransferRequest,
    pub attempt: u32,
    pub state: TransferState,
    pub bytes: String,
    pub total: Option<String>,
    pub revision: u64,
    pub updated_ms: i64,
    /// Remote path represented by this single-file task; absent in old history.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_file: Option<String>,
    /// Known file count for this task; single-file transfers are always 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_files: Option<u32>,
    #[serde(default)]
    pub completed_files: u32,
    /// Average payload bytes per second since Running, excluding queue wait.
    /// Kept exact across IPC and cleared when the task reaches a terminal state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speed_bps: Option<String>,
    #[serde(skip)]
    started_at: Option<Instant>,
    pub receipt: Option<FileTransferReceipt>,
    pub error: Option<FileTransferError>,
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
}
struct State {
    document: Document,
    body: Option<String>,
    tokens: HashMap<String, CancellationTree>,
    lanes: HashMap<String, Weak<Semaphore>>,
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
        let body = DocumentStore::new(&db).get_body_bounded(COLLECTION, "queue", 1024 * 1024)?;
        let mut document = body
            .as_deref()
            .map(serde_json::from_str::<Document>)
            .transpose()
            .map_err(|_| invalid("invalid transfer queue document"))?
            .unwrap_or(Document {
                version: 1,
                tasks: vec![],
            });
        if document.version != 1 || document.tasks.len() > LIMIT {
            return Err(invalid("unsupported or oversized transfer queue"));
        }
        let mut changed = false;
        for task in &mut document.tasks {
            let interrupted = !task.state.terminal();
            let had_live_speed = task.speed_bps.take().is_some();
            if interrupted {
                task.state = TransferState::Failed;
                task.error = Some(FileTransferError::new(
                    "interrupted",
                    "host stopped; retry explicitly after reviewing destination",
                ));
            }
            if interrupted || had_live_speed {
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
                    body,
                    tokens: HashMap::new(),
                    lanes: HashMap::new(),
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
        let task = {
            let mut state = self.inner.state.lock().unwrap_or_else(|p| p.into_inner());
            healthy(&state)?;
            if let Some(task) = state.document.tasks.iter().find(|t| {
                !t.state.terminal()
                    && t.request.connection_id == request.connection_id
                    && t.request.transfer.direction == request.transfer.direction
                    && t.request.transfer.local_path == request.transfer.local_path
                    && t.request.transfer.remote_path == request.transfer.remote_path
            }) {
                return Ok(task.clone());
            }
            if state.document.tasks.len() >= LIMIT {
                return Err(invalid("transfer history is full; clear finished tasks"));
            }
            let current_file = Some(request.transfer.remote_path.clone());
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
                total_files: Some(1),
                completed_files: 0,
                speed_bps: None,
                started_at: None,
                receipt: None,
                error: None,
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
        if let Err(e) = self.inner.persist(&mut state) {
            state.document = before;
            return Err(e);
        }
        if let Some(token) = state.tokens.get(id) {
            token.cancel();
        }
        if queued {
            state.tokens.remove(id);
        }
        Ok(state.document.tasks[index].clone())
    }
    pub async fn retry(&self, id: &str, conflict: FileConflictPolicy) -> Result<TransferTask> {
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
            let before = state.document.clone();
            let task = &mut state.document.tasks[index];
            task.attempt = task
                .attempt
                .checked_add(1)
                .ok_or_else(|| invalid("attempt limit reached"))?;
            task.request.transfer.conflict = conflict;
            task.state = TransferState::Queued;
            task.error = None;
            task.receipt = None;
            task.bytes = "0".into();
            task.total = None;
            task.completed_files = 0;
            task.current_file = Some(task.request.transfer.remote_path.clone());
            task.total_files = Some(1);
            task.speed_bps = None;
            task.started_at = None;
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
        let (lane, cancel) = {
            let mut state = self.inner.state.lock().unwrap_or_else(|p| p.into_inner());
            state.lanes.retain(|_, v| v.strong_count() > 0);
            let lane = state
                .lanes
                .get(&task.request.connection_id)
                .and_then(Weak::upgrade)
                .unwrap_or_else(|| {
                    let lane = Arc::new(Semaphore::new(1));
                    state
                        .lanes
                        .insert(task.request.connection_id.clone(), Arc::downgrade(&lane));
                    lane
                });
            let cancel = self.inner.root.child();
            state.tokens.insert(task.id.clone(), cancel.clone());
            (lane, cancel)
        };
        let weak = Arc::downgrade(&self.inner);
        let task = task.clone();
        let global = self.inner.global.clone();
        let executor = self.inner.executor.clone();
        tokio::spawn(async move {
            let permits = async {
                let lane = lane.acquire_owned().await.ok()?;
                let global = global.acquire_owned().await.ok()?;
                Some((lane, global))
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
            let progress = Arc::new(move |progress: FileTransferProgress| {
                if let Some(inner) = progress_weak.upgrade() {
                    let mut state = inner.state.lock().unwrap_or_else(|p| p.into_inner());
                    if let Some(t) = state
                        .document
                        .tasks
                        .iter_mut()
                        .find(|t| t.id == id && t.attempt == attempt && !t.state.terminal())
                    {
                        t.bytes = progress.bytes.to_string();
                        t.total = progress.total.map(|v| v.to_string());
                        let now = SystemClock.now().as_millis();
                        let started = t.started_at.get_or_insert_with(Instant::now);
                        t.speed_bps = Some(average_speed(progress.bytes, started.elapsed()));
                        t.revision += 1;
                        t.updated_ms = now;
                        inner.notify();
                    }
                }
            });
            let result = executor
                .execute(
                    &task.request.connection_id,
                    task.request.transfer,
                    cancel,
                    progress,
                )
                .await;
            if let Some(inner) = weak.upgrade() {
                let (state, receipt, error) = match result {
                    Ok(receipt) => (TransferState::Completed, Some(receipt), None),
                    Err(error) => (
                        if error.code == "cancelled" {
                            TransferState::Cancelled
                        } else {
                            TransferState::Failed
                        },
                        None,
                        Some(error),
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
        let body = serde_json::to_string(&state.document)
            .map_err(|_| invalid("transfer serialization failed"))?;
        if body.len() > 1024 * 1024 {
            return Err(invalid("transfer document exceeds limit"));
        }
        let store = DocumentStore::new(&self.db);
        let saved = match &state.body {
            Some(previous) => store.replace_body_if_unchanged(
                COLLECTION,
                "queue",
                previous,
                &body,
                SystemClock.now(),
            )?,
            None => store.insert_if_absent(COLLECTION, "queue", &body, SystemClock.now())?,
        };
        if !saved {
            return Err(invalid(
                "transfer coordinator ownership changed; reload host",
            ));
        }
        state.body = Some(body);
        self.notify();
        Ok(())
    }
    fn transition(
        &self,
        id: &str,
        attempt: u32,
        next: TransferState,
        receipt: Option<FileTransferReceipt>,
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
        task.receipt = receipt;
        task.error = error;
        task.revision += 1;
        task.updated_ms = SystemClock.now().as_millis();
        if next == TransferState::Running {
            task.started_at = Some(Instant::now());
            task.speed_bps = None;
            task.completed_files = 0;
        }
        if let Some(receipt) = &task.receipt {
            task.bytes = receipt.bytes.clone();
            if next == TransferState::Completed && !receipt.skipped {
                task.completed_files = task.total_files.unwrap_or(1);
            }
        }
        if next.terminal() {
            task.started_at = None;
            task.speed_bps = None;
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
