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

use std::collections::BTreeMap;
use std::sync::Arc;

use deepagent_core::clock::{Clock, SystemClock};
use deepagent_core::error::{CoreError, Result};
use deepagent_mcp::config::{McpServerConfig, TransportType};
use deepagent_mcp::{connect_transport, McpClient, McpConfig, McpRegistry};
use deepagent_persistence::document_store::DocumentStore;
use deepagent_persistence::Database;
use serde::{Deserialize, Serialize};

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
        }
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

/// UI-facing MCP server management.
pub struct McpService {
    db: Arc<Database>,
}

impl McpService {
    /// Build over the shared database.
    pub fn new(db: Arc<Database>) -> Self {
        Self { db }
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

    /// List all configured servers as DTOs (sorted by name).
    pub fn list(&self) -> Result<Vec<McpServerDto>> {
        let state = self.load_state()?;
        Ok(state
            .config
            .servers
            .iter()
            .map(|(name, cfg)| {
                let enabled = state.enabled.get(name).copied().unwrap_or(true);
                McpServerDto::from_config(name, cfg, enabled)
            })
            .collect())
    }

    /// Get a single server by name.
    pub fn get(&self, name: &str) -> Result<Option<McpServerDto>> {
        Ok(self.list()?.into_iter().find(|s| s.name == name))
    }

    /// Add or update a server (upsert by name). Validates the config.
    pub fn upsert(&self, dto: McpServerDto) -> Result<McpServerDto> {
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
        let mut config = self.enabled_config()?;
        // Expand ${VAR} from the environment (headers/urls/args/env).
        config.expand_with(&|var| std::env::var(var).ok());

        let mut registry = McpRegistry::new();
        let mut failures = Vec::new();

        for (name, cfg) in &config.servers {
            match Self::connect_one(name, cfg).await {
                Ok(client) => {
                    if let Err(e) = registry.register(name, client).await {
                        failures.push((name.clone(), e.to_string()));
                    }
                }
                Err(e) => {
                    tracing::warn!(server = name.as_str(), error = %e, "MCP connect failed; skipping");
                    failures.push((name.clone(), e.to_string()));
                }
            }
        }
        Ok((registry, failures))
    }

    /// Connect a single server and perform the initialize handshake.
    async fn connect_one(name: &str, cfg: &McpServerConfig) -> Result<Arc<McpClient>> {
        let transport = connect_transport(cfg)?;
        let client = Arc::new(McpClient::new(transport));
        client.initialize("deepagent-studio").await?;
        tracing::info!(server = name, "MCP server initialized");
        Ok(client)
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
}
