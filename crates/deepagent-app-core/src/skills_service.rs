//! Skill management for the UI.
//!
//! Wraps [`deepagent_skills::SkillManager`] and exposes serializable DTOs so the
//! desktop app can list, install, uninstall, and preview activation of skills.
//! The networked "download" step (fetch a skill archive from a URL/marketplace)
//! is performed by the app shell; this service consumes an already-unpacked
//! source directory via [`SkillsService::install_from_dir`].

use std::path::PathBuf;

use deepagent_core::error::Result;
use deepagent_skills::{SkillManager, SkillMeta};
use serde::{Deserialize, Serialize};

/// A serializable view of a skill's Level-1 metadata for the UI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillDto {
    /// Stable skill id (slug).
    pub id: String,
    /// Human name.
    pub name: String,
    /// Description (with trigger phrases).
    pub description: String,
    /// Optional version.
    pub version: Option<String>,
    /// Origin label (workspace/user/installed/built_in).
    pub origin: String,
    /// Trigger phrases used for passive activation.
    pub triggers: Vec<String>,
}

impl SkillDto {
    fn from_meta(meta: &SkillMeta, triggers: Vec<String>) -> Self {
        Self {
            id: meta.id.clone(),
            name: meta.name.clone(),
            description: meta.description.clone(),
            version: meta.version.clone(),
            origin: meta.origin.label().to_string(),
            triggers,
        }
    }
}

/// The result of a passive-activation preview.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SkillActivationDto {
    /// The matched skill id.
    pub id: String,
    /// The disclosed Level-2 body (the prompt fragment content).
    pub body: String,
}

/// UI-facing skill management service.
pub struct SkillsService {
    manager: SkillManager,
}

impl SkillsService {
    /// Build over an optional workspace skills dir and a managed install root,
    /// loading all discoverable + installed skills immediately.
    pub fn open(workspace_dir: Option<PathBuf>, install_dir: impl Into<PathBuf>) -> Result<Self> {
        let mut manager = SkillManager::new(workspace_dir, install_dir);
        manager.load_all()?;
        Ok(Self { manager })
    }

    /// Build over an existing manager (e.g. for tests).
    pub fn from_manager(manager: SkillManager) -> Self {
        Self { manager }
    }

    /// Reload all skills from disk.
    pub fn reload(&mut self) -> Result<usize> {
        self.manager.load_all()
    }

    /// List all known skills as DTOs (sorted by id).
    pub fn list(&self) -> Vec<SkillDto> {
        self.manager
            .registry()
            .catalog()
            .into_iter()
            .map(|meta| {
                let triggers = self
                    .manager
                    .registry()
                    .get(&meta.id)
                    .map(|s| s.triggers.clone())
                    .unwrap_or_default();
                SkillDto::from_meta(meta, triggers)
            })
            .collect()
    }

    /// Install a skill from an already-downloaded/unpacked source directory.
    pub fn install_from_dir(&mut self, source: impl AsRef<std::path::Path>) -> Result<SkillDto> {
        let meta = self.manager.install(source)?;
        let triggers = self
            .manager
            .registry()
            .get(&meta.id)
            .map(|s| s.triggers.clone())
            .unwrap_or_default();
        Ok(SkillDto::from_meta(&meta, triggers))
    }

    /// Uninstall a skill by id. Returns whether it existed.
    pub fn uninstall(&mut self, id: &str) -> Result<bool> {
        self.manager.uninstall(id)
    }

    /// The always-resident catalog blurb (Level-1) for the system prompt.
    pub fn catalog_blurb(&self) -> String {
        self.manager.catalog_blurb()
    }

    /// Preview passive activation for a query: the best trigger-matched skill
    /// and its disclosed body, or `None` if nothing matched.
    pub fn preview_activation(&self, query: &str) -> Option<SkillActivationDto> {
        self.manager
            .auto_activate(query)
            .map(|(id, frag)| SkillActivationDto {
                id,
                body: frag.content,
            })
    }

    /// Actively activate a skill by id, returning its disclosed body.
    pub fn activate(&self, id: &str) -> Option<SkillActivationDto> {
        self.manager.activate(id).map(|frag| SkillActivationDto {
            id: id.to_string(),
            body: frag.content,
        })
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
    fn lists_and_previews() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path().join("skills");
        write(
            &ws.join("pdf").join("SKILL.md"),
            "---\nname: PDF\ndescription: Use to \"rotate a pdf\".\n---\nRotate the pdf.",
        );
        let svc = SkillsService::open(Some(ws), tmp.path().join("inst")).unwrap();

        let list = svc.list();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, "pdf");
        assert!(list[0].triggers.contains(&"rotate a pdf".to_string()));

        let preview = svc.preview_activation("please rotate a pdf now").unwrap();
        assert_eq!(preview.id, "pdf");
        assert!(preview.body.contains("Rotate the pdf"));

        // Unrelated query → no activation.
        assert!(svc.preview_activation("what's the weather").is_none());
    }

    #[test]
    fn install_and_uninstall_via_service() {
        let tmp = tempfile::tempdir().unwrap();
        let mut svc = SkillsService::open(None, tmp.path().join("inst")).unwrap();
        assert!(svc.list().is_empty());

        let src = tmp.path().join("commit-helper");
        write(
            &src.join("SKILL.md"),
            "---\nname: Commit Helper\ndescription: \"write a commit\"\n---\nWrite it.",
        );
        let dto = svc.install_from_dir(&src).unwrap();
        assert_eq!(dto.id, "commit-helper");
        assert_eq!(dto.origin, "installed");
        assert_eq!(svc.list().len(), 1);

        assert!(svc.uninstall("commit-helper").unwrap());
        assert!(svc.list().is_empty());
    }

    #[test]
    fn active_activation_by_id() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path().join("skills");
        write(
            &ws.join("fe").join("SKILL.md"),
            "---\nname: FE\ndescription: \"build a dashboard\"\n---\nBuild it well.",
        );
        let svc = SkillsService::open(Some(ws), tmp.path().join("inst")).unwrap();
        let act = svc.activate("fe").unwrap();
        assert!(act.body.contains("Build it well"));
        assert!(svc.activate("missing").is_none());
    }
}
