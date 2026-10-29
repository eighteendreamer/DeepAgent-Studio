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
use crate::{Tool, ToolDescriptor, ToolExecutionContext, ToolOutput};

/// Compile and validate a tool's model-facing parameter schema.
///
/// Provider function parameters are required to be a JSON Schema object.  A
/// JSON Schema can technically use a scalar/boolean root, but those shapes are
/// not valid for the function-tool wire contract used by this project.  Keep
/// the object-root check and compiler invocation in one place so registration,
/// invocation and provider-boundary validation cannot drift apart.
pub fn validate_tool_schema(name: &str, parameters: &serde_json::Value) -> Result<()> {
    compile_tool_schema(name, parameters).map(|_| ())
}

fn compile_tool_schema(
    name: &str,
    parameters: &serde_json::Value,
) -> Result<jsonschema::JSONSchema> {
    if !parameters.is_object() {
        return Err(CoreError::invalid(format!(
            "invalid schema for tool '{name}': root must be a JSON object"
        )));
    }

    jsonschema::JSONSchema::compile(parameters).map_err(|error| {
        // ValidationError's Display includes the rejected schema value. Keep
        // diagnostic locations/rules, not remote descriptions/defaults/secrets.
        let location = deepagent_core::redaction::scrub_secret_literals(
            &error.instance_path.to_string(),
        );
        let rule = deepagent_core::redaction::scrub_secret_literals(&error.schema_path.to_string());
        CoreError::invalid(format!(
            "invalid schema for tool '{name}': JSON Schema compilation failed at '{location}' (schema rule '{rule}')"
        ))
    })
}

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

/// A model-issued invocation that passed descriptor schema validation and the
/// tool's value-level validation/normalization hook.
#[derive(Debug, Clone, PartialEq)]
pub struct ValidatedInvocation {
    pub name: String,
    pub arguments: serde_json::Value,
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

    /// Register a tool. Returns an error if the name is already taken or the
    /// model-facing parameter schema is invalid.
    pub fn register(&mut self, tool: Arc<dyn Tool>) -> Result<()> {
        let descriptor = tool.descriptor();
        let name = descriptor.name.clone();
        if self.tools.contains_key(&name) {
            return Err(CoreError::invalid(format!(
                "tool '{name}' is already registered"
            )));
        }
        validate_tool_schema(&name, &descriptor.parameters)?;
        self.tools.insert(name, ToolSpec { tool, descriptor });
        Ok(())
    }

    /// Replace an existing implementation by descriptor name, or insert it.
    /// Used when a child run rebinds workspace-aware built-ins to an isolated
    /// worktree while retaining root-independent MCP/plugin tools.
    ///
    /// Invalid descriptors are rejected without replacing the previous entry.
    /// Call [`Self::try_replace`] when the caller needs the error returned.
    pub fn replace(&mut self, tool: Arc<dyn Tool>) {
        if let Err(error) = self.try_replace(tool) {
            tracing::warn!(error = %error, "rejecting invalid tool replacement");
        }
    }

    /// Fallible variant of [`Self::replace`] for callers that must surface a
    /// schema failure instead of logging and continuing.
    pub fn try_replace(&mut self, tool: Arc<dyn Tool>) -> Result<()> {
        let descriptor = tool.descriptor();
        let name = descriptor.name.clone();
        validate_tool_schema(&name, &descriptor.parameters)?;
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

    /// Validate a model invocation before hooks, permissions or execution.
    pub fn validate_invocation(
        &self,
        name: &str,
        arguments: serde_json::Value,
    ) -> Result<ValidatedInvocation> {
        let spec = self
            .tools
            .get(name)
            .ok_or_else(|| CoreError::invalid(format!("unknown tool: {name}")))?;
        let compiled = compile_tool_schema(name, &spec.descriptor.parameters)?;
        if let Err(errors) = compiled.validate(&arguments) {
            let detail = errors
                .take(8)
                .map(|error| {
                    let path = error.instance_path.to_string();
                    if path.is_empty() {
                        error.to_string()
                    } else {
                        format!("{path}: {error}")
                    }
                })
                .collect::<Vec<_>>()
                .join("; ");
            return Err(CoreError::invalid(format!(
                "tool '{name}' input validation failed: {detail}"
            )));
        }
        let arguments = spec.tool.validate_input(arguments)?;
        Ok(ValidatedInvocation {
            name: name.to_string(),
            arguments,
        })
    }

    /// Route and execute an invocation, enforcing permissions and approval.
    pub async fn invoke(
        &self,
        name: &str,
        arguments: serde_json::Value,
        granted: &PermissionSet,
        approval_granted: bool,
    ) -> Result<ToolOutput> {
        let validated = self.validate_invocation(name, arguments)?;
        let spec = self
            .check(name, granted, approval_granted)
            .map_err(|reason| CoreError::invalid(reason.to_string()))?;
        tracing::debug!(tool = name, risk = ?spec.descriptor.risk, "invoking tool");
        spec.tool.invoke(validated.arguments).await
    }

    /// Route and execute with controls owned by the current kernel run.
    pub async fn invoke_with_context(
        &self,
        name: &str,
        arguments: serde_json::Value,
        granted: &PermissionSet,
        approval_granted: bool,
        context: ToolExecutionContext,
    ) -> Result<ToolOutput> {
        let validated = self.validate_invocation(name, arguments)?;
        let spec = self
            .check(name, granted, approval_granted)
            .map_err(|reason| CoreError::invalid(reason.to_string()))?;
        tracing::debug!(tool = name, risk = ?spec.descriptor.risk, "invoking tool");
        spec.tool
            .invoke_with_context(validated.arguments, context)
            .await
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

    struct DescriptorTool {
        name: String,
        parameters: serde_json::Value,
    }

    #[async_trait]
    impl Tool for DescriptorTool {
        fn descriptor(&self) -> ToolDescriptor {
            ToolDescriptor {
                name: self.name.clone(),
                description: "descriptor fixture".into(),
                parameters: self.parameters.clone(),
                risk: RiskLevel::Safe,
                required_permissions: PermissionSet::empty(),
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

    #[test]
    fn registration_rejects_invalid_parameter_schema_roots_and_compilation() {
        let invalid = [
            serde_json::Value::Null,
            serde_json::json!("not-a-schema"),
            serde_json::json!([]),
            serde_json::json!(true),
            serde_json::json!(false),
            serde_json::json!({"type": "not-a-json-schema-type"}),
        ];

        for (index, parameters) in invalid.into_iter().enumerate() {
            let mut registry = ToolRegistry::new();
            let error = registry
                .register(Arc::new(DescriptorTool {
                    name: format!("invalid_{index}"),
                    parameters,
                }))
                .expect_err("invalid schema must be rejected before publication");
            assert!(error.to_string().contains("invalid schema"));
            assert!(registry.is_empty());
        }
    }

    #[test]
    fn registration_accepts_complex_object_schema() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "query": {"type": "string", "minLength": 1},
                "limit": {"type": "integer", "minimum": 1, "maximum": 20}
            },
            "required": ["query"],
            "additionalProperties": false
        });
        let mut registry = ToolRegistry::new();
        registry
            .register(Arc::new(DescriptorTool {
                name: "complex".into(),
                parameters: schema,
            }))
            .unwrap();
        assert!(registry.get("complex").is_some());
        assert!(registry
            .validate_invocation("complex", serde_json::json!({"query":"ok"}))
            .is_ok());
        assert!(registry
            .validate_invocation("complex", serde_json::json!({"query":3}))
            .is_err());
    }

    #[test]
    fn schema_compiler_error_omits_rejected_schema_values() {
        let error = validate_tool_schema(
            "bad",
            &serde_json::json!({
                "type":"secret-schema-fixture-value", "description":"private-description-fixture"
            }),
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("JSON Schema compilation failed"));
        assert!(!error.contains("secret-schema-fixture-value"));
        assert!(!error.contains("private-description-fixture"));
    }

    #[test]
    fn invalid_replacement_keeps_previous_valid_tool() {
        let mut registry = ToolRegistry::new();
        registry
            .register(Arc::new(DescriptorTool {
                name: "rebind".into(),
                parameters: serde_json::json!({"type": "object"}),
            }))
            .unwrap();

        let error = registry
            .try_replace(Arc::new(DescriptorTool {
                name: "rebind".into(),
                parameters: serde_json::Value::Null,
            }))
            .expect_err("invalid replacement must fail closed");
        assert!(error.to_string().contains("invalid schema"));
        assert_eq!(
            registry.get("rebind").unwrap().descriptor.parameters,
            serde_json::json!({"type": "object"})
        );

        registry.replace(Arc::new(DescriptorTool {
            name: "rebind".into(),
            parameters: serde_json::Value::Null,
        }));
        assert_eq!(
            registry.get("rebind").unwrap().descriptor.parameters,
            serde_json::json!({"type": "object"})
        );
        registry.replace(Arc::new(DescriptorTool {
            name: "new_invalid".into(),
            parameters: serde_json::json!([]),
        }));
        assert!(registry.get("new_invalid").is_none());
        let replacement = serde_json::json!({"type":"object", "required":["new"]});
        registry
            .try_replace(Arc::new(DescriptorTool {
                name: "rebind".into(),
                parameters: replacement.clone(),
            }))
            .unwrap();
        assert_eq!(
            registry.get("rebind").unwrap().descriptor.parameters,
            replacement
        );
    }

    #[test]
    fn schema_validation_rejects_before_invoke() {
        struct StrictTool;
        #[async_trait]
        impl Tool for StrictTool {
            fn descriptor(&self) -> ToolDescriptor {
                ToolDescriptor {
                    name: "strict".into(),
                    description: "strict".into(),
                    parameters: serde_json::json!({
                        "type": "object",
                        "required": ["path"],
                        "properties": { "path": { "type": "string" } },
                        "additionalProperties": false
                    }),
                    risk: RiskLevel::Safe,
                    required_permissions: PermissionSet::read_only(),
                }
            }
            async fn invoke(&self, arguments: serde_json::Value) -> Result<ToolOutput> {
                Ok(ToolOutput::success(arguments))
            }
        }
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(StrictTool)).unwrap();
        assert!(registry
            .validate_invocation("strict", serde_json::json!({"path": 1}))
            .is_err());
        assert!(registry
            .validate_invocation("strict", serde_json::json!({"path": "a"}))
            .is_ok());
    }
}
