//! Git worktree isolation (开发计划.md Phase 6 §3).
//!
//! Each sub-agent must run in its own workspace so parallel agents do not
//! overwrite each other's code ("Worktree | 0 覆盖"). This module abstracts
//! worktree provisioning behind [`WorktreeProvider`] so the real `git worktree
//! add` implementation lives behind the optional process-running path while
//! tests use an in-memory provider.

use async_trait::async_trait;
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use deepagent_core::error::{CoreError, Result};

/// A handle to an isolated worktree assigned to a sub-agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Worktree {
    /// Branch / worktree name.
    pub name: String,
    /// Filesystem path of the worktree.
    pub path: String,
}

/// Provisions and tears down isolated worktrees.
#[async_trait]
pub trait WorktreeProvider: Send + Sync {
    /// Create an isolated worktree named `name`. Must fail if a worktree with
    /// that name already exists (prevents two agents sharing one).
    async fn create(&self, name: &str) -> Result<Worktree>;

    /// Remove a previously created worktree.
    async fn remove(&self, name: &str) -> Result<()>;
}

/// Real Git-backed detached worktrees for isolated child-agent execution.
#[derive(Debug, Clone)]
pub struct GitWorktrees {
    repo_root: PathBuf,
    base: PathBuf,
    timeout: Duration,
}

impl GitWorktrees {
    pub fn new(repo_root: impl Into<PathBuf>, base: impl Into<PathBuf>) -> Self {
        Self {
            repo_root: repo_root.into(),
            base: base.into(),
            timeout: Duration::from_secs(30),
        }
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    fn path_for(&self, name: &str) -> Result<PathBuf> {
        validate_worktree_name(name)?;
        Ok(self.base.join(name))
    }

    async fn git(&self, args: &[&std::ffi::OsStr]) -> Result<String> {
        let mut command = tokio::process::Command::new("git");
        command
            .arg("-C")
            .arg(&self.repo_root)
            .args(args)
            .kill_on_drop(true)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        let output = tokio::time::timeout(self.timeout, command.output())
            .await
            .map_err(|_| CoreError::other("git worktree command timed out"))?
            .map_err(|error| CoreError::other(format!("failed to start git: {error}")))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            return Err(CoreError::other(format!(
                "git worktree command failed ({}): {}",
                output.status,
                if stderr.is_empty() {
                    "no error output"
                } else {
                    &stderr
                }
            )));
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }
}

#[async_trait]
impl WorktreeProvider for GitWorktrees {
    async fn create(&self, name: &str) -> Result<Worktree> {
        let path = self.path_for(name)?;
        if path.exists() {
            return Err(CoreError::invalid(format!(
                "worktree path already exists: {}",
                path.display()
            )));
        }
        std::fs::create_dir_all(&self.base).map_err(|error| {
            CoreError::other(format!("failed to create worktree base directory: {error}"))
        })?;
        let args = [
            std::ffi::OsStr::new("worktree"),
            std::ffi::OsStr::new("add"),
            std::ffi::OsStr::new("--detach"),
            path.as_os_str(),
            std::ffi::OsStr::new("HEAD"),
        ];
        self.git(&args).await?;
        if !path.is_dir() {
            return Err(CoreError::other(format!(
                "git reported success but worktree is missing: {}",
                path.display()
            )));
        }
        Ok(Worktree {
            name: name.to_string(),
            path: path.to_string_lossy().to_string(),
        })
    }

    async fn remove(&self, name: &str) -> Result<()> {
        let path = self.path_for(name)?;
        let args = [
            std::ffi::OsStr::new("worktree"),
            std::ffi::OsStr::new("remove"),
            std::ffi::OsStr::new("--force"),
            path.as_os_str(),
        ];
        self.git(&args).await?;
        let prune = [
            std::ffi::OsStr::new("worktree"),
            std::ffi::OsStr::new("prune"),
        ];
        self.git(&prune).await?;
        if path.exists() {
            return Err(CoreError::other(format!(
                "worktree still exists after removal: {}",
                path.display()
            )));
        }
        Ok(())
    }
}

fn validate_worktree_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name.len() > 96
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(CoreError::invalid(
            "worktree name must contain only ASCII letters, digits, '-' or '_'",
        ));
    }
    Ok(())
}

/// An in-memory worktree provider for tests / dry runs. Tracks names and
/// fabricates paths; performs no filesystem or git operations.
#[derive(Debug, Default)]
pub struct InMemoryWorktrees {
    base: String,
    active: Mutex<BTreeSet<String>>,
}

impl InMemoryWorktrees {
    /// Build with a base directory used to fabricate worktree paths.
    pub fn new(base: impl Into<String>) -> Self {
        Self {
            base: base.into(),
            active: Mutex::new(BTreeSet::new()),
        }
    }

    /// Currently active worktree names.
    pub fn active(&self) -> Vec<String> {
        self.active
            .lock()
            .expect("worktrees poisoned")
            .iter()
            .cloned()
            .collect()
    }
}

#[async_trait]
impl WorktreeProvider for InMemoryWorktrees {
    async fn create(&self, name: &str) -> Result<Worktree> {
        let mut active = self.active.lock().expect("worktrees poisoned");
        if active.contains(name) {
            return Err(CoreError::invalid(format!(
                "worktree '{name}' already exists"
            )));
        }
        active.insert(name.to_string());
        Ok(Worktree {
            name: name.to_string(),
            path: format!("{}/{}", self.base.trim_end_matches('/'), name),
        })
    }

    async fn remove(&self, name: &str) -> Result<()> {
        let mut active = self.active.lock().expect("worktrees poisoned");
        if !active.remove(name) {
            return Err(CoreError::not_found(format!("worktree '{name}'")));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[tokio::test]
    async fn creates_and_removes_worktree() {
        let provider = InMemoryWorktrees::new("/tmp/wt");
        let wt = provider.create("agent-backend").await.unwrap();
        assert_eq!(wt.path, "/tmp/wt/agent-backend");
        assert_eq!(provider.active(), vec!["agent-backend".to_string()]);
        provider.remove("agent-backend").await.unwrap();
        assert!(provider.active().is_empty());
    }

    #[tokio::test]
    async fn rejects_duplicate_worktree() {
        let provider = InMemoryWorktrees::new("/tmp/wt");
        provider.create("dup").await.unwrap();
        assert!(provider.create("dup").await.is_err());
    }

    #[tokio::test]
    async fn remove_unknown_errors() {
        let provider = InMemoryWorktrees::new("/tmp/wt");
        assert!(provider.remove("ghost").await.is_err());
    }

    #[tokio::test]
    async fn git_provider_rejects_unsafe_names_before_running_git() {
        let provider = GitWorktrees::new(".", ".deepagent/worktrees");
        assert!(provider.create("../escape").await.is_err());
        assert!(provider.remove("nested/path").await.is_err());
    }

    #[tokio::test]
    async fn git_provider_creates_and_removes_real_detached_worktree() {
        if std::process::Command::new("git")
            .arg("--version")
            .output()
            .is_err()
        {
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        run_git(&repo, &["init"]);
        run_git(
            &repo,
            &["config", "user.email", "deepagent@example.invalid"],
        );
        run_git(&repo, &["config", "user.name", "DeepAgent Test"]);
        std::fs::write(repo.join("README.md"), "fixture").unwrap();
        run_git(&repo, &["add", "README.md"]);
        run_git(&repo, &["commit", "-m", "fixture"]);

        let provider = GitWorktrees::new(&repo, temp.path().join("worktrees"));
        let worktree = provider.create("child_1").await.unwrap();
        assert!(Path::new(&worktree.path).join("README.md").is_file());
        provider.remove("child_1").await.unwrap();
        assert!(!Path::new(&worktree.path).exists());
    }

    fn run_git(repo: &Path, args: &[&str]) {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
