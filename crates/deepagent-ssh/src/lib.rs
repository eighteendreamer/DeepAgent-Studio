//! SSH long-lived connection service for DeepAgent Studio.
//!
//! Provides:
//! - [`SshService`] for connection lifecycle, config persistence, and runtime state.
//! - [`SshSession`] for one native SSH client plus optional PTY pipes.
//! - [`SshConnectionConfig`] / [`SshConnectionDto`] as the serialized shape used
//!   by Tauri commands and the frontend.
//!
//! Transport strategy:
//! Native SSH via `russh`, so password and key authentication stay
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

pub use agent::{
    probe_ssh_agent, SshAgentIdentity, SshAgentProbe, SshAgentProvider, SshAgentState,
};
pub use authentication_challenge::{
    SshAuthenticationChallenge, SshAuthenticationKind, SshAuthenticationPrompt,
};
pub use authentication_factors::{
    SshAuthenticationFactor, SshAuthenticationFactorDto, SshAuthenticationFactorRequest,
    MAX_AUTHENTICATION_FACTORS,
};
pub use config::{
    CreateSshConnectionRequest, SshAuthType, SshConnectionConfig, SshConnectionDto,
    SshHostKeyPolicy, SshStatus, UpdateSshConnectionRequest,
};
pub use connection_challenge::{SshConnectionAttempt, SshHostKeyChallenge, SshHostKeyDecision};
pub use default_identities::{
    discover_default_ssh_identities, discover_default_ssh_identities_in, SshDefaultIdentity,
    SshDefaultIdentityState,
};
pub use directory_plan::{DirectoryTransferEntry, DirectoryTransferPlan};
pub use directory_transfer::{
    DirectoryConflictPolicy, DirectoryConflictResolver, DirectoryFileConflict,
    DirectoryFileConflictResolver, DirectoryRootConflict, DirectoryTransferOptions,
    DirectoryTransferProgress, DirectoryTransferReceipt, FileConflictDecision,
};
pub use directory_walk::{DirectoryWalkLimits, RemoteDirectoryEntry, RemoteDirectoryTree};
pub use error::{SshError, SshResult};
pub use file_transfer::{
    FileConflictPolicy, FileTransferDirection, FileTransferError, FileTransferProgress,
    FileTransferReceipt, FileTransferRequest,
};
pub use files::{RemoteFileKind, RemoteFileMetadata, RemoteRealpath};
pub use host_key_rotation::{SshHostKeyRotationPreview, SshHostKeyRotationResult};
pub use host_key_store::{
    SshHostIdentity, SshHostKeyRecord, SshHostKeyRotationRecord, SshHostKeyStore,
};
pub use host_trust::{SshHostTrustInspection, SshKnownHostKey};
pub use host_trust_audit::{
    SshHostKeyFingerprint, SshHostTrustAction, SshHostTrustAudit, SshHostTrustAuditRecord,
};
pub use jump::{
    SshConnectionRole, SshJumpConfig, SshJumpDto, SshJumpEndpoint, SshJumpRequest,
    SshJumpSettingsRequest,
};
pub use known_hosts_import::{
    SshKnownHostsImportPreview, SshKnownHostsImportResult, SshKnownHostsImportRow,
    SshKnownHostsImportStatus,
};
pub use openssh_config::{
    preview_ssh_config, preview_ssh_config_in, resolve_ssh_config, SshConfigImportPreview,
    SshConfigImportedConnection, SshConfigImportedEndpoint, SshConfigOverrides,
};
pub use proxy::{
    SshProxyConfig, SshProxyDto, SshProxyEndpoint, SshProxyRequest, SshProxySettingsRequest,
    SshProxyType,
};
pub use remote::{
    RemoteBundleManifest, RemoteBundleRequest, RemoteBundleResult, RemoteInstallRequest,
    RemoteInstallResult, RemoteManifestEntry, RemoteProbeResult, RemotePushFileRequest,
    RemotePushFileResult, RemoteRequireRequest, RemoteRequireResult, RemoteRuntimeRequirement,
    RemoteVerifyMode,
};
pub use session::{
    SshDirEntry, SshDirListing, SshExecResult, SshFileBinary, SshFileContent, SshPtyHandle,
    SshSession, SshStatusSnapshot, SshTestResult,
};
pub use timing::SshConnectionTiming;
pub use transport::{SshClient, SshCommandOutput};

mod agent;
mod authentication_challenge;
mod authentication_factors;
mod config;
mod conflict_target;
mod connection_challenge;
mod connection_flow;
mod connection_lease;
mod connection_operation;
mod default_identities;
mod directory_plan;
mod directory_publication;
mod directory_transfer;
mod directory_walk;
mod error;
mod file_transfer;
mod files;
mod host_key_activity;
mod host_key_rotation;
mod host_key_store;
mod host_trust;
mod host_trust_audit;
mod jump;
mod known_hosts_import;
mod openssh_config;
mod private_identity;
mod proxy;
mod pty;
mod public_key_candidates;
mod remote;
mod service;
mod session;
mod sftp_packet_stream;
mod timing;
mod transport;
mod transport_authentication;

use async_trait::async_trait;
use deepagent_terminal::{
    TerminalError, TerminalInputHolder, TerminalInputLease, TerminalLeasePersistence,
    TerminalLeaseRegistry, TerminalOpenRequest, TerminalReadChunk, TerminalSession,
    TerminalSessionBackend, TerminalSignal, TerminalTarget,
};
use service::SshServiceImpl;
use std::sync::Arc;

/// Persistence boundary for SSH connection configuration. Implementations may
/// use encrypted SQLite without coupling this transport crate to app-core.
#[async_trait]
pub trait SshConfigStore: Send + Sync {
    /// Private-key bodies must never be persisted by the legacy plaintext JSON adapter.
    fn supports_secret_storage(&self) -> bool {
        false
    }
    async fn load(&self) -> SshResult<Vec<SshConnectionConfig>>;
    async fn save(&self, configs: &[SshConnectionConfig]) -> SshResult<()>;
}

/// SSH implementation of the shared terminal-session contract. The service
/// remains the owner of connection lifecycle; this adapter only adds run
/// scope, cursor and input-lease validation.
pub struct SshTerminalSessionBackend {
    service: Arc<SshService>,
    default_connection: Option<(String, String)>,
    leases: TerminalLeaseRegistry,
    controls: std::sync::Mutex<HashMap<String, std::sync::Weak<tokio::sync::Mutex<()>>>>,
}

struct PendingTerminalLease<'a> {
    registry: &'a TerminalLeaseRegistry,
    lease: Option<TerminalInputLease>,
}

impl Drop for PendingTerminalLease<'_> {
    fn drop(&mut self) {
        if let Some(lease) = self.lease.take() {
            if let Err(error) = self.registry.remove(&lease) {
                tracing::warn!(target:"deepagent_ssh", %error, "failed to revoke unpublished terminal lease");
            }
        }
    }
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
            default_connection: Some((connection_id.into(), token.into())),
            controls: std::sync::Mutex::new(HashMap::new()),
            leases: persistence
                .map(TerminalLeaseRegistry::with_persistence)
                .unwrap_or_default(),
        }
    }

    /// One SSH adapter for the desktop coordinator; target selection stays explicit.
    pub fn for_service(
        service: Arc<SshService>,
        persistence: Option<Arc<dyn TerminalLeasePersistence>>,
    ) -> Self {
        Self {
            service,
            default_connection: None,
            controls: std::sync::Mutex::new(HashMap::new()),
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

    pub async fn recovery_status(
        &self,
        session_id: &str,
    ) -> deepagent_terminal::TerminalResult<deepagent_terminal::TerminalRecoveryStatus> {
        let cursor = self.leases.last_cursor(session_id)?;
        let target = session_id
            .strip_prefix("ssh:")
            .and_then(|id| id.rsplit_once(':'));
        let available = if let Some((connection, token)) = target {
            let session = self
                .service
                .inner
                .sessions
                .read()
                .await
                .get(connection)
                .cloned();
            match session {
                Some(session) => session.pty_running(token).await,
                None => false,
            }
        } else {
            false
        };
        Ok(deepagent_terminal::TerminalRecoveryStatus {
            cursor,
            available,
            control_available: available && self.leases.control_available(session_id)?,
        })
    }

    // Only overlapping operations share a gate; idle/closed sessions keep no lock owner.
    // A stalled open or write on one tab cannot block controls on another tab.
    fn input_gate(&self, id: &str) -> Arc<tokio::sync::Mutex<()>> {
        let mut gates = self
            .controls
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        gates.retain(|_, gate| gate.strong_count() > 0);
        if let Some(gate) = gates.get(id).and_then(std::sync::Weak::upgrade) {
            return gate;
        }
        let gate = Arc::new(tokio::sync::Mutex::new(()));
        gates.insert(id.to_owned(), Arc::downgrade(&gate));
        gate
    }

    fn handle(&self, session: &TerminalSession) -> Result<SshServiceHandle, TerminalError> {
        if session.backend != self.backend_kind() || !session.session_id.starts_with("ssh:") {
            return Err(TerminalError::SessionNotFound(session.session_id.clone()));
        }
        self.leases
            .validate_scope(&session.session_id, &session.run_id)?;
        let (connection_id, token) = session
            .session_id
            .strip_prefix("ssh:")
            .and_then(|id| id.rsplit_once(':'))
            .filter(|(connection, token)| !connection.is_empty() && !token.is_empty())
            .ok_or(TerminalError::InvalidLease)?;
        Ok(SshServiceHandle::new(
            connection_id,
            token,
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

    async fn subscribe_output(
        &self,
        session: &TerminalSession,
    ) -> deepagent_terminal::TerminalResult<deepagent_terminal::TerminalOutputSubscription> {
        let handle = self.handle(session)?;
        self.service
            .inner
            .pty_subscribe_output(&handle)
            .await
            .map_err(|error| TerminalError::Backend(error.to_string()))
    }

    async fn open(
        &self,
        request: TerminalOpenRequest,
    ) -> deepagent_terminal::TerminalResult<(TerminalSession, TerminalInputLease)> {
        let connection_id = match &request.target {
            TerminalTarget::Ssh { connection_id } => connection_id.clone(),
            TerminalTarget::Local => self
                .default_connection
                .as_ref()
                .map(|(id, _)| id.clone())
                .ok_or_else(|| TerminalError::UnsupportedCapability {
                    backend: "ssh".into(),
                    capability: "explicit_ssh_target_required".into(),
                })?,
        };
        let token = uuid::Uuid::new_v4().to_string();
        let session = TerminalSession {
            session_id: format!("ssh:{connection_id}:{token}"),
            run_id: request.run_id,
            backend: self.backend_kind().into(),
            cols: request.cols,
            rows: request.rows,
        };
        let lease =
            self.leases
                .register(&session.session_id, &session.run_id, request.initial_holder)?;
        let mut pending = PendingTerminalLease {
            registry: &self.leases,
            lease: Some(lease.clone()),
        };
        let base =
            SshServiceHandle::new(&connection_id, &connection_id, request.cols, request.rows);
        if let Err(error) = self
            .service
            .inner
            .spawn_pty(&base, request.cols, request.rows, token, false)
            .await
        {
            return Err(TerminalError::Backend(error.to_string()));
        }
        pending.lease = None;
        Ok((session, lease))
    }

    async fn write(
        &self,
        session: &TerminalSession,
        lease: &TerminalInputLease,
        data: &[u8],
    ) -> deepagent_terminal::TerminalResult<()> {
        let gate = self.input_gate(&session.session_id);
        let _control = gate.lock().await;
        self.validate(session, lease)?;
        let handle = self.handle(session)?;
        self.service
            .pty_write(&handle, data)
            .await
            .map_err(|e| TerminalError::Backend(e.to_string()))
    }

    async fn renew(
        &self,
        session: &TerminalSession,
        lease: &TerminalInputLease,
    ) -> deepagent_terminal::TerminalResult<()> {
        self.handle(session)?;
        let gate = self.input_gate(&session.session_id);
        let _control = gate.lock().await;
        self.validate(session, lease)?;
        self.leases.renew(lease)
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
        if chunk.cursor != after_cursor {
            self.leases
                .record_cursor(&session.session_id, chunk.cursor)?;
        }
        Ok(chunk)
    }

    async fn resize(
        &self,
        session: &TerminalSession,
        lease: &TerminalInputLease,
        cols: u16,
        rows: u16,
    ) -> deepagent_terminal::TerminalResult<()> {
        let gate = self.input_gate(&session.session_id);
        let _control = gate.lock().await;
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
        let gate = self.input_gate(&session.session_id);
        let _control = gate.lock().await;
        self.validate(session, lease)?;
        let data = match signal {
            TerminalSignal::Interrupt => vec![3],
            TerminalSignal::Terminate | TerminalSignal::Kill => {
                return Err(TerminalError::UnsupportedCapability {
                    backend: "ssh".into(),
                    capability: "reliable_remote_process_termination".into(),
                })
            }
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
        let gate = self.input_gate(&session.session_id);
        let _control = gate.lock().await;
        self.handle(session)?;
        self.leases.takeover(&session.session_id, holder)
    }

    async fn release(
        &self,
        session: &TerminalSession,
        lease: &TerminalInputLease,
        next_holder: TerminalInputHolder,
    ) -> deepagent_terminal::TerminalResult<TerminalInputLease> {
        let gate = self.input_gate(&session.session_id);
        let _control = gate.lock().await;
        self.validate(session, lease)?;
        self.leases.release(lease, next_holder)
    }

    async fn close(
        &self,
        session: &TerminalSession,
        lease: &TerminalInputLease,
    ) -> deepagent_terminal::TerminalResult<()> {
        let gate = self.input_gate(&session.session_id);
        let _control = gate.lock().await;
        self.validate(session, lease)?;
        let handle = self.handle(session)?;
        self.service
            .pty_close(&handle)
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
    /// Set the application's idle resource retention policy before sharing it.
    pub fn with_connection_idle_timeout(mut self, timeout: std::time::Duration) -> SshResult<Self> {
        if timeout.is_zero() || timeout > std::time::Duration::from_secs(86_400) {
            return Err(SshError::InvalidConfig(
                "SSH connection idle timeout must be positive and at most one day".into(),
            ));
        }
        self.inner.connection_idle_timeout = timeout;
        Ok(self)
    }

    /// Reclaim expired connections after all owned resources have been released.
    pub async fn reap_idle_connections(&self) -> SshResult<usize> {
        self.inner.reap_idle_connections().await
    }

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

    /// Use the application's existing SQLite database for explicit host trust.
    pub fn with_host_key_store(mut self, store: Arc<SshHostKeyStore>) -> Self {
        self.inner.host_key_store = Some(store);
        self
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

    /// Inspect the local trust source without networking, authentication or enrollment.
    pub async fn inspect_host_trust(
        &self,
        host: &str,
        port: u16,
        known_hosts_path: Option<&str>,
    ) -> SshResult<SshHostTrustInspection> {
        self.inner
            .inspect_host_trust(host, port, known_hosts_path)
            .await
    }

    /// Inspect the saved connection's trust file, including its current route identity.
    pub async fn inspect_connection_host_trust(
        &self,
        id: &str,
    ) -> SshResult<SshHostTrustInspection> {
        self.inner.inspect_connection_host_trust(id).await
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

    pub async fn list_host_keys(&self, id: &str) -> SshResult<Vec<SshHostKeyRecord>> {
        self.inner.list_host_keys(id).await
    }

    pub async fn begin_host_key_rotation(&self, id: &str) -> SshResult<SshHostKeyRotationPreview> {
        self.inner.begin_host_key_rotation(id).await
    }

    pub async fn host_trust_audit(&self, id: &str) -> SshResult<SshHostTrustAudit> {
        self.inner.host_trust_audit(id).await
    }

    pub async fn confirm_host_key_rotation(
        &self,
        id: &str,
        attempt_id: &str,
        challenge_id: &str,
    ) -> SshResult<SshHostKeyRotationResult> {
        self.inner
            .confirm_host_key_rotation(id, attempt_id, challenge_id)
            .await
    }

    pub async fn import_host_trust(
        &self,
        id: &str,
        expected: SshHostTrustInspection,
    ) -> SshResult<Vec<SshHostKeyRecord>> {
        self.inner.import_host_trust(id, expected).await
    }

    pub async fn revoke_host_key(&self, id: &str, fingerprint: &str) -> SshResult<()> {
        self.inner.revoke_host_key(id, fingerprint).await
    }

    pub async fn preview_known_hosts_import(
        &self,
        id: &str,
    ) -> SshResult<SshKnownHostsImportPreview> {
        self.inner.preview_known_hosts_import(id).await
    }

    pub async fn import_known_hosts(
        &self,
        id: &str,
        expected: SshKnownHostsImportPreview,
    ) -> SshResult<SshKnownHostsImportResult> {
        self.inner.import_known_hosts(id, expected).await
    }

    pub async fn connect(&self, id: &str) -> SshResult<SshServiceHandle> {
        self.inner.connect(id).await
    }

    pub async fn begin_connection(&self, id: &str) -> SshResult<SshConnectionAttempt> {
        self.inner.begin_connection(id).await
    }

    pub async fn respond_host_key(
        &self,
        id: &str,
        attempt_id: &str,
        challenge_id: &str,
        decision: SshHostKeyDecision,
    ) -> SshResult<SshConnectionAttempt> {
        self.inner
            .respond_host_key(id, attempt_id, challenge_id, decision)
            .await
    }

    pub fn cancel_host_key(&self, id: &str, attempt_id: &str, challenge_id: &str) -> SshResult<()> {
        self.inner.cancel_host_key(id, attempt_id, challenge_id)
    }

    pub async fn respond_keyboard_interactive(
        &self,
        id: &str,
        attempt_id: &str,
        challenge_id: &str,
        responses: Vec<String>,
    ) -> SshResult<SshConnectionAttempt> {
        self.inner
            .respond_keyboard_interactive(id, attempt_id, challenge_id, responses)
            .await
    }

    pub async fn cancel_connection_attempt(
        &self,
        id: &str,
        attempt_id: &str,
        challenge_id: &str,
    ) -> SshResult<()> {
        self.inner
            .cancel_connection_attempt(id, attempt_id, challenge_id)
            .await
    }

    pub async fn exec(&self, handle: &SshServiceHandle, command: &str) -> SshResult<SshExecResult> {
        self.inner.exec(handle, command).await
    }

    pub async fn list_dir(
        &self,
        handle: &SshServiceHandle,
        path: &str,
    ) -> SshResult<SshDirListing> {
        self.inner.list_dir(handle, path).await
    }

    pub async fn read_file(
        &self,
        handle: &SshServiceHandle,
        path: &str,
    ) -> SshResult<SshFileContent> {
        self.inner.read_file(handle, path).await
    }

    /// lstat by default at the UI boundary. Following links must be explicit.
    pub async fn file_metadata(
        &self,
        handle: &SshServiceHandle,
        path: &str,
        follow_symlinks: bool,
    ) -> SshResult<RemoteFileMetadata> {
        self.inner
            .file_metadata(handle, path, follow_symlinks)
            .await
    }

    pub async fn realpath(
        &self,
        handle: &SshServiceHandle,
        path: &str,
    ) -> SshResult<RemoteRealpath> {
        self.inner.realpath(handle, path).await
    }

    pub async fn read_file_binary(
        &self,
        handle: &SshServiceHandle,
        path: &str,
    ) -> SshResult<SshFileBinary> {
        self.inner.read_file_binary(handle, path).await
    }

    pub async fn create_dir(&self, handle: &SshServiceHandle, path: &str) -> SshResult<()> {
        self.inner.create_dir(handle, path).await
    }

    pub async fn create_file(&self, handle: &SshServiceHandle, path: &str) -> SshResult<()> {
        self.inner.create_file(handle, path).await
    }

    pub async fn rename_path(
        &self,
        handle: &SshServiceHandle,
        old_path: &str,
        new_path: &str,
    ) -> SshResult<()> {
        self.inner.rename_path(handle, old_path, new_path).await
    }

    pub async fn remove_path(&self, handle: &SshServiceHandle, path: &str) -> SshResult<()> {
        self.inner.remove_path(handle, path).await
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

    pub async fn pty_close(&self, handle: &SshServiceHandle) -> SshResult<()> {
        self.inner.pty_close(handle).await
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
        if dto.jump.is_some() {
            let cfg = self.inner.config_for(&dto.id).await?;
            return self.inner.test_connection(&cfg).await;
        }
        let cfg = SshConnectionConfig {
            id: dto.id.clone(),
            name: dto.name.clone(),
            host: dto.host.clone(),
            port: dto.port,
            username: dto.username.clone(),
            auth_type: dto.auth_type,
            agent_fingerprint: dto.agent_fingerprint.clone(),
            authentication_factors: Vec::new(),
            jump: None,
            timing: dto.timing,
            proxy: dto.proxy.as_ref().map(|proxy| SshProxyConfig {
                mode: proxy.mode,
                host: proxy.host.clone(),
                port: proxy.port,
                username: proxy.username.clone(),
                password: None,
            }),
            key_path: dto.key_path.clone(),
            key_content: None,
            certificate_path: dto.certificate_path.clone(),
            known_hosts_path: dto.known_hosts_path.clone(),
            host_key_policy: dto.host_key_policy,
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
    async fn h293_renew_and_takeover_keep_the_registered_run_scope_without_network() {
        let dir = tempfile::tempdir().unwrap();
        let service = Arc::new(SshService::with_config_store(
            dir.path().to_path_buf(),
            Arc::new(MemoryConfigStore(Mutex::new(Vec::new()))),
        ));
        let backend = SshTerminalSessionBackend::new(service, "synthetic", "synthetic-token");
        let session = TerminalSession {
            session_id: "ssh:synthetic:synthetic-token".into(),
            run_id: "h293-run".into(),
            backend: "ssh".into(),
            cols: 80,
            rows: 24,
        };
        let old = backend
            .leases
            .register(
                &session.session_id,
                &session.run_id,
                TerminalInputHolder::User,
            )
            .unwrap();
        backend.renew(&session, &old).await.unwrap();
        let mut wrong = session.clone();
        wrong.run_id = "different-run".into();
        assert!(backend.renew(&wrong, &old).await.is_err());
        assert!(backend
            .takeover(&wrong, TerminalInputHolder::Runtime)
            .await
            .is_err());
        let next = backend
            .takeover(&session, TerminalInputHolder::Runtime)
            .await
            .unwrap();
        assert!(backend.renew(&session, &old).await.is_err());
        backend.renew(&session, &next).await.unwrap();
        backend.leases.remove(&next).unwrap();
        // No SSH connection, agent, socket or remote process was opened.
    }

    #[tokio::test]
    async fn unpublished_lease_is_revoked_on_drop_and_control_gates_are_per_session() {
        let leases = TerminalLeaseRegistry::default();
        let lease = leases
            .register("unpublished", "fixture", TerminalInputHolder::User)
            .unwrap();
        {
            let _pending = PendingTerminalLease {
                registry: &leases,
                lease: Some(lease.clone()),
            };
            assert!(leases.validate(&lease).is_ok());
        }
        assert!(leases.validate(&lease).is_err());
        let data = tempfile::tempdir().unwrap();
        let backend = SshTerminalSessionBackend::for_service(
            Arc::new(SshService::new(data.path().to_path_buf())),
            None,
        );
        let first = backend.input_gate("first");
        let held = first.lock().await;
        assert!(backend.input_gate("first").try_lock().is_err());
        assert!(backend.input_gate("second").try_lock().is_ok());
        drop(held);
        assert!(backend.input_gate("first").try_lock().is_ok());
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
                agent_fingerprint: None,
                authentication_factors: None,
                proxy: None,
                jump: None,
                timing: Default::default(),
                key_path: None,
                key_content: None,
                certificate_path: None,
                known_hosts_path: None,
                host_key_policy: SshHostKeyPolicy::Strict,
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
