use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use deepagent_core::error::Result;
use deepagent_persistence::Database;
use deepagent_runtime::{CheckpointManager, RuntimeConfig, ToolResultBudgetConfig};

use crate::settings::VerificationPolicy;

pub(crate) struct KernelRuntimeConfigRequest<'a> {
    pub(crate) db: Arc<Database>,
    pub(crate) run_id: &'a str,
    pub(crate) session_sequence: i64,
    pub(crate) root: &'a Path,
    pub(crate) tool_results_dir: &'a Path,
    pub(crate) plan: deepagent_builtins::PlanMode,
    pub(crate) todo_store: deepagent_builtins::TodoStore,
    pub(crate) verification_policy: VerificationPolicy,
    pub(crate) fire_session_start: bool,
    pub(crate) granted: deepagent_tools::PermissionSet,
    /// Nested-instruction discovery (kernel-refactor Phase C): injects
    /// not-yet-loaded `CLAUDE.md`/`AGENTS.md` when tools touch their
    /// directories. `None` for callers without a hook registry (tests).
    pub(crate) nested_instructions:
        Option<Arc<crate::nested_instructions::NestedInstructionsDecorator>>,
}

pub(crate) struct KernelRuntimeConfig {
    pub(crate) config: RuntimeConfig,
    pub(crate) checkpoint: Arc<CheckpointManager>,
}

pub(crate) fn build_kernel_runtime_config(
    request: KernelRuntimeConfigRequest<'_>,
) -> Result<KernelRuntimeConfig> {
    let checkpoint = Arc::new(CheckpointManager::new(
        request.db.clone(),
        request.run_id.to_string(),
        request.session_sequence,
        request.root,
        request.tool_results_dir.join("checkpoints"),
    )?);

    let mut per_tool_max_tokens = BTreeMap::new();
    per_tool_max_tokens.insert(deepagent_builtins::SKILL_TOOL_NAME.to_string(), 24_000);

    let config = RuntimeConfig {
        fire_session_start: request.fire_session_start,
        permissions: request.granted,
        // Completion is no longer gated on prompt-derived filesystem
        // requirements (intent-guessing anti-pattern removed 2026-07-28).
        // Upstream parity: model self-reports completion, verified by the
        // fact-based build/type plan + hooks. An empty policy never blocks.
        completion_policy: deepagent_runtime::CompletionPolicy::default(),
        checkpoint: Some(checkpoint.clone()),
        artifact_persistence: Some(
            deepagent_runtime::tool_pipeline::ToolArtifactPersistence::new(
                request.db.clone(),
                request.run_id.to_string(),
            ),
        ),
        action_persistence: Some(
            deepagent_runtime::tool_pipeline::ToolActionPersistence::new(
                request.db.clone(),
                request.run_id.to_string(),
                request.run_id.to_string(),
            )?,
        ),
        tool_result_budget: ToolResultBudgetConfig {
            output_dir: PathBuf::from(request.tool_results_dir),
            per_tool_max_tokens,
            ..Default::default()
        },
        tool_result_decorator: Some(Arc::new({
            let mut chain = deepagent_runtime::ChainDecorator::new()
                .push(Arc::new(
                    crate::plan_mode_reminder::PlanModeReminderDecorator::new(request.plan),
                ))
                .push(Arc::new(
                    crate::todo_snapshot_reminder::TodoSnapshotReminderDecorator::new(
                        request.todo_store,
                    ),
                ))
                .push(Arc::new(
                    crate::verification_decorator::VerificationDecorator::with_policy(
                        Arc::new(
                            crate::verification_dispatcher::VerificationDispatcher::standard(),
                        ),
                        Some(request.root.to_path_buf()),
                        request.verification_policy,
                    ),
                ));
            if let Some(nested) = request.nested_instructions {
                chain = chain.push(nested);
            }
            chain
        })),
        ..Default::default()
    };

    Ok(KernelRuntimeConfig { config, checkpoint })
}
