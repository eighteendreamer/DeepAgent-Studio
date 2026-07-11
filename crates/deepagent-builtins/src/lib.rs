//! # deepagent-builtins
//!
//! The built-in tool set, aligned with Claude Code's first-party tools and
//! routed through the [`deepagent_tools`] capability registry (开发计划.md
//! Phase 4; Claude Code 复刻规范 §"工具系统"). These are the tools the agent
//! can use out of the box, every one of them gated by [`Permission`] and
//! [`RiskLevel`] and (for file tools) confined to a [`WorkspaceRoot`].
//!
//! ## Tool inventory
//!
//! | built-in     | Claude Code analogue | risk    | permission        |
//! |--------------|----------------------|---------|-------------------|
//! | `read_file`  | Read                 | Safe    | read-only         |
//! | `write_file` | Write                | Low     | WorkspaceWrite    |
//! | `edit_file`  | Edit                 | Low     | WorkspaceWrite    |
//! | `multi_edit` | MultiEdit            | Low     | WorkspaceWrite    |
//! | `list_dir`   | LS                   | Safe    | read-only         |
//! | `glob`       | Glob                 | Safe    | read-only         |
//! | `grep`       | Grep                 | Safe    | read-only         |
//! | `bash`       | Bash                 | Medium+ | ShellSafe         |
//! | `todo_write` | TodoWrite            | Safe    | read-only         |
//! | `task_list`  | TaskList             | Safe    | read-only         |
//! | `web_fetch`  | WebFetch             | Medium  | Network (http ft) |
//! | `web_search` | WebSearch            | Medium  | Network (http ft) |
//!
//! ## Safety model
//!
//! - **Path confinement** ([`fs_guard`]): file tools reject `..` traversal,
//!   absolute paths outside the root, and sensitive files (`.env`, keys).
//! - **Command safety** ([`bash_tool`]): the `bash` tool enforces a
//!   `Bash(prefix:*)` allow-list and refuses dangerous fragments (`rm -rf`,
//!   `curl | sh`, `sudo`, remote pushes) on its safe path, surfacing them for
//!   explicit approval.
//! - **No external deps**: glob matching ([`glob_match`]) is hand-rolled so the
//!   crate stays light and builds offline.
//!
//! ## Wiring
//!
//! Use [`register_builtins`] to install every built-in into a
//! [`ToolRegistry`], or the per-family helpers ([`file_tools::file_tools`]) for
//! finer control.

#![warn(missing_docs)]

pub mod ask_user_tool;
pub mod bash_tool;
pub mod classifier;
pub mod codegraph_tools;
pub mod file_cache;
pub mod file_tools;
pub mod fs_guard;
pub mod git_tools;
pub mod glob_match;
pub mod guard_hooks;
pub mod knowledge_tools;
pub mod office_tools;
pub mod plan_mode;
pub mod project_map_tools;
pub mod remote_tools;
pub mod skill_tool;
pub mod task_tool;
pub mod todo_tool;
pub mod tool_search;
pub mod web_tools;

#[cfg(feature = "http")]
pub mod reqwest_web;

use std::sync::Arc;

use deepagent_core::error::Result;
use deepagent_hooks::{HookPoint, HookRegistry};
use deepagent_tools::{Tool, ToolRegistry};

pub use ask_user_tool::{
    AskUserQuestionTool, DeclineResponder, Question, QuestionOption, QuestionResponder,
    ASK_USER_QUESTION_TOOL_NAME,
};
pub use bash_tool::{
    is_allowed, is_dangerous, BashTool, CommandExecutor, CommandOutcome, SystemExecutor,
};
pub use classifier::{
    ClassifierConfig, ClassifierRule, SafetyClassifier, SafetyVerdict, VerdictKind,
};
pub use codegraph_tools::{
    CodeGraphBackend, CodeGraphCalleesTool, CodeGraphCallersTool, CodeGraphExploreTool,
    CodeGraphImpactTool, CodeGraphLocateTool, CodeGraphNodeTool, CodeGraphSearchTool,
    CODEGRAPH_CALLEES_TOOL_NAME, CODEGRAPH_CALLERS_TOOL_NAME, CODEGRAPH_EXPLORE_TOOL_NAME,
    CODEGRAPH_IMPACT_TOOL_NAME, CODEGRAPH_LOCATE_TOOL_NAME, CODEGRAPH_NODE_TOOL_NAME,
    CODEGRAPH_SEARCH_TOOL_NAME,
};
pub use file_cache::{CachedFile, FileStateCache};
pub use file_tools::{
    file_tools, EditFileTool, GlobTool, GrepTool, ListDirTool, MultiEditTool, ReadFileTool,
    WriteFileTool,
};
pub use fs_guard::{is_sensitive_path, FsAccess, WorkspaceRoot};
pub use git_tools::{GitCommitTool, GitDiffTool, GitLogTool, GitStatusTool};
pub use glob_match::glob_match;
pub use guard_hooks::{BashGuardHook, PathGuardHook};
pub use knowledge_tools::{
    KnowledgeBackend, KnowledgeSearchTool, KnowledgeToolDraft, KnowledgeToolHit,
    KnowledgeWriteTool, UnavailableKnowledgeBackend, KNOWLEDGE_SEARCH_TOOL_NAME,
    KNOWLEDGE_WRITE_TOOL_NAME,
};
pub use office_tools::{
    OfficeBackend, OfficeDocxCreateTool, OfficeReadTool, OfficeXlsxCreateTool,
    OFFICE_DOCX_CREATE_TOOL_NAME, OFFICE_READ_TOOL_NAME, OFFICE_XLSX_CREATE_TOOL_NAME,
};
pub use plan_mode::{
    is_plan_safe_tool, EnterPlanModeTool, ExitPlanModeTool, PlanMode, PlanModeHook, PLAN_SAFE_TOOLS,
};
pub use project_map_tools::{
    CodeMapImpactTool, CodeMapNeighborsTool, CodeMapOverviewTool, CodeMapSearchTool,
    ProjectMapBackend, CODE_MAP_IMPACT_TOOL_NAME, CODE_MAP_NEIGHBORS_TOOL_NAME,
    CODE_MAP_OVERVIEW_TOOL_NAME, CODE_MAP_SEARCH_TOOL_NAME,
};
pub use remote_tools::{
    RemoteInstallArgs, RemoteInstallTool, RemoteOpsBackend, RemoteProbeArgs, RemoteProbeTool,
    RemotePushBundleArgs, RemotePushBundleTool, RemotePushFileArgs, RemotePushFileTool,
    RemoteRequireArgs, RemoteRequireTool, RemoteRuntimeRequirement, REMOTE_INSTALL_TOOL_NAME,
    REMOTE_PROBE_TOOL_NAME, REMOTE_PUSH_BUNDLE_TOOL_NAME, REMOTE_PUSH_FILE_TOOL_NAME,
    REMOTE_REQUIRE_TOOL_NAME,
};
pub use skill_tool::{SkillTool, SKILL_TOOL_NAME};
pub use task_tool::{
    SubagentRequest, SubagentRunner, TaskTool, UnavailableSubagentRunner, TASK_TOOL_NAME,
};
pub use todo_tool::{TaskListTool, TodoItem, TodoStatus, TodoStore, TodoWriteTool};
pub use tool_search::{
    is_deferred_tool, parse_tool_name, score_tool, DeferredToolSnapshot, ToolSearchMode,
    ToolSearchTool, TOOL_SEARCH_DEFAULT_MAX_RESULTS, TOOL_SEARCH_MAX_RESULTS_CAP,
    TOOL_SEARCH_TOOL_NAME,
};
pub use web_tools::{
    SearchAttempt, SearchResponse, SearchResult, WebClient, WebFetchTool, WebSearchTool,
};

#[cfg(feature = "http")]
pub use reqwest_web::{DeepSeekWebSearchConfig, ReqwestWebClient};

// Re-export the permission vocabulary callers need to grant access.
pub use deepagent_tools::permission::{Permission, PermissionSet, RiskLevel};

/// Configuration for assembling the built-in tool set.
pub struct BuiltinConfig {
    /// The confined workspace root for file tools.
    pub root: WorkspaceRoot,
    /// Working directory for the `bash` tool (usually the root path).
    pub bash_cwd: String,
    /// Allow-listed command prefixes for `bash` (`Bash(prefix:*)` semantics).
    pub bash_allow: Vec<String>,
    /// Shared session todo list backing `todo_write`.
    pub todo_store: TodoStore,
    /// Optional custom command executor for bash/git commands.
    /// When set, this overrides the default SystemExecutor.
    /// Used for remote execution over SSH or local software sandboxes.
    pub command_executor: Option<std::sync::Arc<dyn bash_tool::CommandExecutor>>,
}

impl BuiltinConfig {
    /// Build a config rooted at `root` with a `bash` allow-list. The bash cwd
    /// defaults to the root path and a fresh [`TodoStore`] is created.
    pub fn new(root: WorkspaceRoot, bash_allow: impl IntoIterator<Item = String>) -> Self {
        let bash_cwd = root.path().to_string_lossy().to_string();
        Self {
            root,
            bash_cwd,
            bash_allow: bash_allow.into_iter().collect(),
            todo_store: TodoStore::new(),
            command_executor: None,
        }
    }

    /// Set a custom command executor for bash/git commands.
    pub fn with_command_executor(
        mut self,
        executor: std::sync::Arc<dyn bash_tool::CommandExecutor>,
    ) -> Self {
        self.command_executor = Some(executor);
        self
    }
}

/// Assemble every built-in tool over the given config, using the real
/// [`SystemExecutor`] for `bash` unless a custom [`CommandExecutor`] was
/// supplied via [`BuiltinConfig::with_command_executor`].
///
/// Returns the tools plus the [`TodoStore`] so the caller can render the todo
/// list between turns.
pub fn builtin_tools(config: BuiltinConfig) -> (Vec<Arc<dyn Tool>>, TodoStore) {
    let BuiltinConfig {
        root,
        bash_cwd,
        bash_allow,
        todo_store,
        command_executor,
    } = config;

    let mut tools = file_tools(root);
    if let Some(exec) = command_executor {
        tools.push(Arc::new(BashTool::new(
            exec.clone(),
            bash_cwd.clone(),
            bash_allow,
        )));
        tools.push(Arc::new(GitStatusTool::new(exec.clone(), bash_cwd.clone())));
        tools.push(Arc::new(GitDiffTool::new(exec.clone(), bash_cwd.clone())));
        tools.push(Arc::new(GitLogTool::new(exec.clone(), bash_cwd.clone())));
        tools.push(Arc::new(GitCommitTool::new(exec, bash_cwd)));
    } else {
        tools.push(Arc::new(BashTool::new(
            SystemExecutor,
            bash_cwd.clone(),
            bash_allow,
        )));
        // Git tools (read-only status/diff/log + workspace-write commit). They run
        // through the same SystemExecutor as bash, rooted at the workspace dir.
        tools.push(Arc::new(GitStatusTool::new(
            SystemExecutor,
            bash_cwd.clone(),
        )));
        tools.push(Arc::new(GitDiffTool::new(SystemExecutor, bash_cwd.clone())));
        tools.push(Arc::new(GitLogTool::new(SystemExecutor, bash_cwd.clone())));
        tools.push(Arc::new(GitCommitTool::new(SystemExecutor, bash_cwd)));
    }
    tools.push(Arc::new(TodoWriteTool::new(todo_store.clone())));
    tools.push(Arc::new(TaskListTool::new(todo_store.clone())));
    (tools, todo_store)
}

/// Register every built-in tool into `registry`, returning the [`TodoStore`].
///
/// Tools are registered with the risk/permission metadata from their
/// descriptors; the registry then gates them per the agent's granted
/// permissions. Fails if any tool name collides with one already registered.
pub fn register_builtins(registry: &mut ToolRegistry, config: BuiltinConfig) -> Result<TodoStore> {
    let (tools, store) = builtin_tools(config);
    for tool in tools {
        registry.register(tool)?;
    }
    Ok(store)
}

/// Register the security-boundary guard hooks ([`PathGuardHook`] +
/// [`BashGuardHook`]) at [`HookPoint::BeforeToolUse`].
///
/// This enforces path confinement and command safety *centrally* at the
/// runtime's permission gate, so the boundary holds for every tool — built-in,
/// MCP, or custom — not just the file/bash built-ins themselves.
pub fn register_guard_hooks(
    hooks: &mut HookRegistry,
    root: WorkspaceRoot,
    bash_allow: impl IntoIterator<Item = String>,
) {
    // Full access (the root's FsAccess::Full mode) lets the bash guard pass
    // every command without prompting; otherwise the allow-list + danger
    // classifier apply (dangerous/unlisted commands ask for approval).
    let access = root.access();
    let full_access = access == fs_guard::FsAccess::Full;
    hooks.register(
        HookPoint::BeforeToolUse,
        Arc::new(PathGuardHook::new(root)),
    );
    hooks.register(
        HookPoint::BeforeToolUse,
        Arc::new(
            BashGuardHook::new(bash_allow)
                .with_full_access(full_access)
                .with_sandbox_mode(access),
        ),
    );
}

/// Register the network web tools (`web_fetch` + `web_search`) backed by the
/// real reqwest client. Requires the `http` feature. Both require the
/// [`Permission::Network`] grant to be invoked.
#[cfg(feature = "http")]
pub fn register_web_tools(registry: &mut ToolRegistry) -> Result<()> {
    registry.register(Arc::new(WebFetchTool::new(ReqwestWebClient::new())))?;
    registry.register(Arc::new(WebSearchTool::new(ReqwestWebClient::new())))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_config() -> (tempfile::TempDir, BuiltinConfig) {
        let dir = tempfile::tempdir().unwrap();
        let root = WorkspaceRoot::new(dir.path());
        let config = BuiltinConfig::new(root, ["git".to_string(), "cargo".to_string()]);
        (dir, config)
    }

    #[test]
    fn builtin_tools_includes_every_family() {
        let (_d, config) = temp_config();
        let (tools, _store) = builtin_tools(config);
        let names: Vec<String> = tools.iter().map(|t| t.descriptor().name).collect();
        for expected in [
            "read_file",
            "write_file",
            "edit_file",
            "multi_edit",
            "list_dir",
            "glob",
            "grep",
            "bash",
            "git_status",
            "git_diff",
            "git_log",
            "git_commit",
            "todo_write",
            "task_list",
        ] {
            assert!(names.contains(&expected.to_string()), "missing {expected}");
        }
        assert_eq!(tools.len(), 14);
    }

    #[test]
    fn register_builtins_populates_registry() {
        let (_d, config) = temp_config();
        let mut registry = ToolRegistry::new();
        let _store = register_builtins(&mut registry, config).unwrap();
        assert!(registry.get("read_file").is_some());
        assert!(registry.get("bash").is_some());
        assert!(registry.get("todo_write").is_some());
    }

    #[test]
    fn register_guard_hooks_adds_two_before_tool_use_hooks() {
        let dir = tempfile::tempdir().unwrap();
        let root = WorkspaceRoot::new(dir.path());
        let mut hooks = HookRegistry::new();
        register_guard_hooks(&mut hooks, root, ["git".to_string()]);
        assert_eq!(hooks.count_at(HookPoint::BeforeToolUse), 2);
    }
}
