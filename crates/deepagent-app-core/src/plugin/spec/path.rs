//! Plugin-relative path resolution and root containment per Agent Plugins
//! Specification 1.0.0 §4.1.
//!
//! Two rules from §4.1 drive this module:
//!
//! - A configuration field defined as a plugin-relative path MUST begin with
//!   `./`, resolve against the plugin root, and stay within the
//!   filesystem-resolved plugin root.
//! - Symlinks, junctions, and reparse points MAY resolve to targets *within*
//!   the plugin root, but clients MUST reject package paths that resolve
//!   outside it.
//!
//! That split is why this module exposes two checks. [`resolve_plugin_relative`]
//! is lexical: it validates the declared syntax and joins against the root
//! without touching the filesystem, so a manifest that merely *declares* a
//! missing path still fails for the right reason. [`resolve_existing_within`]
//! is physical: it canonicalizes an existing path so a symlink escaping the
//! root is caught before the path is read or executed.
//!
//! Values that §4.1 defines as opaque strings — command arguments, environment
//! variable values — must not be run through these checks. Doing so would
//! reject legitimate configuration that merely looks path-like.

use std::path::{Component, Path, PathBuf};

/// Why a declared or resolved package path was rejected (§4.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginPathError {
    /// Missing the required `./` prefix.
    NotPluginRelative { raw: String },
    /// Exactly `./`, naming no entry beneath the root.
    EmptyAfterPrefix,
    /// Contains a `..` component.
    ParentComponent { raw: String },
    /// Absolute, root-relative, or drive/UNC qualified.
    Absolute { raw: String },
    /// Resolved outside the plugin root.
    Escapes { raw: String },
    /// The path could not be canonicalized for the physical containment check.
    Unresolvable { raw: String, reason: String },
}

impl std::fmt::Display for PluginPathError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotPluginRelative { raw } => {
                write!(
                    f,
                    "path must start with `./` relative to plugin root: {raw}"
                )
            }
            Self::EmptyAfterPrefix => write!(f, "path must not be `./`"),
            Self::ParentComponent { raw } => write!(f, "path must not contain `..`: {raw}"),
            Self::Absolute { raw } => {
                write!(f, "path must stay within the plugin root: {raw}")
            }
            Self::Escapes { raw } => {
                write!(f, "path resolves outside the plugin root: {raw}")
            }
            Self::Unresolvable { raw, reason } => {
                write!(f, "path could not be resolved: {raw}: {reason}")
            }
        }
    }
}

impl std::error::Error for PluginPathError {}

/// Resolves a declared plugin-relative path lexically.
///
/// Validates the `./` prefix, rejects `..` components and any absolute or
/// drive/UNC-qualified form, then joins against `root`. Performs no filesystem
/// access, so it is safe to call for paths that may not exist yet; call
/// [`resolve_existing_within`] before reading or executing the result.
///
/// Both `/` and `\` are treated as separators regardless of host platform: a
/// plugin authored on Windows must not become traversable when loaded on Linux.
pub fn resolve_plugin_relative(root: &Path, raw: &str) -> Result<PathBuf, PluginPathError> {
    let Some(relative) = raw.strip_prefix("./") else {
        return Err(PluginPathError::NotPluginRelative {
            raw: raw.to_string(),
        });
    };
    if relative.is_empty() {
        return Err(PluginPathError::EmptyAfterPrefix);
    }

    // Check both separators on every platform so a `..\` segment authored on
    // Windows is still rejected when the plugin is loaded on a POSIX host.
    let segments = || relative.split(['/', '\\']);

    if segments().any(|segment| segment == "..") {
        return Err(PluginPathError::ParentComponent {
            raw: raw.to_string(),
        });
    }
    if is_rooted(relative) {
        return Err(PluginPathError::Absolute {
            raw: raw.to_string(),
        });
    }

    let mut resolved = root.to_path_buf();
    for segment in segments() {
        // `./a//b` and `./a/./b` name the same entry as `./a/b`; skipping these
        // keeps the joined path clean without changing its meaning.
        if segment.is_empty() || segment == "." {
            continue;
        }
        resolved.push(segment);
    }

    if resolved == root {
        return Err(PluginPathError::EmptyAfterPrefix);
    }
    if !is_within(root, &resolved) {
        return Err(PluginPathError::Escapes {
            raw: raw.to_string(),
        });
    }
    Ok(resolved)
}

/// Whether `candidate` is lexically inside `root`, treating `root` itself as
/// inside.
///
/// Lexical only — it does not follow symlinks. Use
/// [`resolve_existing_within`] when the path is about to be read or executed.
pub fn is_within(root: &Path, candidate: &Path) -> bool {
    if candidate.components().any(|c| c == Component::ParentDir) {
        return false;
    }
    candidate == root || candidate.starts_with(root)
}

/// Confirms an existing path is inside the filesystem-resolved plugin root.
///
/// Canonicalizes both sides so symlinks, junctions, and reparse points are
/// followed: §4.1 permits them to point *within* the root but requires
/// rejecting anything resolving outside it. Returns the canonical path.
pub fn resolve_existing_within(root: &Path, candidate: &Path) -> Result<PathBuf, PluginPathError> {
    let canonical_root = canonicalize(root)?;
    let canonical = canonicalize(candidate)?;
    if canonical == canonical_root || canonical.starts_with(&canonical_root) {
        Ok(canonical)
    } else {
        Err(PluginPathError::Escapes {
            raw: candidate.display().to_string(),
        })
    }
}

fn canonicalize(path: &Path) -> Result<PathBuf, PluginPathError> {
    path.canonicalize()
        .map_err(|error| PluginPathError::Unresolvable {
            raw: path.display().to_string(),
            reason: error.to_string(),
        })
}

/// Whether a would-be relative path is actually rooted: POSIX absolute, a
/// Windows drive qualifier, or a UNC path.
fn is_rooted(relative: &str) -> bool {
    if relative.starts_with('/') || relative.starts_with('\\') {
        return true;
    }
    // `C:` / `c:\` / `C:relative` are all drive-qualified, so none of them can
    // be trusted to stay under the plugin root.
    matches!(relative.as_bytes(), [drive, b':', ..] if drive.is_ascii_alphabetic())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root() -> PathBuf {
        PathBuf::from(if cfg!(windows) {
            r"C:\plugins\demo"
        } else {
            "/plugins/demo"
        })
    }

    /// The valid example from §4.1: both paths start with `./` and stay inside.
    #[test]
    fn accepts_spec_valid_examples() {
        let root = root();
        assert_eq!(
            resolve_plugin_relative(&root, "./bin/server").unwrap(),
            root.join("bin").join("server")
        );
        assert_eq!(
            resolve_plugin_relative(&root, "./data").unwrap(),
            root.join("data")
        );
    }

    /// The invalid example from §4.1: `../bin/server` escapes the root and
    /// `data` is not a plugin-relative path.
    #[test]
    fn rejects_spec_invalid_examples() {
        let root = root();
        assert_eq!(
            resolve_plugin_relative(&root, "../bin/server"),
            Err(PluginPathError::NotPluginRelative {
                raw: "../bin/server".to_string()
            })
        );
        assert_eq!(
            resolve_plugin_relative(&root, "data"),
            Err(PluginPathError::NotPluginRelative {
                raw: "data".to_string()
            })
        );
    }

    #[test]
    fn rejects_parent_components_after_prefix() {
        let root = root();
        for raw in ["./../escape", "./a/../../escape", "./a/.."] {
            assert_eq!(
                resolve_plugin_relative(&root, raw),
                Err(PluginPathError::ParentComponent {
                    raw: raw.to_string()
                }),
                "expected {raw} to be rejected"
            );
        }
    }

    /// A `..` written with a backslash must be rejected on every platform, not
    /// just Windows — otherwise a Windows-authored plugin becomes traversable
    /// when loaded on Linux.
    #[test]
    fn rejects_backslash_parent_components_on_all_platforms() {
        let root = root();
        assert_eq!(
            resolve_plugin_relative(&root, r".\..\escape"),
            Err(PluginPathError::NotPluginRelative {
                raw: r".\..\escape".to_string()
            })
        );
        assert_eq!(
            resolve_plugin_relative(&root, r"./a\..\escape"),
            Err(PluginPathError::ParentComponent {
                raw: r"./a\..\escape".to_string()
            })
        );
    }

    #[test]
    fn rejects_rooted_paths() {
        let root = root();
        for raw in ["./\\etc\\passwd", ".//etc/passwd", "./C:/windows"] {
            assert_eq!(
                resolve_plugin_relative(&root, raw),
                Err(PluginPathError::Absolute {
                    raw: raw.to_string()
                }),
                "expected {raw} to be rejected"
            );
        }
    }

    #[test]
    fn rejects_bare_prefix() {
        let root = root();
        assert_eq!(
            resolve_plugin_relative(&root, "./"),
            Err(PluginPathError::EmptyAfterPrefix)
        );
        assert_eq!(
            resolve_plugin_relative(&root, "./."),
            Err(PluginPathError::EmptyAfterPrefix)
        );
    }

    #[test]
    fn rejects_paths_without_prefix() {
        let root = root();
        for raw in ["", "bin/server", "/abs", "~/home", "${PLUGIN_ROOT}/x"] {
            assert!(
                matches!(
                    resolve_plugin_relative(&root, raw),
                    Err(PluginPathError::NotPluginRelative { .. })
                ),
                "expected {raw:?} to be rejected as not plugin-relative"
            );
        }
    }

    #[test]
    fn normalizes_redundant_separators_and_dots() {
        let root = root();
        assert_eq!(
            resolve_plugin_relative(&root, "./a//b").unwrap(),
            root.join("a").join("b")
        );
        assert_eq!(
            resolve_plugin_relative(&root, "./a/./b").unwrap(),
            root.join("a").join("b")
        );
    }

    #[test]
    fn is_within_accepts_root_and_descendants() {
        let root = root();
        assert!(is_within(&root, &root));
        assert!(is_within(&root, &root.join("skills")));
        assert!(is_within(&root, &root.join("skills").join("a")));
    }

    #[test]
    fn is_within_rejects_siblings_and_traversal() {
        let root = root();
        let sibling = root.parent().unwrap().join("other");
        assert!(!is_within(&root, &sibling));
        assert!(!is_within(&root, &root.join("..").join("other")));
    }

    /// `starts_with` compares whole components, so a sibling directory sharing
    /// a name prefix must not be treated as contained.
    #[test]
    fn is_within_is_not_fooled_by_name_prefixes() {
        let root = root();
        let lookalike = PathBuf::from(format!("{}-evil", root.display()));
        assert!(!is_within(&root, &lookalike));
    }

    #[test]
    fn resolve_existing_within_accepts_real_descendant() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        let nested = root.join("skills").join("demo");
        std::fs::create_dir_all(&nested).expect("create nested");

        let resolved = resolve_existing_within(root, &nested).expect("nested is inside root");
        assert!(resolved.ends_with(PathBuf::from("skills").join("demo")));
    }

    #[test]
    fn resolve_existing_within_rejects_outside_path() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().join("plugin");
        let outside = tmp.path().join("outside");
        std::fs::create_dir_all(&root).expect("create root");
        std::fs::create_dir_all(&outside).expect("create outside");

        assert!(matches!(
            resolve_existing_within(&root, &outside),
            Err(PluginPathError::Escapes { .. })
        ));
    }

    #[test]
    fn resolve_existing_within_reports_missing_paths() {
        let tmp = tempfile::tempdir().expect("tempdir");
        assert!(matches!(
            resolve_existing_within(tmp.path(), &tmp.path().join("absent")),
            Err(PluginPathError::Unresolvable { .. })
        ));
    }

    /// §4.1 permits symlinks that resolve *within* the root and requires
    /// rejecting those that escape. Creating symlinks needs privileges on
    /// Windows, so the test skips when the platform refuses.
    #[test]
    fn resolve_existing_within_follows_symlinks() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().join("plugin");
        let inside_target = root.join("real");
        let outside_target = tmp.path().join("outside");
        std::fs::create_dir_all(&inside_target).expect("create inside target");
        std::fs::create_dir_all(&outside_target).expect("create outside target");

        let inside_link = root.join("inside-link");
        let escaping_link = root.join("escaping-link");
        if !try_symlink_dir(&inside_target, &inside_link)
            || !try_symlink_dir(&outside_target, &escaping_link)
        {
            eprintln!("skipping: platform does not permit creating directory symlinks");
            return;
        }

        // Resolves within the root: allowed, and canonicalized to the target.
        let resolved = resolve_existing_within(&root, &inside_link).expect("link stays inside");
        assert_eq!(resolved, inside_target.canonicalize().expect("canonical"));

        // Resolves outside the root: rejected even though the link itself sits
        // inside. This is the case a lexical check alone would miss.
        assert!(matches!(
            resolve_existing_within(&root, &escaping_link),
            Err(PluginPathError::Escapes { .. })
        ));
    }

    fn try_symlink_dir(target: &Path, link: &Path) -> bool {
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(target, link).is_ok()
        }
        #[cfg(windows)]
        {
            std::os::windows::fs::symlink_dir(target, link).is_ok()
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = (target, link);
            false
        }
    }
}
