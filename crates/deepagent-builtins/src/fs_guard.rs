//! Workspace confinement for file tools.
//!
//! Every file-touching built-in resolves user-supplied paths through
//! [`WorkspaceRoot`], which rejects path traversal (`..` escaping the root),
//! absolute paths outside the root, and known-sensitive files (`.env`, key
//! material). This is the path-safety half of the Claude Code `PreToolUse`
//! guard, applied inside the tools themselves so the boundary holds regardless
//! of how the tool is invoked.

use std::io;
use std::path::{Component, Path, PathBuf};

use deepagent_core::error::{CoreError, Result};
use deepagent_persistence::fs_commit::canonicalize_with_missing_suffix as physical_with_missing_suffix;

#[cfg(windows)]
mod windows;

mod root;
mod write;
pub(crate) use write::{StagedWrite, WriteTarget};

mod owned;
pub use owned::{validate_storage_name, OwnedDirectory, OwnedDirectoryRoot};

/// How broadly file tools may reach outside the workspace root. Drives the
/// approval-policy → filesystem mapping:
/// - [`FsAccess::Workspace`] (默认权限): reads and writes confined to the root.
/// - [`FsAccess::ReadAnywhere`] (自动审核): reads may go anywhere on disk
///   (sensitive files still blocked); writes stay confined to the root.
/// - [`FsAccess::Full`] (完全访问): reads and writes may go anywhere
///   (sensitive files still blocked to avoid silent credential leaks).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FsAccess {
    /// Confine reads to the workspace root and deny all file writes.
    ReadOnly,
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
    anchor: std::sync::Arc<std::result::Result<root::RootAnchor, (io::ErrorKind, String)>>,
}

/// Internal filesystem observation, not a protocol/UI DTO. The directory entry
/// type is the entry's own type; links may be followed only after authorization.
pub(crate) struct DirectoryEntry {
    pub name: std::ffi::OsString,
    pub is_dir: bool,
    pub is_link: bool,
}

pub(crate) struct DirectorySnapshot {
    pub identity: PathBuf,
    pub entries: Vec<DirectoryEntry>,
}

/// File/dir names that are refused outright (credentials, secrets, and
/// host-control config the model has no business reading/rewriting through
/// its file tools). Aligned with Claude Code's sensitive-file list (§6.2):
/// this system DENIES outright rather than approval-gating — stricter is the
/// safe direction for credentials, and consistent with the existing guard.
/// Host services (McpService / SettingsService / ConfigLoader) read their
/// own config through dedicated code paths, never through these model tools,
/// so denying the model here does not break legitimate host access.
const SENSITIVE_NAMES: &[&str] = &[
    // Environment / credential material.
    ".env",
    ".env.local",
    ".env.production",
    "id_rsa",
    "id_dsa",
    "id_ecdsa",
    "id_ed25519",
    ".npmrc",
    ".pypirc",
    ".netrc",
    ".pgpass",
    ".htpasswd",
    "credentials",
    ".git-credentials",
    // Shell rc / profile files — writable shells are an injection vector
    // (a rewritten rc runs arbitrary code on the next shell).
    ".zshrc",
    ".zshenv",
    ".zprofile",
    ".bashrc",
    ".bash_profile",
    ".profile",
    // Agent/host control config — the model must not read or rewrite the
    // harness's own configuration through its file tools.
    ".mcp.json",
    ".claude.json",
];

/// Substrings in a filename that mark it sensitive.
const SENSITIVE_SUBSTRINGS: &[&str] = &[
    ".env.",
    "secret",
    "credential",
    ".pem",
    ".key",
    ".p12",
    ".pfx",
    ".keystore",
    ".jks",
];

impl WorkspaceRoot {
    /// Create a root from a directory path. The path is normalized but not
    /// required to exist (tools create files under it). Defaults to
    /// [`FsAccess::Workspace`] (fully confined).
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        let anchor = std::sync::Arc::new(
            root::RootAnchor::capture(&root).map_err(|error| (error.kind(), error.to_string())),
        );
        Self {
            root,
            access: FsAccess::Workspace,
            anchor,
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
            FsAccess::ReadOnly | FsAccess::Workspace => self.resolve(input),
            FsAccess::ReadAnywhere | FsAccess::Full => self.resolve_unconfined(input),
        }
    }

    /// Resolve a path for a **write** operation, honoring the access mode:
    /// confined to the root unless [`FsAccess::Full`] is set. Sensitive files
    /// are always rejected.
    pub fn resolve_write(&self, input: &str) -> Result<PathBuf> {
        match self.access {
            FsAccess::ReadOnly => Err(CoreError::invalid(format!(
                "file writes are denied by the current sandbox mode: {input}"
            ))),
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
        self.check_physical(&joined, false)
            .map_err(|error| CoreError::invalid(format!("path authorization failed: {error}")))?;
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

        self.check_physical(&joined, true)
            .map_err(|error| CoreError::invalid(format!("path authorization failed: {error}")))?;
        Ok(joined)
    }

    /// Open the authorized content, not a second string-path lookup after the
    /// check. On Windows the native open refuses reparse traversal in every
    /// component; intended links have already been resolved and authorized.
    pub(crate) fn open_read(&self, path: &Path) -> io::Result<std::fs::File> {
        let _root_pin = self.pin_root(matches!(
            self.access,
            FsAccess::ReadOnly | FsAccess::Workspace
        ))?;
        let physical = self.read_identity(path)?;
        open_physical_read(&physical)
    }

    fn open_write_observation(&self, path: &Path) -> io::Result<Option<std::fs::File>> {
        let _root_pin = self.pin_root(self.access != FsAccess::Full)?;
        let physical = self.write_identity(path)?;
        // A missing actual file is an observation. Authorization errors,
        // including dangling-link NotFound, are never converted to absence.
        match open_physical_read(&physical) {
            Ok(file) => Ok(Some(file)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }

    pub(crate) fn read_identity(&self, path: &Path) -> io::Result<PathBuf> {
        let confined = matches!(self.access, FsAccess::ReadOnly | FsAccess::Workspace);
        self.check_physical(path, confined)
    }

    fn write_identity(&self, path: &Path) -> io::Result<PathBuf> {
        if self.access == FsAccess::ReadOnly {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "file writes are denied",
            ));
        }
        self.check_physical(path, self.access != FsAccess::Full)
    }

    /// Keep final entry replacement semantics while reading its authorized
    /// content (a file link may point elsewhere inside the authorized root).
    pub(crate) fn write_target(
        &self,
        path: &Path,
        create_parents: bool,
    ) -> io::Result<WriteTarget> {
        self.entry_target(path, create_parents)
            .map(|target| target.observe_through(self.clone(), path.to_path_buf()))
    }

    /// Namespace mutations retain the final entry (including a link), while
    /// authorizing and pinning its physical parent through the same boundary.
    pub(crate) fn entry_target(
        &self,
        path: &Path,
        create_parents: bool,
    ) -> io::Result<WriteTarget> {
        let _root_pin = self.pin_root(self.access != FsAccess::Full)?;
        self.write_identity(path)?;
        let parent = path
            .parent()
            .ok_or_else(|| io::Error::other("entry has no parent"))?;
        let name = path
            .file_name()
            .ok_or_else(|| io::Error::other("entry has no name"))?;
        let physical_parent = self.write_identity(parent)?;
        let target = WriteTarget::open(physical_parent.join(name), create_parents)?;
        if self.write_identity(parent)? != physical_parent {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "entry parent identity changed",
            ));
        }
        self.write_identity(path)?;
        Ok(target)
    }

    /// Enumerate the same authorized directory handle that was opened, rather
    /// than re-opening its mutable pathname after authorization on Windows.
    pub(crate) fn read_directory(&self, path: &Path) -> io::Result<DirectorySnapshot> {
        let _root_pin = self.pin_root(matches!(
            self.access,
            FsAccess::ReadOnly | FsAccess::Workspace
        ))?;
        let identity = self.read_identity(path)?;
        #[cfg(windows)]
        let entries = windows::read_directory(&identity)?;
        #[cfg(not(windows))]
        let entries = std::fs::read_dir(&identity)?
            .map(|entry| {
                let entry = entry?;
                let kind = entry.file_type()?;
                Ok(DirectoryEntry {
                    name: entry.file_name(),
                    is_dir: kind.is_dir(),
                    is_link: kind.is_symlink(),
                })
            })
            .collect::<io::Result<Vec<_>>>()?;
        Ok(DirectorySnapshot { identity, entries })
    }

    /// Physical preflight shared by the hook and explicit file tools. This is
    /// not an operation guard for string-based mutation or recursive traversal.
    fn check_physical(&self, path: &Path, confined: bool) -> io::Result<PathBuf> {
        let _root_pin = self.pin_root(confined)?;
        let absolute = std::path::absolute(path)?;
        let root = std::path::absolute(&self.root)?;
        if is_sensitive(path) || (confined && !absolute.starts_with(&root)) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "input path is sensitive or escapes the workspace",
            ));
        }
        let physical_root = if confined {
            self.root_anchor()?.physical.clone()
        } else {
            physical_with_missing_suffix(&root)?
        };
        let physical = physical_with_missing_suffix(&absolute)?;
        let check = |candidate: &Path, traversing_ancestor: bool| -> io::Result<()> {
            if is_sensitive(candidate) {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "access to sensitive physical path is denied",
                ));
            }
            if confined
                && !candidate.starts_with(&physical_root)
                && !(traversing_ancestor && physical_root.starts_with(candidate))
            {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "physical path escapes the workspace",
                ));
            }
            Ok(())
        };
        check(&physical, false)?;
        // The root's own alias is trusted symmetrically; below it every link
        // target must be examined, including hops hidden within one component.
        let start = match absolute.strip_prefix(&root) {
            Ok(relative) => physical_root.join(relative),
            Err(_) => absolute,
        };
        if check_link_chain(&start, check)? != physical {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "physical path changed during authorization",
            ));
        }
        Ok(physical)
    }

    fn pin_root(&self, confined: bool) -> io::Result<Option<root::RootPin>> {
        if !confined {
            return Ok(None);
        }
        self.root_anchor()?.pin(&self.root).map(Some)
    }

    fn root_anchor(&self) -> io::Result<&root::RootAnchor> {
        self.anchor.as_ref().as_ref().map_err(|(kind, message)| {
            io::Error::new(*kind, format!("workspace root capture failed: {message}"))
        })
    }

    /// The path relative to the root (for display), or the input if outside.
    pub fn relativize(&self, path: &Path) -> String {
        path.strip_prefix(&self.root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/")
    }
}

fn open_physical_read(physical: &Path) -> io::Result<std::fs::File> {
    #[cfg(windows)]
    {
        windows::open_read(physical)
    }
    #[cfg(not(windows))]
    {
        // No operation-time no-follow protection is claimed for Unix here.
        std::fs::File::open(physical)
    }
}

/// Walk immediate link targets before canonicalizing the next ordinary entry.
/// This validates policy representations, not an atomic filesystem snapshot;
/// callers still open/pin the authorized identity at their operation boundary.
fn check_link_chain(
    path: &Path,
    check: impl Fn(&Path, bool) -> io::Result<()>,
) -> io::Result<PathBuf> {
    use std::collections::VecDeque;

    // Like the reference permission walker, bound link expansion separately
    // from path depth. Do not silently accept an incomplete chain at the limit.
    const MAX_LINK_HOPS: usize = 40;
    enum Step {
        Component(std::ffi::OsString),
        LinkTargetEnd,
    }
    let mut hops = 0;
    let mut pending: VecDeque<_> = path
        .components()
        .map(|component| Step::Component(component.as_os_str().to_os_string()))
        .collect();
    let mut resolved = PathBuf::new();
    while let Some(step) = pending.pop_front() {
        let part = match step {
            Step::Component(part) => part,
            Step::LinkTargetEnd => {
                // Traversing ancestors is necessary to reach an absolute
                // target, but the target itself must satisfy containment
                // before the original path suffix can return to the root.
                resolved = physical_with_missing_suffix(&resolved)?;
                check(&resolved, false)?;
                continue;
            }
        };
        let component = Path::new(&part).components().next().expect("one component");
        match component {
            Component::Prefix(_) | Component::RootDir => resolved.push(&part),
            Component::CurDir => {}
            Component::ParentDir => {
                resolved.pop();
                check(&resolved, true)?;
            }
            Component::Normal(_) => {
                resolved.push(&part);
                let metadata = match std::fs::symlink_metadata(&resolved) {
                    Ok(metadata) => Some(metadata),
                    Err(error) if error.kind() == io::ErrorKind::NotFound => None,
                    Err(error) => return Err(error),
                };
                if metadata.is_some_and(|metadata| metadata.file_type().is_symlink()) {
                    check(&resolved, true)?;
                    hops += 1;
                    if hops > MAX_LINK_HOPS {
                        return Err(io::Error::new(
                            io::ErrorKind::PermissionDenied,
                            "file link chain exceeds the authorization limit",
                        ));
                    }
                    let target = std::fs::read_link(&resolved)?;
                    resolved.pop();
                    let target = resolved.join(target);
                    if is_sensitive(&target) {
                        return Err(io::Error::new(
                            io::ErrorKind::PermissionDenied,
                            "access to sensitive intermediate link target is denied",
                        ));
                    }
                    pending = target
                        .components()
                        .map(|component| Step::Component(component.as_os_str().to_os_string()))
                        .chain(std::iter::once(Step::LinkTargetEnd))
                        .chain(pending)
                        .collect();
                    resolved.clear();
                } else {
                    resolved = physical_with_missing_suffix(&resolved)?;
                    check(&resolved, true)?;
                }
            }
        }
    }
    // A bare filesystem/volume root has no Normal component to normalize it.
    resolved = physical_with_missing_suffix(&resolved)?;
    check(&resolved, false)?;
    Ok(resolved)
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

    #[test]
    fn rejects_expanded_sensitive_list() {
        let r = root();
        // Shell rc / profile (injection vector).
        for name in [".zshrc", ".bashrc", ".bash_profile", ".zshenv", ".profile"] {
            assert!(r.resolve(name).is_err(), "{name} must be sensitive");
        }
        // Host-control config.
        assert!(r.resolve(".mcp.json").is_err());
        assert!(r.resolve(".claude.json").is_err());
        // Extra credential material + key stores (name + substring).
        assert!(r.resolve(".netrc").is_err());
        assert!(r.resolve(".pgpass").is_err());
        assert!(r.resolve("keys/id_ecdsa").is_err());
        assert!(r.resolve("certs/client.p12").is_err());
        assert!(r.resolve("app.keystore").is_err());
        // Sensitivity holds anywhere in the path, and via resolve_read too.
        assert!(r.resolve("nested/dir/.zshrc").is_err());
        assert!(is_sensitive_path(".claude.json"));
        // Legitimately-named files remain allowed.
        assert!(r.resolve("src/config.rs").is_ok());
        assert!(r.resolve("docs/environment.md").is_ok());
    }

    #[test]
    fn read_only_access_allows_reads_but_denies_writes() {
        let r = WorkspaceRoot::new("/work/proj").with_access(FsAccess::ReadOnly);
        assert!(r.resolve_read("src/lib.rs").is_ok());
        assert!(r.resolve_write("src/lib.rs").is_err());
    }
}

#[cfg(all(test, windows))]
#[path = "fs_guard_windows_tests.rs"]
mod windows_tests;

#[cfg(all(test, windows))]
#[path = "fs_guard_namespace_tests.rs"]
mod namespace_tests;

#[cfg(all(test, unix))]
#[path = "fs_guard_unix_tests.rs"]
mod unix_tests;
