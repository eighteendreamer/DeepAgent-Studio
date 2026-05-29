//! Permission & risk model (开发提示词.md §16).
//!
//! The runtime is zero-trust toward tools: a tool can only run if the calling
//! agent has been granted *all* of the tool's required [`Permission`]s, and
//! high-[`RiskLevel`] operations additionally require explicit approval.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

/// A capability scope that may be granted to an agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Permission {
    /// Read files within the workspace.
    ReadOnly,
    /// Write files within the workspace.
    WorkspaceWrite,
    /// Run shell commands deemed safe (allow-listed).
    ShellSafe,
    /// Run arbitrary / dangerous shell commands.
    ShellDangerous,
    /// Make outbound network requests.
    Network,
    /// Push to a git remote.
    GitPush,
    /// Read secrets / credentials.
    Secrets,
}

/// How dangerous a tool invocation is. Drives whether human approval is needed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RiskLevel {
    /// Safe, read-only or trivially reversible.
    Safe,
    /// Mutates local state but reversible (e.g. writing a file).
    Low,
    /// Notable side effects (e.g. installing deps).
    Medium,
    /// Hard-to-reverse / broad blast radius (e.g. rm -rf, git push, secrets).
    High,
}

impl RiskLevel {
    /// Whether an operation at this risk level requires explicit human approval
    /// before executing.
    pub const fn requires_approval(&self) -> bool {
        matches!(self, RiskLevel::High)
    }
}

/// A set of granted (or required) permissions.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PermissionSet(BTreeSet<Permission>);

impl PermissionSet {
    /// An empty set (no permissions).
    pub fn empty() -> Self {
        Self(BTreeSet::new())
    }

    /// Build from an iterator of permissions.
    pub fn from_iter_perms(perms: impl IntoIterator<Item = Permission>) -> Self {
        Self(perms.into_iter().collect())
    }

    /// A read-only agent.
    pub fn read_only() -> Self {
        Self::from_iter_perms([Permission::ReadOnly])
    }

    /// A typical developer agent: read + write + safe shell + network.
    pub fn developer() -> Self {
        Self::from_iter_perms([
            Permission::ReadOnly,
            Permission::WorkspaceWrite,
            Permission::ShellSafe,
            Permission::Network,
        ])
    }

    /// Add a permission (builder style).
    pub fn with(mut self, p: Permission) -> Self {
        self.0.insert(p);
        self
    }

    /// Whether this set contains `p`.
    pub fn contains(&self, p: Permission) -> bool {
        self.0.contains(&p)
    }

    /// Whether `self` is a superset of `required` (i.e. grants everything
    /// required).
    pub fn grants_all(&self, required: &PermissionSet) -> bool {
        required.0.is_subset(&self.0)
    }

    /// The permissions in `required` that are missing from `self`.
    pub fn missing(&self, required: &PermissionSet) -> Vec<Permission> {
        required.0.difference(&self.0).copied().collect()
    }

    /// Number of permissions in the set.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether the set is empty.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn high_risk_requires_approval() {
        assert!(RiskLevel::High.requires_approval());
        assert!(!RiskLevel::Low.requires_approval());
    }

    #[test]
    fn grants_all_checks_subset() {
        let granted = PermissionSet::developer();
        let required = PermissionSet::from_iter_perms([Permission::ReadOnly, Permission::Network]);
        assert!(granted.grants_all(&required));

        let needs_secrets = PermissionSet::from_iter_perms([Permission::Secrets]);
        assert!(!granted.grants_all(&needs_secrets));
    }

    #[test]
    fn missing_reports_gaps() {
        let granted = PermissionSet::read_only();
        let required =
            PermissionSet::from_iter_perms([Permission::ReadOnly, Permission::WorkspaceWrite]);
        let missing = granted.missing(&required);
        assert_eq!(missing, vec![Permission::WorkspaceWrite]);
    }

    #[test]
    fn builder_adds_permissions() {
        let set = PermissionSet::read_only().with(Permission::GitPush);
        assert!(set.contains(Permission::GitPush));
        assert_eq!(set.len(), 2);
    }
}
