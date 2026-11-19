use super::authentication_challenge::{
    next_challenge, stale_error as stale_authentication_challenge, AuthenticationChallengeRegistry,
};
use super::config::{
    CreateSshConnectionRequest, SshAuthType, SshConnectionConfig, SshConnectionDto, SshStatus,
    UpdateSshConnectionRequest,
};
use super::connection_lease::SessionLease;
use super::connection_operation::{ConnectionOperations, ConnectionPermit};
use super::error::{SshError, SshResult};
use super::host_trust::{HostTrust, SshHostTrustInspection};
use super::remote::{
    RemoteBundleManifest, RemoteBundleRequest, RemoteBundleResult, RemoteInstallRequest,
    RemoteInstallResult, RemoteManifestEntry, RemoteProbeResult, RemotePushFileRequest,
    RemotePushFileResult, RemoteRequireRequest, RemoteRequireResult, RemoteVerifyMode,
};
use super::session::{
    SshDirEntry, SshDirListing, SshExecResult, SshFileBinary, SshFileContent, SshSession,
    SshStatusSnapshot, SshTestResult,
};
use super::timing::ChallengeDeadline;
use super::transport::{AuthenticationStep, SshClient as Client, SshClientConnection};
use super::{SshConfigStore, SshConnectionAttempt, SshServiceHandle};
use async_trait::async_trait;
use base64::Engine;
use deepagent_core::clock::Clock;
use russh::keys::PublicKey;
use russh_sftp::client::SftpSession;
use russh_sftp::protocol::OpenFlags;
use sha2::Digest;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::RwLock;
use tokio::task::JoinSet;
use tokio::time::{timeout, Duration};

const SERVICE_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);
const HEALTH_RECONNECT_INITIAL_SECS: u64 = 5;
const HEALTH_RECONNECT_MAX_SECS: u64 = 300;

#[derive(Default)]
struct ReconnectBackoff {
    failures: u32,
    retry_at: Option<tokio::time::Instant>,
}

impl ReconnectBackoff {
    fn record_failure(&mut self, now: tokio::time::Instant) -> Duration {
        self.failures = self.failures.saturating_add(1);
        let multiplier = 1_u64 << self.failures.saturating_sub(1).min(20);
        let delay = Duration::from_secs(
            HEALTH_RECONNECT_INITIAL_SECS
                .saturating_mul(multiplier)
                .min(HEALTH_RECONNECT_MAX_SECS),
        );
        self.retry_at = Some(now + delay);
        delay
    }

    fn is_due(&self, now: tokio::time::Instant) -> bool {
        match self.retry_at {
            Some(retry_at) => now >= retry_at,
            None => true,
        }
    }
}

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
    // Config identity changes cannot race publication of a verified connection.
    pub(super) lifecycle: RwLock<()>,
    configs: RwLock<HashMap<String, SshConnectionConfig>>,
    pub(super) sessions: RwLock<HashMap<String, Arc<SshSession>>>,
    pub(super) connection_operations: ConnectionOperations,
    pub(super) connection_idle_timeout: Duration,
    reconnect_backoff: Mutex<HashMap<String, ReconnectBackoff>>,
    config_store: Arc<dyn SshConfigStore>,
    pub(super) host_key_store: Option<Arc<crate::SshHostKeyStore>>,
    pub(super) host_challenges: crate::connection_challenge::HostKeyChallenges,
    pub(super) authentication_challenges: Arc<AuthenticationChallengeRegistry>,
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
            lifecycle: RwLock::new(()),
            configs: RwLock::new(HashMap::new()),
            sessions: RwLock::new(HashMap::new()),
            connection_operations: ConnectionOperations::default(),
            connection_idle_timeout: Duration::from_secs(60),
            reconnect_backoff: Mutex::new(HashMap::new()),
            config_store,
            host_key_store: None,
            host_challenges: Default::default(),
            authentication_challenges: Arc::default(),
            data_root: app_data.join("deepagent-ssh"),
            loaded: AtomicBool::new(false),
        }
    }

    fn reconnect_retry_due(&self, id: &str) -> Option<bool> {
        self.reconnect_backoff
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(id)
            .map(|backoff| backoff.is_due(tokio::time::Instant::now()))
    }

    fn record_reconnect_failure(&self, id: &str) -> Duration {
        self.reconnect_backoff
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .entry(id.to_owned())
            .or_default()
            .record_failure(tokio::time::Instant::now())
    }

    fn clear_reconnect_backoff(&self, id: &str) {
        self.reconnect_backoff
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(id);
    }

    fn clear_all_reconnect_backoff(&self) {
        self.reconnect_backoff
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clear();
    }

    pub async fn load(&self) -> SshResult<()> {
        self.connection_operations.ensure_running()?;
        let _lifecycle = self.lifecycle.write().await;
        self.connection_operations.ensure_running()?;
        self.load_locked().await
    }

    pub async fn shutdown(&self) -> SshResult<usize> {
        self.connection_operations.stop_admission();
        self.clear_all_reconnect_backoff();
        timeout(SERVICE_SHUTDOWN_TIMEOUT, async {
            self.connection_operations.wait_for_quiescence().await;
            let _lifecycle = self.lifecycle.write().await;
            let sessions = self
                .sessions
                .read()
                .await
                .values()
                .cloned()
                .collect::<Vec<_>>();
            let mut ids = self
                .configs
                .read()
                .await
                .keys()
                .cloned()
                .collect::<Vec<_>>();
            ids.extend(sessions.iter().map(|session| session.key().to_owned()));
            ids.sort();
            ids.dedup();
            for id in ids {
                self.host_challenges.invalidate(&id);
                self.authentication_challenges.invalidate(&id).await;
            }

            let mut closes = JoinSet::new();
            for session in &sessions {
                let session = Arc::clone(session);
                closes.spawn(async move {
                    let id = session.key().to_owned();
                    (id, Self::close_session(&session).await)
                });
            }
            let mut failure = None;
            while let Some(result) = closes.join_next().await {
                match result {
                    Ok((_, Ok(()))) => {}
                    Ok((_, Err(error))) => {
                        failure.get_or_insert(error);
                    }
                    Err(error) => {
                        failure.get_or_insert_with(|| {
                            SshError::Internal(format!("SSH shutdown task failed: {error}"))
                        });
                    }
                }
            }

            let mut releases = JoinSet::new();
            for session in &sessions {
                let resources = session.resources.clone();
                releases.spawn(async move { resources.wait_released().await });
            }
            while let Some(result) = releases.join_next().await {
                if let Err(error) = result {
                    failure.get_or_insert_with(|| {
                        SshError::Internal(format!("SSH resource drain failed: {error}"))
                    });
                }
            }

            for session in &sessions {
                if let Err(error) = self
                    .update_cached_status(session.key(), SshStatus::Disconnected, None, None)
                    .await
                {
                    failure.get_or_insert(error);
                }
            }
            let mut current = self.sessions.write().await;
            for session in &sessions {
                if current
                    .get(session.key())
                    .is_some_and(|found| Arc::ptr_eq(found, session))
                {
                    current.remove(session.key());
                }
            }
            drop(current);
            failure.map_or(Ok(sessions.len()), Err)
        })
        .await
        .map_err(|_| SshError::Timeout(SERVICE_SHUTDOWN_TIMEOUT))?
    }

    async fn load_locked(&self) -> SshResult<()> {
        let configs = self
            .config_store
            .load()
            .await?
            .into_iter()
            .map(|mut config| {
                self.validate_secret_storage(&config)?;
                config.timing.validate()?;
                if let Some(proxy) = &config.proxy {
                    proxy.validate()?;
                }
                if let Some(jump) = &mut config.jump {
                    jump.validate()?;
                    jump.known_hosts_path =
                        Some(self.resolve_known_hosts(jump.known_hosts_path.as_deref())?);
                }
                if config.auth_type == SshAuthType::Chain {
                    config.validate_credentials()?;
                }
                config.known_hosts_path =
                    Some(self.resolve_known_hosts(config.known_hosts_path.as_deref())?);
                Ok((config.id.clone(), config))
            })
            .collect::<SshResult<HashMap<_, _>>>()?;
        let retired = {
            let previous = self.configs.read().await;
            let mut retired = Vec::new();
            for (id, config) in previous.iter() {
                let unchanged = match configs.get(id) {
                    Some(next) => {
                        crate::connection_challenge::config_proof(config)?
                            == crate::connection_challenge::config_proof(next)?
                    }
                    None => false,
                };
                if !unchanged {
                    retired.push(id.clone());
                }
            }
            retired
        };
        // Preserve unchanged transports/attempts. Retiring changed identities must
        // not persist the outgoing configuration over the new input.
        for id in retired {
            self.connection_operations.retire(&id);
            self.clear_reconnect_backoff(&id);
            self.host_challenges.invalidate(&id);
            self.authentication_challenges.invalidate(&id).await;
            let session = self.sessions.write().await.remove(&id);
            if let Some(session) = session {
                Self::close_session(&session).await?;
            }
        }
        *self.configs.write().await = configs;
        self.loaded.store(true, Ordering::Relaxed);
        Ok(())
    }

    pub async fn persist_configs(&self) -> SshResult<()> {
        let configs = self.configs.read().await;
        let mut values = configs.values().cloned().collect::<Vec<_>>();
        values.sort_by(|left, right| left.id.cmp(&right.id));
        self.config_store.save(&values).await
    }

    fn validate_secret_storage(&self, config: &SshConnectionConfig) -> SshResult<()> {
        if (config.key_content.is_some()
            || config
                .jump
                .as_ref()
                .is_some_and(|jump| jump.contains_secret())
            || config
                .proxy
                .as_ref()
                .is_some_and(|proxy| proxy.password.is_some())
            || config
                .authentication_factors
                .iter()
                .any(|factor| factor.contains_secret()))
            && !self.config_store.supports_secret_storage()
        {
            return Err(SshError::InvalidConfig(
                "private-key content, authentication chain, jump and proxy secrets require secure encrypted configuration storage".into(),
            ));
        }
        Ok(())
    }

    pub async fn list_connections(&self) -> Vec<SshConnectionDto> {
        self.ensure_loaded().await;
        let configs = self.configs.read().await;
        let sessions = self.sessions.read().await;
        let mut out = Vec::with_capacity(configs.len());
        for cfg in configs.values() {
            let (status, last_error, latency_ms) = if let Some(s) = sessions.get(&cfg.id).cloned() {
                let session_status = s.status().await;
                let live_status = if s.client().await.is_some() {
                    Some(session_status)
                } else {
                    (session_status == SshStatus::Error).then_some(session_status)
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
            out.push(dto_from_config(cfg, status, last_error, latency_ms));
        }
        out.sort_by(|a, b| a.name.cmp(&b.name).then(a.host.cmp(&b.host)));
        out
    }

    pub async fn inspect_host_trust(
        &self,
        host: &str,
        port: u16,
        known_hosts_path: Option<&str>,
    ) -> SshResult<SshHostTrustInspection> {
        // Unsaved connection editors need endpoint validation, not credentials or a session.
        let mut config = SshConnectionConfig::new(
            "trust inspection",
            host.trim(),
            port,
            "trust inspection",
            SshAuthType::Agent,
        );
        validate_config(&config)?;
        config.known_hosts_path = Some(self.resolve_known_hosts(known_hosts_path)?);
        Ok(HostTrust::load(&config).await?.inspection(&config))
    }

    pub async fn inspect_connection_host_trust(
        &self,
        id: &str,
    ) -> SshResult<SshHostTrustInspection> {
        self.ensure_loaded().await;
        let _lifecycle = self.lifecycle.read().await;
        let config = self.config_for(id).await?;
        Ok(HostTrust::load(&config).await?.inspection(&config))
    }

    pub async fn list_host_keys(&self, id: &str) -> SshResult<Vec<crate::SshHostKeyRecord>> {
        self.ensure_loaded().await;
        let _lifecycle = self.lifecycle.read().await;
        let config = self.config_for(id).await?;
        let store = self.application_trust_store()?;
        let identity = crate::SshHostIdentity::for_config(&config)?;
        tokio::task::spawn_blocking(move || store.list(&identity))
            .await
            .map_err(|error| SshError::Internal(format!("SSH trust listing task: {error}")))?
    }

    pub async fn import_host_trust(
        &self,
        id: &str,
        expected: SshHostTrustInspection,
    ) -> SshResult<Vec<crate::SshHostKeyRecord>> {
        self.ensure_loaded().await;
        let _lifecycle = self.lifecycle.write().await;
        let config = self.config_for(id).await?;
        let store = self.application_trust_store()?;
        let verified = HostTrust::load(&config).await?;
        if verified.clone().inspection(&config) != expected {
            return Err(HostTrust::error(
                &config,
                "trust preview is stale; inspect and verify again",
            ));
        }
        let identity = crate::SshHostIdentity::for_config(&config)?;
        let keys = verified.public_keys();
        let source = verified.path;
        let records = tokio::task::spawn_blocking(move || {
            store.trust_verified_batch(&identity, &keys, &source)?;
            store.list(&identity)
        })
        .await
        .map_err(|error| SshError::Internal(format!("SSH trust import task: {error}")))??;
        self.disconnect_locked(id).await?;
        Ok(records)
    }

    pub async fn revoke_host_key(&self, id: &str, fingerprint: &str) -> SshResult<()> {
        self.ensure_loaded().await;
        let _lifecycle = self.lifecycle.write().await;
        let config = self.config_for(id).await?;
        let store = self.application_trust_store()?;
        let identity = crate::SshHostIdentity::for_config(&config)?;
        let fingerprint = fingerprint.to_owned();
        tokio::task::spawn_blocking(move || store.revoke(&identity, &fingerprint))
            .await
            .map_err(|error| SshError::Internal(format!("SSH trust revoke task: {error}")))??;
        self.disconnect_locked(id).await
    }

    pub(super) fn application_trust_store(&self) -> SshResult<Arc<crate::SshHostKeyStore>> {
        self.host_key_store.clone().ok_or_else(|| {
            SshError::InvalidConfig("SSH application trust store is not configured".into())
        })
    }

    pub async fn create_connection(
        &self,
        request: CreateSshConnectionRequest,
    ) -> SshResult<SshConnectionDto> {
        self.ensure_loaded().await;
        let _lifecycle = self.lifecycle.read().await;
        self.connection_operations.ensure_running()?;
        let mut config = SshConnectionConfig::new(
            request.name,
            request.host,
            request.port,
            request.username,
            request.auth_type,
        );
        config.known_hosts_path =
            Some(self.resolve_known_hosts(request.known_hosts_path.as_deref())?);
        config.host_key_policy = request.host_key_policy;
        config.key_path = request.key_path;
        config.agent_fingerprint = request.agent_fingerprint;
        config.key_content = request.key_content;
        config.certificate_path = request.certificate_path;
        config.password = request.password;
        config.timing = request.timing;
        config.proxy = crate::proxy::resolve_proxy(request.proxy, None)?;
        config.jump = crate::jump::resolve_jump(
            request.jump,
            None,
            config.known_hosts_path.as_deref().unwrap_or_default(),
        )?;
        if let Some(jump) = &mut config.jump {
            jump.known_hosts_path =
                Some(self.resolve_known_hosts(jump.known_hosts_path.as_deref())?);
        }
        if config.auth_type == SshAuthType::Chain {
            config.authentication_factors = crate::authentication_factors::resolve_factors(
                request.authentication_factors,
                &[],
            )?;
        }
        config.normalize_credentials();
        validate_config(&config)?;
        self.validate_secret_storage(&config)?;
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
        let _lifecycle = self.lifecycle.write().await;
        self.connection_operations.ensure_running()?;
        let mut configs = self.configs.write().await;
        let cfg = configs
            .get_mut(&request.id)
            .ok_or_else(|| SshError::NotFound(request.id.clone()))?;
        let mut candidate = SshConnectionConfig {
            id: cfg.id.clone(),
            name: request.name,
            host: request.host,
            port: request.port,
            username: request.username,
            auth_type: request.auth_type,
            agent_fingerprint: request.agent_fingerprint.or_else(|| {
                (request.auth_type == cfg.auth_type && request.auth_type.uses_agent())
                    .then(|| cfg.agent_fingerprint.clone())
                    .flatten()
            }),
            proxy: crate::proxy::resolve_proxy(request.proxy, cfg.proxy.as_ref())?,
            jump: crate::jump::resolve_jump(
                request.jump,
                cfg.jump.as_ref(),
                cfg.known_hosts_path.as_deref().unwrap_or_default(),
            )?,
            authentication_factors: if request.auth_type == SshAuthType::Chain {
                crate::authentication_factors::resolve_factors(
                    request.authentication_factors,
                    &cfg.authentication_factors,
                )?
            } else {
                Vec::new()
            },
            host_key_policy: request.host_key_policy.unwrap_or(cfg.host_key_policy),
            timing: request.timing.unwrap_or(cfg.timing),
            key_path: request.key_path.clone(),
            key_content: request.key_content.or_else(|| {
                (cfg.auth_type == SshAuthType::KeyContent && request.auth_type == cfg.auth_type)
                    .then(|| cfg.key_content.clone())
                    .flatten()
            }),
            certificate_path: request
                .certificate_path
                .or_else(|| cfg.certificate_path.clone()),
            known_hosts_path: Some(
                self.resolve_known_hosts(
                    request
                        .known_hosts_path
                        .as_deref()
                        .or(cfg.known_hosts_path.as_deref()),
                )?,
            ),
            password: request.password,
            extra_options: cfg.extra_options.clone(),
            control_path: None,
            cached_status: SshStatus::Disconnected,
            cached_last_error: None,
            cached_latency_ms: None,
            cached_checked_at_ms: None,
        };
        candidate.normalize_credentials();
        if let Some(jump) = &mut candidate.jump {
            jump.known_hosts_path =
                Some(self.resolve_known_hosts(jump.known_hosts_path.as_deref())?);
        }
        validate_config(&candidate)?;
        self.validate_secret_storage(&candidate)?;
        self.clear_reconnect_backoff(&request.id);
        *cfg = candidate.clone();
        drop(configs);
        let _ = self.disconnect_locked(&request.id).await;
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
        let _lifecycle = self.lifecycle.write().await;
        self.connection_operations.ensure_running()?;
        let _ = self.disconnect_locked(id).await;
        let mut configs = self.configs.write().await;
        configs.remove(id);
        drop(configs);
        self.persist_configs().await?;
        Ok(())
    }

    pub async fn connect(&self, id: &str) -> SshResult<SshServiceHandle> {
        self.ensure_loaded().await;
        let permit = self.connection_operations.acquire(id).await?;
        let cfg = self.operation_config(id, &permit).await?;
        self.connect_config(&cfg, None, &permit).await
    }

    pub(super) async fn connect_config(
        &self,
        cfg: &SshConnectionConfig,
        confirmed: Option<HostTrust>,
        permit: &ConnectionPermit,
    ) -> SshResult<SshServiceHandle> {
        let attempt_id = uuid::Uuid::new_v4().to_string();
        match self
            .connect_config_attempt(cfg, confirmed, None, &attempt_id, permit)
            .await?
        {
            SshConnectionAttempt::Connected => {
                let session = self.operation_session(cfg, permit).await?;
                let generation = session
                    .transport_generation()
                    .ok_or_else(|| SshError::ConnectionLost(cfg.id.clone()))?;
                self.clear_reconnect_backoff(&cfg.id);
                Ok(SshServiceHandle::new(&cfg.id, &cfg.id, 80, 24)
                    .with_connection_generation(generation))
            }
            SshConnectionAttempt::AuthenticationRequired { challenge } => {
                self.authentication_challenges.invalidate(&cfg.id).await;
                Err(SshError::AuthFailed(
                    cfg.username.clone(),
                    format!(
                        "keyboard-interactive authentication is required; resume attempt {} through begin_connection",
                        challenge.attempt_id
                    ),
                ))
            }
            SshConnectionAttempt::ConfirmationRequired { .. } => Err(SshError::Internal(
                "connection configuration unexpectedly requested host confirmation".into(),
            )),
        }
    }

    pub(super) async fn connect_config_attempt(
        &self,
        cfg: &SshConnectionConfig,
        confirmed: Option<HostTrust>,
        confirmed_jump: Option<HostTrust>,
        attempt_id: &str,
        permit: &ConnectionPermit,
    ) -> SshResult<SshConnectionAttempt> {
        let id = cfg.id.as_str();
        let deadline = self.host_challenges.deadline(id, attempt_id).ok();
        self.authentication_challenges.invalidate(id).await;
        permit.ensure_current()?;
        if let Some((session, _client)) = self.reusable_client(cfg, permit).await? {
            let _lifecycle = self.lock_connection_generation(permit, &session).await?;
            session.set_status(SshStatus::Connected, None).await;
            session.resources.published();
            session.touch_keepalive();
            let _ = self
                .update_cached_status(id, SshStatus::Connected, None, cfg.cached_latency_ms)
                .await;
            return Ok(SshConnectionAttempt::Connected);
        }

        let session = self.operation_session(cfg, permit).await?;
        session.set_connecting_attempt(attempt_id).await;
        let started = std::time::Instant::now();
        let progress = match permit
            .run(cfg.timing.connecting_until(
                connect_client_progress(
                    cfg,
                    self.host_key_store.clone(),
                    confirmed,
                    confirmed_jump,
                    deadline.is_some(),
                ),
                deadline.map(|deadline| deadline.instant),
            ))
            .await
        {
            Ok(verified) => verified,
            Err(err) => {
                let msg = err.to_string();
                let _ = self
                    .set_operation_status(
                        permit,
                        &session,
                        SshStatus::Error,
                        Some(msg),
                        Some(started.elapsed().as_millis() as u64),
                    )
                    .await;
                return Err(err);
            }
        };
        self.accept_connection_progress(
            cfg,
            attempt_id,
            progress,
            started.elapsed(),
            deadline,
            &session,
            permit,
        )
        .await
    }

    pub(super) async fn accept_connection_progress(
        &self,
        cfg: &SshConnectionConfig,
        attempt_id: &str,
        progress: SshClientConnection,
        elapsed: Duration,
        deadline: Option<ChallengeDeadline>,
        session: &Arc<SshSession>,
        permit: &ConnectionPermit,
    ) -> SshResult<SshConnectionAttempt> {
        let id = cfg.id.as_str();
        if permit.ensure_current().is_err()
            || deadline.is_some_and(|deadline| {
                deadline.instant <= tokio::time::Instant::now()
                    || self.host_challenges.ensure_current(id, attempt_id).is_err()
            })
        {
            match progress {
                SshClientConnection::Connected { client, .. } => {
                    let _ = client.disconnect().await;
                }
                SshClientConnection::AuthenticationRequired { mut pending, .. } => {
                    pending.disconnect().await
                }
                SshClientConnection::ConfirmationRequired { .. } => {}
            }
            return Err(stale_authentication_challenge());
        }
        match progress {
            ConnectClientProgress::ConfirmationRequired { pending } => {
                let _lifecycle = self.lock_connection_generation(permit, session).await?;
                let challenge = self.host_challenges.present_target(
                    cfg,
                    attempt_id,
                    pending,
                    self.host_key_store.is_some(),
                )?;
                Ok(SshConnectionAttempt::ConfirmationRequired {
                    challenge: Box::new(challenge),
                })
            }
            ConnectClientProgress::Connected {
                client,
                trust,
                server_key,
            } => {
                self.publish_connected_client(
                    cfg, client, trust, server_key, elapsed, session, permit,
                )
                .await?;
                Ok(SshConnectionAttempt::Connected)
            }
            ConnectClientProgress::AuthenticationRequired { pending, round } => {
                let proof = crate::connection_challenge::config_proof(cfg)?;
                let _lifecycle = self.lock_connection_generation(permit, session).await?;
                let challenge = self
                    .authentication_challenges
                    .present(
                        cfg,
                        attempt_id,
                        proof,
                        elapsed,
                        Arc::clone(session),
                        pending,
                        round,
                        deadline,
                    )
                    .await?;
                let _ = self
                    .update_cached_status(
                        id,
                        SshStatus::Connecting,
                        None,
                        Some(elapsed.as_millis() as u64),
                    )
                    .await;
                Ok(SshConnectionAttempt::AuthenticationRequired {
                    challenge: Box::new(challenge),
                })
            }
        }
    }

    pub async fn respond_keyboard_interactive(
        &self,
        id: &str,
        attempt_id: &str,
        challenge_id: &str,
        responses: Vec<String>,
    ) -> SshResult<SshConnectionAttempt> {
        self.ensure_loaded().await;
        let permit = self.connection_operations.acquire(id).await?;
        let cfg = self.operation_config(id, &permit).await?;
        let proof = crate::connection_challenge::config_proof(&cfg)?;
        let pending = self
            .authentication_challenges
            .get(id, attempt_id, challenge_id)
            .await?;
        let session = pending.session();
        if !self.authentication_challenges.is_current(id, &pending) {
            return Err(stale_authentication_challenge());
        }

        let mut state = pending.state.lock().await;
        let expiration = pending.expiration(state.deadline);
        if expiration.is_some() || !self.authentication_challenges.is_current(id, &pending) {
            drop(state);
            self.authentication_challenges
                .close_if_current(id, &pending)
                .await;
            let error = expiration.unwrap_or_else(stale_authentication_challenge);
            let _ = self
                .set_operation_status(
                    &permit,
                    &session,
                    SshStatus::Error,
                    Some(error.to_string()),
                    None,
                )
                .await;
            return Err(error);
        }
        if state.config_proof != proof {
            let error = SshError::InvalidConfig(
                "SSH connection configuration changed while authentication was pending".into(),
            );
            drop(state);
            self.authentication_challenges
                .close_if_current(id, &pending)
                .await;
            let _ = self
                .set_operation_status(
                    &permit,
                    &session,
                    SshStatus::Error,
                    Some(error.to_string()),
                    None,
                )
                .await;
            return Err(error);
        }
        let expected_responses = state.challenge.prompts.len();
        if responses.len() != expected_responses {
            return Err(SshError::InvalidConfig(format!(
                "SSH authentication challenge expects {expected_responses} responses"
            )));
        }
        let (trust, active_config) = match state.client.as_ref() {
            Some(client) => (client.trust.clone(), client.role.config(&cfg)?),
            None => {
                drop(state);
                self.authentication_challenges
                    .close_if_current(id, &pending)
                    .await;
                return Err(stale_authentication_challenge());
            }
        };
        let trust_check = tokio::select! {
            _ = pending.cancelled.cancelled() => Err(stale_authentication_challenge()),
            result = trust.reload(&active_config, self.host_key_store.clone()) => result,
        };
        match trust_check {
            Ok(current) if current == trust => {}
            Ok(_) => {
                let error = HostTrust::error(
                    &cfg,
                    "host trust changed while keyboard-interactive authentication was pending",
                );
                drop(state);
                self.authentication_challenges
                    .close_if_current(id, &pending)
                    .await;
                let _ = self
                    .set_operation_status(
                        &permit,
                        &session,
                        SshStatus::Error,
                        Some(error.to_string()),
                        None,
                    )
                    .await;
                return Err(error);
            }
            Err(error) => {
                drop(state);
                self.authentication_challenges
                    .close_if_current(id, &pending)
                    .await;
                let _ = self
                    .set_operation_status(
                        &permit,
                        &session,
                        SshStatus::Error,
                        Some(error.to_string()),
                        None,
                    )
                    .await;
                return Err(error);
            }
        }

        pending.resume_network();
        let response_started = std::time::Instant::now();
        let deadline = state.deadline;
        let cancelled = pending.cancelled.clone();
        let step = match state.client.as_mut() {
            Some(client) => {
                cfg.timing
                    .connecting_until(
                        client.respond(&cancelled, &cfg, self.host_key_store.clone(), responses),
                        Some(deadline.instant),
                    )
                    .await
            }
            None => Err(stale_authentication_challenge()),
        };
        state.elapsed += response_started.elapsed();

        match step {
            Ok(AuthenticationStep::ConfirmationRequired(continuation)) => {
                if state.deadline.instant <= tokio::time::Instant::now()
                    || pending.cancelled.is_cancelled()
                    || !self.authentication_challenges.is_current(id, &pending)
                {
                    drop(state);
                    self.authentication_challenges
                        .close_if_current(id, &pending)
                        .await;
                    return Err(stale_authentication_challenge());
                }
                let challenge = self.host_challenges.present_target(
                    &cfg,
                    attempt_id,
                    continuation,
                    self.host_key_store.is_some(),
                )?;
                state.client.take();
                if !self
                    .authentication_challenges
                    .remove_if_current(id, &pending)
                {
                    self.host_challenges.discard(id, attempt_id);
                    return Err(stale_authentication_challenge());
                }
                Ok(SshConnectionAttempt::ConfirmationRequired {
                    challenge: Box::new(challenge),
                })
            }
            Ok(AuthenticationStep::Continue(round)) => {
                let current_client = state
                    .client
                    .as_ref()
                    .ok_or_else(stale_authentication_challenge)?;
                let expected_trust = current_client.trust.clone();
                let current_config = current_client.role.config(&cfg)?;
                let role = current_client.role;
                let current_trust = tokio::select! {
                    _ = pending.cancelled.cancelled() => Err(stale_authentication_challenge()),
                    result = expected_trust.reload(&current_config, self.host_key_store.clone()) => result,
                };
                if !matches!(&current_trust, Ok(current) if current == &expected_trust)
                    || state.deadline.instant <= tokio::time::Instant::now()
                    || !self.authentication_challenges.is_current(id, &pending)
                {
                    drop(state);
                    self.authentication_challenges
                        .close_if_current(id, &pending)
                        .await;
                    let error = current_trust
                        .err()
                        .unwrap_or_else(stale_authentication_challenge);
                    let _ = self
                        .set_operation_status(
                            &permit,
                            &session,
                            SshStatus::Error,
                            Some(error.to_string()),
                            None,
                        )
                        .await;
                    return Err(error);
                }
                let waiting_deadline = pending.await_user(state.deadline);
                let challenge = next_challenge(&cfg, attempt_id, waiting_deadline, round, role)?;
                *pending
                    .active_challenge_id
                    .write()
                    .unwrap_or_else(|error| error.into_inner()) = challenge.challenge_id.clone();
                state.challenge = challenge.clone();
                Ok(SshConnectionAttempt::AuthenticationRequired {
                    challenge: Box::new(challenge),
                })
            }
            Ok(AuthenticationStep::Connected {
                client,
                trust: authenticated_trust,
                server_key,
            }) => {
                let current_trust = tokio::select! {
                    _ = pending.cancelled.cancelled() => Err(stale_authentication_challenge()),
                    result = authenticated_trust.reload(&cfg, self.host_key_store.clone()) => result,
                };
                if !matches!(&current_trust, Ok(current) if current == &authenticated_trust)
                    || state.deadline.instant <= tokio::time::Instant::now()
                    || pending.cancelled.is_cancelled()
                    || !self.authentication_challenges.is_current(id, &pending)
                {
                    let error = current_trust
                        .err()
                        .unwrap_or_else(stale_authentication_challenge);
                    drop(state);
                    let _ = client.disconnect().await;
                    self.authentication_challenges
                        .close_if_current(id, &pending)
                        .await;
                    let _ = self
                        .set_operation_status(
                            &permit,
                            &session,
                            SshStatus::Error,
                            Some(error.to_string()),
                            None,
                        )
                        .await;
                    return Err(error);
                }
                let elapsed = state.elapsed;
                state.client.take();
                if !self
                    .authentication_challenges
                    .remove_if_current(id, &pending)
                {
                    drop(state);
                    let _ = client.disconnect().await;
                    return Err(stale_authentication_challenge());
                }
                drop(state);
                self.host_challenges.discard(id, attempt_id);
                self.publish_connected_client(
                    &cfg,
                    client,
                    authenticated_trust,
                    server_key,
                    elapsed,
                    &session,
                    &permit,
                )
                .await?;
                Ok(SshConnectionAttempt::Connected)
            }
            Err(error) => {
                drop(state);
                self.authentication_challenges
                    .close_if_current(id, &pending)
                    .await;
                let _ = self
                    .set_operation_status(
                        &permit,
                        &session,
                        SshStatus::Error,
                        Some(error.to_string()),
                        None,
                    )
                    .await;
                Err(error)
            }
        }
    }

    pub async fn cancel_connection_attempt(
        &self,
        id: &str,
        attempt_id: &str,
        challenge_id: &str,
    ) -> SshResult<()> {
        self.ensure_loaded().await;
        self.authentication_challenges
            .cancel(id, attempt_id, challenge_id)
            .await?;
        self.host_challenges.discard(id, attempt_id);
        self.set_authentication_status(id, SshStatus::Disconnected, None)
            .await;
        Ok(())
    }

    async fn set_authentication_status(
        &self,
        id: &str,
        status: SshStatus,
        last_error: Option<String>,
    ) {
        if let Some(session) = self.sessions.read().await.get(id).cloned() {
            session.set_status(status, last_error.clone()).await;
        }
        if let Err(error) = self
            .update_cached_status(id, status, last_error, None)
            .await
        {
            tracing::debug!(target: "deepagent_ssh", connection_id = id, %error,
                "failed to persist SSH authentication status");
        }
    }

    pub(super) async fn publish_connected_client(
        &self,
        cfg: &SshConnectionConfig,
        client: Client,
        trust: HostTrust,
        server_key: PublicKey,
        elapsed: Duration,
        session: &Arc<SshSession>,
        permit: &ConnectionPermit,
    ) -> SshResult<SshServiceHandle> {
        let id = cfg.id.as_str();
        if let Err(error) = client
            .verify_jump_trust(cfg, self.host_key_store.clone())
            .await
        {
            let _ = client.disconnect().await;
            return Err(error);
        }
        let _lifecycle = match self.lock_connection_generation(permit, session).await {
            Ok(lifecycle) => lifecycle,
            Err(error) => {
                let _ = client.disconnect().await;
                return Err(error);
            }
        };
        *session.host_trust.write().await = Some(trust);
        session.set_client(Some(client)).await;
        let generation = session
            .transport_generation()
            .ok_or_else(|| SshError::ConnectionLost(id.into()))?;
        session.set_status(SshStatus::Connected, None).await;
        session.touch_keepalive();
        self.clear_reconnect_backoff(id);
        self.record_successful_authentication(cfg, server_key).await;
        let _ = self
            .update_cached_status(
                id,
                SshStatus::Connected,
                None,
                Some(elapsed.as_millis() as u64),
            )
            .await;
        tracing::debug!(
            target: "deepagent_ssh",
            id,
            elapsed_ms = elapsed.as_millis(),
            "ssh connection established"
        );
        Ok(SshServiceHandle::new(id, id, 80, 24).with_connection_generation(generation))
    }

    async fn record_successful_authentication(&self, cfg: &SshConnectionConfig, key: PublicKey) {
        let Some(store) = self.host_key_store.clone() else {
            return;
        };
        let identity = match crate::SshHostIdentity::for_config(cfg) {
            Ok(identity) => identity,
            Err(error) => {
                tracing::debug!(target: "deepagent_ssh", connection_id = cfg.id, %error,
                    "failed to identify endpoint for SSH authentication activity");
                return;
            }
        };
        let result = tokio::task::spawn_blocking(move || {
            store.record_authentication(&identity, &key, deepagent_core::clock::SystemClock.now())
        })
        .await;
        match result {
            Ok(Ok(())) => {}
            Ok(Err(error)) => tracing::debug!(
                target: "deepagent_ssh",
                connection_id = cfg.id,
                %error,
                "failed to record successful SSH authentication activity"
            ),
            Err(error) => tracing::debug!(
                target: "deepagent_ssh",
                connection_id = cfg.id,
                %error,
                "failed to record successful SSH authentication activity"
            ),
        }
    }

    pub async fn exec(&self, handle: &SshServiceHandle, command: &str) -> SshResult<SshExecResult> {
        let session = self.connected_session_for_handle(handle).await?;
        let client = session
            .client()
            .await
            .ok_or_else(|| SshError::ConnectionLost(handle.connection_id.clone()))?;
        let start = std::time::Instant::now();
        let output = client.execute(command).await?;
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

    pub async fn list_dir(
        &self,
        handle: &SshServiceHandle,
        path: &str,
    ) -> SshResult<SshDirListing> {
        let session = self.connected_session_for_handle(handle).await?;
        let client = session
            .client()
            .await
            .ok_or_else(|| SshError::ConnectionLost(handle.connection_id.clone()))?;
        let listing = sftp_list_dir(&handle.connection_id, &client, path).await?;
        session.touch_keepalive();
        Ok(listing)
    }

    pub async fn read_file(
        &self,
        handle: &SshServiceHandle,
        path: &str,
    ) -> SshResult<SshFileContent> {
        let session = self.connected_session_for_handle(handle).await?;
        let client = session
            .client()
            .await
            .ok_or_else(|| SshError::ConnectionLost(handle.connection_id.clone()))?;
        let content = sftp_read_file(&handle.connection_id, &client, path).await?;
        session.touch_keepalive();
        Ok(content)
    }

    pub async fn file_metadata(
        &self,
        handle: &SshServiceHandle,
        path: &str,
        follow_symlinks: bool,
    ) -> SshResult<crate::RemoteFileMetadata> {
        crate::files::validate_path(path)?;
        let (session, sftp) = self.file_session_for_handle(handle).await?;
        let attrs = if follow_symlinks {
            sftp.metadata(path).await
        } else {
            sftp.symlink_metadata(path).await
        };
        sftp_close_quiet(&handle.connection_id, &sftp).await;
        let attrs = attrs.map_err(|error| {
            SshError::Internal(format!(
                "SFTP {}: {error}",
                if follow_symlinks { "stat" } else { "lstat" }
            ))
        })?;
        session.touch_keepalive();
        Ok(crate::files::metadata(
            &handle.connection_id,
            path,
            follow_symlinks,
            attrs,
        ))
    }

    pub async fn realpath(
        &self,
        handle: &SshServiceHandle,
        path: &str,
    ) -> SshResult<crate::RemoteRealpath> {
        crate::files::validate_path(path)?;
        let (session, sftp) = self.file_session_for_handle(handle).await?;
        let canonical_path = sftp.canonicalize(path).await;
        sftp_close_quiet(&handle.connection_id, &sftp).await;
        let canonical_path = canonical_path
            .map_err(|error| SshError::Internal(format!("SFTP realpath: {error}")))?;
        crate::files::validate_path(&canonical_path)?;
        session.touch_keepalive();
        Ok(crate::RemoteRealpath {
            connection_id: handle.connection_id.clone(),
            path: path.into(),
            canonical_path,
        })
    }

    pub(crate) async fn file_session(
        &self,
        connection_id: &str,
    ) -> SshResult<(SessionLease, SftpSession)> {
        let (session, client) = self.file_client(connection_id).await?;
        let sftp = sftp_open(&client, None).await?;
        Ok((session, sftp))
    }

    async fn file_session_for_handle(
        &self,
        handle: &SshServiceHandle,
    ) -> SshResult<(SessionLease, SftpSession)> {
        let session = self.connected_session_for_handle(handle).await?;
        let client = session
            .client()
            .await
            .ok_or_else(|| SshError::ConnectionLost(handle.connection_id.clone()))?;
        let sftp = sftp_open(&client, None).await?;
        Ok((session, sftp))
    }

    pub(crate) async fn file_client(
        &self,
        connection_id: &str,
    ) -> SshResult<(SessionLease, Client)> {
        let session = self.connected_session(connection_id).await?;
        let client = session
            .client()
            .await
            .ok_or_else(|| SshError::ConnectionLost(connection_id.into()))?;
        Ok((session, client))
    }

    pub async fn read_file_binary(
        &self,
        handle: &SshServiceHandle,
        path: &str,
    ) -> SshResult<SshFileBinary> {
        let session = self.connected_session_for_handle(handle).await?;
        let client = session
            .client()
            .await
            .ok_or_else(|| SshError::ConnectionLost(handle.connection_id.clone()))?;
        let content = sftp_read_file_binary(&handle.connection_id, &client, path).await?;
        session.touch_keepalive();
        Ok(content)
    }

    pub async fn create_dir(&self, handle: &SshServiceHandle, path: &str) -> SshResult<()> {
        let session = self.connected_session_for_handle(handle).await?;
        let client = session
            .client()
            .await
            .ok_or_else(|| SshError::ConnectionLost(handle.connection_id.clone()))?;
        sftp_create_dir(&handle.connection_id, &client, path).await?;
        session.touch_keepalive();
        Ok(())
    }

    pub async fn create_file(&self, handle: &SshServiceHandle, path: &str) -> SshResult<()> {
        let session = self.connected_session_for_handle(handle).await?;
        let client = session
            .client()
            .await
            .ok_or_else(|| SshError::ConnectionLost(handle.connection_id.clone()))?;
        sftp_create_file(&handle.connection_id, &client, path).await?;
        session.touch_keepalive();
        Ok(())
    }

    pub async fn rename_path(
        &self,
        handle: &SshServiceHandle,
        old_path: &str,
        new_path: &str,
    ) -> SshResult<()> {
        let session = self.connected_session_for_handle(handle).await?;
        let client = session
            .client()
            .await
            .ok_or_else(|| SshError::ConnectionLost(handle.connection_id.clone()))?;
        sftp_rename_path(&handle.connection_id, &client, old_path, new_path).await?;
        session.touch_keepalive();
        Ok(())
    }

    pub async fn remove_path(&self, handle: &SshServiceHandle, path: &str) -> SshResult<()> {
        let session = self.connected_session_for_handle(handle).await?;
        let client = session
            .client()
            .await
            .ok_or_else(|| SshError::ConnectionLost(handle.connection_id.clone()))?;
        sftp_remove_path(&handle.connection_id, &client, path).await?;
        session.touch_keepalive();
        Ok(())
    }

    pub async fn pty_spawn(
        &self,
        handle: &SshServiceHandle,
        cols: u16,
        rows: u16,
    ) -> SshResult<SshServiceHandle> {
        self.spawn_pty(handle, cols, rows, uuid::Uuid::new_v4().to_string(), true)
            .await
    }

    pub(super) async fn spawn_pty(
        &self,
        handle: &SshServiceHandle,
        cols: u16,
        rows: u16,
        token: String,
        legacy: bool,
    ) -> SshResult<SshServiceHandle> {
        let session = self.connected_session_for_handle(handle).await?;
        let generation = session
            .transport_generation()
            .ok_or_else(|| SshError::ConnectionLost(handle.connection_id.clone()))?;
        let client = session
            .client()
            .await
            .ok_or_else(|| SshError::ConnectionLost(handle.connection_id.clone()))?;

        let mut pty = crate::pty::open(&client, &handle.connection_id, cols, rows).await?;
        pty._resource = Some(
            session
                .resources
                .acquire()
                .ok_or_else(|| SshError::ConnectionLost(handle.connection_id.clone()))?,
        );
        session.insert_pty(token.clone(), pty, legacy).await;
        Ok(
            SshServiceHandle::new(handle.connection_id.clone(), token, cols, rows)
                .with_connection_generation(generation),
        )
    }

    pub async fn pty_write(&self, handle: &SshServiceHandle, data: &[u8]) -> SshResult<()> {
        let session = self.active_pty_connection(handle).await?;
        if session.pty_write(&handle.token, data.to_vec()).await {
            Ok(())
        } else {
            Err(SshError::Pty("no active PTY session".into()))
        }
    }

    pub async fn pty_read(&self, handle: &SshServiceHandle) -> SshResult<Vec<u8>> {
        let session = self.active_pty_connection(handle).await?;
        session
            .pty_read_available(&handle.token)
            .await
            .ok_or_else(|| SshError::Pty("no active PTY session".into()))
    }

    pub(super) async fn pty_subscribe_output(
        &self,
        handle: &SshServiceHandle,
    ) -> SshResult<deepagent_terminal::TerminalOutputSubscription> {
        let session = self.active_pty_connection(handle).await?;
        session
            .subscribe_output(&handle.token)
            .await
            .ok_or_else(|| SshError::Pty("no active PTY session".into()))
    }

    pub async fn pty_read_with_cursor(
        &self,
        handle: &SshServiceHandle,
        after_cursor: u64,
    ) -> SshResult<deepagent_terminal::TerminalReadChunk> {
        let session = self.active_pty_connection(handle).await?;
        session
            .pty_read_with_cursor(&handle.token, after_cursor)
            .await
            .ok_or_else(|| SshError::Pty("no active PTY session".into()))
    }

    pub async fn pty_resize(
        &self,
        handle: &SshServiceHandle,
        cols: u16,
        rows: u16,
    ) -> SshResult<()> {
        let session = self.active_pty_connection(handle).await?;
        if session.pty_resize(&handle.token, cols, rows).await {
            Ok(())
        } else {
            Err(SshError::Pty("no active PTY session".into()))
        }
    }

    pub async fn pty_close(&self, handle: &SshServiceHandle) -> SshResult<()> {
        let session = self
            .sessions
            .read()
            .await
            .get(&handle.connection_id)
            .cloned();
        match session {
            Some(session) => {
                if handle
                    .connection_generation
                    .is_some_and(|generation| session.transport_generation() != Some(generation))
                {
                    return Err(SshError::ConnectionLost(handle.connection_id.clone()));
                }
                session.close_pty(&handle.token).await
            }
            None => Ok(()),
        }
    }

    async fn active_pty_connection(&self, handle: &SshServiceHandle) -> SshResult<Arc<SshSession>> {
        self.ensure_loaded().await;
        let id = handle.connection_id.as_str();
        let permit = self.connection_operations.acquire(id).await?;
        let config = self.operation_config(id, &permit).await?;
        let session = self
            .reusable_client(&config, &permit)
            .await?
            .map(|(session, _)| session)
            .ok_or_else(|| {
                SshError::Pty(
                    "PTY connection is no longer active; open a new terminal explicitly".into(),
                )
            })?;
        if handle
            .connection_generation
            .is_some_and(|generation| session.transport_generation() != Some(generation))
        {
            return Err(SshError::ConnectionLost(handle.connection_id.clone()));
        }
        Ok(session)
    }

    pub async fn disconnect(&self, id: &str) -> SshResult<()> {
        // Wake the network owner before waiting for the short publication lock.
        self.connection_operations.retire(id);
        let _lifecycle = self.lifecycle.write().await;
        self.disconnect_locked(id).await
    }

    pub(super) async fn disconnect_locked(&self, id: &str) -> SshResult<()> {
        self.connection_operations.retire(id);
        self.clear_reconnect_backoff(id);
        self.host_challenges.invalidate(id);
        self.authentication_challenges.invalidate(id).await;
        let session = self.sessions.write().await.remove(id);
        let disconnected = match session {
            Some(session) => Self::close_session(&session).await,
            None => Ok(()),
        };
        // A removed live session must not fall back to a stale Connected cache.
        // Always clear runtime/cache state, but do not swallow transport failures.
        let cached = self
            .update_cached_status(id, SshStatus::Disconnected, None, None)
            .await;
        disconnected.and(cached)
    }

    pub(super) async fn close_session(session: &SshSession) -> SshResult<()> {
        session.resources.retire();
        session.close_all_ptys().await;
        let disconnected = match session.client().await {
            Some(client) => client.disconnect().await,
            None => Ok(()),
        };
        session.set_client(None).await;
        session.set_status(SshStatus::Disconnected, None).await;
        disconnected
    }

    async fn discard_cached_session(&self, expected: &Arc<SshSession>) -> SshResult<()> {
        let removed = {
            let mut sessions = self.sessions.write().await;
            if sessions
                .get(expected.key())
                .is_some_and(|current| Arc::ptr_eq(current, expected))
            {
                sessions.remove(expected.key())
            } else {
                None
            }
        };
        if let Some(session) = removed {
            Self::close_session(&session).await?;
            let sessions = self.sessions.read().await;
            if !sessions.contains_key(session.key()) {
                self.update_cached_status(session.key(), SshStatus::Disconnected, None, None)
                    .await?;
            }
        }
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
                let session_status = s.status().await;
                let session_error = s.last_error().await;
                if let Some(client) = s.client().await {
                    if !client.is_closed() {
                        return Ok(SshStatusSnapshot {
                            status: session_status,
                            last_error: session_error,
                            latency_ms: cached.and_then(|snap| snap.latency_ms),
                        });
                    }
                }
                if session_status == SshStatus::Error {
                    let last_error = session_error.or_else(|| {
                        cached
                            .as_ref()
                            .and_then(|snapshot| snapshot.last_error.clone())
                    });
                    if cached.as_ref().map_or(true, |snapshot| {
                        snapshot.status != session_status || snapshot.last_error != last_error
                    }) {
                        let _ = self
                            .update_cached_status(
                                id,
                                session_status,
                                last_error.clone(),
                                cached.as_ref().and_then(|snapshot| snapshot.latency_ms),
                            )
                            .await;
                    }
                    return Ok(SshStatusSnapshot {
                        status: session_status,
                        last_error,
                        latency_ms: cached.and_then(|snapshot| snapshot.latency_ms),
                    });
                }
                Ok(SshStatusSnapshot {
                    status: session_status,
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
        let permit = self.connection_operations.acquire(&config.id).await?;
        let mut cfg = {
            let _lifecycle = self.lifecycle.read().await;
            permit.ensure_current()?;
            let configs = self.configs.read().await;
            configs
                .get(&config.id)
                .cloned()
                .unwrap_or_else(|| config.clone())
        };
        cfg.known_hosts_path = Some(self.resolve_known_hosts(cfg.known_hosts_path.as_deref())?);
        let start = std::time::Instant::now();
        let result = match permit
            .run(cfg.timing.connecting(async {
                let (client, _) = connect_client(&cfg, self.host_key_store.clone(), None).await?;
                let output = client.execute("echo ok").await?;
                let _ = client.disconnect().await;
                Ok::<_, SshError>(output)
            }))
            .await
        {
            Ok(output) if output.exit_status == 0 => SshTestResult {
                ok: true,
                latency_ms: Some(start.elapsed().as_millis() as u64),
                banner: Some(output.stdout.trim().to_string()),
                error: None,
            },
            Ok(output) => SshTestResult {
                ok: false,
                latency_ms: Some(start.elapsed().as_millis() as u64),
                banner: None,
                error: Some(output.stderr),
            },
            Err(err) => SshTestResult {
                ok: false,
                latency_ms: Some(start.elapsed().as_millis() as u64),
                banner: None,
                error: Some(err.to_string()),
            },
        };

        let status = if result.ok {
            SshStatus::Connected
        } else {
            SshStatus::Error
        };
        let _lifecycle = self.lifecycle.read().await;
        permit.ensure_current()?;
        let _ = self
            .update_cached_status(&cfg.id, status, result.error.clone(), result.latency_ms)
            .await;
        Ok(result)
    }

    pub async fn refresh_due_statuses(&self) -> SshResult<usize> {
        if self.connection_operations.ensure_running().is_err() {
            return Ok(0);
        }
        self.reap_idle_connections().await?;
        self.ensure_loaded().await;
        let configs = {
            let guard = self.configs.read().await;
            guard.values().cloned().collect::<Vec<_>>()
        };
        let mut refreshed = 0usize;
        for previous in configs {
            if self.connection_operations.ensure_running().is_err() {
                break;
            }
            let permit = self.connection_operations.acquire(&previous.id).await?;
            let cfg = match self.operation_config(&previous.id, &permit).await {
                Ok(cfg) => cfg,
                Err(SshError::NotFound(_)) => continue,
                Err(error) => return Err(error),
            };
            if !self.should_refresh_status(&cfg).await {
                continue;
            }
            refreshed += 1;
            if let Err(err) = self.refresh_connection_status(&cfg, &permit).await {
                tracing::debug!(
                    target: "deepagent_ssh",
                    connection_id = cfg.id,
                    error = %err,
                    "ssh background health check failed"
                );
            }
            drop(permit);
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
        let output = client.execute(REMOTE_PROBE_SCRIPT).await?;
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
        sftp_upload_file(id, &client, &request.local_path, &request.remote_path).await?;
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

    pub(super) async fn config_for(&self, id: &str) -> SshResult<SshConnectionConfig> {
        self.ensure_loaded().await;
        let configs = self.configs.read().await;
        configs
            .get(id)
            .cloned()
            .ok_or_else(|| SshError::NotFound(id.to_string()))
    }

    fn resolve_known_hosts(&self, requested: Option<&str>) -> SshResult<String> {
        let path = match requested.filter(|value| !value.trim().is_empty()) {
            Some(path) => PathBuf::from(path),
            None => {
                let root = if self.data_root.is_absolute() {
                    self.data_root.clone()
                } else {
                    std::env::current_dir()?.join(&self.data_root)
                };
                root.join("known_hosts")
            }
        };
        if !path.is_absolute() {
            return Err(SshError::InvalidConfig(
                "known_hosts path must be absolute (no ~ expansion)".into(),
            ));
        }
        path.into_os_string()
            .into_string()
            .map_err(|_| SshError::InvalidConfig("known_hosts path must be valid Unicode".into()))
    }

    pub(super) async fn reusable_client(
        &self,
        cfg: &SshConnectionConfig,
        permit: &ConnectionPermit,
    ) -> SshResult<Option<(Arc<SshSession>, Client)>> {
        // Local trust revalidation and cache retirement must publish before an
        // identity edit can replace this configuration. No handshake runs here.
        let _lifecycle = self.lifecycle.read().await;
        permit.ensure_current()?;
        let session = self.sessions.read().await.get(&cfg.id).cloned();
        let Some(session) = session else {
            return Ok(None);
        };
        let Some(client) = session.client().await else {
            return Ok(None);
        };
        if client.is_closed() {
            self.discard_cached_session(&session).await?;
            return Ok(None);
        }
        if crate::connection_challenge::config_proof(session.config())?
            != crate::connection_challenge::config_proof(cfg)?
        {
            self.discard_cached_session(&session).await?;
            return Ok(None);
        }
        let previous = session.host_trust.read().await.clone();
        if let Err(error) = client
            .verify_jump_trust(cfg, self.host_key_store.clone())
            .await
        {
            self.discard_cached_session(&session).await?;
            return Err(error);
        }
        let current = match &previous {
            Some(previous) => previous.reload(cfg, self.host_key_store.clone()).await,
            None => HostTrust::load_with_store(cfg, self.host_key_store.clone()).await,
        };
        match current {
            Ok(trust) if session.host_trust.read().await.as_ref() == Some(&trust) => {
                Ok(Some((session, client)))
            }
            result => {
                // A changed or revoked source must never inherit the previously authenticated client.
                self.discard_cached_session(&session).await?;
                tracing::debug!(target: "deepagent_ssh", connection_id = cfg.id,
                    "SSH trust source changed; discarded live connection");
                if let Err(error) = result {
                    self.update_cached_status(
                        &cfg.id,
                        SshStatus::Error,
                        Some(error.to_string()),
                        None,
                    )
                    .await?;
                    return Err(error);
                }
                Ok(None)
            }
        }
    }

    pub(super) async fn operation_config(
        &self,
        id: &str,
        permit: &ConnectionPermit,
    ) -> SshResult<SshConnectionConfig> {
        let _lifecycle = self.lifecycle.read().await;
        permit.ensure_current()?;
        self.config_for(id).await
    }

    pub(super) async fn operation_session(
        &self,
        cfg: &SshConnectionConfig,
        permit: &ConnectionPermit,
    ) -> SshResult<Arc<SshSession>> {
        let _lifecycle = self.lifecycle.read().await;
        permit.ensure_current()?;
        Ok(self.session_for(cfg.clone()).await)
    }

    pub(super) async fn lock_connection_generation(
        &self,
        permit: &ConnectionPermit,
        session: &Arc<SshSession>,
    ) -> SshResult<tokio::sync::RwLockReadGuard<'_, ()>> {
        let lifecycle = self.lifecycle.read().await;
        permit.ensure_current()?;
        let sessions = self.sessions.read().await;
        if !sessions
            .get(session.key())
            .is_some_and(|current| Arc::ptr_eq(current, session))
        {
            return Err(SshError::ConnectionLost(session.key().into()));
        }
        Ok(lifecycle)
    }

    async fn set_operation_status(
        &self,
        permit: &ConnectionPermit,
        session: &Arc<SshSession>,
        status: SshStatus,
        error: Option<String>,
        latency_ms: Option<u64>,
    ) -> SshResult<()> {
        let _lifecycle = self.lock_connection_generation(permit, session).await?;
        session.set_status(status, error.clone()).await;
        self.update_cached_status(session.key(), status, error, latency_ms)
            .await
    }

    async fn session_for(&self, cfg: SshConnectionConfig) -> Arc<SshSession> {
        let mut sessions = self.sessions.write().await;
        sessions
            .entry(cfg.id.clone())
            .or_insert_with(|| SshSession::new(cfg))
            .clone()
    }

    async fn connected_session(&self, id: &str) -> SshResult<SessionLease> {
        let _ = self.connect(id).await?;
        let _lifecycle = self.lifecycle.read().await;
        let sessions = self.sessions.read().await;
        let session = sessions
            .get(id)
            .cloned()
            .ok_or_else(|| SshError::NotFound(id.to_string()))?;
        SessionLease::new(session)
    }

    async fn connected_session_for_handle(
        &self,
        handle: &SshServiceHandle,
    ) -> SshResult<SessionLease> {
        let Some(expected_generation) = handle.connection_generation else {
            return self.connected_session(&handle.connection_id).await;
        };

        self.ensure_loaded().await;
        let permit = self
            .connection_operations
            .acquire(&handle.connection_id)
            .await?;
        let config = self
            .operation_config(&handle.connection_id, &permit)
            .await?;
        let (session, client) = self
            .reusable_client(&config, &permit)
            .await?
            .ok_or_else(|| SshError::ConnectionLost(handle.connection_id.clone()))?;
        let _lifecycle = self.lock_connection_generation(&permit, &session).await?;
        if session.transport_generation() != Some(expected_generation) || client.is_closed() {
            return Err(SshError::ConnectionLost(format!(
                "{}: SSH transport generation is no longer active",
                handle.connection_id
            )));
        }
        SessionLease::new(session)
    }

    pub(super) async fn ensure_loaded(&self) {
        if !self.loaded.load(Ordering::Relaxed) {
            let _lifecycle = self.lifecycle.write().await;
            if !self.loaded.load(Ordering::Relaxed) {
                let _ = self.load_locked().await;
            }
        }
    }

    async fn should_refresh_status(&self, cfg: &SshConnectionConfig) -> bool {
        if let Some(due) = self.reconnect_retry_due(&cfg.id) {
            return due;
        }
        let session = {
            let sessions = self.sessions.read().await;
            sessions.get(&cfg.id).cloned()
        };
        let status = if let Some(session) = session {
            if let Some(client) = session.client().await {
                if client.is_closed() {
                    return true;
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

    async fn refresh_connection_status(
        &self,
        cfg: &SshConnectionConfig,
        permit: &ConnectionPermit,
    ) -> SshResult<()> {
        let existing = self.sessions.read().await.get(&cfg.id).cloned();
        if let Some(session) = existing {
            if let Some(client) = session.client().await {
                if client.is_closed() {
                    let error = client
                        .timing_error()
                        .unwrap_or_else(|| "SSH transport closed during health check".into());
                    let _lifecycle = self.lock_connection_generation(permit, &session).await?;
                    if let Err(cleanup) = self.discard_cached_session(&session).await {
                        tracing::warn!(
                            target: "deepagent_ssh",
                            connection_id = cfg.id,
                            %cleanup,
                            "failed to discard closed SSH generation"
                        );
                    }
                    let delay = self.record_reconnect_failure(&cfg.id);
                    self.update_cached_status(&cfg.id, SshStatus::Error, Some(error.clone()), None)
                        .await?;
                    tracing::warn!(
                        target: "deepagent_ssh",
                        connection_id = cfg.id,
                        retry_delay_ms = delay.as_millis() as u64,
                        %error,
                        "SSH transport health check failed; recovery is scheduled"
                    );
                    return Err(SshError::ConnectionLost(format!("{}: {error}", cfg.id)));
                }
            }
        }

        let reusable = permit.run(self.reusable_client(cfg, permit)).await?;
        if reusable.is_none() {
            if let Some(due) = self.reconnect_retry_due(&cfg.id) {
                if due {
                    return self.rebuild_health_transport(cfg, permit).await;
                }
                return Ok(());
            }
        }
        let started = std::time::Instant::now();
        let budget = Duration::from_secs(STATUS_REFRESH_TIMEOUT_SECS);
        let outcome = match &reusable {
            Some((_, client)) => {
                permit
                    .run(async {
                        timeout(budget, execute_health_check(client))
                            .await
                            .map_err(|_| SshError::Timeout(budget))?
                    })
                    .await
            }
            None => {
                permit
                    .run(async {
                        timeout(budget, async {
                            let (client, _) =
                                connect_client(cfg, self.host_key_store.clone(), None).await?;
                            let checked = execute_health_check(&client).await;
                            let _ = client.disconnect().await;
                            checked
                        })
                        .await
                        .map_err(|_| SshError::Timeout(budget))?
                    })
                    .await
            }
        };
        // Only the publication phase owns the configuration lock. An old
        // probe cannot publish cached status into a replacement epoch.
        let _lifecycle = match &reusable {
            Some((session, _)) => self.lock_connection_generation(permit, session).await?,
            None => {
                let lifecycle = self.lifecycle.read().await;
                permit.ensure_current()?;
                lifecycle
            }
        };
        let error = outcome.as_ref().err().map(ToString::to_string);
        let status = if outcome.is_ok() {
            SshStatus::Connected
        } else {
            SshStatus::Error
        };
        if let Some((session, _)) = reusable {
            if outcome.is_err() {
                if let Err(cleanup) = Self::close_session(&session).await {
                    tracing::warn!(target: "deepagent_ssh", connection_id = cfg.id, %cleanup,
                        "failed to close unhealthy SSH generation");
                }
                let mut sessions = self.sessions.write().await;
                if sessions
                    .get(&cfg.id)
                    .is_some_and(|current| Arc::ptr_eq(current, &session))
                {
                    sessions.remove(&cfg.id);
                }
                let delay = self.record_reconnect_failure(&cfg.id);
                tracing::warn!(
                    target: "deepagent_ssh",
                    connection_id = cfg.id,
                    retry_delay_ms = delay.as_millis() as u64,
                    error = outcome.as_ref().err().map(ToString::to_string).unwrap_or_default(),
                    "SSH health check failed; recovery is scheduled"
                );
            } else {
                session.touch_keepalive();
                self.clear_reconnect_backoff(&cfg.id);
            }
            session.set_status(status, error.clone()).await;
        }
        self.update_cached_status(
            &cfg.id,
            status,
            error,
            Some(started.elapsed().as_millis() as u64),
        )
        .await
    }

    async fn rebuild_health_transport(
        &self,
        cfg: &SshConnectionConfig,
        permit: &ConnectionPermit,
    ) -> SshResult<()> {
        let session = self.operation_session(cfg, permit).await?;
        let attempt_id = uuid::Uuid::new_v4().to_string();
        {
            let _lifecycle = self.lock_connection_generation(permit, &session).await?;
            session.set_connecting_attempt(&attempt_id).await;
            self.update_cached_status(&cfg.id, SshStatus::Connecting, None, None)
                .await?;
        }

        let started = std::time::Instant::now();
        let budget = Duration::from_secs(STATUS_REFRESH_TIMEOUT_SECS);
        let progress = permit
            .run(async {
                timeout(
                    budget,
                    connect_client_progress(cfg, self.host_key_store.clone(), None, None, false),
                )
                .await
                .map_err(|_| SshError::Timeout(budget))?
            })
            .await;

        let outcome = match progress {
            Ok(ConnectClientProgress::Connected {
                client,
                trust,
                server_key,
            }) => self
                .publish_connected_client(
                    cfg,
                    client,
                    trust,
                    server_key,
                    started.elapsed(),
                    &session,
                    permit,
                )
                .await
                .map(|_| ()),
            Ok(ConnectClientProgress::ConfirmationRequired { mut pending }) => {
                pending.disconnect().await;
                Err(HostTrust::error(
                    cfg,
                    "automatic SSH recovery requires explicit host-key confirmation",
                ))
            }
            Ok(ConnectClientProgress::AuthenticationRequired { mut pending, .. }) => {
                pending.disconnect().await;
                Err(SshError::AuthFailed(
                    cfg.username.clone(),
                    "automatic SSH recovery requires interactive authentication".into(),
                ))
            }
            Err(error) => Err(error),
        };

        if let Err(error) = outcome {
            let _lifecycle = self.lock_connection_generation(permit, &session).await?;
            let message = error.to_string();
            session
                .set_status(SshStatus::Error, Some(message.clone()))
                .await;
            let delay = self.record_reconnect_failure(&cfg.id);
            self.update_cached_status(
                &cfg.id,
                SshStatus::Error,
                Some(message),
                Some(started.elapsed().as_millis() as u64),
            )
            .await?;
            tracing::warn!(
                target: "deepagent_ssh",
                connection_id = cfg.id,
                retry_delay_ms = delay.as_millis() as u64,
                %error,
                "automatic SSH transport recovery failed"
            );
            return Err(error);
        }

        Ok(())
    }
    pub(super) async fn update_cached_status(
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
        let output = client.execute(command).await?;
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
    let output = client.execute(SSH_HEALTHCHECK_COMMAND).await?;
    let exit_code = i32::try_from(output.exit_status).unwrap_or(i32::MAX);
    if exit_code != 0 {
        return Err(SshError::CommandFailed {
            exit_code,
            stderr: output.stderr,
        });
    }
    Ok(())
}

type ConnectClientProgress = SshClientConnection;

async fn connect_client_progress(
    config: &SshConnectionConfig,
    store: Option<Arc<crate::SshHostKeyStore>>,
    confirmed: Option<HostTrust>,
    confirmed_jump: Option<HostTrust>,
    confirm_unknown: bool,
) -> SshResult<ConnectClientProgress> {
    validate_config(config)?;
    let trust = match confirmed {
        Some(trust) => {
            if trust.reload(config, store.clone()).await? != trust {
                return Err(HostTrust::error(
                    config,
                    "confirmed trust changed before handshake",
                ));
            }
            trust
        }
        None if config.jump.is_some()
            && (confirm_unknown
                || config.host_key_policy == super::SshHostKeyPolicy::AcceptNew) =>
        {
            HostTrust::capture(config, store.clone()).await?
        }
        None => crate::connection_flow::connection_trust(config, store.clone()).await?,
    };
    match Client::connect(config, &trust, store.clone(), confirmed_jump).await? {
        SshClientConnection::ConfirmationRequired { pending } => {
            Ok(ConnectClientProgress::ConfirmationRequired { pending })
        }
        SshClientConnection::Connected {
            client,
            trust,
            server_key,
        } => match trust.reload(config, store).await {
            Ok(current) if current == trust => Ok(ConnectClientProgress::Connected {
                client,
                trust,
                server_key,
            }),
            result => {
                let _ = client.disconnect().await;
                Err(result.err().unwrap_or_else(|| {
                    HostTrust::error(
                        config,
                        "trust source changed during authentication; retry after verification",
                    )
                }))
            }
        },
        SshClientConnection::AuthenticationRequired { mut pending, round } => {
            let pending_trust = pending.trust.clone();
            let active_config = pending.role.config(config)?;
            match pending_trust.reload(&active_config, store).await {
                Ok(current) if current == pending_trust => {
                    Ok(ConnectClientProgress::AuthenticationRequired { pending, round })
                }
                result => {
                    pending.disconnect().await;
                    Err(result.err().unwrap_or_else(|| {
                        HostTrust::error(
                            config,
                            "trust source changed during authentication; retry after verification",
                        )
                    }))
                }
            }
        }
    }
}

async fn connect_client(
    config: &SshConnectionConfig,
    store: Option<Arc<crate::SshHostKeyStore>>,
    confirmed: Option<HostTrust>,
) -> SshResult<(Client, HostTrust)> {
    match connect_client_progress(config, store, confirmed, None, false).await? {
        ConnectClientProgress::ConfirmationRequired { .. } => Err(HostTrust::error(
            config,
            "unknown target requires the connection challenge flow",
        )),
        ConnectClientProgress::Connected { client, trust, .. } => Ok((client, trust)),
        ConnectClientProgress::AuthenticationRequired { mut pending, .. } => {
            pending.disconnect().await;
            Err(SshError::AuthFailed(
                config.username.clone(),
                "server requires keyboard-interactive authentication; use the connection challenge flow".into(),
            ))
        }
    }
}

fn validate_config(config: &SshConnectionConfig) -> SshResult<()> {
    config.timing.validate()?;
    if let Some(jump) = &config.jump {
        jump.validate()?;
    }
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
    config.validate_credentials()
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
        agent_fingerprint: cfg.agent_fingerprint.clone(),
        authentication_factors: cfg
            .authentication_factors
            .iter()
            .map(|factor| factor.dto())
            .collect(),
        proxy: cfg.proxy.as_ref().map(crate::SshProxyConfig::dto),
        jump: cfg.jump.as_ref().map(crate::SshJumpConfig::dto),
        timing: cfg.timing,
        key_path: cfg.key_path.clone(),
        has_key_content: cfg.key_content.is_some(),
        certificate_path: cfg.certificate_path.clone(),
        known_hosts_path: cfg.known_hosts_path.clone(),
        host_key_policy: cfg.host_key_policy,
        status,
        last_error,
        latency_ms,
    }
}

async fn sftp_list_dir(
    connection_id: &str,
    client: &Client,
    path: &str,
) -> SshResult<SshDirListing> {
    let channel = client.get_channel().await?;
    channel
        .request_subsystem(true, "sftp")
        .await
        .map_err(|err| SshError::Internal(err.to_string()))?;
    let sftp = SftpSession::new_opts(channel.into_stream(), None)
        .await
        .map_err(|err| SshError::Internal(err.to_string()))?;

    let listing = async {
        let canonical_path = sftp
            .canonicalize(path)
            .await
            .map_err(|err| SshError::Internal(err.to_string()))?;
        let mut entries = Vec::new();
        for entry in sftp
            .read_dir(canonical_path.as_str())
            .await
            .map_err(|err| SshError::Internal(err.to_string()))?
        {
            let name = entry.file_name();
            let file_type = entry.file_type();
            let metadata = entry.metadata();
            entries.push(SshDirEntry {
                path: join_remote_path(&canonical_path, &name),
                name,
                is_dir: file_type.is_dir(),
                is_symlink: file_type.is_symlink(),
                size: metadata.size,
                modified_ms: metadata.mtime.map(|value| u64::from(value) * 1000),
            });
        }
        entries.sort_by(|a, b| {
            b.is_dir
                .cmp(&a.is_dir)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        Ok::<SshDirListing, SshError>(SshDirListing {
            connection_id: connection_id.to_owned(),
            path: path.to_owned(),
            canonical_path,
            entries,
        })
    }
    .await;

    if let Err(close_err) = sftp.close().await {
        tracing::warn!(
            target: "deepagent_ssh",
            connection_id,
            error = %close_err,
            "sftp session close failed"
        );
    }
    listing
}

const SFTP_READ_FILE_MAX_BYTES: u64 = 1024 * 1024;
const SFTP_READ_FILE_BINARY_MAX_BYTES: u64 = 10 * 1024 * 1024;

/// SFTP 读取的公共字节通道：文本与二进制预览共用，仅上限不同。
/// 返回 (bytes, 远端文件大小, 是否被上限截断)。
async fn sftp_read_file_bytes(
    client: &Client,
    path: &str,
    max_bytes: u64,
) -> SshResult<(Vec<u8>, Option<u64>, bool)> {
    let channel = client.get_channel().await?;
    channel
        .request_subsystem(true, "sftp")
        .await
        .map_err(|err| SshError::Internal(err.to_string()))?;
    let sftp = SftpSession::new_opts(channel.into_stream(), None)
        .await
        .map_err(|err| SshError::Internal(err.to_string()))?;

    let content = async {
        let mut file = sftp
            .open(path)
            .await
            .map_err(|err| SshError::Internal(err.to_string()))?;
        let size = file.metadata().await.ok().and_then(|meta| meta.size);
        let mut reader = (&mut file).take(max_bytes + 1);
        let mut bytes = Vec::new();
        reader
            .read_to_end(&mut bytes)
            .await
            .map_err(|err| SshError::Internal(err.to_string()))?;
        drop(reader);
        let truncated = bytes.len() as u64 > max_bytes;
        bytes.truncate(max_bytes as usize);
        file.shutdown()
            .await
            .map_err(|err| SshError::Internal(err.to_string()))?;
        Ok::<(Vec<u8>, Option<u64>, bool), SshError>((bytes, size, truncated))
    }
    .await;

    if let Err(close_err) = sftp.close().await {
        tracing::warn!(
            target: "deepagent_ssh",
            error = %close_err,
            "sftp session close failed"
        );
    }
    content
}

async fn sftp_read_file(
    connection_id: &str,
    client: &Client,
    path: &str,
) -> SshResult<SshFileContent> {
    let (bytes, size, truncated) =
        sftp_read_file_bytes(client, path, SFTP_READ_FILE_MAX_BYTES).await?;
    // NUL 字节几乎只出现在二进制格式；UTF-8/GBK 等文本不会在头部出现。
    let probe_len = bytes.len().min(8 * 1024);
    let is_binary = bytes[..probe_len].contains(&0);
    Ok(SshFileContent {
        connection_id: connection_id.to_owned(),
        path: path.to_owned(),
        size,
        truncated,
        is_binary,
        content: String::from_utf8_lossy(&bytes).into_owned(),
    })
}

async fn sftp_read_file_binary(
    connection_id: &str,
    client: &Client,
    path: &str,
) -> SshResult<SshFileBinary> {
    let (bytes, size, truncated) =
        sftp_read_file_bytes(client, path, SFTP_READ_FILE_BINARY_MAX_BYTES).await?;
    Ok(SshFileBinary {
        connection_id: connection_id.to_owned(),
        path: path.to_owned(),
        size,
        truncated,
        data_base64: base64::engine::general_purpose::STANDARD.encode(&bytes),
    })
}

/// 打开一次性 SFTP 会话（subsystem 握手），操作结束后必须配对 sftp_close_quiet。
async fn sftp_open(client: &Client, timeout_seconds: Option<u64>) -> SshResult<SftpSession> {
    let channel = client.get_channel().await?;
    channel
        .request_subsystem(true, "sftp")
        .await
        .map_err(|err| SshError::Internal(err.to_string()))?;
    SftpSession::new_opts(
        crate::sftp_packet_stream::packet_stream(
            channel.into_stream(),
            crate::sftp_packet_stream::MAX_RECEIVE_PACKET_BYTES,
        ),
        timeout_seconds,
    )
    .await
    .map_err(|err| SshError::Internal(err.to_string()))
}

async fn sftp_close_quiet(connection_id: &str, sftp: &SftpSession) {
    if let Err(close_err) = sftp.close().await {
        tracing::warn!(
            target: "deepagent_ssh",
            connection_id,
            error = %close_err,
            "sftp session close failed"
        );
    }
}

async fn sftp_upload_file(
    connection_id: &str,
    client: &Client,
    local_path: &str,
    remote_path: &str,
) -> SshResult<()> {
    let sftp = sftp_open(client, Some(600)).await?;
    let result = async {
        let local = tokio::fs::File::open(local_path).await?;
        let mut reader = tokio::io::BufReader::with_capacity(64 * 1024, local);
        let mut remote = sftp
            .open_with_flags(
                remote_path,
                OpenFlags::CREATE | OpenFlags::TRUNCATE | OpenFlags::WRITE | OpenFlags::READ,
            )
            .await
            .map_err(|error| SshError::Internal(format!("open SFTP upload: {error}")))?;
        tokio::io::copy_buf(&mut reader, &mut remote).await?;
        remote.shutdown().await?;
        Ok(())
    }
    .await;
    sftp_close_quiet(connection_id, &sftp).await;
    result
}

async fn sftp_create_dir(connection_id: &str, client: &Client, path: &str) -> SshResult<()> {
    let sftp = sftp_open(client, None).await?;
    let result = sftp
        .create_dir(path)
        .await
        .map_err(|err| SshError::Internal(err.to_string()));
    sftp_close_quiet(connection_id, &sftp).await;
    result
}

/// 新建空文件：CREATE|WRITE|EXCLUDE，同名文件已存在时失败，不覆盖。
async fn sftp_create_file(connection_id: &str, client: &Client, path: &str) -> SshResult<()> {
    let sftp = sftp_open(client, None).await?;
    let result = async {
        let flags = OpenFlags::CREATE | OpenFlags::WRITE | OpenFlags::EXCLUDE;
        sftp.open_with_flags(path, flags)
            .await
            .map_err(|err| SshError::Internal(err.to_string()))?
            .shutdown()
            .await
            .map_err(|err| SshError::Internal(err.to_string()))
    }
    .await;
    sftp_close_quiet(connection_id, &sftp).await;
    result
}

async fn sftp_rename_path(
    connection_id: &str,
    client: &Client,
    old_path: &str,
    new_path: &str,
) -> SshResult<()> {
    let sftp = sftp_open(client, None).await?;
    let result = sftp
        .rename(old_path, new_path)
        .await
        .map_err(|err| SshError::Internal(err.to_string()));
    sftp_close_quiet(connection_id, &sftp).await;
    result
}

/// 递归删除目录条目；symlink 一律只删链接本身，不跟随。
async fn sftp_remove_entry(
    sftp: &SftpSession,
    path: &str,
    is_dir: bool,
    is_symlink: bool,
) -> SshResult<()> {
    if is_dir && !is_symlink {
        for entry in sftp
            .read_dir(path)
            .await
            .map_err(|err| SshError::Internal(err.to_string()))?
        {
            let file_type = entry.file_type();
            let child = join_remote_path(path, &entry.file_name());
            Box::pin(sftp_remove_entry(
                sftp,
                &child,
                file_type.is_dir(),
                file_type.is_symlink(),
            ))
            .await?;
        }
        sftp.remove_dir(path)
            .await
            .map_err(|err| SshError::Internal(err.to_string()))
    } else {
        sftp.remove_file(path)
            .await
            .map_err(|err| SshError::Internal(err.to_string()))
    }
}

/// 删除文件/符号链接/目录（目录递归删除）。顶层类型以 symlink_metadata 为准。
async fn sftp_remove_path(connection_id: &str, client: &Client, path: &str) -> SshResult<()> {
    let sftp = sftp_open(client, None).await?;
    let result = async {
        let meta = sftp
            .symlink_metadata(path)
            .await
            .map_err(|err| SshError::Internal(err.to_string()))?;
        let file_type = meta.file_type();
        sftp_remove_entry(&sftp, path, file_type.is_dir(), file_type.is_symlink()).await
    }
    .await;
    sftp_close_quiet(connection_id, &sftp).await;
    result
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

#[cfg(test)]
mod reconnect_backoff_tests {
    use super::ReconnectBackoff;
    use tokio::time::{Duration, Instant};

    #[test]
    fn reconnect_backoff_grows_exponentially_and_is_capped() {
        let start = Instant::now();
        let mut backoff = ReconnectBackoff::default();

        assert_eq!(backoff.record_failure(start), Duration::from_secs(5));
        assert_eq!(backoff.record_failure(start), Duration::from_secs(10));
        assert_eq!(backoff.record_failure(start), Duration::from_secs(20));
        assert_eq!(backoff.record_failure(start), Duration::from_secs(40));
        assert_eq!(backoff.record_failure(start), Duration::from_secs(80));
        assert_eq!(backoff.record_failure(start), Duration::from_secs(160));
        assert_eq!(backoff.record_failure(start), Duration::from_secs(300));
        assert_eq!(backoff.record_failure(start), Duration::from_secs(300));
    }

    #[test]
    fn reconnect_backoff_becomes_due_at_deadline_and_resets_on_success() {
        let start = Instant::now();
        let mut backoff = ReconnectBackoff::default();

        backoff.record_failure(start);
        assert!(!backoff.is_due(start + Duration::from_secs(4)));
        assert!(backoff.is_due(start + Duration::from_secs(5)));

        backoff = ReconnectBackoff::default();
        assert!(backoff.is_due(start));
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
    let output = client.execute(&cmd).await?;
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
    let path = shell_escape(remote_path);
    let cmd = format!(
        "if command -v sha256sum >/dev/null 2>&1; then sha256sum {path} | awk '{{print $1}}'; \
elif command -v shasum >/dev/null 2>&1; then shasum -a 256 {path} | awk '{{print $1}}'; \
elif command -v openssl >/dev/null 2>&1; then openssl dgst -sha256 {path} | awk '{{print $NF}}'; \
elif command -v python3 >/dev/null 2>&1; then python3 - {path} <<'PY'\nimport hashlib, sys\nh = hashlib.sha256()\nwith open(sys.argv[1], 'rb') as fh:\n    while True:\n        chunk = fh.read(1024 * 1024)\n        if not chunk:\n            break\n        h.update(chunk)\nprint(h.hexdigest())\nPY\nelse echo missing-sha256-tool >&2; exit 127; fi"
    );
    let output = client.execute(&cmd).await?;
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
    if base.ends_with('/') {
        format!("{base}{name}")
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
