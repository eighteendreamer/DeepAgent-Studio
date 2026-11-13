//! Sub-agent execution for the `task` tool: [`ChatSubagentRunner`] runs a
//! nested agent loop over a sub-registry (the built-ins minus `task`, so no
//! recursion) on an ephemeral in-memory session, returning only the final
//! message. Also hosts the runtime agent-definition discovery (`.deepagent/
//! agents` + plugin agent roots) that feeds the `task` tool's agent types.
//!
//! Split out of `chat_service.rs` (kernel-refactor Phase A): the runner is
//! constructed by [`ChatService::run_in_session`] with live run state and
//! registered via [`crate::tool_runtime::register_task_tool`].

use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use deepagent_core::clock::SystemClock;
use deepagent_core::error::{CoreError, Result};
use deepagent_hooks::{HookContext, HookData, HookOutcome, HookPoint, HookRegistry};
use deepagent_models::{ModelClient, ReasoningEffort, ResponseInputItem, ToolSchema};
use deepagent_persistence::Database;
use deepagent_runtime::{RuntimeEvent, RuntimeEventSink};
use deepagent_session::Session;
use deepagent_tools::{Permission, PermissionSet, ToolRegistry};
use tokio_util::sync::CancellationToken;

use crate::chat_service::ChatService;
use crate::system_context::SYSTEM_PROMPT_DYNAMIC_BOUNDARY;
use crate::tool_manifest::{
    effective_tool_search_mode, live_tool_schema_source, register_tool_search_into,
    DiscoveredToolSet,
};

#[cfg(test)]
#[path = "subagent_modalities_tests.rs"]
mod modalities_tests;

#[cfg(all(test, feature = "web"))]
#[path = "subagent_search_capability_tests.rs"]
mod search_capability_tests;

#[cfg(test)]
#[path = "subagent_deadline_tests.rs"]
mod deadline_tests;

#[cfg(test)]
#[path = "subagent_control_scope_tests.rs"]
mod control_scope_tests;

#[cfg(test)]
#[path = "subagent_resume_tests.rs"]
mod resume_tests;

/// Runs a sub-agent for the `task` tool: a nested agent loop over a sub-registry
/// (the built-ins minus `task`, so no recursion), on an ephemeral in-memory
/// session, returning only the sub-agent's final message.
#[derive(Clone)]
pub(crate) struct ChatSubagentRunner {
    pub(crate) db: Arc<Database>,
    pub(crate) parent_run_id: String,
    pub(crate) transcript_root: PathBuf,
    pub(crate) client: Arc<ModelClient>,
    pub(crate) usage_owner: Option<crate::call_usage::RunCallUsageOwner>,
    pub(crate) model: String,
    pub(crate) reasoning_effort: ReasoningEffort,
    pub(crate) registry: Arc<ToolRegistry>,
    /// Workspace used to construct `registry`; never changed by DAG assignment.
    pub(crate) root: PathBuf,
    /// Per-execution scheduler assignment, including runs without persistence.
    pub(crate) assigned_worktree: Option<PathBuf>,
    /// Tool-search mode at the time the parent session built this runner.
    /// Captured per-runner (not per-call) so swapping the user setting
    /// mid-run doesn't change behavior of an in-flight sub-agent.
    pub(crate) tool_search_mode: deepagent_builtins::ToolSearchMode,
    /// Auto-mode threshold inherited from the parent.
    pub(crate) tool_search_auto_threshold: usize,
    /// Snapshot of parent's discovered tool names at runner construction.
    /// Each `run()` call seeds a fresh `Arc<Mutex<HashSet>>` from this
    /// snapshot — sub-agent discoveries DON'T propagate back to the parent.
    pub(crate) parent_discovered_snapshot: std::collections::HashSet<String>,
    /// Local and plugin-provided sub-agent definitions keyed by `subagent_type`.
    pub(crate) agent_definitions: std::collections::BTreeMap<String, RuntimeAgentDefinition>,
    pub(crate) background: Arc<
        std::sync::Mutex<std::collections::HashMap<String, Arc<std::sync::atomic::AtomicBool>>>,
    >,
    pub(crate) events: std::sync::Weak<dyn RuntimeEventSink>,
    pub(crate) skills: Option<Arc<std::sync::Mutex<crate::skills_service::SkillsService>>>,
    pub(crate) host: ChatService,
    pub(crate) access: deepagent_builtins::FsAccess,
    pub(crate) local_execution_mode: crate::settings::LocalExecutionMode,
    pub(crate) bash_full_access: bool,
    pub(crate) hooks: Arc<std::sync::OnceLock<Arc<HookRegistry>>>,
    pub(crate) parent_checkpoint:
        Arc<std::sync::OnceLock<Arc<deepagent_runtime::CheckpointManager>>>,
    /// Parent-run approval gate, shared so a risky sub-agent tool bubbles to the
    /// same UI approval channel instead of auto-denying. Unset outside the
    /// desktop run path (tests / headless), where the engine default applies.
    pub(crate) parent_approvals: Arc<std::sync::OnceLock<Arc<dyn deepagent_runtime::ApprovalGate>>>,
}

/// Internal execution input: public task arguments remain unchanged. Restored
/// context is data, not a new prompt or a second execution path.
#[derive(Debug)]
struct SubagentInput {
    request: deepagent_builtins::SubagentRequest,
    history: Vec<ResponseInputItem>,
}

impl From<deepagent_builtins::SubagentRequest> for SubagentInput {
    fn from(request: deepagent_builtins::SubagentRequest) -> Self {
        Self {
            request,
            history: Vec::new(),
        }
    }
}

/// One owner for the existing child transcript, durable record and terminal
/// event. Normal completion and forced future destruction share this boundary;
/// Drop does not spawn detached asynchronous cleanup or run arbitrary hooks.
struct TrackedSubagent<'a> {
    runner: &'a ChatSubagentRunner,
    id: String,
    request: deepagent_builtins::SubagentRequest,
    created_at: i64,
    persist: bool,
    background: bool,
    armed: bool,
    surface: Option<crate::context_runtime::SessionSurfaceStore>,
    /// Last complete Surface also survives a drop during pre-session hooks.
    seed_history: Vec<ResponseInputItem>,
    // A field, not a later stack local: Drop must retain the origin exclusion
    // until its existing terminal/transcript finalizer has finished too.
    origin_activity: Option<deepagent_persistence::event_store::SessionActivityGuard>,
}

impl TrackedSubagent<'_> {
    fn finish(&mut self, state: &str, summary: &str) -> Result<()> {
        self.armed = false;
        let subagent_id = &self.id;
        let created_at = self.created_at;
        let persist = self.persist;
        let background = self.background;
        let mut bounded_summary = subagent_summary(summary, 2_000);
        let mut terminal_state = state;
        let mut persistence_result = Ok(());
        if persist {
            let transcript_result = self.persist_transcript(state, summary);
            if transcript_result.is_err() {
                if state != "cancelled" {
                    terminal_state = "failed";
                }
                bounded_summary =
                    "sub-agent context could not be persisted; continuous resume is unavailable"
                        .into();
            }
            // Still attempt the durable terminal even when the transcript
            // filesystem is unavailable; return the failure instead of
            // claiming that the whole lifecycle was persisted successfully.
            let record_result =
                deepagent_persistence::subagent_store::SubagentRunStore::new(&self.runner.db)
                    .finish(
                        subagent_id,
                        terminal_state,
                        Some(&bounded_summary),
                        subagent_now_ms(),
                    );
            persistence_result = transcript_result.and(record_result);
        }
        let duration_ms = subagent_now_ms().saturating_sub(created_at) as u64;
        if terminal_state == "cancelled" {
            self.runner
                .emit_subagent_event(RuntimeEvent::SubagentCancelled {
                    id: subagent_id.clone(),
                    parent_run_id: self.runner.parent_run_id.clone(),
                    duration_ms,
                    background,
                });
        } else {
            self.runner
                .emit_subagent_event(RuntimeEvent::SubagentCompleted {
                    id: subagent_id.clone(),
                    parent_run_id: self.runner.parent_run_id.clone(),
                    state: terminal_state.to_string(),
                    summary: bounded_summary.clone(),
                    duration_ms,
                    background,
                });
        }
        persistence_result
    }
    fn persist_transcript(&self, state: &str, summary: &str) -> Result<()> {
        let subagent_id = &self.id;
        let request = &self.request;
        let created_at = self.created_at;
        let transcript_path = self
            .runner
            .transcript_root
            .join(format!("{subagent_id}.json"));
        let record = deepagent_persistence::subagent_store::SubagentRunStore::new(&self.runner.db)
            .get(subagent_id)?
            .ok_or_else(|| CoreError::not_found("sub-agent record missing"))?;
        // Optional versioned metadata in the existing transcript, not a
        // second workspace store. It prevents resume-after-cleanup from
        // silently running this DAG child in the parent workspace.
        let workspace = if request.isolation == "shared" {
            record
                .worktree_path
                .map(|path| serde_json::json!({"version": 1, "kind": "dag", "path": path}))
        } else {
            None
        };
        // Serialize the complete request type once at the persistence
        // boundary. Keeping the writer on the same serde contract as the
        // reader prevents newly added request fields from silently being
        // dropped from resumable child runs.
        let current_attempt = serde_json::json!({
            "request": serde_json::to_value(request)?,
            "state": state,
            "summary": summary,
            "started_at": created_at,
            "finished_at": subagent_now_ms(),
        });
        let mut attempts = match std::fs::read(&transcript_path) {
            Ok(bytes) => serde_json::from_slice::<serde_json::Value>(&bytes)?
                .get("attempts")
                .and_then(|v| v.as_array())
                .cloned()
                .ok_or_else(|| {
                    CoreError::Persistence("sub-agent transcript has no attempts".into())
                })?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(error) => {
                return Err(CoreError::Persistence(format!(
                    "read sub-agent attempts: {error}"
                )))
            }
        };
        attempts.push(current_attempt.clone());
        let items = if let Some(surface) = &self.surface {
            let events = deepagent_persistence::event_store::EventStore::new(&surface.db)
                .load_session(surface.session_id)?;
            crate::input_runtime::response_items_from_events(&events)
        } else {
            self.seed_history.clone()
        };
        let context = (!items.is_empty()).then(
            || serde_json::json!({"version": 1, "attempt": record.resume_count, "items": items}),
        );
        let transcript = serde_json::json!({
            "subagent_id": subagent_id,
            "parent_run_id": self.runner.parent_run_id,
            "request": current_attempt["request"].clone(),
            "workspace": workspace,
            "state": state,
            "summary": summary,
            "created_at": created_at,
            "finished_at": subagent_now_ms(),
            "attempts": attempts,
            "context": context,
        });
        crate::atomic_file::write_file_atomically(
            &transcript_path,
            &serde_json::to_vec_pretty(&transcript)?,
        )
    }
}

impl Drop for TrackedSubagent<'_> {
    fn drop(&mut self) {
        if self.armed {
            // A timed-out/hard-dropped future cannot await worktree removal.
            // Keep its recorded path available to the existing task.cleanup
            // entry point, and never leave the child falsely marked running.
            if let Err(error) = self.finish("cancelled",
                "sub-agent execution future dropped before cleanup completed; retained worktree may require task.cleanup; external work may still be stopping") {
                tracing::error!(%error, subagent_id = self.id, "failed to persist dropped subagent lifecycle");
            }
        }
    }
}

impl ChatSubagentRunner {
    /// Assign a scheduler-owned workspace without changing the registry's
    /// source root. The normal execution path then rebinds workspace tools.
    pub(crate) fn with_assigned_worktree(mut self, root: PathBuf) -> Self {
        self.assigned_worktree = Some(root);
        self
    }

    pub(crate) async fn run_inner(
        &self,
        request: deepagent_builtins::SubagentRequest,
        cancel: Option<Arc<AtomicBool>>,
    ) -> Result<deepagent_builtins::SubagentOutcome> {
        self.run_inner_with_controls(request, cancel, None).await
    }

    async fn run_inner_with_controls(
        &self,
        request: deepagent_builtins::SubagentRequest,
        cancel: Option<Arc<AtomicBool>>,
        cancel_token: Option<CancellationToken>,
    ) -> Result<deepagent_builtins::SubagentOutcome> {
        let subagent_id = format!("sub_{}", deepagent_core::id::EventId::new());
        let persist = self.begin_tracking(&subagent_id, &request, false, None)?;
        self.run_tracked(
            subagent_id,
            request.into(),
            cancel,
            cancel_token,
            persist,
            false,
        )
        .await
    }

    fn begin_tracking(
        &self,
        subagent_id: &str,
        request: &deepagent_builtins::SubagentRequest,
        background: bool,
        worktree_path: Option<&str>,
    ) -> Result<bool> {
        let transcript_path = self.transcript_root.join(format!("{subagent_id}.json"));
        let created_at = subagent_now_ms();
        let persist = deepagent_persistence::run_store::RunStore::new(&self.db)
            .get(&self.parent_run_id)?
            .is_some();
        if persist {
            if let Some(parent) = transcript_path.parent() {
                std::fs::create_dir_all(parent).map_err(|error| {
                    CoreError::Persistence(format!(
                        "failed to create subagent transcript directory: {error}"
                    ))
                })?;
            }
            deepagent_persistence::subagent_store::SubagentRunStore::new(&self.db).create(
                &deepagent_persistence::subagent_store::SubagentRunRecord {
                    id: subagent_id.to_string(),
                    parent_run_id: self.parent_run_id.clone(),
                    origin_parent_run_id: self.parent_run_id.clone(),
                    state: "running".to_string(),
                    agent_type: request
                        .subagent_type
                        .clone()
                        .unwrap_or_else(|| "general".to_string()),
                    transcript_path: Some(transcript_path.to_string_lossy().to_string()),
                    worktree_path: worktree_path.map(str::to_string),
                    summary: None,
                    created_at,
                    updated_at: created_at,
                    finished_at: None,
                    resume_count: 0,
                },
            )?;
        }

        self.emit_subagent_event(RuntimeEvent::SubagentStarted {
            id: subagent_id.to_string(),
            parent_run_id: self.parent_run_id.clone(),
            agent_type: request
                .subagent_type
                .clone()
                .unwrap_or_else(|| "general".to_string()),
            description: request.description.clone(),
            background,
        });

        Ok(persist)
    }

    fn emit_subagent_event(&self, event: RuntimeEvent) {
        let status = if matches!(event, RuntimeEvent::SubagentStarted { .. }) {
            "progress"
        } else {
            "completed"
        };
        if let Err(error) = deepagent_persistence::run_store::RunStore::new(&self.db).append_event(
            &self.parent_run_id,
            subagent_now_ms(),
            "executing_tools",
            status,
            event.label(),
            &deepagent_runtime::redaction::scrub_secrets_value(
                serde_json::to_value(&event).unwrap_or(serde_json::Value::Null),
            ),
        ) {
            tracing::warn!(%error, parent_run_id = self.parent_run_id, event = event.label(), "failed to persist subagent lifecycle event");
        }
        if let Some(events) = self.events.upgrade() {
            events.emit(event);
        }
    }

    fn parent_session_id(&self) -> Result<deepagent_core::id::SessionId> {
        let run = deepagent_persistence::run_store::RunStore::new(&self.db)
            .get(&self.parent_run_id)?
            .ok_or_else(|| CoreError::not_found("parent run is not persisted"))?;
        deepagent_core::id::SessionId::from_str(&run.session_id)
    }

    /// Reconstruct the parent session's conversation for a full-context fork
    /// (Claude Code fork inherits the parent's exact prefix). Best-effort: an
    /// unpersisted parent (ephemeral/test run) or a load failure yields an
    /// empty history, so a fork degrades gracefully to a fresh child rather
    /// than failing.
    fn parent_response_history(&self) -> Vec<deepagent_models::ResponseInputItem> {
        let Ok(session_id) = self.parent_session_id() else {
            return Vec::new();
        };
        let store = deepagent_persistence::event_store::EventStore::new(&self.db);
        match store.load_session(session_id) {
            Ok(events) => crate::input_runtime::response_items_from_events(&events),
            Err(error) => {
                tracing::warn!(%error, "fork: failed to load parent Responses history; starting fresh");
                Vec::new()
            }
        }
    }

    async fn dispatch_child_hook(&self, point: HookPoint, data: HookData) -> Result<HookOutcome> {
        let Some(hooks) = self.hooks.get() else {
            return Ok(HookOutcome::Continue);
        };
        hooks
            .dispatch(&HookContext::new(self.parent_session_id()?, point, data))
            .await
    }

    pub(crate) fn worktree_base(&self) -> PathBuf {
        let project = self
            .root
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("workspace");
        self.root
            .parent()
            .unwrap_or(&self.root)
            .join(".deepagent-worktrees")
            .join(project)
    }

    async fn execution_root(
        &self,
        subagent_id: &str,
        request: &deepagent_builtins::SubagentRequest,
        persist: bool,
    ) -> Result<(PathBuf, bool)> {
        if request.isolation != "worktree" {
            // DAG workspaces are scheduler-owned but recorded in the same
            // child store. Resume must reuse them, not switch to the parent.
            // false leaves cleanup to an explicit task.cleanup.
            let retained = if persist {
                deepagent_persistence::subagent_store::SubagentRunStore::new(&self.db)
                    .get(subagent_id)?
                    .and_then(|record| record.worktree_path)
                    .map(PathBuf::from)
            } else {
                None
            };
            if let Some(path) = retained.or_else(|| self.assigned_worktree.clone()) {
                if !path.is_dir() {
                    return Err(CoreError::not_found("retained DAG worktree is missing"));
                }
                return Ok((path, false));
            }
            return Ok((self.root.clone(), false));
        }
        if !persist {
            return Err(CoreError::invalid(
                "worktree isolation requires Agent Kernel v2 persistence",
            ));
        }
        let store = deepagent_persistence::subagent_store::SubagentRunStore::new(&self.db);
        if let Some(existing) = store
            .get(subagent_id)?
            .and_then(|record| record.worktree_path)
            .map(PathBuf::from)
            .filter(|path| path.is_dir())
        {
            return Ok((existing, true));
        }

        use deepagent_subagents::WorktreeProvider;
        let provider = deepagent_subagents::GitWorktrees::new(&self.root, self.worktree_base());
        let planned = self.worktree_base().join(subagent_id);
        match self
            .dispatch_child_hook(
                HookPoint::WorktreeCreate,
                HookData::Path {
                    path: planned.to_string_lossy().to_string(),
                },
            )
            .await?
        {
            HookOutcome::Deny { reason, .. } | HookOutcome::Ask { reason, .. } => {
                return Err(CoreError::other(format!(
                    "worktree creation blocked by hook: {reason}"
                )))
            }
            HookOutcome::Modify { updated_input, .. } => {
                let rewritten = updated_input
                    .get("path")
                    .and_then(serde_json::Value::as_str)
                    .map(PathBuf::from);
                if rewritten.as_deref() != Some(planned.as_path()) {
                    return Err(CoreError::invalid(
                        "WorktreeCreate hook attempted to rewrite the managed worktree path",
                    ));
                }
            }
            HookOutcome::Continue | HookOutcome::AsyncPending { .. } => {}
        }
        let worktree = provider.create(subagent_id).await?;
        store.set_worktree_path(subagent_id, Some(&worktree.path), subagent_now_ms())?;
        self.emit_subagent_event(RuntimeEvent::WorktreeCreated {
            subagent_id: subagent_id.to_string(),
            path: worktree.path.clone(),
        });
        Ok((PathBuf::from(worktree.path), true))
    }

    async fn remove_worktree(&self, subagent_id: &str, path: &str) -> Result<()> {
        use deepagent_subagents::WorktreeProvider;
        let _ = self
            .dispatch_child_hook(
                HookPoint::WorktreeRemove,
                HookData::Path {
                    path: path.to_string(),
                },
            )
            .await?;
        deepagent_subagents::GitWorktrees::new(&self.root, self.worktree_base())
            .remove(subagent_id)
            .await?;
        deepagent_persistence::subagent_store::SubagentRunStore::new(&self.db).set_worktree_path(
            subagent_id,
            None,
            subagent_now_ms(),
        )?;
        self.emit_subagent_event(RuntimeEvent::WorktreeRemoved {
            subagent_id: subagent_id.to_string(),
            path: path.to_string(),
        });
        Ok(())
    }

    async fn run_tracked(
        &self,
        subagent_id: String,
        input: SubagentInput,
        cancel: Option<Arc<AtomicBool>>,
        cancel_token: Option<CancellationToken>,
        persist: bool,
        background: bool,
    ) -> Result<deepagent_builtins::SubagentOutcome> {
        let SubagentInput { request, history } = input;
        let mut lifecycle = TrackedSubagent {
            runner: self,
            id: subagent_id.clone(),
            request: request.clone(),
            created_at: subagent_now_ms(),
            persist,
            background,
            armed: true,
            surface: None,
            seed_history: history,
            origin_activity: None,
        };
        // Child transcripts may use a separate DB. Their durable origin still
        // owns archive admission, including background work after main teardown.
        let origin_activity: Result<_> = (|| {
            let origin = deepagent_persistence::run_store::RunStore::new(&self.db)
                .get(&self.parent_run_id)?;
            origin
                .map(|origin| {
                    crate::ArchiveService::new(self.db.clone())
                        .active_guard(origin.session_id.parse()?)
                })
                .transpose()
        })();
        match origin_activity {
            Ok(guard) => lifecycle.origin_activity = guard,
            Err(error) => {
                lifecycle.finish("failed", &error.to_string())?;
                return Err(error);
            }
        }
        let _ = self
            .dispatch_child_hook(
                HookPoint::SubagentStart,
                HookData::Subagent {
                    agent_id: subagent_id.clone(),
                    agent_type: request
                        .subagent_type
                        .clone()
                        .unwrap_or_else(|| "general".to_string()),
                    summary: None,
                },
            )
            .await;
        let (execution_root, isolated, preparation_error) =
            match self.execution_root(&subagent_id, &request, persist).await {
                Ok((root, isolated)) => (root, isolated, None),
                Err(error) => {
                    tracing::warn!(%error, subagent_id, "failed to prepare child execution root");
                    (self.root.clone(), false, Some(error))
                }
            };
        // CwdChanged hook (Phase E): the effective working directory diverged
        // from the session workspace (isolated worktree). Observational.
        if execution_root != self.root {
            let _ = self
                .dispatch_child_hook(
                    HookPoint::CwdChanged,
                    HookData::Path {
                        path: execution_root.to_string_lossy().to_string(),
                    },
                )
                .await;
        }
        let mut result = if let Some(error) = preparation_error {
            Err(error)
        } else {
            self.run_active(
                request.clone(),
                cancel.clone(),
                cancel_token.clone(),
                &execution_root,
                &lifecycle.seed_history,
                &mut lifecycle.surface,
            )
            .await
        };
        let cancelled = cancel
            .as_ref()
            .is_some_and(|flag| flag.load(std::sync::atomic::Ordering::Acquire))
            || cancel_token
                .as_ref()
                .is_some_and(CancellationToken::is_cancelled);
        let state = if cancelled {
            "cancelled"
        } else if result.is_ok() {
            "succeeded"
        } else {
            "failed"
        };
        if isolated {
            let path = execution_root.to_string_lossy().to_string();
            if state == "succeeded" {
                if let Ok(outcome) = &mut result {
                    outcome
                        .result
                        .push_str("\n\nIsolated worktree retained at: ");
                    outcome.result.push_str(&path);
                }
            } else if let Err(error) = self.remove_worktree(&subagent_id, &path).await {
                tracing::warn!(%error, subagent_id, "failed to clean up terminal child worktree");
            }
        }
        let summary = match &result {
            Ok(outcome) => outcome.result.clone(),
            Err(error) => error.to_string(),
        };
        let bounded_summary = subagent_summary(&summary, 2_000);
        let _ = self
            .dispatch_child_hook(
                HookPoint::SubagentStop,
                HookData::Subagent {
                    agent_id: subagent_id.clone(),
                    agent_type: request
                        .subagent_type
                        .clone()
                        .unwrap_or_else(|| "general".to_string()),
                    summary: Some(bounded_summary.clone()),
                },
            )
            .await;
        lifecycle.finish(state, &summary)?;
        if background {
            self.emit_subagent_event(RuntimeEvent::SubagentNotification {
                id: subagent_id.clone(),
                parent_run_id: self.parent_run_id.clone(),
                state: state.to_string(),
                summary: bounded_summary.clone(),
            });
            // Notification hook (Phase E): a background child's terminal
            // result surfacing into the parent session is the canonical
            // "notification" lifecycle moment (Claude parity).
            let _ = self
                .dispatch_child_hook(
                    HookPoint::Notification,
                    HookData::Notification {
                        message: format!(
                            "background sub-agent {subagent_id} finished ({state}): {bounded_summary}"
                        ),
                    },
                )
                .await;
        }
        result
    }

    async fn run_active(
        &self,
        request: deepagent_builtins::SubagentRequest,
        cancel: Option<Arc<AtomicBool>>,
        cancel_token: Option<CancellationToken>,
        execution_root: &Path,
        history: &[ResponseInputItem],
        surface: &mut Option<crate::context_runtime::SessionSurfaceStore>,
    ) -> Result<deepagent_builtins::SubagentOutcome> {
        use deepagent_runtime::{ModelAgent, RunOutcome, RuntimeConfig, RuntimeEngine};

        let agent_profile = request
            .subagent_type
            .as_deref()
            .and_then(|agent_type| self.agent_definitions.get(agent_type));
        let sub_discovered: DiscoveredToolSet = Arc::new(std::sync::Mutex::new(
            self.parent_discovered_snapshot.clone(),
        ));
        let mut sub_registry: ToolRegistry = (*self.registry).clone();
        if execution_root != self.root {
            let rebound = self
                .host
                .build_registry(
                    execution_root,
                    self.access,
                    None,
                    None,
                    Some(self.local_execution_mode),
                    self.bash_full_access,
                )?
                .0;
            for spec in rebound.iter_specs() {
                // The parent registry captured the session's project when the
                // run started. Rebinding filesystem tools to an isolated root
                // must not change session_search to the currently active UI
                // project (which may have changed meanwhile). The child also
                // keeps the parent's project knowledge, not a fresh worktree
                // vault or the UI's current selection.
                if matches!(
                    spec.tool.descriptor().name.as_str(),
                    "session_search" | "knowledge_search"
                ) {
                    continue;
                }
                sub_registry.try_replace(spec.tool.clone())?;
            }
        }
        // The tool list is an execution capability boundary, not merely a
        // prompt/schema hint.  Build the same restricted registry that will
        // feed both ModelAgent's schemas and RuntimeEngine's dispatch.  A
        // hidden write tool must therefore resolve as UnknownTool rather than
        // remaining callable through the cloned parent registry.
        if !request.fork {
            retain_runtime_agent_tools(&mut sub_registry, agent_profile, &request.allowed_tools)?;
        }
        let mode = effective_tool_search_mode(
            &sub_registry,
            self.tool_search_mode,
            self.tool_search_auto_threshold,
        );
        register_tool_search_into(
            &mut sub_registry,
            mode,
            sub_discovered.clone(),
            self.tool_search_auto_threshold,
        )?;
        let granted = permission_set_for_agent(agent_profile);
        // Fork (Claude Code full-context fork, `tools:['*']`): the child gets
        // the parent's FULL tool pool — skip the agent-profile / allowlist
        // restrictions. A normal sub-agent stays restricted to its profile.
        let schema_source = live_tool_schema_source(
            sub_registry.clone(),
            granted.clone(),
            mode,
            sub_discovered.clone(),
            self.tool_search_auto_threshold,
        );
        let schema_agent_profile = agent_profile.cloned();
        let schema_allowed_tools = request.allowed_tools.clone();
        let schema_fork = request.fork;
        let schema_source = Arc::new(move || {
            let mut tools = schema_source();
            if !schema_fork {
                if schema_allowed_tools.is_empty() {
                    apply_runtime_agent_tool_filter(&mut tools, schema_agent_profile.as_ref());
                } else {
                    apply_runtime_agent_tool_filter_with_request(
                        &mut tools,
                        schema_agent_profile.as_ref(),
                        &schema_allowed_tools,
                    );
                }
            }
            tools
        });
        let tools = schema_source();
        // Fork inherits the parent's COMPLETE conversation (byte-similar prefix
        // so the child continues from the exact current state). Best-effort:
        // reconstruct from the parent session's event log.
        let fork_response_history = if request.fork {
            self.parent_response_history()
        } else {
            history.to_vec()
        };
        // Skills: agent-file defaults unioned with the per-call `skills` override.
        let mut skills = agent_profile
            .and_then(|agent| agent.def.skills.clone())
            .unwrap_or_default();
        for skill in &request.skills {
            if !skills.iter().any(|existing| existing == skill) {
                skills.push(skill.clone());
            }
        }
        let preloaded_skills = self.preload_skills(&skills)?;
        let system =
            subagent_system_prompt(execution_root, agent_profile, &preloaded_skills, &request);
        let snapshot = crate::runtime_snapshot::SnapshotPlan::new(&system, &fork_response_history)?;
        let db = Arc::new(Database::open_in_memory()?);
        let clock = SystemClock;
        let mut session = Session::create(&db, &clock, Some(&request.description))?;
        for item in &fork_response_history {
            session.append_response_item(item.clone())?;
        }
        session.append(deepagent_core::event::EventPayload::MessageAppended {
            message: deepagent_core::message::Message::user(&request.prompt),
        })?;
        snapshot.persist(&mut session)?;
        // Publish the fully initialized event source before awaited execution.
        // A failed history import retains the complete seed instead of a partial
        // copy; the owner also retains this source if the engine is dropped.
        *surface = Some(crate::context_runtime::SessionSurfaceStore {
            db: db.clone(),
            session_id: session.id(),
        });
        let task = session.create_task(&request.prompt)?;
        let sub_model = request.model.clone().unwrap_or_else(|| self.model.clone());
        let sub_reasoning_effort = subagent_reasoning_effort(
            subagent_effort_str(agent_profile, &request),
            self.reasoning_effort,
        )?;
        // Sub-agents run their own model loop; without compaction a long task
        // would only be bounded by the 64-step / 1M-token hard ceilings. Give it
        // the same reactive + proactive compaction the main run uses. The
        // sub-agent engine has no hook registry, so it compacts without hook
        // dispatch (an empty registry returns `Continue`).
        let context_policy = deepagent_context::ContextPolicy::for_capability(
            &self.client.capability(&sub_model),
            sub_reasoning_effort,
        );
        let mut reactive_compactor = crate::context_runtime::HookedReactiveContextCompactor::new(
            self.client.clone(),
            sub_model.clone(),
            Arc::new(deepagent_hooks::HookRegistry::new()),
            session.id(),
            db.clone(),
        );
        if let Some(owner) = &self.usage_owner {
            reactive_compactor = reactive_compactor.with_call_usage_sink(
                owner.invocation(session.id().to_string(), "subagent_compaction"),
            );
        }
        let reactive_compactor: Arc<dyn deepagent_runtime::ReactiveContextCompactor> =
            Arc::new(reactive_compactor);
        let mut agent = ModelAgent::new(
            self.client.clone(),
            sub_model,
            snapshot.instructions,
            &request.prompt,
            tools,
        )
        .with_tool_schema_source(schema_source)
        .with_response_history(fork_response_history)
        .with_admitted_input_context(snapshot.update)
        .with_surface_persistence(Arc::new(crate::context_runtime::SessionSurfaceStore {
            db: db.clone(),
            session_id: session.id(),
        }))
        .with_reasoning_effort(sub_reasoning_effort)
        .with_reactive_compactor(reactive_compactor)
        .with_proactive_compaction(context_policy.autocompact_threshold_tokens(None, None) as u64)
        .with_context_policy(context_policy, None, None);
        if let Some(owner) = &self.usage_owner {
            agent =
                agent.with_call_usage_sink(owner.invocation(session.id().to_string(), "subagent"));
        }
        let checkpoint = Arc::new(deepagent_runtime::CheckpointManager::new(
            self.db.clone(),
            self.parent_run_id.clone(),
            // Anchor to the parent's session sequence so a session rewind
            // past the parent turn also restores files the sub-agent touched
            // (a fixed 0 would exempt sub-agent checkpoints from every
            // rewind). Falls back to 0 when the parent checkpoint has not
            // been published yet (ephemeral/test runs).
            self.parent_checkpoint
                .get()
                .map(|parent| parent.session_sequence())
                .unwrap_or(0),
            execution_root,
            self.transcript_root.join("checkpoints"),
        )?);
        let config = RuntimeConfig {
            permissions: granted,
            // Completion gate no longer derives requirements from the prompt
            // (intent-guessing anti-pattern removed 2026-07-28). Empty policy
            // = model self-reports; never blocks on keyword heuristics.
            completion_policy: deepagent_runtime::CompletionPolicy::default(),
            checkpoint: Some(checkpoint.clone()),
            // An agent file's `maxTurns` bounds that sub-agent; without one the
            // loop is unbounded like the main run, so delegated long work is
            // not cut off at a step count.
            max_steps: agent_profile.and_then(|agent| agent.def.max_turns),
            ..Default::default()
        };
        let mut engine = RuntimeEngine::new(&sub_registry, Default::default(), config);
        // Bubble a sub-agent tool's approval need to the parent's UI gate when
        // one is wired; otherwise the engine keeps its default (auto-deny).
        // `permissionMode: bypassPermissions` replaces any human gate outright.
        match agent_profile.and_then(|agent| agent.def.permission_mode) {
            Some(deepagent_prompts::PermissionMode::BypassPermissions) => {
                engine = engine.with_approvals(std::sync::Arc::new(
                    deepagent_runtime::approval::AutoApproveGate,
                ));
            }
            _ => {
                if let Some(approvals) = self.parent_approvals.get() {
                    engine = engine.with_approvals(approvals.clone());
                }
            }
        }
        if let Some(cancel) = cancel {
            engine = engine.with_cancel(cancel);
        }
        if let Some(cancel_token) = cancel_token {
            engine = engine.with_cancellation_token(cancel_token);
        }
        let started_at = std::time::Instant::now();
        let outcome = engine.run(&mut session, task, &mut agent).await;
        if let Ok(evidence) = checkpoint.mutation_evidence() {
            if let Some(parent) = self.parent_checkpoint.get() {
                parent.record_external_evidence(evidence);
            }
        }
        match outcome? {
            RunOutcome::Completed(msg) => {
                let tokens = deepagent_runtime::Agent::cumulative_usage(&agent)
                    .map(|usage| usage.total_tokens as u64)
                    .unwrap_or(0);
                Ok(deepagent_builtins::SubagentOutcome {
                    result: msg,
                    tool_use_count: session.state().tool_calls_requested,
                    duration_ms: started_at.elapsed().as_millis() as u64,
                    tokens,
                })
            }
            RunOutcome::AwaitingApproval(message) => Err(CoreError::other(format!(
                "sub-agent paused awaiting approval: {message}"
            ))),
            RunOutcome::StepLimitReached => Err(CoreError::other(
                "sub-agent reached its step limit without a final answer",
            )),
            RunOutcome::Cancelled => Err(CoreError::other("sub-agent was cancelled")),
            RunOutcome::BudgetExceeded(reason) => Err(CoreError::other(format!(
                "sub-agent budget exhausted: {reason}"
            ))),
            RunOutcome::CompletionFailed(reason) => Err(CoreError::other(format!(
                "sub-agent completion verification failed: {reason}"
            ))),
        }
    }

    fn preload_skills(&self, ids: &[String]) -> Result<String> {
        if ids.is_empty() {
            return Ok(String::new());
        }
        let service = self.skills.as_ref().ok_or_else(|| {
            CoreError::invalid("sub-agent requested skills, but no skills service is configured")
        })?;
        let service = service
            .lock()
            .map_err(|_| CoreError::other("skills service mutex poisoned"))?;
        let registry = service.manager().registry();
        let mut output = String::from("# Preloaded skills\n<preloaded-skills>\n");
        for id in ids {
            let skill = registry
                .get(id)
                .ok_or_else(|| CoreError::invalid(format!("unknown sub-agent skill: {id}")))?;
            output.push_str("<skill id=\"");
            output.push_str(&skill.meta.id);
            output.push_str("\" name=\"");
            output.push_str(&skill.meta.name);
            output.push_str("\">\n");
            if let Some(base_dir) = &skill.base_dir {
                output.push_str("Base directory: ");
                output.push_str(&base_dir.display().to_string());
                output.push('\n');
            }
            output.push_str(skill.body.trim());
            output.push_str("\n</skill>\n");
        }
        output.push_str("</preloaded-skills>\n");
        Ok(output)
    }

    fn resume_request(
        &self,
        record: &deepagent_persistence::subagent_store::SubagentRunRecord,
        continuation: &str,
    ) -> Result<SubagentInput> {
        let transcript_path = record.transcript_path.as_deref().ok_or_else(|| {
            CoreError::not_found(format!(
                "NOT_RESUMABLE: sub-agent transcript is unavailable: {}",
                record.id
            ))
        })?;
        let transcript: serde_json::Value =
            serde_json::from_slice(&std::fs::read(transcript_path).map_err(|error| {
                CoreError::Persistence(format!(
                    "NOT_RESUMABLE: failed to read sub-agent transcript: {error}"
                ))
            })?)
            .map_err(|error| {
                CoreError::Persistence(format!(
                    "NOT_RESUMABLE: invalid sub-agent transcript: {error}"
                ))
            })?;
        if transcript["workspace"]["kind"].as_str() == Some("dag") && record.worktree_path.is_none()
        {
            return Err(CoreError::not_found(
                "DAG worktree was explicitly cleaned up; cannot resume in the parent workspace",
            ));
        }
        let saved = transcript
            .get("request")
            .and_then(serde_json::Value::as_object)
            .ok_or_else(|| CoreError::Persistence("sub-agent transcript has no request".into()))?;
        let context = &transcript["context"];
        if transcript["subagent_id"].as_str() != Some(record.id.as_str())
            || context["version"].as_u64() != Some(1)
            || context["attempt"].as_u64() != Some(u64::from(record.resume_count))
        {
            return Err(CoreError::invalid(
                "NOT_RESUMABLE: missing, unsupported or stale sub-agent context; start a new task",
            ));
        }
        let items: Vec<ResponseInputItem> = serde_json::from_value(context["items"].clone())
            .map_err(|_| CoreError::invalid("NOT_RESUMABLE: invalid sub-agent context items"))?;
        if items.is_empty() {
            return Err(CoreError::invalid("NOT_RESUMABLE: empty sub-agent context"));
        }
        // Reuse the provider-neutral repair/identity boundary. An interrupted
        // call gets the existing explicit 'result missing' error, never replayed execution.
        let restored =
            deepagent_models::ResponseRequest::from_response_items(&self.model, None, items);
        restored
            .validate()
            .map_err(|_| CoreError::invalid("NOT_RESUMABLE: ambiguous sub-agent tool history"))?;
        let mut request: deepagent_builtins::SubagentRequest =
            serde_json::from_value(serde_json::Value::Object(saved.clone())).map_err(|error| {
                CoreError::Persistence(format!("invalid sub-agent request: {error}"))
            })?;
        if request.description.trim().is_empty() {
            request.description = "Resume sub-agent".to_string();
        }
        if request.isolation.trim().is_empty() {
            request.isolation = "shared".to_string();
        }
        request.prompt = continuation.to_string();
        request.subagent_type = request
            .subagent_type
            .or_else(|| (record.agent_type != "general").then(|| record.agent_type.clone()));
        // A resumed child continues its own line; it is not a fresh fork.
        request.fork = false;
        Ok(SubagentInput {
            request,
            history: restored.input,
        })
    }

    /// All model-side controls use the persisted origin conversation, not the
    /// shared host handle table or an ID supplied by the model, as authority.
    fn ensure_control_lineage(
        &self,
        record: &deepagent_persistence::subagent_store::SubagentRunRecord,
    ) -> Result<()> {
        let runs = deepagent_persistence::run_store::RunStore::new(&self.db);
        let current = runs
            .get(&self.parent_run_id)?
            .ok_or_else(|| CoreError::not_found("current parent run is not persisted"))?;
        let origin = runs
            .get(&record.origin_parent_run_id)?
            .ok_or_else(|| CoreError::not_found("sub-agent origin run is not persisted"))?;
        if current.session_id != origin.session_id {
            return Err(CoreError::invalid(
                "cannot control a sub-agent from another conversation",
            ));
        }
        Ok(())
    }
}

fn subagent_now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
}

fn subagent_summary(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    let mut summary = value
        .chars()
        .take(max_chars.saturating_sub(1))
        .collect::<String>();
    summary.push('…');
    summary
}

fn subagent_reasoning_effort(
    effort: Option<&str>,
    inherited: ReasoningEffort,
) -> Result<ReasoningEffort> {
    match effort {
        None => Ok(inherited),
        Some(value) => ReasoningEffort::parse(value).ok_or_else(|| {
            CoreError::invalid(format!(
                "invalid sub-agent effort '{value}'; expected none, low, high, or max"
            ))
        }),
    }
}

#[async_trait::async_trait]
impl deepagent_builtins::SubagentRunner for ChatSubagentRunner {
    async fn run(&self, request: deepagent_builtins::SubagentRequest) -> Result<String> {
        self.run_inner(request, None)
            .await
            .map(|outcome| outcome.result)
    }

    async fn run_controlled(
        &self,
        request: deepagent_builtins::SubagentRequest,
        context: deepagent_tools::ToolExecutionContext,
    ) -> Result<deepagent_builtins::SubagentOutcome> {
        self.run_inner_with_controls(
            request,
            Some(context.cancel_flag()),
            Some(context.cancellation_token()),
        )
        .await
    }

    async fn start_background(
        &self,
        request: deepagent_builtins::SubagentRequest,
        context: deepagent_tools::ToolExecutionContext,
    ) -> Result<deepagent_builtins::BackgroundSubagent> {
        if context.is_cancelled() {
            return Err(CoreError::other("parent run is already cancelled"));
        }
        let subagent_id = format!("sub_{}", deepagent_core::id::EventId::new());
        let persist = self.begin_tracking(&subagent_id, &request, true, None)?;
        if !persist {
            return Err(CoreError::other(
                "background sub-agents require Agent Kernel v2 persistence",
            ));
        }
        let child_cancel = Arc::new(AtomicBool::new(false));
        self.background
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .insert(subagent_id.clone(), child_cancel.clone());

        let runner = self.clone();
        let task_id = subagent_id.clone();
        tokio::spawn(async move {
            // Background agents are detached from the foreground run's
            // cancellation signal. They are stopped only by their own
            // `cancel_subagent` request (or by process shutdown).
            let _ = runner
                .run_tracked(
                    task_id.clone(),
                    request.into(),
                    Some(child_cancel.clone()),
                    None,
                    true,
                    true,
                )
                .await;
            runner
                .background
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .remove(&task_id);
        });

        Ok(deepagent_builtins::BackgroundSubagent {
            id: subagent_id,
            state: "running".to_string(),
        })
    }

    async fn status(&self, id: &str) -> Result<deepagent_builtins::SubagentStatus> {
        let record = deepagent_persistence::subagent_store::SubagentRunStore::new(&self.db)
            .get(id)?
            .ok_or_else(|| CoreError::not_found(format!("sub-agent not found: {id}")))?;
        self.ensure_control_lineage(&record)?;
        Ok(deepagent_builtins::SubagentStatus {
            id: record.id,
            state: record.state,
            summary: record.summary,
            worktree_path: record.worktree_path,
        })
    }

    async fn cancel(&self, id: &str) -> Result<bool> {
        let Some(record) =
            deepagent_persistence::subagent_store::SubagentRunStore::new(&self.db).get(id)?
        else {
            return Ok(false);
        };
        self.ensure_control_lineage(&record)?;
        if record.finished_at.is_some() {
            return Ok(false);
        }
        let flag = self
            .background
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .get(id)
            .cloned();
        Ok(flag
            .map(|flag| !flag.swap(true, std::sync::atomic::Ordering::AcqRel))
            .unwrap_or(false))
    }

    async fn resume(
        &self,
        id: &str,
        prompt: &str,
        context: deepagent_tools::ToolExecutionContext,
    ) -> Result<String> {
        if context.is_cancelled() {
            return Err(CoreError::other("parent run is already cancelled"));
        }
        let store = deepagent_persistence::subagent_store::SubagentRunStore::new(&self.db);
        let record = store
            .get(id)?
            .ok_or_else(|| CoreError::not_found(format!("sub-agent not found: {id}")))?;
        self.ensure_control_lineage(&record)?;
        let input = self.resume_request(&record, prompt)?;
        store.resume_from_attempt(
            id,
            &self.parent_run_id,
            record.resume_count,
            subagent_now_ms(),
        )?;
        self.emit_subagent_event(RuntimeEvent::SubagentStarted {
            id: id.to_string(),
            parent_run_id: self.parent_run_id.clone(),
            agent_type: record.agent_type,
            description: input.request.description.clone(),
            background: false,
        });
        self.run_tracked(
            id.to_string(),
            input,
            Some(context.cancel_flag()),
            Some(context.cancellation_token()),
            true,
            false,
        )
        .await
        .map(|outcome| outcome.result)
    }

    async fn cleanup(&self, id: &str) -> Result<bool> {
        let record = deepagent_persistence::subagent_store::SubagentRunStore::new(&self.db)
            .get(id)?
            .ok_or_else(|| CoreError::not_found(format!("sub-agent not found: {id}")))?;
        self.ensure_control_lineage(&record)?;
        if record.state == "running" {
            return Err(CoreError::invalid(
                "cannot remove a worktree while its sub-agent is running",
            ));
        }
        let Some(path) = record.worktree_path else {
            return Ok(false);
        };
        self.remove_worktree(id, &path).await?;
        Ok(true)
    }
}

/// Bridges the production sub-agent loop ([`ChatSubagentRunner`]) to the
/// planner's [`deepagent_subagents::SubAgentExecutor`] seam, so a DAG of
/// sub-tasks runs through the SAME agent loop the `task` tool uses — one
/// executor, not a second orchestration stack (D1 convergence).
///
/// The scheduler ([`deepagent_subagents::DagScheduler`]) owns worktree
/// isolation, so each node's runner is re-rooted at the scheduler-provided
/// worktree and runs with `shared` isolation (it does not provision a second
/// worktree per node).
pub(crate) struct DagSubagentExecutor {
    runner: ChatSubagentRunner,
}

impl DagSubagentExecutor {
    pub(crate) fn new(runner: ChatSubagentRunner) -> Self {
        Self { runner }
    }
}

#[async_trait::async_trait]
impl deepagent_subagents::SubAgentExecutor for DagSubagentExecutor {
    async fn execute(
        &self,
        context: deepagent_subagents::SubAgentContext,
    ) -> Result<deepagent_subagents::SubAgentResult> {
        let mut prompt = context.goal.clone();
        if !context.upstream_results.is_empty() {
            prompt.push_str("\n\n## Upstream results\n");
            for (index, summary) in context.upstream_results.iter().enumerate() {
                prompt.push_str(&format!("{}. {}\n", index + 1, summary));
            }
        }
        let request = deepagent_builtins::SubagentRequest {
            description: context.node_id.clone(),
            prompt,
            subagent_type: context.role.clone(),
            allowed_tools: Vec::new(),
            model: None,
            effort: None,
            skills: Vec::new(),
            // The DAG scheduler owns the worktree; run inside it rather than
            // provisioning a second one.
            isolation: "shared".to_string(),
            fork: false,
            // Frontmatter fields from DAG context
            phase: context.phase.clone(),
            label: Some(context.node_id.clone()),
            schema: None,
        };
        // The existing child id also names its worktree, so durable lookup,
        // resume and explicit cleanup agree on the same managed resource.
        let id = context.worktree.name.clone();
        let persist =
            self.runner
                .begin_tracking(&id, &request, false, Some(&context.worktree.path))?;
        self.runner
            .emit_subagent_event(RuntimeEvent::WorktreeCreated {
                subagent_id: id.clone(),
                path: context.worktree.path.clone(),
            });
        let runner = self
            .runner
            .clone()
            .with_assigned_worktree(PathBuf::from(context.worktree.path.clone()));
        match runner
            .run_tracked(id, request.into(), None, None, persist, false)
            .await
        {
            Ok(outcome) => Ok(deepagent_subagents::SubAgentResult::success(
                context.node_id,
                subagent_summary(&outcome.result, 2000),
            )),
            Err(error) => Ok(deepagent_subagents::SubAgentResult::failure(
                context.node_id,
                error.to_string(),
            )),
        }
    }
}

#[cfg(test)]
#[path = "subagent_workspace_tests.rs"]
mod workspace_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use deepagent_models::ToolSchema;
    use deepagent_prompts::AgentDef;
    use deepagent_tools::{RiskLevel, Tool, ToolDescriptor, ToolOutput};

    fn profile(tools: &[&str], disallowed: &[&str]) -> Option<RuntimeAgentDefinition> {
        let def = AgentDef {
            name: "t".into(),
            description: "d".into(),
            tools: tools.iter().map(|s| s.to_string()).collect(),
            model: deepagent_prompts::ModelPref::Inherit,
            color: None,
            body: "b".into(),
            disallowed_tools: (!disallowed.is_empty())
                .then(|| disallowed.iter().map(|s| s.to_string()).collect()),
            permission_mode: None,
            max_turns: None,
            effort: None,
            skills: None,
            background: None,
            isolation: None,
        };
        Some(RuntimeAgentDefinition {
            type_name: "t".into(),
            source_label: "test".into(),
            def,
        })
    }

    fn allowlist_profile_tools(tools: &[&str]) -> Vec<ToolSchema> {
        tools
            .iter()
            .map(|name| ToolSchema::function(*name, "desc", serde_json::Value::Null))
            .collect()
    }

    struct DeferredWriteTool;

    #[async_trait::async_trait]
    impl Tool for DeferredWriteTool {
        fn descriptor(&self) -> ToolDescriptor {
            ToolDescriptor {
                name: "write_file".into(),
                description: "deferred audit write capability".into(),
                parameters: serde_json::json!({"type": "object"}),
                risk: RiskLevel::Low,
                required_permissions: PermissionSet::from_iter_perms([Permission::WorkspaceWrite]),
            }
        }

        async fn invoke(&self, _: serde_json::Value) -> Result<ToolOutput> {
            Ok(ToolOutput::success(serde_json::json!({"unexpected": true})))
        }

        fn should_defer(&self) -> bool {
            true
        }

        fn discovery_source(&self) -> Option<&str> {
            Some("audit-mcp")
        }
    }

    #[derive(Default)]
    struct AuditChildWriteTransport {
        tools_seen: std::sync::Mutex<Vec<Vec<String>>>,
        runtime_contexts_seen: std::sync::Mutex<Vec<String>>,
    }

    #[async_trait::async_trait]
    impl deepagent_models::transport::HttpTransport for AuditChildWriteTransport {
        async fn stream(
            &self,
            request: deepagent_models::transport::TransportRequest,
            sink: &mut dyn deepagent_models::transport::EventSink,
        ) -> Result<()> {
            let sink: &mut dyn deepagent_models::transport::EventSink =
                &mut crate::test_sequence::sink(sink);
            let body: serde_json::Value = serde_json::from_str(&request.body)?;
            let context = body["input"]
                .as_array()
                .into_iter()
                .flatten()
                .find_map(|item| {
                    let item = serde_json::from_value(item.clone()).ok()?;
                    deepagent_core::runtime_snapshot::RuntimeSnapshot::from_item(&item)
                        .map(|snapshot| snapshot.context)
                })
                .unwrap_or_default();
            self.runtime_contexts_seen.lock().unwrap().push(context);
            let tools = body["tools"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|tool| tool["name"].as_str().map(str::to_string))
                .collect();
            let step = {
                let mut seen = self.tools_seen.lock().unwrap();
                seen.push(tools);
                seen.len()
            };
            let mut output = Vec::new();
            if step == 1 {
                let item = serde_json::json!({"type":"function_call","id":"write-item",
                    "call_id":"hidden-write","name":"write_file",
                    "arguments":r#"{"path":"audit-child.txt","content":"hidden tool executed"}"#});
                output.push(item.clone());
                sink.on_event(
                    &serde_json::json!({"type":"response.output_item.added","item":item})
                        .to_string(),
                )?;
                sink.on_event(
                    &serde_json::json!({"type":"response.output_item.done","item":item})
                        .to_string(),
                )?;
            } else {
                sink.on_event(r#"{"type":"response.output_text.delta","delta":"audit finished"}"#)?;
            }
            sink.on_event(&serde_json::json!({"type":"response.completed","response":{"id":"fixture-response","object":"response","created_at":1,"model":"deepseek-flash","output":output,"status":"completed"}}).to_string())?;
            Ok(())
        }
    }

    pub(super) fn audit_child_runner(
        root: &Path,
        transport: Arc<dyn deepagent_models::transport::HttpTransport>,
    ) -> ChatSubagentRunner {
        let db = Arc::new(Database::open_in_memory().unwrap());
        audit_child_runner_with_db(root, transport, db)
    }

    pub(super) fn audit_child_runner_with_db(
        root: &Path,
        transport: Arc<dyn deepagent_models::transport::HttpTransport>,
        db: Arc<Database>,
    ) -> ChatSubagentRunner {
        let runs = deepagent_persistence::run_store::RunStore::new(&db);
        if runs.get("audit-parent").unwrap().is_none() {
            let session = Session::create(&db, &SystemClock, Some("audit parent")).unwrap();
            runs.create(
                "audit-parent",
                &session.id().to_string(),
                None,
                subagent_now_ms(),
            )
            .unwrap();
        }
        let settings = Arc::new(crate::settings::SettingsService::new(
            db.clone(),
            transport.clone(),
            Arc::new(crate::secret_store::MemorySecretStore::new()),
        ));
        let host = ChatService::new(db.clone(), settings, transport.clone(), root);
        let mut registry = ToolRegistry::new();
        for tool in deepagent_builtins::file_tools(deepagent_builtins::WorkspaceRoot::new(root)) {
            registry.register(tool).unwrap();
        }
        let definitions: std::collections::BTreeMap<_, _> = builtin_runtime_agent_definitions()
            .into_iter()
            .map(|def| (def.type_name.clone(), def))
            .collect();
        let sink: Arc<dyn RuntimeEventSink> = Arc::new(deepagent_runtime::NullEventSink);
        ChatSubagentRunner {
            db: db.clone(),
            parent_run_id: "audit-parent".into(),
            usage_owner: None,
            transcript_root: root.join("transcripts"),
            client: Arc::new(ModelClient::new(
                transport.clone(),
                deepagent_models::ModelConfig::deepseek("audit-placeholder"),
            )),
            model: "deepseek-flash".into(),
            reasoning_effort: ReasoningEffort::None,
            registry: Arc::new(registry),
            root: root.to_path_buf(),
            assigned_worktree: None,
            tool_search_mode: deepagent_builtins::ToolSearchMode::Disabled,
            tool_search_auto_threshold: 10_000,
            parent_discovered_snapshot: Default::default(),
            agent_definitions: definitions,
            background: Default::default(),
            events: Arc::downgrade(&sink),
            skills: None,
            host,
            access: deepagent_builtins::FsAccess::Workspace,
            local_execution_mode: crate::settings::LocalExecutionMode::Direct,
            bash_full_access: false,
            hooks: Default::default(),
            parent_checkpoint: Default::default(),
            parent_approvals: Default::default(),
        }
    }

    #[cfg(windows)]
    #[tokio::test]
    #[ignore = "audit H-109: SubagentStop payload must satisfy its required input schema"]
    async fn audit_subagent_stop_hook_includes_required_fields() {
        use deepagent_hooks::{HookDefinitions, SystemHookRunner};

        let dir = tempfile::tempdir().unwrap();
        let start_payload_path = dir.path().join("subagent-start.json");
        let stop_payload_path = dir.path().join("subagent-stop.json");
        let capture_command = |path: &Path| {
            let quoted = path.to_string_lossy().replace('\'', "''");
            format!(
                "$payload = [Console]::In.ReadToEnd(); [System.IO.File]::WriteAllText('{quoted}', $payload)"
            )
        };
        let definitions = HookDefinitions::parse(
            &serde_json::json!({
                "hooks": {
                    "SubagentStart": [{ "hooks": [{
                        "type": "command", "shell": "powershell", "timeout": 5,
                        "command": capture_command(&start_payload_path)
                    }] }],
                    "SubagentStop": [{ "hooks": [{
                        "type": "command", "shell": "powershell", "timeout": 5,
                        "command": capture_command(&stop_payload_path)
                    }] }]
                }
            })
            .to_string(),
        )
        .unwrap();
        let mut hooks = HookRegistry::new();
        assert_eq!(
            definitions.register_into(&mut hooks, Arc::new(SystemHookRunner)),
            2
        );

        let transport = Arc::new(AuditResumeTransport::default());
        let runner = audit_child_runner(dir.path(), transport);
        assert!(runner.hooks.set(Arc::new(hooks)).is_ok());
        runner
            .run_inner(
                deepagent_builtins::SubagentRequest {
                    description: "audit subagent hook payload".into(),
                    prompt: "Return a short result".into(),
                    subagent_type: Some("review".into()),
                    allowed_tools: Vec::new(),
                    model: None,
                    effort: None,
                    skills: Vec::new(),
                    isolation: "shared".into(),
                    fork: false,
                    phase: None,
                    label: None,
                    schema: None,
                },
                None,
            )
            .await
            .unwrap();

        let start_payload: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(start_payload_path).expect("SubagentStart hook ran"),
        )
        .unwrap();
        let stop_payload: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(stop_payload_path).expect("SubagentStop hook ran"),
        )
        .unwrap();
        eprintln!("SubagentStart payload: {start_payload}");
        eprintln!("SubagentStop payload: {stop_payload}");
        assert!(start_payload["agent_id"].is_string());
        assert!(start_payload["agent_type"].is_string());
        assert!(
            stop_payload["stop_hook_active"].is_boolean()
                && stop_payload["agent_id"].is_string()
                && stop_payload["agent_transcript_path"].is_string()
                && stop_payload["agent_type"].is_string(),
            "SubagentStop payload misses required schema fields: {stop_payload}"
        );
    }

    #[tokio::test]
    async fn h104_real_child_lifecycle_uses_the_parent_owned_native_input() {
        struct Capture(
            std::sync::Mutex<Vec<(HookPoint, String)>>,
            deepagent_core::id::SessionId,
        );
        #[async_trait::async_trait]
        impl deepagent_hooks::Hook for Capture {
            fn name(&self) -> &str {
                "h104_child_input"
            }
            async fn run(&self, ctx: &HookContext) -> Result<HookOutcome> {
                assert_eq!(ctx.session_id, self.1);
                let base = ctx.base_input.as_ref().unwrap();
                let events: Vec<deepagent_core::event::Event> =
                    serde_json::from_str(&std::fs::read_to_string(&base.transcript_path).unwrap())
                        .unwrap();
                assert!(!events.is_empty());
                assert!(events.iter().all(|event| event.session_id == self.1));
                self.0
                    .lock()
                    .unwrap()
                    .push((ctx.point, base.transcript_path.clone()));
                Ok(HookOutcome::Continue)
            }
        }
        let directory = tempfile::tempdir().unwrap();
        let runner =
            audit_child_runner(directory.path(), Arc::new(AuditResumeTransport::default()));
        let capture = Arc::new(Capture(
            Default::default(),
            runner.parent_session_id().unwrap(),
        ));
        let scope = crate::hook_input::HookInputScope::new(runner.db.clone(), runner.root.clone());
        let mut hooks = HookRegistry::new();
        for point in [HookPoint::SubagentStart, HookPoint::SubagentStop] {
            hooks.register(point, scope.wrap(capture.clone()));
        }
        runner
            .hooks
            .set(Arc::new(hooks))
            .unwrap_or_else(|_| panic!("one registry"));
        runner.run_inner(serde_json::from_value(serde_json::json!({
            "description":"H104 synthetic child", "prompt":"Return a short result", "isolation":"shared"
        })).unwrap(), None).await.unwrap();
        let records = capture.0.lock().unwrap();
        assert_eq!(
            records.iter().map(|(point, _)| *point).collect::<Vec<_>>(),
            [HookPoint::SubagentStart, HookPoint::SubagentStop]
        );
        assert!(records
            .iter()
            .all(|(_, path)| !std::path::Path::new(path).exists()));
    }

    #[cfg(windows)]
    #[tokio::test]
    #[ignore = "audit H-112: worktree lifecycle hook payloads must satisfy their event schemas"]
    async fn audit_worktree_hook_payloads_include_required_fields() {
        use deepagent_hooks::{HookData, HookDefinitions, HookPoint, SystemHookRunner};

        let dir = tempfile::tempdir().unwrap();
        let create_path = dir.path().join("worktree-create.json");
        let remove_path = dir.path().join("worktree-remove.json");
        let cwd_path = dir.path().join("cwd-changed.json");
        let capture_command = |path: &Path| {
            let quoted = path.to_string_lossy().replace('\'', "''");
            format!(
                "$payload = [Console]::In.ReadToEnd(); [System.IO.File]::WriteAllText('{quoted}', $payload)"
            )
        };
        let definitions = HookDefinitions::parse(
            &serde_json::json!({
                "hooks": {
                    "WorktreeCreate": [{ "hooks": [{
                        "type": "command", "shell": "powershell", "timeout": 5,
                        "command": capture_command(&create_path)
                    }] }],
                    "WorktreeRemove": [{ "hooks": [{
                        "type": "command", "shell": "powershell", "timeout": 5,
                        "command": capture_command(&remove_path)
                    }] }],
                    "CwdChanged": [{ "hooks": [{
                        "type": "command", "shell": "powershell", "timeout": 5,
                        "command": capture_command(&cwd_path)
                    }] }]
                }
            })
            .to_string(),
        )
        .unwrap();
        let mut hooks = HookRegistry::new();
        assert_eq!(
            definitions.register_into(&mut hooks, Arc::new(SystemHookRunner)),
            3
        );

        let runner = audit_child_runner(dir.path(), Arc::new(AuditResumeTransport::default()));
        assert!(runner.hooks.set(Arc::new(hooks)).is_ok());
        for (point, path) in [
            (HookPoint::WorktreeCreate, "planned-worktree"),
            (HookPoint::WorktreeRemove, "existing-worktree"),
            (HookPoint::CwdChanged, "child-worktree"),
        ] {
            runner
                .dispatch_child_hook(
                    point,
                    HookData::Path {
                        path: path.to_string(),
                    },
                )
                .await
                .unwrap();
        }

        let create: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(create_path).unwrap()).unwrap();
        let remove: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(remove_path).unwrap()).unwrap();
        let cwd: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(cwd_path).unwrap()).unwrap();
        eprintln!("WorktreeCreate payload: {create}");
        eprintln!("WorktreeRemove payload: {remove}");
        eprintln!("CwdChanged payload: {cwd}");
        assert!(
            create["name"].is_string()
                && remove["worktree_path"].is_string()
                && cwd["old_cwd"].is_string()
                && cwd["new_cwd"].is_string(),
            "worktree lifecycle payloads miss required event fields: create={create}, remove={remove}, cwd={cwd}"
        );
    }

    #[cfg(windows)]
    #[tokio::test]
    #[ignore = "audit H-114: Notification hook payload must include its required notification type"]
    async fn audit_background_subagent_notification_includes_required_fields() {
        use deepagent_hooks::{HookDefinitions, SystemHookRunner};

        let dir = tempfile::tempdir().unwrap();
        let payload_path = dir.path().join("notification.json");
        let quoted = payload_path.to_string_lossy().replace('\'', "''");
        let definitions = HookDefinitions::parse(
            &serde_json::json!({
                "hooks": {
                    "Notification": [{ "hooks": [{
                        "type": "command", "shell": "powershell", "timeout": 5,
                        "command": format!(
                            "$payload = [Console]::In.ReadToEnd(); [System.IO.File]::WriteAllText('{quoted}', $payload)"
                        )
                    }] }]
                }
            })
            .to_string(),
        )
        .unwrap();
        let mut hooks = HookRegistry::new();
        assert_eq!(
            definitions.register_into(&mut hooks, Arc::new(SystemHookRunner)),
            1
        );

        let runner = audit_child_runner(dir.path(), Arc::new(AuditResumeTransport::default()));
        assert!(runner.hooks.set(Arc::new(hooks)).is_ok());
        let request = deepagent_builtins::SubagentRequest {
            description: "audit background notification".into(),
            prompt: "Return a short result".into(),
            subagent_type: Some("review".into()),
            allowed_tools: Vec::new(),
            model: None,
            effort: None,
            skills: Vec::new(),
            isolation: "shared".into(),
            fork: false,
            phase: None,
            label: None,
            schema: None,
        };
        let subagent_id = "sub_audit_notification";
        assert!(runner
            .begin_tracking(subagent_id, &request, true, None)
            .unwrap());
        runner
            .run_tracked(
                subagent_id.to_string(),
                request.into(),
                Some(Arc::new(AtomicBool::new(false))),
                None,
                true,
                true,
            )
            .await
            .unwrap();

        let payload: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(payload_path).unwrap()).unwrap();
        eprintln!("Notification payload: {payload}");
        assert!(
            payload["message"].is_string() && payload["notification_type"].is_string(),
            "Notification payload misses required schema fields: {payload}"
        );
    }

    #[derive(Default)]
    struct AuditPendingChildTransport {
        entered: tokio::sync::Notify,
    }

    #[async_trait::async_trait]
    impl deepagent_models::transport::HttpTransport for AuditPendingChildTransport {
        async fn stream(
            &self,
            _request: deepagent_models::transport::TransportRequest,
            _sink: &mut dyn deepagent_models::transport::EventSink,
        ) -> Result<()> {
            self.entered.notify_one();
            std::future::pending().await
        }

        async fn stream_cancelled(
            &self,
            request: deepagent_models::transport::TransportRequest,
            sink: &mut dyn deepagent_models::transport::EventSink,
            cancel: Arc<AtomicBool>,
        ) -> Result<()> {
            tokio::select! {
                result = self.stream(request, sink) => result,
                _ = async {
                    while !cancel.load(std::sync::atomic::Ordering::Acquire) {
                        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                    }
                } => Err(CoreError::other("audit child cancelled")),
            }
        }
    }

    fn h215_child_request() -> deepagent_builtins::SubagentRequest {
        deepagent_builtins::SubagentRequest {
            description: "synthetic archive descendant".into(),
            prompt: "Return a short result".into(),
            subagent_type: Some("review".into()),
            allowed_tools: Vec::new(),
            model: None,
            effort: None,
            skills: Vec::new(),
            isolation: "shared".into(),
            fork: false,
            phase: None,
            label: None,
            schema: None,
        }
    }

    #[tokio::test]
    async fn h215_archived_parent_rejects_child_before_model_work() {
        let root = tempfile::tempdir().unwrap();
        let transport = Arc::new(AuditResumeTransport::default());
        let runner = audit_child_runner(root.path(), transport.clone());
        let id = runner.parent_session_id().unwrap().to_string();
        crate::ArchiveService::new(runner.db.clone())
            .archive_session(&id)
            .unwrap();
        assert!(runner.run_inner(h215_child_request(), None).await.is_err());
        assert!(transport.requests.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn h215_live_background_descendant_excludes_origin_archive_until_its_owner_exits() {
        let root = tempfile::tempdir().unwrap();
        let transport = Arc::new(AuditPendingChildTransport::default());
        let runner = audit_child_runner(root.path(), transport.clone());
        let archive = crate::ArchiveService::new(runner.db.clone());
        let id = runner.parent_session_id().unwrap().to_string();
        let request = h215_child_request();
        assert!(runner
            .begin_tracking("sub_h215_background", &request, true, None)
            .unwrap());
        let task = tokio::spawn(async move {
            runner
                .run_tracked(
                    "sub_h215_background".into(),
                    request.into(),
                    None,
                    None,
                    true,
                    true,
                )
                .await
        });
        transport.entered.notified().await;
        let result = archive.archive_session(&id);
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert!(
            result.is_err(),
            "a child using a separate transcript DB still belongs to the durable origin"
        );
        assert!(!archive.is_archived(&id).unwrap());
        assert!(archive.archive_session(&id).unwrap());
    }

    struct AuditDelegateAgent;

    #[async_trait::async_trait]
    impl deepagent_runtime::Agent for AuditDelegateAgent {
        async fn think(
            &mut self,
            _step: usize,
            _last: &[deepagent_runtime::Observation],
        ) -> Result<deepagent_runtime::AgentDecision> {
            Ok(deepagent_runtime::AgentDecision::CallTool(
                deepagent_tools::ToolInvocation {
                    id: Some("audit-delegate".into()),
                    name: deepagent_builtins::TASK_TOOL_NAME.into(),
                    arguments: serde_json::json!({
                        "description": "audit pending child", "prompt": "wait for cancellation",
                    }),
                },
            ))
        }
    }

    #[tokio::test]
    async fn audit_parent_deadline_finalizes_foreground_child() {
        let mut observations = Vec::new();
        for user_cancel in [true, false] {
            let root = tempfile::tempdir().unwrap();
            let transport = Arc::new(AuditPendingChildTransport::default());
            let runner = audit_child_runner(root.path(), transport.clone());
            let db = runner.db.clone();
            let parent = deepagent_persistence::run_store::RunStore::new(&db)
                .get("audit-parent")
                .unwrap()
                .unwrap();
            let mut session =
                Session::recover(&db, &SystemClock, parent.session_id.parse().unwrap()).unwrap();
            let task = session.create_task("audit delegation").unwrap();
            let mut registry = ToolRegistry::new();
            registry
                .register(Arc::new(deepagent_builtins::TaskTool::new(
                    runner.clone(),
                    Vec::<String>::new(),
                )))
                .unwrap();
            let config = deepagent_runtime::RuntimeConfig {
                task_timeout: Some(std::time::Duration::from_secs(1)),
                ..Default::default()
            };
            let kernel = deepagent_runtime::AgentKernel::<SystemClock>::new(
                db.clone(),
                &registry,
                Default::default(),
                config,
                "audit-parent",
            );
            let control = kernel.handle();
            let mut agent = AuditDelegateAgent;
            let (terminal, ()) = tokio::join!(
                kernel.start(deepagent_runtime::RunRequest::new(
                    &mut session,
                    task,
                    &mut agent
                )),
                async {
                    tokio::time::timeout(
                        std::time::Duration::from_secs(2),
                        transport.entered.notified(),
                    )
                    .await
                    .expect("the actual foreground child must enter ModelClient before stopping");
                    if user_cancel {
                        control.cancel();
                    }
                }
            );
            let terminal = terminal.unwrap();
            assert_eq!(
                terminal.kind,
                if user_cancel {
                    deepagent_runtime::TerminalKind::Cancelled
                } else {
                    deepagent_runtime::TerminalKind::BudgetExceeded
                }
            );
            let children = deepagent_persistence::subagent_store::SubagentRunStore::new(&db)
                .list_for_parent("audit-parent")
                .unwrap();
            assert_eq!(children.len(), 1);
            let child = &children[0];
            let events = deepagent_persistence::run_store::RunStore::new(&db)
                .events_after("audit-parent", None)
                .unwrap();
            let cleanup_error = deepagent_builtins::SubagentRunner::cleanup(&runner, &child.id)
                .await
                .err()
                .map(|error| error.to_string());
            observations.push(serde_json::json!({
                "user_cancel": user_cancel,
                "parent_kind": terminal.kind.label(),
                "root_cancelled": control.is_cancelled(),
                "child_state": child.state,
                "child_finished": child.finished_at.is_some(),
                "transcript_exists": child.transcript_path.as_ref().is_some_and(|p| Path::new(p).exists()),
                "child_terminal_events": events.iter().filter(|e| matches!(e.event_type.as_str(), "subagent_cancelled" | "subagent_completed")).count(),
                "cleanup_error": cleanup_error,
            }));
        }
        eprintln!("{}", serde_json::to_string_pretty(&observations).unwrap());
        assert!(
            observations.iter().all(|o| {
                o["root_cancelled"] == true
                    && o["child_state"] == "cancelled"
                    && o["child_finished"] == true
                    && o["transcript_exists"] == true
                    && o["child_terminal_events"] == 1
                    && o["cleanup_error"].is_null()
            }),
            "a terminal parent must not leave its dropped foreground child running"
        );
    }

    #[tokio::test]
    // Regression for H-19: profile/allowlist restrictions must constrain the
    // registry used by RuntimeEngine, not only the advertised schemas.
    async fn audit_read_only_subagent_rejects_tools_hidden_by_its_profile() {
        let mut unexpected_writes = Vec::new();
        for scope in ["explore", "plan", "request_allowlist", "profile_denylist"] {
            let dir = tempfile::tempdir().unwrap();
            let transport = Arc::new(AuditChildWriteTransport::default());
            let mut runner = audit_child_runner(dir.path(), transport.clone());
            let profile_name = if scope == "profile_denylist" {
                let def = profile(&[], &["Write"]).unwrap();
                runner.agent_definitions.insert(def.type_name.clone(), def);
                Some("t".into())
            } else if scope == "request_allowlist" {
                None
            } else {
                Some(scope.into())
            };
            let request = deepagent_builtins::SubagentRequest {
                description: format!("audit {scope}"),
                prompt: "Inspect without modifying files".into(),
                subagent_type: profile_name,
                allowed_tools: if scope == "request_allowlist" {
                    vec!["Read".into()]
                } else {
                    vec![]
                },
                model: None,
                effort: None,
                skills: vec![],
                isolation: "shared".into(),
                fork: false,
                phase: None,
                label: None,
                schema: None,
            };
            let outcome = runner.run_inner(request, None).await;
            let seen = transport.tools_seen.lock().unwrap();
            assert!(
                !seen.is_empty(),
                "child never called the model: {outcome:?}"
            );
            assert!(
                seen.iter()
                    .all(|tools| !tools.iter().any(|name| name == "write_file")),
                "fixture requires write_file to be absent from every request schema"
            );
            let wrote = std::fs::read_to_string(dir.path().join("audit-child.txt")).ok();
            eprintln!("audit child dispatch: scope={scope}, wire_tools={seen:?}, outcome={outcome:?}, written={wrote:?}");
            unexpected_writes.push(wrote.is_some());
        }
        // Production file tool + engine ran; all writes stay inside TempDir and are removed on drop.
        assert!(
            unexpected_writes.iter().all(|wrote| !wrote),
            "child executed tools excluded by declared restrictions: {unexpected_writes:?}"
        );
    }

    #[tokio::test]
    async fn unrestricted_subagent_keeps_declared_write_tool() {
        let dir = tempfile::tempdir().unwrap();
        let transport = Arc::new(AuditChildWriteTransport::default());
        let runner = audit_child_runner(dir.path(), transport.clone());
        let outcome = runner
            .run_inner(
                deepagent_builtins::SubagentRequest {
                    description: "audit unrestricted child".into(),
                    prompt: "Create the requested file".into(),
                    subagent_type: None,
                    allowed_tools: Vec::new(),
                    model: None,
                    effort: None,
                    skills: Vec::new(),
                    isolation: "shared".into(),
                    fork: false,
                    phase: None,
                    label: None,
                    schema: None,
                },
                None,
            )
            .await
            .unwrap();
        let written = std::fs::read_to_string(dir.path().join("audit-child.txt")).unwrap();
        let seen = transport.tools_seen.lock().unwrap();
        assert_eq!(written, "hidden tool executed");
        assert!(seen
            .iter()
            .all(|tools| tools.iter().any(|name| name == "write_file")));
        assert_eq!(outcome.tool_use_count, 1);
    }

    #[derive(Default)]
    struct AuditResumeTransport {
        requests: std::sync::Mutex<Vec<serde_json::Value>>,
    }

    #[tokio::test]
    async fn dag_outputs_reopen_through_existing_child_store_and_cleanup_exact_tree() {
        use deepagent_builtins::SubagentRunner;
        use deepagent_persistence::subagent_store::SubagentRunStore;
        use deepagent_planner::{PlanDag, PlanNode};
        use deepagent_subagents::{DagScheduler, GitWorktrees};

        fn git(root: &Path, args: &[&str]) -> String {
            let output = std::process::Command::new("git")
                .arg("-C")
                .arg(root)
                .args([
                    "-c",
                    "user.name=DAG Fixture",
                    "-c",
                    "user.email=dag@example.invalid",
                    "-c",
                    "commit.gpgsign=false",
                ])
                .args(args)
                .output()
                .expect("Git required");
            assert!(
                output.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8(output.stdout).unwrap().trim().to_string()
        }

        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("repo");
        std::fs::create_dir(&root).unwrap();
        git(&root, &["init"]);
        std::fs::write(root.join("base.txt"), "main remains unchanged").unwrap();
        git(&root, &["add", "."]);
        git(&root, &["commit", "-m", "fixture"]);
        let original_head = git(&root, &["rev-parse", "HEAD"]);
        let db_path = temp.path().join("state.sqlite");
        let transport = Arc::new(AuditResumeTransport::default());
        let report = {
            let db = Arc::new(Database::open(&db_path).unwrap());
            let mut runner = audit_child_runner_with_db(&root, transport.clone(), db.clone());
            runner.transcript_root = temp.path().join("transcripts");
            let provider = GitWorktrees::new(&root, runner.worktree_base());
            let dag = PlanDag::new([
                PlanNode::new("first", "Return a short result"),
                PlanNode::new("next", "Return a short result").depends_on(["first".to_string()]),
            ])
            .unwrap();
            let executor = DagSubagentExecutor::new(runner);
            let report = DagScheduler::new(&executor, &provider)
                .run(&dag)
                .await
                .unwrap();
            assert!(report.all_succeeded);
            let records = SubagentRunStore::new(&db)
                .list_for_parent("audit-parent")
                .unwrap();
            assert_eq!(records.len(), 2);
            for result in report.results.values() {
                let output = result.output.as_ref().unwrap();
                let record = records
                    .iter()
                    .find(|record| record.id == output.worktree.name)
                    .unwrap();
                assert_eq!(
                    record.worktree_path.as_deref(),
                    Some(output.worktree.path.as_str())
                );
                assert_eq!(record.state, "succeeded");
                assert!(Path::new(record.transcript_path.as_ref().unwrap()).is_file());
                assert_eq!(
                    git(
                        &root,
                        &[
                            "rev-parse",
                            &format!("refs/deepagent/dag-output/{}", record.id)
                        ]
                    ),
                    output.revision.as_ref().unwrap().as_str()
                );
            }
            report
        }; // Close all original database owners before reopening.
        let db = Arc::new(Database::open(&db_path).unwrap());
        let mut runner = audit_child_runner_with_db(&root, transport.clone(), db.clone());
        runner.transcript_root = temp.path().join("transcripts");
        let first = report.results["first"].output.as_ref().unwrap();
        let next = report.results["next"].output.as_ref().unwrap();
        let store = SubagentRunStore::new(&db);
        let record = store.get(&first.worktree.name).unwrap().unwrap();
        assert_eq!(
            record.worktree_path.as_deref(),
            Some(first.worktree.path.as_str())
        );
        let replay = deepagent_persistence::run_store::RunStore::new(&db)
            .events_after("audit-parent", None)
            .unwrap();
        for output in [first, next] {
            assert!(replay
                .iter()
                .any(|event| event.event_type == "worktree_created"
                    && event.data["subagent_id"].as_str() == Some(output.worktree.name.as_str())
                    && event.data["path"].as_str() == Some(output.worktree.path.as_str())));
        }
        let request = runner.resume_request(&record, "Continue").unwrap().request;
        let held = temp.path().join("temporarily_unavailable_worktree");
        std::fs::rename(&first.worktree.path, &held).unwrap();
        assert!(runner
            .execution_root(&record.id, &request, true)
            .await
            .is_err());
        std::fs::rename(&held, &first.worktree.path).unwrap();
        let (restored_root, auto_cleanup) = runner
            .execution_root(&record.id, &request, true)
            .await
            .unwrap();
        assert_eq!(restored_root, PathBuf::from(&first.worktree.path));
        assert!(!auto_cleanup);
        runner
            .resume(&record.id, "Continue", Default::default())
            .await
            .unwrap();
        assert_eq!(store.get(&record.id).unwrap().unwrap().resume_count, 1);
        assert!(runner.cleanup(&record.id).await.unwrap());
        assert!(!Path::new(&first.worktree.path).exists());
        assert!(Path::new(&next.worktree.path).is_dir());
        assert!(store
            .get(&record.id)
            .unwrap()
            .unwrap()
            .worktree_path
            .is_none());
        assert!(git(
            &root,
            &[
                "for-each-ref",
                &format!("refs/deepagent/dag-output/{}", record.id)
            ]
        )
        .is_empty());
        assert!(runner.cleanup(&next.worktree.name).await.unwrap());
        let cleaned = store.get(&record.id).unwrap().unwrap();
        assert!(runner
            .resume_request(&cleaned, "Continue")
            .unwrap_err()
            .to_string()
            .contains("explicitly cleaned up"));
        assert_eq!(git(&root, &["rev-parse", "HEAD"]), original_head);
        assert_eq!(
            std::fs::read_to_string(root.join("base.txt")).unwrap(),
            "main remains unchanged"
        );
        assert_eq!(transport.requests.lock().unwrap().len(), 3);
    }

    #[async_trait::async_trait]
    impl deepagent_models::transport::HttpTransport for AuditResumeTransport {
        async fn stream(
            &self,
            request: deepagent_models::transport::TransportRequest,
            sink: &mut dyn deepagent_models::transport::EventSink,
        ) -> Result<()> {
            let sink: &mut dyn deepagent_models::transport::EventSink =
                &mut crate::test_sequence::sink(sink);
            self.requests
                .lock()
                .unwrap()
                .push(serde_json::from_str(&request.body)?);
            sink.on_event(
                &serde_json::json!({"type":"response.output_text.delta",
                "delta":r#"{"audit_resume_value":"kept"}"#})
                .to_string(),
            )?;
            sink.on_event(r#"{"type":"response.completed","response":{"id":"fixture-response","object":"response","created_at":1,"model":"deepseek-flash","output":[],"status":"completed"}}"#)?;
            Ok(())
        }
    }

    #[tokio::test]
    async fn audit_child_resume_preserves_schema_phase_and_label() {
        use deepagent_builtins::SubagentRunner;
        use deepagent_persistence::subagent_store::SubagentRunStore;

        let dir = tempfile::tempdir().unwrap();
        let transport = Arc::new(AuditResumeTransport::default());
        let runner = audit_child_runner(dir.path(), transport.clone());
        let original = deepagent_builtins::SubagentRequest {
            description: "audit resume".into(),
            prompt: "Return the required JSON value".into(),
            subagent_type: None,
            allowed_tools: vec!["Read".into()],
            model: None,
            effort: None,
            skills: vec![],
            isolation: "shared".into(),
            fork: false,
            phase: Some("audit-contract-phase".into()),
            label: Some("audit-contract-label".into()),
            schema: Some(serde_json::json!({"type":"object","properties":{
                "audit_resume_value":{"type":"string"}},"required":["audit_resume_value"]})),
        };
        runner.run_inner(original.clone(), None).await.unwrap();
        let store = SubagentRunStore::new(&runner.db);
        let record = store
            .list_for_parent(&runner.parent_run_id)
            .unwrap()
            .pop()
            .unwrap();
        assert_eq!(record.state, "succeeded");
        let restored = runner
            .resume_request(&record, "Continue with the same contract")
            .unwrap()
            .request;
        runner
            .resume(
                &record.id,
                "Continue with the same contract",
                Default::default(),
            )
            .await
            .unwrap();
        let after_first = store.get(&record.id).unwrap().unwrap();
        assert_eq!(after_first.state, "succeeded");
        assert_eq!(after_first.resume_count, 1);
        runner
            .resume(
                &record.id,
                "Continue again with the same contract",
                Default::default(),
            )
            .await
            .unwrap();
        let after = store.get(&record.id).unwrap().unwrap();
        assert_eq!(after.state, "succeeded");
        assert_eq!(after.resume_count, 2);
        let requests = transport.requests.lock().unwrap();
        assert_eq!(requests.len(), 3);
        let runtime_context = |request: &serde_json::Value| {
            request["input"]
                .as_array()
                .unwrap()
                .iter()
                .rev()
                .find_map(|item| {
                    let item = serde_json::from_value(item.clone()).ok()?;
                    deepagent_core::runtime_snapshot::RuntimeSnapshot::from_item(&item)
                        .map(|snapshot| snapshot.context)
                })
                .expect("sub-agent runtime snapshot is in Responses input")
        };
        let initial = runtime_context(&requests[0]);
        let resumed = runtime_context(&requests[1]);
        let resumed_again = runtime_context(&requests[2]);
        assert!(
            initial.contains("audit-contract-phase")
                && initial.contains("audit-contract-label")
                && initial.contains("audit_resume_value"),
            "original contract must be sent"
        );
        eprintln!("audit child resume: restored_phase={:?}, restored_label={:?}, restored_schema={:?}, resumed_wire_phase={}, resumed_wire_label={}, resumed_wire_schema={}, resume_count={}",
            restored.phase, restored.label, restored.schema,
            resumed.contains("audit-contract-phase"), resumed.contains("audit-contract-label"),
            resumed.contains("audit_resume_value"), after.resume_count);
        assert_eq!(
            (restored.phase, restored.label, restored.schema),
            (original.phase, original.label, original.schema),
            "resume lost the original child contract after durable tracking"
        );
        assert!(
            resumed_again.contains("audit-contract-phase")
                && resumed_again.contains("audit-contract-label")
                && resumed_again.contains("audit_resume_value"),
            "second resume lost the original child contract"
        );
    }

    #[test]
    fn resume_request_rejects_legacy_transcript_without_context() {
        use deepagent_persistence::subagent_store::SubagentRunRecord;

        let dir = tempfile::tempdir().unwrap();
        let runner = audit_child_runner(dir.path(), Arc::new(AuditResumeTransport::default()));
        let transcript_path = dir.path().join("legacy-child.json");
        std::fs::write(
            &transcript_path,
            serde_json::to_vec(&serde_json::json!({
                "request": {
                    "description": "legacy child",
                    "prompt": "keep the original task",
                    "subagent_type": "explore",
                    "allowed_tools": ["Read"],
                    "model": "deepseek-audit",
                    "effort": "high",
                    "skills": ["review"],
                    "isolation": "shared"
                }
            }))
            .unwrap(),
        )
        .unwrap();
        let record = SubagentRunRecord {
            id: "legacy-child".into(),
            parent_run_id: runner.parent_run_id.clone(),
            origin_parent_run_id: runner.parent_run_id.clone(),
            state: "succeeded".into(),
            agent_type: "explore".into(),
            transcript_path: Some(transcript_path.to_string_lossy().into_owned()),
            worktree_path: None,
            summary: Some("legacy result".into()),
            created_at: 1,
            updated_at: 2,
            finished_at: Some(2),
            resume_count: 0,
        };

        assert!(runner
            .resume_request(&record, "continue safely")
            .unwrap_err()
            .to_string()
            .contains("NOT_RESUMABLE"));
        // Older request fields/defaults remain readable when accompanied by
        // an explicit complete context, rather than a summary-only transcript.
        let mut value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&transcript_path).unwrap()).unwrap();
        value["subagent_id"] = serde_json::json!(record.id);
        value["context"] = serde_json::json!({"version": 1, "attempt": 0,
            "items": [{"type":"message","role":"user","content":"keep the original task"},
                {"type":"message","role":"assistant","content":"legacy result"}]});
        std::fs::write(&transcript_path, serde_json::to_vec(&value).unwrap()).unwrap();
        let restored = runner
            .resume_request(&record, "continue safely")
            .unwrap()
            .request;
        assert_eq!(restored.description, "legacy child");
        assert_eq!(restored.prompt, "continue safely");
        assert_eq!(restored.subagent_type.as_deref(), Some("explore"));
        assert_eq!(restored.allowed_tools, ["Read"]);
        assert_eq!(restored.model.as_deref(), Some("deepseek-audit"));
        assert_eq!(restored.effort.as_deref(), Some("high"));
        assert_eq!(restored.skills, ["review"]);
        assert_eq!(restored.isolation, "shared");
        assert!(!restored.fork);
        assert_eq!(restored.phase, None);
        assert_eq!(restored.label, None);
        assert_eq!(restored.schema, None);
    }

    #[tokio::test]
    async fn audit_dag_child_rebinds_file_tools_to_assigned_worktree() {
        use deepagent_subagents::{SubAgentContext, SubAgentExecutor, Worktree};

        let temp = tempfile::tempdir().unwrap();
        let parent = temp.path().join("parent");
        let child = temp.path().join("assigned-worktree");
        std::fs::create_dir_all(&parent).unwrap();
        std::fs::create_dir_all(&child).unwrap();
        let transport = Arc::new(AuditChildWriteTransport::default());
        let runner = audit_child_runner(&parent, transport.clone());
        let result = DagSubagentExecutor::new(runner)
            .execute(SubAgentContext {
                node_id: "audit-node".into(),
                goal: "Create the requested file in your working directory".into(),
                role: None,
                phase: None,
                worktree: Worktree {
                    name: "audit-node".into(),
                    path: child.to_string_lossy().to_string(),
                },
                upstream_results: vec![],
            })
            .await
            .unwrap();
        assert!(
            result.ok,
            "child failed before path verification: {result:?}"
        );
        let runtime_contexts = transport.runtime_contexts_seen.lock().unwrap();
        let assigned = format!("Working directory: {}", child.display());
        assert!(
            !runtime_contexts.is_empty() && runtime_contexts.iter().all(|s| s.contains(&assigned)),
            "fixture requires the actual request to name the assigned child directory"
        );
        let parent_output = std::fs::read_to_string(parent.join("audit-child.txt")).ok();
        let child_output = std::fs::read_to_string(child.join("audit-child.txt")).ok();
        eprintln!("audit DAG tool root: child_ok={}, wire_names_child=true, parent_output={parent_output:?}, child_output={child_output:?}", result.ok);
        assert!(
            parent_output.is_none() && child_output.as_deref() == Some("hidden tool executed"),
            "DAG child wrote through the parent-bound tool registry"
        );
    }

    #[test]
    fn tool_filter_drops_disallowed_without_allowlist() {
        // No `tools:` allowlist at all — only `disallowedTools: Bash`.
        let mut schemas = allowlist_profile_tools(&["read_file", "bash", "grep"]);
        let profile = profile(&[], &["Bash"]);
        apply_runtime_agent_tool_filter(&mut schemas, profile.as_ref());
        let names: Vec<&str> = schemas.iter().map(|s| s.function.name.as_str()).collect();
        assert_eq!(names, vec!["read_file", "grep"]);
    }

    #[test]
    fn tool_filter_disallowed_overrides_allowlist() {
        let mut schemas = allowlist_profile_tools(&["read_file", "bash"]);
        let profile = profile(&["Read", "Bash"], &["Bash"]);
        apply_runtime_agent_tool_filter(&mut schemas, profile.as_ref());
        let names: Vec<&str> = schemas.iter().map(|s| s.function.name.as_str()).collect();
        assert_eq!(names, vec!["read_file"], "disallowedTools wins over tools");
    }

    #[test]
    fn tool_filter_none_profile_passes_through() {
        let mut schemas = allowlist_profile_tools(&["bash"]);
        apply_runtime_agent_tool_filter(&mut schemas, None);
        assert_eq!(schemas.len(), 1);
    }

    #[test]
    fn registry_filter_removes_deferred_disallowed_capability_before_search() {
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(DeferredWriteTool)).unwrap();
        let profile = profile(&[], &["Write"]);
        retain_runtime_agent_tools(&mut registry, profile.as_ref(), &[]).unwrap();
        assert!(registry.get("write_file").is_none());
        let discovered: DiscoveredToolSet = Arc::default();
        assert!(register_tool_search_into(
            &mut registry,
            deepagent_builtins::ToolSearchMode::Enabled,
            discovered,
            0,
        )
        .unwrap()
        .is_empty());
        assert!(registry.get("tool_search").is_none());
    }

    #[test]
    fn permission_mode_bypass_grants_everything() {
        let def = profile(&[], &[]).map(|mut p| {
            p.def.permission_mode = Some(deepagent_prompts::PermissionMode::BypassPermissions);
            p
        });
        let granted = permission_set_for_agent(def.as_ref());
        for p in [
            Permission::ReadOnly,
            Permission::WorkspaceWrite,
            Permission::ShellSafe,
            Permission::ShellDangerous,
            Permission::Network,
            Permission::GitPush,
            Permission::Secrets,
            Permission::Sandbox,
            Permission::Subagent,
        ] {
            assert!(granted.contains(p), "bypassPermissions must grant {p:?}");
        }
        assert_eq!(granted.len(), 9);
    }

    #[test]
    fn permission_mode_default_is_developer() {
        assert_eq!(
            permission_set_for_agent(profile(&[], &[]).as_ref()),
            PermissionSet::developer()
        );
        assert_eq!(permission_set_for_agent(None), PermissionSet::developer());
    }

    #[test]
    fn effort_precedence_request_then_file_then_inherit() {
        let mut file_deep = profile(&[], &[]).unwrap();
        file_deep.def.effort = Some(deepagent_prompts::EffortLevel::Max);

        let request_with_effort = deepagent_builtins::SubagentRequest {
            description: "d".into(),
            prompt: "p".into(),
            subagent_type: None,
            allowed_tools: vec![],
            model: None,
            effort: Some("none".into()),
            skills: vec![],
            isolation: "shared".into(),
            fork: false,
            label: None,
            phase: None,
            schema: None,
        };
        assert_eq!(
            subagent_effort_str(Some(&file_deep), &request_with_effort),
            Some("none"),
            "per-call effort wins over file default"
        );
        let request_default = deepagent_builtins::SubagentRequest {
            effort: None,
            ..request_with_effort.clone()
        };
        assert_eq!(
            subagent_effort_str(Some(&file_deep), &request_default),
            Some("max"),
            "file default applies when no per-call effort"
        );
        assert_eq!(
            subagent_effort_str(profile(&[], &[]).as_ref(), &request_default),
            None,
            "no file effort, no request effort -> inherit parent"
        );
    }

    #[test]
    fn subagent_effort_rejects_noncanonical_values() {
        for value in ["none", "low", "high", "max"] {
            assert!(subagent_reasoning_effort(Some(value), ReasoningEffort::None).is_ok());
        }
        for alias in ["minimal", "medium", "xhigh", "simple", "deep"] {
            assert!(subagent_reasoning_effort(Some(alias), ReasoningEffort::None).is_err());
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RuntimeAgentDefinition {
    pub(crate) type_name: String,
    pub(crate) source_label: String,
    pub(crate) def: deepagent_prompts::AgentDef,
}

impl RuntimeAgentDefinition {
    pub(crate) fn task_agent_type(&self) -> deepagent_builtins::TaskAgentType {
        deepagent_builtins::TaskAgentType::new(self.type_name.clone(), self.def.description.clone())
    }
}

pub(crate) fn collect_runtime_agent_definitions(
    local_roots: impl IntoIterator<Item = PathBuf>,
    plugin_projection: Option<&crate::plugin_runtime::PluginRuntimeProjection>,
) -> Vec<RuntimeAgentDefinition> {
    let mut definitions = Vec::new();
    let mut seen_types = std::collections::HashSet::new();
    let mut seen_local_roots = std::collections::HashSet::new();

    for root in local_roots {
        if seen_local_roots.insert(root.clone()) {
            let dir = root.join(".deepagent").join("agents");
            collect_runtime_agent_dir(&dir, None, &mut definitions, &mut seen_types);
        }
    }

    if let Some(projection) = plugin_projection {
        for root in &projection.agent_roots {
            collect_runtime_agent_dir(
                &root.path,
                Some(root.plugin_name.as_str()),
                &mut definitions,
                &mut seen_types,
            );
        }
    }

    // Built-in specialized agents (Claude Code parity: explore / plan). Added
    // LAST with the same dedup set, so a project/plugin agent of the same
    // `type_name` overrides the built-in (CC precedence: built-in < user/
    // project). Read-only by construction: their tool allowlist carries only
    // read/search tools, so file-writing/bash tools are filtered out for the
    // child (our positive-allowlist equivalent of CC's `disallowedTools`).
    for builtin in builtin_runtime_agent_definitions() {
        if seen_types.insert(builtin.type_name.clone()) {
            definitions.push(builtin);
        }
    }

    definitions
}

/// The read-only tool allowlist shared by the built-in `explore` and `plan`
/// agents (internal tool names; `apply_runtime_agent_tool_filter` keeps only
/// these, dropping every write/bash/subagent tool).
const READ_ONLY_AGENT_TOOLS: &[&str] = &["read_file", "glob", "grep", "list_dir"];

/// Built-in specialized sub-agents, aligned with Claude Code's Explore/Plan
/// (`builtInAgents.ts`). Both are strictly read-only. Returned as
/// project-source [`RuntimeAgentDefinition`]s so they flow through the same
/// tool-filter + system-prompt path as user/plugin agents.
pub(crate) fn builtin_runtime_agent_definitions() -> Vec<RuntimeAgentDefinition> {
    let read_only_tools: Vec<String> = READ_ONLY_AGENT_TOOLS
        .iter()
        .map(|name| name.to_string())
        .collect();
    let mut plan_tools = read_only_tools.clone();
    // Plan may record an in-memory todo list (Safe, no workspace write).
    plan_tools.push("todo_write".to_string());

    vec![
        RuntimeAgentDefinition {
            type_name: "explore".to_string(),
            source_label: "built-in".to_string(),
            def: deepagent_prompts::AgentDef {
                name: "explore".to_string(),
                description: "Fast READ-ONLY codebase exploration: find files by glob, search \
                    code by regex, and read files to answer questions about the codebase. \
                    Returns a findings report; makes NO changes."
                    .to_string(),
                tools: read_only_tools,
                model: deepagent_prompts::ModelPref::Inherit,
                color: None,
                body: EXPLORE_AGENT_BODY.to_string(),
                disallowed_tools: None,
                permission_mode: None,
                max_turns: None,
                effort: None,
                skills: None,
                background: None,
                isolation: None,
            },
        },
        RuntimeAgentDefinition {
            type_name: "plan".to_string(),
            source_label: "built-in".to_string(),
            def: deepagent_prompts::AgentDef {
                name: "plan".to_string(),
                description: "READ-ONLY implementation planning: explore the codebase and design \
                    a concrete implementation plan (files to change, sequence, trade-offs). \
                    Makes NO changes; returns the plan plus the critical files."
                    .to_string(),
                tools: plan_tools,
                model: deepagent_prompts::ModelPref::Inherit,
                color: None,
                body: PLAN_AGENT_BODY.to_string(),
                disallowed_tools: None,
                permission_mode: None,
                max_turns: None,
                effort: None,
                skills: None,
                background: None,
                isolation: None,
            },
        },
    ]
}

/// Read-only exploration prompt (Claude Code `exploreAgent.ts` parity, DeepSeek
/// wording). Advisory-but-firm read-only contract; the tool allowlist enforces
/// it structurally.
const EXPLORE_AGENT_BODY: &str = "You are a fast, thorough codebase exploration specialist.\n\n\
=== READ-ONLY MODE ===\n\
This is a READ-ONLY task. You do not have write, edit, or shell tools — only \
read_file, glob, grep, and list_dir. Do not attempt to create, modify, move, or \
delete files; such attempts will simply fail.\n\n\
Your strengths: quickly finding files by glob pattern, searching code with regex, \
and reading files to answer questions. Prefer several parallel read/search calls \
over long sequential probing. Adapt breadth to the requested thoroughness.\n\n\
Return your findings directly as your final message (file paths with `path:line` \
references, concise summaries) — the calling agent sees only that message.";

/// Read-only planning prompt (Claude Code `planAgent.ts` parity, DeepSeek
/// wording).
const PLAN_AGENT_BODY: &str = "You are a software architect and planning specialist.\n\n\
=== READ-ONLY MODE ===\n\
This is a READ-ONLY planning task. You have only read/search tools (and an \
in-memory todo list); you cannot and must not modify the workspace.\n\n\
Process: (1) understand the requirements; (2) explore thoroughly — read the files \
you are given, find existing patterns/conventions, trace relevant code paths; \
(3) design one concrete approach and commit to it; (4) detail a phased, \
step-by-step implementation plan with dependencies and trade-offs.\n\n\
End your response with a `Critical Files for Implementation` section listing the \
3-5 files most central to the plan. Return everything as your final message.";

fn collect_runtime_agent_dir(
    dir: &std::path::Path,
    plugin_name: Option<&str>,
    definitions: &mut Vec<RuntimeAgentDefinition>,
    seen_types: &mut std::collections::HashSet<String>,
) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut paths: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|e| e.to_str()) == Some("md"))
        .collect();
    paths.sort();

    for path in paths {
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Some(def) = deepagent_prompts::AgentDef::parse(&content) else {
            continue;
        };
        let type_name = runtime_agent_type_name(plugin_name, &def.name);
        if !seen_types.insert(type_name.clone()) {
            continue;
        }
        let source_label = plugin_name
            .map(|name| format!("plugin:{name}"))
            .unwrap_or_else(|| "project".to_string());
        definitions.push(RuntimeAgentDefinition {
            type_name,
            source_label,
            def,
        });
    }
}

fn runtime_agent_type_name(plugin_name: Option<&str>, agent_name: &str) -> String {
    match plugin_name {
        Some(plugin) => format!("{plugin}:{agent_name}"),
        None => agent_name.to_string(),
    }
}

pub(crate) fn subagent_system_prompt(
    root: &std::path::Path,
    agent_profile: Option<&RuntimeAgentDefinition>,
    preloaded_skills: &str,
    request: &deepagent_builtins::SubagentRequest,
) -> String {
    let mut system = format!(
        "{base}{boundary}",
        base = crate::system_prompt::system_prompt_base(),
        boundary = SYSTEM_PROMPT_DYNAMIC_BOUNDARY,
    );

    // Inject frontmatter metadata (phase, label, schema) if present
    let has_frontmatter =
        request.phase.is_some() || request.label.is_some() || request.schema.is_some();

    if has_frontmatter {
        system.push_str("---\n");
        if let Some(phase) = &request.phase {
            system.push_str("phase: ");
            system.push_str(phase);
            system.push('\n');
        }
        if let Some(label) = &request.label {
            system.push_str("label: ");
            system.push_str(label);
            system.push('\n');
        }
        if let Some(schema) = &request.schema {
            system.push_str("schema: ");
            system.push_str(&serde_json::to_string(schema).unwrap_or_default());
            system.push('\n');
        }
        system.push_str("---\n\n");
    }

    if let Some(agent) = agent_profile {
        system.push_str("# Sub-agent identity\n");
        system.push_str("- Agent type: ");
        system.push_str(&agent.type_name);
        system.push('\n');
        system.push_str("- Source: ");
        system.push_str(&agent.source_label);
        system.push('\n');
        system.push_str("- Description: ");
        system.push_str(&agent.def.description);
        system.push('\n');
        if !agent.def.tools.is_empty() {
            system.push_str("- Declared tools: ");
            system.push_str(&agent.def.tools.join(", "));
            system.push('\n');
        }
        if let Some(model) = agent.def.model.name() {
            system.push_str("- Preferred model: ");
            system.push_str(model);
            system.push_str(" (host may still use the session model)\n");
        }
        let body = agent.def.body.trim();
        if !body.is_empty() {
            system.push('\n');
            system.push_str(body);
            system.push_str("\n\n");
        }
    }

    if !preloaded_skills.is_empty() {
        system.push_str(preloaded_skills);
        system.push('\n');
    }

    system.push_str("# Sub-agent task\n");
    let task_instruction = if request.schema.is_some() {
        "You are a focused sub-agent. Do exactly the delegated task and return a complete, \
         self-contained final answer - the calling agent sees only your final message, not \
         your intermediate steps. YOUR RESPONSE MUST BE VALID JSON MATCHING THE SCHEMA ABOVE.\n- Working directory: "
    } else {
        "You are a focused sub-agent. Do exactly the delegated task and return a complete, \
         self-contained final answer - the calling agent sees only your final message, not \
         your intermediate steps.\n- Working directory: "
    };
    system.push_str(task_instruction);
    system.push_str(&root.display().to_string());
    system
}

pub(crate) fn apply_runtime_agent_tool_filter(
    tools: &mut Vec<ToolSchema>,
    agent_profile: Option<&RuntimeAgentDefinition>,
) {
    apply_runtime_agent_tool_filter_with_request(tools, agent_profile, &[]);
}

fn apply_runtime_agent_tool_filter_with_request(
    tools: &mut Vec<ToolSchema>,
    agent_profile: Option<&RuntimeAgentDefinition>,
    request_allowed_tools: &[String],
) {
    let Some(agent) = agent_profile else {
        if let Some(allowlist) = runtime_agent_tool_allowlist(request_allowed_tools) {
            tools.retain(|tool| allowlist.contains(&tool.function.name));
        }
        return;
    };
    let request_allowlist = runtime_agent_tool_allowlist(request_allowed_tools);
    tools.retain(|tool| {
        runtime_agent_tool_is_allowed(&tool.function.name, agent, request_allowlist.as_ref())
    });
}

/// Apply the profile and per-request allowlist to the actual registry used by
/// `RuntimeEngine`.  Rebuilding through the existing `ToolRegistry` keeps all
/// validation, permissions, hooks and approval logic on the original route;
/// it only removes capabilities that this child is not allowed to resolve.
fn retain_runtime_agent_tools(
    registry: &mut ToolRegistry,
    agent_profile: Option<&RuntimeAgentDefinition>,
    request_allowed_tools: &[String],
) -> Result<()> {
    let request_allowlist = runtime_agent_tool_allowlist(request_allowed_tools);
    let retained: Vec<_> = registry
        .iter_specs()
        .filter(|spec| {
            let request_allowed = request_allowlist
                .as_ref()
                .map_or(true, |set| set.contains(&spec.descriptor.name));
            request_allowed
                && agent_profile.map_or(true, |agent| {
                    runtime_agent_tool_is_allowed(
                        &spec.descriptor.name,
                        agent,
                        request_allowlist.as_ref(),
                    )
                })
        })
        .map(|spec| spec.tool.clone())
        .collect();
    let mut filtered = ToolRegistry::new();
    for tool in retained {
        filtered.register(tool)?;
    }
    *registry = filtered;
    Ok(())
}

fn runtime_agent_tool_is_allowed(
    tool_name: &str,
    agent: &RuntimeAgentDefinition,
    request_allowlist: Option<&std::collections::HashSet<String>>,
) -> bool {
    let profile_allowlist = runtime_agent_tool_allowlist(&agent.def.tools);
    let disallowed =
        runtime_agent_tool_allowlist(agent.def.disallowed_tools.as_deref().unwrap_or_default());
    profile_allowlist
        .as_ref()
        .map_or(true, |set| set.contains(tool_name))
        && request_allowlist.map_or(true, |set| set.contains(tool_name))
        && !disallowed
            .as_ref()
            .is_some_and(|set| set.contains(tool_name))
}

fn runtime_agent_tool_allowlist(
    declared_tools: &[String],
) -> Option<std::collections::HashSet<String>> {
    if declared_tools.is_empty() {
        return None;
    }
    let mut allowlist = std::collections::HashSet::new();
    for raw in declared_tools {
        for name in normalize_runtime_agent_tool_name(raw) {
            allowlist.insert(name);
        }
    }
    Some(allowlist)
}

fn normalize_runtime_agent_tool_name(raw: &str) -> Vec<String> {
    let name = raw
        .trim()
        .split_once('(')
        .map(|(name, _)| name)
        .unwrap_or(raw)
        .trim();
    if name.is_empty() {
        return Vec::new();
    }

    let compact = name
        .chars()
        .filter(|c| *c != '_' && *c != '-' && !c.is_whitespace())
        .collect::<String>()
        .to_ascii_lowercase();
    let mapped = match compact.as_str() {
        "bash" => Some("bash"),
        "glob" => Some("glob"),
        "grep" => Some("grep"),
        "read" => Some("read_file"),
        "write" => Some("write_file"),
        "edit" => Some("edit_file"),
        "multiedit" => Some("multi_edit"),
        "ls" | "list" | "listdir" => Some("list_dir"),
        "todowrite" => Some("todo_write"),
        "tasklist" => Some("task_list"),
        "webfetch" => Some("web_fetch"),
        "websearch" => Some("web_search"),
        "skill" => Some("skill"),
        "askuserquestion" => Some("ask_user_question"),
        "task" => None,
        _ => None,
    };

    let mut names = Vec::new();
    if let Some(mapped) = mapped {
        names.push(mapped.to_string());
    } else {
        names.push(name.to_string());
        let snake = deepagent_builtins::parse_tool_name(name).join("_");
        if !snake.is_empty() && snake != name {
            names.push(snake);
        }
    }
    names.sort();
    names.dedup();
    names
}

/// Sub-agent granted permissions from the agent profile's `permissionMode`.
/// `None`/`Default` → the regular developer set (high-risk calls still bubble to
/// the parent approval channel). `AcceptEdits` keeps the same set but the
/// calling loop skips no risk gate for file writes — expressed below by the
/// approval gate, and `BypassPermissions` grants every permission and replaces
/// any human gate with `AutoApproveGate`.
///
/// Kept behind this one function so the permission model has a single mapping
/// point (`PermissionSet` as defined in `deepagent-tools`; no second permission
/// model — AGENTS.md §5.5).
fn permission_set_for_agent(profile: Option<&RuntimeAgentDefinition>) -> PermissionSet {
    let Some(agent) = profile else {
        return PermissionSet::developer();
    };
    match agent.def.permission_mode {
        Some(deepagent_prompts::PermissionMode::BypassPermissions) => {
            PermissionSet::from_iter_perms([
                Permission::ReadOnly,
                Permission::WorkspaceWrite,
                Permission::ShellSafe,
                Permission::ShellDangerous,
                Permission::Network,
                Permission::GitPush,
                Permission::Secrets,
                Permission::Sandbox,
                Permission::Subagent,
            ])
        }
        Some(_) | None => PermissionSet::developer(),
    }
}

/// Effective sub-agent reasoning effort: per-call value wins, then the agent
/// file's declared value, then the parent's inherited effort.
fn subagent_effort_str<'a>(
    profile: Option<&'a RuntimeAgentDefinition>,
    request: &'a deepagent_builtins::SubagentRequest,
) -> Option<&'a str> {
    request
        .effort
        .as_deref()
        .or_else(|| match profile?.def.effort? {
            deepagent_prompts::EffortLevel::None => Some("none"),
            deepagent_prompts::EffortLevel::Low => Some("low"),
            deepagent_prompts::EffortLevel::High => Some("high"),
            deepagent_prompts::EffortLevel::Max => Some("max"),
        })
}
