//! Auto-mode safety classifier (Phase 1A — gap-closure spec).
//!
//! When the approval policy is "auto review" (自动审核), the system needs to
//! decide **per tool call** whether to auto-approve or prompt the user. The
//! current hardcoded `is_dangerous` / `is_allowed` logic is too coarse: it
//! blocks `curl localhost` (harmless) and asks for every unlisted command.
//!
//! This module provides a configurable, rule-based [`SafetyClassifier`] that
//! replaces the hardcoded logic with a nuanced decision:
//!
//! - **Allow** — safe, auto-approve without prompting.
//! - **Ask** — uncertain or mildly risky, prompt the user.
//! - **Deny** — always blocked (sensitive files, destructive ops).
//!
//! The classifier is driven by a [`ClassifierConfig`] (loaded from
//! `.deepagent/classifier.json` when present, else built-in defaults). Rules
//! are evaluated in priority order: deny > ask > allow > fallback.

use serde::{Deserialize, Serialize};

/// The classifier's verdict for a single tool call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SafetyVerdict {
    /// Safe to auto-approve.
    Allow,
    /// Needs user confirmation (with a reason).
    Ask(String),
    /// Hard-denied (with a reason).
    Deny(String),
}

/// A single classification rule: matches a tool name pattern + optional
/// argument content pattern, and maps to a verdict.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClassifierRule {
    /// Tool name glob (e.g. "bash", "write_file", "*").
    pub tool: String,
    /// Optional: if present, the rule only fires when the stringified arguments
    /// contain this substring (case-insensitive).
    #[serde(default)]
    pub args_contains: Option<String>,
    /// The verdict when this rule matches.
    pub verdict: VerdictKind,
    /// Human reason (shown in the approval dialog or deny message).
    #[serde(default)]
    pub reason: String,
}

/// The kind of verdict a rule produces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerdictKind {
    /// Auto-approve.
    Allow,
    /// Prompt the user.
    Ask,
    /// Hard-deny.
    Deny,
}

/// Configuration for the safety classifier: ordered rule lists evaluated in
/// priority (deny checked first, then ask, then allow). Anything not matching
/// any rule falls through to the `fallback` verdict.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClassifierConfig {
    /// Rules that deny (highest priority, checked first).
    #[serde(default)]
    pub deny: Vec<ClassifierRule>,
    /// Rules that ask for approval.
    #[serde(default)]
    pub ask: Vec<ClassifierRule>,
    /// Rules that auto-allow.
    #[serde(default)]
    pub allow: Vec<ClassifierRule>,
    /// What to do when no rule matches (default: ask).
    #[serde(default = "default_fallback")]
    pub fallback: VerdictKind,
}

fn default_fallback() -> VerdictKind {
    VerdictKind::Ask
}

impl Default for ClassifierConfig {
    fn default() -> Self {
        Self {
            deny: default_deny_rules(),
            ask: default_ask_rules(),
            allow: default_allow_rules(),
            fallback: VerdictKind::Ask,
        }
    }
}

/// The safety classifier: evaluates rules against a tool call and returns a
/// verdict. Stateless and cheap to call on every BeforeToolUse.
#[derive(Debug, Clone)]
pub struct SafetyClassifier {
    config: ClassifierConfig,
}

impl SafetyClassifier {
    /// Build from a config.
    pub fn new(config: ClassifierConfig) -> Self {
        Self { config }
    }

    /// Build with the built-in default rules.
    pub fn with_defaults() -> Self {
        Self::new(ClassifierConfig::default())
    }

    /// Classify a tool call. `tool` is the tool name; `args` is the JSON
    /// arguments object. Returns the verdict.
    pub fn classify(&self, tool: &str, args: &serde_json::Value) -> SafetyVerdict {
        let args_str = args.to_string().to_lowercase();

        // Priority 1: deny rules.
        for rule in &self.config.deny {
            if matches_rule(rule, tool, &args_str) {
                let reason = if rule.reason.is_empty() {
                    format!("denied by classifier rule: {}", rule.tool)
                } else {
                    rule.reason.clone()
                };
                return SafetyVerdict::Deny(reason);
            }
        }

        // Priority 2: ask rules.
        for rule in &self.config.ask {
            if matches_rule(rule, tool, &args_str) {
                let reason = if rule.reason.is_empty() {
                    format!("needs approval: {}", rule.tool)
                } else {
                    rule.reason.clone()
                };
                return SafetyVerdict::Ask(reason);
            }
        }

        // Priority 3: allow rules.
        for rule in &self.config.allow {
            if matches_rule(rule, tool, &args_str) {
                return SafetyVerdict::Allow;
            }
        }

        // Fallback.
        match self.config.fallback {
            VerdictKind::Allow => SafetyVerdict::Allow,
            VerdictKind::Ask => SafetyVerdict::Ask("no matching rule; needs approval".into()),
            VerdictKind::Deny => SafetyVerdict::Deny("no matching rule; denied by default".into()),
        }
    }

    /// The underlying config (for serialization / display).
    pub fn config(&self) -> &ClassifierConfig {
        &self.config
    }
}

/// Whether a rule matches a tool call.
fn matches_rule(rule: &ClassifierRule, tool: &str, args_lower: &str) -> bool {
    // Tool name match: "*" matches everything; otherwise case-insensitive prefix.
    let tool_match = rule.tool == "*"
        || tool.eq_ignore_ascii_case(&rule.tool)
        || tool.to_lowercase().starts_with(&rule.tool.to_lowercase());

    if !tool_match {
        return false;
    }

    // Args content match (optional).
    if let Some(pattern) = &rule.args_contains {
        if !args_lower.contains(&pattern.to_lowercase()) {
            return false;
        }
    }

    true
}

// ---- Built-in default rules ------------------------------------------------

fn default_deny_rules() -> Vec<ClassifierRule> {
    vec![
        // Sensitive credential files (any tool).
        ClassifierRule {
            tool: "*".into(),
            args_contains: Some(".env".into()),
            verdict: VerdictKind::Deny,
            reason: "access to credential/env files is always denied".into(),
        },
        ClassifierRule {
            tool: "*".into(),
            args_contains: Some("id_rsa".into()),
            verdict: VerdictKind::Deny,
            reason: "access to private keys is always denied".into(),
        },
        ClassifierRule {
            tool: "*".into(),
            args_contains: Some(".pem".into()),
            verdict: VerdictKind::Deny,
            reason: "access to certificate keys is always denied".into(),
        },
    ]
}

fn default_ask_rules() -> Vec<ClassifierRule> {
    vec![
        // Destructive bash commands.
        ClassifierRule {
            tool: "bash".into(),
            args_contains: Some("rm -rf".into()),
            verdict: VerdictKind::Ask,
            reason: "recursive delete is destructive".into(),
        },
        ClassifierRule {
            tool: "bash".into(),
            args_contains: Some("rm -fr".into()),
            verdict: VerdictKind::Ask,
            reason: "recursive delete is destructive".into(),
        },
        ClassifierRule {
            tool: "bash".into(),
            args_contains: Some("sudo".into()),
            verdict: VerdictKind::Ask,
            reason: "elevated privileges need approval".into(),
        },
        ClassifierRule {
            tool: "bash".into(),
            args_contains: Some("git push".into()),
            verdict: VerdictKind::Ask,
            reason: "pushing to remote needs approval".into(),
        },
        ClassifierRule {
            tool: "bash".into(),
            args_contains: Some("| sh".into()),
            verdict: VerdictKind::Ask,
            reason: "piping to shell is risky".into(),
        },
        ClassifierRule {
            tool: "bash".into(),
            args_contains: Some("| bash".into()),
            verdict: VerdictKind::Ask,
            reason: "piping to shell is risky".into(),
        },
        ClassifierRule {
            tool: "bash".into(),
            args_contains: Some("mkfs".into()),
            verdict: VerdictKind::Ask,
            reason: "filesystem format is destructive".into(),
        },
        ClassifierRule {
            tool: "bash".into(),
            args_contains: Some("dd if=".into()),
            verdict: VerdictKind::Ask,
            reason: "raw disk write is destructive".into(),
        },
        // Network fetch (may exfiltrate data).
        ClassifierRule {
            tool: "bash".into(),
            args_contains: Some("curl".into()),
            verdict: VerdictKind::Ask,
            reason: "network request needs approval".into(),
        },
        ClassifierRule {
            tool: "bash".into(),
            args_contains: Some("wget".into()),
            verdict: VerdictKind::Ask,
            reason: "network download needs approval".into(),
        },
    ]
}

fn default_allow_rules() -> Vec<ClassifierRule> {
    vec![
        // All read-only tools are safe.
        ClassifierRule {
            tool: "read_file".into(),
            args_contains: None,
            verdict: VerdictKind::Allow,
            reason: String::new(),
        },
        ClassifierRule {
            tool: "list_dir".into(),
            args_contains: None,
            verdict: VerdictKind::Allow,
            reason: String::new(),
        },
        ClassifierRule {
            tool: "glob".into(),
            args_contains: None,
            verdict: VerdictKind::Allow,
            reason: String::new(),
        },
        ClassifierRule {
            tool: "grep".into(),
            args_contains: None,
            verdict: VerdictKind::Allow,
            reason: String::new(),
        },
        ClassifierRule {
            tool: "web_search".into(),
            args_contains: None,
            verdict: VerdictKind::Allow,
            reason: String::new(),
        },
        ClassifierRule {
            tool: "web_fetch".into(),
            args_contains: None,
            verdict: VerdictKind::Allow,
            reason: String::new(),
        },
        ClassifierRule {
            tool: "knowledge_search".into(),
            args_contains: None,
            verdict: VerdictKind::Allow,
            reason: String::new(),
        },
        ClassifierRule {
            tool: "task_list".into(),
            args_contains: None,
            verdict: VerdictKind::Allow,
            reason: String::new(),
        },
        ClassifierRule {
            tool: "ask_user_question".into(),
            args_contains: None,
            verdict: VerdictKind::Allow,
            reason: String::new(),
        },
        // Workspace writes (non-destructive file creation/edit).
        ClassifierRule {
            tool: "write_file".into(),
            args_contains: None,
            verdict: VerdictKind::Allow,
            reason: String::new(),
        },
        ClassifierRule {
            tool: "edit_file".into(),
            args_contains: None,
            verdict: VerdictKind::Allow,
            reason: String::new(),
        },
        ClassifierRule {
            tool: "multi_edit".into(),
            args_contains: None,
            verdict: VerdictKind::Allow,
            reason: String::new(),
        },
        ClassifierRule {
            tool: "knowledge_write".into(),
            args_contains: None,
            verdict: VerdictKind::Allow,
            reason: String::new(),
        },
        ClassifierRule {
            tool: "todo_write".into(),
            args_contains: None,
            verdict: VerdictKind::Allow,
            reason: String::new(),
        },
        // Safe bash commands (common dev tools).
        ClassifierRule {
            tool: "bash".into(),
            args_contains: Some("cargo".into()),
            verdict: VerdictKind::Allow,
            reason: String::new(),
        },
        ClassifierRule {
            tool: "bash".into(),
            args_contains: Some("npm".into()),
            verdict: VerdictKind::Allow,
            reason: String::new(),
        },
        ClassifierRule {
            tool: "bash".into(),
            args_contains: Some("pnpm".into()),
            verdict: VerdictKind::Allow,
            reason: String::new(),
        },
        ClassifierRule {
            tool: "bash".into(),
            args_contains: Some("node".into()),
            verdict: VerdictKind::Allow,
            reason: String::new(),
        },
        ClassifierRule {
            tool: "bash".into(),
            args_contains: Some("python".into()),
            verdict: VerdictKind::Allow,
            reason: String::new(),
        },
        ClassifierRule {
            tool: "bash".into(),
            args_contains: Some("git status".into()),
            verdict: VerdictKind::Allow,
            reason: String::new(),
        },
        ClassifierRule {
            tool: "bash".into(),
            args_contains: Some("git diff".into()),
            verdict: VerdictKind::Allow,
            reason: String::new(),
        },
        ClassifierRule {
            tool: "bash".into(),
            args_contains: Some("git log".into()),
            verdict: VerdictKind::Allow,
            reason: String::new(),
        },
        ClassifierRule {
            tool: "bash".into(),
            args_contains: Some("git show".into()),
            verdict: VerdictKind::Allow,
            reason: String::new(),
        },
        ClassifierRule {
            tool: "bash".into(),
            args_contains: Some("ls".into()),
            verdict: VerdictKind::Allow,
            reason: String::new(),
        },
        ClassifierRule {
            tool: "bash".into(),
            args_contains: Some("dir".into()),
            verdict: VerdictKind::Allow,
            reason: String::new(),
        },
        ClassifierRule {
            tool: "bash".into(),
            args_contains: Some("cat".into()),
            verdict: VerdictKind::Allow,
            reason: String::new(),
        },
        ClassifierRule {
            tool: "bash".into(),
            args_contains: Some("echo".into()),
            verdict: VerdictKind::Allow,
            reason: String::new(),
        },
        ClassifierRule {
            tool: "bash".into(),
            args_contains: Some("pwd".into()),
            verdict: VerdictKind::Allow,
            reason: String::new(),
        },
        // Sub-agent tasks are isolated.
        ClassifierRule {
            tool: "task".into(),
            args_contains: None,
            verdict: VerdictKind::Allow,
            reason: String::new(),
        },
    ]
}

/// Try to load a classifier config from a JSON file. Returns `None` if the file
/// doesn't exist or is unparseable (tolerant — falls back to defaults).
pub fn load_config(path: &std::path::Path) -> Option<ClassifierConfig> {
    let content = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&content).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn classifier() -> SafetyClassifier {
        SafetyClassifier::with_defaults()
    }

    #[test]
    fn read_tools_are_allowed() {
        let c = classifier();
        assert_eq!(
            c.classify("read_file", &serde_json::json!({"path": "src/main.rs"})),
            SafetyVerdict::Allow
        );
        assert_eq!(
            c.classify("glob", &serde_json::json!({"pattern": "**/*.rs"})),
            SafetyVerdict::Allow
        );
    }

    #[test]
    fn workspace_writes_are_allowed() {
        let c = classifier();
        assert_eq!(
            c.classify(
                "write_file",
                &serde_json::json!({"path": "new.rs", "content": "x"})
            ),
            SafetyVerdict::Allow
        );
        assert_eq!(
            c.classify(
                "edit_file",
                &serde_json::json!({"path": "x.rs", "old": "a", "new": "b"})
            ),
            SafetyVerdict::Allow
        );
    }

    #[test]
    fn sensitive_files_are_denied() {
        let c = classifier();
        assert!(matches!(
            c.classify("read_file", &serde_json::json!({"path": ".env"})),
            SafetyVerdict::Deny(_)
        ));
        assert!(matches!(
            c.classify("write_file", &serde_json::json!({"path": "keys/id_rsa"})),
            SafetyVerdict::Deny(_)
        ));
    }

    #[test]
    fn safe_bash_commands_are_allowed() {
        let c = classifier();
        assert_eq!(
            c.classify(
                "bash",
                &serde_json::json!({"command": "cargo test --workspace"})
            ),
            SafetyVerdict::Allow
        );
        assert_eq!(
            c.classify("bash", &serde_json::json!({"command": "git status"})),
            SafetyVerdict::Allow
        );
        assert_eq!(
            c.classify("bash", &serde_json::json!({"command": "ls -la"})),
            SafetyVerdict::Allow
        );
        assert_eq!(
            c.classify(
                "bash",
                &serde_json::json!({"command": "dir G:\\Code\\Kotlin_code"})
            ),
            SafetyVerdict::Allow
        );
    }

    #[test]
    fn dangerous_bash_commands_ask() {
        let c = classifier();
        assert!(matches!(
            c.classify("bash", &serde_json::json!({"command": "rm -rf /tmp/x"})),
            SafetyVerdict::Ask(_)
        ));
        assert!(matches!(
            c.classify(
                "bash",
                &serde_json::json!({"command": "git push origin main"})
            ),
            SafetyVerdict::Ask(_)
        ));
        assert!(matches!(
            c.classify(
                "bash",
                &serde_json::json!({"command": "sudo apt install x"})
            ),
            SafetyVerdict::Ask(_)
        ));
    }

    #[test]
    fn curl_asks_for_approval() {
        let c = classifier();
        assert!(matches!(
            c.classify(
                "bash",
                &serde_json::json!({"command": "curl https://example.com"})
            ),
            SafetyVerdict::Ask(_)
        ));
    }

    #[test]
    fn unknown_tool_falls_through_to_ask() {
        let c = classifier();
        assert!(matches!(
            c.classify("some_mcp_tool", &serde_json::json!({"x": 1})),
            SafetyVerdict::Ask(_)
        ));
    }

    #[test]
    fn deny_takes_priority_over_allow() {
        // write_file is in allow, but .env in args triggers deny (higher priority).
        let c = classifier();
        assert!(matches!(
            c.classify(
                "write_file",
                &serde_json::json!({"path": ".env", "content": "SECRET=x"})
            ),
            SafetyVerdict::Deny(_)
        ));
    }

    #[test]
    fn task_tool_is_allowed() {
        let c = classifier();
        assert_eq!(
            c.classify(
                "task",
                &serde_json::json!({"description": "explore", "prompt": "do it"})
            ),
            SafetyVerdict::Allow
        );
    }

    #[test]
    fn custom_config_overrides_defaults() {
        let config = ClassifierConfig {
            deny: vec![],
            ask: vec![],
            allow: vec![ClassifierRule {
                tool: "*".into(),
                args_contains: None,
                verdict: VerdictKind::Allow,
                reason: String::new(),
            }],
            fallback: VerdictKind::Allow,
        };
        let c = SafetyClassifier::new(config);
        // Everything is allowed, even dangerous commands.
        assert_eq!(
            c.classify("bash", &serde_json::json!({"command": "rm -rf /"})),
            SafetyVerdict::Allow
        );
    }
}
