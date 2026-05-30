//! Skill installation / uninstallation into a managed skills root.
//!
//! This is the "download & install" half of the skill system. Network fetching
//! (HTTP/git) is intentionally *not* in this crate — like `deepagent-models`
//! and `deepagent-mcp`, the heavy/networked transport lives behind the app
//! shell. Here we own the local, offline-safe, deterministic steps:
//!
//! - **install from a prepared source directory** (an already-downloaded /
//!   unpacked skill folder containing a `SKILL.md`) into the managed root,
//! - **uninstall** a previously installed skill by id,
//! - **list** installed skills,
//!
//! so the desktop app can: download → unpack to temp → [`SkillInstaller::install_dir`].
//! Each installed skill lives at `<root>/<id>/` and is loaded with
//! [`SkillOrigin::Installed`].

use std::path::{Path, PathBuf};

use deepagent_core::error::{CoreError, Result};

use crate::loader::{self, SKILL_FILE};
use crate::skill::{Skill, SkillOrigin};

/// Manages a directory of installed skills (`<root>/<id>/SKILL.md`).
#[derive(Debug, Clone)]
pub struct SkillInstaller {
    root: PathBuf,
}

impl SkillInstaller {
    /// Build an installer over a managed skills root. The directory is created
    /// on first install if absent.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// The managed root path.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Install a skill from a prepared source directory (must contain a valid
    /// `SKILL.md`). The skill is copied to `<root>/<id>/` and loaded with
    /// [`SkillOrigin::Installed`]. An existing install with the same id is
    /// replaced.
    pub fn install_dir(&self, source: impl AsRef<Path>) -> Result<Skill> {
        let source = source.as_ref();
        let skill_md = source.join(SKILL_FILE);
        if !skill_md.is_file() {
            return Err(CoreError::invalid(format!(
                "source has no {SKILL_FILE}: {}",
                source.display()
            )));
        }

        // Parse first to derive the id and validate frontmatter.
        let parsed = loader::load_skill_dir(source, SkillOrigin::Installed)?
            .ok_or_else(|| CoreError::invalid("source is not a valid skill"))?;
        let id = parsed.meta.id.clone();

        let dest = self.root.join(&id);
        if dest.exists() {
            std::fs::remove_dir_all(&dest)
                .map_err(|e| CoreError::other(format!("replace existing skill {id}: {e}")))?;
        }
        std::fs::create_dir_all(&dest)
            .map_err(|e| CoreError::other(format!("create skill dir {id}: {e}")))?;
        copy_tree(source, &dest)?;

        // Reload from the installed location so resource paths are correct.
        loader::load_skill_dir(&dest, SkillOrigin::Installed)?
            .ok_or_else(|| CoreError::other("installed skill failed to reload"))
    }

    /// Uninstall a skill by id. Returns `true` if it existed and was removed.
    pub fn uninstall(&self, id: &str) -> Result<bool> {
        let dest = self.root.join(id);
        if !dest.exists() {
            return Ok(false);
        }
        std::fs::remove_dir_all(&dest)
            .map_err(|e| CoreError::other(format!("uninstall skill {id}: {e}")))?;
        Ok(true)
    }

    /// Whether a skill with `id` is installed.
    pub fn is_installed(&self, id: &str) -> bool {
        self.root.join(id).join(SKILL_FILE).is_file()
    }

    /// Load all installed skills (auto-discovery over the managed root).
    pub fn installed(&self) -> Result<Vec<Skill>> {
        loader::discover(&self.root, SkillOrigin::Installed)
    }
}

/// Recursively copy a directory tree from `src` to `dst` (which must exist).
fn copy_tree(src: &Path, dst: &Path) -> Result<()> {
    let rd = std::fs::read_dir(src)
        .map_err(|e| CoreError::other(format!("read {}: {e}", src.display())))?;
    for entry in rd.filter_map(|e| e.ok()) {
        let from = entry.path();
        let name = entry.file_name();
        let to = dst.join(&name);
        if from.is_dir() {
            std::fs::create_dir_all(&to)
                .map_err(|e| CoreError::other(format!("mkdir {}: {e}", to.display())))?;
            copy_tree(&from, &to)?;
        } else {
            std::fs::copy(&from, &to)
                .map_err(|e| CoreError::other(format!("copy {}: {e}", from.display())))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write(path: &Path, content: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, content).unwrap();
    }

    /// Build a prepared source skill dir in a temp location.
    fn make_source(dir: &Path) {
        write(
            &dir.join(SKILL_FILE),
            "---\nname: PDF Editor\ndescription: Use to \"rotate a pdf\".\nversion: 1.0.0\n---\nRotate the PDF.",
        );
        write(&dir.join("scripts").join("rotate.py"), "print('x')");
        write(&dir.join("references").join("schema.md"), "# schema");
    }

    #[test]
    fn install_copies_and_loads() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("download").join("pdf-editor");
        make_source(&src);
        let installer = SkillInstaller::new(tmp.path().join("installed"));

        let skill = installer.install_dir(&src).unwrap();
        assert_eq!(skill.meta.id, "pdf-editor");
        assert_eq!(skill.meta.origin, SkillOrigin::Installed);
        // Resources copied (script + reference).
        assert_eq!(skill.resources.len(), 2);
        assert!(installer.is_installed("pdf-editor"));
        // Files are physically present in the managed root.
        assert!(installer
            .root()
            .join("pdf-editor")
            .join("scripts")
            .join("rotate.py")
            .is_file());
    }

    #[test]
    fn install_then_list_then_uninstall() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("pdf-editor");
        make_source(&src);
        let installer = SkillInstaller::new(tmp.path().join("installed"));

        installer.install_dir(&src).unwrap();
        let listed = installer.installed().unwrap();
        assert_eq!(listed.len(), 1);

        assert!(installer.uninstall("pdf-editor").unwrap());
        assert!(!installer.is_installed("pdf-editor"));
        assert!(installer.installed().unwrap().is_empty());
        // Uninstalling again is a no-op false.
        assert!(!installer.uninstall("pdf-editor").unwrap());
    }

    #[test]
    fn install_replaces_existing() {
        let tmp = tempfile::tempdir().unwrap();
        let installer = SkillInstaller::new(tmp.path().join("installed"));

        // Same directory name → same id → second install replaces the first.
        let src = tmp.path().join("pdf-editor");
        write(
            &src.join(SKILL_FILE),
            "---\nname: PDF Editor\ndescription: \"rotate a pdf\"\nversion: 1.0.0\n---\nV1",
        );
        installer.install_dir(&src).unwrap();

        write(
            &src.join(SKILL_FILE),
            "---\nname: PDF Editor\ndescription: \"rotate a pdf\"\nversion: 2.0.0\n---\nV2",
        );
        let reinstalled = installer.install_dir(&src).unwrap();
        assert_eq!(reinstalled.meta.id, "pdf-editor");
        assert_eq!(reinstalled.meta.version.as_deref(), Some("2.0.0"));
        assert!(reinstalled.body.contains("V2"));
        // Only one installed skill remains.
        assert_eq!(installer.installed().unwrap().len(), 1);
    }

    #[test]
    fn install_invalid_source_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("empty");
        fs::create_dir_all(&src).unwrap();
        let installer = SkillInstaller::new(tmp.path().join("installed"));
        assert!(installer.install_dir(&src).is_err());
    }
}
