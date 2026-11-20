//! Interactive terminal for the desktop Terminal panel (Phase C).
//!
//! Runs one-shot shell commands in the **active project** directory and returns
//! captured output. Two safety properties mirror the agent's `bash` tool:
//! - **Dangerous commands are refused** (`rm -rf`, `curl | sh`, `sudo`, …) and
//!   reported as `blocked` rather than executed — these need explicit approval.
//! - Commands run with the active project folder as the working directory, so
//!   the panel operates within the same project the agent does.
//!
//! Unlike the agent's `bash` tool this panel is **not** allow-list gated: it is
//! a user-driven terminal, so any non-dangerous command the user types runs.
//! The dangerous-command refusal remains as a guardrail.

use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use deepagent_builtins::{is_dangerous, CommandExecutor, SystemExecutor};
use deepagent_core::error::{CoreError, Result};
use deepagent_terminal::{
    TerminalError, TerminalInputHolder, TerminalInputLease, TerminalLeasePersistence,
    TerminalLeaseRegistry, TerminalOpenRequest, TerminalReadChunk, TerminalRecoveryStatus,
    TerminalSession, TerminalSessionBackend, TerminalSignal,
};
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, Mutex, RwLock};

use crate::dto::TerminalResultDto;
use crate::project_service::ProjectService;
use crate::runtime_service::RuntimeBroker;
use crate::settings::TerminalShell;
use crate::terminal_lease::SqliteTerminalLeaseStore;
use deepagent_persistence::Database;

pub const DEFAULT_TERMINAL_LEASE_TTL_MS: i64 = 30 * 60 * 1_000;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalPtyHandle {
    pub pty_id: String,
    pub cols: u16,
    pub rows: u16,
}

/// A bounded, process-local PTY output slice. `cursor` is a monotonically
/// increasing byte offset and can be supplied to `pty_read_with_cursor` on a
/// reconnect. Output older than the bounded history is reported as truncated.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PtyReadChunk {
    /// Cursor immediately after the bytes returned in `data`.
    pub cursor: u64,
    pub data: Vec<u8>,
    /// The requested cursor was older than the retained history.
    pub truncated: bool,
}

#[derive(Clone, PartialEq, Eq)]
enum PtyOwner {
    Legacy,
    Shared { backend_id: String, run_id: String },
}

struct PtyControl {
    owner: PtyOwner,
    reaped: bool,
}

impl PtyControl {
    fn ensure_live(&self) -> deepagent_terminal::TerminalResult<()> {
        if self.reaped {
            return Err(TerminalError::Backend(
                "PTY has been reaped; retry close".into(),
            ));
        }
        Ok(())
    }
}

struct LocalPtyState {
    output_signal: deepagent_terminal::TerminalOutputSignal,
    // One gate covers authorization, the actual blocking operation and cleanup.
    // Its guard lives in the worker, so dropping the awaiting future cannot
    // release ownership while that operation is still running.
    control: std::sync::Mutex<PtyControl>,
    writer: Mutex<Box<dyn Write + Send>>,
    reader: Mutex<mpsc::Receiver<Vec<u8>>>,
    master: std::sync::Mutex<Option<Box<dyn portable_pty::MasterPty + Send>>>,
    child: std::sync::Mutex<Box<dyn portable_pty::Child + Send>>,
    output_cursor: AtomicU64,
    history: Mutex<VecDeque<(u64, Vec<u8>)>>,
    history_bytes: Mutex<usize>,
}

/// Runs interactive terminal commands in the active project directory.
pub struct TerminalService {
    projects: Arc<ProjectService>,
    /// Fallback working directory when no project is active.
    default_cwd: String,
    executor: Arc<dyn CommandExecutor>,
    runtime: Option<Arc<RuntimeBroker>>,
    ptys: Arc<RwLock<HashMap<String, Arc<LocalPtyState>>>>,
    next_pty_id: AtomicU64,
}

/// Direct host PTY implementation of the shared terminal-session contract.
pub struct DirectTerminalSessionBackend {
    service: Arc<TerminalService>,
    shell: TerminalShell,
    leases: Arc<TerminalLeaseRegistry>,
    backend_id: String,
}

impl DirectTerminalSessionBackend {
    pub fn new(service: Arc<TerminalService>, shell: TerminalShell) -> Self {
        Self::with_lease_persistence(service, shell, None)
    }

    /// Construct the host PTY adapter with the shared durable lease store.
    /// Keeping this explicit prevents callers from accidentally assuming that
    /// the compatibility constructor provides cross-process fencing.
    pub fn with_lease_persistence(
        service: Arc<TerminalService>,
        shell: TerminalShell,
        persistence: Option<Arc<dyn TerminalLeasePersistence>>,
    ) -> Self {
        Self {
            service,
            shell,
            leases: Arc::new(
                persistence
                    .map(TerminalLeaseRegistry::with_persistence)
                    .unwrap_or_default(),
            ),
            backend_id: uuid::Uuid::new_v4().to_string(),
        }
    }

    /// Production-oriented constructor backed by the application's SQLite
    /// database. The fallible return prevents silently falling back to memory
    /// if the lease store configuration is invalid.
    pub fn with_sqlite_lease_store(
        service: Arc<TerminalService>,
        shell: TerminalShell,
        database: Arc<Database>,
    ) -> Result<Self> {
        let store = SqliteTerminalLeaseStore::new(database, DEFAULT_TERMINAL_LEASE_TTL_MS)?;
        Ok(Self::with_lease_persistence(
            service,
            shell,
            Some(Arc::new(store)),
        ))
    }

    fn owner(&self, session: &TerminalSession) -> deepagent_terminal::TerminalResult<PtyOwner> {
        if session.backend != self.backend_kind() || session.connection_generation.is_some() {
            return Err(TerminalError::InvalidLease);
        }
        Ok(PtyOwner::Shared {
            backend_id: self.backend_id.clone(),
            run_id: session.run_id.clone(),
        })
    }

    fn input_owner(
        &self,
        session: &TerminalSession,
        lease: &TerminalInputLease,
    ) -> deepagent_terminal::TerminalResult<PtyOwner> {
        if session.session_id != lease.session_id || session.run_id != lease.run_id {
            return Err(TerminalError::InvalidLease);
        }
        self.owner(session)
    }

    async fn with_input<T, F>(
        &self,
        session: &TerminalSession,
        lease: &TerminalInputLease,
        operation: F,
    ) -> deepagent_terminal::TerminalResult<T>
    where
        T: Send + 'static,
        F: FnOnce(&LocalPtyState) -> deepagent_terminal::TerminalResult<T> + Send + 'static,
    {
        let owner = self.input_owner(session, lease)?;
        let leases = self.leases.clone();
        let lease = lease.clone();
        self.service
            .with_pty_control(&session.session_id, owner, move |state, control| {
                leases.validate(&lease)?;
                control.ensure_live()?;
                operation(state)
            })
            .await
    }

    pub fn last_cursor(&self, session_id: &str) -> deepagent_terminal::TerminalResult<u64> {
        self.leases.last_cursor(session_id)
    }

    pub async fn recovery_status(
        &self,
        session: &TerminalSession,
    ) -> deepagent_terminal::TerminalResult<TerminalRecoveryStatus> {
        let owner = self.owner(session)?;
        let session_id = &session.session_id;
        let cursor = self.last_cursor(session_id)?;
        let state = self.service.ptys.read().await.get(session_id).cloned();
        let Some(state) = state else {
            return Ok(TerminalRecoveryStatus {
                cursor,
                available: false,
                control_available: false,
            });
        };
        let leases = self.leases.clone();
        let id = session_id.to_string();
        tokio::task::spawn_blocking(move || {
            let control = state.control.lock().map_err(pty_error)?;
            if control.owner != owner {
                return Err(TerminalError::InvalidLease);
            }
            let alive = !control.reaped
                && state
                    .child
                    .lock()
                    .map_err(pty_error)?
                    .try_wait()
                    .map_err(pty_error)?
                    .is_none();
            Ok(TerminalRecoveryStatus {
                cursor,
                available: alive,
                control_available: alive && leases.control_available(&id)?,
            })
        })
        .await
        .map_err(pty_error)?
    }
}

impl TerminalService {
    /// Build over the project registry, with a default cwd used when no project
    /// is active. Uses the real [`SystemExecutor`].
    pub fn new(
        projects: Arc<ProjectService>,
        default_cwd: impl Into<String>,
        runtime: Arc<RuntimeBroker>,
    ) -> Self {
        Self {
            projects,
            default_cwd: default_cwd.into(),
            executor: Arc::new(SystemExecutor),
            runtime: Some(runtime),
            ptys: Arc::new(RwLock::new(HashMap::new())),
            next_pty_id: AtomicU64::new(1),
        }
    }

    /// Build with a custom executor (for tests).
    pub fn with_executor(
        projects: Arc<ProjectService>,
        default_cwd: impl Into<String>,
        executor: Arc<dyn CommandExecutor>,
    ) -> Self {
        Self {
            projects,
            default_cwd: default_cwd.into(),
            executor,
            runtime: None,
            ptys: Arc::new(RwLock::new(HashMap::new())),
            next_pty_id: AtomicU64::new(1),
        }
    }

    /// The working directory for terminal commands: the active project, else
    /// the default cwd.
    fn cwd(&self) -> String {
        self.projects
            .active()
            .ok()
            .flatten()
            .filter(|p| !p.trim().is_empty())
            .unwrap_or_else(|| self.default_cwd.clone())
    }

    /// Run `command` in the active project directory. Dangerous commands are
    /// refused (reported as `blocked`) instead of being executed.
    pub async fn run(&self, command: &str) -> Result<TerminalResultDto> {
        let command = command.trim();
        let cwd = self.cwd();

        if command.is_empty() {
            return Ok(TerminalResultDto {
                command: command.to_string(),
                cwd,
                exit_code: Some(0),
                stdout: String::new(),
                stderr: String::new(),
                blocked: false,
            });
        }

        if is_dangerous(command) {
            return Ok(TerminalResultDto {
                command: command.to_string(),
                cwd,
                exit_code: None,
                stdout: String::new(),
                stderr: format!("blocked: '{command}' is high-risk and requires explicit approval"),
                blocked: true,
            });
        }

        let outcome = self.executor.run(command, &cwd).await?;
        Ok(TerminalResultDto {
            command: command.to_string(),
            cwd,
            exit_code: outcome.exit_code,
            stdout: outcome.stdout,
            stderr: outcome.stderr,
            blocked: false,
        })
    }

    /// The current working directory (for the prompt display).
    pub fn current_dir(&self) -> String {
        self.cwd()
    }

    /// Spawn a local PTY-backed interactive shell rooted at the active project.
    pub async fn pty_spawn(
        &self,
        shell: TerminalShell,
        cols: u16,
        rows: u16,
    ) -> Result<LocalPtyHandle> {
        let cwd = self.cwd();
        self.pty_spawn_in(shell, &cwd, cols, rows).await
    }

    pub async fn pty_spawn_in(
        &self,
        shell: TerminalShell,
        cwd: &str,
        cols: u16,
        rows: u16,
    ) -> Result<LocalPtyHandle> {
        self.pty_spawn_owned_in(
            self.allocate_pty_id(),
            shell,
            cwd,
            cols,
            rows,
            PtyOwner::Legacy,
        )
        .await
    }

    fn allocate_pty_id(&self) -> String {
        format!(
            "local-pty-{}",
            self.next_pty_id.fetch_add(1, Ordering::Relaxed)
        )
    }

    async fn pty_spawn_owned_in(
        &self,
        id: String,
        shell: TerminalShell,
        cwd: &str,
        cols: u16,
        rows: u16,
        owner: PtyOwner,
    ) -> Result<LocalPtyHandle> {
        let environment = self
            .runtime
            .as_ref()
            .map(|runtime| runtime.build_process_environment(Some(std::path::Path::new(&cwd))))
            .unwrap_or_default();
        let state = spawn_local_pty(shell, cwd, cols, rows, &environment, owner)?;
        self.ptys.write().await.insert(id.clone(), Arc::new(state));
        Ok(LocalPtyHandle {
            pty_id: id,
            cols,
            rows,
        })
    }

    async fn with_pty_control<T, F>(
        &self,
        id: &str,
        owner: PtyOwner,
        operation: F,
    ) -> deepagent_terminal::TerminalResult<T>
    where
        T: Send + 'static,
        F: FnOnce(&LocalPtyState, &mut PtyControl) -> deepagent_terminal::TerminalResult<T>
            + Send
            + 'static,
    {
        let state = self
            .ptys
            .read()
            .await
            .get(id)
            .cloned()
            .ok_or_else(|| TerminalError::SessionNotFound(id.into()))?;
        tokio::task::spawn_blocking(move || {
            let mut control = state.control.lock().map_err(pty_error)?;
            if control.owner != owner {
                return Err(TerminalError::InvalidLease);
            }
            operation(&state, &mut control)
        })
        .await
        .map_err(pty_error)?
    }

    pub async fn pty_write(&self, handle: &LocalPtyHandle, data: &[u8]) -> Result<()> {
        let payload = data.to_vec();
        self.with_pty_control(&handle.pty_id, PtyOwner::Legacy, move |state, control| {
            control.ensure_live()?;
            state.write(&payload)
        })
        .await
        .map_err(|error| CoreError::other(error.to_string()))
    }

    pub async fn pty_read(&self, handle: &LocalPtyHandle) -> Result<Vec<u8>> {
        let state = self
            .ptys
            .read()
            .await
            .get(&handle.pty_id)
            .cloned()
            .ok_or_else(|| CoreError::not_found(format!("pty {} not found", handle.pty_id)))?;
        let chunks = drain_pty_output(&state).await;
        Ok(chunks.into_iter().flat_map(|(_, data)| data).collect())
    }

    /// Check whether a local PTY handle is still owned by this process.
    pub async fn pty_exists(&self, pty_id: &str) -> bool {
        self.ptys.read().await.contains_key(pty_id)
    }

    /// Read PTY output after a byte cursor. The returned cursor acknowledges
    /// only bytes included in this response; it never advances over a suffix
    /// that was retained but capped by the response size.
    pub async fn pty_read_with_cursor(
        &self,
        handle: &LocalPtyHandle,
        after_cursor: u64,
    ) -> Result<PtyReadChunk> {
        let state = self
            .ptys
            .read()
            .await
            .get(&handle.pty_id)
            .cloned()
            .ok_or_else(|| CoreError::not_found(format!("pty {} not found", handle.pty_id)))?;
        let _ = drain_pty_output(&state).await;
        let history = state.history.lock().await;
        let current = state.output_cursor.load(Ordering::Acquire);
        let oldest = history.front().map(|(start, _)| *start).unwrap_or(current);
        let truncated = after_cursor < oldest && oldest > 0;
        let read_from = after_cursor.max(oldest);
        let mut data = Vec::new();
        let mut next_cursor = read_from;
        for (start, chunk) in history.iter() {
            let end = *start + chunk.len() as u64;
            if end <= read_from {
                continue;
            }
            let offset = read_from.saturating_sub(*start) as usize;
            let offset = offset.min(chunk.len());
            let remaining = 64 * 1024 - data.len();
            let take = remaining.min(chunk.len() - offset);
            data.extend_from_slice(&chunk[offset..offset + take]);
            next_cursor = (*start + offset as u64) + take as u64;
            if data.len() == 64 * 1024 {
                break;
            }
        }
        Ok(PtyReadChunk {
            cursor: if data.is_empty() {
                current
            } else {
                next_cursor
            },
            data,
            truncated,
        })
    }

    async fn pty_output_finished(
        &self,
        id: &str,
        cursor: u64,
    ) -> deepagent_terminal::TerminalResult<bool> {
        let state = self
            .ptys
            .read()
            .await
            .get(id)
            .cloned()
            .ok_or_else(|| TerminalError::SessionNotFound(id.into()))?;
        let reader = state.reader.lock().await;
        Ok(reader.is_closed()
            && reader.is_empty()
            && cursor >= state.output_cursor.load(Ordering::Acquire))
    }

    pub async fn pty_resize(&self, handle: &LocalPtyHandle, cols: u16, rows: u16) -> Result<()> {
        self.with_pty_control(&handle.pty_id, PtyOwner::Legacy, move |state, control| {
            control.ensure_live()?;
            state.resize(cols, rows)
        })
        .await
        .map_err(|error| CoreError::other(error.to_string()))
    }

    async fn close_owned_pty<V, R>(
        &self,
        id: &str,
        owner: PtyOwner,
        validate: V,
        release: R,
    ) -> deepagent_terminal::TerminalResult<()>
    where
        V: FnOnce() -> deepagent_terminal::TerminalResult<()> + Send + 'static,
        R: FnOnce() -> deepagent_terminal::TerminalResult<()> + Send + 'static,
    {
        let ptys = self.ptys.clone();
        let id_owned = id.to_string();
        let result = self
            .with_pty_control(id, owner, move |state, control| {
                validate()?;
                if !control.reaped {
                    state.reap()?;
                    control.reaped = true;
                }
                // A failed durable release retains a reaped owner. Explicit
                // takeover can rotate an expired lease and retry this close.
                release()?;
                ptys.blocking_write().remove(&id_owned);
                Ok(())
            })
            .await;
        match &result {
            Ok(()) => tracing::debug!(pty_id = id, "PTY close reaped child and released owner"),
            Err(error) => {
                tracing::warn!(pty_id = id, %error, "PTY close incomplete; retained owner can be retried")
            }
        }
        result
    }

    pub async fn pty_close(&self, handle: &LocalPtyHandle) -> Result<()> {
        self.close_owned_pty(&handle.pty_id, PtyOwner::Legacy, || Ok(()), || Ok(()))
            .await
            .map_err(|error| CoreError::other(error.to_string()))
    }

    /// Launch the user's system terminal in the current working directory,
    /// using the preferred shell from settings.
    pub fn open_system(&self, shell: TerminalShell) -> Result<String> {
        let cwd = self.cwd();
        let environment = self
            .runtime
            .as_ref()
            .map(|runtime| runtime.build_process_environment(Some(std::path::Path::new(&cwd))))
            .unwrap_or_default();
        spawn_system_terminal(shell, &cwd, &environment)?;
        Ok(cwd)
    }
}

#[async_trait::async_trait]
impl TerminalSessionBackend for DirectTerminalSessionBackend {
    fn backend_kind(&self) -> &'static str {
        "direct"
    }

    async fn subscribe_output(
        &self,
        session: &TerminalSession,
    ) -> deepagent_terminal::TerminalResult<deepagent_terminal::TerminalOutputSubscription> {
        let owner = self.owner(session)?;
        self.service
            .with_pty_control(&session.session_id, owner, |state, _| {
                Ok(state.output_signal.subscribe())
            })
            .await
    }

    async fn open(
        &self,
        request: TerminalOpenRequest,
    ) -> deepagent_terminal::TerminalResult<(TerminalSession, TerminalInputLease)> {
        if !matches!(request.target, deepagent_terminal::TerminalTarget::Local) {
            return Err(TerminalError::UnsupportedCapability {
                backend: "direct".into(),
                capability: "ssh_target".into(),
            });
        }
        let session = TerminalSession {
            session_id: self.service.allocate_pty_id(),
            run_id: request.run_id,
            backend: self.backend_kind().into(),
            cols: request.cols,
            rows: request.rows,
            connection_generation: None,
        };
        // Do not start a process until durable ownership exists. A failed lease
        // acquisition can no longer leave an unpublished, uncontrollable child.
        let lease =
            self.leases
                .register(&session.session_id, &session.run_id, request.initial_holder)?;
        let owner = self.owner(&session)?;
        if let Err(error) = self
            .service
            .pty_spawn_owned_in(
                session.session_id.clone(),
                self.shell,
                &request.cwd,
                request.cols,
                request.rows,
                owner,
            )
            .await
        {
            if let Err(release) = self.leases.remove(&lease) {
                return Err(TerminalError::Backend(format!(
                    "{error}; failed-open lease release failed: {release}"
                )));
            }
            return Err(pty_error(error));
        }
        Ok((session, lease))
    }

    async fn write(
        &self,
        session: &TerminalSession,
        lease: &TerminalInputLease,
        data: &[u8],
    ) -> deepagent_terminal::TerminalResult<()> {
        let data = data.to_vec();
        self.with_input(session, lease, move |state| state.write(&data))
            .await
    }

    async fn renew(
        &self,
        session: &TerminalSession,
        lease: &TerminalInputLease,
    ) -> deepagent_terminal::TerminalResult<()> {
        let owner = self.input_owner(session, lease)?;
        let leases = self.leases.clone();
        let lease = lease.clone();
        self.service
            .with_pty_control(&session.session_id, owner, move |_, _| leases.renew(&lease))
            .await
    }

    async fn read(
        &self,
        session: &TerminalSession,
        after_cursor: u64,
    ) -> deepagent_terminal::TerminalResult<TerminalReadChunk> {
        // Read does not require input ownership, but still belongs to this host/run.
        let owner = self.owner(session)?;
        self.service
            .with_pty_control(&session.session_id, owner, |state, _| {
                state.finish_exited_output()
            })
            .await?;
        let chunk = self
            .service
            .pty_read_with_cursor(
                &LocalPtyHandle {
                    pty_id: session.session_id.clone(),
                    cols: session.cols,
                    rows: session.rows,
                },
                after_cursor,
            )
            .await
            .map_err(pty_error)?;
        if chunk.cursor != after_cursor {
            self.leases
                .record_cursor(&session.session_id, chunk.cursor)?;
        }
        Ok(TerminalReadChunk {
            cursor: chunk.cursor,
            data: chunk.data,
            truncated: chunk.truncated,
            eof: self
                .service
                .pty_output_finished(&session.session_id, chunk.cursor)
                .await?,
        })
    }

    async fn resize(
        &self,
        session: &TerminalSession,
        lease: &TerminalInputLease,
        cols: u16,
        rows: u16,
    ) -> deepagent_terminal::TerminalResult<()> {
        self.with_input(session, lease, move |state| state.resize(cols, rows))
            .await
    }

    async fn signal(
        &self,
        session: &TerminalSession,
        lease: &TerminalInputLease,
        signal: TerminalSignal,
    ) -> deepagent_terminal::TerminalResult<()> {
        self.with_input(session, lease, move |state| match signal {
            TerminalSignal::Interrupt => state.write(&[3]),
            TerminalSignal::Terminate | TerminalSignal::Kill => state.kill(),
        })
        .await
    }

    async fn takeover(
        &self,
        session: &TerminalSession,
        holder: TerminalInputHolder,
    ) -> deepagent_terminal::TerminalResult<TerminalInputLease> {
        let owner = self.owner(session)?;
        let leases = self.leases.clone();
        let id = session.session_id.clone();
        self.service
            .with_pty_control(&session.session_id, owner, move |_, _| {
                let next = leases.takeover(&id, holder)?;
                tracing::debug!(
                    session_id = id,
                    epoch = next.epoch,
                    ?holder,
                    "terminal input owner rotated"
                );
                Ok(next)
            })
            .await
    }

    async fn release(
        &self,
        session: &TerminalSession,
        lease: &TerminalInputLease,
        next_holder: TerminalInputHolder,
    ) -> deepagent_terminal::TerminalResult<TerminalInputLease> {
        let owner = self.input_owner(session, lease)?;
        let leases = self.leases.clone();
        let lease = lease.clone();
        self.service
            .with_pty_control(&session.session_id, owner, move |_, _| {
                leases.release(&lease, next_holder)
            })
            .await
    }

    async fn close(
        &self,
        session: &TerminalSession,
        lease: &TerminalInputLease,
    ) -> deepagent_terminal::TerminalResult<()> {
        let owner = self.input_owner(session, lease)?;
        let validating = self.leases.clone();
        let releasing = self.leases.clone();
        let before = lease.clone();
        let after = lease.clone();
        self.service
            .close_owned_pty(
                &session.session_id,
                owner,
                move || validating.validate(&before),
                move || releasing.remove(&after),
            )
            .await
    }
}

fn pty_error(error: impl std::fmt::Display) -> TerminalError {
    TerminalError::Backend(error.to_string())
}

impl LocalPtyState {
    fn write(&self, data: &[u8]) -> deepagent_terminal::TerminalResult<()> {
        let mut writer = self.writer.blocking_lock();
        writer.write_all(data).map_err(pty_error)?;
        writer.flush().map_err(pty_error)
    }

    fn resize(&self, cols: u16, rows: u16) -> deepagent_terminal::TerminalResult<()> {
        self.master
            .lock()
            .map_err(pty_error)?
            .as_ref()
            .ok_or_else(|| pty_error("PTY output is closing after process exit"))?
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(pty_error)
    }

    fn finish_exited_output(&self) -> deepagent_terminal::TerminalResult<()> {
        if self
            .child
            .lock()
            .map_err(pty_error)?
            .try_wait()
            .map_err(pty_error)?
            .is_some()
        {
            if let Some(master) = self.master.lock().map_err(pty_error)?.take() {
                // portable-pty 0.8 ConPTY keeps its output pipe open until the
                // pseudoconsole is closed, even after cmd.exe has exited.
                // ClosePseudoConsole can wait for output to drain: never hold
                // the input gate or stop the reader while dropping it.
                tokio::task::spawn_blocking(move || drop(master));
            }
        }
        Ok(())
    }

    fn kill(&self) -> deepagent_terminal::TerminalResult<()> {
        self.child
            .lock()
            .map_err(pty_error)?
            .kill()
            .map_err(pty_error)
    }

    fn reap(&self) -> deepagent_terminal::TerminalResult<()> {
        let mut child = self.child.lock().map_err(pty_error)?;
        if child.try_wait().map_err(pty_error)?.is_none() {
            // Do not wait after a failed kill: the still-live child may never
            // exit. Retain the map entry and return the real failure to retry.
            child.kill().map_err(pty_error)?;
        }
        child.wait().map_err(pty_error)?;
        Ok(())
    }
}

impl Drop for LocalPtyState {
    fn drop(&mut self) {
        // Final host teardown is not a public legacy-control escape hatch.
        // Normal close reports errors and keeps the owner for explicit retry.
        if let Err(error) = self.reap() {
            tracing::warn!(%error, "PTY host teardown could not reap child");
        }
    }
}

fn spawn_local_pty(
    shell: TerminalShell,
    cwd: &str,
    cols: u16,
    rows: u16,
    environment: &std::collections::BTreeMap<String, String>,
    owner: PtyOwner,
) -> Result<LocalPtyState> {
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| CoreError::other(format!("open pty failed: {e}")))?;

    let cmd = terminal_command(shell, cwd, environment)?;
    let mut child = pair
        .slave
        .spawn_command(cmd)
        .map_err(|e| CoreError::other(format!("spawn shell failed: {e}")))?;

    let io = (|| {
        let reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| CoreError::other(format!("clone pty reader failed: {e}")))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|e| CoreError::other(format!("take pty writer failed: {e}")))?;
        Ok::<_, CoreError>((reader, writer))
    })();
    let (reader, writer) = match io {
        Ok(io) => io,
        Err(error) => {
            child.kill().map_err(|cleanup| {
                CoreError::other(format!("{error}; unpublished child kill failed: {cleanup}"))
            })?;
            child.wait().map_err(|cleanup| {
                CoreError::other(format!("{error}; unpublished child wait failed: {cleanup}"))
            })?;
            return Err(error);
        }
    };

    let (stdout_tx, stdout_rx) = mpsc::channel::<Vec<u8>>(256);
    let output_signal = deepagent_terminal::TerminalOutputSignal::default();
    let activity = output_signal.clone();
    std::thread::spawn(move || {
        let mut reader = reader;
        let mut buf = [0u8; 4096];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if stdout_tx.blocking_send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                    activity.notify();
                }
                Err(_) => break,
            }
        }
        drop(stdout_tx);
        activity.notify();
    });

    Ok(LocalPtyState {
        output_signal,
        control: std::sync::Mutex::new(PtyControl {
            owner,
            reaped: false,
        }),
        writer: Mutex::new(writer),
        reader: Mutex::new(stdout_rx),
        master: std::sync::Mutex::new(Some(pair.master)),
        child: std::sync::Mutex::new(child),
        output_cursor: AtomicU64::new(0),
        history: Mutex::new(VecDeque::new()),
        history_bytes: Mutex::new(0),
    })
}

async fn drain_pty_output(state: &LocalPtyState) -> Vec<(u64, Vec<u8>)> {
    let mut rx = state.reader.lock().await;
    let mut history = state.history.lock().await;
    let mut history_bytes = state.history_bytes.lock().await;
    let mut chunks = Vec::new();
    while let Ok(chunk) = rx.try_recv() {
        if chunk.is_empty() {
            continue;
        }
        let start = state
            .output_cursor
            .fetch_add(chunk.len() as u64, Ordering::AcqRel);
        *history_bytes += chunk.len();
        history.push_back((start, chunk.clone()));
        chunks.push((start, chunk));
        while *history_bytes > 256 * 1024 {
            if let Some((_, old)) = history.pop_front() {
                *history_bytes = history_bytes.saturating_sub(old.len());
            } else {
                break;
            }
        }
    }
    chunks
}

fn terminal_command(
    shell: TerminalShell,
    cwd: &str,
    environment: &std::collections::BTreeMap<String, String>,
) -> Result<CommandBuilder> {
    let mut cmd = match shell {
        TerminalShell::PowerShell => {
            let mut cmd = CommandBuilder::new("powershell.exe");
            cmd.arg("-NoLogo");
            cmd
        }
        TerminalShell::CommandPrompt => CommandBuilder::new("cmd.exe"),
        TerminalShell::GitBash => {
            let bash = find_git_bash_exe()
                .ok_or_else(|| CoreError::not_found("Git Bash was not found on this computer"))?;
            let mut cmd = CommandBuilder::new(bash);
            cmd.arg("--login");
            cmd.arg("-i");
            cmd
        }
        TerminalShell::Wsl => {
            let mut cmd = CommandBuilder::new("wsl.exe");
            if let Some(wsl_cwd) = to_wsl_path(cwd) {
                cmd.arg("--cd");
                cmd.arg(wsl_cwd);
            }
            cmd
        }
    };
    cmd.cwd(cwd);
    for (key, value) in environment {
        cmd.env(key, value);
    }
    Ok(cmd)
}

#[cfg(target_os = "windows")]
fn spawn_system_terminal(
    shell: TerminalShell,
    cwd: &str,
    environment: &std::collections::BTreeMap<String, String>,
) -> Result<()> {
    use std::process::Command;

    match shell {
        TerminalShell::PowerShell => {
            Command::new("powershell.exe")
                .args([
                    "-NoExit",
                    "-Command",
                    &format!("Set-Location -LiteralPath {}", powershell_quote(cwd)),
                ])
                .envs(environment)
                .spawn()
                .map_err(|e| CoreError::other(format!("failed to open PowerShell: {e}")))?;
        }
        TerminalShell::CommandPrompt => {
            Command::new("cmd.exe")
                .args(["/K", &format!("cd /d {}", cmd_quote(cwd))])
                .envs(environment)
                .spawn()
                .map_err(|e| CoreError::other(format!("failed to open Command Prompt: {e}")))?;
        }
        TerminalShell::GitBash => {
            let bash = find_git_bash_exe()
                .ok_or_else(|| CoreError::not_found("Git Bash was not found on this computer"))?;
            Command::new(bash)
                .args([
                    "--login",
                    "-i",
                    "-c",
                    &format!("cd {}; exec bash -i", bash_quote(cwd)),
                ])
                .envs(environment)
                .spawn()
                .map_err(|e| CoreError::other(format!("failed to open Git Bash: {e}")))?;
        }
        TerminalShell::Wsl => {
            let mut cmd = Command::new("wsl.exe");
            if let Some(wsl_cwd) = to_wsl_path(cwd) {
                cmd.args(["--cd", &wsl_cwd]);
            }
            cmd.envs(environment);
            cmd.spawn()
                .map_err(|e| CoreError::other(format!("failed to open WSL: {e}")))?;
        }
    }
    Ok(())
}

#[cfg(not(target_os = "windows"))]
fn spawn_system_terminal(
    _shell: TerminalShell,
    _cwd: &str,
    _environment: &std::collections::BTreeMap<String, String>,
) -> Result<()> {
    Err(CoreError::other(
        "opening a system terminal is currently supported on Windows only",
    ))
}

#[cfg(target_os = "windows")]
fn powershell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

#[cfg(target_os = "windows")]
fn cmd_quote(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}

#[cfg(target_os = "windows")]
fn bash_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

#[cfg(target_os = "windows")]
fn find_git_bash_exe() -> Option<String> {
    let mut candidates = Vec::new();
    if let Ok(program_files) = std::env::var("ProgramFiles") {
        candidates.push(format!(r"{program_files}\Git\bin\bash.exe"));
    }
    if let Ok(program_files_x86) = std::env::var("ProgramFiles(x86)") {
        candidates.push(format!(r"{program_files_x86}\Git\bin\bash.exe"));
    }
    if let Ok(local_app_data) = std::env::var("LocalAppData") {
        candidates.push(format!(r"{local_app_data}\Programs\Git\bin\bash.exe"));
    }
    candidates.push("bash.exe".to_string());
    candidates
        .into_iter()
        .find(|path| path.eq_ignore_ascii_case("bash.exe") || std::path::Path::new(path).exists())
}

#[cfg(not(target_os = "windows"))]
fn find_git_bash_exe() -> Option<String> {
    None
}

#[cfg(target_os = "windows")]
fn to_wsl_path(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    let bytes = trimmed.as_bytes();
    if bytes.len() >= 3 && bytes[1] == b':' && (bytes[2] == b'\\' || bytes[2] == b'/') {
        let drive = (bytes[0] as char).to_ascii_lowercase();
        let rest = trimmed[3..].replace('\\', "/");
        return Some(format!("/mnt/{drive}/{rest}"));
    }
    Some(trimmed.replace('\\', "/"))
}

#[cfg(not(target_os = "windows"))]
fn to_wsl_path(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use deepagent_builtins::CommandOutcome;
    use deepagent_persistence::Database;
    use std::sync::Mutex;

    struct RecordingExecutor {
        ran: Mutex<Vec<(String, String)>>,
    }

    #[async_trait]
    impl CommandExecutor for RecordingExecutor {
        async fn run(&self, command: &str, cwd: &str) -> Result<CommandOutcome> {
            self.ran
                .lock()
                .unwrap()
                .push((command.to_string(), cwd.to_string()));
            Ok(CommandOutcome {
                exit_code: Some(0),
                stdout: format!("ran: {command}"),
                stderr: String::new(),
            })
        }
    }

    fn service() -> (TerminalService, Arc<RecordingExecutor>, Arc<ProjectService>) {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let projects = Arc::new(ProjectService::new(db));
        let exec = Arc::new(RecordingExecutor {
            ran: Mutex::new(Vec::new()),
        });
        let svc = TerminalService::with_executor(projects.clone(), "/default", exec.clone());
        (svc, exec, projects)
    }

    #[cfg(windows)]
    #[derive(Debug)]
    struct FailingChild {
        inner: Box<dyn portable_pty::Child + Send>,
        fail_kill: bool,
        fail_wait: bool,
    }

    #[cfg(windows)]
    impl portable_pty::ChildKiller for FailingChild {
        fn kill(&mut self) -> std::io::Result<()> {
            if std::mem::take(&mut self.fail_kill) {
                return Err(std::io::Error::other("h293 injected kill failure"));
            }
            self.inner.kill()
        }
        fn clone_killer(&self) -> Box<dyn portable_pty::ChildKiller + Send + Sync> {
            self.inner.clone_killer()
        }
    }

    #[cfg(windows)]
    impl portable_pty::Child for FailingChild {
        fn try_wait(&mut self) -> std::io::Result<Option<portable_pty::ExitStatus>> {
            self.inner.try_wait()
        }
        fn wait(&mut self) -> std::io::Result<portable_pty::ExitStatus> {
            if std::mem::take(&mut self.fail_wait) {
                return Err(std::io::Error::other("h293 injected wait failure"));
            }
            self.inner.wait()
        }
        fn process_id(&self) -> Option<u32> {
            self.inner.process_id()
        }
        fn as_raw_handle(&self) -> Option<std::os::windows::io::RawHandle> {
            self.inner.as_raw_handle()
        }
    }

    // Independent fixture cleanup also runs on the old remove-before-kill bug.
    #[cfg(windows)]
    impl Drop for FailingChild {
        fn drop(&mut self) {
            let _ = self.inner.kill();
            let _ = self.inner.wait();
        }
    }

    #[cfg(windows)]
    async fn h293_cleanup_failure_is_retryable(fail_kill: bool) {
        let dir = tempfile::tempdir().unwrap();
        let db = Arc::new(Database::open_in_memory().unwrap());
        let svc = TerminalService::with_executor(
            Arc::new(ProjectService::new(db)),
            dir.path().to_string_lossy(),
            Arc::new(SystemExecutor),
        );
        let handle = svc
            .pty_spawn_in(
                TerminalShell::CommandPrompt,
                &dir.path().to_string_lossy(),
                80,
                24,
            )
            .await
            .unwrap();
        let state = svc.ptys.read().await.get(&handle.pty_id).unwrap().clone();
        {
            let mut child = state.child.lock().unwrap();
            let mut slot = std::process::Command::new("cmd.exe")
                .args(["/C", "exit", "0"])
                .spawn()
                .unwrap();
            slot.wait().unwrap();
            let inner = std::mem::replace(&mut *child, Box::new(slot));
            *child = Box::new(FailingChild {
                inner,
                fail_kill,
                fail_wait: true,
            });
        }
        let first = svc.pty_close(&handle).await;
        let retained = svc.pty_exists(&handle.pty_id).await;
        if retained {
            // A kill failure does not consume the independent wait fault.
            if svc.pty_close(&handle).await.is_err() {
                svc.pty_close(&handle).await.unwrap();
            }
        }
        assert!(!svc.pty_exists(&handle.pty_id).await);
        assert!(first.is_err(), "cleanup errors must not be swallowed");
        assert!(retained, "cleanup failure must retain the owner for retry");
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn h293_kill_failure_retains_retryable_pty_owner() {
        h293_cleanup_failure_is_retryable(true).await;
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn h293_wait_failure_retains_retryable_pty_owner() {
        h293_cleanup_failure_is_retryable(false).await;
    }

    #[cfg(windows)]
    async fn h293_shared_fixture() -> (
        tempfile::TempDir,
        Arc<TerminalService>,
        Arc<DirectTerminalSessionBackend>,
        TerminalSession,
        TerminalInputLease,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let db = Arc::new(Database::open(dir.path().join("lease.sqlite")).unwrap());
        let store = Arc::new(SqliteTerminalLeaseStore::new(db.clone(), 60_000).unwrap());
        let svc = Arc::new(TerminalService::with_executor(
            Arc::new(ProjectService::new(db)),
            dir.path().to_string_lossy(),
            Arc::new(SystemExecutor),
        ));
        let backend = Arc::new(DirectTerminalSessionBackend::with_lease_persistence(
            svc.clone(),
            TerminalShell::CommandPrompt,
            Some(store),
        ));
        let (session, lease) = backend
            .open(TerminalOpenRequest {
                target: Default::default(),
                run_id: "h293-shared".into(),
                cwd: dir.path().to_string_lossy().into_owned(),
                cols: 80,
                rows: 24,
                initial_holder: TerminalInputHolder::User,
            })
            .await
            .unwrap();
        (dir, svc, backend, session, lease)
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn output_notifications_wake_real_local_pty_without_consuming_cursor_data() {
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            let (_dir, _svc, backend, session, lease) = h293_shared_fixture().await;
            let mut notices = backend.subscribe_output(&session).await.unwrap();
            let mut forged = session.clone();
            forged.run_id = "wrong-run".into();
            assert!(backend.subscribe_output(&forged).await.is_err());
            backend
                .write(&session, &lease, b"echo notification-ready\r\n")
                .await
                .unwrap();
            let mut revision = 0;
            let mut cursor = 0;
            let mut bytes = Vec::new();
            // ConPTY can replace CR/LF with VT cursor movement; require both
            // echoed command and executed result rather than assuming wire EOLs.
            while String::from_utf8_lossy(&bytes)
                .matches("notification-ready")
                .count()
                < 2
            {
                let notice = notices
                    .wait_after(revision, std::time::Duration::from_secs(1))
                    .await;
                revision = notice.revision;
                let chunk = backend.read(&session, cursor).await.unwrap();
                assert!(backend
                    .read(&session, cursor)
                    .await
                    .unwrap()
                    .data
                    .starts_with(&chunk.data));
                cursor = chunk.cursor;
                bytes.extend(chunk.data);
            }
            eprintln!("local notification output observed: {} bytes", bytes.len());
            backend.write(&session, &lease, b"exit\r\n").await.unwrap();
            loop {
                let notice = notices
                    .wait_after(revision, std::time::Duration::from_secs(1))
                    .await;
                revision = notice.revision;
                let chunk = backend.read(&session, cursor).await.unwrap();
                cursor = chunk.cursor;
                if chunk.eof {
                    break;
                }
            }
            backend.close(&session, &lease).await.unwrap();
            assert!(backend.subscribe_output(&session).await.is_err());
        })
        .await
        .expect("native notification and EOF must settle");
    }

    #[cfg(windows)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn h293_cancelled_blocking_write_keeps_gate_until_before_takeover() {
        let (_dir, svc, backend, session, lease) = h293_shared_fixture().await;
        let state = svc
            .ptys
            .read()
            .await
            .get(&session.session_id)
            .unwrap()
            .clone();
        let writer = state.writer.lock().await;
        let task_backend = backend.clone();
        let task_session = session.clone();
        let task_lease = lease.clone();
        let write = tokio::spawn(async move {
            task_backend
                .write(&task_session, &task_lease, b"echo owned > owned.txt\r\n")
                .await
        });
        let gate_acquired = tokio::time::timeout(std::time::Duration::from_secs(3), async {
            while state.control.try_lock().is_ok() {
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await
        .is_ok();
        write.abort();
        let _ = write.await;
        let task_backend = backend.clone();
        let task_session = session.clone();
        let takeover = tokio::spawn(async move {
            task_backend
                .takeover(&task_session, TerminalInputHolder::Runtime)
                .await
        });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let takeover_waited = !takeover.is_finished();
        drop(writer);
        let next = tokio::time::timeout(std::time::Duration::from_secs(3), takeover)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let stale_rejected = backend
            .write(&session, &lease, b"echo stale\r\n")
            .await
            .is_err();
        backend.close(&session, &next).await.unwrap();
        assert!(gate_acquired && takeover_waited && stale_rejected);
        assert!(!svc.pty_exists(&session.session_id).await);
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn h293_recovery_status_rejects_mismatched_session_scope() {
        let (_dir, _svc, backend, session, lease) = h293_shared_fixture().await;
        assert_eq!(session.connection_generation, None);
        let mut wrong_run = session.clone();
        wrong_run.run_id = "another-run".into();
        let wrong_run_status = backend.recovery_status(&wrong_run).await;
        let mut ssh_generation = session.clone();
        ssh_generation.connection_generation = Some(42);
        let generation_status = backend.recovery_status(&ssh_generation).await;
        let generation_read = backend.read(&ssh_generation, 0).await;
        let generation_write = backend
            .write(&ssh_generation, &lease, b"must-not-run\r\n")
            .await;
        let generation_close = backend.close(&ssh_generation, &lease).await;
        let current_status = backend.recovery_status(&session).await;
        backend.close(&session, &lease).await.unwrap();

        assert!(
            wrong_run_status.is_err(),
            "recovery must check the full session owner"
        );
        assert!(generation_status.is_err());
        assert!(generation_read.is_err());
        assert!(generation_write.is_err());
        assert!(generation_close.is_err());
        assert!(current_status.unwrap().available);
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn h293_recovery_status_checks_actual_child_exit_and_cleanup() {
        let (_dir, svc, backend, session, lease) = h293_shared_fixture().await;
        let initial = backend.recovery_status(&session).await.unwrap();
        backend.write(&session, &lease, b"exit\r\n").await.unwrap();
        let exited = tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                let status = backend.recovery_status(&session).await.unwrap();
                if !status.available {
                    break status;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await;
        let retained = svc.pty_exists(&session.session_id).await;
        backend.close(&session, &lease).await.unwrap();
        assert!(initial.available && initial.control_available);
        assert!(retained, "the exited child remains owned until close");
        assert!(!exited.unwrap().control_available);
        let closed = backend.recovery_status(&session).await.unwrap();
        assert!(!closed.available && !closed.control_available);
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn h293_shared_close_durable_failure_retains_reaped_owner_for_retry() {
        let (dir, svc, backend, session, lease) = h293_shared_fixture().await;
        // A separate connection injects a failure after real kill/wait, not a
        // fake backend success. Revoke must be retried without resurrecting PTY.
        let db = Database::open(dir.path().join("lease.sqlite")).unwrap();
        db.with_conn(|conn| {
            conn.execute_batch("CREATE TRIGGER h293_revoke_fail BEFORE UPDATE OF revoked_at ON execution_leases BEGIN SELECT RAISE(ABORT,'h293 revoke fault'); END;").unwrap();
            Ok(())
        }).unwrap();
        let first = backend.close(&session, &lease).await;
        let retained = svc.pty_exists(&session.session_id).await;
        let status = backend.recovery_status(&session).await.unwrap();
        let input = backend
            .write(&session, &lease, b"echo must-not-run\r\n")
            .await;
        db.with_conn(|conn| {
            conn.execute_batch("DROP TRIGGER h293_revoke_fail;")
                .unwrap();
            Ok(())
        })
        .unwrap();
        backend.close(&session, &lease).await.unwrap();
        assert!(first.is_err() && retained && input.is_err());
        assert!(!status.available && !status.control_available);
        assert!(!svc.pty_exists(&session.session_id).await);
    }

    struct RejectAcquire;

    impl TerminalLeasePersistence for RejectAcquire {
        fn acquire(
            &self,
            _: &TerminalInputLease,
            _: i64,
        ) -> deepagent_terminal::TerminalResult<u64> {
            Err(TerminalError::Backend("h293 acquire fault".into()))
        }
        fn transfer(
            &self,
            _: &TerminalInputLease,
            _: &TerminalInputLease,
            _: i64,
            _: &str,
        ) -> deepagent_terminal::TerminalResult<u64> {
            Err(TerminalError::InvalidLease)
        }
        fn validate(
            &self,
            _: &TerminalInputLease,
            _: i64,
        ) -> deepagent_terminal::TerminalResult<bool> {
            Ok(false)
        }
        fn revoke(
            &self,
            _: &TerminalInputLease,
            _: i64,
            _: &str,
        ) -> deepagent_terminal::TerminalResult<()> {
            Err(TerminalError::InvalidLease)
        }
    }

    #[tokio::test]
    async fn h293_open_does_not_spawn_before_durable_owner_acquisition() {
        let dir = tempfile::tempdir().unwrap();
        let (svc, _, _) = service();
        let svc = Arc::new(svc);
        let backend = DirectTerminalSessionBackend::with_lease_persistence(
            svc.clone(),
            TerminalShell::CommandPrompt,
            Some(Arc::new(RejectAcquire)),
        );
        let error = backend
            .open(TerminalOpenRequest {
                target: Default::default(),
                run_id: "h293-open".into(),
                cwd: dir
                    .path()
                    .join("nonexistent")
                    .to_string_lossy()
                    .into_owned(),
                cols: 0,
                rows: 24,
                initial_holder: TerminalInputHolder::User,
            })
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("h293 acquire fault"),
            "lease acquisition must precede any attempt to create a zero-column native PTY: {error}"
        );
        assert!(svc.ptys.read().await.is_empty());
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn h293_failed_pty_creation_releases_the_preacquired_lease() {
        let dir = tempfile::tempdir().unwrap();
        let db = Arc::new(Database::open_in_memory().unwrap());
        let store = Arc::new(SqliteTerminalLeaseStore::new(db.clone(), 60_000).unwrap());
        let svc = Arc::new(TerminalService::with_executor(
            Arc::new(ProjectService::new(db.clone())),
            dir.path().to_string_lossy(),
            Arc::new(SystemExecutor),
        ));
        let backend = DirectTerminalSessionBackend::with_lease_persistence(
            svc.clone(),
            TerminalShell::CommandPrompt,
            Some(store),
        );
        // portable-pty 0.8.1 forwards cols/rows to CreatePseudoConsole;
        // Microsoft winconpty rejects size.X == 0 with E_INVALIDARG.
        // Unlike a nonexistent cwd (which this dependency falls back from),
        // this is an actual native PTY creation error before any shell exists.
        assert!(backend
            .open(TerminalOpenRequest {
                target: Default::default(),
                run_id: "h293-open".into(),
                cwd: dir
                    .path()
                    .join("nonexistent")
                    .to_string_lossy()
                    .into_owned(),
                cols: 0,
                rows: 24,
                initial_holder: TerminalInputHolder::User,
            })
            .await
            .is_err());
        assert!(svc.ptys.read().await.is_empty());
        assert!(!backend.leases.control_available("local-pty-1").unwrap());
        db.with_conn(|conn| {
            let (total, active): (i64, i64) = conn
                .query_row(
                    "SELECT COUNT(*),COALESCE(SUM(revoked_at IS NULL),0) FROM execution_leases",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .unwrap();
            assert_eq!((total, active), (1, 0));
            Ok(())
        })
        .unwrap();
    }

    #[tokio::test]
    async fn runs_in_active_project_dir() {
        let (svc, exec, projects) = service();
        projects.add_project("/work/proj").unwrap();
        let res = svc.run("echo hi").await.unwrap();
        assert_eq!(res.cwd, "/work/proj");
        assert_eq!(res.exit_code, Some(0));
        assert!(!res.blocked);
        assert_eq!(exec.ran.lock().unwrap()[0].1, "/work/proj");
    }

    #[tokio::test]
    async fn falls_back_to_default_cwd() {
        let (svc, _exec, _projects) = service();
        let res = svc.run("echo hi").await.unwrap();
        assert_eq!(res.cwd, "/default");
    }

    #[tokio::test]
    async fn dangerous_command_is_blocked_not_run() {
        let (svc, exec, _projects) = service();
        let res = svc.run("rm -rf /").await.unwrap();
        assert!(res.blocked);
        assert!(res.stderr.contains("high-risk"));
        // The executor was never invoked.
        assert!(exec.ran.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn empty_command_is_noop() {
        let (svc, exec, _projects) = service();
        let res = svc.run("   ").await.unwrap();
        assert!(!res.blocked);
        assert!(exec.ran.lock().unwrap().is_empty());
    }
}
