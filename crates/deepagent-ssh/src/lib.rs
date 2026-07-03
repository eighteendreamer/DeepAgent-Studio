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
//! Connection configs are stored in the app data directory
//! (`<app_data>/deepagent-ssh/connections.json`) so they survive app restarts.

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

use service::SshServiceImpl;

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
