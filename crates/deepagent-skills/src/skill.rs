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
    /// If `true`, the skill is hidden from the model: it is excluded from the
    /// catalog reminder and the `skill` tool refuses to invoke it. The skill
    /// can still be activated by the user (slash command / UI button).
    ///
    /// Sourced from the SKILL.md frontmatter key `disable-model-invocation`
    /// (the underscore form `disable_model_invocation` is also accepted for
    /// compatibility). Defaults to `false`.
    #[serde(default)]
    pub disable_model_invocation: bool,
}

impl SkillMeta {
    /// The Level-1 metadata blurb that stays resident in context (~one line).
    pub fn blurb(&self) -> String {
        format!("- {} — {}", self.name, self.description)
    }
}

/// Output of [`crate::registry::SkillRegistry::body_for_invoke`] — the payload
/// returned to the model when it invokes the `skill` tool (channel B per
/// design.md §Auto-Activation).
///
/// Carries the disclosed Level-2 body (the SKILL.md content with `${ARGS}` /
/// `$ARGS` substituted) plus the on-disk base directory and any known Level-3
/// resource paths so the model can use `read_file` / `grep` to pull deeper
/// context as it needs it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillToolOutput {
    /// Skill id (slug).
    pub id: String,
    /// Display name (frontmatter `name`).
    pub name: String,
    /// Disclosed Level-2 body with `${ARGS}` / `$ARGS` substituted.
    pub body: String,
    /// Absolute path to the skill directory on disk, when known. Used by the
    /// model to address Level-3 references via `read_file` / `grep`. `None`
    /// when the skill was registered programmatically (e.g. test fixtures
    /// without a backing directory) or when the path is otherwise unknown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_dir: Option<String>,
    /// Forward-slashed paths (relative to `base_dir`) of the skill's known
    /// Level-3 resources (`references/`, `examples/`, `scripts/`, `assets/`).
    pub resources: Vec<String>,
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
    /// Absolute path to the skill's source directory, when discovered from
    /// disk. `None` for programmatically registered skills (built-in fixtures,
    /// tests). Set by [`crate::loader::load_skill_dir`] using the canonical
    /// path of the skill directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_dir: Option<std::path::PathBuf>,
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
        // Accept both the canonical kebab-case key and the underscore variant
        // for compatibility with hand-edited SKILL.md files.
        let disable_model_invocation = fm
            .get("disable-model-invocation")
            .or_else(|| fm.get("disable_model_invocation"))
            .map(|v| v.trim().eq_ignore_ascii_case("true"))
            .unwrap_or(false);
        Some(Self {
            meta: SkillMeta {
                id: id.into(),
                name,
                description,
                version: fm.get("version").map(|s| s.to_string()),
                origin,
                disable_model_invocation,
            },
            body: fm.body.clone(),
            resources: Vec::new(),
            triggers,
            base_dir: None,
        })
    }

    /// Builder helper: set the on-disk base directory of the skill. Used by
    /// the loader so the `skill` tool can surface Level-3 resource paths to
    /// the model.
    pub fn with_base_dir(mut self, base_dir: impl Into<std::path::PathBuf>) -> Self {
        self.base_dir = Some(base_dir.into());
        self
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
            disable_model_invocation: false,
        };
        assert_eq!(meta.blurb(), "- X — does x");
    }

    #[test]
    fn parses_disable_model_invocation_true() {
        let fm = frontmatter::parse(
            "---\nname: Hidden\ndescription: \"only via /hidden\"\ndisable-model-invocation: true\n---\nbody",
        );
        let skill = Skill::from_frontmatter("hidden", &fm, SkillOrigin::Workspace).unwrap();
        assert!(skill.meta.disable_model_invocation);
    }

    #[test]
    fn parses_disable_model_invocation_false() {
        let fm = frontmatter::parse(
            "---\nname: Visible\ndescription: \"normal skill\"\ndisable-model-invocation: false\n---\nbody",
        );
        let skill = Skill::from_frontmatter("visible", &fm, SkillOrigin::Workspace).unwrap();
        assert!(!skill.meta.disable_model_invocation);
    }

    #[test]
    fn defaults_disable_model_invocation_when_absent() {
        let fm = frontmatter::parse("---\nname: Plain\ndescription: \"no flag\"\n---\nbody");
        let skill = Skill::from_frontmatter("plain", &fm, SkillOrigin::Workspace).unwrap();
        assert!(!skill.meta.disable_model_invocation);
    }

    #[test]
    fn accepts_underscore_disable_model_invocation_alias() {
        let fm = frontmatter::parse(
            "---\nname: Hidden\ndescription: \"underscore form\"\ndisable_model_invocation: true\n---\nbody",
        );
        let skill = Skill::from_frontmatter("hidden", &fm, SkillOrigin::Workspace).unwrap();
        assert!(skill.meta.disable_model_invocation);
    }
}
