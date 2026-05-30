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
    /// A named model (e.g. "deepseek-chat", "deepseek-reasoner", "sonnet").
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
            ModelPref::parse("deepseek-reasoner"),
            ModelPref::Named("deepseek-reasoner".into())
        );
    }
}
