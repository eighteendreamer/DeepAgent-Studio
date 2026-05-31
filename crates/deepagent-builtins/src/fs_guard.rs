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

/// How broadly file tools may reach outside the workspace root. Drives the
/// approval-policy → filesystem mapping:
/// - [`FsAccess::Workspace`] (默认权限): reads and writes confined to the root.
/// - [`FsAccess::ReadAnywhere`] (自动审核): reads may go anywhere on disk
///   (sensitive files still blocked); writes stay confined to the root.
/// - [`FsAccess::Full`] (完全访问): reads and writes may go anywhere
///   (sensitive files still blocked to avoid silent credential leaks).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FsAccess {
    /// Confine reads and writes to the workspace root (the safe default).
    #[default]
    Workspace,
    /// Allow reads anywhere on disk; confine writes to the root.
    ReadAnywhere,
    /// Allow reads and writes anywhere on disk.
    Full,
}

/// A confined workspace root. Paths are always resolved relative to it and may
/// never escape it.
#[derive(Debug, Clone)]
pub struct WorkspaceRoot {
    root: PathBuf,
    access: FsAccess,
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
    /// required to exist (tools create files under it). Defaults to
    /// [`FsAccess::Workspace`] (fully confined).
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            access: FsAccess::Workspace,
        }
    }

    /// Set the filesystem access mode (builder style).
    pub fn with_access(mut self, access: FsAccess) -> Self {
        self.access = access;
        self
    }

    /// The current filesystem access mode.
    pub fn access(&self) -> FsAccess {
        self.access
    }

    /// The root path.
    pub fn path(&self) -> &Path {
        &self.root
    }

    /// Resolve a path for a **read** operation, honoring the access mode:
    /// confined to the root under [`FsAccess::Workspace`], or allowed anywhere
    /// on disk under [`FsAccess::ReadAnywhere`] / [`FsAccess::Full`]. Sensitive
    /// files are always rejected.
    pub fn resolve_read(&self, input: &str) -> Result<PathBuf> {
        match self.access {
            FsAccess::Workspace => self.resolve(input),
            FsAccess::ReadAnywhere | FsAccess::Full => self.resolve_unconfined(input),
        }
    }

    /// Resolve a path for a **write** operation, honoring the access mode:
    /// confined to the root unless [`FsAccess::Full`] is set. Sensitive files
    /// are always rejected.
    pub fn resolve_write(&self, input: &str) -> Result<PathBuf> {
        match self.access {
            FsAccess::Workspace | FsAccess::ReadAnywhere => self.resolve(input),
            FsAccess::Full => self.resolve_unconfined(input),
        }
    }

    /// Resolve a path anywhere on disk (used by the unconfined access modes),
    /// still rejecting known-sensitive credential files.
    fn resolve_unconfined(&self, input: &str) -> Result<PathBuf> {
        let candidate = Path::new(input);
        let joined = if candidate.is_absolute() {
            candidate.to_path_buf()
        } else {
            self.root.join(candidate)
        };
        if is_sensitive(&joined) {
            return Err(CoreError::invalid(format!(
                "access to sensitive file is denied: {input}"
            )));
        }
        Ok(joined)
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

/// Whether a user-supplied path string refers to a sensitive credential file.
/// Public helper for guards that classify a path before resolving it.
pub fn is_sensitive_path(input: &str) -> bool {
    is_sensitive(Path::new(input))
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
