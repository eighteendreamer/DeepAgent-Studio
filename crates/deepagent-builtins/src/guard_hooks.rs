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
use crate::fs_guard::{FsAccess, WorkspaceRoot};

/// Argument keys that carry a filesystem path across the built-in tools.
const PATH_KEYS: &[&str] = &["path", "file_path", "filename"];

/// Tool names that only **read** the filesystem (vs. write/edit).
fn is_read_tool(name: &str) -> bool {
    matches!(name, "read_file" | "list_dir" | "glob" | "grep")
}

/// Denies (or asks approval for) tool calls whose path argument escapes the
/// workspace root or targets a sensitive file. Registered at
/// [`HookPoint::BeforeToolUse`]. Policy-aware via the root's [`FsAccess`] mode:
///
/// - Sensitive credential files are **always denied**.
/// - In-workspace paths always **continue**.
/// - Out-of-workspace **reads** continue under `ReadAnywhere`/`Full`, else deny.
/// - Out-of-workspace **writes** continue under `Full`, else deny.
///
/// Returning `Deny` makes the filesystem sandbox a hard boundary: approval
/// policy cannot expand the selected sandbox mode.
///
/// [`HookPoint::BeforeToolUse`]: deepagent_hooks::HookPoint::BeforeToolUse
pub struct PathGuardHook {
    root: WorkspaceRoot,
}

impl PathGuardHook {
    /// Build over the confined workspace root (carrying its access mode).
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
        if let HookData::Tool {
            name, arguments, ..
        } = &ctx.data
        {
            let read_tool = is_read_tool(name);
            for key in PATH_KEYS {
                if let Some(path) = arguments.get(*key).and_then(|v| v.as_str()) {
                    // Sensitive files are always denied outright, regardless of
                    // mode (never silently read/leak credentials).
                    if crate::fs_guard::is_sensitive_path(path) {
                        return Ok(HookOutcome::deny_from(
                            format!("access to sensitive file is denied: {path}"),
                            DecisionSource::Policy,
                        ));
                    }
                    // Out-of-workspace: does the access mode auto-allow it?
                    let allowed = if read_tool {
                        self.root.resolve_read(path).is_ok()
                    } else {
                        self.root.resolve_write(path).is_ok()
                    };
                    if allowed {
                        continue;
                    }
                    let verb = if read_tool { "read" } else { "modify" };
                    return Ok(HookOutcome::deny_from(
                        format!("{verb} is blocked by the current sandbox mode: '{path}'"),
                        DecisionSource::Policy,
                    ));
                }
            }
        }
        Ok(HookOutcome::Continue)
    }
}

/// Patterns that indicate a shell command will write to the filesystem via
/// redirections or pipe-to-file idioms. Used by `BashGuardHook` to deny
/// write-through-bash in ReadOnly/Workspace sandbox modes.
const WRITE_REDIRECT_PATTERNS: &[&str] = &[
    " > ",
    " >> ",
    ">|",
    " tee ",
    "<<EOF",
    "<<'EOF'",
    "<<\"EOF\"",
];

/// Detect whether a shell command contains file-write redirections or idioms.
fn has_write_redirect(command: &str) -> bool {
    // Check known patterns
    if WRITE_REDIRECT_PATTERNS.iter().any(|p| command.contains(p)) {
        return true;
    }
    // Also catch `>file` without leading space (e.g. `echo foo >out.txt`)
    // Look for `>` that isn't `>>` and isn't preceded by a digit (fd redirect like 2>&1)
    let bytes = command.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        if b == b'>' {
            // Skip `>>` (already covered above as " >> ")
            if i + 1 < bytes.len() && bytes[i + 1] == b'>' {
                continue;
            }
            // Skip fd redirects like `2>` or `2>&1`
            if i > 0 && bytes[i - 1].is_ascii_digit() {
                continue;
            }
            // Skip `>&` (fd duplication)
            if i + 1 < bytes.len() && bytes[i + 1] == b'&' {
                continue;
            }
            return true;
        }
    }
    false
}

/// Guards `bash` tool calls: denies commands outside the allow-list and asks
/// for approval on dangerous ones. Registered at [`HookPoint::BeforeToolUse`].
///
/// Policy-aware: under **full access** every command is allowed without prompt
/// (the user has opted into unrestricted computer operation). Otherwmands ask too (so the user can approve a one-off rather
/// than being hard-denied — "computer operations need manual approval").
///
/// Sandbox-aware: in **ReadOnly** mode, commands containing file-write
/// redirections (`>`, `>>`, `tee`, etc.) are denied outright so the model
/// cannot bypass the file-write restriction through shell idioms.
///
/// [`HookPoint::BeforeToolUse`]: deepagent_hooks::HookPoint::BeforeToolUse
pub struct BashGuardHook {
    allow: Vec<String>,
    full_access: bool,
    sandbox_mode: FsAccess,
}

impl BashGuardHook {
    /// Build with the allow-listed command prefixes (`Bash(prefix:*)`).
    pub fn new(allow: impl IntoIterator<Item = String>) -> Self {
        Self {
            allow: allow.into_iter().collect(),
            full_access: false,
            sandbox_mode: FsAccess::Workspace,
        }
    }

    /// Allow every command without prompting (builder style; for 完全访问).
    pub fn with_full_access(mut self, full_access: bool) -> Self {
        self.full_access = full_access;
        self
    }

    /// Set the sandbox mode so write-redirect detection can enforce it.
    pub fn with_sandbox_mode(mut self, mode: FsAccess) -> Self {
        self.sandbox_mode = mode;
        self
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
            // Full access: the user has opted into unrestricted shell. Let the
            // command through without prompting.
            if self.full_access {
                return Ok(HookOutcome::Continue);
            }
            let Some(command) = arguments.get("command").and_then(|v| v.as_str()) else {
                return Ok(HookOutcome::Continue);
            };

            // Escalation protocol: if the model requests `require_escalated`,
            // surface the justification to the user for approval rather than
            // applying the normal sandbox deny/ask logic.
            let escalated = arguments
                .get("sandbox_permissions")
                .and_then(|v| v.as_str())
                == Some("require_escalated");
            if escalated {
                let justification = arguments
                    .get("justification")
                    .and_then(|v| v.as_str())
                    .unwrap_or(
                        "This command needs elevated permissions to run outside the sandbox.",
                    );
                return Ok(HookOutcome::ask_from(
                    format!("[Sandbox escalation] {justification}\nCommand: {command}"),
                    DecisionSource::Policy,
                ));
            }

            // Sandbox write-redirect check: in ReadOnly mode, deny commands
            // that contain file-write shell idioms (>, >>, tee, etc.) so the
            // model cannot bypass the sandbox through bash.
            if self.sandbox_mode == FsAccess::ReadOnly && has_write_redirect(command) {
                return Ok(HookOutcome::deny_from(
                    format!(
                        "command contains file-write operations which are blocked in read-only sandbox mode: '{command}'"
                    ),
                    DecisionSource::Policy,
                ));
            }

            // Dangerous fragments require human approval (ask), classified by
            // the danger detector.
            if is_dangerous(command) {
                return Ok(HookOutcome::ask_from(
                    format!("command '{command}' is high-risk and needs approval"),
                    DecisionSource::Classifier,
                ));
            }
            // Not on the allow-list → a computer operation that needs the user's
            // explicit OK. Ask (rather than hard-deny) so it can be approved.
            if !is_allowed(command, &self.allow) {
                return Ok(HookOutcome::ask_from(
                    format!(
                        "command '{}' runs on your computer and needs approval",
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
    async fn path_guard_denies_sensitive_and_outside_workspace() {
        let guard = PathGuardHook::new(WorkspaceRoot::new("/work"));
        // Out-of-workspace read under the default (Workspace) mode is denied.
        let out = guard
            .run(&tool_ctx(
                "read_file",
                serde_json::json!({"path": "../../etc/passwd"}),
            ))
            .await
            .unwrap();
        assert!(out.is_deny());

        // Sensitive credential file → always denied.
        let out = guard
            .run(&tool_ctx("read_file", serde_json::json!({"path": ".env"})))
            .await
            .unwrap();
        assert!(out.is_deny());
    }

    #[tokio::test]
    async fn path_guard_read_anywhere_allows_outside_reads() {
        use crate::fs_guard::FsAccess;
        let guard =
            PathGuardHook::new(WorkspaceRoot::new("/work").with_access(FsAccess::ReadAnywhere));
        // Out-of-workspace read auto-allowed under ReadAnywhere.
        let out = guard
            .run(&tool_ctx(
                "read_file",
                serde_json::json!({"path": "/etc/hosts"}),
            ))
            .await
            .unwrap();
        assert_eq!(out, HookOutcome::Continue);
        // But an out-of-workspace WRITE still asks.
        let out = guard
            .run(&tool_ctx(
                "write_file",
                serde_json::json!({"path": "/etc/hosts"}),
            ))
            .await
            .unwrap();
        assert!(out.is_deny());
    }

    #[tokio::test]
    async fn path_guard_read_only_denies_in_workspace_writes() {
        use crate::fs_guard::FsAccess;
        let guard = PathGuardHook::new(WorkspaceRoot::new("/work").with_access(FsAccess::ReadOnly));
        let out = guard
            .run(&tool_ctx(
                "write_file",
                serde_json::json!({"path": "src/main.rs"}),
            ))
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
    async fn bash_guard_asks_on_unlisted() {
        let guard = BashGuardHook::new(["git".to_string()]);
        let out = guard
            .run(&tool_ctx("bash", serde_json::json!({"command": "ls -la"})))
            .await
            .unwrap();
        assert!(out.is_ask());
    }

    #[tokio::test]
    async fn bash_guard_full_access_allows_everything() {
        let guard = BashGuardHook::new(["git".to_string()]).with_full_access(true);
        let out = guard
            .run(&tool_ctx(
                "bash",
                serde_json::json!({"command": "rm -rf /tmp/x"}),
            ))
            .await
            .unwrap();
        assert_eq!(out, HookOutcome::Continue);
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

    #[tokio::test]
    async fn bash_guard_readonly_denies_write_redirect() {
        use crate::fs_guard::FsAccess;
        let guard = BashGuardHook::new(["echo".to_string()]).with_sandbox_mode(FsAccess::ReadOnly);
        // `echo > file` should be denied
        let out = guard
            .run(&tool_ctx(
                "bash",
                serde_json::json!({"command": "echo hello > output.txt"}),
            ))
            .await
            .unwrap();
        assert!(out.is_deny());
    }

    #[tokio::test]
    async fn bash_guard_readonly_denies_append_redirect() {
        use crate::fs_guard::FsAccess;
        let guard = BashGuardHook::new(["echo".to_string()]).with_sandbox_mode(FsAccess::ReadOnly);
        let out = guard
            .run(&tool_ctx(
                "bash",
                serde_json::json!({"command": "echo hello >> output.txt"}),
            ))
            .await
            .unwrap();
        assert!(out.is_deny());
    }

    #[tokio::test]
    async fn bash_guard_readonly_denies_tee() {
        use crate::fs_guard::FsAccess;
        let guard = BashGuardHook::new(["cat".to_string()]).with_sandbox_mode(FsAccess::ReadOnly);
        let out = guard
            .run(&tool_ctx(
                "bash",
                serde_json::json!({"command": "cat file | tee output.txt"}),
            ))
            .await
            .unwrap();
        assert!(out.is_deny());
    }

    #[tokio::test]
    async fn bash_guard_readonly_allows_safe_command() {
        use crate::fs_guard::FsAccess;
        let guard = BashGuardHook::new(["git".to_string(), "cargo".to_string()])
            .with_sandbox_mode(FsAccess::ReadOnly);
        // `git status` has no write redirects → allowed
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
    async fn bash_guard_workspace_mode_allows_write_redirect() {
        use crate::fs_guard::FsAccess;
        let guard = BashGuardHook::new(["echo".to_string()]).with_sandbox_mode(FsAccess::Workspace);
        // In Workspace mode, write redirects are allowed (Sandboxie handles confinement)
        let out = guard
            .run(&tool_ctx(
                "bash",
                serde_json::json!({"command": "echo hello > output.txt"}),
            ))
            .await
            .unwrap();
        assert_eq!(out, HookOutcome::Continue);
    }

    #[tokio::test]
    async fn has_write_redirect_detects_patterns() {
        assert!(super::has_write_redirect("echo foo > bar.txt"));
        assert!(super::has_write_redirect("echo foo >> bar.txt"));
        assert!(super::has_write_redirect("cat x | tee y.txt"));
        assert!(super::has_write_redirect("cat <<EOF\nhello\nEOF"));
        assert!(super::has_write_redirect("echo x >out.txt"));
        // Should NOT trigger on stderr redirect or fd dup
        assert!(!super::has_write_redirect("cmd 2>&1"));
        assert!(!super::has_write_redirect("git status"));
        assert!(!super::has_write_redirect("cargo test"));
    }

    #[tokio::test]
    async fn bash_guard_escalation_asks_user() {
        use crate::fs_guard::FsAccess;
        let guard = BashGuardHook::new(["echo".to_string()]).with_sandbox_mode(FsAccess::ReadOnly);
        let out = guard
            .run(&tool_ctx(
                "bash",
                serde_json::json!({
                    "command": "echo hello > output.txt",
                    "sandbox_permissions": "require_escalated",
                    "justification": "Need to create the output file for the build"
                }),
            ))
            .await
            .unwrap();
        // Escalation should ASK (not deny), even in ReadOnly mode
        assert!(out.is_ask());
    }

    #[tokio::test]
    async fn bash_guard_escalation_without_justification_still_asks() {
        let guard = BashGuardHook::new(["echo".to_string()]);
        let out = guard
            .run(&tool_ctx(
                "bash",
                serde_json::json!({
                    "command": "echo hello > output.txt",
                    "sandbox_permissions": "require_escalated"
                }),
            ))
            .await
            .unwrap();
        assert!(out.is_ask());
    }
}
