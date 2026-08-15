//! Skill discovery at the fixed `skills/` location (spec §7.1).
//!
//! §7.1 is narrow and precise:
//!
//! > The fixed discovery location is `skills/`. Each immediate child directory
//! > containing a path named exactly `SKILL.md` that resolves to a regular file
//! > is treated as one skill. Clients MUST NOT recursively search deeper
//! > descendants for additional skills.
//!
//! Three consequences shape this module:
//!
//! - **Only immediate children.** A `skills/a/nested/SKILL.md` is not a skill.
//! - **`skills/SKILL.md` is not a skill either.** The fixed location holds skill
//!   *directories*; the Codex dialect separately allows a manifest to point
//!   `skills` straight at one skill directory, and that behavior belongs to the
//!   dialect layer, not here.
//! - **"named exactly `SKILL.md`"** is case-sensitive. Windows and macOS default
//!   to case-insensitive filesystems, where `skill.md` would satisfy a naive
//!   `path.join("SKILL.md").is_file()`. This module compares directory entry
//!   names instead, so the same plugin is accepted or rejected identically on
//!   every platform.
//!
//! The Agent Skills specification owns the `SKILL.md` *content* contract
//! (frontmatter, `scripts/`, `references/`). This module only locates skills.

use std::path::{Path, PathBuf};

use crate::plugin::model::{ComponentKind, PluginDiagnostic};
use crate::plugin::spec::path::resolve_existing_within;
use crate::plugin::spec::schema::AGENT_PLUGIN_SKILLS_RELATIVE_PATH;

/// The file name §7.1 requires, matched exactly.
pub const SKILL_MANIFEST_FILE_NAME: &str = "SKILL.md";

/// One skill discovered beneath `skills/`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillComponent {
    /// The skill's directory name, which §7.1 treats as its identity within the
    /// plugin.
    pub name: String,
    /// Absolute path to the skill directory.
    pub dir: PathBuf,
    /// Absolute path to its `SKILL.md`.
    pub manifest: PathBuf,
}

/// Discovers skills at the fixed `skills/` location.
///
/// Returns the skills in directory-name order plus any findings. Ordering is
/// stabilized because `read_dir` order is filesystem-defined and an unstable
/// projection would make downstream snapshots flaky.
///
/// A missing `skills/` is not an error (§6.2). A present-but-not-a-directory
/// `skills` marks the component type invalid and leaves other component types
/// loadable (§6.2). A single unusable skill is skipped (§7.1).
pub fn discover_skills(plugin_root: &Path) -> (Vec<SkillComponent>, Vec<PluginDiagnostic>) {
    let mut diagnostics = Vec::new();
    let skills_dir = plugin_root.join(AGENT_PLUGIN_SKILLS_RELATIVE_PATH);

    // §6.2: an absent fixed location is not an error.
    if !skills_dir.exists() {
        return (Vec::new(), diagnostics);
    }

    // §6.2: present but the wrong filesystem kind invalidates this component
    // type only.
    if !skills_dir.is_dir() {
        diagnostics.push(PluginDiagnostic::ComponentInvalid {
            component: ComponentKind::Skills,
            path: Some(skills_dir),
            reason: "`skills` is not a directory".to_string(),
        });
        return (Vec::new(), diagnostics);
    }

    let entries = match std::fs::read_dir(&skills_dir) {
        Ok(entries) => entries,
        Err(error) => {
            diagnostics.push(PluginDiagnostic::ComponentInvalid {
                component: ComponentKind::Skills,
                path: Some(skills_dir),
                reason: format!("cannot read the skills directory: {error}"),
            });
            return (Vec::new(), diagnostics);
        }
    };

    let mut skills = Vec::new();
    for entry in entries.flatten() {
        let dir = entry.path();

        // A non-directory child is simply not a skill. A stray README or the
        // plugin author's notes must not produce noise.
        if !dir.is_dir() {
            continue;
        }
        let Some(name) = dir.file_name().and_then(|name| name.to_str()) else {
            diagnostics.push(PluginDiagnostic::SkillSkipped {
                path: dir.clone(),
                reason: "directory name is not valid UTF-8".to_string(),
            });
            continue;
        };

        // "named exactly SKILL.md": compare the real entry name so a
        // case-insensitive filesystem cannot smuggle in `skill.md`.
        let Some(manifest) = exact_skill_manifest(&dir) else {
            // No SKILL.md means this directory is not a skill (§7.1). That is
            // not a failure — `skills/shared/` holding helpers is legitimate.
            continue;
        };

        // §4.1 failure boundary 3: a SKILL.md resolving outside the plugin root
        // is skipped rather than read.
        match resolve_existing_within(plugin_root, &manifest) {
            Ok(_) => skills.push(SkillComponent {
                name: name.to_string(),
                dir,
                manifest,
            }),
            Err(error) => diagnostics.push(PluginDiagnostic::SkillSkipped {
                path: dir,
                reason: format!("SKILL.md does not resolve inside the plugin root: {error}"),
            }),
        }
    }

    skills.sort_by(|a, b| a.name.cmp(&b.name));
    (skills, diagnostics)
}

/// Returns the path to `SKILL.md` when the directory holds an entry with that
/// exact name which resolves to a regular file.
fn exact_skill_manifest(dir: &Path) -> Option<PathBuf> {
    let entries = std::fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        if entry.file_name() != SKILL_MANIFEST_FILE_NAME {
            continue;
        }
        let path = entry.path();
        // `metadata` follows symlinks: §7.1 asks whether the path *resolves to*
        // a regular file, so a link to a real file qualifies.
        if std::fs::metadata(&path).is_ok_and(|meta| meta.is_file()) {
            return Some(path);
        }
        return None;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, contents: &str) {
        std::fs::create_dir_all(path.parent().expect("parent")).expect("create parent");
        std::fs::write(path, contents).expect("write");
    }

    fn skill(root: &Path, name: &str) {
        write(
            &root.join("skills").join(name).join("SKILL.md"),
            "---\nname: demo\ndescription: demo\n---\n",
        );
    }

    #[test]
    fn discovers_immediate_child_directories() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        skill(root, "deploy");
        skill(root, "summarize");

        let (skills, diagnostics) = discover_skills(root);

        assert_eq!(
            skills.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
            vec!["deploy", "summarize"]
        );
        assert!(diagnostics.is_empty());
        assert_eq!(skills[0].dir, root.join("skills").join("deploy"));
        assert_eq!(skills[0].manifest, skills[0].dir.join("SKILL.md"));
    }

    /// §7.1 forbids recursing into deeper descendants. This is the rule most
    /// likely to be broken by a convenience `walkdir`.
    #[test]
    fn does_not_recurse_into_deeper_descendants() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        write(
            &root
                .join("skills")
                .join("a")
                .join("nested")
                .join("SKILL.md"),
            "nested",
        );

        let (skills, diagnostics) = discover_skills(root);

        assert!(skills.is_empty(), "nested SKILL.md must not be discovered");
        assert!(
            diagnostics.is_empty(),
            "a non-skill directory is not a finding"
        );
    }

    /// A skill at the top level plus a nested one: only the top level counts,
    /// and the nested file must not inflate the result.
    #[test]
    fn nested_manifest_does_not_add_a_second_skill() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        skill(root, "deploy");
        write(
            &root
                .join("skills")
                .join("deploy")
                .join("inner")
                .join("SKILL.md"),
            "inner",
        );

        let (skills, _) = discover_skills(root);

        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "deploy");
    }

    /// §7.1: the fixed location holds skill *directories*. A `SKILL.md` sitting
    /// directly in `skills/` is not a skill.
    #[test]
    fn skill_manifest_directly_in_skills_dir_is_not_a_skill() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        write(&root.join("skills").join("SKILL.md"), "loose");

        let (skills, diagnostics) = discover_skills(root);

        assert!(skills.is_empty());
        assert!(diagnostics.is_empty());
    }

    /// §6.2: an absent fixed location is not an error.
    #[test]
    fn missing_skills_dir_is_not_an_error() {
        let tmp = tempfile::tempdir().expect("tempdir");

        let (skills, diagnostics) = discover_skills(tmp.path());

        assert!(skills.is_empty());
        assert!(diagnostics.is_empty());
    }

    /// §6.2: present but the wrong filesystem kind invalidates this component
    /// type, and the caller keeps loading the others.
    #[test]
    fn skills_as_a_file_is_reported_as_invalid_component() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        write(&root.join("skills"), "not a directory");

        let (skills, diagnostics) = discover_skills(root);

        assert!(skills.is_empty());
        assert_eq!(diagnostics.len(), 1);
        match &diagnostics[0] {
            PluginDiagnostic::ComponentInvalid {
                component,
                reason,
                path,
            } => {
                assert_eq!(*component, ComponentKind::Skills);
                assert!(reason.contains("not a directory"));
                assert!(path.is_some());
            }
            other => panic!("expected ComponentInvalid, got {other:?}"),
        }
    }

    /// A directory without `SKILL.md` is not a skill and not a finding —
    /// `skills/shared/` holding helper files is legitimate.
    #[test]
    fn directory_without_manifest_is_silently_not_a_skill() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        write(
            &root.join("skills").join("shared").join("helper.md"),
            "help",
        );
        skill(root, "real");

        let (skills, diagnostics) = discover_skills(root);

        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "real");
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn loose_files_beside_skill_dirs_are_ignored() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        skill(root, "deploy");
        write(&root.join("skills").join("README.md"), "readme");

        let (skills, diagnostics) = discover_skills(root);

        assert_eq!(skills.len(), 1);
        assert!(diagnostics.is_empty());
    }

    /// "named exactly SKILL.md" — on a case-insensitive filesystem a naive
    /// `join("SKILL.md").is_file()` would accept this.
    #[test]
    fn lowercase_manifest_name_is_not_accepted() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        write(
            &root.join("skills").join("deploy").join("skill.md"),
            "lower",
        );

        let (skills, diagnostics) = discover_skills(root);

        assert!(
            skills.is_empty(),
            "only an entry named exactly SKILL.md counts"
        );
        assert!(diagnostics.is_empty());
    }

    /// A `SKILL.md` that is a directory does not "resolve to a regular file".
    #[test]
    fn manifest_as_a_directory_is_not_a_skill() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        std::fs::create_dir_all(root.join("skills").join("deploy").join("SKILL.md"))
            .expect("create dir");

        let (skills, diagnostics) = discover_skills(root);

        assert!(skills.is_empty());
        assert!(diagnostics.is_empty());
    }

    /// Results are ordered by name so downstream snapshots stay stable
    /// regardless of filesystem `read_dir` order.
    #[test]
    fn results_are_ordered_by_name() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        for name in ["zeta", "alpha", "middle"] {
            skill(root, name);
        }

        let (skills, _) = discover_skills(root);

        assert_eq!(
            skills.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
            vec!["alpha", "middle", "zeta"]
        );
    }

    /// §4.1 permits a symlink resolving *within* the plugin root and requires
    /// rejecting one that escapes. Creating symlinks needs privileges on
    /// Windows, so the test skips when the platform refuses.
    #[test]
    fn symlinked_manifest_escaping_the_root_is_skipped() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().join("plugin");
        let outside = tmp.path().join("outside");
        std::fs::create_dir_all(root.join("skills").join("escaping")).expect("create skill dir");
        write(&outside.join("SKILL.md"), "outside");

        let link = root.join("skills").join("escaping").join("SKILL.md");
        if !try_symlink_file(&outside.join("SKILL.md"), &link) {
            eprintln!("skipping: platform does not permit creating file symlinks");
            return;
        }

        let (skills, diagnostics) = discover_skills(&root);

        assert!(skills.is_empty(), "an escaping manifest must not be loaded");
        assert_eq!(diagnostics.len(), 1);
        assert!(matches!(
            &diagnostics[0],
            PluginDiagnostic::SkillSkipped { reason, .. }
                if reason.contains("does not resolve inside")
        ));
    }

    #[test]
    fn symlinked_manifest_inside_the_root_is_accepted() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        write(&root.join("shared").join("SKILL.md"), "shared");
        std::fs::create_dir_all(root.join("skills").join("linked")).expect("create skill dir");

        let link = root.join("skills").join("linked").join("SKILL.md");
        if !try_symlink_file(&root.join("shared").join("SKILL.md"), &link) {
            eprintln!("skipping: platform does not permit creating file symlinks");
            return;
        }

        let (skills, diagnostics) = discover_skills(root);

        assert_eq!(skills.len(), 1, "a link staying inside the root is allowed");
        assert_eq!(skills[0].name, "linked");
        assert!(diagnostics.is_empty());
    }

    fn try_symlink_file(target: &Path, link: &Path) -> bool {
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(target, link).is_ok()
        }
        #[cfg(windows)]
        {
            std::os::windows::fs::symlink_file(target, link).is_ok()
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = (target, link);
            false
        }
    }
}
