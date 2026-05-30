//! Security-boundary hooks, registered at [`HookPoint::BeforeToolUse`].
//!
//! These re-express the in-tool safety checks ([`fs_guard`](crate::fs_guard),
//! [`bash_tool`](crate::bash_tool)) as `BeforeToolUse` hooks so the boundary is
//! enforced *centrally* — at the runtime's permission gate — regardless of how
//! a tool is invoked (built-in, MCP, or a future custom tool). This mirrors
//! Claude Code's `PreToolUse` guard and exercises the §13 permission protocol:
//!
//! - [`PathGuardHook`] — **denies** any tool call whose `path`/`file_path`
//!   argument escapes the workspace or targets a sensitive file.
//! - [`BashGuardHook`] — **denies** non-allow-listed `bash` commands and
//!   **asks** for approval on dangerous fragments (`rm -rf`, `curl | sh`, …).
//!
//! Both ignore tool calls that don't carry the arguments they guard, so they
//! compose freely with other hooks.

use async_trait::async_trait;

use deepagent_core::error::Result;
use deepagent_hooks::{DecisionSource, Hook, HookContext, HookData, HookOutcome};

use crate::bash_tool::{is_allowed, is_dangerous};
use crate::fs_guard::WorkspaceRoot;

/// Argument keys that carry a filesystem path across the built-in tools.
const PATH_KEYS: &[&str] = &["path", "file_path", "filename"];

/// Denies tool calls whose path argument escapes the workspace root or targets
/// a sensitive file. Registered at [`HookPoint::BeforeToolUse`].
///
/// [`HookPoint::BeforeToolUse`]: deepagent_hooks::HookPoint::BeforeToolUse
pub struct PathGuardHook {
    root: WorkspaceRoot,
}

impl PathGuardHook {
    /// Build over the confined workspace root.
    pub fn new(root: WorkspaceRoot) -> Self {
        Self { root }
    }
}

#[async_trait]
impl Hook for PathGuardHook {
    fn name(&self) -> &str {
        "path_guard"
    }

    async fn run(&self, ctx: &HookContext) -> Result<HookOutcome> {
        if let HookData::Tool { arguments, .. } = &ctx.data {
            for key in PATH_KEYS {
                if let Some(path) = arguments.get(*key).and_then(|v| v.as_str()) {
                    if let Err(e) = self.root.resolve(path) {
                        return Ok(HookOutcome::deny_from(
                            format!("path '{path}' rejected: {e}"),
                            DecisionSource::Policy,
                        ));
                    }
                }
            }
        }
        Ok(HookOutcome::Continue)
    }
}

/// Guards `bash` tool calls: denies commands outside the allow-list and asks
/// for approval on dangerous ones. Registered at [`HookPoint::BeforeToolUse`].
///
/// [`HookPoint::BeforeToolUse`]: deepagent_hooks::HookPoint::BeforeToolUse
pub struct BashGuardHook {
    allow: Vec<String>,
}

impl BashGuardHook {
    /// Build with the allow-listed command prefixes (`Bash(prefix:*)`).
    pub fn new(allow: impl IntoIterator<Item = String>) -> Self {
        Self {
            allow: allow.into_iter().collect(),
        }
    }

    /// The tool names this guard applies to.
    fn applies_to(name: &str) -> bool {
        name == "bash" || name == "shell"
    }
}

#[async_trait]
impl Hook for BashGuardHook {
    fn name(&self) -> &str {
        "bash_guard"
    }

    async fn run(&self, ctx: &HookContext) -> Result<HookOutcome> {
        if let HookData::Tool {
            name, arguments, ..
        } = &ctx.data
        {
            if !Self::applies_to(name) {
                return Ok(HookOutcome::Continue);
            }
            let Some(command) = arguments.get("command").and_then(|v| v.as_str()) else {
                return Ok(HookOutcome::Continue);
            };

            // Dangerous fragments require human approval (ask), classified by
            // the danger detector.
            if is_dangerous(command) {
                return Ok(HookOutcome::ask_from(
                    format!("command '{command}' is high-risk and needs approval"),
                    DecisionSource::Classifier,
                ));
            }
            // Not on the allow-list → deny by policy.
            if !is_allowed(command, &self.allow) {
                return Ok(HookOutcome::deny_from(
                    format!(
                        "command '{}' is not allow-listed",
                        command.split_whitespace().next().unwrap_or("")
                    ),
                    DecisionSource::Policy,
                ));
            }
        }
        Ok(HookOutcome::Continue)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use deepagent_core::id::SessionId;
    use deepagent_hooks::HookPoint;

    fn tool_ctx(name: &str, args: serde_json::Value) -> HookContext {
        HookContext::new(
            SessionId::nil(),
            HookPoint::BeforeToolUse,
            HookData::before_tool(name, args),
        )
    }

    #[tokio::test]
    async fn path_guard_denies_traversal_and_sensitive() {
        let guard = PathGuardHook::new(WorkspaceRoot::new("/work"));
        let out = guard
            .run(&tool_ctx(
                "read_file",
                serde_json::json!({"path": "../../etc/passwd"}),
            ))
            .await
            .unwrap();
        assert!(out.is_deny());

        let out = guard
            .run(&tool_ctx("read_file", serde_json::json!({"path": ".env"})))
            .await
            .unwrap();
        assert!(out.is_deny());
    }

    #[tokio::test]
    async fn path_guard_allows_clean_path() {
        let guard = PathGuardHook::new(WorkspaceRoot::new("/work"));
        let out = guard
            .run(&tool_ctx(
                "write_file",
                serde_json::json!({"path": "src/main.rs"}),
            ))
            .await
            .unwrap();
        assert_eq!(out, HookOutcome::Continue);
    }

    #[tokio::test]
    async fn path_guard_ignores_tools_without_path() {
        let guard = PathGuardHook::new(WorkspaceRoot::new("/work"));
        let out = guard
            .run(&tool_ctx(
                "bash",
                serde_json::json!({"command": "git status"}),
            ))
            .await
            .unwrap();
        assert_eq!(out, HookOutcome::Continue);
    }

    #[tokio::test]
    async fn bash_guard_denies_unlisted() {
        let guard = BashGuardHook::new(["git".to_string()]);
        let out = guard
            .run(&tool_ctx("bash", serde_json::json!({"command": "ls -la"})))
            .await
            .unwrap();
        assert!(out.is_deny());
    }

    #[tokio::test]
    async fn bash_guard_asks_on_dangerous() {
        let guard = BashGuardHook::new(["git".to_string()]);
        let out = guard
            .run(&tool_ctx(
                "bash",
                serde_json::json!({"command": "git push origin main"}),
            ))
            .await
            .unwrap();
        assert!(out.is_ask());
    }

    #[tokio::test]
    async fn bash_guard_allows_listed_safe() {
        let guard = BashGuardHook::new(["git".to_string(), "cargo".to_string()]);
        let out = guard
            .run(&tool_ctx(
                "bash",
                serde_json::json!({"command": "cargo test"}),
            ))
            .await
            .unwrap();
        assert_eq!(out, HookOutcome::Continue);
    }

    #[tokio::test]
    async fn bash_guard_ignores_non_bash_tools() {
        let guard = BashGuardHook::new(["git".to_string()]);
        let out = guard
            .run(&tool_ctx("read_file", serde_json::json!({"path": "x"})))
            .await
            .unwrap();
        assert_eq!(out, HookOutcome::Continue);
    }
}
