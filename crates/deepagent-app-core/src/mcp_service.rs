//! MCP server management for the UI (Phase B — visual MCP config).
//!
//! Persists the `.mcp.json`-style server set (plus an enabled flag per server)
//! in the document store, and exposes CRUD + DTOs so the desktop "MCP 服务器"
//! panel can add / edit / toggle / remove servers without hand-editing JSON.
//! A [`McpServerDto`] is the unified wire shape the form binds to.
//!
//! Connection (spawn stdio / connect HTTP, `tools/list`) is performed by
//! [`McpServiceConnect`] behind feature flags so the kernel workspace stays
//! offline; the persisted config is always available.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use deepagent_core::clock::{Clock, SystemClock};
use deepagent_core::error::{CoreError, Result};
use deepagent_mcp::config::{McpServerConfig, TransportType};
use deepagent_mcp::{connect_transport, McpClient, McpConfig, McpRegistry};
use deepagent_persistence::document_store::DocumentStore;
use deepagent_persistence::Database;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex as AsyncMutex;

use crate::plugin_runtime::PluginMcpServerSource;
use crate::runtime_service::{RuntimeKind, RuntimePreference, RuntimeRequirement};
use crate::{ProjectService, RuntimeBroker};

/// Document-store location for the MCP config.
const MCP_COLLECTION: &str = "mcp";
const MCP_ID: &str = "servers";

/// Persisted MCP state: the server config map + per-server enabled flags.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct McpState {
    #[serde(flatten)]
    config: McpConfig,
    /// Server name → enabled (default true when absent).
    #[serde(default)]
    enabled: BTreeMap<String, bool>,
}

impl Default for McpState {
    fn default() -> Self {
        Self {
            config: McpConfig {
                servers: BTreeMap::new(),
            },
            enabled: BTreeMap::new(),
        }
    }
}

/// A UI-facing MCP server entry (the add/edit form binds to this).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpServerDto {
    /// Server name (unique key).
    pub name: String,
    /// Transport type: "stdio" | "sse" | "http" | "ws".
    pub transport: String,
    /// Whether the server is enabled.
    pub enabled: bool,
    // --- stdio ---
    /// Launch command (stdio).
    #[serde(default)]
    pub command: Option<String>,
    /// Arguments (stdio).
    #[serde(default)]
    pub args: Vec<String>,
    /// Environment variables (stdio).
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    // --- network ---
    /// Endpoint URL (sse/http/ws).
    #[serde(default)]
    pub url: Option<String>,
    /// Request headers (sse/http/ws).
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    /// Runtime provenance: "user" for persisted config, "plugin" for overlays.
    #[serde(default = "default_mcp_server_source")]
    pub source: String,
    /// Plugin id when this is contributed by an enabled plugin.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_plugin_id: Option<String>,
    /// Plugin display/name when this is contributed by an enabled plugin.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_plugin_name: Option<String>,
    /// Original server key inside the plugin's `.mcp.json`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub declared_name: Option<String>,
    /// Source manifest/config path when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_path: Option<String>,
    /// True for plugin-provided overlay entries. Manage the plugin instead.
    #[serde(default)]
    pub read_only: bool,
    /// Reason this overlay entry is present in the UI but skipped at runtime.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conflict: Option<String>,
}

impl McpServerDto {
    fn transport_label(t: TransportType) -> &'static str {
        match t {
            TransportType::Stdio => "stdio",
            TransportType::Sse => "sse",
            TransportType::Http => "http",
            TransportType::Ws => "ws",
        }
    }

    fn parse_transport(s: &str) -> Result<TransportType> {
        match s.to_lowercase().as_str() {
            "stdio" => Ok(TransportType::Stdio),
            "sse" => Ok(TransportType::Sse),
            "http" => Ok(TransportType::Http),
            "ws" => Ok(TransportType::Ws),
            other => Err(CoreError::invalid(format!("unknown transport: {other}"))),
        }
    }

    fn from_config(name: &str, cfg: &McpServerConfig, enabled: bool) -> Self {
        let transport = cfg
            .effective_type()
            .map(Self::transport_label)
            .unwrap_or("stdio");
        Self {
            name: name.to_string(),
            transport: transport.to_string(),
            enabled,
            command: cfg.command.clone(),
            args: cfg.args.clone(),
            env: cfg.env.clone(),
            url: cfg.url.clone(),
            headers: cfg.headers.clone(),
            source: default_mcp_server_source(),
            source_plugin_id: None,
            source_plugin_name: None,
            declared_name: None,
            source_path: None,
            read_only: false,
            conflict: None,
        }
    }

    fn from_plugin_config(
        name: &str,
        cfg: &McpServerConfig,
        source: Option<&PluginMcpServerSource>,
        enabled: bool,
        conflict: Option<String>,
    ) -> Self {
        let mut dto = Self::from_config(name, cfg, enabled);
        dto.source = "plugin".to_string();
        dto.source_plugin_id = source.map(|source| source.plugin_id.clone());
        dto.source_plugin_name = source.map(|source| source.plugin_name.clone());
        dto.declared_name = source.map(|source| source.declared_name.clone());
        dto.source_path = source.and_then(|source| source.source_path.clone());
        dto.read_only = true;
        dto.conflict = conflict;
        dto
    }

    fn to_config(&self) -> Result<McpServerConfig> {
        let cfg = McpServerConfig {
            transport: Some(Self::parse_transport(&self.transport)?),
            command: self.command.clone().filter(|c| !c.trim().is_empty()),
            args: self.args.clone(),
            env: self.env.clone(),
            url: self.url.clone().filter(|u| !u.trim().is_empty()),
            headers: self.headers.clone(),
        };
        cfg.validate()?;
        Ok(cfg)
    }
}

fn default_mcp_server_source() -> String {
    "user".to_string()
}

fn normalized_mcp_name(name: &str) -> String {
    name.trim().to_ascii_lowercase()
}

fn plugin_mcp_conflict_reason(
    runtime_name: &str,
    declared_name: &str,
    user_names: &BTreeSet<String>,
) -> Option<String> {
    let runtime_key = normalized_mcp_name(runtime_name);
    if user_names.contains(&runtime_key) {
        return Some(format!(
            "skipped because user MCP server '{runtime_name}' has priority"
        ));
    }
    let declared_key = normalized_mcp_name(declared_name);
    if user_names.contains(&declared_key) {
        return Some(format!(
            "skipped because user MCP server '{declared_name}' has priority"
        ));
    }
    None
}

struct EffectiveMcpServer {
    dto: McpServerDto,
    config: McpServerConfig,
}

/// A tool exposed by a connected MCP server (name + description), surfaced in
/// the UI's per-server tool list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpToolInfoDto {
    /// Server-local tool name (as advertised by `tools/list`).
    pub name: String,
    /// Human-readable description.
    pub description: String,
}

/// The live connection status of one MCP server, with its discovered tools.
///
/// `status` is one of `"connected"` | `"failed"` | `"disabled"`, kept as a
/// plain string so the React UI can switch on it directly.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpConnectionStatusDto {
    /// Server name (matches the [`McpServerDto`] key).
    pub name: String,
    /// `"connected"` | `"failed"` | `"disabled"`.
    pub status: String,
    /// Error message when `status == "failed"`.
    #[serde(default)]
    pub error: Option<String>,
    /// Tools discovered via `tools/list` (empty unless connected).
    #[serde(default)]
    pub tools: Vec<McpToolInfoDto>,
}

/// UI-facing MCP server management.
pub struct McpService {
    db: Arc<Database>,
    runtime: Option<Arc<RuntimeBroker>>,
    projects: Option<Arc<ProjectService>>,
    /// Cross-run cache of the connected registry, keyed by a fingerprint of the
    /// enabled-server config. Lets a chat run reuse already-spawned MCP servers
    /// (stdio/npx cold-start is the dominant per-run latency) instead of
    /// re-connecting every message; the cache is invalidated (old connections
    /// closed) whenever the enabled config changes.
    cache: Arc<AsyncMutex<Option<CachedRegistry>>>,
}

/// A cached, live [`McpRegistry`] plus the config fingerprint it was built for.
struct CachedRegistry {
    fingerprint: String,
    registry: Arc<McpRegistry>,
    failures: Vec<(String, String)>,
}

impl McpService {
    /// Build over the shared database.
    pub fn new(db: Arc<Database>) -> Self {
        Self {
            db,
            runtime: None,
            projects: None,
            cache: Arc::new(AsyncMutex::new(None)),
        }
    }

    pub fn with_runtime(
        mut self,
        runtime: Arc<RuntimeBroker>,
        projects: Arc<ProjectService>,
    ) -> Self {
        self.runtime = Some(runtime);
        self.projects = Some(projects);
        self
    }

    fn prepare_runtime_config(&self, config: &mut McpConfig) {
        let project_root = self
            .projects
            .as_ref()
            .and_then(|projects| projects.active().ok().flatten())
            .map(std::path::PathBuf::from);
        let environment = self
            .runtime
            .as_ref()
            .map(|runtime| runtime.build_process_environment(project_root.as_deref()))
            .unwrap_or_default();
        // Expand placeholders before resolving plugin-declared requirements.
        config.expand_with(&|var| {
            if var == "DEEPAGENT_PROJECT_ROOT" {
                project_root.as_ref().map(|root| root.display().to_string())
            } else {
                environment
                    .get(var)
                    .cloned()
                    .or_else(|| std::env::var(var).ok())
            }
        });
        for server in config.servers.values_mut() {
            let preference = server
                .env
                .get("DEEPAGENT_RUNTIME_PREFERENCE")
                .and_then(|value| RuntimePreference::parse(value))
                .unwrap_or(RuntimePreference::PreferLocal);
            let declared = [
                (
                    RuntimeKind::Node,
                    "DEEPAGENT_RUNTIME_NODE_REQUIREMENT",
                    "node",
                ),
                (
                    RuntimeKind::Python,
                    "DEEPAGENT_RUNTIME_PYTHON_REQUIREMENT",
                    "python",
                ),
                (
                    RuntimeKind::Java,
                    "DEEPAGENT_RUNTIME_JAVA_REQUIREMENT",
                    "java",
                ),
            ];
            if let Some(runtime) = self.runtime.as_ref() {
                for (kind, requirement_key, label) in declared {
                    let Some(version) = server.env.get(requirement_key).cloned() else {
                        continue;
                    };
                    let requirement = RuntimeRequirement {
                        kind,
                        version: Some(version),
                        preference,
                    };
                    match runtime.resolve(&requirement, project_root.as_deref(), None) {
                        Ok(resolution) => {
                            server.command = Some(resolution.executable.display().to_string())
                        }
                        Err(error) => {
                            server.command = Some(format!("__deepagent_missing_{label}_runtime__"));
                            server
                                .env
                                .insert("DEEPAGENT_RUNTIME_ERROR".to_string(), error.to_string());
                        }
                    }
                    break;
                }
            }
            let Some(command) = server.command.as_deref() else {
                continue;
            };
            if let Some(executable) = [
                ("node", "DEEPAGENT_NODE"),
                ("python", "DEEPAGENT_PYTHON"),
                ("python3", "DEEPAGENT_PYTHON"),
                ("java", "DEEPAGENT_JAVA"),
            ]
            .iter()
            .find_map(|(name, key)| {
                command
                    .eq_ignore_ascii_case(name)
                    .then(|| environment.get(*key))
            })
            .flatten()
            {
                server.command = Some(executable.clone());
            }
            for (key, value) in &environment {
                server
                    .env
                    .entry(key.clone())
                    .or_insert_with(|| value.clone());
            }
            if let Some(root) = project_root.as_ref() {
                server
                    .env
                    .entry("DEEPAGENT_PROJECT_ROOT".to_string())
                    .or_insert_with(|| root.display().to_string());
            }
        }
    }

    fn load_state(&self) -> Result<McpState> {
        let store = DocumentStore::new(&self.db);
        match store.get(MCP_COLLECTION, MCP_ID)? {
            Some(doc) => Ok(serde_json::from_str(&doc.body).unwrap_or_default()),
            None => Ok(McpState::default()),
        }
    }

    fn save_state(&self, state: &McpState) -> Result<()> {
        let store = DocumentStore::new(&self.db);
        let body = serde_json::to_string(state)?;
        store.put(MCP_COLLECTION, MCP_ID, &body, None, SystemClock.now())
    }

    fn user_entries(state: &McpState) -> Vec<EffectiveMcpServer> {
        state
            .config
            .servers
            .iter()
            .map(|(name, cfg)| {
                let enabled = state.enabled.get(name).copied().unwrap_or(true);
                EffectiveMcpServer {
                    dto: McpServerDto::from_config(name, cfg, enabled),
                    config: cfg.clone(),
                }
            })
            .collect()
    }

    fn effective_entries_with_plugin_overlay(
        &self,
        overlay: McpConfig,
        sources: &BTreeMap<String, PluginMcpServerSource>,
    ) -> Result<Vec<EffectiveMcpServer>> {
        let state = self.load_state()?;
        let user_names = state
            .config
            .servers
            .keys()
            .map(|name| normalized_mcp_name(name))
            .collect::<BTreeSet<_>>();
        let mut entries = Self::user_entries(&state);
        let mut active_names = entries
            .iter()
            .map(|entry| normalized_mcp_name(&entry.dto.name))
            .collect::<BTreeSet<_>>();

        for (runtime_name, server) in overlay.servers {
            let source = sources.get(&runtime_name);
            let declared_name = source
                .map(|source| source.declared_name.as_str())
                .unwrap_or(runtime_name.as_str());
            let conflict = plugin_mcp_conflict_reason(&runtime_name, declared_name, &user_names)
                .or_else(|| {
                    let runtime_key = normalized_mcp_name(&runtime_name);
                    if active_names.contains(&runtime_key) {
                        Some(format!(
                            "skipped because another MCP server already uses runtime name '{runtime_name}'"
                        ))
                    } else {
                        None
                    }
                });
            let enabled = conflict.is_none();
            if enabled {
                active_names.insert(normalized_mcp_name(&runtime_name));
            }
            entries.push(EffectiveMcpServer {
                dto: McpServerDto::from_plugin_config(
                    &runtime_name,
                    &server,
                    source,
                    enabled,
                    conflict,
                ),
                config: server,
            });
        }
        Ok(entries)
    }

    /// List all configured servers as DTOs (sorted by name).
    pub fn list(&self) -> Result<Vec<McpServerDto>> {
        let state = self.load_state()?;
        Ok(Self::user_entries(&state)
            .into_iter()
            .map(|entry| entry.dto)
            .collect())
    }

    /// List persisted user MCP servers plus enabled plugin overlay servers.
    /// Plugin entries are read-only and include their plugin provenance.
    pub fn list_with_plugin_overlay(
        &self,
        overlay: McpConfig,
        sources: &BTreeMap<String, PluginMcpServerSource>,
    ) -> Result<Vec<McpServerDto>> {
        Ok(self
            .effective_entries_with_plugin_overlay(overlay, sources)?
            .into_iter()
            .map(|entry| entry.dto)
            .collect())
    }

    /// Get a single server by name.
    pub fn get(&self, name: &str) -> Result<Option<McpServerDto>> {
        Ok(self.list()?.into_iter().find(|s| s.name == name))
    }

    /// Add or update a server (upsert by name). Validates the config.
    pub fn upsert(&self, dto: McpServerDto) -> Result<McpServerDto> {
        if dto.read_only || dto.source != "user" {
            return Err(CoreError::invalid(
                "plugin MCP servers are read-only; manage the owning plugin instead",
            ));
        }
        if dto.name.trim().is_empty() {
            return Err(CoreError::invalid("MCP server name must not be empty"));
        }
        let cfg = dto.to_config()?;
        let mut state = self.load_state()?;
        state.config.servers.insert(dto.name.clone(), cfg);
        state.enabled.insert(dto.name.clone(), dto.enabled);
        self.save_state(&state)?;
        Ok(dto)
    }

    /// Remove a server by name. Returns whether it existed.
    pub fn remove(&self, name: &str) -> Result<bool> {
        let mut state = self.load_state()?;
        let existed = state.config.servers.remove(name).is_some();
        state.enabled.remove(name);
        if existed {
            self.save_state(&state)?;
        }
        Ok(existed)
    }

    /// Enable / disable a server. Returns whether it existed.
    pub fn set_enabled(&self, name: &str, enabled: bool) -> Result<bool> {
        let mut state = self.load_state()?;
        if !state.config.servers.contains_key(name) {
            return Ok(false);
        }
        state.enabled.insert(name.to_string(), enabled);
        self.save_state(&state)?;
        Ok(true)
    }

    /// The merged `.mcp.json` config of **enabled** servers (for the runtime to
    /// connect at session start).
    pub fn enabled_config(&self) -> Result<McpConfig> {
        let state = self.load_state()?;
        let servers = state
            .config
            .servers
            .iter()
            .filter(|(name, _)| state.enabled.get(*name).copied().unwrap_or(true))
            .map(|(n, c)| (n.clone(), c.clone()))
            .collect();
        Ok(McpConfig { servers })
    }

    /// Enabled user MCP config plus a runtime-only overlay, used by plugins.
    /// Persisted user config wins on name collision; plugin callers should
    /// namespace server names before passing the overlay.
    pub fn enabled_config_with_overlay(&self, overlay: McpConfig) -> Result<McpConfig> {
        let mut config = self.enabled_config()?;
        for (name, server) in overlay.servers {
            if config.servers.contains_key(&name) {
                tracing::warn!(
                    server = name.as_str(),
                    "skipping plugin MCP overlay because a user MCP server has the same name"
                );
                continue;
            }
            config.servers.insert(name, server);
        }
        Ok(config)
    }

    /// Enabled user MCP config plus plugin overlay servers. User MCP entries
    /// win both on the runtime key and on the plugin's declared server name,
    /// even when the user entry is disabled.
    pub fn enabled_config_with_plugin_overlay(
        &self,
        overlay: McpConfig,
        sources: &BTreeMap<String, PluginMcpServerSource>,
    ) -> Result<McpConfig> {
        let servers = self
            .effective_entries_with_plugin_overlay(overlay, sources)?
            .into_iter()
            .filter(|entry| entry.dto.enabled && entry.dto.conflict.is_none())
            .map(|entry| (entry.dto.name, entry.config))
            .collect();
        Ok(McpConfig { servers })
    }

    /// Connect every **enabled** server, run the `initialize` + `tools/list`
    /// handshake, and return a populated [`McpRegistry`] whose namespaced tools
    /// can be adapted into the runtime tool system (live tool registration).
    ///
    /// `${VAR}` placeholders in each server config are expanded from the process
    /// environment first. Connection failures are **non-fatal**: a server that
    /// can't be reached (or needs the `http` feature) is logged and skipped so
    /// one bad entry never blocks a chat run. Returns the registry plus the list
    /// of `(server, error)` pairs that failed, for surfacing in the UI.
    pub async fn connect_enabled(&self) -> Result<(McpRegistry, Vec<(String, String)>)> {
        self.connect_config(self.enabled_config()?).await
    }

    /// Connect enabled user MCP servers plus a runtime-only overlay. This path
    /// is intentionally not persisted and is used by enabled plugins.
    pub async fn connect_enabled_with_overlay(
        &self,
        overlay: McpConfig,
    ) -> Result<(McpRegistry, Vec<(String, String)>)> {
        self.connect_config(self.enabled_config_with_overlay(overlay)?)
            .await
    }

    /// Connect enabled user MCP servers plus plugin overlay servers.
    pub async fn connect_enabled_with_plugin_overlay(
        &self,
        overlay: McpConfig,
        sources: &BTreeMap<String, PluginMcpServerSource>,
    ) -> Result<(McpRegistry, Vec<(String, String)>)> {
        self.connect_config(self.enabled_config_with_plugin_overlay(overlay, sources)?)
            .await
    }

    async fn connect_config(
        &self,
        mut config: McpConfig,
    ) -> Result<(McpRegistry, Vec<(String, String)>)> {
        self.prepare_runtime_config(&mut config);

        // Connect (spawn + initialize) every server concurrently: the process
        // cold-start (npx/node) dominates, so fanning out turns a "sum of
        // servers" wait into "slowest server". tools/list then runs per server.
        let connects = config.servers.iter().map(|(name, cfg)| {
            let name = name.clone();
            let cfg = cfg.clone();
            async move {
                let result = Self::connect_one_resilient(&name, &cfg).await;
                (name, result)
            }
        });
        let connected = futures::future::join_all(connects).await;

        let mut registry = McpRegistry::new();
        let mut failures = Vec::new();
        for (name, result) in connected {
            match result {
                Ok(client) => {
                    if let Err(e) = registry.register(&name, client).await {
                        failures.push((name, e.to_string()));
                    }
                }
                Err(e) => {
                    tracing::warn!(server = name.as_str(), error = %e, "MCP connect failed; skipping");
                    failures.push((name, e.to_string()));
                }
            }
        }
        Ok((registry, failures))
    }

    /// Return a **shared, cached** connected registry for the current enabled
    /// config, connecting only when the config changed since the last call.
    ///
    /// This is the per-run entry point: the first run pays the connect cost
    /// (now parallel), and subsequent runs reuse the already-spawned servers
    /// with near-zero latency. When the enabled config changes (add / edit /
    /// toggle / remove a server) the fingerprint no longer matches, so the old
    /// connections are closed and fresh ones are established.
    pub async fn connected_registry(&self) -> Result<(Arc<McpRegistry>, Vec<(String, String)>)> {
        self.connected_registry_for_config(self.enabled_config()?)
            .await
    }

    /// Close and drop the cached connected registry, if any.
    ///
    /// Plugin runtime changes can remove overlay MCP servers without touching
    /// the user's persisted MCP config, so callers need an explicit invalidation
    /// hook instead of waiting for the next effective-config fingerprint check.
    pub async fn invalidate_connected_registry(&self) -> Result<()> {
        let mut guard = self.cache.lock().await;
        if let Some(cached) = guard.take() {
            cached.registry.close_all().await?;
        }
        Ok(())
    }

    /// Cached connected registry for enabled user config plus plugin overlay.
    pub async fn connected_registry_with_overlay(
        &self,
        overlay: McpConfig,
    ) -> Result<(Arc<McpRegistry>, Vec<(String, String)>)> {
        self.connected_registry_for_config(self.enabled_config_with_overlay(overlay)?)
            .await
    }

    /// Cached connected registry for enabled user config plus plugin overlay.
    pub async fn connected_registry_with_plugin_overlay(
        &self,
        overlay: McpConfig,
        sources: &BTreeMap<String, PluginMcpServerSource>,
    ) -> Result<(Arc<McpRegistry>, Vec<(String, String)>)> {
        self.connected_registry_for_config(
            self.enabled_config_with_plugin_overlay(overlay, sources)?,
        )
        .await
    }

    async fn connected_registry_for_config(
        &self,
        mut config: McpConfig,
    ) -> Result<(Arc<McpRegistry>, Vec<(String, String)>)> {
        self.prepare_runtime_config(&mut config);
        // Fingerprint = the serialized effective config. Any change to which
        // servers are enabled or how they're configured busts the cache.
        let fingerprint = serde_json::to_string(&config).unwrap_or_default();

        let mut guard = self.cache.lock().await;
        if let Some(cached) = guard.as_ref() {
            if cached.fingerprint == fingerprint {
                return Ok((cached.registry.clone(), cached.failures.clone()));
            }
            // Config changed: tear down the stale connections before reconnecting.
            let _ = cached.registry.close_all().await;
        }

        let (registry, failures) = self.connect_config(config).await?;
        let mut registry = registry;
        // Long-lived cached connections get a background liveness probe (MCP
        // spec `ping` utility, §5.2): a server that dies between runs is
        // detected and respawned by the self-healing transport underneath the
        // ping, instead of failing the next tool call. Probes stop when the
        // cache entry is replaced/dropped.
        registry.enable_liveness(deepagent_mcp::LivenessConfig::default());
        let registry = Arc::new(registry);
        *guard = Some(CachedRegistry {
            fingerprint,
            registry: registry.clone(),
            failures: failures.clone(),
        });
        Ok((registry, failures))
    }

    /// Connect a single server and perform the initialize handshake. Used by
    /// one-shot probes (`test_server` / `connection_status`), which close the
    /// connection immediately — no self-healing needed.
    async fn connect_one(name: &str, cfg: &McpServerConfig) -> Result<Arc<McpClient>> {
        let transport = connect_transport(cfg)?;
        let client = Arc::new(McpClient::new(transport));
        client.initialize("deepagent-studio").await?;
        tracing::info!(server = name, "MCP server initialized");
        Ok(client)
    }

    /// Connect a single server for the long-lived run registry, wrapping the
    /// transport with bounded auto-reconnect (§5.2 MCP resilience, Grok
    /// `mcp_restart.rs` backoff): if the server crashes mid-run, a later tool
    /// call transparently respawns it instead of staying dead for the session.
    async fn connect_one_resilient(name: &str, cfg: &McpServerConfig) -> Result<Arc<McpClient>> {
        let transport = connect_transport(cfg)?;
        let factory = Arc::new(deepagent_mcp::ConfigReconnectFactory::new(
            cfg.clone(),
            "deepagent-studio",
        ));
        let resilient: Arc<dyn deepagent_mcp::McpTransport> = Arc::new(
            deepagent_mcp::ReconnectingTransport::new(transport, factory),
        );
        let client = Arc::new(McpClient::new(resilient));
        client.initialize("deepagent-studio").await?;
        tracing::info!(server = name, "MCP server initialized (self-healing)");
        Ok(client)
    }

    /// Test-connect a single (possibly unsaved) server config: expand `${VAR}`
    /// placeholders, connect + `initialize` + `tools/list`, then close the
    /// connection. Connection problems never error — they are captured as a
    /// `"failed"` status so the "test connection" button can render the reason.
    pub async fn test_server(&self, dto: McpServerDto) -> Result<McpConnectionStatusDto> {
        let mut cfg = dto.to_config()?;
        cfg.expand_with(&|var| std::env::var(var).ok());
        Ok(Self::probe(&dto.name, &cfg).await)
    }

    /// Live status of every saved server: disabled ones are reported as
    /// `"disabled"` without connecting; enabled ones are probed (connect +
    /// `tools/list`) with failures captured per-server. Ordered by name.
    ///
    /// The per-server probes run **concurrently** (`join_all`): each probe is
    /// IO-bound (spawn / network + handshake), so fanning them out means a
    /// "refresh status" over N servers costs roughly the slowest one rather
    /// than the sum. Result order still matches the (name-sorted) server map.
    pub async fn connection_status(&self) -> Result<Vec<McpConnectionStatusDto>> {
        let state = self.load_state()?;
        self.connection_status_for_entries(Self::user_entries(&state))
            .await
    }

    /// Live status of persisted user servers plus plugin overlay servers.
    pub async fn connection_status_with_plugin_overlay(
        &self,
        overlay: McpConfig,
        sources: &BTreeMap<String, PluginMcpServerSource>,
    ) -> Result<Vec<McpConnectionStatusDto>> {
        self.connection_status_for_entries(
            self.effective_entries_with_plugin_overlay(overlay, sources)?,
        )
        .await
    }

    async fn connection_status_for_entries(
        &self,
        entries: Vec<EffectiveMcpServer>,
    ) -> Result<Vec<McpConnectionStatusDto>> {
        let probes = entries.into_iter().map(|entry| {
            let name = entry.dto.name.clone();
            let enabled = entry.dto.enabled && entry.dto.conflict.is_none();
            let conflict = entry.dto.conflict.clone();
            let mut cfg = entry.config;
            async move {
                if !enabled {
                    return McpConnectionStatusDto {
                        name,
                        status: "disabled".into(),
                        error: conflict,
                        tools: Vec::new(),
                    };
                }
                cfg.expand_with(&|var| std::env::var(var).ok());
                Self::probe(&name, &cfg).await
            }
        });
        Ok(futures::future::join_all(probes).await)
    }

    /// Connect + `initialize` + `tools/list` one config, mapping any failure to
    /// a `"failed"` status. The connection is closed before returning.
    async fn probe(name: &str, cfg: &McpServerConfig) -> McpConnectionStatusDto {
        let failed = |e: CoreError| McpConnectionStatusDto {
            name: name.to_string(),
            status: "failed".into(),
            error: Some(e.to_string()),
            tools: Vec::new(),
        };
        let client = match Self::connect_one(name, cfg).await {
            Ok(client) => client,
            Err(e) => return failed(e),
        };
        let result = client.list_tools().await;
        let _ = client.close().await;
        match result {
            Ok(defs) => McpConnectionStatusDto {
                name: name.to_string(),
                status: "connected".into(),
                error: None,
                tools: defs
                    .into_iter()
                    .map(|d| McpToolInfoDto {
                        name: d.name,
                        description: d.description,
                    })
                    .collect(),
            },
            Err(e) => failed(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn service() -> McpService {
        McpService::new(Arc::new(Database::open_in_memory().unwrap()))
    }

    fn stdio_dto(name: &str) -> McpServerDto {
        McpServerDto {
            name: name.to_string(),
            transport: "stdio".into(),
            enabled: true,
            command: Some("npx".into()),
            args: vec!["-y".into(), "server-filesystem".into()],
            env: BTreeMap::new(),
            url: None,
            headers: BTreeMap::new(),
            source: "user".into(),
            source_plugin_id: None,
            source_plugin_name: None,
            declared_name: None,
            source_path: None,
            read_only: false,
            conflict: None,
        }
    }

    fn stdio_config(command: &str) -> McpServerConfig {
        McpServerConfig {
            transport: None,
            command: Some(command.into()),
            args: Vec::new(),
            env: BTreeMap::new(),
            url: None,
            headers: BTreeMap::new(),
        }
    }

    fn plugin_source(
        runtime_name: &str,
        declared_name: &str,
        plugin_id: &str,
    ) -> PluginMcpServerSource {
        PluginMcpServerSource {
            plugin_id: plugin_id.to_string(),
            plugin_name: plugin_id
                .split_once('@')
                .map(|(name, _)| name)
                .unwrap_or(plugin_id)
                .to_string(),
            declared_name: declared_name.to_string(),
            runtime_name: runtime_name.to_string(),
            source_path: Some("plugin/.mcp.json".to_string()),
        }
    }

    #[test]
    fn upsert_list_get_roundtrip() {
        let svc = service();
        assert!(svc.list().unwrap().is_empty());
        svc.upsert(stdio_dto("fs")).unwrap();
        let list = svc.list().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "fs");
        assert_eq!(list[0].transport, "stdio");
        assert_eq!(list[0].command.as_deref(), Some("npx"));
        assert!(list[0].enabled);

        let got = svc.get("fs").unwrap().unwrap();
        assert_eq!(got.args, vec!["-y", "server-filesystem"]);
    }

    #[test]
    fn upsert_replaces_same_name() {
        let svc = service();
        svc.upsert(stdio_dto("fs")).unwrap();
        let mut updated = stdio_dto("fs");
        updated.command = Some("node".into());
        svc.upsert(updated).unwrap();
        let list = svc.list().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].command.as_deref(), Some("node"));
    }

    #[test]
    fn http_server_requires_secure_url() {
        let svc = service();
        let mut dto = stdio_dto("api");
        dto.transport = "http".into();
        dto.command = None;
        dto.url = Some("http://evil.example.com/mcp".into());
        assert!(svc.upsert(dto).is_err()); // insecure url rejected by validate
    }

    #[test]
    fn http_server_accepts_https() {
        let svc = service();
        let mut dto = stdio_dto("api");
        dto.transport = "http".into();
        dto.command = None;
        dto.url = Some("https://mcp.example.com/mcp".into());
        assert!(svc.upsert(dto).is_ok());
    }

    #[test]
    fn enable_disable_and_enabled_config() {
        let svc = service();
        svc.upsert(stdio_dto("a")).unwrap();
        svc.upsert(stdio_dto("b")).unwrap();
        assert!(svc.set_enabled("b", false).unwrap());
        // enabled_config only contains "a".
        let cfg = svc.enabled_config().unwrap();
        assert!(cfg.servers.contains_key("a"));
        assert!(!cfg.servers.contains_key("b"));
        // list still shows both, b disabled.
        let list = svc.list().unwrap();
        assert_eq!(list.len(), 2);
        assert!(!list.iter().find(|s| s.name == "b").unwrap().enabled);
    }

    #[test]
    fn remove_server() {
        let svc = service();
        svc.upsert(stdio_dto("fs")).unwrap();
        assert!(svc.remove("fs").unwrap());
        assert!(svc.list().unwrap().is_empty());
        assert!(!svc.remove("fs").unwrap());
    }

    #[test]
    fn empty_name_rejected() {
        let svc = service();
        let mut dto = stdio_dto("");
        dto.name = "  ".into();
        assert!(svc.upsert(dto).is_err());
    }

    #[test]
    fn set_enabled_unknown_is_false() {
        let svc = service();
        assert!(!svc.set_enabled("ghost", true).unwrap());
    }

    #[tokio::test]
    async fn connect_enabled_skips_unreachable_servers() {
        let svc = service();
        // A stdio server whose command does not exist: connect must fail
        // gracefully (recorded as a failure, not an error), leaving an empty
        // registry rather than blocking the run.
        let mut dto = stdio_dto("ghost");
        dto.command = Some("definitely-not-a-real-binary-xyz".into());
        dto.args = vec![];
        svc.upsert(dto).unwrap();

        let (registry, failures) = svc.connect_enabled().await.unwrap();
        assert_eq!(registry.server_count(), 0);
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].0, "ghost");
    }

    #[tokio::test]
    async fn connect_enabled_empty_when_no_servers() {
        let svc = service();
        let (registry, failures) = svc.connect_enabled().await.unwrap();
        assert_eq!(registry.server_count(), 0);
        assert!(failures.is_empty());
    }

    #[tokio::test]
    async fn connect_enabled_ignores_disabled_servers() {
        let svc = service();
        let mut dto = stdio_dto("off");
        dto.command = Some("definitely-not-a-real-binary-xyz".into());
        svc.upsert(dto).unwrap();
        svc.set_enabled("off", false).unwrap();
        // Disabled server is not even attempted → no failures.
        let (_registry, failures) = svc.connect_enabled().await.unwrap();
        assert!(failures.is_empty());
    }

    #[tokio::test]
    async fn test_server_reports_failed_for_bad_command() {
        let svc = service();
        let mut dto = stdio_dto("ghost");
        dto.command = Some("definitely-not-a-real-binary-xyz".into());
        dto.args = vec![];
        // test_server never errors on connect problems — it captures them.
        let status = svc.test_server(dto).await.unwrap();
        assert_eq!(status.name, "ghost");
        assert_eq!(status.status, "failed");
        assert!(status.error.is_some());
        assert!(status.tools.is_empty());
    }

    #[tokio::test]
    async fn test_server_invalid_config_errors() {
        // An invalid DTO (insecure http url) is rejected before any connection
        // attempt — this is a genuine Err, not a "failed" status.
        let svc = service();
        let mut dto = stdio_dto("api");
        dto.transport = "http".into();
        dto.command = None;
        dto.url = Some("http://evil.example.com/mcp".into());
        assert!(svc.test_server(dto).await.is_err());
    }

    #[tokio::test]
    async fn connection_status_marks_disabled_without_connecting() {
        let svc = service();
        let mut dto = stdio_dto("off");
        dto.command = Some("definitely-not-a-real-binary-xyz".into());
        svc.upsert(dto).unwrap();
        svc.set_enabled("off", false).unwrap();

        let statuses = svc.connection_status().await.unwrap();
        assert_eq!(statuses.len(), 1);
        assert_eq!(statuses[0].name, "off");
        assert_eq!(statuses[0].status, "disabled");
        assert!(statuses[0].error.is_none());
    }

    #[tokio::test]
    async fn connection_status_probes_enabled_servers() {
        let svc = service();
        let mut dto = stdio_dto("ghost");
        dto.command = Some("definitely-not-a-real-binary-xyz".into());
        dto.args = vec![];
        svc.upsert(dto).unwrap();

        let statuses = svc.connection_status().await.unwrap();
        assert_eq!(statuses.len(), 1);
        assert_eq!(statuses[0].status, "failed");
        assert!(statuses[0].error.is_some());
    }

    #[tokio::test]
    async fn connected_registry_reuses_until_config_changes() {
        let svc = service();
        let mut dto = stdio_dto("ghost");
        dto.command = Some("definitely-not-a-real-binary-xyz".into());
        dto.args = vec![];
        svc.upsert(dto).unwrap();

        let (reg1, failures1) = svc.connected_registry().await.unwrap();
        assert_eq!(failures1.len(), 1);
        // Same config → the exact same Arc is reused (no reconnect).
        let (reg2, _) = svc.connected_registry().await.unwrap();
        assert!(
            Arc::ptr_eq(&reg1, &reg2),
            "registry should be reused when config is unchanged"
        );

        // Config change (add a server) busts the cache → a fresh Arc.
        let mut other = stdio_dto("ghost2");
        other.command = Some("definitely-not-a-real-binary-xyz".into());
        other.args = vec![];
        svc.upsert(other).unwrap();
        let (reg3, _) = svc.connected_registry().await.unwrap();
        assert!(
            !Arc::ptr_eq(&reg1, &reg3),
            "registry should be rebuilt after a config change"
        );
    }

    #[tokio::test]
    async fn invalidate_connected_registry_drops_cached_registry() {
        let svc = service();
        let (reg1, failures) = svc.connected_registry().await.unwrap();
        assert!(failures.is_empty());
        assert!(svc.cache.lock().await.is_some());

        svc.invalidate_connected_registry().await.unwrap();
        assert!(svc.cache.lock().await.is_none());

        let (reg2, _) = svc.connected_registry().await.unwrap();
        assert!(
            !Arc::ptr_eq(&reg1, &reg2),
            "registry should be rebuilt after explicit invalidation"
        );
    }

    #[test]
    fn plugin_overlay_entries_are_listed_as_read_only() {
        let svc = service();
        let mut overlay = McpConfig {
            servers: BTreeMap::new(),
        };
        overlay.servers.insert(
            "plugin__docs_plugin__docs".to_string(),
            stdio_config("node"),
        );
        let mut sources = BTreeMap::new();
        sources.insert(
            "plugin__docs_plugin__docs".to_string(),
            plugin_source("plugin__docs_plugin__docs", "docs", "docs-plugin@workspace"),
        );

        let list = svc
            .list_with_plugin_overlay(overlay.clone(), &sources)
            .unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].source, "plugin");
        assert!(list[0].read_only);
        assert!(list[0].enabled);
        assert_eq!(
            list[0].source_plugin_id.as_deref(),
            Some("docs-plugin@workspace")
        );
        assert_eq!(list[0].declared_name.as_deref(), Some("docs"));

        let effective = svc
            .enabled_config_with_plugin_overlay(overlay, &sources)
            .unwrap();
        assert!(effective.servers.contains_key("plugin__docs_plugin__docs"));
    }

    #[test]
    fn user_mcp_server_name_has_priority_over_plugin_declared_name() {
        let svc = service();
        svc.upsert(stdio_dto("docs")).unwrap();
        svc.set_enabled("docs", false).unwrap();

        let mut overlay = McpConfig {
            servers: BTreeMap::new(),
        };
        overlay.servers.insert(
            "plugin__docs_plugin__docs".to_string(),
            stdio_config("node"),
        );
        let mut sources = BTreeMap::new();
        sources.insert(
            "plugin__docs_plugin__docs".to_string(),
            plugin_source("plugin__docs_plugin__docs", "docs", "docs-plugin@workspace"),
        );

        let list = svc
            .list_with_plugin_overlay(overlay.clone(), &sources)
            .unwrap();
        let plugin = list
            .iter()
            .find(|server| server.name == "plugin__docs_plugin__docs")
            .unwrap();
        assert!(plugin.read_only);
        assert!(!plugin.enabled);
        assert!(plugin
            .conflict
            .as_deref()
            .unwrap()
            .contains("user MCP server 'docs' has priority"));

        let effective = svc
            .enabled_config_with_plugin_overlay(overlay, &sources)
            .unwrap();
        assert!(effective.servers.is_empty());
    }

    #[tokio::test]
    async fn plugin_overlay_conflict_reports_disabled_status() {
        let svc = service();
        svc.upsert(stdio_dto("docs")).unwrap();

        let mut overlay = McpConfig {
            servers: BTreeMap::new(),
        };
        overlay.servers.insert(
            "plugin__docs_plugin__docs".to_string(),
            stdio_config("definitely-not-a-real-binary-xyz"),
        );
        let mut sources = BTreeMap::new();
        sources.insert(
            "plugin__docs_plugin__docs".to_string(),
            plugin_source("plugin__docs_plugin__docs", "docs", "docs-plugin@workspace"),
        );

        let statuses = svc
            .connection_status_with_plugin_overlay(overlay, &sources)
            .await
            .unwrap();
        let plugin = statuses
            .iter()
            .find(|status| status.name == "plugin__docs_plugin__docs")
            .unwrap();
        assert_eq!(plugin.status, "disabled");
        assert!(plugin
            .error
            .as_deref()
            .unwrap()
            .contains("user MCP server 'docs' has priority"));
    }

    #[tokio::test]
    async fn connection_status_mixes_states_in_name_order() {
        // Concurrent probing must preserve the (name-sorted) order and each
        // server's own status: enabled+bad → failed, disabled → disabled.
        let svc = service();
        for name in ["a_on", "b_off", "c_on"] {
            let mut dto = stdio_dto(name);
            dto.command = Some("definitely-not-a-real-binary-xyz".into());
            dto.args = vec![];
            svc.upsert(dto).unwrap();
        }
        svc.set_enabled("b_off", false).unwrap();

        let statuses = svc.connection_status().await.unwrap();
        let names: Vec<_> = statuses.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["a_on", "b_off", "c_on"]);
        assert_eq!(statuses[0].status, "failed");
        assert_eq!(statuses[1].status, "disabled");
        assert_eq!(statuses[2].status, "failed");
    }

    /// Live smoke test against the official MCP "everything" reference server.
    /// Ignored by default (needs network + npx). Run manually on Windows:
    ///   cargo test -p deepagent-app-core --lib -- --ignored everything
    #[cfg(windows)]
    #[tokio::test]
    #[ignore = "requires network + npx (Node)"]
    async fn connects_to_everything_reference_server() {
        let svc = service();
        let dto = McpServerDto {
            name: "everything".into(),
            transport: "stdio".into(),
            enabled: true,
            // On Windows `npx` is `npx.cmd`; spawn it via `cmd /c`.
            command: Some("cmd".into()),
            args: vec![
                "/c".into(),
                "npx".into(),
                "-y".into(),
                "@modelcontextprotocol/server-everything".into(),
            ],
            env: BTreeMap::new(),
            url: None,
            headers: BTreeMap::new(),
            source: "user".into(),
            source_plugin_id: None,
            source_plugin_name: None,
            declared_name: None,
            source_path: None,
            read_only: false,
            conflict: None,
        };
        let status = svc.test_server(dto).await.unwrap();
        assert_eq!(
            status.status, "connected",
            "connect error: {:?}",
            status.error
        );
        assert!(
            status.tools.iter().any(|t| t.name == "echo"),
            "expected an 'echo' tool, got: {:?}",
            status.tools.iter().map(|t| &t.name).collect::<Vec<_>>()
        );
    }
}
