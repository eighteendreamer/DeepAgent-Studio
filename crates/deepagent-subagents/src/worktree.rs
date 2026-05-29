//! Git worktree isolation (开发计划.md Phase 6 §3).
//!
//! Each sub-agent must run in its own workspace so parallel agents do not
//! overwrite each other's code ("Worktree | 0 覆盖"). This module abstracts
//! worktree provisioning behind [`WorktreeProvider`] so the real `git worktree
//! add` implementation lives behind the optional process-running path while
//! tests use an in-memory provider.

use async_trait::async_trait;
use std::collections::BTreeSet;
use std::sync::Mutex;

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
}
