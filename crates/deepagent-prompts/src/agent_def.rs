//! Agent definitions, aligned with Claude Code's `agents/<name>.md`.
//!
//! An agent file is YAML frontmatter (`name`, `description`, `tools`, `model`,
//! `color`) followed by a Markdown system prompt (the agent's identity, process,
//! and output guidance). [`AgentDef`] is the parsed shape; the body becomes the
//! `AgentIdentity` layer of an assembled system prompt.

use serde::{Deserialize, Serialize};

use crate::frontmatter::{self, Frontmatter};

/// Which model an agent prefers. `Inherit` means "use the session default".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelPref {
    /// Inherit the caller/session model.
    Inherit,
    /// A named model (e.g. "deepseek-v4-flash", "deepseek-v4-pro", "sonnet").
    Named(String),
}

impl ModelPref {
    /// Parse a frontmatter `model:` value.
    pub fn parse(value: &str) -> Self {
        let v = value.trim();
        if v.is_empty() || v.eq_ignore_ascii_case("inherit") {
            ModelPref::Inherit
        } else {
            ModelPref::Named(v.to_string())
        }
    }

    /// The named model, if any.
    pub fn name(&self) -> Option<&str> {
        match self {
            ModelPref::Named(n) => Some(n),
            ModelPref::Inherit => None,
        }
    }
}

/// CC agent `permissionMode`（子代理语义子集）。
///
/// Claude Code accepts `acceptEdits / bypassPermissions / default / dontAsk / plan`
/// (see `借鉴/claudecode/.../types/permissions.ts`); only the three modes with a
/// clear meaning inside a sub-agent are modeled here — the others are ignored at
/// parse time with the same "invalid mode is dropped, not fatal" behavior CC uses.
/// `plan`/read-only is expressed structurally via the tool allowlist instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionMode {
    /// `default` — regular developer permissions; high-risk calls still bubble
    /// to the parent approval channel (the current behavior).
    Default,
    /// `acceptEdits` — low-risk file writes are pre-approved and never interrupt.
    AcceptEdits,
    /// `bypassPermissions` — full permissions; every approval resolves to allow.
    BypassPermissions,
}

impl PermissionMode {
    /// Parse a frontmatter `permissionMode:` value. `None` for empty or
    /// unrecognized modes (dropped, never fatal).
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "" | "default" => Some(Self::Default),
            "acceptedits" => Some(Self::AcceptEdits),
            "bypasspermissions" => Some(Self::BypassPermissions),
            _ => None,
        }
    }
}

/// CC agent `effort` — default reasoning depth for the sub-agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffortLevel {
    /// Shallow reasoning, low latency.
    Simple,
    /// Balanced reasoning.
    Medium,
    /// Deep reasoning for hard tasks.
    Deep,
}

impl EffortLevel {
    /// Parse a frontmatter `effort:` value.
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "simple" => Some(Self::Simple),
            "medium" => Some(Self::Medium),
            "deep" => Some(Self::Deep),
            _ => None,
        }
    }
}

/// CC agent `isolation` — how the sub-agent's execution root is isolated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IsolationChoice {
    /// Run in the parent's workspace.
    Shared,
    /// Run in a dedicated git worktree.
    Worktree,
}

impl IsolationChoice {
    /// Parse a frontmatter `isolation:` value.
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "" | "shared" => Some(Self::Shared),
            "worktree" => Some(Self::Worktree),
            _ => None,
        }
    }
}

/// A parsed agent definition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentDef {
    /// Agent id/name (frontmatter `name`).
    pub name: String,
    /// Description of when to use the agent (frontmatter `description`).
    pub description: String,
    /// Tools the agent may use (frontmatter `tools`); empty = inherit defaults.
    #[serde(default)]
    pub tools: Vec<String>,
    /// Preferred model.
    pub model: ModelPref,
    /// Optional UI color (frontmatter `color`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// The Markdown system prompt body (the agent's identity + process).
    pub body: String,
    /// `disallowedTools` — a negative allowlist; a tool named here is dropped
    /// from the child even when the profile's positive list would keep it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disallowed_tools: Option<Vec<String>>,
    /// `permissionMode` — sub-agent permission posture (see [`PermissionMode`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_mode: Option<PermissionMode>,
    /// `maxTurns` — max agentic turns, mapped onto the engine's step cap.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_turns: Option<usize>,
    /// `effort` — default reasoning depth for the child.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<EffortLevel>,
    /// `skills` — skill ids preloaded before the child's first turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skills: Option<Vec<String>>,
    /// `background` — always start in the background when spawned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background: Option<bool>,
    /// `isolation` — worktree vs shared execution root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub isolation: Option<IsolationChoice>,
}

impl AgentDef {
    /// Build from parsed frontmatter. Returns `None` if `name`/`description`
    /// are missing.
    pub fn from_frontmatter(fm: &Frontmatter) -> Option<Self> {
        let name = fm.get("name")?.trim().to_string();
        let description = fm.get("description")?.trim().to_string();
        if name.is_empty() || description.is_empty() {
            return None;
        }
        let list_or_none = |key: &str| {
            let values = fm.get_list(key);
            if values.is_empty() {
                None
            } else {
                Some(values)
            }
        };
        // CC maxTurns is a positive integer; `0`/non-numeric values are dropped.
        let max_turns = fm
            .get("maxTurns")
            .and_then(|s| s.trim().parse::<usize>().ok())
            .filter(|n| *n > 0);
        Some(Self {
            name,
            description,
            tools: fm.get_list("tools"),
            model: fm
                .get("model")
                .map(ModelPref::parse)
                .unwrap_or(ModelPref::Inherit),
            color: fm.get("color").map(|s| s.to_string()),
            body: fm.body.clone(),
            disallowed_tools: list_or_none("disallowedTools"),
            permission_mode: fm.get("permissionMode").and_then(PermissionMode::parse),
            max_turns,
            effort: fm.get("effort").and_then(EffortLevel::parse),
            skills: list_or_none("skills"),
            background: fm.get_bool("background"),
            isolation: fm.get("isolation").and_then(IsolationChoice::parse),
        })
    }

    /// Parse an agent `.md` document end to end.
    pub fn parse(input: &str) -> Option<Self> {
        let fm = frontmatter::parse(input);
        Self::from_frontmatter(&fm)
    }

    /// Whether the agent restricts itself to a tool allow-list.
    pub fn restricts_tools(&self) -> bool {
        !self.tools.is_empty()
    }

    /// Whether the agent declares a negative tool allow-list.
    pub fn disallows_tools(&self) -> bool {
        self.disallowed_tools
            .as_ref()
            .is_some_and(|list| !list.is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "---\nname: code-architect\ndescription: Designs feature architectures\ntools: Glob, Grep, Read, TodoWrite\nmodel: sonnet\ncolor: green\n---\nYou are a senior software architect.";

    #[test]
    fn parses_full_agent() {
        let a = AgentDef::parse(SAMPLE).unwrap();
        assert_eq!(a.name, "code-architect");
        assert_eq!(a.tools, vec!["Glob", "Grep", "Read", "TodoWrite"]);
        assert_eq!(a.model, ModelPref::Named("sonnet".into()));
        assert_eq!(a.color.as_deref(), Some("green"));
        assert!(a.body.contains("senior software architect"));
        assert!(a.restricts_tools());
    }

    #[test]
    fn model_inherit() {
        let a = AgentDef::parse("---\nname: x\ndescription: d\nmodel: inherit\n---\nbody").unwrap();
        assert_eq!(a.model, ModelPref::Inherit);
        assert!(a.model.name().is_none());
        assert!(!a.restricts_tools());
    }

    #[test]
    fn missing_required_fields() {
        assert!(AgentDef::parse("---\nname: only\n---\nbody").is_none());
    }

    #[test]
    fn model_pref_parse() {
        assert_eq!(ModelPref::parse(""), ModelPref::Inherit);
        assert_eq!(ModelPref::parse("Inherit"), ModelPref::Inherit);
        assert_eq!(
            ModelPref::parse("deepseek-v4-pro"),
            ModelPref::Named("deepseek-v4-pro".into())
        );
    }

    const TEAMMATE: &str = "---\nname: code-reviewer\ndescription: Reviews code diffs\ntools: Read, Grep\ndisallowedTools: Bash, Write\nmodel: inherit\npermissionMode: acceptEdits\nmaxTurns: 12\neffort: deep\nskills: unit-testing\nbackground: true\n---\nYou are a code reviewer.";

    #[test]
    fn parses_teammate_frontmatter() {
        let a = AgentDef::parse(TEAMMATE).unwrap();
        assert_eq!(
            a.disallowed_tools.as_deref(),
            Some(&["Bash".to_string(), "Write".to_string()][..])
        );
        assert!(a.disallows_tools());
        assert_eq!(a.permission_mode, Some(PermissionMode::AcceptEdits));
        assert_eq!(a.max_turns, Some(12));
        assert_eq!(a.effort, Some(EffortLevel::Deep));
        assert_eq!(a.skills.as_deref(), Some(&["unit-testing".to_string()][..]));
        assert_eq!(a.background, Some(true));
        // Not declared in this file.
        assert!(a.isolation.is_none());
    }

    #[test]
    fn legacy_agent_without_teammate_fields_stays_compatible() {
        let a = AgentDef::parse(SAMPLE).unwrap();
        assert!(a.disallowed_tools.is_none());
        assert!(a.permission_mode.is_none());
        assert!(a.max_turns.is_none());
        assert!(a.effort.is_none());
        assert!(a.skills.is_none());
        assert!(a.background.is_none());
        assert!(a.isolation.is_none());
        assert!(!a.disallows_tools());
    }

    #[test]
    fn permission_and_isolation_parsers() {
        assert_eq!(PermissionMode::parse(""), Some(PermissionMode::Default));
        assert_eq!(
            PermissionMode::parse("bypassPermissions"),
            Some(PermissionMode::BypassPermissions)
        );
        // CC accepts `dontAsk`/`plan` but we drop them for sub-agents.
        assert_eq!(PermissionMode::parse("plan"), None);
        assert_eq!(EffortLevel::parse("medium"), Some(EffortLevel::Medium));
        assert_eq!(EffortLevel::parse("bogus"), None);
        assert_eq!(
            IsolationChoice::parse("worktree"),
            Some(IsolationChoice::Worktree)
        );
        assert_eq!(IsolationChoice::parse("remote"), None);
    }

    #[test]
    fn max_turns_rejects_zero_and_non_numeric() {
        let a = AgentDef::parse("---\nname: x\ndescription: d\nmaxTurns: 0\n---\nbody").unwrap();
        assert!(a.max_turns.is_none());
        let a = AgentDef::parse("---\nname: x\ndescription: d\nmaxTurns: nope\n---\nbody").unwrap();
        assert!(a.max_turns.is_none());
    }
}
