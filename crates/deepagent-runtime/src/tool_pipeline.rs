//! Canonical tool execution pipeline shared by the query loop and tests.

use std::collections::HashSet;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use deepagent_core::error::Result;
use deepagent_core::id::SessionId;
use deepagent_hooks::{HookContext, HookData, HookOutcome, HookPoint, HookRegistry};
use deepagent_persistence::artifact_store::{ToolArtifactRecord, ToolArtifactStore};
use deepagent_persistence::run_store::RunStore;
use deepagent_persistence::Database;
use deepagent_tools::permission::PermissionSet;
use deepagent_tools::{ToolExecutionContext, ToolInvocation, ToolOutput, ToolRegistry};

use crate::approval::{ApprovalDecision, ApprovalGate, ApprovalRequest, AutoDenyGate};
use crate::checkpoint::{CheckpointManager, MutationKind};
use crate::empty_stub::ensure_non_empty_output;
use crate::events::{tool_ui_metadata, NullEventSink, RuntimeEvent, RuntimeEventSink};
use crate::tool_budget::{apply_tool_result_budget, saved_path, ToolResultBudgetConfig};
use crate::tool_result_decorator::ToolResultDecorator;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolPipelineStage {
    Validation,
    PreToolUse,
    Permission,
    Execution,
}

#[derive(Debug, Clone)]
pub struct PreparedToolInvocation {
    pub call_id: String,
    pub name: String,
    pub arguments: serde_json::Value,
}

#[derive(Debug, Clone)]
pub struct ToolPipelineResult {
    pub call_id: String,
    pub name: String,
    pub arguments: serde_json::Value,
    pub output: ToolOutput,
    pub duration_ms: u64,
    pub stage: ToolPipelineStage,
}

pub enum ToolPreparation {
    Ready(PreparedToolInvocation),
    Blocked(ToolPipelineResult),
}

#[derive(Clone)]
pub struct ToolArtifactPersistence {
    db: Arc<Database>,
    run_id: String,
}

impl std::fmt::Debug for ToolArtifactPersistence {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ToolArtifactPersistence")
            .field("run_id", &self.run_id)
            .finish()
    }
}

impl ToolArtifactPersistence {
    pub fn new(db: Arc<Database>, run_id: impl Into<String>) -> Self {
        Self {
            db,
            run_id: run_id.into(),
        }
    }

    fn record(&self, call_id: &str, path: &std::path::Path) -> Result<()> {
        // Legacy RuntimeEngine runs do not own a v2 `runs` row. Keep the
        // compatibility path functional without creating orphan metadata.
        if RunStore::new(&self.db).get(&self.run_id)?.is_none() {
            return Ok(());
        }
        let byte_size = std::fs::metadata(path)
            .map(|metadata| metadata.len() as i64)
            .unwrap_or(0);
        ToolArtifactStore::new(&self.db).put(&ToolArtifactRecord {
            id: format!("artifact_{}", deepagent_core::id::EventId::new()),
            run_id: self.run_id.clone(),
            call_id: call_id.to_string(),
            path: path.to_string_lossy().to_string(),
            media_type: Some("application/json".to_string()),
            byte_size,
            digest: None,
            created_at: now_millis(),
        })
    }
}

/// Implements the invariant ordering used by Claude-compatible tool calls:
/// schema/value validation -> PreToolUse -> permission/approval -> execute ->
/// output budget -> PostToolUse/PostToolUseFailure.
pub struct ToolExecutionPipeline<'a> {
    registry: &'a ToolRegistry,
    session_id: SessionId,
    permissions: PermissionSet,
    auto_approve: bool,
    hooks: Option<&'a HookRegistry>,
    approvals: Arc<dyn ApprovalGate>,
    events: Arc<dyn RuntimeEventSink>,
    cancel: Arc<AtomicBool>,
    timeout: Duration,
    budget: ToolResultBudgetConfig,
    decorator: Option<Arc<dyn ToolResultDecorator>>,
    created_paths: Arc<Mutex<Vec<std::path::PathBuf>>>,
    checkpoint: Option<Arc<CheckpointManager>>,
    artifact_persistence: Option<ToolArtifactPersistence>,
}

impl<'a> ToolExecutionPipeline<'a> {
    pub fn new(
        registry: &'a ToolRegistry,
        session_id: SessionId,
        permissions: PermissionSet,
    ) -> Self {
        Self {
            registry,
            session_id,
            permissions,
            auto_approve: false,
            hooks: None,
            approvals: Arc::new(AutoDenyGate),
            events: Arc::new(NullEventSink),
            cancel: Arc::new(AtomicBool::new(false)),
            timeout: Duration::from_secs(120),
            budget: ToolResultBudgetConfig::default(),
            decorator: None,
            created_paths: Arc::new(Mutex::new(Vec::new())),
            checkpoint: None,
            artifact_persistence: None,
        }
    }

    pub fn with_hooks(mut self, hooks: Option<&'a HookRegistry>) -> Self {
        self.hooks = hooks;
        self
    }

    pub fn with_approvals(mut self, approvals: Arc<dyn ApprovalGate>) -> Self {
        self.approvals = approvals;
        self
    }

    pub fn with_events(mut self, events: Arc<dyn RuntimeEventSink>) -> Self {
        self.events = events;
        self
    }

    pub fn with_auto_approve(mut self, auto_approve: bool) -> Self {
        self.auto_approve = auto_approve;
        self
    }

    pub fn with_execution_controls(mut self, cancel: Arc<AtomicBool>, timeout: Duration) -> Self {
        self.cancel = cancel;
        self.timeout = timeout;
        self
    }

    pub fn with_result_policy(
        mut self,
        budget: ToolResultBudgetConfig,
        decorator: Option<Arc<dyn ToolResultDecorator>>,
        created_paths: Arc<Mutex<Vec<std::path::PathBuf>>>,
    ) -> Self {
        self.budget = budget;
        self.decorator = decorator;
        self.created_paths = created_paths;
        self
    }

    pub fn with_checkpoint(mut self, checkpoint: Option<Arc<CheckpointManager>>) -> Self {
        self.checkpoint = checkpoint;
        self
    }

    pub fn with_artifact_persistence(
        mut self,
        persistence: Option<ToolArtifactPersistence>,
    ) -> Self {
        self.artifact_persistence = persistence;
        self
    }

    async fn fire(&self, point: HookPoint, data: HookData) -> Result<HookOutcome> {
        match self.hooks {
            Some(hooks) => {
                hooks
                    .dispatch(&HookContext::new(self.session_id, point, data))
                    .await
            }
            None => Ok(HookOutcome::Continue),
        }
    }

    pub async fn prepare(
        &self,
        mut invocation: ToolInvocation,
        before_override: Option<HookOutcome>,
    ) -> Result<ToolPreparation> {
        let call_id = invocation
            .id
            .clone()
            .unwrap_or_else(|| format!("call_{}", deepagent_core::id::EventId::new()));
        let name = invocation.name.clone();

        // Sentinel from the stream decoder: the provider emitted argument
        // bytes that were not valid JSON. Reject deterministically BEFORE
        // schema validation so no hook, permission check, or process ever
        // sees fabricated arguments — the model receives a paired failure.
        if invocation
            .arguments
            .get("__invalid_tool_arguments__")
            .is_some()
        {
            let parse_error = invocation
                .arguments
                .get("parse_error")
                .and_then(|value| value.as_str())
                .unwrap_or("invalid JSON");
            return Ok(ToolPreparation::Blocked(self.blocked(
                call_id,
                name,
                invocation.arguments.clone(),
                format!("model produced invalid tool argument JSON ({parse_error}); the call was not executed — retry with valid JSON arguments"),
                "input_validation_error",
                ToolPipelineStage::Validation,
                false,
            )));
        }

        let validated = match self
            .registry
            .validate_invocation(&name, invocation.arguments.clone())
        {
            Ok(value) => value,
            Err(error) => {
                return Ok(ToolPreparation::Blocked(self.blocked(
                    call_id,
                    name,
                    invocation.arguments,
                    error.to_string(),
                    "input_validation_error",
                    ToolPipelineStage::Validation,
                    false,
                )));
            }
        };
        invocation.arguments = validated.arguments;

        let before = match before_override {
            Some(outcome) => outcome,
            None => {
                self.fire(
                    HookPoint::BeforeToolUse,
                    HookData::before_tool(name.clone(), invocation.arguments.clone()),
                )
                .await?
            }
        };
        let mut approval_granted = false;
        match before {
            HookOutcome::Continue => {}
            HookOutcome::Modify { updated_input, .. } => {
                match self.registry.validate_invocation(&name, updated_input) {
                    Ok(value) => invocation.arguments = value.arguments,
                    Err(error) => {
                        return Ok(ToolPreparation::Blocked(self.blocked(
                            call_id,
                            name,
                            invocation.arguments,
                            format!("hook produced invalid tool input: {error}"),
                            "hook_validation_error",
                            ToolPipelineStage::PreToolUse,
                            false,
                        )));
                    }
                }
            }
            HookOutcome::Deny { reason, .. } => {
                return Ok(ToolPreparation::Blocked(self.blocked(
                    call_id,
                    name,
                    invocation.arguments,
                    format!("blocked by hook: {reason}"),
                    "hook_denied",
                    ToolPipelineStage::PreToolUse,
                    false,
                )));
            }
            HookOutcome::Ask { reason, .. } => {
                if let Some(blocked) = self
                    .request_approval(&call_id, &name, &invocation.arguments, reason, "hook")
                    .await?
                {
                    return Ok(ToolPreparation::Blocked(blocked));
                }
                approval_granted = true;
            }
        }

        // Permission capabilities are checked independently from approval.
        let spec = match self.registry.check(&name, &self.permissions, true) {
            Ok(spec) => spec,
            Err(error) => {
                return Ok(ToolPreparation::Blocked(self.blocked(
                    call_id,
                    name,
                    invocation.arguments,
                    error.to_string(),
                    "permission_denied",
                    ToolPipelineStage::Permission,
                    false,
                )));
            }
        };
        if spec.descriptor.risk.requires_approval() && !approval_granted {
            let reason = format!("high-risk tool '{}' requires approval", name);
            if let Some(blocked) = self
                .request_approval(&call_id, &name, &invocation.arguments, reason, "high")
                .await?
            {
                return Ok(ToolPreparation::Blocked(blocked));
            }
        }

        Ok(ToolPreparation::Ready(PreparedToolInvocation {
            call_id,
            name,
            arguments: invocation.arguments,
        }))
    }

    async fn request_approval(
        &self,
        call_id: &str,
        name: &str,
        arguments: &serde_json::Value,
        reason: String,
        risk: &str,
    ) -> Result<Option<ToolPipelineResult>> {
        let permission_hook = self
            .fire(
                HookPoint::PermissionRequest,
                HookData::Permission {
                    tool: name.to_string(),
                    arguments: arguments.clone(),
                    reason: reason.clone(),
                },
            )
            .await?;
        if let HookOutcome::Deny {
            reason: hook_reason,
            ..
        } = permission_hook
        {
            return Ok(Some(self.blocked(
                call_id.to_string(),
                name.to_string(),
                arguments.clone(),
                hook_reason,
                "permission_hook_denied",
                ToolPipelineStage::Permission,
                false,
            )));
        }
        if self.auto_approve {
            return Ok(None);
        }
        self.events.emit(RuntimeEvent::ToolBlocked {
            name: name.to_string(),
            reason: reason.clone(),
            needs_approval: true,
        });
        let decision = self
            .approvals
            .request(ApprovalRequest {
                call_id: call_id.to_string(),
                tool: name.to_string(),
                reason: reason.clone(),
                risk: risk.to_string(),
                arguments: arguments.clone(),
            })
            .await;
        Ok(match decision {
            ApprovalDecision::Allow => None,
            ApprovalDecision::Deny => Some(self.blocked(
                call_id.to_string(),
                name.to_string(),
                arguments.clone(),
                format!("approval denied: {reason}"),
                "approval_denied",
                ToolPipelineStage::Permission,
                false,
            )),
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn blocked(
        &self,
        call_id: String,
        name: String,
        arguments: serde_json::Value,
        reason: String,
        error_type: &str,
        stage: ToolPipelineStage,
        needs_approval: bool,
    ) -> ToolPipelineResult {
        self.events.emit(RuntimeEvent::ToolBlocked {
            name: name.clone(),
            reason: reason.clone(),
            needs_approval,
        });
        ToolPipelineResult {
            call_id,
            name,
            arguments,
            output: ToolOutput::failure(reason).with_error_type(error_type),
            duration_ms: 0,
            stage,
        }
    }

    pub async fn execute_prepared(
        &self,
        prepared: PreparedToolInvocation,
    ) -> Result<ToolPipelineResult> {
        let mut mutation_targets = Vec::new();
        if let Some(checkpoint) = self.checkpoint.as_ref() {
            for path in mutation_paths(&prepared.name, &prepared.arguments) {
                mutation_targets.push(checkpoint.normalize_target(&path)?);
                checkpoint.capture_before(path)?;
            }
        }
        let metadata = tool_ui_metadata(&prepared.name, &prepared.arguments, None);
        self.events.emit(RuntimeEvent::ToolStarted {
            name: prepared.name.clone(),
            call_id: prepared.call_id.clone(),
            arguments: prepared.arguments.clone(),
            tool_kind: metadata.tool_kind,
            file_path: metadata.file_path,
            summary: metadata.summary,
            meta: metadata.meta,
        });
        let started = Instant::now();
        let result = self
            .registry
            .invoke_with_context(
                &prepared.name,
                prepared.arguments.clone(),
                &self.permissions,
                true,
                ToolExecutionContext::new(self.cancel.clone()).with_timeout(self.timeout),
            )
            .await;
        let duration_ms = started.elapsed().as_millis() as u64;
        self.complete_prepared_inner(prepared, result, duration_ms, false, mutation_targets)
            .await
    }

    /// Commit an attempt-local read-only execution after the provider stream
    /// succeeds. The invocation has already passed `prepare`; this method makes
    /// the previously private result visible through the normal budget,
    /// decorator, event and PostToolUse stages.
    pub async fn complete_prepared(
        &self,
        prepared: PreparedToolInvocation,
        result: Result<ToolOutput>,
        duration_ms: u64,
    ) -> Result<ToolPipelineResult> {
        let speculative_safe = self.registry.get(&prepared.name).is_some_and(|spec| {
            spec.descriptor.risk == deepagent_tools::permission::RiskLevel::Safe
                && spec.tool.is_concurrency_safe(&prepared.arguments)
        });
        if !speculative_safe {
            return Err(deepagent_core::error::CoreError::invalid(format!(
                "tool '{}' is not eligible for speculative completion",
                prepared.name
            )));
        }
        self.complete_prepared_inner(prepared, result, duration_ms, true, Vec::new())
            .await
    }

    async fn complete_prepared_inner(
        &self,
        prepared: PreparedToolInvocation,
        result: Result<ToolOutput>,
        duration_ms: u64,
        emit_started: bool,
        mutation_targets: Vec<std::path::PathBuf>,
    ) -> Result<ToolPipelineResult> {
        if emit_started {
            let metadata = tool_ui_metadata(&prepared.name, &prepared.arguments, None);
            self.events.emit(RuntimeEvent::ToolStarted {
                name: prepared.name.clone(),
                call_id: prepared.call_id.clone(),
                arguments: prepared.arguments.clone(),
                tool_kind: metadata.tool_kind,
                file_path: metadata.file_path,
                summary: metadata.summary,
                meta: metadata.meta,
            });
        }
        let mut output = match result {
            Ok(output) => output,
            Err(error) => ToolOutput::failure(error.to_string()),
        };
        output = apply_tool_result_budget(
            &self.budget,
            &self.session_id.to_string(),
            &prepared.name,
            &prepared.call_id,
            output,
        )
        .await;
        ensure_non_empty_output(&mut output, &prepared.name);
        if let Some(decorator) = self.decorator.as_ref() {
            decorator.decorate(&prepared.name, &mut output).await;
        }
        if let Some(path) = saved_path(&output) {
            self.created_paths
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .push(path);
            if let Some(persistence) = self.artifact_persistence.as_ref() {
                if let Some(path) = saved_path(&output) {
                    persistence.record(&prepared.call_id, &path)?;
                }
            }
        }

        let metadata = tool_ui_metadata(&prepared.name, &prepared.arguments, Some(&output.value));
        self.events.emit(RuntimeEvent::ToolCompleted {
            name: prepared.name.clone(),
            call_id: prepared.call_id.clone(),
            ok: output.ok,
            output: output.value.clone(),
            duration_ms,
            tool_kind: metadata.tool_kind,
            file_path: metadata.file_path,
            summary: metadata.summary,
            meta: metadata.meta,
        });
        self.fire(
            if output.ok {
                HookPoint::AfterToolUse
            } else {
                HookPoint::PostToolUseFailure
            },
            HookData::after_tool(prepared.name.clone(), prepared.arguments.clone(), output.ok),
        )
        .await?;
        if output.ok {
            self.fire_file_changed_hooks(&mutation_targets).await?;
        }

        Ok(ToolPipelineResult {
            call_id: prepared.call_id,
            name: prepared.name,
            arguments: prepared.arguments,
            output,
            duration_ms,
            stage: ToolPipelineStage::Execution,
        })
    }

    async fn fire_file_changed_hooks(&self, mutation_targets: &[std::path::PathBuf]) -> Result<()> {
        let Some(checkpoint) = self.checkpoint.as_ref() else {
            return Ok(());
        };
        if mutation_targets.is_empty() {
            return Ok(());
        }
        let target_set = mutation_targets.iter().cloned().collect::<HashSet<_>>();
        for evidence in checkpoint.mutation_evidence()? {
            if target_set.contains(&evidence.path) && evidence.kind != MutationKind::Unchanged {
                self.fire(
                    HookPoint::FileChanged,
                    HookData::FileChange {
                        path: evidence.path.to_string_lossy().to_string(),
                        kind: mutation_kind_label(evidence.kind).to_string(),
                    },
                )
                .await?;
            }
        }
        Ok(())
    }

    pub async fn execute(&self, invocation: ToolInvocation) -> Result<ToolPipelineResult> {
        match self.prepare(invocation, None).await? {
            ToolPreparation::Ready(prepared) => self.execute_prepared(prepared).await,
            ToolPreparation::Blocked(result) => Ok(result),
        }
    }
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

fn mutation_paths(name: &str, arguments: &serde_json::Value) -> Vec<std::path::PathBuf> {
    match name {
        "write_file" | "edit_file" | "multi_edit" | "delete_path" => arguments
            .get("path")
            .and_then(|value| value.as_str())
            .map(std::path::PathBuf::from)
            .into_iter()
            .collect(),
        "move_path" => ["source", "destination"]
            .into_iter()
            .filter_map(|key| arguments.get(key).and_then(serde_json::Value::as_str))
            .map(std::path::PathBuf::from)
            .collect(),
        _ => Vec::new(),
    }
}

fn mutation_kind_label(kind: MutationKind) -> &'static str {
    match kind {
        MutationKind::Created => "created",
        MutationKind::Modified => "modified",
        MutationKind::Deleted => "deleted",
        MutationKind::Unchanged => "unchanged",
    }
}

trait ToolOutputErrorType {
    fn with_error_type(self, error_type: &str) -> Self;
}

impl ToolOutputErrorType for ToolOutput {
    fn with_error_type(mut self, error_type: &str) -> Self {
        if let Some(object) = self.value.as_object_mut() {
            object.insert("error_type".to_string(), serde_json::json!(error_type));
        }
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use deepagent_hooks::{Hook, HookContext};
    use deepagent_tools::{PermissionSet, RiskLevel, Tool, ToolDescriptor};
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct CountingTool {
        calls: Arc<AtomicUsize>,
        risk: RiskLevel,
    }

    struct WriteFileProbeTool;

    #[async_trait]
    impl Tool for CountingTool {
        fn descriptor(&self) -> ToolDescriptor {
            ToolDescriptor {
                name: "counting".into(),
                description: "test".into(),
                parameters: serde_json::json!({
                    "type": "object",
                    "required": ["value"],
                    "properties": { "value": { "type": "string" } },
                    "additionalProperties": false
                }),
                risk: self.risk,
                required_permissions: PermissionSet::read_only(),
            }
        }

        async fn invoke(&self, arguments: serde_json::Value) -> Result<ToolOutput> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(ToolOutput::success(arguments))
        }
    }

    #[async_trait]
    impl Tool for WriteFileProbeTool {
        fn descriptor(&self) -> ToolDescriptor {
            ToolDescriptor {
                name: "write_file".into(),
                description: "writes a file for pipeline tests".into(),
                parameters: serde_json::json!({
                    "type": "object",
                    "required": ["path", "content"],
                    "properties": {
                        "path": { "type": "string" },
                        "content": { "type": "string" }
                    },
                    "additionalProperties": false
                }),
                risk: RiskLevel::Medium,
                required_permissions: PermissionSet::developer(),
            }
        }

        async fn invoke(&self, arguments: serde_json::Value) -> Result<ToolOutput> {
            let path = arguments["path"].as_str().unwrap();
            let content = arguments["content"].as_str().unwrap();
            if let Some(parent) = std::path::Path::new(path).parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(path, content).unwrap();
            Ok(ToolOutput::success(serde_json::json!({ "path": path })))
        }
    }

    struct CountingHook(Arc<AtomicUsize>);

    struct FileChangeHook(Arc<Mutex<Vec<(String, String)>>>);

    #[async_trait]
    impl Hook for CountingHook {
        fn name(&self) -> &str {
            "counting-hook"
        }

        async fn run(&self, _ctx: &HookContext) -> Result<HookOutcome> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(HookOutcome::Continue)
        }
    }

    #[async_trait]
    impl Hook for FileChangeHook {
        fn name(&self) -> &str {
            "file-change-hook"
        }

        async fn run(&self, ctx: &HookContext) -> Result<HookOutcome> {
            if let HookData::FileChange { path, kind } = &ctx.data {
                self.0
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .push((path.clone(), kind.clone()));
            }
            Ok(HookOutcome::Continue)
        }
    }

    struct CountingApproval(Arc<AtomicUsize>);

    #[async_trait]
    impl ApprovalGate for CountingApproval {
        async fn request(&self, _request: ApprovalRequest) -> ApprovalDecision {
            self.0.fetch_add(1, Ordering::SeqCst);
            ApprovalDecision::Allow
        }
    }

    fn registry(risk: RiskLevel, calls: Arc<AtomicUsize>) -> ToolRegistry {
        let mut registry = ToolRegistry::new();
        registry
            .register(Arc::new(CountingTool { calls, risk }))
            .unwrap();
        registry
    }

    #[tokio::test]
    async fn invalid_schema_never_runs_hooks_permissions_or_tool() {
        let tool_calls = Arc::new(AtomicUsize::new(0));
        let hook_calls = Arc::new(AtomicUsize::new(0));
        let approval_calls = Arc::new(AtomicUsize::new(0));
        let registry = registry(RiskLevel::High, tool_calls.clone());
        let mut hooks = HookRegistry::new();
        hooks.register(
            HookPoint::BeforeToolUse,
            Arc::new(CountingHook(hook_calls.clone())),
        );
        let pipeline =
            ToolExecutionPipeline::new(&registry, SessionId::new(), PermissionSet::read_only())
                .with_hooks(Some(&hooks))
                .with_approvals(Arc::new(CountingApproval(approval_calls.clone())));

        let result = pipeline
            .execute(ToolInvocation::new(
                "counting",
                serde_json::json!({"value": 42}),
            ))
            .await
            .unwrap();

        assert!(!result.output.ok);
        assert_eq!(result.stage, ToolPipelineStage::Validation);
        assert_eq!(hook_calls.load(Ordering::SeqCst), 0);
        assert_eq!(approval_calls.load(Ordering::SeqCst), 0);
        assert_eq!(tool_calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn high_risk_tool_is_approved_once_then_executes() {
        let tool_calls = Arc::new(AtomicUsize::new(0));
        let approval_calls = Arc::new(AtomicUsize::new(0));
        let registry = registry(RiskLevel::High, tool_calls.clone());
        let pipeline =
            ToolExecutionPipeline::new(&registry, SessionId::new(), PermissionSet::read_only())
                .with_approvals(Arc::new(CountingApproval(approval_calls.clone())));

        let result = pipeline
            .execute(ToolInvocation::new(
                "counting",
                serde_json::json!({"value": "ok"}),
            ))
            .await
            .unwrap();

        assert!(result.output.ok);
        assert_eq!(approval_calls.load(Ordering::SeqCst), 1);
        assert_eq!(tool_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn speculative_completion_rejects_non_safe_tools() {
        let registry = registry(RiskLevel::High, Arc::new(AtomicUsize::new(0)));
        let pipeline =
            ToolExecutionPipeline::new(&registry, SessionId::new(), PermissionSet::read_only());
        let error = pipeline
            .complete_prepared(
                PreparedToolInvocation {
                    call_id: "call-high".into(),
                    name: "counting".into(),
                    arguments: serde_json::json!({"value": "not-allowed"}),
                },
                Ok(ToolOutput::success(serde_json::json!({"value": "ignored"}))),
                1,
            )
            .await
            .unwrap_err();

        assert!(error
            .to_string()
            .contains("not eligible for speculative completion"));
    }

    #[tokio::test]
    async fn successful_file_mutation_fires_file_changed_hook() {
        let db = Arc::new(deepagent_persistence::Database::open_in_memory().unwrap());
        let workspace = tempfile::tempdir().unwrap();
        let checkpoint_root = tempfile::tempdir().unwrap();
        let target = workspace.path().join("created.txt");
        let checkpoint = Arc::new(
            CheckpointManager::new(
                db,
                "run-file-hook",
                0,
                workspace.path(),
                checkpoint_root.path(),
            )
            .unwrap(),
        );
        let changes = Arc::new(Mutex::new(Vec::new()));
        let mut hooks = HookRegistry::new();
        hooks.register(
            HookPoint::FileChanged,
            Arc::new(FileChangeHook(changes.clone())),
        );
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(WriteFileProbeTool)).unwrap();
        let pipeline =
            ToolExecutionPipeline::new(&registry, SessionId::new(), PermissionSet::developer())
                .with_hooks(Some(&hooks))
                .with_checkpoint(Some(checkpoint))
                .with_auto_approve(true);

        let result = pipeline
            .execute(ToolInvocation::new(
                "write_file",
                serde_json::json!({
                    "path": target.to_string_lossy().to_string(),
                    "content": "hello"
                }),
            ))
            .await
            .unwrap();

        assert!(result.output.ok);
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "hello");
        let recorded = changes.lock().unwrap().clone();
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].0, target.to_string_lossy());
        assert_eq!(recorded[0].1, "created");
    }
}
