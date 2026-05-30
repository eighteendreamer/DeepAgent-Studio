//! Workspace confinement for file tools.
//!
//! Every file-touching built-in resolves user-supplied paths through
//! [`WorkspaceRoot`], which rejects path traversal (`..` escaping the root),
//! absolute paths outside the root, and known-sensitive files (`.env`, key
//! material). This is the path-safety half of the Claude Code `PreToolUse`
//! guard, applied inside the tools themselves so the boundary holds regardless
//! of how the tool is invoked.

use std::path::{Component, Path, PathBuf};

use deepagent_core::error::{CoreError, Result};

/// A confined workspace root. Paths are always resolved relative to it and may
/// never escape it.
#[derive(Debug, Clone)]
pub struct WorkspaceRoot {
    root: PathBuf,
}

/// File/dir names that are refused outright (credentials & secrets).
const SENSITIVE_NAMES: &[&str] = &[
    ".env",
    ".env.local",
    ".env.production",
    "id_rsa",
    "id_ed25519",
    ".npmrc",
    ".pypirc",
    "credentials",
    ".git-credentials",
];

/// Substrings in a filename that mark it sensitive.
const SENSITIVE_SUBSTRINGS: &[&str] = &[".env.", "secret", "credential", ".pem", ".key"];

impl WorkspaceRoot {
    /// Create a root from a directory path. The path is normalized but not
    /// required to exist (tools create files under it).
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// The root path.
    pub fn path(&self) -> &Path {
        &self.root
    }

    /// Resolve a user-supplied relative (or absolute-within-root) path into an
    /// absolute path confined to the root. Rejects traversal and sensitive files.
    pub fn resolve(&self, input: &str) -> Result<PathBuf> {
        let candidate = Path::new(input);

        // Reject explicit parent-dir traversal anywhere in the input.
        if candidate
            .components()
            .any(|c| matches!(c, Component::ParentDir))
        {
            return Err(CoreError::invalid(format!(
                "path traversal ('..') is not allowed: {input}"
            )));
        }

        // Build the absolute path: absolute inputs must already be under root;
        // relative inputs are joined to root.
        let joined = if candidate.is_absolute() {
            if !candidate.starts_with(&self.root) {
                return Err(CoreError::invalid(format!(
                    "absolute path escapes the workspace: {input}"
                )));
            }
            candidate.to_path_buf()
        } else {
            self.root.join(candidate)
        };

        // Sensitive-file check on the final component(s).
        if is_sensitive(&joined) {
            return Err(CoreError::invalid(format!(
                "access to sensitive file is denied: {input}"
            )));
        }

        // Final containment check (defense in depth, lexical).
        if !joined.starts_with(&self.root) {
            return Err(CoreError::invalid(format!(
                "resolved path escapes the workspace: {input}"
            )));
        }

        Ok(joined)
    }

    /// The path relative to the root (for display), or the input if outside.
    pub fn relativize(&self, path: &Path) -> String {
        path.strip_prefix(&self.root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/")
    }
}

/// Whether any component of the path is a sensitive credential file.
fn is_sensitive(path: &Path) -> bool {
    path.components().any(|c| {
        if let Component::Normal(os) = c {
            let name = os.to_string_lossy().to_lowercase();
            SENSITIVE_NAMES.iter().any(|n| name == *n)
                || SENSITIVE_SUBSTRINGS.iter().any(|s| name.contains(s))
        } else {
            false
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root() -> WorkspaceRoot {
        WorkspaceRoot::new("/work/proj")
    }

    #[test]
    fn resolves_relative_path() {
        let r = root();
        let p = r.resolve("src/main.rs").unwrap();
        assert!(p.ends_with("src/main.rs") || p.ends_with("src\\main.rs"));
        assert!(p.starts_with("/work/proj"));
    }

    #[test]
    fn rejects_parent_traversal() {
        let r = root();
        assert!(r.resolve("../etc/passwd").is_err());
        assert!(r.resolve("src/../../secret").is_err());
    }

    #[test]
    fn rejects_absolute_outside_root() {
        let r = root();
        assert!(r.resolve("/etc/passwd").is_err());
    }

    #[test]
    fn allows_absolute_inside_root() {
        let r = root();
        assert!(r.resolve("/work/proj/src/lib.rs").is_ok());
    }

    #[test]
    fn rejects_sensitive_files() {
        let r = root();
        assert!(r.resolve(".env").is_err());
        assert!(r.resolve("config/.env.production").is_err());
        assert!(r.resolve("keys/server.pem").is_err());
        assert!(r.resolve("my_secret_config.json").is_err());
        // ordinary files are fine
        assert!(r.resolve("config/app.toml").is_ok());
    }
}
