//! The [`Skill`] model and its three-level progressive-disclosure shape.
//!
//! Aligned with Claude Code's `SKILL.md`:
//! - **Level 1 — metadata** (`name` + `description`): always in context (~100
//!   words). The `description` carries *trigger phrases* used for passive
//!   (model-driven) activation.
//! - **Level 2 — body**: the Markdown instructions, loaded when the skill is
//!   activated (<5k words).
//! - **Level 3 — resources** (`references/`, `examples/`, `scripts/`,
//!   `assets/`): loaded/executed on demand, never auto-loaded.

use serde::{Deserialize, Serialize};

use crate::frontmatter::Frontmatter;

/// How a skill was made available to the runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SkillOrigin {
    /// Discovered in the workspace `.deepagent/skills/` tree.
    #[default]
    Workspace,
    /// Provided at the user level (`~/.deepagent/skills/`).
    User,
    /// Installed from a package/marketplace.
    Installed,
    /// Registered programmatically (built-in / test).
    BuiltIn,
}

impl SkillOrigin {
    /// Stable string label.
    pub const fn label(&self) -> &'static str {
        match self {
            SkillOrigin::Workspace => "workspace",
            SkillOrigin::User => "user",
            SkillOrigin::Installed => "installed",
            SkillOrigin::BuiltIn => "built_in",
        }
    }
}

/// A bundled resource directory kind (Level 3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceKind {
    /// `references/` — docs loaded into context as needed.
    Reference,
    /// `examples/` — worked examples.
    Example,
    /// `scripts/` — executable code (may run without loading into context).
    Script,
    /// `assets/` — output files (templates, fonts, images).
    Asset,
}

/// A reference to a bundled resource file (Level 3), recorded but not loaded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillResource {
    /// Which bundle the file belongs to.
    pub kind: ResourceKind,
    /// Path relative to the skill directory (forward-slashed).
    pub rel_path: String,
}

/// Skill metadata — Level 1, always resident.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillMeta {
    /// Canonical skill id (slug; derived from the directory name).
    pub id: String,
    /// Human name (frontmatter `name`).
    pub name: String,
    /// Description with trigger phrases (frontmatter `description`).
    pub description: String,
    /// Optional version (frontmatter `version`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// Where the skill came from.
    #[serde(default)]
    pub origin: SkillOrigin,
}

impl SkillMeta {
    /// The Level-1 metadata blurb that stays resident in context (~one line).
    pub fn blurb(&self) -> String {
        format!("- {} — {}", self.name, self.description)
    }
}

/// A fully-parsed skill: metadata + body + resource manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Skill {
    /// Level-1 metadata.
    pub meta: SkillMeta,
    /// Level-2 body (Markdown instructions).
    pub body: String,
    /// Level-3 resource manifest (paths only; not loaded).
    #[serde(default)]
    pub resources: Vec<SkillResource>,
    /// Lower-cased trigger phrases extracted from the description (for passive
    /// matching).
    #[serde(default)]
    pub triggers: Vec<String>,
}

impl Skill {
    /// Build a skill from parsed frontmatter, an id, and an origin. Returns
    /// `None` if the mandatory `name`/`description` fields are absent.
    pub fn from_frontmatter(
        id: impl Into<String>,
        fm: &Frontmatter,
        origin: SkillOrigin,
    ) -> Option<Self> {
        let name = fm.get("name")?.to_string();
        let description = fm.get("description")?.to_string();
        if name.trim().is_empty() || description.trim().is_empty() {
            return None;
        }
        let triggers = extract_triggers(&description);
        Some(Self {
            meta: SkillMeta {
                id: id.into(),
                name,
                description,
                version: fm.get("version").map(|s| s.to_string()),
                origin,
            },
            body: fm.body.clone(),
            resources: Vec::new(),
            triggers,
        })
    }

    /// Approximate word count of the body (for budget reporting).
    pub fn body_word_count(&self) -> usize {
        self.body.split_whitespace().count()
    }

    /// Whether the skill carries any Level-3 resources.
    pub fn has_resources(&self) -> bool {
        !self.resources.is_empty()
    }
}

/// Extract trigger phrases from a Claude Code-style description.
///
/// Claude Code descriptions embed exact phrases in quotes, e.g.
/// `This skill should be used when the user asks to "create a hook",
/// "add a PreToolUse hook"`. We lift every quoted span as a trigger. If no
/// quoted spans exist, we fall back to the salient lower-cased words of the
/// description so passive matching still has something to work with.
pub fn extract_triggers(description: &str) -> Vec<String> {
    let mut triggers = Vec::new();

    // 1. Quoted phrases ("..." or '...').
    for quote in ['"', '\''] {
        let mut rest = description;
        while let Some(start) = rest.find(quote) {
            let after = &rest[start + 1..];
            if let Some(end) = after.find(quote) {
                let phrase = after[..end].trim().to_lowercase();
                if phrase.len() >= 2 && !triggers.contains(&phrase) {
                    triggers.push(phrase);
                }
                rest = &after[end + 1..];
            } else {
                break;
            }
        }
    }

    triggers
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontmatter;

    #[test]
    fn builds_from_frontmatter() {
        let fm = frontmatter::parse(
            "---\nname: Hook Dev\ndescription: This skill should be used when the user asks to \"create a hook\", \"add a PreToolUse hook\".\nversion: 0.1.0\n---\nBody text here.",
        );
        let skill = Skill::from_frontmatter("hook-dev", &fm, SkillOrigin::Workspace).unwrap();
        assert_eq!(skill.meta.name, "Hook Dev");
        assert_eq!(skill.meta.version.as_deref(), Some("0.1.0"));
        assert_eq!(skill.meta.origin, SkillOrigin::Workspace);
        assert!(skill.body.contains("Body text"));
        assert_eq!(
            skill.triggers,
            vec!["create a hook", "add a pretooluse hook"]
        );
    }

    #[test]
    fn missing_fields_yields_none() {
        let fm = frontmatter::parse("---\nname: Only Name\n---\nbody");
        assert!(Skill::from_frontmatter("x", &fm, SkillOrigin::Workspace).is_none());
    }

    #[test]
    fn extracts_quoted_triggers() {
        let desc = "Use when user says \"build a dashboard\" or \"make a chart\".";
        let triggers = extract_triggers(desc);
        assert!(triggers.contains(&"build a dashboard".to_string()));
        assert!(triggers.contains(&"make a chart".to_string()));
    }

    #[test]
    fn no_quotes_yields_no_triggers() {
        let triggers = extract_triggers("A plain description without any quoted phrases.");
        assert!(triggers.is_empty());
    }

    #[test]
    fn blurb_is_one_line() {
        let meta = SkillMeta {
            id: "x".into(),
            name: "X".into(),
            description: "does x".into(),
            version: None,
            origin: SkillOrigin::BuiltIn,
        };
        assert_eq!(meta.blurb(), "- X — does x");
    }
}
