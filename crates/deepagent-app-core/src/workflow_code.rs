//! Run-scoped code handles. This is a Kernel tool adapter, not another tool
//! registry, SDK finder, approval center or process runner.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use deepagent_core::error::{CoreError, Result};
use deepagent_runtime::workflow::code::{
    CodeBridge, CodeInvocation, CodeLanguage, CodeRequest, CODE_EXECUTE_TOOL,
};
use deepagent_tools::{
    Permission, PermissionSet, RiskLevel, Tool, ToolDescriptor, ToolExecutionContext, ToolOutput,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::runtime_service::{RuntimeBroker, RuntimeKind};
use crate::sandbox_backend::{SandboxBackend, SandboxCodeRequest};

struct PendingCode {
    invocation: CodeInvocation,
    request: CodeRequest,
}

// Same hard ceiling as the workflow's item lanes. Each validated request is
// at most 1 MiB; the run-local cache remains bounded even before dispatch.
const MAX_PENDING_CODE_REQUESTS: usize = 50;

pub(crate) struct WorkflowCodeGateway {
    pending: Mutex<BTreeMap<String, PendingCode>>,
    broker: Option<Arc<RuntimeBroker>>,
    backend: Option<Arc<dyn SandboxBackend>>,
}

impl WorkflowCodeGateway {
    pub(crate) fn new(
        broker: Option<Arc<RuntimeBroker>>,
        backend: Option<Arc<dyn SandboxBackend>>,
    ) -> Self {
        Self {
            pending: Mutex::new(BTreeMap::new()),
            broker,
            backend,
        }
    }

    pub(crate) fn tool(self: &Arc<Self>) -> WorkflowCodeTool {
        WorkflowCodeTool {
            gateway: self.clone(),
        }
    }

    fn parse(&self, arguments: Value) -> Result<CodeInvocation> {
        let invocation: CodeInvocation = serde_json::from_value(arguments)
            .map_err(|_| CoreError::invalid("CodeHandleInvalid: invalid code handle"))?;
        let pending = self
            .pending
            .lock()
            .map_err(|_| CoreError::other("code handle lock poisoned"))?;
        if pending
            .get(&invocation.request_id)
            .map(|code| &code.invocation)
            != Some(&invocation)
        {
            return Err(CoreError::invalid(
                "CodeHandleInvalid: unknown, consumed or mismatched code handle",
            ));
        }
        Ok(invocation)
    }

    async fn execute(&self, arguments: Value, context: ToolExecutionContext) -> Result<Value> {
        let invocation = self.parse(arguments)?;
        // Atomically consume before any external operation. No retry path may
        // dispatch the same approved handle twice, including on ambiguous exit.
        let pending = self
            .pending
            .lock()
            .map_err(|_| CoreError::other("code handle lock poisoned"))?
            .remove(&invocation.request_id)
            .ok_or_else(|| CoreError::invalid("CodeHandleInvalid: code handle was consumed"))?;
        if context.is_cancelled() {
            return Err(CoreError::other(
                "code execution cancelled before SDK resolution",
            ));
        }
        let broker = self.broker.as_ref().ok_or_else(|| {
            CoreError::invalid("CodeRuntimeUnavailable: no SDK broker was attached")
        })?;
        let backend = self.backend.as_ref().ok_or_else(|| {
            CoreError::invalid("CodeIsolationUnavailable: no compatible code backend was bound")
        })?;
        let kind = match pending.request.language {
            CodeLanguage::Javascript => RuntimeKind::Node,
            CodeLanguage::Python => RuntimeKind::Python,
        };
        let requirement = broker.service().managed_requirement(kind)?;
        let broker = broker.clone();
        // The existing Broker's version probe is blocking; do not stall the
        // async Kernel worker or move this work ahead of permission/approval.
        let runtime = tokio::task::spawn_blocking(move || broker.resolve(&requirement, None, None))
            .await
            .map_err(|_| CoreError::other("code SDK resolver task failed"))??;
        if context.is_cancelled() {
            return Err(CoreError::other(
                "code execution cancelled after SDK resolution",
            ));
        }
        let outputs = pending.request.outputs.clone();
        let expected_version = runtime.version.clone();
        let response = backend
            .execute_code(SandboxCodeRequest {
                code: pending.request,
                runtime,
                context,
            })
            .await?;
        if response.receipt.runtime_version != expected_version {
            return Err(CoreError::invalid(
                "CodeResultInvalid: executor SDK version does not match the resolved runtime",
            ));
        }
        response.validate(&invocation, &outputs)?;
        Ok(serde_json::to_value(response)?)
    }
}

impl CodeBridge for WorkflowCodeGateway {
    fn prepare(&self, request: CodeRequest) -> Result<CodeInvocation> {
        request.validate()?;
        let invocation = CodeInvocation {
            request_id: deepagent_core::id::EventId::new().to_string(),
            language: request.language,
            script_sha256: format!("{:x}", Sha256::digest(request.script.as_bytes())),
        };
        let mut pending = self
            .pending
            .lock()
            .map_err(|_| CoreError::other("code handle lock poisoned"))?;
        // Iteration items may prepare together; only the Kernel may consume
        // their independent handles. Preparation still does not resolve SDKs
        // or execute code, and the cache is neither global nor unbounded.
        if pending.len() >= MAX_PENDING_CODE_REQUESTS {
            return Err(CoreError::invalid(
                "CodeHandleInvalid: pending code request limit reached",
            ));
        }
        pending.insert(
            invocation.request_id.clone(),
            PendingCode {
                invocation: invocation.clone(),
                request,
            },
        );
        Ok(invocation)
    }

    fn release(&self, request_id: &str) {
        // Revocation/cleanup executes nothing. Recover only to drop private
        // data; prepare/dispatch still reject a poisoned lock.
        self.pending
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .remove(request_id);
    }
}

pub(crate) struct WorkflowCodeTool {
    gateway: Arc<WorkflowCodeGateway>,
}

#[async_trait]
impl Tool for WorkflowCodeTool {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: CODE_EXECUTE_TOOL.into(),
            description: "Execute one approved code-node handle with a managed SDK and a strict code sandbox. Source and inputs remain private to this run; no host fallback or automatic retry.".into(),
            parameters: json!({
                "type":"object", "additionalProperties":false,
                "properties": {
                    "request_id":{"type":"string","minLength":1,"maxLength":256},
                    "language":{"enum":["javascript","python"]},
                    "script_sha256":{"type":"string","minLength":64,"maxLength":64}
                },
                "required":["request_id","language","script_sha256"]
            }),
            risk: RiskLevel::High,
            required_permissions: PermissionSet::from_iter_perms([Permission::Sandbox]),
        }
    }

    fn validate_input(&self, arguments: Value) -> Result<Value> {
        Ok(serde_json::to_value(self.gateway.parse(arguments)?)?)
    }

    async fn invoke(&self, arguments: Value) -> Result<ToolOutput> {
        self.invoke_with_context(arguments, ToolExecutionContext::default())
            .await
    }

    async fn invoke_with_context(
        &self,
        arguments: Value,
        context: ToolExecutionContext,
    ) -> Result<ToolOutput> {
        self.gateway
            .execute(arguments, context)
            .await
            .map(ToolOutput::success)
    }
}

#[cfg(test)]
mod tests;
