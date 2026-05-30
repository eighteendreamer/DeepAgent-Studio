//! Filesystem auto-discovery for skills.
//!
//! Mirrors Claude Code's auto-discovery: scan a `skills/` root, find every
//! subdirectory containing a `SKILL.md`, parse its frontmatter into a [`Skill`],
//! and record any bundled `references/`/`examples/`/`scripts/`/`assets/` files
//! in the resource manifest (paths only — Level 3 stays on disk until needed).

use std::path::{Path, PathBuf};

use deepagent_core::error::{CoreError, Result};

use crate::frontmatter;
use crate::skill::{ResourceKind, Skill, SkillOrigin, SkillResource};

/// The conventional `SKILL.md` filename.
pub const SKILL_FILE: &str = "SKILL.md";

/// Discover all skills under `root` (a directory whose immediate children are
/// skill directories). Each child containing a `SKILL.md` is parsed. Children
/// without one, or with invalid/incomplete frontmatter, are skipped (with a
/// warning) rather than failing the whole scan.
pub fn discover(root: impl AsRef<Path>, origin: SkillOrigin) -> Result<Vec<Skill>> {
    let root = root.as_ref();
    if !root.exists() {
        // A missing skills dir is not an error — just no skills.
        return Ok(Vec::new());
    }
    let read = std::fs::read_dir(root)
        .map_err(|e| CoreError::other(format!("read skills dir {}: {e}", root.display())))?;

    let mut skills = Vec::new();
    let mut dirs: Vec<PathBuf> = read
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();

    for dir in dirs {
        match load_skill_dir(&dir, origin) {
            Ok(Some(skill)) => skills.push(skill),
            Ok(None) => {} // no SKILL.md → not a skill dir
            Err(e) => {
                tracing::warn!(dir = %dir.display(), error = %e, "skipping invalid skill");
            }
        }
    }
    Ok(skills)
}

/// Load a single skill directory. Returns `Ok(None)` if it has no `SKILL.md`.
pub fn load_skill_dir(dir: impl AsRef<Path>, origin: SkillOrigin) -> Result<Option<Skill>> {
    let dir = dir.as_ref();
    let skill_md = dir.join(SKILL_FILE);
    if !skill_md.is_file() {
        return Ok(None);
    }

    let raw = std::fs::read_to_string(&skill_md)
        .map_err(|e| CoreError::other(format!("read {}: {e}", skill_md.display())))?;
    let fm = frontmatter::parse(&raw);

    let id = dir
        .file_name()
        .and_then(|n| n.to_str())
        .map(slugify)
        .ok_or_else(|| CoreError::invalid("skill directory has no usable name"))?;

    let Some(mut skill) = Skill::from_frontmatter(id, &fm, origin) else {
        return Err(CoreError::invalid(format!(
            "{} is missing required 'name'/'description' frontmatter",
            skill_md.display()
        )));
    };

    skill.resources = scan_resources(dir);
    Ok(Some(skill))
}

/// Collect bundled resources (Level 3) under a skill directory.
fn scan_resources(dir: &Path) -> Vec<SkillResource> {
    let mut out = Vec::new();
    for (sub, kind) in [
        ("references", ResourceKind::Reference),
        ("examples", ResourceKind::Example),
        ("scripts", ResourceKind::Script),
        ("assets", ResourceKind::Asset),
    ] {
        let sub_dir = dir.join(sub);
        if sub_dir.is_dir() {
            collect_files(&sub_dir, dir, kind, &mut out);
        }
    }
    out.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    out
}

/// Recursively collect files under `base`, recording paths relative to `root`.
fn collect_files(base: &Path, root: &Path, kind: ResourceKind, out: &mut Vec<SkillResource>) {
    let Ok(rd) = std::fs::read_dir(base) else {
        return;
    };
    for entry in rd.filter_map(|e| e.ok()) {
        let path = entry.path();
        if path.is_dir() {
            collect_files(&path, root, kind, out);
        } else if let Ok(rel) = path.strip_prefix(root) {
            out.push(SkillResource {
                kind,
                rel_path: rel.to_string_lossy().replace('\\', "/"),
            });
        }
    }
}

/// Turn a directory name into a stable lower-case slug id.
pub fn slugify(name: &str) -> String {
    let mut slug = String::with_capacity(name.len());
    let mut prev_dash = false;
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash && !slug.is_empty() {
            slug.push('-');
            prev_dash = true;
        }
    }
    slug.trim_end_matches('-').to_string()
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

    #[test]
    fn slugify_normalizes() {
        assert_eq!(slugify("Hook Development"), "hook-development");
        assert_eq!(slugify("PDF_Editor!!"), "pdf-editor");
        assert_eq!(slugify("already-slug"), "already-slug");
    }

    #[test]
    fn discover_finds_skill_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(
            &root.join("pdf-editor").join("SKILL.md"),
            "---\nname: PDF Editor\ndescription: Use to \"rotate a pdf\".\n---\nRotate it.",
        );
        write(
            &root.join("pdf-editor").join("scripts").join("rotate.py"),
            "print('rotate')",
        );
        // A non-skill dir is ignored.
        fs::create_dir_all(root.join("not-a-skill")).unwrap();

        let skills = discover(root, SkillOrigin::Workspace).unwrap();
        assert_eq!(skills.len(), 1);
        let s = &skills[0];
        assert_eq!(s.meta.id, "pdf-editor");
        assert_eq!(s.meta.name, "PDF Editor");
        assert_eq!(s.resources.len(), 1);
        assert_eq!(s.resources[0].rel_path, "scripts/rotate.py");
        assert_eq!(s.resources[0].kind, ResourceKind::Script);
    }

    #[test]
    fn discover_missing_root_is_empty() {
        let skills = discover("/nonexistent/skills/path", SkillOrigin::Workspace).unwrap();
        assert!(skills.is_empty());
    }

    #[test]
    fn invalid_skill_is_skipped_not_fatal() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        // Missing description → invalid, skipped.
        write(
            &root.join("bad").join("SKILL.md"),
            "---\nname: Bad\n---\nbody",
        );
        // Valid one alongside.
        write(
            &root.join("good").join("SKILL.md"),
            "---\nname: Good\ndescription: \"do good\"\n---\nbody",
        );
        let skills = discover(root, SkillOrigin::Workspace).unwrap();
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].meta.id, "good");
    }

    #[test]
    fn load_single_dir_without_skill_md_is_none() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join("empty")).unwrap();
        let res = load_skill_dir(tmp.path().join("empty"), SkillOrigin::User).unwrap();
        assert!(res.is_none());
    }
}
