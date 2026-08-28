//! Canonical tool execution pipeline shared by the query loop and tests.

use std::collections::HashSet;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use deepagent_core::error::Result;
use deepagent_core::id::SessionId;
use deepagent_hooks::{HookContext, HookData, HookOutcome, HookPoint, HookRegistry};
use deepagent_persistence::artifact_store::{ToolArtifactRecord, ToolArtifactStore};
use deepagent_persistence::run_control::{
    NewRunAction, NewRunApproval, RunActionState, RunControlStore,
};
use deepagent_persistence::run_store::RunStore;
use deepagent_persistence::Database;
use deepagent_tools::permission::PermissionSet;
use deepagent_tools::{ToolExecutionContext, ToolInvocation, ToolOutput, ToolRegistry};
use sha2::{Digest, Sha256};

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

/// Durable action projection attached to one kernel run. Registration happens
/// before any parallel execution so sequence follows model call order.
#[derive(Clone)]
pub struct ToolActionPersistence {
    db: Arc<Database>,
    run_id: String,
    turn_id: String,
    next_sequence: Arc<std::sync::atomic::AtomicU64>,
}

impl std::fmt::Debug for ToolActionPersistence {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ToolActionPersistence")
            .field("run_id", &self.run_id)
            .field("turn_id", &self.turn_id)
            .finish()
    }
}

impl ToolActionPersistence {
    pub fn new(
        db: Arc<Database>,
        run_id: impl Into<String>,
        turn_id: impl Into<String>,
    ) -> Result<Self> {
        let run_id = run_id.into();
        let next = RunControlStore::new(&db).next_action_sequence(&run_id)?;
        Ok(Self {
            db,
            run_id,
            turn_id: turn_id.into(),
            next_sequence: Arc::new(std::sync::atomic::AtomicU64::new(next)),
        })
    }

    pub(crate) fn register_invocations(
        &self,
        invocations: &mut [ToolInvocation],
        registry: &ToolRegistry,
    ) -> Result<()> {
        for invocation in invocations {
            let call_id = invocation
                .id
                .get_or_insert_with(|| format!("call_{}", deepagent_core::id::EventId::new()))
                .clone();
            let arguments = serde_json::to_vec(&invocation.arguments)?;
            let arguments_hash = format!("sha256:{:x}", Sha256::digest(arguments));
            let controls = RunControlStore::new(&self.db);
            if let Some(existing) = controls.get_action(&self.run_id, &call_id)? {
                if existing.turn_id == self.turn_id
                    && existing.tool_name == invocation.name
                    && existing.arguments_hash == arguments_hash
                {
                    continue;
                }
                return Err(deepagent_core::error::CoreError::invalid(format!(
                    "idempotency conflict for action {}/{}",
                    self.run_id, call_id
                )));
            }
            // Allocate only after the idempotency lookup. Provider retries may
            // repeat an already-recorded call id and must not create gaps in
            // the model-order sequence for later calls.
            let sequence = self
                .next_sequence
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let risk = registry
                .get(&invocation.name)
                .map(|spec| format!("{:?}", spec.descriptor.risk).to_ascii_lowercase())
                .unwrap_or_else(|| "unknown".to_string());
            controls.create_action(&NewRunAction {
                run_id: &self.run_id,
                turn_id: &self.turn_id,
                call_id: &call_id,
                sequence,
                tool_name: &invocation.name,
                arguments_hash: &arguments_hash,
                risk: &risk,
                parent_action_id: None,
                now: now_millis(),
            })?;
        }
        Ok(())
    }

    pub(crate) fn transition(
        &self,
        call_id: &str,
        state: RunActionState,
        blocked_reason: Option<&str>,
        result_ref: Option<&str>,
    ) -> Result<()> {
        RunControlStore::new(&self.db).transition_action(
            &self.run_id,
            call_id,
            state,
            now_millis(),
            blocked_reason,
            result_ref,
        )?;
        Ok(())
    }

    pub(crate) fn fail(&self, call_id: &str, reason: &str) -> Result<()> {
        let store = RunControlStore::new(&self.db);
        if store
            .get_action(&self.run_id, call_id)?
            .is_some_and(|action| action.state.is_terminal())
        {
            return Ok(());
        }
        self.transition(call_id, RunActionState::Failed, Some(reason), None)
    }

    pub(crate) fn cancel(&self, call_id: &str, reason: &str) -> Result<()> {
        let store = RunControlStore::new(&self.db);
        if store
            .get_action(&self.run_id, call_id)?
            .is_some_and(|action| action.state.is_terminal())
        {
            return Ok(());
        }
        self.transition(call_id, RunActionState::Cancelled, Some(reason), None)
    }

    pub(crate) fn request_approval(&self, call_id: &str, risk: &str, reason: &str) -> Result<()> {
        RunControlStore::new(&self.db).request_approval(&NewRunApproval {
            approval_id: call_id,
            run_id: &self.run_id,
            call_id,
            scope: "single_call",
            risk,
            reason: Some(reason),
            policy_snapshot: None,
            expires_at: None,
            now: now_millis(),
        })?;
        Ok(())
    }

    pub(crate) fn approve(&self, call_id: &str) -> Result<()> {
        RunControlStore::new(&self.db).respond_approval(
            call_id,
            true,
            "approval_gate",
            now_millis(),
        )?;
        Ok(())
    }

    pub(crate) fn deny(&self, call_id: &str) -> Result<()> {
        RunControlStore::new(&self.db).respond_approval(
            call_id,
            false,
            "approval_gate",
            now_millis(),
        )?;
        Ok(())
    }
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
    action_persistence: Option<ToolActionPersistence>,
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
            action_persistence: None,
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

    pub fn with_action_persistence(mut self, persistence: Option<ToolActionPersistence>) -> Self {
        self.action_persistence = persistence;
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
            )?));
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
                )?));
            }
        };
        invocation.arguments = validated.arguments;
        if let Some(persistence) = self.action_persistence.as_ref() {
            persistence.transition(&call_id, RunActionState::Prepared, None, None)?;
        }

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
                        )?));
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
                )?));
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
                )?));
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
            )?));
        }
        if self.auto_approve {
            return Ok(None);
        }
        self.events.emit(RuntimeEvent::ToolBlocked {
            name: name.to_string(),
            reason: reason.clone(),
            needs_approval: true,
        });
        if let Some(persistence) = self.action_persistence.as_ref() {
            persistence.request_approval(call_id, risk, &reason)?;
        }
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
            ApprovalDecision::Allow => {
                if let Some(persistence) = self.action_persistence.as_ref() {
                    persistence.approve(call_id)?;
                }
                None
            }
            ApprovalDecision::Deny => {
                if let Some(persistence) = self.action_persistence.as_ref() {
                    persistence.deny(call_id)?;
                }
                Some(self.blocked(
                    call_id.to_string(),
                    name.to_string(),
                    arguments.clone(),
                    format!("approval denied: {reason}"),
                    "approval_denied",
                    ToolPipelineStage::Permission,
                    false,
                )?)
            }
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
    ) -> Result<ToolPipelineResult> {
        if let Some(persistence) = self.action_persistence.as_ref() {
            persistence.fail(&call_id, &reason)?;
        }
        self.events.emit(RuntimeEvent::ToolBlocked {
            name: name.clone(),
            reason: reason.clone(),
            needs_approval,
        });
        Ok(ToolPipelineResult {
            call_id,
            name,
            arguments,
            output: ToolOutput::failure(reason).with_error_type(error_type),
            duration_ms: 0,
            stage,
        })
    }

    pub async fn execute_prepared(
        &self,
        prepared: PreparedToolInvocation,
    ) -> Result<ToolPipelineResult> {
        if let Some(persistence) = self.action_persistence.as_ref() {
            persistence.transition(&prepared.call_id, RunActionState::Running, None, None)?;
        }
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
            if let Some(persistence) = self.action_persistence.as_ref() {
                persistence.transition(&prepared.call_id, RunActionState::Running, None, None)?;
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
        if let Some(persistence) = self.action_persistence.as_ref() {
            if output.ok {
                persistence.transition(&prepared.call_id, RunActionState::Completed, None, None)?;
            } else if self.cancel.load(std::sync::atomic::Ordering::Acquire) {
                persistence.cancel(&prepared.call_id, "tool execution observed cancellation")?;
            } else {
                persistence.fail(&prepared.call_id, "tool execution failed")?;
            }
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
    use deepagent_core::clock::Timestamp;
    use deepagent_hooks::{Hook, HookContext};
    use deepagent_persistence::event_store::EventStore;
    use deepagent_persistence::run_control::{ApprovalState, RunActionState, RunControlStore};
    use deepagent_persistence::run_store::RunStore;
    use deepagent_tools::{PermissionSet, RiskLevel, Tool, ToolDescriptor};
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct CountingTool {
        calls: Arc<AtomicUsize>,
        risk: RiskLevel,
    }

    struct WriteFileProbeTool;

    struct DelayedTool {
        completions: Arc<Mutex<Vec<String>>>,
    }

    struct CancellationAwareTool;

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

    #[async_trait]
    impl Tool for DelayedTool {
        fn descriptor(&self) -> ToolDescriptor {
            ToolDescriptor {
                name: "delayed".into(),
                description: "records completion order".into(),
                parameters: serde_json::json!({
                    "type": "object",
                    "required": ["label", "delay_ms"],
                    "properties": {
                        "label": { "type": "string" },
                        "delay_ms": { "type": "integer", "minimum": 0 }
                    },
                    "additionalProperties": false
                }),
                risk: RiskLevel::Safe,
                required_permissions: PermissionSet::read_only(),
            }
        }

        async fn invoke(&self, arguments: serde_json::Value) -> Result<ToolOutput> {
            let delay = arguments["delay_ms"].as_u64().unwrap();
            let label = arguments["label"].as_str().unwrap().to_string();
            tokio::time::sleep(Duration::from_millis(delay)).await;
            self.completions
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .push(label.clone());
            Ok(ToolOutput::success(serde_json::json!({ "label": label })))
        }
    }

    #[async_trait]
    impl Tool for CancellationAwareTool {
        fn descriptor(&self) -> ToolDescriptor {
            ToolDescriptor {
                name: "cancellation_aware".into(),
                description: "observes the execution cancellation flag".into(),
                parameters: serde_json::json!({ "type": "object" }),
                risk: RiskLevel::Safe,
                required_permissions: PermissionSet::read_only(),
            }
        }

        async fn invoke(&self, _arguments: serde_json::Value) -> Result<ToolOutput> {
            unreachable!("the context-aware implementation must be used")
        }

        async fn invoke_with_context(
            &self,
            _arguments: serde_json::Value,
            context: ToolExecutionContext,
        ) -> Result<ToolOutput> {
            assert!(context.is_cancelled());
            Ok(ToolOutput::failure("cancelled before side effect"))
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

    struct PersistingApproval {
        db: Arc<Database>,
        approved: bool,
    }

    #[async_trait]
    impl ApprovalGate for CountingApproval {
        async fn request(&self, _request: ApprovalRequest) -> ApprovalDecision {
            self.0.fetch_add(1, Ordering::SeqCst);
            ApprovalDecision::Allow
        }
    }

    #[async_trait]
    impl ApprovalGate for PersistingApproval {
        async fn request(&self, request: ApprovalRequest) -> ApprovalDecision {
            RunControlStore::new(&self.db)
                .respond_approval(
                    &request.call_id,
                    self.approved,
                    "test_external_client",
                    now_millis(),
                )
                .unwrap();
            if self.approved {
                ApprovalDecision::Allow
            } else {
                ApprovalDecision::Deny
            }
        }
    }

    fn registry(risk: RiskLevel, calls: Arc<AtomicUsize>) -> ToolRegistry {
        let mut registry = ToolRegistry::new();
        registry
            .register(Arc::new(CountingTool { calls, risk }))
            .unwrap();
        registry
    }

    fn action_fixture(registry: &ToolRegistry) -> (Arc<Database>, ToolActionPersistence) {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let session_id = SessionId::new();
        EventStore::new(&db)
            .create_session(session_id, None, Timestamp::from_millis(1))
            .unwrap();
        RunStore::new(&db)
            .create(
                "run-action-test",
                &session_id.to_string(),
                Some("turn-1"),
                1,
            )
            .unwrap();
        let persistence =
            ToolActionPersistence::new(db.clone(), "run-action-test", "turn-1").unwrap();
        assert!(
            registry.get("counting").is_some()
                || registry.get("delayed").is_some()
                || registry.get("cancellation_aware").is_some()
        );
        (db, persistence)
    }

    #[test]
    fn durable_registration_preserves_model_order_without_retry_gaps() {
        let registry = registry(RiskLevel::Safe, Arc::new(AtomicUsize::new(0)));
        let (db, persistence) = action_fixture(&registry);
        let mut invocations = (0..8)
            .map(|index| {
                ToolInvocation::new(
                    "counting",
                    serde_json::json!({ "value": index.to_string() }),
                )
                .with_id(format!("call-{index}"))
            })
            .collect::<Vec<_>>();
        persistence
            .register_invocations(&mut invocations, &registry)
            .unwrap();

        let mut provider_retry = vec![invocations[3].clone()];
        persistence
            .register_invocations(&mut provider_retry, &registry)
            .unwrap();
        let mut later =
            vec![
                ToolInvocation::new("counting", serde_json::json!({ "value": "later" }))
                    .with_id("call-8"),
            ];
        persistence
            .register_invocations(&mut later, &registry)
            .unwrap();

        let actions = RunControlStore::new(&db)
            .list_actions("run-action-test")
            .unwrap();
        assert_eq!(actions.len(), 9);
        assert_eq!(
            actions
                .iter()
                .map(|action| action.sequence)
                .collect::<Vec<_>>(),
            (0..9).collect::<Vec<_>>()
        );
        assert_eq!(actions[3].call_id, "call-3");
        assert_eq!(actions[8].call_id, "call-8");
    }

    #[tokio::test]
    async fn durable_parallel_completion_keeps_persisted_model_order() {
        let completions = Arc::new(Mutex::new(Vec::new()));
        let mut registry = ToolRegistry::new();
        registry
            .register(Arc::new(DelayedTool {
                completions: completions.clone(),
            }))
            .unwrap();
        let (db, persistence) = action_fixture(&registry);
        let mut invocations = vec![
            ToolInvocation::new(
                "delayed",
                serde_json::json!({ "label": "slow", "delay_ms": 40 }),
            )
            .with_id("call-slow"),
            ToolInvocation::new(
                "delayed",
                serde_json::json!({ "label": "fast", "delay_ms": 1 }),
            )
            .with_id("call-fast"),
        ];
        persistence
            .register_invocations(&mut invocations, &registry)
            .unwrap();
        let pipeline =
            ToolExecutionPipeline::new(&registry, SessionId::new(), PermissionSet::read_only())
                .with_action_persistence(Some(persistence));

        let (slow, fast) = tokio::join!(
            pipeline.execute(invocations[0].clone()),
            pipeline.execute(invocations[1].clone())
        );
        assert!(slow.unwrap().output.ok);
        assert!(fast.unwrap().output.ok);
        assert_eq!(
            completions.lock().unwrap().as_slice(),
            &["fast".to_string(), "slow".to_string()]
        );
        let actions = RunControlStore::new(&db)
            .list_actions("run-action-test")
            .unwrap();
        assert_eq!(actions[0].call_id, "call-slow");
        assert_eq!(actions[0].sequence, 0);
        assert_eq!(actions[1].call_id, "call-fast");
        assert_eq!(actions[1].sequence, 1);
        assert!(actions
            .iter()
            .all(|action| action.state == RunActionState::Completed));
    }

    #[tokio::test]
    async fn external_approval_and_runtime_confirmation_are_idempotent() {
        let calls = Arc::new(AtomicUsize::new(0));
        let registry = registry(RiskLevel::High, calls.clone());
        let (db, persistence) = action_fixture(&registry);
        let mut invocations =
            vec![
                ToolInvocation::new("counting", serde_json::json!({ "value": "approved" }))
                    .with_id("call-approved"),
            ];
        persistence
            .register_invocations(&mut invocations, &registry)
            .unwrap();
        let pipeline =
            ToolExecutionPipeline::new(&registry, SessionId::new(), PermissionSet::read_only())
                .with_approvals(Arc::new(PersistingApproval {
                    db: db.clone(),
                    approved: true,
                }))
                .with_action_persistence(Some(persistence));

        let result = pipeline.execute(invocations.remove(0)).await.unwrap();
        assert!(result.output.ok);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            RunControlStore::new(&db)
                .get_approval("call-approved")
                .unwrap()
                .unwrap()
                .state,
            ApprovalState::Approved
        );
        let action = RunControlStore::new(&db)
            .get_action("run-action-test", "call-approved")
            .unwrap()
            .unwrap();
        assert_eq!(action.state, RunActionState::Completed);
        assert_eq!(action.attempt, 1);
    }

    #[tokio::test]
    async fn denied_approval_is_terminal_and_never_executes_tool() {
        let calls = Arc::new(AtomicUsize::new(0));
        let registry = registry(RiskLevel::High, calls.clone());
        let (db, persistence) = action_fixture(&registry);
        let mut invocations =
            vec![
                ToolInvocation::new("counting", serde_json::json!({ "value": "denied" }))
                    .with_id("call-denied"),
            ];
        persistence
            .register_invocations(&mut invocations, &registry)
            .unwrap();
        let pipeline =
            ToolExecutionPipeline::new(&registry, SessionId::new(), PermissionSet::read_only())
                .with_approvals(Arc::new(PersistingApproval {
                    db: db.clone(),
                    approved: false,
                }))
                .with_action_persistence(Some(persistence));

        let result = pipeline.execute(invocations.remove(0)).await.unwrap();
        assert!(!result.output.ok);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert_eq!(
            RunControlStore::new(&db)
                .get_action("run-action-test", "call-denied")
                .unwrap()
                .unwrap()
                .state,
            RunActionState::Denied
        );
    }

    #[tokio::test]
    async fn validation_failure_and_cancelled_execution_have_distinct_terminal_states() {
        let registry = registry(RiskLevel::Safe, Arc::new(AtomicUsize::new(0)));
        let (db, persistence) = action_fixture(&registry);
        let mut invalid = vec![
            ToolInvocation::new("counting", serde_json::json!({ "value": 42 }))
                .with_id("call-invalid"),
        ];
        persistence
            .register_invocations(&mut invalid, &registry)
            .unwrap();
        let pipeline =
            ToolExecutionPipeline::new(&registry, SessionId::new(), PermissionSet::read_only())
                .with_action_persistence(Some(persistence));
        assert!(!pipeline.execute(invalid.remove(0)).await.unwrap().output.ok);
        assert_eq!(
            RunControlStore::new(&db)
                .get_action("run-action-test", "call-invalid")
                .unwrap()
                .unwrap()
                .state,
            RunActionState::Failed
        );

        let mut cancelled_registry = ToolRegistry::new();
        cancelled_registry
            .register(Arc::new(CancellationAwareTool))
            .unwrap();
        let (cancel_db, cancel_persistence) = action_fixture(&cancelled_registry);
        let mut cancelled = vec![
            ToolInvocation::new("cancellation_aware", serde_json::json!({}))
                .with_id("call-cancelled"),
        ];
        cancel_persistence
            .register_invocations(&mut cancelled, &cancelled_registry)
            .unwrap();
        let cancel = Arc::new(AtomicBool::new(true));
        let cancel_pipeline = ToolExecutionPipeline::new(
            &cancelled_registry,
            SessionId::new(),
            PermissionSet::read_only(),
        )
        .with_execution_controls(cancel, Duration::from_secs(1))
        .with_action_persistence(Some(cancel_persistence));
        assert!(
            !cancel_pipeline
                .execute(cancelled.remove(0))
                .await
                .unwrap()
                .output
                .ok
        );
        assert_eq!(
            RunControlStore::new(&cancel_db)
                .get_action("run-action-test", "call-cancelled")
                .unwrap()
                .unwrap()
                .state,
            RunActionState::Cancelled
        );
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
