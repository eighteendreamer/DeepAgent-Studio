//! # deepagent-skills
//!
//! The Skill system, aligned with Claude Code's `SKILL.md` (开发提示词.md §14;
//! 对齐ClaudeCode计划 维度 2).
//!
//! A *skill* is a self-contained package of procedural knowledge — an
//! "onboarding guide" that turns the general agent into a specialist for a
//! task. Skills use **three-level progressive disclosure** so they cost almost
//! nothing until needed:
//!
//! | Level | Content | When loaded | Cost |
//! |-------|---------|-------------|------|
//! | 1 | metadata (`name` + `description` w/ trigger phrases) | always | ~100 words |
//! | 2 | `SKILL.md` body (instructions) | on activation | <5k words |
//! | 3 | `references/`/`examples/`/`scripts/`/`assets/` | on demand | unbounded |
//!
//! ## What this crate provides
//!
//! - [`frontmatter`] — a tiny YAML-frontmatter splitter (no external dep).
//! - [`skill::Skill`] — the parsed model + trigger extraction.
//! - [`loader`] — filesystem **auto-discovery** (scan for `SKILL.md`).
//! - [`installer::SkillInstaller`] — **install / uninstall** into a managed root
//!   (the local half of "download & install").
//! - [`registry::SkillRegistry`] — **dynamic registration** + **passive**
//!   (trigger-matched, model-driven) and **active** (by-id) activation, with
//!   Level-2 bodies surfaced as context [`PromptFragment`]s.
//! - [`SkillManager`] — a façade that composes discovery + install + registry.
//!
//! ## Active vs passive use
//!
//! - **Passive** ([`SkillRegistry::match_query`] / [`SkillRegistry::auto_activate`]):
//!   the model/runtime matches the user's prompt against trigger phrases and
//!   pulls in the best skill automatically.
//! - **Active** ([`SkillRegistry::activate`]): a caller (user/slash command)
//!   names a skill explicitly.
//!
//! Both paths converge on the same disclosure step: the Level-2 body becomes a
//! high-priority [`PromptFragment`] for the context pipeline.

#![warn(missing_docs)]

pub mod frontmatter;
pub mod installer;
pub mod loader;
pub mod marketplace;
pub mod registry;
pub mod scanner;
pub mod skill;

use std::path::PathBuf;

use deepagent_context::PromptFragment;
use deepagent_core::error::Result;

pub use frontmatter::Frontmatter;
pub use installer::SkillInstaller;
pub use marketplace::{
    build_download_candidates, resolve_api_key, ApiKeySource, GitTreeEntry, GitTreeResponse,
    GithubLocator, MarketSearchData, MarketSkill, Pagination, SearchQuery, SkillsMpClient,
    SkillsMpClientHandle, SortBy, TempSkillDir, BROWSE_FALLBACK_QUERY, BUILTIN_SKILLSMP_API_KEY,
    DEFAULT_GITHUB_MIRRORS, GITHUB_API_BASE,
};
pub use registry::{
    BodyForInvokeError, SkillMatch, SkillRegistry, MAX_LISTING_DESC_CHARS, MIN_DESC_LENGTH,
};
pub use scanner::{
    scan_dir, FileInfo, RiskCategory, RiskItem, RiskSeverity, ScanReport, MAX_TEXT_FILE_BYTES,
};
pub use skill::{ResourceKind, Skill, SkillMeta, SkillOrigin, SkillResource, SkillToolOutput};

/// The four storage roots that the marketplace-aware loader walks. Each root
/// corresponds to one [`SkillOrigin`]:
///
/// | Field | Origin | Typical path |
/// |-------|--------|--------------|
/// | `builtin` | [`SkillOrigin::BuiltIn`] | `<exe>/resources/skills/` (Tauri resource) |
/// | `user` | [`SkillOrigin::User`] | `~/.deepagent/skills/` (user-level, top-level skills) |
/// | `marketplace` | [`SkillOrigin::Installed`] | `~/.deepagent/skills/marketplace/` |
/// | `workspace` | [`SkillOrigin::Workspace`] | `<project>/.deepagent/skills/` (optional) |
///
/// Used by `SkillsService::open_v2` (in `deepagent-app-core`) to assemble a
/// single registry with conflict precedence
/// `Workspace > User > Installed > BuiltIn`.
#[derive(Debug, Clone)]
pub struct SkillsRoots {
    /// Built-in skills shipped with the application binary.
    pub builtin: PathBuf,
    /// User-level skills (top-level directories under this root).
    pub user: PathBuf,
    /// Marketplace-installed skills (typically nested under `user`).
    pub marketplace: PathBuf,
    /// Optional workspace-scoped skills directory.
    pub workspace: Option<PathBuf>,
}

/// A façade composing auto-discovery, installation, and the live registry.
///
/// Typical wiring:
/// 1. [`SkillManager::new`] over a workspace skills dir + a managed install dir.
/// 2. [`SkillManager::load_all`] to discover workspace + installed skills.
/// 3. At prompt time, [`SkillManager::auto_activate`] (passive) or
///    [`SkillManager::activate`] (active) to disclose a skill body.
/// 4. [`SkillManager::install`] / [`SkillManager::uninstall`] to manage the set
///    at runtime; both keep the registry in sync.
#[derive(Debug, Clone)]
pub struct SkillManager {
    workspace_dir: Option<PathBuf>,
    installer: SkillInstaller,
    registry: SkillRegistry,
}

impl SkillManager {
    /// Build a manager over an optional workspace skills directory and a
    /// managed install root.
    pub fn new(workspace_dir: Option<PathBuf>, install_dir: impl Into<PathBuf>) -> Self {
        Self {
            workspace_dir,
            installer: SkillInstaller::new(install_dir),
            registry: SkillRegistry::new(),
        }
    }

    /// Discover and register all workspace + installed skills, replacing the
    /// current registry contents. Returns the number of skills registered.
    pub fn load_all(&mut self) -> Result<usize> {
        let mut registry = SkillRegistry::new();

        if let Some(dir) = &self.workspace_dir {
            for skill in loader::discover(dir, SkillOrigin::Workspace)? {
                registry.register(skill);
            }
        }
        for skill in self.installer.installed()? {
            registry.register(skill);
        }

        let count = registry.len();
        self.registry = registry;
        Ok(count)
    }

    /// Install a skill from a prepared source directory and register it live.
    pub fn install(&mut self, source: impl AsRef<std::path::Path>) -> Result<SkillMeta> {
        let skill = self.installer.install_dir(source)?;
        let meta = skill.meta.clone();
        self.registry.register(skill);
        Ok(meta)
    }

    /// Uninstall a skill by id and drop it from the registry.
    pub fn uninstall(&mut self, id: &str) -> Result<bool> {
        let removed = self.installer.uninstall(id)?;
        if removed {
            self.registry.unregister(id);
        }
        Ok(removed)
    }

    /// Register a skill directly (e.g. a built-in), bypassing the filesystem.
    pub fn register(&mut self, skill: Skill) {
        self.registry.register(skill);
    }

    /// The live registry (for catalog / inspection).
    pub fn registry(&self) -> &SkillRegistry {
        &self.registry
    }

    /// Passive activation: match the query and disclose the best skill body.
    pub fn auto_activate(&self, query: &str) -> Option<(String, PromptFragment)> {
        self.registry.auto_activate(query)
    }

    /// Active activation by id.
    pub fn activate(&self, id: &str) -> Option<PromptFragment> {
        self.registry.activate(id)
    }

    /// The always-resident Level-1 catalog blurb for the system prompt.
    pub fn catalog_blurb(&self) -> String {
        self.registry.catalog_blurb()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;

    fn write(path: &Path, content: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, content).unwrap();
    }

    #[test]
    fn manager_loads_workspace_and_installed() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path().join("workspace-skills");
        let install = tmp.path().join("installed-skills");

        // A workspace skill.
        write(
            &ws.join("commit-helper").join("SKILL.md"),
            "---\nname: Commit Helper\ndescription: Use to \"write a commit message\".\n---\nWrite it.",
        );

        let mut mgr = SkillManager::new(Some(ws), &install);

        // Install one from a prepared source.
        let src = tmp.path().join("src").join("pdf-editor");
        write(
            &src.join("SKILL.md"),
            "---\nname: PDF Editor\ndescription: Use to \"rotate a pdf\".\n---\nRotate.",
        );
        mgr.install(&src).unwrap();

        // load_all sees both.
        let count = mgr.load_all().unwrap();
        assert_eq!(count, 2);
        assert!(mgr.registry().contains("commit-helper"));
        assert!(mgr.registry().contains("pdf-editor"));
    }

    #[test]
    fn passive_then_active_activation() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path().join("skills");
        write(
            &ws.join("pdf").join("SKILL.md"),
            "---\nname: PDF\ndescription: Use to \"rotate a pdf\" or \"merge pdfs\".\n---\nDo it.",
        );
        let mut mgr = SkillManager::new(Some(ws), tmp.path().join("inst"));
        mgr.load_all().unwrap();

        // Passive.
        let (id, frag) = mgr.auto_activate("please rotate a pdf").unwrap();
        assert_eq!(id, "pdf");
        assert!(frag.content.contains("# Skill: PDF"));

        // Active.
        let frag2 = mgr.activate("pdf").unwrap();
        assert!(frag2.content.contains("Do it."));
    }

    #[test]
    fn uninstall_syncs_registry() {
        let tmp = tempfile::tempdir().unwrap();
        let mut mgr = SkillManager::new(None, tmp.path().join("inst"));
        let src = tmp.path().join("src");
        write(
            &src.join("SKILL.md"),
            "---\nname: X\ndescription: \"do x\"\n---\nbody",
        );
        let meta = mgr.install(&src).unwrap();
        assert!(mgr.registry().contains(&meta.id));
        assert!(mgr.uninstall(&meta.id).unwrap());
        assert!(!mgr.registry().contains(&meta.id));
    }

    #[test]
    fn direct_register_builtin() {
        let mut mgr = SkillManager::new(None, std::env::temp_dir().join("ds-skills-noop"));
        let fm = frontmatter::parse("---\nname: BuiltIn\ndescription: \"do builtin\"\n---\nbody");
        let skill = Skill::from_frontmatter("builtin", &fm, SkillOrigin::BuiltIn).unwrap();
        mgr.register(skill);
        assert!(mgr.registry().contains("builtin"));
    }
}
