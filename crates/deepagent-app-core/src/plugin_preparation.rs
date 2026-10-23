//! Plugin readiness belongs to the host, not to a model turn.
use super::*;
use notify::{Config, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;

type ReadySnapshot = (PluginRuntimeProjection, Arc<deepagent_mcp::McpRegistry>);

pub(super) struct PluginPreparation {
    started: AtomicBool,
    revision: AtomicU64,
    ready: RwLock<ReadySnapshot>,
    states: Mutex<BTreeMap<String, (PluginLifecycleState, PluginHealthStatus, Option<String>)>>,
    cancel: Mutex<deepagent_runtime::CancellationTree>,
    verified: Mutex<BTreeMap<String, (Vec<PathSnapshot>, PluginRuntimeProjection)>>,
    watcher: Mutex<Option<RecommendedWatcher>>,
    mcp_owners: Mutex<BTreeSet<String>>,
}

impl Default for PluginPreparation {
    fn default() -> Self {
        Self {
            started: AtomicBool::new(false),
            revision: AtomicU64::new(1),
            ready: RwLock::new((
                PluginRuntimeProjection::default(),
                Arc::new(deepagent_mcp::McpRegistry::new()),
            )),
            states: Mutex::new(BTreeMap::new()),
            cancel: Mutex::new(Default::default()),
            verified: Mutex::new(BTreeMap::new()),
            watcher: Mutex::new(None),
            mcp_owners: Mutex::new(BTreeSet::new()),
        }
    }
}

impl PluginPreparation {
    pub(super) fn is_started(&self) -> bool {
        self.started.load(Ordering::Acquire)
    }
    pub(super) fn invalidate(&self) {
        self.cancel
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .cancel();
        self.revision.fetch_add(1, Ordering::AcqRel);
        *self.ready.write().unwrap_or_else(|p| p.into_inner()) = (
            PluginRuntimeProjection::default(),
            Arc::new(deepagent_mcp::McpRegistry::new()),
        );
        self.states
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clear();
    }
}

impl PluginService {
    /// One immutable generation contains both advertised capabilities and routes.
    pub fn ready_snapshot(&self) -> ReadySnapshot {
        self.preparation
            .ready
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    pub(super) fn preparation_status(
        &self,
        id: &str,
    ) -> Option<(PluginLifecycleState, PluginHealthStatus, Option<String>)> {
        self.preparation
            .states
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(id)
            .cloned()
    }

    /// Starts once per host. Filesystem observations and retries never run on
    /// the request path. Weak ownership lets shutdown stop the observer.
    pub fn start_preparation(
        self: &Arc<Self>,
        mcp: Arc<crate::McpService>,
        changed: Arc<dyn Fn() + Send + Sync>,
    ) {
        if self.preparation.started.swap(true, Ordering::AcqRel) {
            return;
        }
        self.start_runtime_watcher();
        let weak = Arc::downgrade(self);
        tokio::spawn(async move {
            let mut observed_revision = 0;
            let mut observed_paths: Vec<PathSnapshot> = Vec::new();
            loop {
                let Some(service) = weak.upgrade() else {
                    break;
                };
                let revision = service.preparation.revision.load(Ordering::Acquire);
                let reader = service.clone();
                let paths = observed_paths.clone();
                let unchanged = tokio::task::spawn_blocking(move || {
                    paths.iter().all(PathSnapshot::still_matches)
                })
                .await
                .unwrap_or(false);
                if revision != observed_revision || !unchanged {
                    if !unchanged {
                        service.preparation.invalidate();
                    }
                    let revision = service.preparation.revision.load(Ordering::Acquire);
                    let cancel = deepagent_runtime::CancellationTree::new();
                    *service
                        .preparation
                        .cancel
                        .lock()
                        .unwrap_or_else(|p| p.into_inner()) = cancel.clone();
                    let discovery = tokio::task::spawn_blocking(move || {
                        let state = reader.load_state()?;
                        let roots = reader.effective_roots();
                        let loaded = load_plugins(&roots);
                        let dependencies = reader.dependency_outcome(&loaded, &state);
                        let paths = runtime_watch_paths(&roots, &reader.state_path, &loaded)
                            .into_iter()
                            .map(PathSnapshot::capture)
                            .collect();
                        let enabled = loaded
                            .into_iter()
                            .filter(|plugin| {
                                reader.is_effectively_enabled(plugin, &state)
                                    && !dependencies.demoted.contains(&plugin.id)
                            })
                            .collect::<Vec<_>>();
                        Ok::<_, CoreError>((enabled, paths))
                    })
                    .await;
                    match discovery {
                        Ok(Ok((plugins, paths))) => {
                            observed_paths = paths;
                            observed_revision = revision;
                            let current_owners = plugins
                                .iter()
                                .map(|plugin| format!("plugin:{}", plugin.id))
                                .collect::<BTreeSet<_>>();
                            let stale_owners = {
                                let mut owners = service
                                    .preparation
                                    .mcp_owners
                                    .lock()
                                    .unwrap_or_else(|p| p.into_inner());
                                let stale = owners
                                    .difference(&current_owners)
                                    .cloned()
                                    .collect::<Vec<_>>();
                                *owners = current_owners;
                                stale
                            };
                            for owner in stale_owners {
                                if let Err(error) =
                                    mcp.reconcile_owner(&owner, BTreeSet::new()).await
                                {
                                    tracing::warn!(%error, owner, "failed to retire plugin MCP connections");
                                }
                            }
                            let mut ready_plugins: Vec<LoadedPlugin> = Vec::new();
                            let mut ready_mcp = deepagent_mcp::McpRegistry::new();
                            use futures::stream::{FuturesUnordered, StreamExt};
                            let mut jobs = FuturesUnordered::new();
                            for plugin in plugins {
                                service.set_preparation_state(
                                    &plugin.id,
                                    PluginLifecycleState::Preparing,
                                    PluginHealthStatus::Unknown,
                                    None,
                                );
                                let worker = service.clone();
                                let mcp = mcp.clone();
                                let cancel = cancel.clone();
                                jobs.push(async move {
                                    let candidate = plugin.clone();
                                    let cancelled = cancel.clone();
                                    let prepared = tokio::task::spawn_blocking(move || worker.prepare_candidate(&candidate, &cancelled)).await;
                                    let result = match prepared {
                                        Ok(Ok(projection)) => {
                                            let mut registry = deepagent_mcp::McpRegistry::new();
                                            let mut error = None;
                                            let owner = format!("plugin:{}", plugin.id);
                                            let mut owned_names = BTreeSet::new();
                                            for (name, config) in projection.mcp_config.servers {
                                                owned_names.insert(name.clone());
                                                let connected = tokio::select! {
                                                    biased;
                                                    _ = cancel.cancelled() => Err(CoreError::other("plugin preparation cancelled")),
                                                    result = mcp.prepare_server_with_cancel(&name, config, Some(cancel.clone()), true) => result,
                                                };
                                                match connected {
                                                    Ok(server) => {
                                                        if let Err(e) = registry.extend_ready(&server) { error = Some(e); break; }
                                                    }
                                                    Err(e) => { error = Some(e); break; }
                                                }
                                            }
                                            if let Some(error) = error {
                                                let _ = mcp.reconcile_owner(&owner, BTreeSet::new()).await;
                                                Err(error)
                                            } else {
                                                mcp.reconcile_owner(&owner, owned_names).await.map(|_| registry)
                                            }
                                        }
                                        Ok(Err(error)) => Err(error),
                                        Err(error) => Err(CoreError::other(error.to_string())),
                                    };
                                    if result.is_err() {
                                        let owner = format!("plugin:{}", plugin.id);
                                        let _ = mcp.reconcile_owner(&owner, BTreeSet::new()).await;
                                    }
                                    (plugin, result)
                                });
                            }
                            changed();
                            while let Some((plugin, result)) = jobs.next().await {
                                if revision != service.preparation.revision.load(Ordering::Acquire)
                                {
                                    cancel.cancel();
                                    continue;
                                }
                                match result {
                                    Ok(registry) => {
                                        if let Err(error) = ready_mcp.extend_ready(&registry) {
                                            service.set_preparation_state(
                                                &plugin.id,
                                                PluginLifecycleState::Failed,
                                                PluginHealthStatus::ConnectionUnavailable,
                                                Some(error.to_string()),
                                            );
                                        } else {
                                            service.set_preparation_state(
                                                &plugin.id,
                                                PluginLifecycleState::RuntimeReady,
                                                PluginHealthStatus::Ready,
                                                None,
                                            );
                                            ready_plugins.push(plugin);
                                            let worker = service.clone();
                                            let plugins = ready_plugins.clone();
                                            let projection = tokio::task::spawn_blocking(move || {
                                                let dependencies = crate::plugin_dependency::verify_plugin_dependencies(&plugins, |_| true);
                                                let plugins = plugins.into_iter().filter(|plugin| !dependencies.demoted.contains(&plugin.id)).collect::<Vec<_>>();
                                                worker.project_candidates(&plugins)
                                            }).await;
                                            if let Ok(projection) = projection {
                                                let mut ready = service
                                                    .preparation
                                                    .ready
                                                    .write()
                                                    .unwrap_or_else(|p| p.into_inner());
                                                if revision
                                                    == service
                                                        .preparation
                                                        .revision
                                                        .load(Ordering::Acquire)
                                                {
                                                    let mut visible = ready_mcp.clone();
                                                    visible.retain_servers(|name| {
                                                        projection
                                                            .mcp_config
                                                            .servers
                                                            .contains_key(name)
                                                    });
                                                    *ready = (projection, Arc::new(visible));
                                                }
                                            }
                                        }
                                    }
                                    Err(error) => {
                                        let health = service
                                            .configuration_health(&plugin)
                                            .unwrap_or(PluginHealthStatus::Failed);
                                        let lifecycle = if matches!(
                                            health,
                                            PluginHealthStatus::NeedsAuthorization
                                                | PluginHealthStatus::NeedsConfiguration
                                        ) {
                                            PluginLifecycleState::Incomplete
                                        } else {
                                            PluginLifecycleState::Failed
                                        };
                                        service.set_preparation_state(
                                            &plugin.id,
                                            lifecycle,
                                            health,
                                            Some(error.to_string()),
                                        );
                                    }
                                }
                                changed();
                            }
                            let dependencies = crate::plugin_dependency::verify_plugin_dependencies(
                                &ready_plugins,
                                |_| true,
                            );
                            for plugin in ready_plugins
                                .iter()
                                .filter(|plugin| dependencies.demoted.contains(&plugin.id))
                            {
                                let owner = format!("plugin:{}", plugin.id);
                                if let Err(error) =
                                    mcp.reconcile_owner(&owner, BTreeSet::new()).await
                                {
                                    tracing::warn!(%error, owner, "failed to retire MCP for a plugin with unavailable dependencies");
                                }
                                service.set_preparation_state(
                                    &plugin.id,
                                    PluginLifecycleState::Incomplete,
                                    PluginHealthStatus::Incomplete,
                                    Some("required plugin dependency is not ready".into()),
                                );
                            }
                        }
                        Ok(Err(error)) => tracing::error!(%error, "plugin host discovery failed"),
                        Err(error) => tracing::error!(%error, "plugin host discovery task failed"),
                    }
                }
                drop(service);
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        });
    }

    fn set_preparation_state(
        &self,
        id: &str,
        state: PluginLifecycleState,
        health: PluginHealthStatus,
        error: Option<String>,
    ) {
        self.preparation
            .states
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(id.to_owned(), (state, health, error));
        *self.list_cache.lock().unwrap_or_else(|p| p.into_inner()) = None;
        *self.summary_cache.lock().unwrap_or_else(|p| p.into_inner()) = None;
    }

    fn configuration_health(&self, plugin: &LoadedPlugin) -> Option<PluginHealthStatus> {
        let manifest = &plugin.resolved()?.manifest;
        if manifest_needs_host_authorization(manifest) {
            Some(PluginHealthStatus::NeedsAuthorization)
        } else if !missing_credential_env_hints(&plugin.root).is_empty() {
            Some(PluginHealthStatus::NeedsConfiguration)
        } else {
            None
        }
    }

    fn prepare_candidate(
        &self,
        plugin: &LoadedPlugin,
        cancel: &deepagent_runtime::CancellationTree,
    ) -> Result<PluginRuntimeProjection> {
        let cached = self
            .preparation
            .verified
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(&plugin.id)
            .cloned();
        if let Some((paths, projection)) = cached {
            if paths.iter().all(PathSnapshot::still_matches) {
                return Ok(projection);
            }
        }
        if has_fatal_plugin_errors(plugin) || plugin.resolved().is_none() {
            return Err(CoreError::invalid("plugin manifest is not executable"));
        }
        let manifest = &plugin.resolved().unwrap().manifest;
        if manifest_needs_host_authorization(manifest)
            || !missing_credential_env_hints(&plugin.root).is_empty()
        {
            return Err(CoreError::invalid(
                "plugin requires configuration or authorization",
            ));
        }
        let data = self.data_root.join(sanitize_file_name(&plugin.id));
        std::fs::create_dir_all(&data).map_err(|e| CoreError::Persistence(e.to_string()))?;
        crate::runtime_payload::prepare_runtime_payload_with_cancel(&plugin.root, &data, cancel)?;
        let projection = self.project_candidates(std::slice::from_ref(plugin));
        if !projection.errors.is_empty() {
            return Err(CoreError::invalid(
                "plugin runtime projection is incomplete",
            ));
        }
        let mut paths = self.runtime_cache_snapshots(std::slice::from_ref(plugin));
        paths.retain(|snapshot| snapshot.path.starts_with(&plugin.root));
        paths.push(PathSnapshot::capture(plugin.root.join("runtime.zip")));
        paths.push(PathSnapshot::capture(data.join("runtime")));
        if let Some(watcher) = self
            .preparation
            .watcher
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .as_mut()
        {
            watcher
                .watch(&plugin.root, RecursiveMode::Recursive)
                .map_err(|e| CoreError::other(format!("watch plugin source: {e}")))?;
            if data.join("runtime").exists() {
                watcher
                    .watch(&data.join("runtime"), RecursiveMode::Recursive)
                    .map_err(|e| CoreError::other(format!("watch plugin runtime: {e}")))?;
            }
            self.preparation
                .verified
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .insert(plugin.id.clone(), (paths, projection.clone()));
        }
        Ok(projection)
    }

    fn start_runtime_watcher(self: &Arc<Self>) {
        let weak = Arc::downgrade(self);
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
                        let mut cached = service
                            .preparation
                            .verified
                            .lock()
                            .unwrap_or_else(|p| p.into_inner());
                        let before = cached.len();
                        cached.retain(|_, (paths, _)| {
                            !event.paths.iter().any(|changed| {
                                paths.iter().any(|path| changed.starts_with(&path.path))
                            })
                        });
                        let changed = cached.len() != before;
                        drop(cached);
                        if changed {
                            service.preparation.invalidate();
                        }
                    }
                    Err(error) => {
                        tracing::error!(%error, "plugin runtime watcher failed; invalidating verified state");
                        service
                            .preparation
                            .verified
                            .lock()
                            .unwrap_or_else(|p| p.into_inner())
                            .clear();
                        service.preparation.invalidate();
                    }
                    _ => {}
                }
            },
            Config::default(),
        );
        match watcher {
            Ok(watcher) => {
                *self
                    .preparation
                    .watcher
                    .lock()
                    .unwrap_or_else(|p| p.into_inner()) = Some(watcher)
            }
            Err(error) => {
                tracing::error!(%error, "plugin watcher unavailable; maintenance will reverify payloads")
            }
        }
    }

    fn project_candidates(&self, plugins: &[LoadedPlugin]) -> PluginRuntimeProjection {
        PluginRuntimeProjection::from_enabled_plugins(plugins.iter().filter_map(|plugin| {
            Some(EnabledPluginRuntimeInput {
                id: &plugin.id,
                name: &plugin.name,
                source_priority: plugin_runtime_priority(plugin.origin),
                root: &plugin.root,
                data_dir: self.data_root.join(sanitize_file_name(&plugin.id)),
                plugin: plugin.resolved()?,
            })
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{FileTimes, OpenOptions};
    use std::time::SystemTime;

    #[test]
    #[ignore = "audit probe for plugin watcher cache invalidation"]
    fn audit_same_size_edits_and_replacements_invalidate_prepared_projection() {
        let temp = tempfile::tempdir().unwrap();
        let plugin_root = temp.path().join("builtin").join("audit-plugin");
        let styles = plugin_root.join("output-styles");
        std::fs::create_dir_all(plugin_root.join(".codex-plugin")).unwrap();
        std::fs::create_dir_all(plugin_root.join("skills")).unwrap();
        std::fs::create_dir_all(&styles).unwrap();
        std::fs::write(plugin_root.join("skills").join("SKILL.md"), "Audit fixture").unwrap();
        std::fs::write(
            plugin_root.join(".codex-plugin").join("plugin.json"),
            serde_json::json!({
                "name": "Audit plugin",
                "version": "0.1.0",
                "skills": "skills",
                "outputStyles": "output-styles"
            })
            .to_string(),
        )
        .unwrap();
        let style_path = styles.join("concise.md");
        let original = "# Concise audit\n\nKeep replies short and concrete.";
        let edited = "# Concise audit\n\nKeep replies clear and specific.";
        assert_eq!(original.len(), edited.len());
        std::fs::write(&style_path, original).unwrap();

        let roots = PluginRoots {
            session: Vec::new(),
            builtin: temp.path().join("builtin"),
            workspace: None,
            personal: temp.path().join("personal"),
        };
        let service = Arc::new(PluginService::new(
            roots.clone(),
            temp.path().join("app-data"),
        ));
        let plugin = crate::plugin_loader::load_plugins(&roots)
            .into_iter()
            .find(|plugin| plugin.name == "Audit plugin")
            .expect("audit plugin fixture should load");

        let cancellation = deepagent_runtime::CancellationTree::new();
        service
            .prepare_candidate(&plugin, &cancellation)
            .expect("initial plugin preparation should succeed");
        service.start_runtime_watcher();
        let initial = service
            .prepare_candidate(&plugin, &cancellation)
            .expect("plugin preparation after watcher startup should succeed");
        assert!(initial.output_styles[0]
            .prompt
            .contains("short and concrete"));

        let cached_snapshots = service
            .preparation
            .verified
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(&plugin.id)
            .expect("prepared projection should be cached before the edit")
            .0
            .clone();
        let original_mtime = std::fs::metadata(&style_path)
            .unwrap()
            .modified()
            .unwrap_or(SystemTime::UNIX_EPOCH);
        std::fs::write(&style_path, edited).unwrap();
        OpenOptions::new()
            .write(true)
            .open(&style_path)
            .unwrap()
            .set_times(FileTimes::new().set_modified(original_mtime))
            .unwrap();

        let snapshots_match = cached_snapshots.iter().all(PathSnapshot::still_matches);
        assert!(
            snapshots_match,
            "the watcher event is required for this edit"
        );

        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while std::time::Instant::now() < deadline {
            let invalidated = !service
                .preparation
                .verified
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .contains_key(&plugin.id);
            if invalidated {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }

        assert!(!service
            .preparation
            .verified
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .contains_key(&plugin.id));

        let refreshed = service
            .prepare_candidate(&plugin, &cancellation)
            .expect("edited plugin should prepare after watcher invalidation");
        assert!(refreshed.output_styles[0]
            .prompt
            .contains("clear and specific"));

        let replacement = "# Concise audit\n\nKeep replies plain and specific.";
        assert_eq!(replacement.len(), edited.len());
        let replacement_mtime = std::fs::metadata(&style_path)
            .unwrap()
            .modified()
            .unwrap_or(SystemTime::UNIX_EPOCH);
        let staged_path = styles.join(".concise.md.audit-tmp");
        std::fs::write(&staged_path, replacement).unwrap();
        OpenOptions::new()
            .write(true)
            .open(&staged_path)
            .unwrap()
            .set_times(FileTimes::new().set_modified(replacement_mtime))
            .unwrap();
        std::fs::remove_file(&style_path).unwrap();
        std::fs::rename(&staged_path, &style_path).unwrap();

        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while std::time::Instant::now() < deadline {
            let invalidated = !service
                .preparation
                .verified
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .contains_key(&plugin.id);
            if invalidated {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }

        assert!(!service
            .preparation
            .verified
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .contains_key(&plugin.id));
        let replaced = service
            .prepare_candidate(&plugin, &cancellation)
            .expect("replaced plugin should prepare after watcher invalidation");
        assert!(replaced.output_styles[0]
            .prompt
            .contains("plain and specific"));
    }
}
