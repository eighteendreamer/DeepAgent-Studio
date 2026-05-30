//! Streamed chat orchestration (P1-C): run an agent and push live events.
//!
//! This is the connection layer between the kernel and the desktop UI's chat:
//! it assembles the tool registry (built-ins), a DeepSeek-backed [`ModelAgent`],
//! a [`RuntimeEngine`], and a [`ChannelSink`], then runs one turn-loop while
//! forwarding every [`RuntimeEvent`] to a caller-supplied callback (which the
//! Tauri layer bridges to `app.emit`, or a web layer to SSE/WS).
//!
//! The model client is built from the persisted [`ModelCatalog`] + the API key
//! from the secret store, so the UI only needs to call [`ChatService::run`].

use std::path::PathBuf;
use std::sync::Arc;

use deepagent_builtins::WorkspaceRoot;
use deepagent_core::clock::SystemClock;
use deepagent_core::error::{CoreError, Result};
use deepagent_hooks::{
    HookCommandRunner, HookPoint, HookRegistry, PermissionRulesHook, SystemHookRunner,
};
use deepagent_models::transport::HttpTransport;
use deepagent_models::{ModelClient, ModelConfig, ModelRole, ToolSchema};
use deepagent_persistence::Database;
use deepagent_runtime::{
    ChannelSink, ModelAgent, RuntimeConfig, RuntimeEngine, RuntimeEvent, RuntimeEventSink,
};
use deepagent_session::Session;
use deepagent_tools::{PermissionSet, ToolRegistry};

use crate::approval_bridge::{ChannelApprovalGate, PendingApprovals, PolicyGate};
use crate::dto::ApprovalRequestDto;
use crate::settings::SettingsService;

/// The system prompt seeded into every chat run (kept minimal here; the full
/// layered prompt assembly lives in `deepagent-prompts`).
const SYSTEM_PROMPT: &str = "You are DeepAgent, a verifiable Rust-native coding agent. \
Use the available tools to inspect and modify the workspace. Prefer read tools before write tools. \
Never run destructive commands without approval.";

/// Orchestrates streamed chat runs over the kernel.
pub struct ChatService {
    db: Arc<Database>,
    settings: Arc<SettingsService>,
    transport: Arc<dyn HttpTransport>,
    /// Default workspace root (the launch directory) used when no project
    /// registry is attached or no project is active.
    workspace: PathBuf,
    /// Allow-listed bash command prefixes.
    bash_allow: Vec<String>,
    /// Shared registry of in-flight approval requests (the UI resolves these).
    pending: PendingApprovals,
    /// Optional MCP server manager: when set, enabled MCP servers are connected
    /// at run time and their tools registered into the runtime tool registry.
    mcp: Option<Arc<crate::mcp_service::McpService>>,
    /// Optional project registry: when set, each run is rooted at (and the new
    /// session attached to) the **active** project's folder.
    projects: Option<Arc<crate::project_service::ProjectService>>,
}

impl ChatService {
    /// Build a chat service over the shared DB, settings, model transport, and
    /// workspace root.
    pub fn new(
        db: Arc<Database>,
        settings: Arc<SettingsService>,
        transport: Arc<dyn HttpTransport>,
        workspace: impl Into<PathBuf>,
    ) -> Self {
        Self {
            db,
            settings,
            transport,
            workspace: workspace.into(),
            bash_allow: default_bash_allow(),
            pending: PendingApprovals::new(),
            mcp: None,
            projects: None,
        }
    }

    /// Attach an [`McpService`](crate::mcp_service::McpService) so enabled MCP
    /// servers are connected and their tools live-registered on each run.
    pub fn with_mcp(mut self, mcp: Arc<crate::mcp_service::McpService>) -> Self {
        self.mcp = Some(mcp);
        self
    }

    /// Attach a [`ProjectService`](crate::project_service::ProjectService) so
    /// each run is rooted at the active project's folder and the new session is
    /// attached to it.
    pub fn with_projects(mut self, projects: Arc<crate::project_service::ProjectService>) -> Self {
        self.projects = Some(projects);
        self
    }

    /// The effective project root for this run: the active project's folder when
    /// a project registry is attached and a project is active, else the default
    /// launch workspace.
    fn effective_root(&self) -> PathBuf {
        if let Some(projects) = &self.projects {
            if let Ok(Some(active)) = projects.active() {
                if !active.trim().is_empty() {
                    return PathBuf::from(active);
                }
            }
        }
        self.workspace.clone()
    }

    /// The shared pending-approvals registry. The UI calls
    /// [`PendingApprovals::resolve_approved`] on this to answer a dialog.
    pub fn pending_approvals(&self) -> PendingApprovals {
        self.pending.clone()
    }

    /// Build the tool registry with the built-ins confined to `root`.
    fn build_registry(&self, root: &std::path::Path) -> Result<ToolRegistry> {
        use deepagent_builtins::{register_builtins, BuiltinConfig, WorkspaceRoot};
        let mut registry = ToolRegistry::new();
        let config = BuiltinConfig::new(
            WorkspaceRoot::new(root.to_path_buf()),
            self.bash_allow.clone(),
        );
        register_builtins(&mut registry, config)?;
        // Network web tools (web_fetch / web_search) when built with `web`.
        #[cfg(feature = "web")]
        deepagent_builtins::register_web_tools(&mut registry)?;
        Ok(registry)
    }

    /// Build a model client for the given role from persisted settings + the
    /// stored API key.
    fn build_model(&self, role: ModelRole) -> Result<(Arc<ModelClient>, String)> {
        let settings = self
            .settings
            .load()?
            .ok_or_else(|| CoreError::invalid("project not initialized: set an API key first"))?;
        let api_key = self
            .settings
            .api_key()?
            .ok_or_else(|| CoreError::invalid("API key not set: initialize the project first"))?;
        let model = settings.catalog.model_for(role).to_string();
        let config = ModelConfig::from_catalog(api_key, &settings.catalog, role);
        let client = Arc::new(ModelClient::new(self.transport.clone(), config));
        Ok((client, model))
    }

    /// Run one streamed chat turn-loop for `prompt`, forwarding every
    /// [`RuntimeEvent`] to `on_event` and any approval request to `on_approval`.
    /// Returns the new session id.
    ///
    /// Approval handling follows the persisted approval policy: `AutoReview` /
    /// `FullAccess` resolve automatically (no prompt); `AlwaysAsk` emits an
    /// [`ApprovalRequestDto`] via `on_approval` and the run **pauses** until the
    /// UI calls `resolve_approved` on [`ChatService::pending_approvals`].
    pub async fn run<F, A>(&self, prompt: &str, on_event: F, on_approval: A) -> Result<String>
    where
        F: Fn(RuntimeEvent) + Send + 'static,
        A: Fn(ApprovalRequestDto) + Send + Sync + 'static,
    {
        let root = self.effective_root();
        let registry = self.build_registry(&root)?;
        let (client, model) = self.build_model(ModelRole::Chat)?;
        let policy = self.settings.approval_policy()?;

        // Live MCP tool registration: connect enabled servers and register their
        // namespaced tools into the runtime registry, so they are advertised to
        // the model and routed like built-ins. Connection failures are
        // non-fatal (logged + skipped) so one bad server never blocks a run.
        let mut registry = registry;
        if let Some(mcp) = &self.mcp {
            match mcp.connect_enabled().await {
                Ok((mcp_registry, failures)) => {
                    if !failures.is_empty() {
                        tracing::warn!(
                            count = failures.len(),
                            "some MCP servers failed to connect"
                        );
                    }
                    let mcp_registry = std::sync::Arc::new(mcp_registry);
                    for adapter in deepagent_mcp::adapters_for(mcp_registry) {
                        if let Err(e) = registry.register(adapter) {
                            tracing::warn!(error = %e, "failed to register MCP tool adapter");
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, "MCP connect_enabled failed; continuing without MCP tools");
                }
            }
        }

        // Advertise the registry's visible tools to the model.
        let granted = PermissionSet::developer();
        let tools: Vec<ToolSchema> = registry
            .visible_to(&granted)
            .into_iter()
            .map(|d| ToolSchema::function(d.name, d.description, d.parameters))
            .collect();

        // Wire the event sink: a channel the loop emits into, drained by a task
        // that calls `on_event`.
        let (sink, mut rx) = ChannelSink::new();
        let sink: Arc<dyn RuntimeEventSink> = Arc::new(sink);
        let pump = tokio::spawn(async move {
            while let Some(ev) = rx.recv().await {
                on_event(ev);
            }
        });

        // Wire the approval gate: AlwaysAsk → channel gate (prompts the UI);
        // auto policies short-circuit to allow.
        let channel_gate = ChannelApprovalGate::new(self.pending.clone(), Arc::new(on_approval));
        let gate: Arc<dyn deepagent_runtime::ApprovalGate> =
            Arc::new(PolicyGate::new(policy, Arc::new(channel_gate)));

        // Wire hooks: declarative permission rules + path/bash safety guards +
        // declarative external hooks (hooks.json), all at their lifecycle
        // points. The rules resolve allow/ask/deny; the guards add
        // path-confinement and command-safety as a centralized boundary; the
        // external hooks run user/plugin-declared commands (e.g. a PreToolUse
        // validator that blocks dangerous bash via exit code 2).
        let rules = self.settings.permission_rules().unwrap_or_default();
        let mut hooks = HookRegistry::new();
        if !rules.is_empty() {
            hooks.register(
                HookPoint::BeforeToolUse,
                Arc::new(PermissionRulesHook::new(rules)),
            );
        }
        // Declarative external hooks from hooks.json (best-effort: malformed
        // JSON is logged and skipped rather than failing the run).
        match self.settings.hook_definitions() {
            Ok(defs) if !defs.is_empty() => {
                let runner: Arc<dyn HookCommandRunner> = Arc::new(SystemHookRunner);
                let n = defs.register_into(&mut hooks, runner);
                tracing::info!(count = n, "registered external hooks from hooks.json");
            }
            Ok(_) => {}
            Err(e) => {
                tracing::warn!(error = %e, "ignoring malformed hooks.json");
            }
        }
        deepagent_builtins::register_guard_hooks(
            &mut hooks,
            WorkspaceRoot::new(root.clone()),
            self.bash_allow.clone(),
        );

        let clock = SystemClock;
        // Bind the session to the active project (its folder path) so the
        // sidebar groups it under the right project folder.
        let project = root.to_string_lossy().into_owned();
        let mut session = Session::create_in_project(
            &self.db,
            &clock,
            Some(prompt),
            Default::default(),
            Some(&project),
        )?;
        let session_id = session.id().to_string();
        let task = session.create_task(prompt)?;

        let mut agent =
            ModelAgent::new(client, model, SYSTEM_PROMPT, prompt, tools).with_events(sink.clone());

        let config = RuntimeConfig {
            permissions: granted,
            ..Default::default()
        };
        let engine = RuntimeEngine::new(&registry, Default::default(), config)
            .with_events(sink)
            .with_approvals(gate)
            .with_hooks(&hooks);

        // Run the loop. Errors are surfaced as a terminal RunFailed event so the
        // UI always gets a clean end, then returned to the caller.
        let run_result = engine.run(&mut session, task, &mut agent).await;

        // Drop everything holding a clone of the event-sink sender so the
        // channel closes and the pump task can finish; then await it to ensure
        // all events were delivered.
        drop(engine);
        drop(agent);
        let _ = pump.await;

        run_result.map(|_| session_id)
    }
}

/// A conservative default bash allow-list (read-ish / build commands).
fn default_bash_allow() -> Vec<String> {
    [
        "git status",
        "git diff",
        "git log",
        "git show",
        "ls",
        "cat",
        "echo",
        "pwd",
        "cargo build",
        "cargo test",
        "cargo check",
        "cargo fmt",
        "cargo clippy",
        "npm run",
        "pnpm",
        "node",
        "python",
        "rustc",
    ]
    .into_iter()
    .map(String::from)
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secret_store::MemorySecretStore;
    use deepagent_models::transport::MockTransport;

    /// A transport that answers model discovery (GET) AND a streamed chat (the
    /// agent's first turn) so a full run completes offline.
    fn chat_transport() -> Arc<dyn HttpTransport> {
        // The mock streams its `events` for `stream`, and returns `get_response`
        // for discovery. We only need streaming here (settings are seeded
        // separately), so build one that completes immediately.
        Arc::new(MockTransport::new([
            r#"{"choices":[{"delta":{"content":"Hello from the agent."},"finish_reason":"stop"}]}"#
                .to_string(),
            "[DONE]".to_string(),
        ]))
    }

    fn discovery_transport() -> Arc<dyn HttpTransport> {
        let body = r#"{"object":"list","data":[
            {"id":"deepseek-chat","object":"model","owned_by":"deepseek"},
            {"id":"deepseek-reasoner","object":"model","owned_by":"deepseek"}
        ]}"#;
        Arc::new(MockTransport::with_get_json(body))
    }

    async fn seeded() -> (Arc<Database>, Arc<SettingsService>, tempfile::TempDir) {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let secrets = Arc::new(MemorySecretStore::new());
        let settings = Arc::new(SettingsService::new(
            db.clone(),
            discovery_transport(),
            secrets,
        ));
        settings.initialize("sk-test-1234").await.unwrap();
        let dir = tempfile::tempdir().unwrap();
        (db, settings, dir)
    }

    #[tokio::test]
    async fn streams_a_chat_run_end_to_end() {
        let (db, settings, dir) = seeded().await;
        let chat = ChatService::new(db, settings, chat_transport(), dir.path());

        let collected = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let sink = collected.clone();
        let session_id = chat
            .run(
                "say hello",
                move |ev| {
                    sink.lock().unwrap().push(ev.label().to_string());
                },
                |_approval| {},
            )
            .await
            .unwrap();

        assert!(session_id.starts_with("ses_"));
        let labels = collected.lock().unwrap().clone();
        assert_eq!(labels.first().map(String::as_str), Some("run_started"));
        assert!(labels.iter().any(|l| l == "content_delta"));
        assert_eq!(labels.last().map(String::as_str), Some("run_completed"));
    }

    #[tokio::test]
    async fn run_without_init_errors() {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let secrets = Arc::new(MemorySecretStore::new());
        let settings = Arc::new(SettingsService::new(
            db.clone(),
            discovery_transport(),
            secrets,
        ));
        let dir = tempfile::tempdir().unwrap();
        let chat = ChatService::new(db, settings, chat_transport(), dir.path());
        // No initialize() call → no settings → error.
        let res = chat.run("hi", |_| {}, |_| {}).await;
        assert!(res.is_err());
    }
}
