//! Declarative permission rules (Phase B — `permissions.{allow,ask,deny}`).
//!
//! Mirrors Claude Code's `settings.json` permission rules: instead of writing a
//! Rust hook per policy, the user declares lists of patterns and the matching
//! [`PermissionRules`] become a `BeforeToolUse` [`Hook`]
//! ([`PermissionRulesHook`]) that resolves each call to allow / ask / deny.
//!
//! ## Pattern syntax
//!
//! A rule pattern matches a tool call. Two forms (aligned with Claude Code's
//! `Bash(git:*)` syntax):
//! - `Tool` — matches any call to that tool (e.g. `WebFetch`, `bash`).
//! - `Tool(prefix:*)` — matches a `bash`/`shell` call whose **command** begins
//!   with `prefix` (e.g. `Bash(git:*)` matches `git status`; `Bash(rm:*)`
//!   matches `rm -rf`). The `:*` suffix is optional (`Bash(git)` works too).
//!
//! ## Precedence
//!
//! `deny` > `ask` > `allow`. A call matching a deny pattern is denied even if it
//! also matches an allow pattern. A call matching nothing falls through to
//! `Continue` (the runtime's risk-based gate still applies).

use serde::{Deserialize, Serialize};

use async_trait::async_trait;

use deepagent_core::error::Result;

use crate::hook::{DecisionSource, Hook, HookOutcome};
use crate::lifecycle::{HookContext, HookData};

/// A declarative set of permission rules (the persisted `permissions` block).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionRules {
    /// Patterns that are explicitly allowed (skip the risk gate).
    #[serde(default)]
    pub allow: Vec<String>,
    /// Patterns that require approval (`ask`).
    #[serde(default)]
    pub ask: Vec<String>,
    /// Patterns that are denied outright.
    #[serde(default)]
    pub deny: Vec<String>,
}

impl PermissionRules {
    /// Build from explicit lists.
    pub fn new(
        allow: impl IntoIterator<Item = String>,
        ask: impl IntoIterator<Item = String>,
        deny: impl IntoIterator<Item = String>,
    ) -> Self {
        Self {
            allow: allow.into_iter().collect(),
            ask: ask.into_iter().collect(),
            deny: deny.into_iter().collect(),
        }
    }

    /// Whether there are no rules at all.
    pub fn is_empty(&self) -> bool {
        self.allow.is_empty() && self.ask.is_empty() && self.deny.is_empty()
    }

    /// Resolve a tool call (by name + args) into a decision. Returns `None` when
    /// no rule matches (the caller should fall through to `Continue`).
    pub fn evaluate(&self, tool: &str, arguments: &serde_json::Value) -> Option<RuleDecision> {
        // Precedence: deny > ask > allow.
        if self
            .deny
            .iter()
            .any(|p| matches_pattern(p, tool, arguments))
        {
            return Some(RuleDecision::Deny);
        }
        if self.ask.iter().any(|p| matches_pattern(p, tool, arguments)) {
            return Some(RuleDecision::Ask);
        }
        if self
            .allow
            .iter()
            .any(|p| matches_pattern(p, tool, arguments))
        {
            return Some(RuleDecision::Allow);
        }
        None
    }
}

/// The outcome of evaluating the rules against a call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleDecision {
    /// Explicitly allowed.
    Allow,
    /// Requires approval.
    Ask,
    /// Denied.
    Deny,
}

/// Whether `pattern` matches a call to `tool` with `arguments`.
///
/// Case-insensitive on the tool name (so `Bash`/`bash` both match the `bash`
/// tool, and `WebFetch`/`web_fetch` are normalized).
fn matches_pattern(pattern: &str, tool: &str, arguments: &serde_json::Value) -> bool {
    let Some((pat_tool, inner)) = rule_parts(pattern) else {
        return false;
    };

    if !tool_names_match(pat_tool, tool) {
        return false;
    }

    match inner {
        None => true, // bare tool pattern matches any call to that tool
        Some(prefix) => {
            // Command-prefix match against the bash/shell `command` arg.
            let command = arguments
                .get("command")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim();
            command == prefix
                || command.starts_with(&format!("{prefix} "))
                || command.starts_with(&format!("{prefix}\t"))
        }
    }
}

/// The `(tool, inner-spec)` halves of a rule pattern, with the Claude Code `:*`
/// (or bare `*`) suffix already stripped from the inner spec.
///
/// `WebFetch` → `Some(("WebFetch", None))`; `Bash(git:*)` → `Some(("Bash",
/// Some("git")))`; a blank pattern → `None`. An empty inner spec (`Bash()`)
/// normalizes to `None`, i.e. "any call to that tool", which is what matching
/// already does.
pub fn rule_parts(pattern: &str) -> Option<(&str, Option<&str>)> {
    let pattern = pattern.trim();
    if pattern.is_empty() {
        return None;
    }
    match pattern.split_once('(') {
        Some((tool, rest)) => {
            let inner = rest.strip_suffix(')').unwrap_or(rest).trim();
            let inner = inner.strip_suffix(":*").unwrap_or(inner);
            let inner = inner.strip_suffix('*').unwrap_or(inner).trim();
            Some((tool.trim(), (!inner.is_empty()).then_some(inner)))
        }
        None => Some((pattern, None)),
    }
}

/// The rule text a command-prefix grant is stored as (`p` in an approval prompt).
pub fn bash_prefix_rule(prefix: &str) -> String {
    format!("Bash({prefix}:*)")
}

/// The command prefix `pattern` authorizes, or `Some("")` when it authorizes
/// every command (`bash`). `None` when the pattern is about another tool.
pub fn rule_command_prefix(pattern: &str) -> Option<&str> {
    let (pat_tool, inner) = rule_parts(pattern)?;
    if !tool_names_match(pat_tool, "bash") {
        return None;
    }
    Some(inner.unwrap_or(""))
}

/// Compare a pattern's tool name to an actual tool name, tolerating the
/// Claude-Code PascalCase aliases (Bash/Read/Write/...) vs our snake_case names.
fn tool_names_match(pattern_tool: &str, actual: &str) -> bool {
    let norm = |s: &str| s.to_lowercase().replace(['_', '-'], "");
    let p = norm(pattern_tool);
    let a = norm(actual);
    if p == a {
        return true;
    }
    // A few well-known aliases (Claude Code name → our tool name).
    matches!(
        (p.as_str(), a.as_str()),
        ("bash", "shell")
            | ("shell", "bash")
            | ("webfetch", "webfetch")
            | ("websearch", "websearch")
            | ("multiedit", "multiedit")
            | ("edit", "editfile")
            | ("write", "writefile")
            | ("read", "readfile")
            | ("ls", "listdir")
    )
}

/// A `BeforeToolUse` hook backed by declarative [`PermissionRules`].
pub struct PermissionRulesHook {
    rules: PermissionRules,
}

impl PermissionRulesHook {
    /// Build from rules.
    pub fn new(rules: PermissionRules) -> Self {
        Self { rules }
    }
}

#[async_trait]
impl Hook for PermissionRulesHook {
    fn name(&self) -> &str {
        "permission_rules"
    }

    async fn run(&self, ctx: &HookContext) -> Result<HookOutcome> {
        if let HookData::Tool {
            name, arguments, ..
        } = &ctx.data
        {
            match self.rules.evaluate(name, arguments) {
                Some(RuleDecision::Deny) => {
                    return Ok(HookOutcome::deny_from(
                        format!("tool '{name}' denied by permission rule"),
                        DecisionSource::Policy,
                    ));
                }
                Some(RuleDecision::Ask) => {
                    return Ok(HookOutcome::ask_from(
                        format!("tool '{name}' requires approval by permission rule"),
                        DecisionSource::Policy,
                    ));
                }
                // Allow / no-match → let other gates decide.
                Some(RuleDecision::Allow) | None => {}
            }
        }
        Ok(HookOutcome::Continue)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bash(cmd: &str) -> serde_json::Value {
        serde_json::json!({ "command": cmd })
    }

    #[test]
    fn bare_tool_pattern_matches_any_call() {
        let r = PermissionRules::new([], [], ["web_fetch".to_string()]);
        assert_eq!(
            r.evaluate("web_fetch", &serde_json::json!({})),
            Some(RuleDecision::Deny)
        );
        assert_eq!(r.evaluate("read_file", &serde_json::json!({})), None);
    }

    #[test]
    fn bash_prefix_pattern() {
        let r = PermissionRules::new(["Bash(git:*)".to_string()], [], []);
        assert_eq!(
            r.evaluate("bash", &bash("git status")),
            Some(RuleDecision::Allow)
        );
        assert_eq!(r.evaluate("bash", &bash("git")), Some(RuleDecision::Allow));
        assert_eq!(r.evaluate("bash", &bash("npm install")), None);
    }

    #[test]
    fn deny_beats_allow() {
        let r = PermissionRules::new(
            ["Bash(git:*)".to_string()],
            [],
            ["Bash(git push:*)".to_string()],
        );
        // git status → allowed; git push → denied (deny wins).
        assert_eq!(
            r.evaluate("bash", &bash("git status")),
            Some(RuleDecision::Allow)
        );
        assert_eq!(
            r.evaluate("bash", &bash("git push origin main")),
            Some(RuleDecision::Deny)
        );
    }

    #[test]
    fn ask_beats_allow() {
        let r = PermissionRules::new(["bash".to_string()], ["Bash(rm:*)".to_string()], []);
        assert_eq!(r.evaluate("bash", &bash("ls")), Some(RuleDecision::Allow));
        assert_eq!(
            r.evaluate("bash", &bash("rm -rf x")),
            Some(RuleDecision::Ask)
        );
    }

    #[test]
    fn claude_code_alias_names() {
        // "Bash" pattern matches our "bash"; "Read" matches "read_file".
        let r = PermissionRules::new([], [], ["Read".to_string()]);
        assert_eq!(
            r.evaluate("read_file", &serde_json::json!({"path": "x"})),
            Some(RuleDecision::Deny)
        );
    }

    #[tokio::test]
    async fn hook_denies_and_asks() {
        use crate::lifecycle::{HookData, HookPoint};
        use deepagent_core::id::SessionId;

        let rules = PermissionRules::new([], ["Bash(rm:*)".to_string()], ["WebSearch".to_string()]);
        let hook = PermissionRulesHook::new(rules);

        let ctx_deny = HookContext::new(
            SessionId::nil(),
            HookPoint::BeforeToolUse,
            HookData::before_tool("web_search", serde_json::json!({"query": "x"})),
        );
        assert!(hook.run(&ctx_deny).await.unwrap().is_deny());

        let ctx_ask = HookContext::new(
            SessionId::nil(),
            HookPoint::BeforeToolUse,
            HookData::before_tool("bash", bash("rm -rf /")),
        );
        assert!(hook.run(&ctx_ask).await.unwrap().is_ask());

        let ctx_pass = HookContext::new(
            SessionId::nil(),
            HookPoint::BeforeToolUse,
            HookData::before_tool("bash", bash("ls")),
        );
        assert_eq!(hook.run(&ctx_pass).await.unwrap(), HookOutcome::Continue);
    }

    #[test]
    fn empty_rules() {
        assert!(PermissionRules::default().is_empty());
        assert_eq!(
            PermissionRules::default().evaluate("bash", &bash("anything")),
            None
        );
    }

    #[test]
    fn serde_roundtrip() {
        let r = PermissionRules::new(
            ["bash".to_string()],
            ["Bash(rm:*)".to_string()],
            ["WebFetch".to_string()],
        );
        let json = serde_json::to_string(&r).unwrap();
        let back: PermissionRules = serde_json::from_str(&json).unwrap();
        assert_eq!(r, back);
    }

    #[test]
    fn rule_grammar_reads_a_prefix_back_out_of_the_rule_it_wrote() {
        assert_eq!(
            rule_parts("Bash(git status:*)"),
            Some(("Bash", Some("git status")))
        );
        assert_eq!(rule_parts("Bash(npm run)"), Some(("Bash", Some("npm run"))));
        assert_eq!(rule_parts("bash"), Some(("bash", None)));
        // An empty inner spec means "any call", same as a bare tool rule.
        assert_eq!(rule_parts("Bash()"), Some(("Bash", None)));
        assert_eq!(rule_parts("   "), None);

        let written = bash_prefix_rule("git status");
        assert_eq!(rule_command_prefix(&written), Some("git status"));
        assert_eq!(rule_command_prefix("Read"), None);
        assert_eq!(rule_command_prefix("shell"), Some(""));
        // The alias spellings are the same rule.
        assert_eq!(rule_command_prefix("Shell(curl:*)"), Some("curl"));
        assert_eq!(
            rule_command_prefix("BASH(git status:*)"),
            Some("git status")
        );
    }

    #[test]
    fn prefix_matching_survives_the_grammar_extraction() {
        let r = PermissionRules::new([bash_prefix_rule("git status")], [], []);
        assert_eq!(
            r.evaluate("bash", &bash("git status")),
            Some(RuleDecision::Allow)
        );
        assert_eq!(
            r.evaluate("bash", &bash("git status --short")),
            Some(RuleDecision::Allow)
        );
        // Token boundary: a longer word is not the same command.
        assert_eq!(r.evaluate("bash", &bash("git statusx")), None);
    }
}
