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
use deepagent_models::{ModelClient, ThinkingDepth, ToolSchema};
use deepagent_persistence::Database;
use deepagent_runtime::{RuntimeEvent, RuntimeEventSink};
use deepagent_session::Session;
use deepagent_tools::{PermissionSet, ToolRegistry};

use crate::chat_service::ChatService;
use crate::system_context::SYSTEM_PROMPT_DYNAMIC_BOUNDARY;
use crate::tool_manifest::{
    build_visible_tool_schemas, register_tool_search_into, DiscoveredToolSet,
};

/// Runs a sub-agent for the `task` tool: a nested agent loop over a sub-registry
/// (the built-ins minus `task`, so no recursion), on an ephemeral in-memory
/// session, returning only the sub-agent's final message.
#[derive(Clone)]
pub(crate) struct ChatSubagentRunner {
    pub(crate) db: Arc<Database>,
    pub(crate) parent_run_id: String,
    pub(crate) transcript_root: PathBuf,
    pub(crate) client: Arc<ModelClient>,
    pub(crate) model: String,
    pub(crate) thinking_depth: ThinkingDepth,
    pub(crate) registry: Arc<ToolRegistry>,
    pub(crate) root: PathBuf,
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
}

impl ChatSubagentRunner {
    async fn run_inner(
        &self,
        request: deepagent_builtins::SubagentRequest,
        cancel: Option<Arc<AtomicBool>>,
    ) -> Result<String> {
        let subagent_id = format!("sub_{}", deepagent_core::id::EventId::new());
        let persist = self.begin_tracking(&subagent_id, &request, false)?;
        self.run_tracked(subagent_id, request, cancel, persist, false)
            .await
    }

    fn begin_tracking(
        &self,
        subagent_id: &str,
        request: &deepagent_builtins::SubagentRequest,
        background: bool,
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
                    worktree_path: None,
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

    async fn dispatch_child_hook(&self, point: HookPoint, data: HookData) -> Result<HookOutcome> {
        let Some(hooks) = self.hooks.get() else {
            return Ok(HookOutcome::Continue);
        };
        hooks
            .dispatch(&HookContext::new(self.parent_session_id()?, point, data))
            .await
    }

    fn worktree_base(&self) -> PathBuf {
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
            HookOutcome::Continue => {}
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
        request: deepagent_builtins::SubagentRequest,
        cancel: Option<Arc<AtomicBool>>,
        persist: bool,
        background: bool,
    ) -> Result<String> {
        let transcript_path = self.transcript_root.join(format!("{subagent_id}.json"));
        let created_at = subagent_now_ms();
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
            self.run_active(request.clone(), cancel.clone(), &execution_root)
                .await
        };
        let cancelled = cancel
            .as_ref()
            .is_some_and(|flag| flag.load(std::sync::atomic::Ordering::Acquire));
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
                if let Ok(summary) = &mut result {
                    summary.push_str("\n\nIsolated worktree retained at: ");
                    summary.push_str(&path);
                }
            } else if let Err(error) = self.remove_worktree(&subagent_id, &path).await {
                tracing::warn!(%error, subagent_id, "failed to clean up terminal child worktree");
            }
        }
        let summary = match &result {
            Ok(summary) => summary.clone(),
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
        if persist {
            let current_attempt = serde_json::json!({
                "request": {
                    "description": request.description,
                    "prompt": request.prompt,
                    "subagent_type": request.subagent_type,
                    "allowed_tools": request.allowed_tools,
                    "model": request.model,
                    "effort": request.effort,
                    "skills": request.skills,
                    "isolation": request.isolation,
                },
                "state": state,
                "summary": summary,
                "started_at": created_at,
                "finished_at": subagent_now_ms(),
            });
            let mut attempts = std::fs::read(&transcript_path)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
                .and_then(|previous| previous.get("attempts").and_then(|v| v.as_array()).cloned())
                .unwrap_or_default();
            attempts.push(current_attempt.clone());
            let transcript = serde_json::json!({
                "subagent_id": subagent_id,
                "parent_run_id": self.parent_run_id,
                "request": current_attempt["request"].clone(),
                "state": state,
                "summary": summary,
                "created_at": created_at,
                "finished_at": subagent_now_ms(),
                "attempts": attempts,
            });
            if let Err(error) = std::fs::write(
                &transcript_path,
                serde_json::to_vec_pretty(&transcript).unwrap_or_default(),
            ) {
                tracing::warn!(%error, path = %transcript_path.display(), "failed to write subagent transcript");
            }
            if let Err(error) =
                deepagent_persistence::subagent_store::SubagentRunStore::new(&self.db).finish(
                    &subagent_id,
                    state,
                    Some(&bounded_summary),
                    subagent_now_ms(),
                )
            {
                tracing::warn!(%error, subagent_id, "failed to finalize subagent run record");
            }
        }
        let duration_ms = subagent_now_ms().saturating_sub(created_at) as u64;
        if cancelled {
            self.emit_subagent_event(RuntimeEvent::SubagentCancelled {
                id: subagent_id.clone(),
                parent_run_id: self.parent_run_id.clone(),
                duration_ms,
                background,
            });
        } else {
            self.emit_subagent_event(RuntimeEvent::SubagentCompleted {
                id: subagent_id.clone(),
                parent_run_id: self.parent_run_id.clone(),
                state: state.to_string(),
                summary: bounded_summary.clone(),
                duration_ms,
                background,
            });
        }
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
        execution_root: &Path,
    ) -> Result<String> {
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
                sub_registry.replace(spec.tool.clone());
            }
        }
        let _ = register_tool_search_into(
            &mut sub_registry,
            self.tool_search_mode,
            sub_discovered.clone(),
            self.tool_search_auto_threshold,
        );
        let granted = PermissionSet::developer();
        let mut tools: Vec<ToolSchema> = build_visible_tool_schemas(
            &sub_registry,
            &granted,
            self.tool_search_mode,
            &sub_discovered,
        );
        apply_runtime_agent_tool_filter(&mut tools, agent_profile);
        if let Some(allowlist) = runtime_agent_tool_allowlist(&request.allowed_tools) {
            tools.retain(|tool| allowlist.contains(&tool.function.name));
        }
        let preloaded_skills = self.preload_skills(&request.skills)?;
        let system = subagent_system_prompt(execution_root, agent_profile, &preloaded_skills);
        let db = Database::open_in_memory()?;
        let clock = SystemClock;
        let mut session = Session::create(&db, &clock, Some(&request.description))?;
        let task = session.create_task(&request.prompt)?;
        let mut agent = ModelAgent::new(
            self.client.clone(),
            request.model.clone().unwrap_or_else(|| self.model.clone()),
            system,
            &request.prompt,
            tools,
        )
        .with_thinking_depth(subagent_thinking_depth(
            request.effort.as_deref(),
            self.thinking_depth,
        )?);
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
            ..Default::default()
        };
        let mut engine = RuntimeEngine::new(&sub_registry, Default::default(), config);
        if let Some(cancel) = cancel {
            engine = engine.with_cancel(cancel);
        }
        let outcome = engine.run(&mut session, task, &mut agent).await;
        if let Ok(evidence) = checkpoint.mutation_evidence() {
            if let Some(parent) = self.parent_checkpoint.get() {
                parent.record_external_evidence(evidence);
            }
        }
        match outcome? {
            RunOutcome::Completed(msg) => Ok(msg),
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
    ) -> Result<deepagent_builtins::SubagentRequest> {
        let transcript_path = record.transcript_path.as_deref().ok_or_else(|| {
            CoreError::not_found(format!(
                "sub-agent transcript is unavailable: {}",
                record.id
            ))
        })?;
        let transcript: serde_json::Value =
            serde_json::from_slice(&std::fs::read(transcript_path).map_err(|error| {
                CoreError::Persistence(format!("failed to read sub-agent transcript: {error}"))
            })?)
            .map_err(|error| {
                CoreError::Persistence(format!("invalid sub-agent transcript: {error}"))
            })?;
        let saved = transcript
            .get("request")
            .and_then(serde_json::Value::as_object)
            .ok_or_else(|| CoreError::Persistence("sub-agent transcript has no request".into()))?;
        let saved_prompt = saved
            .get("prompt")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        let previous_summary = record.summary.as_deref().unwrap_or_default();
        let read_array = |key: &str| {
            saved
                .get(key)
                .and_then(serde_json::Value::as_array)
                .map(|values| {
                    values
                        .iter()
                        .filter_map(serde_json::Value::as_str)
                        .map(ToOwned::to_owned)
                        .collect()
                })
                .unwrap_or_default()
        };
        Ok(deepagent_builtins::SubagentRequest {
            description: saved
                .get("description")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("Resume sub-agent")
                .to_string(),
            prompt: format!(
                "Resume the prior delegated task using the preserved result below.\n\nOriginal task:\n{saved_prompt}\n\nPrevious result:\n{previous_summary}\n\nContinuation request:\n{continuation}"
            ),
            subagent_type: saved
                .get("subagent_type")
                .and_then(serde_json::Value::as_str)
                .map(ToOwned::to_owned)
                .or_else(|| {
                    (record.agent_type != "general").then(|| record.agent_type.clone())
                }),
            allowed_tools: read_array("allowed_tools"),
            model: saved
                .get("model")
                .and_then(serde_json::Value::as_str)
                .map(ToOwned::to_owned),
            effort: saved
                .get("effort")
                .and_then(serde_json::Value::as_str)
                .map(ToOwned::to_owned),
            skills: read_array("skills"),
            isolation: saved
                .get("isolation")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("shared")
                .to_string(),
        })
    }

    fn ensure_resume_lineage(
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
                "cannot resume a sub-agent from another conversation",
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

fn subagent_thinking_depth(
    effort: Option<&str>,
    inherited: ThinkingDepth,
) -> Result<ThinkingDepth> {
    match effort {
        None => Ok(inherited),
        Some("simple") => Ok(ThinkingDepth::Simple),
        Some("medium") => Ok(ThinkingDepth::Medium),
        Some("deep") => Ok(ThinkingDepth::Deep),
        Some(value) => Err(CoreError::invalid(format!(
            "invalid sub-agent effort '{value}'; expected simple, medium, or deep"
        ))),
    }
}

#[async_trait::async_trait]
impl deepagent_builtins::SubagentRunner for ChatSubagentRunner {
    async fn run(&self, request: deepagent_builtins::SubagentRequest) -> Result<String> {
        self.run_inner(request, None).await
    }

    async fn run_controlled(
        &self,
        request: deepagent_builtins::SubagentRequest,
        context: deepagent_tools::ToolExecutionContext,
    ) -> Result<String> {
        self.run_inner(request, Some(context.cancel_flag())).await
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
        let persist = self.begin_tracking(&subagent_id, &request, true)?;
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
        let parent_cancel = context.cancel_flag();
        tokio::spawn(async move {
            let run = runner.run_tracked(
                task_id.clone(),
                request,
                Some(child_cancel.clone()),
                true,
                true,
            );
            tokio::pin!(run);
            tokio::select! {
                _ = wait_for_subagent_cancel(parent_cancel) => {
                    child_cancel.store(true, std::sync::atomic::Ordering::Release);
                    let _ = run.await;
                }
                _ = &mut run => {}
            }
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
        self.ensure_resume_lineage(&record)?;
        Ok(deepagent_builtins::SubagentStatus {
            id: record.id,
            state: record.state,
            summary: record.summary,
            worktree_path: record.worktree_path,
        })
    }

    async fn cancel(&self, id: &str) -> Result<bool> {
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
        self.ensure_resume_lineage(&record)?;
        let request = self.resume_request(&record, prompt)?;
        store.resume(id, &self.parent_run_id, subagent_now_ms())?;
        self.emit_subagent_event(RuntimeEvent::SubagentStarted {
            id: id.to_string(),
            parent_run_id: self.parent_run_id.clone(),
            agent_type: record.agent_type,
            description: request.description.clone(),
            background: false,
        });
        self.run_tracked(
            id.to_string(),
            request,
            Some(context.cancel_flag()),
            true,
            false,
        )
        .await
    }

    async fn cleanup(&self, id: &str) -> Result<bool> {
        let record = deepagent_persistence::subagent_store::SubagentRunStore::new(&self.db)
            .get(id)?
            .ok_or_else(|| CoreError::not_found(format!("sub-agent not found: {id}")))?;
        self.ensure_resume_lineage(&record)?;
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

async fn wait_for_subagent_cancel(cancel: Arc<AtomicBool>) {
    while !cancel.load(std::sync::atomic::Ordering::Acquire) {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
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

    definitions
}

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
) -> String {
    let mut system = format!(
        "{base}{boundary}",
        base = crate::system_prompt::system_prompt_base(),
        boundary = SYSTEM_PROMPT_DYNAMIC_BOUNDARY,
    );

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
    system.push_str(
        "You are a focused sub-agent. Do exactly the delegated task and return a complete, \
         self-contained final answer - the calling agent sees only your final message, not \
         your intermediate steps.\n- Working directory: ",
    );
    system.push_str(&root.display().to_string());
    system
}

pub(crate) fn apply_runtime_agent_tool_filter(
    tools: &mut Vec<ToolSchema>,
    agent_profile: Option<&RuntimeAgentDefinition>,
) {
    let Some(agent) = agent_profile else {
        return;
    };
    let Some(allowlist) = runtime_agent_tool_allowlist(&agent.def.tools) else {
        return;
    };
    tools.retain(|tool| allowlist.contains(&tool.function.name));
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
