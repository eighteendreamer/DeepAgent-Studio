//! SSH long-lived connection service for DeepAgent Studio.
//!
//! Provides:
//! - [`SshService`] for connection lifecycle, config persistence, and runtime state.
//! - [`SshSession`] for one native SSH client plus optional PTY pipes.
//! - [`SshConnectionConfig`] / [`SshConnectionDto`] as the serialized shape used
//!   by Tauri commands and the frontend.
//!
//! Transport strategy:
//! Native SSH via `async-ssh2-tokio`, so password and key authentication stay
//! inside the Rust binary without relying on `ssh.exe`, `plink`, or `sshpass`.
//!
//! Persistence:
//! The desktop application injects an encrypted SQLite-backed
//! [`SshConfigStore`], so connection metadata and credentials survive app
//! restarts without remaining in a plaintext JSON file. The default
//! compatibility constructor still uses the legacy JSON store for callers
//! outside the desktop application; it is not the desktop production path.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

pub use config::{
    CreateSshConnectionRequest, SshAuthType, SshConnectionConfig, SshConnectionDto, SshStatus,
    UpdateSshConnectionRequest,
};
pub use error::{SshError, SshResult};
pub use remote::{
    RemoteBundleManifest, RemoteBundleRequest, RemoteBundleResult, RemoteInstallRequest,
    RemoteInstallResult, RemoteManifestEntry, RemoteProbeResult, RemotePushFileRequest,
    RemotePushFileResult, RemoteRequireRequest, RemoteRequireResult, RemoteRuntimeRequirement,
    RemoteVerifyMode,
};
pub use session::{SshExecResult, SshPtyHandle, SshSession, SshStatusSnapshot, SshTestResult};

mod config;
mod error;
mod remote;
mod service;
mod session;

use async_trait::async_trait;
use deepagent_terminal::{
    TerminalError, TerminalInputHolder, TerminalInputLease, TerminalLeasePersistence,
    TerminalLeaseRegistry, TerminalOpenRequest, TerminalReadChunk, TerminalSession,
    TerminalSessionBackend, TerminalSignal,
};
use service::SshServiceImpl;
use std::sync::Arc;

/// Persistence boundary for SSH connection configuration. Implementations may
/// use encrypted SQLite without coupling this transport crate to app-core.
#[async_trait]
pub trait SshConfigStore: Send + Sync {
    async fn load(&self) -> SshResult<Vec<SshConnectionConfig>>;
    async fn save(&self, configs: &[SshConnectionConfig]) -> SshResult<()>;
}

/// SSH implementation of the shared terminal-session contract. The service
/// remains the owner of connection lifecycle; this adapter only adds run
/// scope, cursor and input-lease validation.
pub struct SshTerminalSessionBackend {
    service: Arc<SshService>,
    connection_id: String,
    token: String,
    leases: TerminalLeaseRegistry,
}

impl SshTerminalSessionBackend {
    pub fn new(
        service: Arc<SshService>,
        connection_id: impl Into<String>,
        token: impl Into<String>,
    ) -> Self {
        Self::with_lease_persistence(service, connection_id, token, None)
    }

    /// Construct the SSH adapter with the shared durable lease store so
    /// takeover and fencing survive adapter/process recreation.
    pub fn with_lease_persistence(
        service: Arc<SshService>,
        connection_id: impl Into<String>,
        token: impl Into<String>,
        persistence: Option<Arc<dyn TerminalLeasePersistence>>,
    ) -> Self {
        Self {
            service,
            connection_id: connection_id.into(),
            token: token.into(),
            leases: persistence
                .map(TerminalLeaseRegistry::with_persistence)
                .unwrap_or_default(),
        }
    }

    /// Return the last output cursor durably acknowledged for this session.
    /// A zero value is returned by the compatibility in-memory registry.
    pub fn last_cursor(&self, session_id: &str) -> deepagent_terminal::TerminalResult<u64> {
        self.leases.last_cursor(session_id)
    }

    fn handle(&self, session: &TerminalSession) -> Result<SshServiceHandle, TerminalError> {
        if session.backend != self.backend_kind() || !session.session_id.starts_with("ssh:") {
            return Err(TerminalError::SessionNotFound(session.session_id.clone()));
        }
        Ok(SshServiceHandle::new(
            self.connection_id.clone(),
            self.token.clone(),
            session.cols,
            session.rows,
        ))
    }

    fn validate(
        &self,
        session: &TerminalSession,
        lease: &TerminalInputLease,
    ) -> Result<(), TerminalError> {
        if session.session_id != lease.session_id || session.run_id != lease.run_id {
            return Err(TerminalError::InvalidLease);
        }
        self.leases.validate(lease)
    }
}

#[async_trait]
impl TerminalSessionBackend for SshTerminalSessionBackend {
    fn backend_kind(&self) -> &'static str {
        "ssh"
    }

    async fn open(
        &self,
        request: TerminalOpenRequest,
    ) -> deepagent_terminal::TerminalResult<(TerminalSession, TerminalInputLease)> {
        let base =
            SshServiceHandle::new(&self.connection_id, &self.token, request.cols, request.rows);
        let handle = self
            .service
            .pty_spawn(&base, request.cols, request.rows)
            .await
            .map_err(|e| TerminalError::Backend(e.to_string()))?;
        let session = TerminalSession {
            session_id: format!("ssh:{}:{}", self.connection_id, handle.token),
            run_id: request.run_id,
            backend: self.backend_kind().to_string(),
            cols: request.cols,
            rows: request.rows,
        };
        let lease =
            self.leases
                .register(&session.session_id, &session.run_id, request.initial_holder)?;
        Ok((session, lease))
    }

    async fn write(
        &self,
        session: &TerminalSession,
        lease: &TerminalInputLease,
        data: &[u8],
    ) -> deepagent_terminal::TerminalResult<()> {
        self.validate(session, lease)?;
        let handle = self.handle(session)?;
        self.service
            .pty_write(&handle, data)
            .await
            .map_err(|e| TerminalError::Backend(e.to_string()))
    }

    async fn read(
        &self,
        session: &TerminalSession,
        after_cursor: u64,
    ) -> deepagent_terminal::TerminalResult<TerminalReadChunk> {
        let handle = self.handle(session)?;
        let chunk = self
            .service
            .pty_read_with_cursor(&handle, after_cursor)
            .await
            .map_err(|e| TerminalError::Backend(e.to_string()))?;
        // Keep SSH and Direct backends on the same durable cursor contract.
        // The cursor is an acknowledgement point only; the PTY history remains
        // owned by the SSH service and may still be unavailable after restart.
        self.leases
            .record_cursor(&session.session_id, chunk.cursor)?;
        Ok(chunk)
    }

    async fn resize(
        &self,
        session: &TerminalSession,
        lease: &TerminalInputLease,
        cols: u16,
        rows: u16,
    ) -> deepagent_terminal::TerminalResult<()> {
        self.validate(session, lease)?;
        let handle = self.handle(session)?;
        self.service
            .pty_resize(&handle, cols, rows)
            .await
            .map_err(|e| TerminalError::Backend(e.to_string()))
    }

    async fn signal(
        &self,
        session: &TerminalSession,
        lease: &TerminalInputLease,
        signal: TerminalSignal,
    ) -> deepagent_terminal::TerminalResult<()> {
        self.validate(session, lease)?;
        let data = match signal {
            TerminalSignal::Interrupt => vec![3],
            TerminalSignal::Terminate => vec![28],
            TerminalSignal::Kill => vec![4],
        };
        let handle = self.handle(session)?;
        self.service
            .pty_write(&handle, &data)
            .await
            .map_err(|e| TerminalError::Backend(e.to_string()))
    }

    async fn takeover(
        &self,
        session: &TerminalSession,
        holder: TerminalInputHolder,
    ) -> deepagent_terminal::TerminalResult<TerminalInputLease> {
        self.handle(session)?;
        self.leases.takeover(&session.session_id, holder)
    }

    async fn release(
        &self,
        session: &TerminalSession,
        lease: &TerminalInputLease,
        next_holder: TerminalInputHolder,
    ) -> deepagent_terminal::TerminalResult<TerminalInputLease> {
        self.validate(session, lease)?;
        self.leases.release(lease, next_holder)
    }

    async fn close(
        &self,
        session: &TerminalSession,
        lease: &TerminalInputLease,
    ) -> deepagent_terminal::TerminalResult<()> {
        self.validate(session, lease)?;
        self.service
            .disconnect(&self.connection_id)
            .await
            .map_err(|e| TerminalError::Backend(e.to_string()))?;
        self.leases.remove(lease)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SshServiceHandle {
    pub connection_id: String,
    pub token: String,
    pub cols: u16,
    pub rows: u16,
}

impl SshServiceHandle {
    pub fn new(
        connection_id: impl Into<String>,
        token: impl Into<String>,
        cols: u16,
        rows: u16,
    ) -> Self {
        Self {
            connection_id: connection_id.into(),
            token: token.into(),
            cols,
            rows,
        }
    }
}

/// The SSH service — owns all connection configs and live sessions.
pub struct SshService {
    inner: SshServiceImpl,
}

impl SshService {
    pub fn new(app_data: PathBuf) -> Self {
        Self {
            inner: SshServiceImpl::new(app_data),
        }
    }

    pub fn with_config_store(app_data: PathBuf, store: Arc<dyn SshConfigStore>) -> Self {
        Self {
            inner: SshServiceImpl::with_config_store(app_data, store),
        }
    }

    pub async fn load(&self) -> SshResult<()> {
        self.inner.load().await
    }

    pub async fn persist_configs(&self) -> SshResult<()> {
        self.inner.persist_configs().await
    }

    pub async fn list_connections(&self) -> Vec<SshConnectionDto> {
        self.inner.list_connections().await
    }

    pub async fn create_connection(
        &self,
        request: CreateSshConnectionRequest,
    ) -> SshResult<SshConnectionDto> {
        self.inner.create_connection(request).await
    }

    pub async fn update_connection(
        &self,
        request: UpdateSshConnectionRequest,
    ) -> SshResult<SshConnectionDto> {
        self.inner.update_connection(request).await
    }

    pub async fn remove_connection(&self, id: &str) -> SshResult<()> {
        self.inner.remove_connection(id).await
    }

    pub async fn connect(&self, id: &str) -> SshResult<SshServiceHandle> {
        self.inner.connect(id).await
    }

    pub async fn exec(&self, handle: &SshServiceHandle, command: &str) -> SshResult<SshExecResult> {
        self.inner.exec(handle, command).await
    }

    pub async fn pty_spawn(
        &self,
        handle: &SshServiceHandle,
        cols: u16,
        rows: u16,
    ) -> SshResult<SshServiceHandle> {
        self.inner.pty_spawn(handle, cols, rows).await
    }

    pub async fn pty_write(&self, handle: &SshServiceHandle, data: &[u8]) -> SshResult<()> {
        self.inner.pty_write(handle, data).await
    }

    pub async fn pty_read(&self, handle: &SshServiceHandle) -> SshResult<Vec<u8>> {
        self.inner.pty_read(handle).await
    }

    pub async fn pty_read_with_cursor(
        &self,
        handle: &SshServiceHandle,
        after_cursor: u64,
    ) -> SshResult<TerminalReadChunk> {
        self.inner.pty_read_with_cursor(handle, after_cursor).await
    }

    pub async fn pty_resize(
        &self,
        handle: &SshServiceHandle,
        cols: u16,
        rows: u16,
    ) -> SshResult<()> {
        self.inner.pty_resize(handle, cols, rows).await
    }

    pub async fn disconnect(&self, id: &str) -> SshResult<()> {
        self.inner.disconnect(id).await
    }

    pub async fn status(&self, id: &str) -> SshResult<SshStatusSnapshot> {
        self.inner.status(id).await
    }

    pub async fn test_connection(
        &self,
        dto: &SshConnectionDto,
    ) -> SshResult<crate::session::SshTestResult> {
        let cfg = SshConnectionConfig {
            id: dto.id.clone(),
            name: dto.name.clone(),
            host: dto.host.clone(),
            port: dto.port,
            username: dto.username.clone(),
            auth_type: dto.auth_type,
            key_path: dto.key_path.clone(),
            password: None,
            extra_options: HashMap::new(),
            control_path: None,
            cached_status: dto.status,
            cached_last_error: dto.last_error.clone(),
            cached_latency_ms: dto.latency_ms,
            cached_checked_at_ms: None,
        };
        self.inner.test_connection(&cfg).await
    }

    pub async fn refresh_due_statuses(&self) -> SshResult<usize> {
        self.inner.refresh_due_statuses().await
    }

    pub async fn remote_probe(
        &self,
        id: &str,
        force_refresh: bool,
    ) -> SshResult<RemoteProbeResult> {
        self.inner.remote_probe(id, force_refresh).await
    }

    pub async fn remote_push_file(
        &self,
        id: &str,
        request: RemotePushFileRequest,
    ) -> SshResult<RemotePushFileResult> {
        self.inner.remote_push_file(id, request).await
    }

    pub async fn remote_push_bundle(
        &self,
        id: &str,
        request: RemoteBundleRequest,
    ) -> SshResult<RemoteBundleResult> {
        self.inner.remote_push_bundle(id, request).await
    }

    pub async fn remote_require(
        &self,
        id: &str,
        request: RemoteRequireRequest,
    ) -> SshResult<RemoteRequireResult> {
        self.inner.remote_require(id, request).await
    }

    pub async fn remote_install(
        &self,
        id: &str,
        request: RemoteInstallRequest,
    ) -> SshResult<RemoteInstallResult> {
        self.inner.remote_install(id, request).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct MemoryConfigStore(Mutex<Vec<SshConnectionConfig>>);

    #[async_trait]
    impl SshConfigStore for MemoryConfigStore {
        async fn load(&self) -> SshResult<Vec<SshConnectionConfig>> {
            Ok(self.0.lock().unwrap().clone())
        }

        async fn save(&self, configs: &[SshConnectionConfig]) -> SshResult<()> {
            *self.0.lock().unwrap() = configs.to_vec();
            Ok(())
        }
    }

    #[tokio::test]
    async fn service_persists_configs_through_injected_store() {
        let store = Arc::new(MemoryConfigStore(Mutex::new(Vec::new())));
        let service = SshService::with_config_store(
            std::env::temp_dir().join("deepagent-ssh-test"),
            store.clone(),
        );
        let created = service
            .create_connection(CreateSshConnectionRequest {
                name: "test".into(),
                host: "127.0.0.1".into(),
                port: 22,
                username: "tester".into(),
                auth_type: SshAuthType::Agent,
                key_path: None,
                password: None,
            })
            .await
            .unwrap();
        assert_eq!(store.0.lock().unwrap().len(), 1);

        let restored = SshService::with_config_store(
            std::env::temp_dir().join("deepagent-ssh-test-restored"),
            store,
        );
        restored.load().await.unwrap();
        let listed = restored.list_connections().await;
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, created.id);
    }
}
