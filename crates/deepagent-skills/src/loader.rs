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
///
/// This is equivalent to [`discover_recursive`] with `max_depth = 1`.
pub fn discover(root: impl AsRef<Path>, origin: SkillOrigin) -> Result<Vec<Skill>> {
    discover_recursive(root, origin, 1)
}

/// Recursively discover skills under `root` up to `max_depth` levels deep.
///
/// Each directory containing a `SKILL.md` becomes a skill. If a parent and
/// its child both have `SKILL.md`, the parent wins — once a directory is
/// recognized as a skill, its subtree is *not* further explored (the parent
/// skill controls its own resources).
///
/// `max_depth = 1` matches the legacy [`discover`] behavior: only the
/// immediate children of `root` are inspected. `max_depth = 3` is the default
/// used by the marketplace loader so nested layouts like
/// `superpowers/skills/<sub>/SKILL.md` are picked up.
///
/// Invalid skills (missing required frontmatter) are logged and skipped, not
/// fatal. A missing `root` returns an empty list.
pub fn discover_recursive(
    root: impl AsRef<Path>,
    origin: SkillOrigin,
    max_depth: usize,
) -> Result<Vec<Skill>> {
    discover_recursive_excluding(root, origin, max_depth, &[])
}

/// Like [`discover_recursive`], but skips any directory whose canonical path
/// matches one of `excluded`. Used when one storage root is nested inside
/// another (e.g. the marketplace subdir lives under the user root) so we
/// don't double-register the same skill under two origins.
pub fn discover_recursive_excluding(
    root: impl AsRef<Path>,
    origin: SkillOrigin,
    max_depth: usize,
    excluded: &[PathBuf],
) -> Result<Vec<Skill>> {
    let root = root.as_ref();
    if !root.exists() {
        // A missing skills dir is not an error — just no skills.
        return Ok(Vec::new());
    }
    if max_depth == 0 {
        return Ok(Vec::new());
    }
    let canonical_excluded: Vec<PathBuf> = excluded
        .iter()
        .map(|p| std::fs::canonicalize(p).unwrap_or_else(|_| p.clone()))
        .collect();
    let mut out = Vec::new();
    walk_recursive(root, origin, 1, max_depth, &canonical_excluded, &mut out)?;
    Ok(out)
}

fn walk_recursive(
    dir: &Path,
    origin: SkillOrigin,
    depth: usize,
    max_depth: usize,
    excluded: &[PathBuf],
    out: &mut Vec<Skill>,
) -> Result<()> {
    if depth > max_depth {
        return Ok(());
    }
    let read = std::fs::read_dir(dir)
        .map_err(|e| CoreError::other(format!("read skills dir {}: {e}", dir.display())))?;
    let mut children: Vec<PathBuf> = read
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .filter(|p| !is_excluded(p, excluded))
        .collect();
    children.sort();

    for child in children {
        if child.join(SKILL_FILE).is_file() {
            // Parent wins: this directory is a skill. Register it and stop —
            // its subtree (resources, nested SKILL.mds) is owned by the skill.
            match load_skill_dir(&child, origin) {
                Ok(Some(skill)) => out.push(skill),
                Ok(None) => {
                    // Unreachable: we just verified SKILL.md exists.
                }
                Err(e) => {
                    tracing::warn!(dir = %child.display(), error = %e, "skipping invalid skill");
                }
            }
        } else if depth < max_depth {
            // No SKILL.md here — keep looking deeper.
            walk_recursive(&child, origin, depth + 1, max_depth, excluded, out)?;
        }
    }
    Ok(())
}

fn is_excluded(p: &Path, excluded: &[PathBuf]) -> bool {
    if excluded.is_empty() {
        return false;
    }
    let canonical = std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    excluded.iter().any(|e| e == &canonical || e == p)
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
            "{} is empty or missing a usable description/body",
            skill_md.display()
        )));
    };

    skill.resources = scan_resources(dir);
    // Record the absolute skill directory so the `skill` tool can surface
    // Level-3 resource paths to the model. Fall back to the (possibly
    // relative) input path if canonicalization fails (e.g. on platforms
    // where the path can't be resolved at this moment).
    let abs_dir = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    skill.base_dir = Some(abs_dir);
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
        write(&root.join("bad").join("SKILL.md"), "---\nname: Bad\n---\n");
        // Valid one alongside.
        write(
            &root.join("good").join("SKILL.md"),
            "---\nname: Good\n---\nbody fallback",
        );
        let skills = discover(root, SkillOrigin::Workspace).unwrap();
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].meta.id, "good");
        assert_eq!(skills[0].meta.description, "body fallback");
    }

    #[test]
    fn discover_accepts_skill_without_frontmatter() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(
            &root.join("meeting-notes").join("SKILL.md"),
            "# Meeting Notes\n\nUse this when turning transcripts into minutes.",
        );

        let skills = discover(root, SkillOrigin::Workspace).unwrap();
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].meta.id, "meeting-notes");
        assert_eq!(skills[0].meta.name, "meeting-notes");
        assert_eq!(
            skills[0].meta.description,
            "Use this when turning transcripts into minutes."
        );
    }

    #[test]
    fn load_single_dir_without_skill_md_is_none() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join("empty")).unwrap();
        let res = load_skill_dir(tmp.path().join("empty"), SkillOrigin::User).unwrap();
        assert!(res.is_none());
    }

    #[test]
    fn discover_recursive_depth_one_matches_discover() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(
            &root.join("alpha").join("SKILL.md"),
            "---\nname: Alpha\ndescription: \"do alpha\"\n---\nbody",
        );
        write(
            &root.join("beta").join("SKILL.md"),
            "---\nname: Beta\ndescription: \"do beta\"\n---\nbody",
        );
        // A nested skill at depth 2 — should NOT be picked up at depth 1.
        write(
            &root.join("nested").join("inner").join("SKILL.md"),
            "---\nname: Inner\ndescription: \"do inner\"\n---\nbody",
        );

        let legacy = discover(root, SkillOrigin::Workspace).unwrap();
        let recursive = discover_recursive(root, SkillOrigin::Workspace, 1).unwrap();

        // Same set, same order, same contents.
        assert_eq!(legacy.len(), recursive.len());
        let legacy_ids: Vec<_> = legacy.iter().map(|s| s.meta.id.clone()).collect();
        let recursive_ids: Vec<_> = recursive.iter().map(|s| s.meta.id.clone()).collect();
        assert_eq!(legacy_ids, recursive_ids);
        // Inner skill not picked up at depth 1.
        assert!(!recursive_ids.iter().any(|id| id == "inner"));
    }

    #[test]
    fn discover_recursive_finds_nested_skill_at_depth_three() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        // Mimic the superpowers layout: <root>/superpowers/skills/<sub>/SKILL.md
        write(
            &root
                .join("superpowers")
                .join("skills")
                .join("debugging")
                .join("SKILL.md"),
            "---\nname: Debugging\ndescription: \"systematic debugging\"\n---\nbody",
        );
        write(
            &root
                .join("superpowers")
                .join("skills")
                .join("tdd")
                .join("SKILL.md"),
            "---\nname: TDD\ndescription: \"red green refactor\"\n---\nbody",
        );
        // A flat skill at depth 1 alongside the nested tree.
        write(
            &root.join("flat").join("SKILL.md"),
            "---\nname: Flat\ndescription: \"flat top-level\"\n---\nbody",
        );

        let skills = discover_recursive(root, SkillOrigin::BuiltIn, 3).unwrap();
        let ids: Vec<_> = skills.iter().map(|s| s.meta.id.clone()).collect();
        assert!(ids.contains(&"flat".to_string()));
        assert!(ids.contains(&"debugging".to_string()));
        assert!(ids.contains(&"tdd".to_string()));
        assert_eq!(skills.len(), 3);

        // depth 2 finds flat but not debugging/tdd (those are at depth 3).
        let shallow = discover_recursive(root, SkillOrigin::BuiltIn, 2).unwrap();
        let shallow_ids: Vec<_> = shallow.iter().map(|s| s.meta.id.clone()).collect();
        assert!(shallow_ids.contains(&"flat".to_string()));
        assert!(!shallow_ids.contains(&"debugging".to_string()));
    }

    #[test]
    fn discover_recursive_parent_wins_over_child() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        // Parent has a SKILL.md AND a nested child also has one.
        write(
            &root.join("parent").join("SKILL.md"),
            "---\nname: Parent\ndescription: \"parent skill\"\n---\nparent body",
        );
        write(
            &root.join("parent").join("nested").join("SKILL.md"),
            "---\nname: Nested\ndescription: \"nested skill\"\n---\nshould be hidden",
        );

        let skills = discover_recursive(root, SkillOrigin::Workspace, 3).unwrap();
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].meta.id, "parent");
        // The nested SKILL.md is shadowed.
        assert!(!skills.iter().any(|s| s.meta.id == "nested"));
    }

    #[test]
    fn discover_recursive_excluding_skips_named_subdir() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        let marketplace = root.join("marketplace");

        write(
            &root.join("user-skill").join("SKILL.md"),
            "---\nname: User Skill\ndescription: \"top-level user skill\"\n---\nbody",
        );
        write(
            &marketplace.join("market-skill").join("SKILL.md"),
            "---\nname: Market Skill\ndescription: \"installed from market\"\n---\nbody",
        );

        // Without exclusion, both are found at depth 3.
        let all = discover_recursive(&root, SkillOrigin::User, 3).unwrap();
        let all_ids: Vec<_> = all.iter().map(|s| s.meta.id.clone()).collect();
        assert!(all_ids.contains(&"user-skill".to_string()));
        assert!(all_ids.contains(&"market-skill".to_string()));

        // With marketplace excluded, only the top-level user skill is found.
        let filtered =
            discover_recursive_excluding(&root, SkillOrigin::User, 3, &[marketplace]).unwrap();
        let filtered_ids: Vec<_> = filtered.iter().map(|s| s.meta.id.clone()).collect();
        assert_eq!(filtered_ids, vec!["user-skill".to_string()]);
    }
}
