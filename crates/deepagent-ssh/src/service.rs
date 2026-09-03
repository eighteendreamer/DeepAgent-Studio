use super::config::{
    CreateSshConnectionRequest, SshAuthType, SshConnectionConfig, SshConnectionDto, SshStatus,
    UpdateSshConnectionRequest,
};
use super::error::{SshError, SshResult};
use super::remote::{
    RemoteBundleManifest, RemoteBundleRequest, RemoteBundleResult, RemoteInstallRequest,
    RemoteInstallResult, RemoteManifestEntry, RemoteProbeResult, RemotePushFileRequest,
    RemotePushFileResult, RemoteRequireRequest, RemoteRequireResult, RemoteVerifyMode,
};
use super::session::{PtyState, SshExecResult, SshSession, SshStatusSnapshot, SshTestResult};
use super::{SshConfigStore, SshServiceHandle};
use async_ssh2_tokio::{AuthMethod, Client, ServerCheckMethod};
use async_trait::async_trait;
use sha2::Digest;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::SystemTime;
use tokio::io::AsyncReadExt;
use tokio::sync::{mpsc, RwLock};
use tokio::time::{timeout, Duration};

struct JsonFileSshConfigStore {
    path: PathBuf,
}

impl JsonFileSshConfigStore {
    fn new(path: PathBuf) -> Self {
        Self { path }
    }
}

#[async_trait]
impl SshConfigStore for JsonFileSshConfigStore {
    async fn load(&self) -> SshResult<Vec<SshConnectionConfig>> {
        let data = match tokio::fs::read_to_string(&self.path).await {
            Ok(data) => data,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(SshError::Persistence(error.to_string())),
        };
        let configs: HashMap<String, SshConnectionConfig> = serde_json::from_str(&data)
            .map_err(|error| SshError::Persistence(error.to_string()))?;
        Ok(configs.into_values().collect())
    }

    async fn save(&self, configs: &[SshConnectionConfig]) -> SshResult<()> {
        let values = configs
            .iter()
            .cloned()
            .map(|config| (config.id.clone(), config))
            .collect::<HashMap<_, _>>();
        let json = serde_json::to_string_pretty(&values)
            .map_err(|error| SshError::Persistence(error.to_string()))?;
        if let Some(parent) = self.path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|error| SshError::Persistence(error.to_string()))?;
        }
        tokio::fs::write(&self.path, json)
            .await
            .map_err(|error| SshError::Persistence(error.to_string()))
    }
}

pub struct SshServiceImpl {
    configs: RwLock<HashMap<String, SshConnectionConfig>>,
    sessions: RwLock<HashMap<String, Arc<SshSession>>>,
    config_store: Arc<dyn SshConfigStore>,
    data_root: PathBuf,
    loaded: AtomicBool,
}

impl SshServiceImpl {
    pub fn new(app_data: PathBuf) -> Self {
        let data_root = app_data.join("deepagent-ssh");
        let store = Arc::new(JsonFileSshConfigStore::new(
            data_root.join("connections.json"),
        ));
        Self::with_config_store(app_data, store)
    }

    pub fn with_config_store(app_data: PathBuf, config_store: Arc<dyn SshConfigStore>) -> Self {
        Self {
            configs: RwLock::new(HashMap::new()),
            sessions: RwLock::new(HashMap::new()),
            config_store,
            data_root: app_data.join("deepagent-ssh"),
            loaded: AtomicBool::new(false),
        }
    }

    pub async fn load(&self) -> SshResult<()> {
        let configs = self
            .config_store
            .load()
            .await?
            .into_iter()
            .map(|config| (config.id.clone(), config))
            .collect();
        *self.configs.write().await = configs;
        Ok(())
    }

    pub async fn persist_configs(&self) -> SshResult<()> {
        let configs = self.configs.read().await;
        let mut values = configs.values().cloned().collect::<Vec<_>>();
        values.sort_by(|left, right| left.id.cmp(&right.id));
        self.config_store.save(&values).await
    }

    pub async fn list_connections(&self) -> Vec<SshConnectionDto> {
        self.ensure_loaded().await;
        let configs = self.configs.read().await;
        let sessions = self.sessions.read().await;
        let mut out = Vec::with_capacity(configs.len());
        for cfg in configs.values() {
            let (status, last_error, latency_ms) = if let Some(s) = sessions.get(&cfg.id).cloned() {
                let live_status = if let Some(client) = s.client().await {
                    if client.is_closed() {
                        None
                    } else {
                        Some(s.status().await)
                    }
                } else {
                    None
                };
                (
                    live_status.unwrap_or(cfg.cached_status),
                    s.last_error()
                        .await
                        .or_else(|| cfg.cached_last_error.clone()),
                    cfg.cached_latency_ms,
                )
            } else {
                (
                    cfg.cached_status,
                    cfg.cached_last_error.clone(),
                    cfg.cached_latency_ms,
                )
            };
            out.push(SshConnectionDto {
                id: cfg.id.clone(),
                name: cfg.name.clone(),
                host: cfg.host.clone(),
                port: cfg.port,
                username: cfg.username.clone(),
                auth_type: cfg.auth_type,
                key_path: cfg.key_path.clone(),
                status,
                last_error,
                latency_ms,
            });
        }
        out.sort_by(|a, b| a.name.cmp(&b.name).then(a.host.cmp(&b.host)));
        out
    }

    pub async fn create_connection(
        &self,
        request: CreateSshConnectionRequest,
    ) -> SshResult<SshConnectionDto> {
        self.ensure_loaded().await;
        let mut config = SshConnectionConfig::new(
            request.name,
            request.host,
            request.port,
            request.username,
            request.auth_type,
        );
        validate_config(
            &config,
            request.key_path.as_deref(),
            request.password.as_deref(),
        )?;
        config.key_path = request.key_path.clone();
        config.password = request.password;
        let id = config.id.clone();
        let mut configs = self.configs.write().await;
        if configs.contains_key(&id) {
            return Err(SshError::AlreadyExists(id));
        }
        configs.insert(id.clone(), config.clone());
        drop(configs);
        self.persist_configs().await?;
        Ok(dto_from_config(
            &config,
            SshStatus::Disconnected,
            None,
            None,
        ))
    }

    pub async fn update_connection(
        &self,
        request: UpdateSshConnectionRequest,
    ) -> SshResult<SshConnectionDto> {
        self.ensure_loaded().await;
        let mut configs = self.configs.write().await;
        let cfg = configs
            .get_mut(&request.id)
            .ok_or_else(|| SshError::NotFound(request.id.clone()))?;
        let candidate = SshConnectionConfig {
            id: cfg.id.clone(),
            name: request.name,
            host: request.host,
            port: request.port,
            username: request.username,
            auth_type: request.auth_type,
            key_path: request.key_path.clone(),
            password: request.password,
            extra_options: cfg.extra_options.clone(),
            control_path: None,
            cached_status: SshStatus::Disconnected,
            cached_last_error: None,
            cached_latency_ms: None,
            cached_checked_at_ms: None,
        };
        validate_config(
            &candidate,
            candidate.key_path.as_deref(),
            candidate.password.as_deref(),
        )?;
        *cfg = candidate.clone();
        drop(configs);
        let _ = self.disconnect(&request.id).await;
        self.persist_configs().await?;
        Ok(dto_from_config(
            &candidate,
            SshStatus::Disconnected,
            None,
            None,
        ))
    }

    pub async fn remove_connection(&self, id: &str) -> SshResult<()> {
        self.ensure_loaded().await;
        let _ = self.disconnect(id).await;
        let mut configs = self.configs.write().await;
        configs.remove(id);
        drop(configs);
        self.persist_configs().await?;
        Ok(())
    }

    pub async fn connect(&self, id: &str) -> SshResult<SshServiceHandle> {
        self.ensure_loaded().await;
        let cfg = self.config_for(id).await?;
        let session = self.session_for(cfg.clone()).await;
        if let Some(client) = session.client().await {
            if !client.is_closed() {
                session.set_status(SshStatus::Connected, None).await;
                session.touch_keepalive();
                let _ = self
                    .update_cached_status(id, SshStatus::Connected, None, cfg.cached_latency_ms)
                    .await;
                return Ok(SshServiceHandle::new(id, id, 80, 24));
            }
        }

        session.set_status(SshStatus::Connecting, None).await;
        let started = std::time::Instant::now();
        let client = match timeout(Duration::from_secs(20), connect_client(&cfg)).await {
            Ok(Ok(client)) => client,
            Ok(Err(err)) => {
                let msg = err.to_string();
                session
                    .set_status(SshStatus::Error, Some(msg.clone()))
                    .await;
                let _ = self
                    .update_cached_status(
                        id,
                        SshStatus::Error,
                        Some(msg),
                        Some(started.elapsed().as_millis() as u64),
                    )
                    .await;
                return Err(err);
            }
            Err(_) => {
                let err = SshError::Timeout(Duration::from_secs(20));
                session
                    .set_status(SshStatus::Error, Some(err.to_string()))
                    .await;
                let _ = self
                    .update_cached_status(
                        id,
                        SshStatus::Error,
                        Some(err.to_string()),
                        Some(started.elapsed().as_millis() as u64),
                    )
                    .await;
                return Err(err);
            }
        };
        session.set_client(Some(client)).await;
        session.set_status(SshStatus::Connected, None).await;
        session.touch_keepalive();
        let _ = self
            .update_cached_status(
                id,
                SshStatus::Connected,
                None,
                Some(started.elapsed().as_millis() as u64),
            )
            .await;
        tracing::debug!(
            target: "deepagent_ssh",
            id,
            elapsed_ms = started.elapsed().as_millis(),
            "ssh connection established"
        );
        Ok(SshServiceHandle::new(id, id, 80, 24))
    }

    pub async fn exec(&self, handle: &SshServiceHandle, command: &str) -> SshResult<SshExecResult> {
        let session = self.connected_session(&handle.connection_id).await?;
        let client = session
            .client()
            .await
            .ok_or_else(|| SshError::ConnectionLost(handle.connection_id.clone()))?;
        let start = std::time::Instant::now();
        let output = client.execute(command).await.map_err(map_ssh_error)?;
        let duration_ms = start.elapsed().as_millis() as u64;
        let exit_code = i32::try_from(output.exit_status).unwrap_or(i32::MAX);
        if exit_code != 0 {
            return Err(SshError::CommandFailed {
                exit_code,
                stderr: output.stderr,
            });
        }
        session.touch_keepalive();
        Ok(SshExecResult {
            exit_code,
            stdout: output.stdout,
            stderr: output.stderr,
            duration_ms,
        })
    }

    pub async fn pty_spawn(
        &self,
        handle: &SshServiceHandle,
        cols: u16,
        rows: u16,
    ) -> SshResult<SshServiceHandle> {
        let session = self.connected_session(&handle.connection_id).await?;
        let client = session
            .client()
            .await
            .ok_or_else(|| SshError::ConnectionLost(handle.connection_id.clone()))?;

        let (stdin_tx, stdin_rx) = mpsc::channel::<Vec<u8>>(128);
        let (stdout_tx, stdout_rx) = mpsc::channel::<Vec<u8>>(256);
        let command = remote_shell_command();
        let session_for_task = session.clone();
        let connection_id = handle.connection_id.clone();
        let join = tokio::spawn(async move {
            let result = client
                .execute_io(&command, stdout_tx, None, Some(stdin_rx), true, Some(0))
                .await;
            if let Err(err) = result {
                session_for_task
                    .set_status(SshStatus::Error, Some(err.to_string()))
                    .await;
                tracing::warn!(target: "deepagent_ssh", connection_id, error = %err, "ssh pty task ended with error");
            }
        });
        session
            .replace_pty(Some(PtyState {
                stdin: stdin_tx,
                stdout: tokio::sync::Mutex::new(stdout_rx),
                join,
                cols,
                rows,
                output_cursor: AtomicU64::new(0),
                history: tokio::sync::Mutex::new(std::collections::VecDeque::new()),
                history_bytes: tokio::sync::Mutex::new(0),
            }))
            .await;
        Ok(SshServiceHandle::new(
            handle.connection_id.clone(),
            handle.token.clone(),
            cols,
            rows,
        ))
    }

    pub async fn pty_write(&self, handle: &SshServiceHandle, data: &[u8]) -> SshResult<()> {
        let session = self.connected_session(&handle.connection_id).await?;
        if session.pty_write(data.to_vec()).await {
            Ok(())
        } else {
            Err(SshError::Pty("no active PTY session".into()))
        }
    }

    pub async fn pty_read(&self, handle: &SshServiceHandle) -> SshResult<Vec<u8>> {
        let session = self.connected_session(&handle.connection_id).await?;
        session
            .pty_read_available()
            .await
            .ok_or_else(|| SshError::Pty("no active PTY session".into()))
    }

    pub async fn pty_read_with_cursor(
        &self,
        handle: &SshServiceHandle,
        after_cursor: u64,
    ) -> SshResult<deepagent_terminal::TerminalReadChunk> {
        let session = self.connected_session(&handle.connection_id).await?;
        session
            .pty_read_with_cursor(after_cursor)
            .await
            .ok_or_else(|| SshError::Pty("no active PTY session".into()))
    }

    pub async fn pty_resize(
        &self,
        handle: &SshServiceHandle,
        cols: u16,
        rows: u16,
    ) -> SshResult<()> {
        let session = self.connected_session(&handle.connection_id).await?;
        if session.pty_resize(cols, rows).await {
            Ok(())
        } else {
            Err(SshError::Pty("no active PTY session".into()))
        }
    }

    pub async fn disconnect(&self, id: &str) -> SshResult<()> {
        let sessions = self.sessions.read().await;
        if let Some(session) = sessions.get(id) {
            session.replace_pty(None).await;
            if let Some(client) = session.client().await {
                let _ = client.disconnect().await;
            }
            session.set_client(None).await;
            session.set_status(SshStatus::Disconnected, None).await;
        }
        drop(sessions);
        self.sessions.write().await.remove(id);
        Ok(())
    }

    pub async fn status(&self, id: &str) -> SshResult<SshStatusSnapshot> {
        self.ensure_loaded().await;
        let cached = {
            let configs = self.configs.read().await;
            configs.get(id).map(|cfg| SshStatusSnapshot {
                status: cfg.cached_status,
                last_error: cfg.cached_last_error.clone(),
                latency_ms: cfg.cached_latency_ms,
            })
        };
        let sessions = self.sessions.read().await;
        match sessions.get(id) {
            Some(s) => {
                if let Some(client) = s.client().await {
                    if client.is_closed() {
                        return cached.ok_or_else(|| SshError::NotFound(id.to_string()));
                    } else {
                        return Ok(SshStatusSnapshot {
                            status: s.status().await,
                            last_error: s.last_error().await,
                            latency_ms: cached.and_then(|snap| snap.latency_ms),
                        });
                    }
                }
                Ok(SshStatusSnapshot {
                    status: cached
                        .as_ref()
                        .map(|snap| snap.status)
                        .unwrap_or(SshStatus::Disconnected),
                    last_error: s
                        .last_error()
                        .await
                        .or_else(|| cached.as_ref().and_then(|snap| snap.last_error.clone())),
                    latency_ms: cached.and_then(|snap| snap.latency_ms),
                })
            }
            None => cached.ok_or_else(|| SshError::NotFound(id.to_string())),
        }
    }

    pub async fn test_connection(&self, config: &SshConnectionConfig) -> SshResult<SshTestResult> {
        self.ensure_loaded().await;
        let cfg = {
            let configs = self.configs.read().await;
            configs
                .get(&config.id)
                .cloned()
                .unwrap_or_else(|| config.clone())
        };
        let start = std::time::Instant::now();
        let result = match timeout(Duration::from_secs(20), async {
            let client = connect_client(&cfg).await?;
            let output = client.execute("echo ok").await.map_err(map_ssh_error)?;
            let _ = client.disconnect().await;
            Ok::<_, SshError>(output)
        })
        .await
        {
            Ok(Ok(output)) if output.exit_status == 0 => SshTestResult {
                ok: true,
                latency_ms: Some(start.elapsed().as_millis() as u64),
                banner: Some(output.stdout.trim().to_string()),
                error: None,
            },
            Ok(Ok(output)) => SshTestResult {
                ok: false,
                latency_ms: Some(start.elapsed().as_millis() as u64),
                banner: None,
                error: Some(output.stderr),
            },
            Ok(Err(err)) => SshTestResult {
                ok: false,
                latency_ms: Some(start.elapsed().as_millis() as u64),
                banner: None,
                error: Some(err.to_string()),
            },
            Err(_) => SshTestResult {
                ok: false,
                latency_ms: Some(start.elapsed().as_millis() as u64),
                banner: None,
                error: Some(SshError::Timeout(Duration::from_secs(20)).to_string()),
            },
        };

        let status = if result.ok {
            SshStatus::Connected
        } else {
            SshStatus::Error
        };
        let _ = self
            .update_cached_status(&cfg.id, status, result.error.clone(), result.latency_ms)
            .await;
        Ok(result)
    }

    pub async fn refresh_due_statuses(&self) -> SshResult<usize> {
        self.ensure_loaded().await;
        let configs = {
            let guard = self.configs.read().await;
            guard.values().cloned().collect::<Vec<_>>()
        };
        let mut refreshed = 0usize;
        for cfg in configs {
            if !self.should_refresh_status(&cfg).await {
                continue;
            }
            refreshed += 1;
            if let Err(err) = self.refresh_connection_status(&cfg).await {
                tracing::debug!(
                    target: "deepagent_ssh",
                    connection_id = cfg.id,
                    error = %err,
                    "ssh background health check failed"
                );
            }
            tokio::time::sleep(Duration::from_millis(STATUS_MONITOR_BETWEEN_CHECKS_MS)).await;
        }
        Ok(refreshed)
    }

    pub async fn remote_probe(
        &self,
        id: &str,
        force_refresh: bool,
    ) -> SshResult<RemoteProbeResult> {
        self.ensure_loaded().await;
        if !force_refresh {
            if let Some(cached) = self.read_probe_cache(id).await? {
                if now_epoch_ms().saturating_sub(cached.probed_at_ms) <= PROBE_CACHE_TTL_MS {
                    return Ok(cached);
                }
            }
        }

        let session = self.connected_session(id).await?;
        let client = session
            .client()
            .await
            .ok_or_else(|| SshError::ConnectionLost(id.to_string()))?;
        let output = client
            .execute(REMOTE_PROBE_SCRIPT)
            .await
            .map_err(map_ssh_error)?;
        let exit_code = i32::try_from(output.exit_status).unwrap_or(i32::MAX);
        if exit_code != 0 {
            return Err(SshError::CommandFailed {
                exit_code,
                stderr: output.stderr,
            });
        }
        let probe = parse_probe_output(&output.stdout)?;
        self.write_probe_cache(id, &probe).await?;
        session.touch_keepalive();
        Ok(probe)
    }

    pub async fn remote_push_file(
        &self,
        id: &str,
        request: RemotePushFileRequest,
    ) -> SshResult<RemotePushFileResult> {
        self.ensure_loaded().await;
        let session = self.connected_session(id).await?;
        let client = session
            .client()
            .await
            .ok_or_else(|| SshError::ConnectionLost(id.to_string()))?;
        let started = std::time::Instant::now();
        let local_path = PathBuf::from(&request.local_path);
        let meta = tokio::fs::metadata(&local_path)
            .await
            .map_err(SshError::Process)?;
        if !meta.is_file() {
            return Err(SshError::InvalidConfig(format!(
                "local_path is not a file: {}",
                local_path.display()
            )));
        }
        if request.create_parent {
            if let Some(parent) = parent_dir_string(&request.remote_path) {
                if !parent.is_empty() {
                    let cmd = format!("mkdir -p {}", shell_escape(&parent));
                    self.exec_remote_raw(&client, &cmd).await?;
                }
            }
        }
        if !request.overwrite {
            let cmd = format!(
                "if [ -e {path} ]; then echo exists; exit 1; fi",
                path = shell_escape(&request.remote_path)
            );
            self.exec_remote_raw(&client, &cmd).await?;
        }
        client
            .upload_file(
                &request.local_path,
                request.remote_path.clone(),
                Some(600),
                Some(64 * 1024),
                false,
            )
            .await
            .map_err(map_ssh_error)?;
        session.touch_keepalive();
        let local_sha256 = match request.verify_mode {
            RemoteVerifyMode::Sha256 => Some(sha256_file(&local_path).await?),
            _ => None,
        };
        let remote_size = remote_file_size(&client, &request.remote_path).await?;
        if remote_size != meta.len() {
            return Err(SshError::IntegrityMismatch(format!(
                "remote size mismatch for {}: local={} remote={}",
                request.remote_path,
                meta.len(),
                remote_size
            )));
        }
        let remote_sha256 = if matches!(request.verify_mode, RemoteVerifyMode::Sha256) {
            Some(remote_sha256(&client, &request.remote_path).await?)
        } else {
            None
        };
        if let (Some(local), Some(remote)) = (&local_sha256, &remote_sha256) {
            if local != remote {
                return Err(SshError::IntegrityMismatch(format!(
                    "sha256 mismatch for {}",
                    request.remote_path
                )));
            }
        }
        Ok(RemotePushFileResult {
            ok: true,
            remote_path: request.remote_path,
            bytes: meta.len(),
            local_sha256,
            remote_sha256,
            integrity_verified: !matches!(request.verify_mode, RemoteVerifyMode::None),
            duration_ms: started.elapsed().as_millis() as u64,
        })
    }

    pub async fn remote_push_bundle(
        &self,
        id: &str,
        request: RemoteBundleRequest,
    ) -> SshResult<RemoteBundleResult> {
        self.ensure_loaded().await;
        let session = self.connected_session(id).await?;
        let client = session
            .client()
            .await
            .ok_or_else(|| SshError::ConnectionLost(id.to_string()))?;
        let started = std::time::Instant::now();
        let local_root = PathBuf::from(&request.local_path);
        let root_meta = tokio::fs::metadata(&local_root)
            .await
            .map_err(SshError::Process)?;
        if !root_meta.is_dir() {
            return Err(SshError::InvalidConfig(format!(
                "local_path is not a directory: {}",
                local_root.display()
            )));
        }
        let root_name = local_root
            .file_name()
            .and_then(|s| s.to_str())
            .filter(|s| !s.is_empty())
            .unwrap_or("bundle")
            .to_string();
        let temp_base =
            std::env::temp_dir().join(format!("deepagent-bundle-{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir_all(&temp_base)
            .await
            .map_err(SshError::Process)?;
        let archive_path = temp_base.join("bundle.tar.gz");
        let manifest_path = temp_base.join("manifest.json");
        let manifest =
            build_bundle_archive(&local_root, &root_name, &archive_path, &manifest_path).await?;
        if request.create_parent {
            let cmd = format!("mkdir -p {}", shell_escape(&request.remote_path));
            self.exec_remote_raw(&client, &cmd).await?;
        }
        if !request.overwrite {
            let cmd = format!(
                "if [ -e {path} ]; then echo exists; exit 1; fi",
                path = shell_escape(&request.remote_path)
            );
            self.exec_remote_raw(&client, &cmd).await?;
        }
        let remote_archive_path = join_remote_path(
            &request.remote_path,
            &format!(".deepagent-bundle-{}.tar.gz", uuid::Uuid::new_v4()),
        );
        let remote_manifest_path = format!("{remote_archive_path}.manifest.json");
        let archive_push = self
            .remote_push_file(
                id,
                RemotePushFileRequest {
                    local_path: archive_path.to_string_lossy().to_string(),
                    remote_path: remote_archive_path.clone(),
                    create_parent: true,
                    overwrite: true,
                    verify_mode: request.verify_mode,
                },
            )
            .await?;
        let _manifest_push = self
            .remote_push_file(
                id,
                RemotePushFileRequest {
                    local_path: manifest_path.to_string_lossy().to_string(),
                    remote_path: remote_manifest_path.clone(),
                    create_parent: true,
                    overwrite: true,
                    verify_mode: request.verify_mode,
                },
            )
            .await?;
        let extract_cmd = format!(
            "mkdir -p {dest} && tar -xzf {archive} -C {dest}",
            dest = shell_escape(&request.remote_path),
            archive = shell_escape(&remote_archive_path)
        );
        self.exec_remote_raw(&client, &extract_cmd).await?;
        let verify_cmd = format!(
            "python3 - {root} {manifest} <<'PY'\nimport hashlib, json, os, sys\nroot = sys.argv[1]\nmanifest_path = sys.argv[2]\nwith open(manifest_path, 'r', encoding='utf-8') as fh:\n    data = json.load(fh)\nerrors = []\nfor entry in data.get('entries', []):\n    path = os.path.join(root, entry['path'])\n    if not os.path.exists(path):\n        errors.append('missing:' + entry['path'])\n        continue\n    if os.path.getsize(path) != entry['size']:\n        errors.append('size:' + entry['path'])\n        continue\n    h = hashlib.sha256()\n    with open(path, 'rb') as item:\n        while True:\n            chunk = item.read(1024 * 1024)\n            if not chunk:\n                break\n            h.update(chunk)\n    if h.hexdigest() != entry['sha256']:\n        errors.append('sha256:' + entry['path'])\nif errors:\n    print('\\n'.join(errors[:20]))\n    sys.exit(1)\nprint('ok')\nPY",
            root = shell_escape(&request.remote_path),
            manifest = shell_escape(&remote_manifest_path)
        );
        self.exec_remote_raw(&client, &verify_cmd).await?;
        if request.remove_archive_after_extract {
            let cleanup_cmd = format!(
                "rm -f {archive} {manifest}",
                archive = shell_escape(&remote_archive_path),
                manifest = shell_escape(&remote_manifest_path)
            );
            let _ = self.exec_remote_raw(&client, &cleanup_cmd).await;
        }
        let _ = tokio::fs::remove_file(&archive_path).await;
        let _ = tokio::fs::remove_file(&manifest_path).await;
        let _ = tokio::fs::remove_dir_all(&temp_base).await;
        Ok(RemoteBundleResult {
            ok: true,
            remote_path: request.remote_path,
            remote_archive_path,
            remote_manifest_path,
            files: manifest.entries.len() as u64,
            bytes: archive_push.bytes,
            local_archive_sha256: archive_push.local_sha256,
            remote_archive_sha256: archive_push.remote_sha256,
            integrity_verified: archive_push.integrity_verified,
            extract_verified: true,
            duration_ms: started.elapsed().as_millis() as u64,
        })
    }

    pub async fn remote_require(
        &self,
        id: &str,
        request: RemoteRequireRequest,
    ) -> SshResult<RemoteRequireResult> {
        let probe = self.remote_probe(id, false).await?;
        let mut missing_commands = Vec::new();
        for command in normalize_string_list(request.commands) {
            if !probe.commands.get(&command).copied().unwrap_or(false) {
                missing_commands.push(command);
            }
        }
        let mut missing_archive_tools = Vec::new();
        for archive in normalize_string_list(request.archives) {
            for need in archive_requirements(&archive) {
                if !probe.commands.get(*need).copied().unwrap_or(false) {
                    missing_archive_tools.push(need.to_string());
                }
            }
        }
        let mut missing_runtimes = Vec::new();
        for runtime in request.runtimes {
            let key = runtime.name.trim().to_lowercase();
            let current = probe.runtimes.get(&key);
            let missing = match (&runtime.version, current) {
                (_, None) => true,
                (Some(expected), Some(found)) => !found.contains(expected),
                (None, Some(_)) => false,
            };
            if missing {
                missing_runtimes.push(match runtime.version {
                    Some(version) if !version.trim().is_empty() => format!("{key}@{version}"),
                    _ => key,
                });
            }
        }
        missing_archive_tools.sort();
        missing_archive_tools.dedup();
        let package_manager = preferred_package_manager(&probe.package_managers);
        let packages = package_names_for_requirements(
            package_manager.as_deref(),
            &missing_commands,
            &missing_archive_tools,
            &missing_runtimes,
        );
        let install_commands =
            build_install_commands(package_manager.as_deref(), &probe, &packages, true);
        let can_install = !install_commands.is_empty();
        Ok(RemoteRequireResult {
            package_manager,
            package_managers: probe.package_managers.clone(),
            missing_commands,
            missing_runtimes,
            missing_archive_tools,
            install_commands,
            can_install,
            probe,
        })
    }

    pub async fn remote_install(
        &self,
        id: &str,
        request: RemoteInstallRequest,
    ) -> SshResult<RemoteInstallResult> {
        let probe = self.remote_probe(id, false).await?;
        let package_manager = request
            .package_manager
            .clone()
            .or_else(|| preferred_package_manager(&probe.package_managers));
        let mut missing_runtimes = Vec::new();
        for runtime in request.runtimes {
            missing_runtimes.push(match runtime.version {
                Some(version) if !version.trim().is_empty() => {
                    format!("{}@{}", runtime.name.trim().to_lowercase(), version)
                }
                _ => runtime.name.trim().to_lowercase(),
            });
        }
        let mut requested_packages = normalize_string_list(request.packages);
        requested_packages.extend(package_names_for_requirements(
            package_manager.as_deref(),
            &normalize_string_list(request.commands),
            &[],
            &missing_runtimes,
        ));
        requested_packages.sort();
        requested_packages.dedup();
        let install_commands = build_install_commands(
            package_manager.as_deref(),
            &probe,
            &requested_packages,
            request.update_index,
        );
        if install_commands.is_empty() {
            return Ok(RemoteInstallResult {
                ok: false,
                package_manager,
                commands_run: Vec::new(),
                stdout: String::new(),
                stderr: "unable to determine an installation strategy for this remote host".into(),
                installed_packages: requested_packages,
                probe: Some(probe),
            });
        }
        let session = self.connected_session(id).await?;
        let client = session
            .client()
            .await
            .ok_or_else(|| SshError::ConnectionLost(id.to_string()))?;
        let mut stdout = String::new();
        let mut stderr = String::new();
        for command in &install_commands {
            let result = self.exec_remote_raw(&client, command).await?;
            stdout.push_str(&result.stdout);
            stderr.push_str(&result.stderr);
        }
        let refreshed = self.remote_probe(id, true).await.ok();
        Ok(RemoteInstallResult {
            ok: true,
            package_manager,
            commands_run: install_commands,
            stdout,
            stderr,
            installed_packages: requested_packages,
            probe: refreshed,
        })
    }

    async fn config_for(&self, id: &str) -> SshResult<SshConnectionConfig> {
        self.ensure_loaded().await;
        let configs = self.configs.read().await;
        configs
            .get(id)
            .cloned()
            .ok_or_else(|| SshError::NotFound(id.to_string()))
    }

    async fn session_for(&self, cfg: SshConnectionConfig) -> Arc<SshSession> {
        let mut sessions = self.sessions.write().await;
        sessions
            .entry(cfg.id.clone())
            .or_insert_with(|| SshSession::new(cfg))
            .clone()
    }

    async fn connected_session(&self, id: &str) -> SshResult<Arc<SshSession>> {
        let _ = self.connect(id).await?;
        let sessions = self.sessions.read().await;
        sessions
            .get(id)
            .cloned()
            .ok_or_else(|| SshError::NotFound(id.to_string()))
    }

    async fn ensure_loaded(&self) {
        if !self.loaded.load(Ordering::Relaxed) {
            let _ = self.load().await;
            self.loaded.store(true, Ordering::Relaxed);
        }
    }

    async fn should_refresh_status(&self, cfg: &SshConnectionConfig) -> bool {
        let session = {
            let sessions = self.sessions.read().await;
            sessions.get(&cfg.id).cloned()
        };
        let status = if let Some(session) = session {
            if let Some(client) = session.client().await {
                if client.is_closed() {
                    cfg.cached_status
                } else {
                    session.status().await
                }
            } else {
                cfg.cached_status
            }
        } else {
            cfg.cached_status
        };
        let interval_ms = status_refresh_interval_ms(status);
        match cfg.cached_checked_at_ms {
            Some(last_checked_at_ms) => {
                now_epoch_ms().saturating_sub(last_checked_at_ms) >= interval_ms
            }
            None => true,
        }
    }

    async fn refresh_connection_status(&self, cfg: &SshConnectionConfig) -> SshResult<()> {
        let session = {
            let sessions = self.sessions.read().await;
            sessions.get(&cfg.id).cloned()
        };
        if let Some(session) = session {
            if let Some(client) = session.client().await {
                if client.is_closed() {
                    session.set_client(None).await;
                    session.set_status(SshStatus::Disconnected, None).await;
                    self.sessions.write().await.remove(&cfg.id);
                } else {
                    let started = std::time::Instant::now();
                    let outcome = timeout(
                        Duration::from_secs(STATUS_REFRESH_TIMEOUT_SECS),
                        execute_health_check(&client),
                    )
                    .await;
                    return match outcome {
                        Ok(Ok(())) => {
                            session.set_status(SshStatus::Connected, None).await;
                            session.touch_keepalive();
                            self.update_cached_status(
                                &cfg.id,
                                SshStatus::Connected,
                                None,
                                Some(started.elapsed().as_millis() as u64),
                            )
                            .await
                        }
                        Ok(Err(err)) => {
                            let msg = err.to_string();
                            let _ = client.disconnect().await;
                            session.replace_pty(None).await;
                            session.set_client(None).await;
                            session
                                .set_status(SshStatus::Error, Some(msg.clone()))
                                .await;
                            self.sessions.write().await.remove(&cfg.id);
                            self.update_cached_status(
                                &cfg.id,
                                SshStatus::Error,
                                Some(msg),
                                Some(started.elapsed().as_millis() as u64),
                            )
                            .await
                        }
                        Err(_) => {
                            let err =
                                SshError::Timeout(Duration::from_secs(STATUS_REFRESH_TIMEOUT_SECS));
                            let _ = client.disconnect().await;
                            session.replace_pty(None).await;
                            session.set_client(None).await;
                            session
                                .set_status(SshStatus::Error, Some(err.to_string()))
                                .await;
                            self.sessions.write().await.remove(&cfg.id);
                            self.update_cached_status(
                                &cfg.id,
                                SshStatus::Error,
                                Some(err.to_string()),
                                Some(started.elapsed().as_millis() as u64),
                            )
                            .await
                        }
                    };
                }
            }
        }

        let started = std::time::Instant::now();
        let outcome = timeout(Duration::from_secs(STATUS_REFRESH_TIMEOUT_SECS), async {
            let client = connect_client(cfg).await?;
            execute_health_check(&client).await?;
            let _ = client.disconnect().await;
            Ok::<(), SshError>(())
        })
        .await;
        match outcome {
            Ok(Ok(())) => {
                self.update_cached_status(
                    &cfg.id,
                    SshStatus::Connected,
                    None,
                    Some(started.elapsed().as_millis() as u64),
                )
                .await
            }
            Ok(Err(err)) => {
                self.update_cached_status(
                    &cfg.id,
                    SshStatus::Error,
                    Some(err.to_string()),
                    Some(started.elapsed().as_millis() as u64),
                )
                .await
            }
            Err(_) => {
                let err = SshError::Timeout(Duration::from_secs(STATUS_REFRESH_TIMEOUT_SECS));
                self.update_cached_status(
                    &cfg.id,
                    SshStatus::Error,
                    Some(err.to_string()),
                    Some(started.elapsed().as_millis() as u64),
                )
                .await
            }
        }
    }

    async fn update_cached_status(
        &self,
        id: &str,
        status: SshStatus,
        last_error: Option<String>,
        latency_ms: Option<u64>,
    ) -> SshResult<()> {
        let mut configs = self.configs.write().await;
        let Some(cfg) = configs.get_mut(id) else {
            return Ok(());
        };
        cfg.cached_status = status;
        cfg.cached_last_error = last_error;
        cfg.cached_latency_ms = latency_ms;
        cfg.cached_checked_at_ms = Some(now_epoch_ms());
        drop(configs);
        self.persist_configs().await
    }

    async fn read_probe_cache(&self, id: &str) -> SshResult<Option<RemoteProbeResult>> {
        let path = self.probe_cache_path(id);
        let data = match tokio::fs::read_to_string(&path).await {
            Ok(data) => data,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(err) => return Err(SshError::Persistence(err.to_string())),
        };
        let value =
            serde_json::from_str(&data).map_err(|err| SshError::Persistence(err.to_string()))?;
        Ok(Some(value))
    }

    async fn write_probe_cache(&self, id: &str, probe: &RemoteProbeResult) -> SshResult<()> {
        let path = self.probe_cache_path(id);
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|err| SshError::Persistence(err.to_string()))?;
        }
        let data = serde_json::to_string_pretty(probe)
            .map_err(|err| SshError::Persistence(err.to_string()))?;
        tokio::fs::write(path, data)
            .await
            .map_err(|err| SshError::Persistence(err.to_string()))
    }

    fn probe_cache_path(&self, id: &str) -> PathBuf {
        self.data_root.join("probes").join(format!("{id}.json"))
    }

    async fn exec_remote_raw(&self, client: &Client, command: &str) -> SshResult<SshExecResult> {
        let start = std::time::Instant::now();
        let output = client.execute(command).await.map_err(map_ssh_error)?;
        let exit_code = i32::try_from(output.exit_status).unwrap_or(i32::MAX);
        if exit_code != 0 {
            return Err(SshError::CommandFailed {
                exit_code,
                stderr: output.stderr,
            });
        }
        Ok(SshExecResult {
            exit_code,
            stdout: output.stdout,
            stderr: output.stderr,
            duration_ms: start.elapsed().as_millis() as u64,
        })
    }
}

async fn execute_health_check(client: &Client) -> SshResult<()> {
    let output = client
        .execute(SSH_HEALTHCHECK_COMMAND)
        .await
        .map_err(map_ssh_error)?;
    let exit_code = i32::try_from(output.exit_status).unwrap_or(i32::MAX);
    if exit_code != 0 {
        return Err(SshError::CommandFailed {
            exit_code,
            stderr: output.stderr,
        });
    }
    Ok(())
}

async fn connect_client(config: &SshConnectionConfig) -> SshResult<Client> {
    validate_config(
        config,
        config.key_path.as_deref(),
        config.password.as_deref(),
    )?;
    let auth = auth_method(config)?;
    let addr = (config.host.as_str(), config.port);
    Client::connect(addr, &config.username, auth, ServerCheckMethod::NoCheck)
        .await
        .map_err(map_ssh_error)
}

fn auth_method(config: &SshConnectionConfig) -> SshResult<AuthMethod> {
    match config.auth_type {
        SshAuthType::Password => {
            let password = config.password.as_deref().ok_or_else(|| {
                SshError::InvalidConfig("password authentication requires a password".into())
            })?;
            Ok(AuthMethod::with_password(password))
        }
        SshAuthType::KeyFile => {
            let key_path = config.key_path.as_deref().ok_or_else(|| {
                SshError::InvalidConfig("key_file authentication requires key_path".into())
            })?;
            Ok(AuthMethod::with_key_file(
                key_path,
                config.password.as_deref(),
            ))
        }
        SshAuthType::Agent => auth_agent_method(),
    }
}

#[cfg(not(target_os = "windows"))]
fn auth_agent_method() -> SshResult<AuthMethod> {
    Ok(AuthMethod::with_agent())
}

#[cfg(target_os = "windows")]
fn auth_agent_method() -> SshResult<AuthMethod> {
    Err(SshError::InvalidConfig(
        "SSH agent authentication is not supported by async-ssh2-tokio on Windows; use password or key file auth"
            .into(),
    ))
}

fn validate_config(
    config: &SshConnectionConfig,
    key_path: Option<&str>,
    password: Option<&str>,
) -> SshResult<()> {
    if config.host.trim().is_empty() {
        return Err(SshError::InvalidConfig("host is required".into()));
    }
    if config.username.trim().is_empty() {
        return Err(SshError::InvalidConfig("username is required".into()));
    }
    if config.port == 0 {
        return Err(SshError::InvalidConfig(
            "port must be greater than 0".into(),
        ));
    }
    match config.auth_type {
        SshAuthType::Password if password.unwrap_or_default().is_empty() => {
            Err(SshError::InvalidConfig("password is required".into()))
        }
        SshAuthType::KeyFile if key_path.unwrap_or_default().is_empty() => {
            Err(SshError::InvalidConfig("key path is required".into()))
        }
        _ => Ok(()),
    }
}

fn dto_from_config(
    cfg: &SshConnectionConfig,
    status: SshStatus,
    last_error: Option<String>,
    latency_ms: Option<u64>,
) -> SshConnectionDto {
    SshConnectionDto {
        id: cfg.id.clone(),
        name: cfg.name.clone(),
        host: cfg.host.clone(),
        port: cfg.port,
        username: cfg.username.clone(),
        auth_type: cfg.auth_type,
        key_path: cfg.key_path.clone(),
        status,
        last_error,
        latency_ms,
    }
}

fn map_ssh_error(err: async_ssh2_tokio::Error) -> SshError {
    match err {
        async_ssh2_tokio::Error::PasswordWrong
        | async_ssh2_tokio::Error::KeyAuthFailed
        | async_ssh2_tokio::Error::KeyboardInteractiveAuthFailed
        | async_ssh2_tokio::Error::AgentAuthenticationFailed => {
            SshError::AuthFailed("ssh".into(), err.to_string())
        }
        async_ssh2_tokio::Error::AddressInvalid(inner) => {
            SshError::InvalidConfig(inner.to_string())
        }
        async_ssh2_tokio::Error::IoError(inner) => SshError::Process(inner),
        other => SshError::Internal(other.to_string()),
    }
}

const PROBE_CACHE_TTL_MS: u64 = 10 * 60 * 1000;
const STATUS_REFRESH_TIMEOUT_SECS: u64 = 12;
const STATUS_MONITOR_BETWEEN_CHECKS_MS: u64 = 1_500;
const SSH_HEALTHCHECK_COMMAND: &str = "echo deepagent-ssh-healthcheck";
const REMOTE_PROBE_SCRIPT: &str = r#"set +e
printf '__os__=%s\n' "$(uname -s 2>/dev/null || printf '%s' "${OS:-}")"
printf '__arch__=%s\n' "$(uname -m 2>/dev/null || printf '')"
printf '__user__=%s\n' "$(whoami 2>/dev/null || printf '%s' "${USERNAME:-}")"
printf '__shell__=%s\n' "${SHELL:-}"
printf '__cwd__=%s\n' "$(pwd 2>/dev/null || printf '')"
if [ -r /etc/os-release ]; then . /etc/os-release 2>/dev/null; printf '__distro__=%s\n' "${ID:-${NAME:-}}"; printf '__distro_version__=%s\n' "${VERSION_ID:-}"; fi
printf '__path__=%s\n' "${PATH:-}"
for name in apt yum dnf apk zypper brew winget choco scoop sudo tar gzip xz unzip curl wget git python3 pip node npm pnpm docker systemctl; do
  if command -v "$name" >/dev/null 2>&1; then printf 'cmd:%s=1\n' "$name"; else printf 'cmd:%s=0\n' "$name"; fi
done
if command -v docker >/dev/null 2>&1 && docker compose version >/dev/null 2>&1; then printf 'cmd:docker_compose=1\n'; else printf 'cmd:docker_compose=0\n'; fi
for name in apt yum dnf apk zypper brew winget choco scoop; do
  if command -v "$name" >/dev/null 2>&1; then printf 'pkg:%s=1\n' "$name"; fi
done
if command -v python3 >/dev/null 2>&1; then printf 'rt:python3=%s\n' "$(python3 --version 2>&1 | head -n 1)"; fi
if command -v pip >/dev/null 2>&1; then printf 'rt:pip=%s\n' "$(pip --version 2>&1 | head -n 1)"; fi
if command -v node >/dev/null 2>&1; then printf 'rt:node=%s\n' "$(node --version 2>&1 | head -n 1)"; fi
if command -v npm >/dev/null 2>&1; then printf 'rt:npm=%s\n' "$(npm --version 2>&1 | head -n 1)"; fi
if command -v pnpm >/dev/null 2>&1; then printf 'rt:pnpm=%s\n' "$(pnpm --version 2>&1 | head -n 1)"; fi
if command -v docker >/dev/null 2>&1; then printf 'rt:docker=%s\n' "$(docker --version 2>&1 | head -n 1)"; fi
if command -v docker >/dev/null 2>&1 && docker compose version >/dev/null 2>&1; then printf 'rt:docker_compose=%s\n' "$(docker compose version 2>&1 | head -n 1)"; fi
"#;

fn parse_probe_output(stdout: &str) -> SshResult<RemoteProbeResult> {
    let mut result = RemoteProbeResult {
        probed_at_ms: now_epoch_ms(),
        ..RemoteProbeResult::default()
    };
    for line in stdout.lines() {
        if let Some(value) = line.strip_prefix("__os__=") {
            result.os = sanitize_optional(value);
        } else if let Some(value) = line.strip_prefix("__arch__=") {
            result.arch = sanitize_optional(value);
        } else if let Some(value) = line.strip_prefix("__user__=") {
            result.user = sanitize_optional(value);
        } else if let Some(value) = line.strip_prefix("__shell__=") {
            result.shell = sanitize_optional(value);
        } else if let Some(value) = line.strip_prefix("__cwd__=") {
            result.cwd = sanitize_optional(value);
        } else if let Some(value) = line.strip_prefix("__distro__=") {
            result.distro = sanitize_optional(value);
        } else if let Some(value) = line.strip_prefix("__distro_version__=") {
            result.distro_version = sanitize_optional(value);
        } else if let Some(value) = line.strip_prefix("__path__=") {
            result.path = sanitize_optional(value);
        } else if let Some(rest) = line.strip_prefix("cmd:") {
            if let Some((name, value)) = rest.split_once('=') {
                result
                    .commands
                    .insert(name.to_string(), value.trim() == "1");
            }
        } else if let Some(rest) = line.strip_prefix("pkg:") {
            if let Some((name, value)) = rest.split_once('=') {
                if value.trim() == "1" {
                    result.package_managers.push(name.to_string());
                }
            }
        } else if let Some(rest) = line.strip_prefix("rt:") {
            if let Some((name, value)) = rest.split_once('=') {
                if let Some(v) = sanitize_optional(value) {
                    result.runtimes.insert(name.to_string(), v);
                }
            }
        }
    }
    if result.os.is_none() && result.user.is_none() && result.cwd.is_none() {
        return Err(SshError::Internal(
            "remote probe returned no usable data".into(),
        ));
    }
    result.package_managers.sort();
    result.package_managers.dedup();
    Ok(result)
}

fn sanitize_optional(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn now_epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn status_refresh_interval_ms(status: SshStatus) -> u64 {
    match status {
        SshStatus::Connected => 30_000,
        SshStatus::Connecting => 15_000,
        SshStatus::Disconnected | SshStatus::Error => 90_000,
    }
}

async fn sha256_file(path: &Path) -> SshResult<String> {
    let mut file = tokio::fs::File::open(path)
        .await
        .map_err(SshError::Process)?;
    let mut hasher = sha2::Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buf).await.map_err(SshError::Process)?;
        if read == 0 {
            break;
        }
        sha2::Digest::update(&mut hasher, &buf[..read]);
    }
    Ok(format!("{:x}", sha2::Digest::finalize(hasher)))
}

async fn remote_file_size(client: &Client, remote_path: &str) -> SshResult<u64> {
    let cmd = format!(
        "wc -c < {path} 2>/dev/null || stat -c %s {path} 2>/dev/null || stat -f %z {path} 2>/dev/null",
        path = shell_escape(remote_path)
    );
    let output = client.execute(&cmd).await.map_err(map_ssh_error)?;
    let exit_code = i32::try_from(output.exit_status).unwrap_or(i32::MAX);
    if exit_code != 0 {
        return Err(SshError::CommandFailed {
            exit_code,
            stderr: output.stderr,
        });
    }
    output
        .stdout
        .split_whitespace()
        .find_map(|part| part.trim().parse::<u64>().ok())
        .ok_or_else(|| {
            SshError::Internal(format!(
                "failed to parse remote file size for {remote_path}"
            ))
        })
}

async fn remote_sha256(client: &Client, remote_path: &str) -> SshResult<String> {
    let quoted = shell_escape(remote_path);
    let cmd = format!(
        "if command -v sha256sum >/dev/null 2>&1; then sha256sum {path} | awk '{{print $1}}'; \
elif command -v shasum >/dev/null 2>&1; then shasum -a 256 {path} | awk '{{print $1}}'; \
elif command -v openssl >/dev/null 2>&1; then openssl dgst -sha256 {path} | awk '{{print $NF}}'; \
elif command -v python3 >/dev/null 2>&1; then python3 - {path} <<'PY'\nimport hashlib, sys\nh = hashlib.sha256()\nwith open(sys.argv[1], 'rb') as fh:\n    while True:\n        chunk = fh.read(1024 * 1024)\n        if not chunk:\n            break\n        h.update(chunk)\nprint(h.hexdigest())\nPY\nelse echo missing-sha256-tool >&2; exit 127; fi",
        path = quoted
    );
    let output = client.execute(&cmd).await.map_err(map_ssh_error)?;
    let exit_code = i32::try_from(output.exit_status).unwrap_or(i32::MAX);
    if exit_code != 0 {
        return Err(SshError::CommandFailed {
            exit_code,
            stderr: output.stderr,
        });
    }
    let hash = output
        .stdout
        .lines()
        .next()
        .unwrap_or_default()
        .trim()
        .to_string();
    if hash.is_empty() {
        return Err(SshError::IntegrityMismatch(format!(
            "remote sha256 command returned no hash for {remote_path}"
        )));
    }
    Ok(hash)
}

async fn build_bundle_archive(
    local_root: &Path,
    root_name: &str,
    archive_path: &Path,
    manifest_path: &Path,
) -> SshResult<RemoteBundleManifest> {
    let local_root = local_root.to_path_buf();
    let root_name = root_name.to_string();
    let archive_path = archive_path.to_path_buf();
    let manifest_path = manifest_path.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let mut manifest = RemoteBundleManifest {
            version: 1,
            root_name: root_name.clone(),
            entries: Vec::new(),
        };
        let archive_file = std::fs::File::create(&archive_path).map_err(SshError::Process)?;
        let encoder = flate2::write::GzEncoder::new(archive_file, flate2::Compression::default());
        let mut builder = tar::Builder::new(encoder);
        let mut stack = vec![local_root.clone()];
        while let Some(dir) = stack.pop() {
            let mut entries: Vec<std::fs::DirEntry> = std::fs::read_dir(&dir)
                .map_err(SshError::Process)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(SshError::Process)?;
            entries.sort_by_key(|entry| entry.path());
            for entry in entries {
                let path = entry.path();
                let rel = path
                    .strip_prefix(&local_root)
                    .map_err(|err| SshError::Internal(err.to_string()))?;
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                if !path.is_file() {
                    continue;
                }
                let archive_rel = Path::new(&root_name).join(rel);
                builder
                    .append_path_with_name(&path, &archive_rel)
                    .map_err(SshError::Process)?;
                let size = std::fs::metadata(&path).map_err(SshError::Process)?.len();
                let sha256 = sha256_file_sync(&path)?;
                manifest.entries.push(RemoteManifestEntry {
                    path: archive_rel.to_string_lossy().replace('\\', "/"),
                    size,
                    sha256,
                });
            }
        }
        builder.finish().map_err(SshError::Process)?;
        let mut file = std::fs::File::create(&manifest_path).map_err(SshError::Process)?;
        file.write_all(
            serde_json::to_string_pretty(&manifest)
                .map_err(|err| SshError::Internal(err.to_string()))?
                .as_bytes(),
        )
        .map_err(SshError::Process)?;
        Ok::<RemoteBundleManifest, SshError>(manifest)
    })
    .await
    .map_err(|err| SshError::Internal(err.to_string()))?
}

fn sha256_file_sync(path: &Path) -> SshResult<String> {
    let mut file = std::fs::File::open(path).map_err(SshError::Process)?;
    let mut hasher = sha2::Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buf).map_err(SshError::Process)?;
        if read == 0 {
            break;
        }
        sha2::Digest::update(&mut hasher, &buf[..read]);
    }
    Ok(format!("{:x}", sha2::Digest::finalize(hasher)))
}

fn shell_escape(input: &str) -> String {
    format!("'{}'", input.replace('\'', "'\"'\"'"))
}

fn parent_dir_string(path: &str) -> Option<String> {
    let normalized = path.replace('\\', "/");
    let idx = normalized.rfind('/')?;
    Some(normalized[..idx].to_string())
}

fn join_remote_path(base: &str, name: &str) -> String {
    let base = base.trim_end_matches('/');
    if base.is_empty() {
        name.to_string()
    } else {
        format!("{base}/{name}")
    }
}

fn normalize_string_list(values: Vec<String>) -> Vec<String> {
    let mut out = Vec::new();
    for value in values {
        let normalized = value.trim().to_lowercase();
        if !normalized.is_empty() {
            out.push(normalized);
        }
    }
    out.sort();
    out.dedup();
    out
}

fn archive_requirements(kind: &str) -> &'static [&'static str] {
    match kind.trim().to_lowercase().as_str() {
        "zip" => &["unzip"],
        "tar" => &["tar"],
        "tar.gz" | "tgz" => &["tar", "gzip"],
        "tar.xz" | "txz" => &["tar", "xz"],
        _ => &[],
    }
}

fn preferred_package_manager(package_managers: &[String]) -> Option<String> {
    for candidate in [
        "apt", "dnf", "yum", "apk", "zypper", "brew", "winget", "choco", "scoop",
    ] {
        if package_managers.iter().any(|name| name == candidate) {
            return Some(candidate.to_string());
        }
    }
    package_managers.first().cloned()
}

fn package_names_for_requirements(
    package_manager: Option<&str>,
    missing_commands: &[String],
    missing_archive_tools: &[String],
    missing_runtimes: &[String],
) -> Vec<String> {
    let mut packages = Vec::new();
    for item in missing_commands
        .iter()
        .chain(missing_archive_tools.iter())
        .chain(missing_runtimes.iter())
    {
        if let Some(pkg) = package_name_for_requirement(package_manager, item) {
            packages.push(pkg);
        }
    }
    packages.sort();
    packages.dedup();
    packages
}

fn package_name_for_requirement(
    package_manager: Option<&str>,
    requirement: &str,
) -> Option<String> {
    let name = requirement.split('@').next().unwrap_or(requirement).trim();
    if name.is_empty() {
        return None;
    }
    let package = match (package_manager.unwrap_or_default(), name) {
        ("apk", "xz") => "xz",
        ("brew", "xz") => "xz",
        (_, "tar") => "tar",
        (_, "gzip") => "gzip",
        (_, "xz") => "xz-utils",
        (_, "unzip") => "unzip",
        (_, "curl") => "curl",
        (_, "wget") => "wget",
        (_, "git") => "git",
        (_, "python3") => "python3",
        ("apt", "pip") => "python3-pip",
        ("dnf", "pip") | ("yum", "pip") | ("zypper", "pip") => "python3-pip",
        ("apk", "pip") => "py3-pip",
        (_, "pip") => "pip",
        ("apt", "node") | ("apt", "npm") => "nodejs npm",
        ("dnf", "node") | ("dnf", "npm") | ("yum", "node") | ("yum", "npm") => "nodejs npm",
        ("apk", "node") | ("apk", "npm") => "nodejs npm",
        ("brew", "node") | ("brew", "npm") => "node",
        (_, "node") | (_, "npm") => "nodejs",
        (_, "pnpm") => "pnpm",
        (_, "docker") => "docker",
        (_, "docker_compose") => "docker-compose",
        (_, "systemctl") => "systemd",
        _ => name,
    };
    Some(package.to_string())
}

fn build_install_commands(
    package_manager: Option<&str>,
    probe: &RemoteProbeResult,
    packages: &[String],
    update_index: bool,
) -> Vec<String> {
    if packages.is_empty() {
        return Vec::new();
    }
    let Some(pm) = package_manager else {
        return Vec::new();
    };
    let needs_sudo = matches!(pm, "apt" | "dnf" | "yum" | "apk" | "zypper")
        && probe.user.as_deref() != Some("root");
    let prefix = if needs_sudo {
        if probe.commands.get("sudo").copied().unwrap_or(false) {
            "sudo "
        } else {
            return Vec::new();
        }
    } else {
        ""
    };
    let package_list = packages.join(" ");
    match pm {
        "apt" => {
            let mut cmds = Vec::new();
            if update_index {
                cmds.push(format!("{prefix}apt-get update"));
            }
            cmds.push(format!("{prefix}apt-get install -y {package_list}"));
            cmds
        }
        "dnf" => vec![format!("{prefix}dnf install -y {package_list}")],
        "yum" => vec![format!("{prefix}yum install -y {package_list}")],
        "apk" => vec![format!("{prefix}apk add {package_list}")],
        "zypper" => vec![format!("{prefix}zypper --non-interactive install {package_list}")],
        "brew" => vec![format!("brew install {package_list}")],
        "winget" => packages
            .iter()
            .map(|pkg| format!("winget install --silent --accept-package-agreements --accept-source-agreements {pkg}"))
            .collect(),
        "choco" => vec![format!("choco install -y {package_list}")],
        "scoop" => vec![format!("scoop install {package_list}")],
        _ => Vec::new(),
    }
}

fn remote_shell_command() -> String {
    if cfg!(target_os = "windows") {
        "cmd.exe".to_string()
    } else {
        "sh -l".to_string()
    }
}
