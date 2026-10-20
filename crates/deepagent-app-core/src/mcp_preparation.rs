//! Host-owned MCP startup. Request readers never join startup jobs.
use super::*;
use futures::stream::{FuturesUnordered, StreamExt};
use notify::{Config, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, RwLock};
use std::time::Duration;
use tokio::sync::Semaphore;

type ServerSlot = Arc<AsyncMutex<Option<CachedRegistry>>>;

pub(super) struct McpPreparation {
    slots: Mutex<BTreeMap<String, ServerSlot>>,
    owners: Mutex<BTreeMap<String, BTreeSet<String>>>,
    ready: RwLock<Arc<McpRegistry>>,
    ready_fingerprints: RwLock<BTreeMap<String, String>>,
    active_fingerprint: Mutex<Option<String>>,
    generation: AtomicU64,
    cancelled: Mutex<deepagent_runtime::CancellationTree>,
    local: Semaphore,
    remote: Semaphore,
    payload_verified: std::sync::atomic::AtomicBool,
    payload_change_generation: AtomicU64,
    payload_lock: AsyncMutex<()>,
    watcher: Mutex<Option<RecommendedWatcher>>,
    #[cfg(test)]
    startup_attempts: std::sync::atomic::AtomicUsize,
}

impl Default for McpPreparation {
    fn default() -> Self {
        Self {
            slots: Mutex::new(BTreeMap::new()),
            owners: Mutex::new(BTreeMap::new()),
            ready: RwLock::new(Arc::new(McpRegistry::new())),
            ready_fingerprints: RwLock::new(BTreeMap::new()),
            active_fingerprint: Mutex::new(None),
            generation: AtomicU64::new(0),
            cancelled: Mutex::new(Default::default()),
            local: Semaphore::new(2),
            remote: Semaphore::new(4),
            payload_verified: std::sync::atomic::AtomicBool::new(false),
            payload_change_generation: AtomicU64::new(0),
            payload_lock: AsyncMutex::new(()),
            watcher: Mutex::new(None),
            #[cfg(test)]
            startup_attempts: std::sync::atomic::AtomicUsize::new(0),
        }
    }
}

impl McpService {
    async fn refresh_builtin_payload_after_change(self: &Arc<Self>) {
        let user_names = match self.load_state() {
            Ok(state) => state
                .config
                .servers
                .keys()
                .cloned()
                .collect::<BTreeSet<_>>(),
            Err(error) => {
                tracing::error!(%error, "could not reload MCP config after bundled payload change");
                return;
            }
        };
        let builtin_names = match self.builtin_config() {
            Ok(config) => config
                .servers
                .keys()
                .filter(|name| !user_names.contains(*name))
                .cloned()
                .collect::<BTreeSet<_>>(),
            Err(error) => {
                tracing::error!(%error, "could not reload bundled MCP config after payload change");
                return;
            }
        };

        for name in &builtin_names {
            if let Err(error) = self.retire_server(name).await {
                tracing::warn!(%error, server = name, "could not retire MCP server after bundled payload change");
            }
        }
        let mut ready = (*self.ready_registry()).clone();
        ready.retain_servers(|name| !builtin_names.contains(name));
        self.preparation
            .ready_fingerprints
            .write()
            .unwrap_or_else(|p| p.into_inner())
            .retain(|name, _| !builtin_names.contains(name));
        *self
            .preparation
            .ready
            .write()
            .unwrap_or_else(|p| p.into_inner()) = Arc::new(ready);
        *self
            .preparation
            .active_fingerprint
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = None;
        self.start_preparation();
    }

    fn schedule_builtin_payload_refresh(self: &Arc<Self>, runtime: Option<tokio::runtime::Handle>) {
        self.preparation
            .payload_verified
            .store(false, Ordering::Release);
        let generation = self
            .preparation
            .payload_change_generation
            .fetch_add(1, Ordering::AcqRel)
            + 1;
        let Some(runtime) = runtime else {
            *self
                .preparation
                .active_fingerprint
                .lock()
                .unwrap_or_else(|p| p.into_inner()) = None;
            return;
        };
        let weak = Arc::downgrade(self);
        runtime.spawn(async move {
            tokio::time::sleep(Duration::from_millis(250)).await;
            let Some(service) = weak.upgrade() else {
                return;
            };
            if service
                .preparation
                .payload_change_generation
                .load(Ordering::Acquire)
                == generation
            {
                service.refresh_builtin_payload_after_change().await;
            }
        });
    }

    pub fn ready_registry(&self) -> Arc<McpRegistry> {
        self.preparation
            .ready
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    pub(super) fn cached_status(&self, name: &str) -> McpConnectionStatusDto {
        let slot = self
            .preparation
            .slots
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(name)
            .cloned();
        let mut result = McpConnectionStatusDto {
            name: name.to_owned(),
            status: "preparing".into(),
            error: None,
            tools: Vec::new(),
        };
        if let Some(slot) = slot {
            if let Ok(guard) = slot.try_lock() {
                if let Some(cached) = guard.as_ref() {
                    if let Some((_, error)) = cached.failures.first() {
                        result.status = "failed".into();
                        result.error = Some(error.clone());
                    } else {
                        result.status = "connected".into();
                        result.tools = cached
                            .registry
                            .all_tools()
                            .into_iter()
                            .map(|tool| McpToolInfoDto {
                                name: tool.local_name,
                                description: tool.description,
                            })
                            .collect();
                    }
                }
            }
        }
        result
    }

    pub(crate) async fn retire_server(&self, name: &str) -> Result<()> {
        self.preparation
            .owners
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .values_mut()
            .for_each(|names| {
                names.remove(name);
            });
        let slot = self
            .preparation
            .slots
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(name);
        if let Some(slot) = slot {
            // Drop this owner. Active request snapshots keep their client Arc
            // alive until they finish, so retiring a config cannot interrupt
            // an in-flight tool call.
            drop(slot.lock().await.take());
        }
        Ok(())
    }

    pub(crate) async fn reconcile_owner(&self, owner: &str, names: BTreeSet<String>) -> Result<()> {
        let retire = {
            let mut owners = self
                .preparation
                .owners
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            let previous = owners.insert(owner.to_owned(), names).unwrap_or_default();
            previous
                .into_iter()
                .filter(|name| !owners.values().any(|set| set.contains(name)))
                .collect::<Vec<_>>()
        };
        for name in retire {
            self.retire_server(&name).await?;
        }
        Ok(())
    }

    /// Initialize one identity at most once; simultaneous host callers share
    /// its slot. Neither the snapshot lock nor the slot map spans an await.
    pub(crate) async fn prepare_server_with_cancel(
        &self,
        name: &str,
        cfg: McpServerConfig,
        cancellation: Option<deepagent_runtime::CancellationTree>,
        retry_failed: bool,
    ) -> Result<Arc<McpRegistry>> {
        let mut config = McpConfig {
            servers: BTreeMap::from([(name.to_owned(), cfg)]),
        };
        self.prepare_runtime_config(&mut config);
        let cfg = config
            .servers
            .remove(name)
            .ok_or_else(|| CoreError::invalid("missing MCP config"))?;
        let fingerprint = serde_json::to_string(&cfg)?;
        let slot = self
            .preparation
            .slots
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .entry(name.to_owned())
            .or_insert_with(|| Arc::new(AsyncMutex::new(None)))
            .clone();
        let mut slot = slot.lock().await;
        if let Some(cached) = slot.as_ref() {
            if cached.fingerprint == fingerprint {
                if let Some((_, error)) = cached.failures.first() {
                    if !retry_failed {
                        return Err(CoreError::other(error.clone()));
                    }
                } else {
                    return Ok(cached.registry.clone());
                }
            }
            // Requests may still hold clients from this registry. Dropping the
            // cache owner lets those requests finish before transport teardown.
            drop(slot.take());
        }
        let semaphore = if matches!(cfg.effective_type(), Ok(TransportType::Stdio)) {
            &self.preparation.local
        } else {
            &self.preparation.remote
        };
        let _permit = if let Some(cancel) = &cancellation {
            tokio::select! {
                _ = cancel.cancelled() => return Err(CoreError::other("MCP preparation cancelled")),
                result = semaphore.acquire() => result.map_err(|e| CoreError::other(e.to_string()))?,
            }
        } else {
            semaphore
                .acquire()
                .await
                .map_err(|e| CoreError::other(e.to_string()))?
        };
        #[cfg(test)]
        self.preparation
            .startup_attempts
            .fetch_add(1, Ordering::AcqRel);
        let connect = async {
            let (client, instructions) = Self::connect_one_resilient(name, &cfg).await?;
            let mut registry = McpRegistry::new();
            if let Err(error) = registry
                .register_with_instructions(name, client.clone(), instructions)
                .await
            {
                let _ = client.close().await;
                return Err(error);
            }
            registry.enable_liveness(deepagent_mcp::LivenessConfig::default());
            Ok::<_, CoreError>(Arc::new(registry))
        };
        let result = if let Some(cancel) = &cancellation {
            tokio::select! {
                _ = cancel.cancelled() => Err(CoreError::other("MCP preparation cancelled")),
                result = tokio::time::timeout(Duration::from_secs(30), connect) =>
                    result.unwrap_or_else(|_| Err(CoreError::other("MCP startup timed out after 30 seconds"))),
            }
        } else {
            tokio::time::timeout(Duration::from_secs(30), connect)
                .await
                .unwrap_or_else(|_| Err(CoreError::other("MCP startup timed out after 30 seconds")))
        };
        let (registry, failures) = match &result {
            Ok(registry) => (registry.clone(), Vec::new()),
            Err(error) => (
                Arc::new(McpRegistry::new()),
                vec![(name.to_owned(), error.to_string())],
            ),
        };
        *slot = Some(CachedRegistry {
            fingerprint,
            registry,
            failures,
        });
        result
    }

    async fn ensure_builtin_payload(
        &self,
        cancel: &deepagent_runtime::CancellationTree,
    ) -> Result<()> {
        let (Some(root), Some(data)) = (&self.builtin_mcp_root, &self.builtin_mcp_data) else {
            return Ok(());
        };
        if self.preparation.payload_verified.load(Ordering::Acquire) {
            return Ok(());
        }
        let _guard = tokio::select! {
            _ = cancel.cancelled() => return Err(CoreError::other("MCP preparation cancelled")),
            guard = self.preparation.payload_lock.lock() => guard,
        };
        if self.preparation.payload_verified.load(Ordering::Acquire) {
            return Ok(());
        }
        let root = root.clone();
        let data = data.clone();
        let cancellation = cancel.clone();
        tokio::task::spawn_blocking(move || {
            crate::runtime_payload::prepare_runtime_payload_with_cancel(&root, &data, &cancellation)
        })
        .await
        .map_err(|error| CoreError::other(error.to_string()))??;
        self.preparation
            .payload_verified
            .store(true, Ordering::Release);
        Ok(())
    }

    fn watch_builtin_payload(self: &Arc<Self>) {
        if self
            .preparation
            .watcher
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .is_some()
        {
            return;
        }
        let Some(root) = self.builtin_mcp_root.clone() else {
            return;
        };
        let data = self.builtin_mcp_data.clone();
        let watched_root = root.clone();
        let watched_data = data.clone();
        let weak = Arc::downgrade(self);
        let runtime = tokio::runtime::Handle::try_current().ok();
        let watcher = RecommendedWatcher::new(
            move |event: notify::Result<notify::Event>| {
                let Some(service) = weak.upgrade() else {
                    return;
                };
                match event {
                    Ok(event)
                        if matches!(
                            event.kind,
                            EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
                        ) =>
                    {
                        let touches_payload = event.paths.iter().any(|path| {
                            path.starts_with(&watched_root)
                                || watched_data
                                    .as_ref()
                                    .is_some_and(|data| path.starts_with(data))
                        });
                        if touches_payload {
                            service.schedule_builtin_payload_refresh(runtime.clone());
                        }
                    }
                    Err(error) => {
                        tracing::error!(%error, "built-in MCP payload watcher failed; next host preparation will reverify");
                        service.schedule_builtin_payload_refresh(runtime.clone());
                    }
                    _ => {}
                }
            },
            Config::default(),
        );
        match watcher {
            Ok(mut watcher) => {
                let mut watched = watcher.watch(&root, RecursiveMode::Recursive).is_ok();
                if let Some(data) = data {
                    watched |= watcher.watch(&data, RecursiveMode::Recursive).is_ok();
                }
                if watched {
                    *self
                        .preparation
                        .watcher
                        .lock()
                        .unwrap_or_else(|p| p.into_inner()) = Some(watcher);
                } else {
                    tracing::warn!(
                        "could not watch built-in MCP payload; will reverify on preparation"
                    );
                }
            }
            Err(error) => {
                tracing::warn!(%error, "could not watch built-in MCP payload; will reverify on preparation")
            }
        }
    }

    /// Start configured user servers independently of the bundled payload.
    /// Slow bundle verification cannot delay other servers or conversation.
    pub fn start_preparation(self: &Arc<Self>) {
        // Resolve and fingerprint the effective host config before scheduling
        // work. Repeated callers with the same environment/config share the
        // current preparation instead of cancelling and restarting it.
        let state = match self.load_state() {
            Ok(state) => state,
            Err(error) => {
                tracing::error!(%error, "MCP configuration preparation failed");
                return;
            }
        };
        let mut config = McpConfig {
            servers: state
                .config
                .servers
                .into_iter()
                .filter(|(name, _)| state.enabled.get(name).copied().unwrap_or(true))
                .collect(),
        };
        let user_names: BTreeSet<_> = config.servers.keys().cloned().collect();
        let builtin = match self.builtin_config() {
            Ok(config) => config,
            Err(error) => {
                tracing::warn!(%error, "built-in MCP configuration unavailable; user MCP servers continue");
                McpConfig {
                    servers: BTreeMap::new(),
                }
            }
        };
        for (name, server) in builtin.servers {
            if config.servers.contains_key(&name) {
                tracing::warn!(
                    server = name.as_str(),
                    "skipping built-in MCP because a user MCP server has the same name"
                );
                continue;
            }
            config.servers.insert(name, server);
        }
        self.prepare_runtime_config(&mut config);
        let config_fingerprint = match serde_json::to_string(&config) {
            Ok(fingerprint) => fingerprint,
            Err(error) => {
                tracing::error!(%error, "could not fingerprint MCP configuration");
                return;
            }
        };
        {
            let mut active = self
                .preparation
                .active_fingerprint
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            if active.as_deref() == Some(config_fingerprint.as_str()) {
                return;
            }
            *active = Some(config_fingerprint);
        }

        let cancel = deepagent_runtime::CancellationTree::new();
        {
            let mut active_cancel = self
                .preparation
                .cancelled
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            active_cancel.cancel();
            *active_cancel = cancel.clone();
        }
        let generation = self.preparation.generation.fetch_add(1, Ordering::AcqRel) + 1;

        let desired_fingerprints = config
            .servers
            .iter()
            .map(|(name, config)| {
                (
                    name.clone(),
                    serde_json::to_string(config).unwrap_or_default(),
                )
            })
            .collect::<BTreeMap<_, _>>();
        let mut ready = (*self.ready_registry()).clone();
        let mut ready_fingerprints = self
            .preparation
            .ready_fingerprints
            .write()
            .unwrap_or_else(|p| p.into_inner());
        ready.retain_servers(|name| {
            desired_fingerprints
                .get(name)
                .is_some_and(|fingerprint| ready_fingerprints.get(name) == Some(fingerprint))
        });
        ready_fingerprints
            .retain(|name, fingerprint| desired_fingerprints.get(name) == Some(fingerprint));
        *self
            .preparation
            .ready
            .write()
            .unwrap_or_else(|p| p.into_inner()) = Arc::new(ready.clone());
        drop(ready_fingerprints);

        let service = self.clone();
        service.watch_builtin_payload();
        tokio::spawn(async move {
            let desired = config.servers.keys().cloned().collect::<BTreeSet<_>>();
            if let Err(error) = service.reconcile_owner("host", desired).await {
                tracing::warn!(%error, "could not retire stale host MCP servers");
            }
            let mut jobs = FuturesUnordered::new();
            for (name, cfg) in config.servers {
                let worker = service.clone();
                let cancelled = cancel.clone();
                let prepare_payload =
                    !user_names.contains(&name) && worker.builtin_mcp_root.is_some();
                let fingerprint = serde_json::to_string(&cfg).unwrap_or_default();
                jobs.push(async move {
                    if prepare_payload {
                        worker.ensure_builtin_payload(&cancelled).await?;
                    }
                    worker
                        .prepare_server_with_cancel(&name, cfg, Some(cancelled.clone()), false)
                        .await
                        .map(|registry| (name, fingerprint, registry))
                });
            }
            while let Some(result) = jobs.next().await {
                if service.preparation.generation.load(Ordering::Acquire) != generation {
                    return;
                }
                match result {
                    Ok((name, fingerprint, registry)) => {
                        if let Err(error) = ready.extend_ready(&registry) {
                            tracing::error!(%error, "MCP snapshot conflict");
                            continue;
                        }
                        service
                            .preparation
                            .ready_fingerprints
                            .write()
                            .unwrap_or_else(|p| p.into_inner())
                            .insert(name, fingerprint);
                        *service
                            .preparation
                            .ready
                            .write()
                            .unwrap_or_else(|p| p.into_inner()) = Arc::new(ready.clone());
                    }
                    Err(error) => {
                        tracing::warn!(%error, "MCP preparation failed; excluded from requests")
                    }
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use deepagent_persistence::Database;

    fn missing_stdio_config() -> McpServerConfig {
        McpServerConfig {
            transport: None,
            command: Some("deepagent-missing-mcp-test-binary".into()),
            args: Vec::new(),
            env: BTreeMap::new(),
            cwd: None,
            url: None,
            headers: BTreeMap::new(),
        }
    }

    #[tokio::test]
    async fn concurrent_preparation_shares_a_server_startup() {
        let service = McpService::new(Arc::new(Database::open_in_memory().unwrap()));
        let config = missing_stdio_config();
        let (first, second) = tokio::join!(
            service.prepare_server_with_cancel("ghost", config.clone(), None, false),
            service.prepare_server_with_cancel("ghost", config.clone(), None, false),
        );

        assert!(first.is_err());
        assert!(second.is_err());
        assert_eq!(
            service.preparation.startup_attempts.load(Ordering::Acquire),
            1,
            "concurrent callers must share one failed startup result"
        );

        // A caller that explicitly requests retry is allowed to start again.
        assert!(service
            .prepare_server_with_cancel("ghost", config, None, true)
            .await
            .is_err());
        assert_eq!(
            service.preparation.startup_attempts.load(Ordering::Acquire),
            2
        );
    }

    #[tokio::test]
    async fn repeated_host_preparation_with_same_config_is_a_noop() {
        let service = Arc::new(McpService::new(Arc::new(
            Database::open_in_memory().unwrap(),
        )));

        service.start_preparation();
        let generation = service.preparation.generation.load(Ordering::Acquire);
        service.start_preparation();

        assert_eq!(generation, 1);
        assert_eq!(
            service.preparation.generation.load(Ordering::Acquire),
            generation,
            "same config must keep the in-progress or published preparation"
        );
    }
}
