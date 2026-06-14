//! The Capability Registry & Tool Router (开发提示词.md §15; 开发计划.md Phase 4).
//!
//! > 不能：模型直接知道所有工具。必须：Capability Registry。
//!
//! The [`ToolRegistry`] holds registered [`Tool`]s and mediates every
//! invocation:
//! 1. **Routing** — resolve a name to a tool.
//! 2. **Capability filtering** — only expose / allow tools the calling agent's
//!    [`PermissionSet`] grants.
//! 3. **Approval gating** — block [`RiskLevel::High`] tools unless approval is
//!    signalled.

use std::collections::BTreeMap;
use std::sync::Arc;

use deepagent_core::error::{CoreError, Result};

use crate::permission::PermissionSet;
use crate::{Tool, ToolDescriptor, ToolOutput};

/// A registered tool plus its cached descriptor.
#[derive(Clone)]
pub struct ToolSpec {
    /// The tool implementation.
    pub tool: Arc<dyn Tool>,
    /// Its descriptor (cached so listing does not re-run `descriptor()`).
    pub descriptor: ToolDescriptor,
}

/// Reasons an invocation may be denied before it ever runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DenyReason {
    /// No tool with that name is registered.
    UnknownTool(String),
    /// The caller lacks one or more required permissions.
    MissingPermissions(Vec<crate::permission::Permission>),
    /// The tool is high-risk and approval was not granted.
    ApprovalRequired,
}

impl std::fmt::Display for DenyReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DenyReason::UnknownTool(n) => write!(f, "unknown tool: {n}"),
            DenyReason::MissingPermissions(p) => {
                write!(f, "missing permissions: {p:?}")
            }
            DenyReason::ApprovalRequired => {
                write!(f, "high-risk tool requires explicit approval")
            }
        }
    }
}

/// The capability registry / router.
#[derive(Default, Clone)]
pub struct ToolRegistry {
    tools: BTreeMap<String, ToolSpec>,
}

impl ToolRegistry {
    /// New empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a tool. Returns an error if the name is already taken.
    pub fn register(&mut self, tool: Arc<dyn Tool>) -> Result<()> {
        let descriptor = tool.descriptor();
        let name = descriptor.name.clone();
        if self.tools.contains_key(&name) {
            return Err(CoreError::invalid(format!(
                "tool '{name}' is already registered"
            )));
        }
        self.tools.insert(name, ToolSpec { tool, descriptor });
        Ok(())
    }

    /// Number of registered tools.
    pub fn len(&self) -> usize {
        self.tools.len()
    }

    /// Whether the registry is empty.
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    /// Look up a tool spec by name.
    pub fn get(&self, name: &str) -> Option<&ToolSpec> {
        self.tools.get(name)
    }

    /// Iterate over every registered [`ToolSpec`]. Used by the lazy-tool-
    /// loading layer (`tool_search`) to discover deferred tools by reading
    /// their descriptor + invoking trait methods like `should_defer`.
    pub fn iter_specs(&self) -> impl Iterator<Item = &ToolSpec> {
        self.tools.values()
    }

    /// List descriptors of all tools the given permission set can access. This
    /// is the *Tool Router* dynamic-filtering behaviour: the model is only told
    /// about tools the current agent is allowed to use.
    pub fn visible_to(&self, granted: &PermissionSet) -> Vec<ToolDescriptor> {
        self.tools
            .values()
            .filter(|spec| granted.grants_all(&spec.descriptor.required_permissions))
            .map(|spec| spec.descriptor.clone())
            .collect()
    }

    /// Check whether an invocation would be permitted, without running it.
    pub fn check(
        &self,
        name: &str,
        granted: &PermissionSet,
        approval_granted: bool,
    ) -> std::result::Result<&ToolSpec, DenyReason> {
        let spec = self
            .tools
            .get(name)
            .ok_or_else(|| DenyReason::UnknownTool(name.to_string()))?;

        let missing = granted.missing(&spec.descriptor.required_permissions);
        if !missing.is_empty() {
            return Err(DenyReason::MissingPermissions(missing));
        }

        if spec.descriptor.risk.requires_approval() && !approval_granted {
            return Err(DenyReason::ApprovalRequired);
        }

        Ok(spec)
    }

    /// Route and execute an invocation, enforcing permissions and approval.
    pub async fn invoke(
        &self,
        name: &str,
        arguments: serde_json::Value,
        granted: &PermissionSet,
        approval_granted: bool,
    ) -> Result<ToolOutput> {
        let spec = self
            .check(name, granted, approval_granted)
            .map_err(|reason| CoreError::invalid(reason.to_string()))?;
        tracing::debug!(tool = name, risk = ?spec.descriptor.risk, "invoking tool");
        spec.tool.invoke(arguments).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::permission::{Permission, PermissionSet, RiskLevel};
    use async_trait::async_trait;

    struct EchoTool {
        risk: RiskLevel,
        perms: PermissionSet,
    }

    #[async_trait]
    impl Tool for EchoTool {
        fn descriptor(&self) -> ToolDescriptor {
            ToolDescriptor {
                name: "echo".into(),
                description: "echoes input".into(),
                parameters: serde_json::json!({"type": "object"}),
                risk: self.risk,
                required_permissions: self.perms.clone(),
            }
        }
        async fn invoke(&self, arguments: serde_json::Value) -> Result<ToolOutput> {
            Ok(ToolOutput::success(arguments))
        }
    }

    fn registry_with(risk: RiskLevel, perms: PermissionSet) -> ToolRegistry {
        let mut r = ToolRegistry::new();
        r.register(Arc::new(EchoTool { risk, perms })).unwrap();
        r
    }

    #[tokio::test]
    async fn invoke_succeeds_with_permissions() {
        let r = registry_with(RiskLevel::Safe, PermissionSet::read_only());
        let out = r
            .invoke(
                "echo",
                serde_json::json!({"msg": "hi"}),
                &PermissionSet::read_only(),
                false,
            )
            .await
            .unwrap();
        assert!(out.ok);
        assert_eq!(out.value["msg"], "hi");
    }

    #[tokio::test]
    async fn missing_permission_is_denied() {
        let r = registry_with(
            RiskLevel::Low,
            PermissionSet::from_iter_perms([Permission::WorkspaceWrite]),
        );
        let res = r.check("echo", &PermissionSet::read_only(), false);
        assert!(matches!(res, Err(DenyReason::MissingPermissions(_))));
    }

    #[tokio::test]
    async fn high_risk_requires_approval() {
        let r = registry_with(RiskLevel::High, PermissionSet::read_only());
        let denied = r.check("echo", &PermissionSet::read_only(), false);
        assert_eq!(denied.err(), Some(DenyReason::ApprovalRequired));
        // With approval it passes.
        assert!(r.check("echo", &PermissionSet::read_only(), true).is_ok());
    }

    #[test]
    fn visible_to_filters_by_permission() {
        let r = registry_with(
            RiskLevel::Safe,
            PermissionSet::from_iter_perms([Permission::Secrets]),
        );
        // read-only agent can't see the secrets-requiring tool.
        assert_eq!(r.visible_to(&PermissionSet::read_only()).len(), 0);
        // an agent with Secrets can.
        let privileged = PermissionSet::from_iter_perms([Permission::Secrets]);
        assert_eq!(r.visible_to(&privileged).len(), 1);
    }

    #[test]
    fn duplicate_registration_fails() {
        let mut r = registry_with(RiskLevel::Safe, PermissionSet::empty());
        let err = r.register(Arc::new(EchoTool {
            risk: RiskLevel::Safe,
            perms: PermissionSet::empty(),
        }));
        assert!(err.is_err());
    }
}
