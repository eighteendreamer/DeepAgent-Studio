//! Locate the read-only resources an installer ships next to the executable.
//!
//! Built-in skills, plugins and MCP bundles come from the app package, while the
//! user's own assets live in the data root (`crate::data_root::UserPaths`).
//! Both the desktop shell and the CLI resolve them through these probes, so a
//! bundled plugin cannot be found by one host and missed by the other.
//!
//! The candidate lists defend against the per-platform bundle layouts
//! `tauri::path::PathResolver::resource_dir` can produce; see
//! [`locate_builtin_skills_dir`].

use std::path::{Path, PathBuf};

/// Environment variable listing extra session-scoped plugin roots.
pub const SESSION_PLUGIN_ROOTS_ENV: &str = "DEEPAGENT_SESSION_PLUGIN_ROOTS";

/// Locate the bundled built-in skills directory at runtime.
///
/// Production-tested layout (Windows NSIS / standard Tauri 2 desktop bundle)
/// is `<resource_dir>/resources/skills/`. But:
///
/// - macOS `.app` bundles can land at `<resource_dir>/_up_/resources/skills/`
///   when `tauri-bundler` rewrites `..`-rooted resource entries — the `_up_`
///   shim shifts the layout one level deeper.
/// - Some Linux packaging tools (notably AppImage variants) flatten the
///   `resources/` prefix and place the assets directly under
///   `<resource_dir>/skills/`.
/// - A manual zip distribution can hoist things to the resource root itself.
///
/// Probes the candidates in priority order and returns the first directory that
/// *looks like a skills collection* (per [`dir_has_skill_md`]). When nothing
/// matches it returns the production-layout candidate plus a `stderr` warning
/// naming every path tried, so the loader surfaces a clean "no built-in skills
/// found" instead of a panic.
pub fn locate_builtin_skills_dir(resource_dir: &Path) -> PathBuf {
    let candidates: [PathBuf; 4] = [
        resource_dir.join("resources").join("skills"),
        resource_dir.join("skills"),
        resource_dir.join("_up_").join("resources").join("skills"),
        resource_dir.to_path_buf(),
    ];

    for candidate in &candidates {
        if dir_has_skill_md(candidate) {
            return candidate.clone();
        }
    }

    eprintln!(
        "[builtin-skills] no SKILL.md found under any of {} candidate paths; \
         falling back to the production layout. Candidates: {}",
        candidates.len(),
        candidates
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join(" | ")
    );
    candidates
        .into_iter()
        .next()
        .expect("non-empty candidate list")
}

/// True iff `dir` is a directory and *looks like a skills collection root*.
///
/// Two shapes count:
/// 1. **Single-skill root** — `dir/SKILL.md` exists.
/// 2. **Multi-skill collection root** — at least one immediate subdirectory
///    contains a `SKILL.md`.
///
/// Cheap (at most one `read_dir` + per-entry stat) and only runs at startup.
pub fn dir_has_skill_md(dir: &Path) -> bool {
    if !dir.is_dir() {
        return false;
    }
    if dir.join("SKILL.md").is_file() {
        return true;
    }
    let Ok(read) = std::fs::read_dir(dir) else {
        return false;
    };
    for entry in read.flatten() {
        let path = entry.path();
        if path.is_dir() && path.join("SKILL.md").is_file() {
            return true;
        }
    }
    false
}

/// Locate the bundled built-in plugins directory.
///
/// `development_source` is the source tree the shell was built from: Tauri dev
/// copies resources into `target/debug` but never prunes directories removed
/// from source, so reading the source in development stops a deleted bundled
/// plugin from reappearing from a stale resource copy.
pub fn locate_builtin_plugins_dir(
    resource_dir: &Path,
    development_source: Option<&Path>,
) -> PathBuf {
    if let Some(source) = development_source.filter(|path| path.is_dir()) {
        return source.to_path_buf();
    }
    let candidates: [PathBuf; 3] = [
        resource_dir.join("resources").join("plugins"),
        resource_dir.join("plugins"),
        resource_dir.join("_up_").join("resources").join("plugins"),
    ];
    for candidate in &candidates {
        if candidate.is_dir() {
            return candidate.clone();
        }
    }
    candidates
        .into_iter()
        .next()
        .expect("non-empty plugin resource candidate list")
}

/// Locate the bundled built-in MCP directory.
pub fn locate_builtin_mcp_dir(resource_dir: &Path) -> PathBuf {
    let candidates = [
        resource_dir.join("resources").join("mcp"),
        resource_dir.join("mcp"),
        resource_dir.join("_up_").join("resources").join("mcp"),
    ];
    candidates
        .into_iter()
        .find(|path| path.is_dir())
        .unwrap_or_else(|| resource_dir.join("resources").join("mcp"))
}

/// Session-scoped plugin roots contributed over the environment.
pub fn session_plugin_roots_from_env() -> Vec<PathBuf> {
    let Some(raw) = std::env::var_os(SESSION_PLUGIN_ROOTS_ENV) else {
        return Vec::new();
    };
    std::env::split_paths(&raw)
        .filter(|path| path.is_dir())
        .collect()
}

/// The directory a packaged app keeps its shipped resources in, derived from the
/// running executable. Used by hosts that have no `tauri::AppHandle`.
pub fn install_resource_dir() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
}

#[cfg(test)]
mod tests {
    use super::{
        dir_has_skill_md, locate_builtin_mcp_dir, locate_builtin_plugins_dir,
        locate_builtin_skills_dir,
    };
    use std::path::Path;

    fn touch(path: &Path) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(
            path,
            b"---\nname: x\ndescription: \"test fixture\"\n---\nbody",
        )
        .unwrap();
    }

    #[test]
    fn skills_resolver_picks_the_production_layout() {
        let tmp = tempfile::tempdir().unwrap();
        touch(&tmp.path().join("resources/skills/foo/SKILL.md"));
        assert_eq!(
            locate_builtin_skills_dir(tmp.path()),
            tmp.path().join("resources").join("skills")
        );
    }

    #[test]
    fn skills_resolver_falls_through_to_the_flattened_layout() {
        let tmp = tempfile::tempdir().unwrap();
        touch(&tmp.path().join("skills/foo/SKILL.md"));
        assert_eq!(
            locate_builtin_skills_dir(tmp.path()),
            tmp.path().join("skills")
        );
    }

    #[test]
    fn skills_resolver_handles_the_macos_up_shim() {
        let tmp = tempfile::tempdir().unwrap();
        touch(&tmp.path().join("_up_/resources/skills/foo/SKILL.md"));
        assert_eq!(
            locate_builtin_skills_dir(tmp.path()),
            tmp.path().join("_up_").join("resources").join("skills")
        );
    }

    #[test]
    fn skills_resolver_returns_the_production_candidate_when_nothing_matches() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(
            locate_builtin_skills_dir(tmp.path()),
            tmp.path().join("resources").join("skills")
        );
    }

    #[test]
    fn skills_resolver_priority_order_is_stable() {
        let tmp = tempfile::tempdir().unwrap();
        touch(&tmp.path().join("resources/skills/foo/SKILL.md"));
        touch(&tmp.path().join("skills/bar/SKILL.md"));
        touch(&tmp.path().join("_up_/resources/skills/baz/SKILL.md"));
        assert_eq!(
            locate_builtin_skills_dir(tmp.path()),
            tmp.path().join("resources").join("skills")
        );
    }

    #[test]
    fn skill_marker_accepts_a_root_file_or_one_level_deep() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(!dir_has_skill_md(tmp.path()));
        touch(&tmp.path().join("SKILL.md"));
        assert!(dir_has_skill_md(tmp.path()));

        let nested = tempfile::tempdir().unwrap();
        touch(&nested.path().join("one").join("SKILL.md"));
        assert!(dir_has_skill_md(nested.path()));
    }

    #[test]
    fn skill_marker_rejects_empty_wrong_and_too_deep_directories() {
        let empty = tempfile::tempdir().unwrap();
        assert!(!dir_has_skill_md(empty.path()));
        assert!(!dir_has_skill_md(&empty.path().join("missing")));

        let wrong = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(wrong.path().join("one")).unwrap();
        std::fs::write(wrong.path().join("one").join("README.md"), "x").unwrap();
        assert!(!dir_has_skill_md(wrong.path()));

        let deep = tempfile::tempdir().unwrap();
        touch(&deep.path().join("one").join("two").join("SKILL.md"));
        assert!(!dir_has_skill_md(deep.path()));
    }

    #[test]
    fn development_plugins_ignore_stale_copied_resources() {
        let tmp = tempfile::tempdir().unwrap();
        let copied = tmp.path().join("resources").join("plugins");
        let source = tmp.path().join("source").join("plugins");
        std::fs::create_dir_all(copied.join("removed-plugin")).unwrap();
        std::fs::create_dir_all(source.join("browser")).unwrap();

        assert_eq!(
            locate_builtin_plugins_dir(tmp.path(), Some(&source)),
            source
        );
        assert_eq!(locate_builtin_plugins_dir(tmp.path(), None), copied);
    }

    #[test]
    fn mcp_resolver_prefers_the_packaged_prefix_then_flattens() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(
            locate_builtin_mcp_dir(tmp.path()),
            tmp.path().join("resources").join("mcp"),
            "a missing bundle still reports the expected location"
        );
        std::fs::create_dir_all(tmp.path().join("mcp")).unwrap();
        assert_eq!(locate_builtin_mcp_dir(tmp.path()), tmp.path().join("mcp"));
    }
}
