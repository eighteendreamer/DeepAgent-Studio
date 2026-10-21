//! The application service: the single entry point the UI calls.
//!
//! [`AppService`] owns the [`Database`] and exposes high-level, DTO-returning
//! operations (list sessions, open a session with its timeline + stats). Tauri
//! commands (or a web handler) are thin wrappers over these methods.

use std::str::FromStr;

use deepagent_core::clock::{Clock, SystemClock};
use deepagent_core::error::{CoreError, Result};
use deepagent_core::id::{SessionId, TaskId};
use deepagent_core::task::TaskState;
use deepagent_observation::{build_timeline, export_transcript, SessionStats, TranscriptFormat};
use deepagent_persistence::checkpoint_store::CheckpointStore;
use deepagent_persistence::cost_store::CostStore;
use deepagent_persistence::event_store::{EventStore, SessionRecord};
use deepagent_persistence::run_control::RunControlStore;
use deepagent_persistence::run_store::RunStore;
use deepagent_persistence::Database;
use deepagent_runtime::{tool_ui_metadata, CheckpointManager};
use deepagent_session::Session;

use crate::commands::{builtin_commands, commands_from_roots, filter_commands};
use crate::diff::{diff_lines, DiffResult};
use crate::dto::{
    AttachmentDto, CommandDto, ConversationMessageDto, ConversationPartDto, ConversationTurnDto,
    ConversationUsageDto, ForkResultDto, RewindResultDto, RunRecoveryDto, SessionDetailDto,
    SessionSearchHitDto, SessionStatsDto, SessionSummaryDto, TimelineEntryDto, TranscriptDto,
};
use crate::host_lock::{self, HostRole};
use crate::{ArchiveService, ProjectService, SessionStateService};

/// Elect this process as the data root's primary host, or mark it standby.
///
/// A lock failure is reported and treated as primary: keeping crash clean-up
/// working is the lesser harm, and the reason stays in the log.
fn claim_host(db: &Database) -> HostRole {
    match host_lock::claim(db.data_root()) {
        Ok(role) => role,
        Err(error) => {
            tracing::warn!(%error, "host election unavailable; assuming primary host");
            HostRole::Primary
        }
    }
}

/// The application service backing the UI.
pub struct AppService {
    db: std::sync::Arc<Database>,
    host: HostRole,
}

impl AppService {
    /// Open the service over a database at `path` (created + migrated if new).
    pub fn open(path: impl AsRef<std::path::Path>) -> Result<Self> {
        let db = std::sync::Arc::new(Database::open(path)?);
        ArchiveService::new(db.clone()).gc_deleted_sessions(30)?;
        Ok(Self {
            host: claim_host(&db),
            db,
        })
    }

    /// Build over an existing database (e.g. in-memory for tests).
    pub fn new(db: Database) -> Self {
        Self::from_shared(std::sync::Arc::new(db))
    }

    /// Build over a shared database handle (so settings + sessions share one DB).
    pub fn from_shared(db: std::sync::Arc<Database>) -> Self {
        if let Err(error) = ArchiveService::new(db.clone()).gc_deleted_sessions(30) {
            tracing::warn!(%error, "failed to collect expired recycled conversations");
        }
        let host = claim_host(&db);
        Self { db, host }
    }

    /// Whether this process owns the data root it opened.
    pub fn host_role(&self) -> &HostRole {
        &self.host
    }

    /// Borrow the database (for callers that also run the runtime against it).
    pub fn database(&self) -> &Database {
        &self.db
    }

    /// The shared database handle (e.g. to build a `SettingsService` on the same DB).
    pub fn shared_database(&self) -> std::sync::Arc<Database> {
        self.db.clone()
    }

    /// Close out kernel runs that were left non-terminal by a previous process
    /// crash or forced app shutdown. This does not resume model execution yet;
    /// it makes the persisted state truthful and prevents stale running tasks
    /// from blocking cancellation/UI state on the next boot.
    ///
    /// A standby host must not run this: its own rows look non-terminal while the
    /// primary host is still executing them, and closing them out would fail a live
    /// run in the other process.
    pub fn recover_unfinished_runs(&self) -> Result<Vec<RunRecoveryDto>> {
        if let HostRole::Standby { holder } = &self.host {
            tracing::warn!(
                %holder,
                "startup recovery skipped: another process owns this data root"
            );
            return Ok(Vec::new());
        }
        let clock = SystemClock;
        let recovery_now = now_millis();
        // Recover the durable control projection before finalizing lifecycle
        // rows. This closes the crash window where `runs` is repaired but a
        // running action, expired approval, or stale execution lease remains
        // visible to the next process.
        let control_recovery = RunControlStore::new(&self.db).recover(recovery_now)?;
        let store = RunStore::new(&self.db);
        let runs = store.unfinished()?;
        let mut recovered = Vec::new();
        for run in runs {
            let reason = format!(
                "run was not terminal when the application started; previous state was {}",
                run.state
            );
            let checkpoint_ids: Vec<String> = CheckpointStore::new(&self.db)
                .list_for_run(&run.id)
                .map(|records| records.into_iter().map(|record| record.id).collect())
                .unwrap_or_default();
            let mut interruption: Option<String> = None;
            if let Ok(session_id) = SessionId::from_str(&run.session_id) {
                if let Ok(events) = EventStore::new(&self.db).load_session(session_id) {
                    interruption = classify_interruption(&events).map(str::to_string);
                }
                if let Ok(mut session) = Session::recover(&self.db, &clock, session_id) {
                    if let Some(task_id) = run
                        .task_id
                        .as_deref()
                        .and_then(|task_id| TaskId::from_str(task_id).ok())
                    {
                        if session
                            .state()
                            .task(task_id)
                            .is_some_and(|task| task.state.is_active())
                        {
                            if let Err(error) = session.transition_task(task_id, TaskState::Failed)
                            {
                                tracing::warn!(
                                    run_id = %run.id,
                                    task_id = %task_id,
                                    error = %error,
                                    "failed to mark recovered task failed"
                                );
                            }
                        }
                    }
                }
            }
            store.append_event(
                &run.id,
                recovery_now,
                "finalizing",
                "completed",
                "run_recovered_after_startup",
                &serde_json::json!({
                    "session_id": run.session_id.clone(),
                    "task_id": run.task_id.clone(),
                    "previous_state": run.state.clone(),
                    "terminal_kind": "failed",
                    "reason": reason,
                    "interruption": interruption,
                    // Phase D: incremental checkpoint commits keep backups
                    // reachable across a crash. Surface them so the UI (or a
                    // later auto-resume) can offer rolling files back to the
                    // pre-run state via the standard rewind path.
                    "checkpoint_ids": checkpoint_ids,
                    "has_file_backups": !checkpoint_ids.is_empty(),
                    "control_recovery": {
                        "failed_running_actions": control_recovery.failed_running_actions,
                        "expired_approvals": control_recovery.expired_approvals,
                        "revoked_leases": control_recovery.revoked_leases,
                    },
                }),
            )?;
            store.finish(&run.id, "failed", Some(&reason), recovery_now)?;
            recovered.push(RunRecoveryDto {
                run_id: run.id,
                session_id: run.session_id,
                task_id: run.task_id,
                previous_state: run.state,
                terminal_kind: "failed".to_string(),
                terminal_reason: reason,
                interruption,
            });
        }
        Ok(recovered)
    }

    /// List all sessions, newest first, as summaries for the sidebar.
    pub fn list_sessions(&self) -> Result<Vec<SessionSummaryDto>> {
        let store = EventStore::new(&self.db);
        let archived = ArchiveService::new(self.db.clone()).archived_ids()?;
        let pinned = SessionStateService::new(self.db.clone()).pinned_ids()?;
        let projects = ProjectService::new(self.db.clone());
        let (registered_projects, project_names) = projects.session_projection()?;
        let records = store.list_sessions()?;
        let mut file_backed_records = Vec::with_capacity(records.len());
        for record in records {
            if store.event_count(record.id)? > 0 {
                file_backed_records.push(record);
            }
        }
        Ok(file_backed_records
            .into_iter()
            .filter(|r| !archived.contains(&r.id.to_string()))
            // V11 Responses cutover clears legacy transcript sessions while
            // retaining their rows for cost foreign keys.
            .filter(|r| !(r.ended_at.is_some() && r.title.is_none()))
            .filter(|r| {
                r.project
                    .as_deref()
                    .map(|path| registered_projects.contains(path))
                    .unwrap_or(true)
            })
            .map(|record| {
                let project = record.project.as_deref().map(|path| {
                    project_names
                        .get(path)
                        .filter(|name| !name.trim().is_empty())
                        .cloned()
                        .unwrap_or_else(|| project_display_name(path))
                });
                let is_pinned = pinned.contains(&record.id.to_string());
                session_summary_dto(record, project, is_pinned)
            })
            .collect())
    }

    /// Search settled conversation bodies while preserving the existing
    /// session-list projection and archive/project visibility rules.
    pub fn search_sessions(
        &self,
        query: &str,
        project: Option<&str>,
        limit: usize,
    ) -> Result<Vec<SessionSearchHitDto>> {
        let store = EventStore::new(&self.db);
        let archived = ArchiveService::new(self.db.clone()).archived_ids()?;
        let pinned = SessionStateService::new(self.db.clone()).pinned_ids()?;
        let projects = ProjectService::new(self.db.clone());
        let (registered_projects, project_names) = projects.session_projection()?;
        let scope = project.map_or(
            deepagent_persistence::event_store::SessionSearchScope::All,
            deepagent_persistence::event_store::SessionSearchScope::Project,
        );
        let hits = store.search(query, scope, limit)?;
        Ok(hits
            .into_iter()
            .filter(|hit| !archived.contains(&hit.session.id.to_string()))
            .filter(|hit| {
                hit.session
                    .project
                    .as_deref()
                    .map(|path| registered_projects.contains(path))
                    .unwrap_or(true)
            })
            .map(|hit| {
                let project = hit.session.project.as_deref().map(|path| {
                    project_names
                        .get(path)
                        .filter(|name| !name.trim().is_empty())
                        .cloned()
                        .unwrap_or_else(|| project_display_name(path))
                });
                let is_pinned = pinned.contains(&hit.session.id.to_string());
                SessionSearchHitDto {
                    session: session_summary_dto(hit.session, project, is_pinned),
                    sequence: hit.sequence,
                    role: hit.role.map(|role| role.as_str().to_owned()),
                    timestamp: hit.timestamp.as_millis(),
                    snippet: hit.snippet,
                }
            })
            .collect())
    }

    /// Open a session: its summary, full timeline, and aggregated stats.
    pub fn session_detail(&self, session_id: &str) -> Result<SessionDetailDto> {
        let id = SessionId::from_str(session_id)
            .map_err(|e| CoreError::invalid(format!("bad session id: {e}")))?;
        let store = EventStore::new(&self.db);
        let projects = ProjectService::new(self.db.clone());
        let pinned = SessionStateService::new(self.db.clone()).pinned_ids()?;
        let record = store
            .get_session(id)?
            .ok_or_else(|| CoreError::not_found(format!("session {session_id}")))?;
        let events = store.load_session(id)?;
        if events.is_empty() {
            return Err(CoreError::not_found(format!("session {session_id}")));
        }

        let timeline = build_timeline(&events)
            .into_iter()
            .map(|t| TimelineEntryDto {
                sequence: t.sequence,
                timestamp: t.timestamp.as_millis(),
                kind: t.kind,
                icon: t.icon.to_string(),
                label: t.label,
                detail: t.detail,
                duration_ms: t.duration_ms,
            })
            .collect();

        let s = SessionStats::from_events(&events);
        let stats = SessionStatsDto {
            event_count: s.event_count,
            messages: s.messages,
            tool_calls: s.tool_calls,
            tool_successes: s.tool_successes,
            tool_failures: s.tool_failures,
            total_tool_ms: s.total_tool_ms,
            tool_success_rate: s.tool_success_rate(),
            duration_ms: s.duration_ms,
        };
        let conversation = self.session_conversation_from_events(session_id, &events)?;

        Ok(SessionDetailDto {
            version: 1,
            project_path: record.project.clone(),
            summary: {
                let project = record.project.as_deref().map(|path| {
                    projects
                        .display_name(path)
                        .unwrap_or_else(|_| project_display_name(path))
                });
                let is_pinned = pinned.contains(&record.id.to_string());
                session_summary_dto(record, project, is_pinned)
            },
            timeline,
            stats,
            conversation,
            context_usage: self.session_context_snapshot(session_id, &events)?,
        })
    }

    /// Resolve a session's persisted project without replaying its event log.
    /// Used by open-target actions that only need the working directory.
    pub fn session_project_path(&self, session_id: &str) -> Result<Option<String>> {
        let id = SessionId::from_str(session_id)
            .map_err(|e| CoreError::invalid(format!("bad session id: {e}")))?;
        match EventStore::new(&self.db).get_session(id)? {
            Some(record) => Ok(record.project),
            None => Err(CoreError::not_found(format!("session {session_id}"))),
        }
    }

    fn session_context_snapshot(
        &self,
        session_id: &str,
        events: &[deepagent_core::event::Event],
    ) -> Result<Option<deepagent_context::ContextUsageSnapshot>> {
        let runs = deepagent_persistence::run_store::RunStore::new(&self.db);
        let Some(run) = runs.list_for_session(session_id)?.into_iter().next() else {
            return Ok(None);
        };
        let saved = runs.events_after(&run.id, None)?;
        let Some(event) = saved
            .iter()
            .rev()
            .find(|event| event.event_type == "context_usage")
        else {
            return Ok(None);
        };
        // A rewind or a Surface edit committed after the snapshot makes that
        // old projection unusable; the next request will fully remeasure it.
        if events
            .last()
            .map_or(true, |last| event.timestamp > last.timestamp.as_millis())
            || events.iter().any(|source| {
                source.timestamp.as_millis() > event.timestamp
                    && matches!(
                        source.payload,
                        deepagent_core::event::EventPayload::ContextSurfacePatched { .. }
                            | deepagent_core::event::EventPayload::ContextSnipped { .. }
                            | deepagent_core::event::EventPayload::ContextCompacted { .. }
                    )
            })
        {
            return Ok(None);
        }
        match serde_json::from_value(
            event
                .data
                .get("snapshot")
                .cloned()
                .unwrap_or(serde_json::Value::Null),
        ) {
            Ok(snapshot) => Ok(Some(snapshot)),
            Err(error) => {
                tracing::warn!(run_id = %run.id, %error, "saved context snapshot cannot be restored");
                Ok(None)
            }
        }
    }

    /// Rename a session's display title.
    pub fn rename_session(&self, session_id: &str, title: &str) -> Result<SessionSummaryDto> {
        let id = SessionId::from_str(session_id)
            .map_err(|e| CoreError::invalid(format!("bad session id: {e}")))?;
        let trimmed = title.trim();
        if trimmed.is_empty() {
            return Err(CoreError::invalid("session title cannot be empty"));
        }

        let store = EventStore::new(&self.db);
        let now = SystemClock.now();
        if !store.rename_session(id, Some(trimmed), now)? {
            return Err(CoreError::not_found(format!("session {session_id}")));
        }

        Ok(self.session_detail(session_id)?.summary)
    }

    /// The command-palette commands, optionally filtered by a fuzzy `query`.
    pub fn commands(&self, query: &str) -> Vec<CommandDto> {
        filter_commands(query, &builtin_commands())
    }

    /// Command-palette commands merged with dynamic command files discovered
    /// from the provided project/workspace roots.
    pub fn commands_with_roots(
        &self,
        query: &str,
        roots: impl IntoIterator<Item = std::path::PathBuf>,
    ) -> Vec<CommandDto> {
        commands_from_roots(query, roots)
    }

    /// Reconstruct a session's conversation as ordered, styled messages so the
    /// UI can replay a returned-to session with the SAME tool cards / reasoning
    /// / text it showed live — not flattened timeline lines.
    ///
    /// Walks the event log in order: user/assistant `MessageAppended` become
    /// message turns, provider-native reasoning items retain their original
    /// position, and
    /// `ToolCallRequested`/`ToolCallCompleted` fold into a tool card attached
    /// (in chronological position) to the current assistant turn.
    pub fn session_conversation(&self, session_id: &str) -> Result<Vec<ConversationMessageDto>> {
        let id = SessionId::from_str(session_id)
            .map_err(|e| CoreError::invalid(format!("bad session id: {e}")))?;
        let store = EventStore::new(&self.db);
        let events = store.load_session(id)?;
        self.session_conversation_from_events(session_id, &events)
    }

    fn session_conversation_from_events(
        &self,
        session_id: &str,
        events: &[deepagent_core::event::Event],
    ) -> Result<Vec<ConversationMessageDto>> {
        use deepagent_core::event::EventPayload;
        use deepagent_core::message::Role;

        if events.is_empty() {
            return Err(CoreError::not_found(format!("session {session_id}")));
        }
        let mut recorded_costs = CostStore::new(&self.db)
            .session_costs_detailed(session_id)?
            .into_iter();

        let mut messages: Vec<ConversationMessageDto> = Vec::new();
        let run_ids: std::collections::HashMap<_, _> = RunStore::new(&self.db)
            .list_for_session(session_id)?
            .into_iter()
            .filter_map(|run| run.task_id.map(|task| (task, run.id)))
            .collect();
        let mut turns: Vec<(usize, ConversationTurnDto)> = Vec::new();
        // Index of tool parts by call_id within the current assistant turn, so a
        // completion updates the request card in place.
        let mut tool_pos: std::collections::HashMap<String, (usize, usize)> =
            std::collections::HashMap::new();
        // Native Responses reasoning is persisted separately from the final
        // MessageAppended projection. Replay exact duplicate provider items
        // only once within this user turn, matching the stream accumulator.
        let mut seen_reasoning_items = std::collections::HashSet::new();
        // Assistant text exists in two shapes for the SAME fact: the provider
        // item written per model turn, and the `MessageAppended` written once at
        // completion (`Session::append` additionally projects the message back
        // into an item). Track the texts already turned into a part this user
        // turn so neither shape renders twice.
        let mut seen_assistant_texts = std::collections::HashSet::new();
        let mut native_reasoning_since_tool = String::new();

        // Ensure there is a trailing assistant turn to attach tool cards to.
        let ensure_assistant = |messages: &mut Vec<ConversationMessageDto>| -> usize {
            if let Some(last) = messages.last() {
                if last.role == "assistant" {
                    return messages.len() - 1;
                }
            }
            messages.push(ConversationMessageDto {
                id: None,
                turn: None,
                role: "assistant".to_string(),
                content: String::new(),
                attachments: Vec::new(),
                parts: Vec::new(),
                usage: None,
            });
            messages.len() - 1
        };

        for (event_index, ev) in events.iter().enumerate() {
            match &ev.payload {
                EventPayload::MessageAppended { message } => match message.role {
                    Role::User => {
                        let text = message.content.trim().to_string();
                        if text.is_empty() {
                            continue;
                        }
                        // A new user turn ends the previous assistant turn's
                        // tool-card grouping.
                        tool_pos.clear();
                        seen_reasoning_items.clear();
                        seen_assistant_texts.clear();
                        native_reasoning_since_tool.clear();
                        let attachments = parse_conversation_attachments(&text, session_id);
                        turns.push((
                            messages.len(),
                            ConversationTurnDto {
                                version: 1,
                                id: format!("{session_id}:{}", ev.id),
                                terminal_kind: None,
                                final_answer: None,
                            },
                        ));
                        messages.push(ConversationMessageDto {
                            id: Some(ev.id.to_string()),
                            turn: None,
                            role: "user".to_string(),
                            content: text.clone(),
                            attachments,
                            parts: vec![ConversationPartDto::Text { text }],
                            usage: None,
                        });
                    }
                    Role::Assistant => {
                        let content = message.content.trim();
                        if message.tool_calls.is_empty() && !content.is_empty() {
                            if let Some((_, turn)) = turns.last_mut() {
                                turn.final_answer = Some(content.to_string());
                            }
                        }
                        let reasoning = message
                            .reasoning_content
                            .as_deref()
                            .map(str::trim)
                            .filter(|s| !s.is_empty());
                        if content.is_empty() && reasoning.is_none() {
                            // Pure tool-call assistant turn: cards already added.
                            continue;
                        }
                        let idx = ensure_assistant(&mut messages);
                        if let Some(r) =
                            reasoning.filter(|r| *r != native_reasoning_since_tool.trim())
                        {
                            messages[idx].parts.push(ConversationPartDto::Reasoning {
                                text: r.to_string(),
                            });
                        }
                        native_reasoning_since_tool.clear();
                        if !content.is_empty() {
                            if seen_assistant_texts.insert(content.to_string()) {
                                messages[idx].parts.push(ConversationPartDto::Text {
                                    text: content.to_string(),
                                });
                            }
                            if messages[idx].content.is_empty() {
                                messages[idx].content = content.to_string();
                            } else {
                                messages[idx].content.push('\n');
                                messages[idx].content.push_str(content);
                            }
                        }
                    }
                    _ => {}
                },
                EventPayload::TaskCreated { task_id, .. } => {
                    if let Some((_, turn)) = turns.last_mut() {
                        turn.id = run_ids
                            .get(&task_id.to_string())
                            .cloned()
                            .unwrap_or_else(|| format!("{session_id}:{task_id}"));
                    }
                }
                EventPayload::RunTerminal { kind, reason } => {
                    if let Some((_, turn)) = turns.last_mut() {
                        turn.terminal_kind = Some(kind.clone());
                        if turn.final_answer.is_none() {
                            turn.final_answer = reason.clone();
                        }
                    }
                }
                EventPayload::ResponseItemAppended {
                    item: deepagent_core::response_item::ResponseItem::Reasoning { id, content },
                } if !content.trim().is_empty() => {
                    // Session::append(MessageAppended) also writes a synthetic
                    // Responses item immediately afterwards. That legacy
                    // projection is already represented by the message above.
                    let projected_from_previous_message = event_index > 0
                        && matches!(
                            &events[event_index - 1].payload,
                            EventPayload::MessageAppended { message }
                                if message.role == Role::Assistant
                                    && message.reasoning_content.as_deref() == Some(content)
                        );
                    if projected_from_previous_message
                        || !seen_reasoning_items.insert((id.clone(), content.clone()))
                    {
                        continue;
                    }
                    let idx = ensure_assistant(&mut messages);
                    messages[idx].parts.push(ConversationPartDto::Reasoning {
                        text: content.clone(),
                    });
                    native_reasoning_since_tool.push_str(content);
                }
                EventPayload::ResponseItemAppended {
                    item: deepagent_core::response_item::ResponseItem::Message { role, content },
                } if role == "assistant" && !content.trim().is_empty() => {
                    // Narration the model emits before or between tool calls is
                    // ONLY persisted as a provider item — the assistant
                    // `MessageAppended` is written once, at completion — so
                    // replay has to project it here or the UI loses its
                    // intermediate-output rows after a reload. A retried request
                    // can leave the same item twice, and the final answer exists
                    // as both an item and a message, hence the shared per-turn
                    // text set.
                    let text = content.trim();
                    if !seen_assistant_texts.insert(text.to_string()) {
                        continue;
                    }
                    let idx = ensure_assistant(&mut messages);
                    messages[idx].parts.push(ConversationPartDto::Text {
                        text: text.to_string(),
                    });
                }
                EventPayload::ToolCallRequested { call } => {
                    native_reasoning_since_tool.clear();
                    let idx = ensure_assistant(&mut messages);
                    let part_idx = messages[idx].parts.len();
                    let metadata = tool_ui_metadata(&call.name, &call.arguments, None);
                    messages[idx].parts.push(ConversationPartDto::Tool {
                        call_id: call.id.clone(),
                        name: call.name.clone(),
                        args: serde_json::to_string_pretty(&call.arguments)
                            .unwrap_or_else(|_| call.arguments.to_string()),
                        status: "running".to_string(),
                        duration_ms: None,
                        detail: None,
                        output: None,
                        tool_kind: metadata.tool_kind,
                        file_path: metadata.file_path,
                        summary: metadata.summary,
                        meta: metadata.meta,
                    });
                    tool_pos.insert(call.id.clone(), (idx, part_idx));
                }
                EventPayload::ToolCallCompleted {
                    call_id,
                    ok,
                    output,
                    duration_ms,
                } => {
                    if let Some(&(idx, part_idx)) = tool_pos.get(call_id) {
                        if let Some(ConversationPartDto::Tool {
                            status,
                            name,
                            args,
                            duration_ms: dm,
                            detail,
                            output: tool_output,
                            tool_kind,
                            file_path,
                            summary,
                            meta,
                            ..
                        }) = messages[idx].parts.get_mut(part_idx)
                        {
                            *status = if *ok {
                                "ok".to_string()
                            } else {
                                "error".to_string()
                            };
                            *dm = Some(*duration_ms);
                            *detail = Some(summarize_output_for_tool(name, output));
                            *tool_output = Some(output.clone());
                            let arguments =
                                serde_json::from_str(args).unwrap_or(serde_json::Value::Null);
                            let metadata = tool_ui_metadata(name, &arguments, Some(output));
                            *tool_kind = metadata.tool_kind;
                            *file_path = metadata.file_path;
                            *summary = metadata.summary;
                            *meta = metadata.meta;
                        }
                    }
                }
                EventPayload::UsageRecorded {
                    prompt_tokens,
                    completion_tokens,
                    reasoning_tokens,
                    total_tokens,
                    prompt_cache_hit_tokens,
                    prompt_cache_miss_tokens,
                    duration_ms,
                    raw_responses_usage,
                } => {
                    // Attach the run's persisted usage to the current (last)
                    // assistant turn so the replayed footer matches the live one.
                    let idx = ensure_assistant(&mut messages);
                    messages[idx].usage = Some(ConversationUsageDto {
                        prompt_tokens: *prompt_tokens,
                        completion_tokens: *completion_tokens,
                        reasoning_tokens: *reasoning_tokens,
                        total_tokens: *total_tokens,
                        prompt_cache_hit_tokens: *prompt_cache_hit_tokens,
                        prompt_cache_miss_tokens: *prompt_cache_miss_tokens,
                        duration_ms: *duration_ms,
                        raw_responses_usage: raw_responses_usage.clone(),
                        cost_yuan: recorded_costs.next().and_then(|c| c.cost_yuan),
                    });
                }
                // A durable compaction boundary is part of what the user reads
                // back after a reload; metrics-only records (micro-compaction)
                // changed nothing in the model-facing view, so they stay hidden.
                EventPayload::ContextCompacted {
                    tokens_before,
                    tokens_after,
                    strategy,
                    summary: Some(summary),
                    first_kept_sequence,
                } => {
                    // The boundary closes the current turn's grouping: what
                    // follows is a new context window.
                    tool_pos.clear();
                    seen_reasoning_items.clear();
                    seen_assistant_texts.clear();
                    native_reasoning_since_tool.clear();
                    messages.push(ConversationMessageDto {
                        id: Some(ev.id.to_string()),
                        turn: None,
                        role: "system".to_string(),
                        content: String::new(),
                        attachments: Vec::new(),
                        parts: vec![ConversationPartDto::CompactBoundary {
                            tokens_before: *tokens_before,
                            tokens_after: *tokens_after,
                            strategy: strategy.clone(),
                            summary: summary.clone(),
                            first_kept_sequence: *first_kept_sequence,
                        }],
                        usage: None,
                    });
                }
                EventPayload::MemoriesInjected {
                    ids,
                    count,
                    latency_ms,
                    ..
                } => {
                    if *count == 0 {
                        continue;
                    }
                    messages.push(ConversationMessageDto {
                        id: Some(ev.id.to_string()),
                        turn: None,
                        role: "system".to_string(),
                        content: String::new(),
                        attachments: Vec::new(),
                        parts: vec![ConversationPartDto::Memories {
                            count: *count,
                            ids: ids.clone(),
                            latency_ms: *latency_ms,
                        }],
                        usage: None,
                    });
                }
                // Bookkeeping without a transcript projection. Recorded rather
                // than swallowed: a new content-bearing payload that nobody maps
                // here is exactly how "content disappears after a reload" starts.
                other => {
                    tracing::debug!(
                        kind = other.kind(),
                        session_id,
                        "durable event has no conversation projection"
                    );
                }
            }
        }

        for (index, (start, turn)) in turns.iter().enumerate() {
            let end = turns
                .get(index + 1)
                .map_or(messages.len(), |(start, _)| *start);
            for (offset, message) in messages[*start..end].iter_mut().enumerate() {
                if message.role == "system" {
                    continue;
                }
                message
                    .id
                    .get_or_insert_with(|| format!("{}:message:{offset}", turn.id));
                message.turn = Some(turn.clone());
            }
        }

        messages.retain(|m| {
            if m.role != "assistant" {
                return true;
            }
            if !m.content.trim().is_empty() {
                return true;
            }
            m.parts.iter().any(|p| match p {
                ConversationPartDto::Text { text } | ConversationPartDto::Reasoning { text } => {
                    !text.trim().is_empty()
                }
                ConversationPartDto::Tool { .. }
                | ConversationPartDto::CompactBoundary { .. }
                | ConversationPartDto::Memories { .. } => true,
            })
        });

        Ok(messages)
    }

    /// **Fork** a session at sequence `at_seq`: restore workspace files to the
    /// same checkpoint horizon, then create a new sibling branch whose stream
    /// copies events `0..=at_seq` from the source. The source event log is left
    /// untouched; the workspace is stateful and follows the fork point.
    pub fn fork_session(&self, session_id: &str, at_seq: u64) -> Result<ForkResultDto> {
        let id = SessionId::from_str(session_id)
            .map_err(|e| CoreError::invalid(format!("bad session id: {e}")))?;
        let clock = SystemClock;
        let restored_paths = self.restore_checkpoints_after(session_id, at_seq)?;
        let forked = Session::fork(&self.db, &clock, id, at_seq)?;
        Ok(ForkResultDto {
            new_session_id: forked.id().to_string(),
            source_session_id: session_id.to_string(),
            forked_at: at_seq,
            restored_paths,
        })
    }

    /// **Rewind** a session in place to sequence `to_seq`: discard every event
    /// after `to_seq`. Destructive and user-initiated; the session is reopened
    /// so new turns append from the kept tail. Returns how many events were
    /// removed.
    pub fn rewind_session(&self, session_id: &str, to_seq: u64) -> Result<RewindResultDto> {
        let id = SessionId::from_str(session_id)
            .map_err(|e| CoreError::invalid(format!("bad session id: {e}")))?;
        let clock = SystemClock;
        let mut session = Session::recover(&self.db, &clock, id)?;
        let restored_paths = self.restore_checkpoints_after(session_id, to_seq)?;
        let removed = session.rewind(to_seq)?;
        Ok(RewindResultDto {
            session_id: session_id.to_string(),
            kept_through: to_seq,
            events_removed: removed,
            restored_paths,
        })
    }

    fn restore_checkpoints_after(&self, session_id: &str, sequence: u64) -> Result<Vec<String>> {
        let checkpoints = CheckpointStore::new(&self.db)
            .list_for_session_after_sequence(session_id, sequence as i64)?;
        let mut restored_paths = Vec::new();
        for checkpoint in checkpoints {
            for path in CheckpointManager::restore(&self.db, &checkpoint.id)? {
                let rendered = path.to_string_lossy().to_string();
                if !restored_paths.contains(&rendered) {
                    restored_paths.push(rendered);
                }
            }
        }
        Ok(restored_paths)
    }

    /// Export a session's transcript in `format` ("markdown"/"md" or "json").
    pub fn export_transcript(&self, session_id: &str, format: &str) -> Result<TranscriptDto> {
        let id = SessionId::from_str(session_id)
            .map_err(|e| CoreError::invalid(format!("bad session id: {e}")))?;
        let fmt = TranscriptFormat::parse(format)
            .ok_or_else(|| CoreError::invalid(format!("unknown transcript format: {format}")))?;
        let store = EventStore::new(&self.db);
        let record = store
            .get_session(id)?
            .ok_or_else(|| CoreError::not_found(format!("session {session_id}")))?;
        let events = store.load_session(id)?;
        if events.is_empty() {
            return Err(CoreError::not_found(format!("session {session_id}")));
        }
        let content = export_transcript(record.title.as_deref(), &events, fmt)
            .map_err(|e| CoreError::invalid(format!("transcript render failed: {e}")))?;
        Ok(TranscriptDto {
            session_id: session_id.to_string(),
            format: format.trim().to_ascii_lowercase(),
            extension: fmt.extension().to_string(),
            content,
        })
    }

    /// Compute a line-based diff between `old` and `new` for the Diff view.
    pub fn diff(&self, old: &str, new: &str) -> DiffResult {
        diff_lines(old, new)
    }
}

/// Map a stored project path to its display name (last folder component).
fn session_summary_dto(
    record: SessionRecord,
    project: Option<String>,
    pinned: bool,
) -> SessionSummaryDto {
    SessionSummaryDto {
        id: record.id.to_string(),
        project,
        title: record.title,
        mode: record.mode.label().to_string(),
        created_at: record.created_at.as_millis(),
        updated_at: record.updated_at.as_millis(),
        ended: record.ended_at.is_some(),
        pinned,
    }
}

fn project_display_name(path: &str) -> String {
    crate::project_service::folder_name(path)
}

fn summarize_output_for_tool(tool_name: &str, output: &serde_json::Value) -> String {
    if tool_name == "web_search" {
        return summarize_web_search_output(output);
    }
    summarize_output(output)
}

fn summarize_web_search_output(output: &serde_json::Value) -> String {
    if let Some(e) = output.get("error").and_then(|v| v.as_str()) {
        return e.to_string();
    }
    let provider = output
        .get("provider")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    let count = output
        .get("count")
        .and_then(|v| v.as_u64())
        .or_else(|| {
            output
                .get("results")
                .and_then(|v| v.as_array())
                .map(|a| a.len() as u64)
        })
        .unwrap_or(0);
    let attempts = output
        .get("attempts")
        .and_then(|v| v.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    let provider = item.get("provider").and_then(|v| v.as_str())?;
                    let ok = item.get("ok").and_then(|v| v.as_bool()).unwrap_or(false);
                    Some(format!("{provider}:{}", if ok { "ok" } else { "failed" }))
                })
                .collect::<Vec<_>>()
                .join(", ")
        })
        .filter(|s| !s.is_empty());

    match attempts {
        Some(attempts) => {
            format!("web_search: {provider} returned {count} result(s); attempts: {attempts}")
        }
        None => format!("web_search: {provider} returned {count} result(s)"),
    }
}

/// Summarize a tool's JSON output into a short one-line detail string for the
/// replayed tool card (mirrors the frontend's live `summarize`).
fn summarize_output(output: &serde_json::Value) -> String {
    const MAX: usize = 200;
    let raw = if let Some(s) = output.as_str() {
        s.to_string()
    } else if let Some(e) = output.get("error").and_then(|v| v.as_str()) {
        e.to_string()
    } else if let Some(arr) = output.get("results").and_then(|v| v.as_array()) {
        format!("{} result(s)", arr.len())
    } else if let Some(n) = output.get("count").and_then(|v| v.as_u64()) {
        format!("{n} result(s)")
    } else if let Some(p) = output.get("path").and_then(|v| v.as_str()) {
        p.to_string()
    } else {
        output.to_string()
    };
    let one_line = raw.replace('\n', " ");
    if one_line.chars().count() <= MAX {
        one_line
    } else {
        let truncated: String = one_line.chars().take(MAX).collect();
        format!("{truncated}…")
    }
}

fn parse_conversation_attachments(content: &str, session_id: &str) -> Vec<AttachmentDto> {
    let Some((_, after_open)) = content.split_once("<attachments>") else {
        return Vec::new();
    };
    let Some((attachments_block, _)) = after_open.split_once("</attachments>") else {
        return Vec::new();
    };

    let mut attachments = Vec::new();
    let mut rest = attachments_block;
    while let Some(start) = rest.find("<attachment") {
        rest = &rest[start + "<attachment".len()..];
        let Some(tag_end) = rest.find('>') else {
            break;
        };
        let attrs = parse_attachment_attrs(&rest[..tag_end]);
        let body_start = tag_end + 1;
        let Some(body_end) = rest[body_start..].find("</attachment>") else {
            break;
        };
        let body = &rest[body_start..body_start + body_end];
        rest = &rest[body_start + body_end + "</attachment>".len()..];

        let name = attrs
            .get("name")
            .cloned()
            .unwrap_or_else(|| "attachment".to_string());
        let mime = attrs
            .get("type")
            .cloned()
            .unwrap_or_else(|| "application/octet-stream".to_string());
        let kind = attrs
            .get("kind")
            .cloned()
            .unwrap_or_else(|| infer_attachment_kind(&mime).to_string());
        let original_path = attachment_body_value(body, "path:");
        let storage_dir = original_path
            .as_deref()
            .and_then(|path| std::path::Path::new(path).parent())
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_default();
        let size_bytes = attachment_body_value(body, "size:")
            .and_then(|value| {
                value
                    .strip_suffix("bytes")
                    .map(str::trim)
                    .map(str::to_string)
            })
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or_default();
        let source = attachment_body_value(body, "source:").unwrap_or_else(|| "paste".to_string());
        let sha256 = attachment_body_value(body, "sha256:");
        let extracted_text = attachment_body_extracted_text(body);
        let id = attrs
            .get("index")
            .map(|index| format!("history-{index}-{name}"))
            .unwrap_or_else(|| format!("history-{}-{name}", attachments.len() + 1));

        attachments.push(AttachmentDto {
            id,
            session_id: Some(session_id.to_string()),
            kind,
            name,
            mime,
            size_bytes,
            source,
            storage_dir,
            original_path,
            extracted_text,
            preview: None,
            sha256,
            status: "ready".to_string(),
            message: None,
        });
    }
    attachments
}

fn parse_attachment_attrs(raw: &str) -> std::collections::HashMap<String, String> {
    let mut attrs = std::collections::HashMap::new();
    let mut rest = raw.trim();
    while let Some(eq) = rest.find('=') {
        let key = rest[..eq].trim();
        rest = rest[eq + 1..].trim_start();
        if key.is_empty() || !rest.starts_with('"') {
            break;
        }
        rest = &rest[1..];
        let Some(end_quote) = rest.find('"') else {
            break;
        };
        attrs.insert(key.to_string(), rest[..end_quote].to_string());
        rest = rest[end_quote + 1..].trim_start();
    }
    attrs
}

fn attachment_body_value(body: &str, prefix: &str) -> Option<String> {
    body.lines()
        .find_map(|line| line.trim().strip_prefix(prefix).map(str::trim))
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn attachment_body_extracted_text(body: &str) -> Option<String> {
    let mut content = Vec::new();
    let mut in_content = false;
    for line in body.lines() {
        let trimmed = line.trim();
        if !in_content
            && (trimmed.is_empty()
                || trimmed.starts_with("source:")
                || trimmed.starts_with("size:")
                || trimmed.starts_with("path:")
                || trimmed.starts_with("sha256:"))
        {
            if trimmed.is_empty() {
                in_content = true;
            }
            continue;
        }
        in_content = true;
        content.push(line);
    }
    let text = content.join("\n").trim().to_string();
    if text.is_empty()
        || text.starts_with("This image is attached but could not be recognized")
        || text.starts_with("Binary or unsupported file content was not read")
    {
        None
    } else {
        Some(text)
    }
}

fn infer_attachment_kind(mime: &str) -> &str {
    if mime.starts_with("image/") {
        "image"
    } else if mime.starts_with("text/") {
        "text"
    } else {
        "file"
    }
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
}

/// Classify an interrupted run from its session event log (Claude Code
/// `detectTurnInterruption` parity). Read-only: surfaces *why* a recovered run
/// was interrupted without changing the recovery behavior.
///
/// - `Some("interrupted_prompt")` — the last decisive turn is a user message:
///   the prompt never got a reply.
/// - `Some("interrupted_turn")` — the run died mid-turn (a tool call/output has
///   no completing assistant reply).
/// - `None` — the run ended between turns (last turn is a completed assistant
///   reply) or no decisive turn exists.
fn classify_interruption(events: &[deepagent_core::event::Event]) -> Option<&'static str> {
    use deepagent_core::event::EventPayload;
    use deepagent_core::message::Role;
    use deepagent_core::response_item::ResponseItem;
    for event in events.iter().rev() {
        match &event.payload {
            EventPayload::ResponseItemAppended { item } => match item {
                ResponseItem::Message { role, .. } | ResponseItem::InputMessage { role, .. } => {
                    match role.as_str() {
                        "assistant" => return None,
                        "user" => return Some("interrupted_prompt"),
                        _ => continue,
                    }
                }
                ResponseItem::FunctionCall { .. }
                | ResponseItem::FunctionCallOutput { .. }
                | ResponseItem::CustomToolCall { .. }
                | ResponseItem::CustomToolCallOutput { .. }
                | ResponseItem::WebSearchCall { .. } => return Some("interrupted_turn"),
                ResponseItem::Reasoning { .. } => continue,
            },
            EventPayload::MessageAppended { message } => match message.role {
                Role::Assistant => return None,
                Role::User => return Some("interrupted_prompt"),
                Role::Tool => return Some("interrupted_turn"),
                Role::System => continue,
            },
            EventPayload::ToolCallRequested { .. } | EventPayload::ToolCallCompleted { .. } => {
                return Some("interrupted_turn")
            }
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use deepagent_core::clock::FixedClock;
    use deepagent_session::Session;

    fn seeded_service() -> (AppService, String) {
        let db = Database::open_in_memory().unwrap();
        let clock = FixedClock::new(1_000);
        let sid;
        {
            let mut s = Session::create(&db, &clock, Some("demo")).unwrap();
            sid = s.id().to_string();
            let task = s.create_task("do the thing").unwrap();
            s.transition_task(task, deepagent_core::task::TaskState::Running)
                .unwrap();
            s.append(deepagent_core::event::EventPayload::ToolCallCompleted {
                call_id: "c1".into(),
                ok: true,
                output: serde_json::json!({"ok": true}),
                duration_ms: 25,
            })
            .unwrap();
        }
        (AppService::new(db), sid)
    }

    #[test]
    fn session_conversation_projects_all_terminals_with_stable_run_identity() {
        use deepagent_core::event::EventPayload;
        use deepagent_core::message::Message;

        for kind in [
            "succeeded",
            "failed",
            "cancelled",
            "blocked",
            "max_turns",
            "budget_exceeded",
            "context_exhausted",
        ] {
            let db = Database::open_in_memory().unwrap();
            let clock = FixedClock::new(1_000);
            let mut session = Session::create(&db, &clock, Some("turn replay")).unwrap();
            let sid = session.id().to_string();
            session
                .append(EventPayload::MessageAppended {
                    message: Message::user("question"),
                })
                .unwrap();
            let task = session.create_task("question").unwrap();
            RunStore::new(&db)
                .create("run-turn", &sid, Some(&task.to_string()), 1_000)
                .unwrap();
            session
                .append(EventPayload::ResponseItemAppended {
                    item: deepagent_core::response_item::ResponseItem::Reasoning {
                        id: Some("r1".into()),
                        content: "inspect".into(),
                    },
                })
                .unwrap();
            // A system marker must not steal the terminal or final-answer owner.
            session
                .append(EventPayload::MemoriesInjected {
                    ids: vec!["memory".into()],
                    count: 1,
                    latency_ms: 2,
                    trigger: "prefetch".into(),
                })
                .unwrap();
            session
                .append(EventPayload::MessageAppended {
                    message: Message::assistant("final answer"),
                })
                .unwrap();
            session
                .append(EventPayload::RunTerminal {
                    kind: kind.into(),
                    reason: Some("terminal detail".into()),
                })
                .unwrap();
            let service = AppService::new(db);
            let first = service.session_conversation(&sid).unwrap();
            let again = service.session_conversation(&sid).unwrap();
            assert_eq!(first, again);
            for message in first.iter().filter(|message| message.role != "system") {
                let turn = message.turn.as_ref().unwrap();
                assert_eq!(turn.version, 1);
                assert_eq!(turn.id, "run-turn");
                assert_eq!(turn.terminal_kind.as_deref(), Some(kind));
                assert_eq!(turn.final_answer.as_deref(), Some("final answer"));
                assert!(message.id.is_some());
            }
            assert!(first
                .iter()
                .filter(|message| message.role == "system")
                .all(|message| message.turn.is_none()));
        }
    }

    #[test]
    fn session_conversation_keeps_legacy_unknown_and_separates_adjacent_runs() {
        use deepagent_core::event::EventPayload;
        use deepagent_core::message::Message;

        let db = Database::open_in_memory().unwrap();
        let clock = FixedClock::new(1_000);
        let mut session = Session::create(&db, &clock, Some("legacy turns")).unwrap();
        let sid = session.id().to_string();
        for text in ["old question", "new question"] {
            session
                .append(EventPayload::MessageAppended {
                    message: Message::user(text),
                })
                .unwrap();
            session
                .append(EventPayload::MessageAppended {
                    message: Message::assistant(text),
                })
                .unwrap();
        }
        session
            .append(EventPayload::RunTerminal {
                kind: "cancelled".into(),
                reason: None,
            })
            .unwrap();
        let conversation = AppService::new(db).session_conversation(&sid).unwrap();
        assert_ne!(
            conversation[0].turn.as_ref().unwrap().id,
            conversation[2].turn.as_ref().unwrap().id
        );
        assert!(conversation[0]
            .turn
            .as_ref()
            .unwrap()
            .terminal_kind
            .is_none());
        assert_eq!(
            conversation[2]
                .turn
                .as_ref()
                .unwrap()
                .terminal_kind
                .as_deref(),
            Some("cancelled")
        );
        // Old DTOs still deserialize without the optional projection.
        let old: ConversationMessageDto = serde_json::from_value(
            serde_json::json!({ "role":"assistant", "content":"answer", "parts":[] }),
        )
        .unwrap();
        assert!(old.turn.is_none());
        assert!(old.id.is_none());
    }

    #[test]
    fn lists_sessions() {
        let (svc, sid) = seeded_service();
        let sessions = svc.list_sessions().unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, sid);
        assert_eq!(sessions[0].title.as_deref(), Some("demo"));
    }

    #[test]
    fn searches_conversation_body_and_excludes_archived_sessions() {
        let db = std::sync::Arc::new(Database::open_in_memory().unwrap());
        let clock = FixedClock::new(1_000);
        let mut session = Session::create(&db, &clock, Some("searchable")).unwrap();
        session
            .append(deepagent_core::event::EventPayload::MessageAppended {
                message: deepagent_core::message::Message::user("中文缓存命中率检查"),
            })
            .unwrap();
        let session_id = session.id().to_string();
        let service = AppService::from_shared(db.clone());

        let hits = service.search_sessions("缓存", None, 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].session.id, session_id);
        assert!(hits[0].snippet.contains("缓存"));

        ArchiveService::new(db)
            .archive_session(&session_id)
            .unwrap();
        assert!(service
            .search_sessions("缓存", None, 10)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn list_sessions_hides_removed_project_sessions() {
        let db = std::sync::Arc::new(Database::open_in_memory().unwrap());
        let clock = FixedClock::new(1_000);
        let projects = ProjectService::new(db.clone());
        projects.add_project("/work/a").unwrap();
        projects.add_project("/work/b").unwrap();
        Session::create_in_project(&db, &clock, Some("a"), Default::default(), Some("/work/a"))
            .unwrap();
        Session::create_in_project(&db, &clock, Some("b"), Default::default(), Some("/work/b"))
            .unwrap();
        let svc = AppService::from_shared(db);

        assert_eq!(svc.list_sessions().unwrap().len(), 2);
        projects.remove_project("/work/b").unwrap();
        let sessions = svc.list_sessions().unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].project.as_deref(), Some("a"));
    }

    #[test]
    fn list_sessions_hides_rows_without_file_backed_events() {
        let db = Database::open_in_memory().unwrap();
        EventStore::new(&db)
            .create_session(
                SessionId::new(),
                Some("metadata only"),
                deepagent_core::clock::Timestamp::from_millis(1),
            )
            .unwrap();
        assert!(AppService::new(db).list_sessions().unwrap().is_empty());
    }

    #[test]
    fn fileless_legacy_row_cannot_be_opened_exported_or_forked() {
        let db = std::sync::Arc::new(Database::open_in_memory().unwrap());
        let id = SessionId::new();
        db.with_conn(|connection| {
            connection
                .execute(
                    "INSERT INTO sessions
                     (id, title, mode, project, created_at, updated_at, ended_at)
                     VALUES (?1, 'legacy', 'normal', NULL, 1, 1, NULL)",
                    [id.to_string()],
                )
                .map_err(|error| CoreError::Persistence(error.to_string()))?;
            Ok(())
        })
        .unwrap();
        let service = AppService::from_shared(db.clone());
        let id = id.to_string();

        assert!(matches!(
            service.session_detail(&id),
            Err(CoreError::NotFound(_))
        ));
        assert!(matches!(
            service.session_conversation(&id),
            Err(CoreError::NotFound(_))
        ));
        assert!(matches!(
            service.export_transcript(&id, "json"),
            Err(CoreError::NotFound(_))
        ));
        assert!(matches!(
            service.fork_session(&id, 0),
            Err(CoreError::NotFound(_))
        ));
        let count = db
            .with_conn(|connection| {
                connection
                    .query_row("SELECT COUNT(*) FROM sessions", [], |row| {
                        row.get::<_, i64>(0)
                    })
                    .map_err(|error| CoreError::Persistence(error.to_string()))
            })
            .unwrap();
        assert_eq!(count, 1, "failed fork must not create an orphan session");
        assert!(service.list_sessions().unwrap().is_empty());
    }

    #[test]
    fn session_detail_has_timeline_and_stats() {
        let (svc, sid) = seeded_service();
        let detail = svc.session_detail(&sid).unwrap();
        assert_eq!(detail.summary.id, sid);
        assert_eq!(detail.version, 1);
        assert!(detail.project_path.is_none());
        assert!(!detail.timeline.is_empty());
        assert_eq!(detail.stats.tool_successes, 1);
        assert_eq!(detail.stats.tool_success_rate, Some(1.0));
        // Timeline is ordered by sequence.
        for w in detail.timeline.windows(2) {
            assert!(w[0].sequence < w[1].sequence);
        }
    }

    #[test]
    fn session_detail_restores_the_durable_context_projection() {
        let (svc, sid) = seeded_service();
        let policy = deepagent_context::ContextPolicy::for_capability(
            &deepagent_models::ModelCapabilityResolver::new().resolve_model_id("deepseek-flash"),
            deepagent_models::ThinkingDepth::Medium,
        );
        let mut snapshot = deepagent_context::ContextUsageSnapshot::from_pack(
            &policy,
            &deepagent_context::ContextPack::default(),
            false,
        );
        snapshot.record_projection(
            deepagent_context::SurfaceTokenBreakdown {
                messages: 1_000,
                ..Default::default()
            },
            1_200,
        );
        let runs = deepagent_persistence::run_store::RunStore::new(&svc.db);
        runs.create("context-restore", &sid, None, 1).unwrap();
        runs.append_event(
            "context-restore",
            1,
            "running_turn",
            "progress",
            "context_usage",
            &serde_json::json!({"snapshot": snapshot}),
        )
        .unwrap();
        let detail = svc.session_detail(&sid).unwrap();
        assert_eq!(
            detail.context_usage.unwrap().projected_request_tokens,
            Some(1_200)
        );
        let mut legacy = serde_json::to_value(svc.session_detail(&sid).unwrap()).unwrap();
        legacy.as_object_mut().unwrap().remove("context_usage");
        assert!(serde_json::from_value::<SessionDetailDto>(legacy)
            .unwrap()
            .context_usage
            .is_none());
    }

    #[test]
    fn session_detail_preserves_project_identity_and_legacy_payloads() {
        let db = std::sync::Arc::new(Database::open_in_memory().unwrap());
        let clock = FixedClock::new(1_000);
        let projects = ProjectService::new(db.clone());
        for path in ["/work/first/app", "/work/second/app"] {
            projects.add_project(path).unwrap();
        }
        projects.set_active("/work/second/app").unwrap();
        let session = Session::create_in_project(
            &db,
            &clock,
            Some("first project"),
            Default::default(),
            Some("/work/first/app"),
        )
        .unwrap();
        let detail = AppService::from_shared(db.clone())
            .session_detail(&session.id().to_string())
            .unwrap();
        assert_eq!(detail.summary.project.as_deref(), Some("app"));
        assert_eq!(detail.project_path.as_deref(), Some("/work/first/app"));
        let mut legacy = serde_json::to_value(&detail).unwrap();
        legacy.as_object_mut().unwrap().remove("version");
        legacy.as_object_mut().unwrap().remove("project_path");
        let decoded: SessionDetailDto = serde_json::from_value(legacy).unwrap();
        assert_eq!(decoded.version, 0);
        assert!(decoded.project_path.is_none());
    }

    #[test]
    fn session_conversation_preserves_web_search_output_metadata() {
        use deepagent_core::event::EventPayload;
        use deepagent_core::message::ToolCall;

        let db = Database::open_in_memory().unwrap();
        let clock = FixedClock::new(1_000);
        let sid;
        {
            let mut s = Session::create(&db, &clock, Some("search")).unwrap();
            sid = s.id().to_string();
            s.append(EventPayload::ToolCallRequested {
                call: ToolCall {
                    id: "search-1".into(),
                    name: "web_search".into(),
                    arguments: serde_json::json!({"query": "deepseek web search", "limit": 2}),
                },
            })
            .unwrap();
            s.append(EventPayload::ToolCallCompleted {
                call_id: "search-1".into(),
                ok: true,
                output: serde_json::json!({
                    "query": "deepseek web search",
                    "provider": "searxng",
                    "count": 1,
                    "results": [{
                        "title": "DeepSeek",
                        "url": "https://deepseek.com",
                        "snippet": "result"
                    }],
                    "attempts": [
                        {"provider": "deepseek", "ok": false, "error": "not enabled"},
                        {"provider": "searxng", "ok": true, "count": 1}
                    ]
                }),
                duration_ms: 42,
            })
            .unwrap();
        }

        let svc = AppService::new(db);
        let conversation = svc.session_conversation(&sid).unwrap();
        let tool = conversation
            .iter()
            .flat_map(|message| message.parts.iter())
            .find_map(|part| match part {
                ConversationPartDto::Tool {
                    name,
                    detail,
                    output,
                    ..
                } if name == "web_search" => Some((detail, output)),
                _ => None,
            })
            .expect("web_search tool card");

        let detail = tool.0.as_deref().unwrap_or_default();
        assert!(detail.contains("searxng returned 1 result"));
        assert!(detail.contains("deepseek:failed"));
        let output = tool.1.as_ref().expect("raw output");
        assert_eq!(output["provider"], "searxng");
        assert_eq!(output["attempts"][0]["provider"], "deepseek");
        assert_eq!(output["attempts"][1]["ok"], true);
    }

    #[test]
    fn session_conversation_replays_native_reasoning_without_duplicating_provider_items() {
        use deepagent_core::event::EventPayload;
        use deepagent_core::message::Message;
        use deepagent_core::response_item::ResponseItem;

        let db = Database::open_in_memory().unwrap();
        let clock = FixedClock::new(1_000);
        let mut session = Session::create(&db, &clock, Some("reasoning replay")).unwrap();
        let sid = session.id().to_string();

        session
            .append(EventPayload::MessageAppended {
                message: Message::user("first"),
            })
            .unwrap();
        for _ in 0..2 {
            // A retried request can leave the same provider item in the file
            // twice, as observed in an affected real session.
            session
                .append_response_item(ResponseItem::Reasoning {
                    id: Some("rs_1".into()),
                    content: "inspect available skills".into(),
                })
                .unwrap();
            session
                .append_response_item(ResponseItem::Message {
                    role: "assistant".into(),
                    content: "first answer".into(),
                })
                .unwrap();
        }
        session
            .append_without_response_projection(EventPayload::MessageAppended {
                message: Message::assistant("first answer"),
            })
            .unwrap();

        session
            .append(EventPayload::MessageAppended {
                message: Message::user("second"),
            })
            .unwrap();
        session
            .append_response_item(ResponseItem::Reasoning {
                id: Some("rs_1".into()),
                content: "check the correction".into(),
            })
            .unwrap();
        session
            .append_without_response_projection(EventPayload::MessageAppended {
                message: Message::assistant("second answer"),
            })
            .unwrap();

        let conversation = AppService::new(db).session_conversation(&sid).unwrap();
        let assistant_parts: Vec<_> = conversation
            .iter()
            .filter(|message| message.role == "assistant")
            .map(|message| &message.parts)
            .collect();
        assert_eq!(assistant_parts.len(), 2);
        assert_eq!(
            assistant_parts[0],
            &vec![
                ConversationPartDto::Reasoning {
                    text: "inspect available skills".into(),
                },
                ConversationPartDto::Text {
                    text: "first answer".into(),
                },
            ]
        );
        assert_eq!(
            assistant_parts[1],
            &vec![
                ConversationPartDto::Reasoning {
                    text: "check the correction".into(),
                },
                ConversationPartDto::Text {
                    text: "second answer".into(),
                },
            ]
        );
    }

    #[test]
    fn session_conversation_does_not_repeat_synthetic_reasoning_projection() {
        use deepagent_core::event::EventPayload;
        use deepagent_core::message::Message;

        let db = Database::open_in_memory().unwrap();
        let clock = FixedClock::new(1_000);
        let mut session = Session::create(&db, &clock, Some("legacy reasoning")).unwrap();
        let sid = session.id().to_string();
        session
            .append(EventPayload::MessageAppended {
                message: Message::user("question"),
            })
            .unwrap();
        session
            .append(EventPayload::MessageAppended {
                message: Message::assistant("answer").with_reasoning("legacy thought"),
            })
            .unwrap();

        let conversation = AppService::new(db).session_conversation(&sid).unwrap();
        assert_eq!(
            conversation[1].parts,
            vec![
                ConversationPartDto::Reasoning {
                    text: "legacy thought".into(),
                },
                ConversationPartDto::Text {
                    text: "answer".into(),
                },
            ]
        );
    }

    #[test]
    fn session_conversation_keeps_native_reasoning_before_tool_and_final_text() {
        use deepagent_core::event::EventPayload;
        use deepagent_core::message::{Message, ToolCall};
        use deepagent_core::response_item::ResponseItem;

        let db = Database::open_in_memory().unwrap();
        let clock = FixedClock::new(1_000);
        let mut session = Session::create(&db, &clock, Some("ordered process")).unwrap();
        let sid = session.id().to_string();
        session
            .append(EventPayload::MessageAppended {
                message: Message::user("inspect"),
            })
            .unwrap();
        session
            .append_response_item(ResponseItem::Reasoning {
                id: Some("rs_tool".into()),
                content: "read the file first".into(),
            })
            .unwrap();
        session
            .append_without_response_projection(EventPayload::ToolCallRequested {
                call: ToolCall {
                    id: "call_1".into(),
                    name: "read_file".into(),
                    arguments: serde_json::json!({"path": "fixture.txt"}),
                },
            })
            .unwrap();
        session
            .append(EventPayload::ToolCallCompleted {
                call_id: "call_1".into(),
                ok: true,
                output: serde_json::json!({"content": "fixture"}),
                duration_ms: 12,
            })
            .unwrap();
        session
            .append_without_response_projection(EventPayload::MessageAppended {
                message: Message::assistant("done"),
            })
            .unwrap();

        let conversation = AppService::new(db).session_conversation(&sid).unwrap();
        let parts = &conversation[1].parts;
        assert!(matches!(
            &parts[..],
            [
                ConversationPartDto::Reasoning { text },
                ConversationPartDto::Tool {
                    call_id,
                    status,
                    ..
                },
                ConversationPartDto::Text { text: answer }
            ] if text == "read the file first"
                && call_id == "call_1"
                && status == "ok"
                && answer == "done"
        ));
    }

    #[test]
    fn durable_compaction_boundary_is_replayed_as_its_own_row() {
        use crate::dto::ConversationPartDto;
        use deepagent_core::event::EventPayload;
        use deepagent_core::message::Message;

        let db = Database::open_in_memory().unwrap();
        let clock = FixedClock::new(1_000);
        let mut session = Session::create(&db, &clock, Some("boundary")).unwrap();
        let sid = session.id().to_string();
        session
            .append(EventPayload::MessageAppended {
                message: Message::user("first question"),
            })
            .unwrap();
        session
            .append(EventPayload::MessageAppended {
                message: Message::assistant("first answer"),
            })
            .unwrap();
        // The durable boundary written by auto-compaction or /compact.
        session
            .append(EventPayload::ContextCompacted {
                tokens_before: 9_000,
                tokens_after: 2_000,
                strategy: "session_memory".to_string(),
                summary: Some("- decided: one vault".to_string()),
                first_kept_sequence: Some(2),
            })
            .unwrap();
        session
            .append(EventPayload::MessageAppended {
                message: Message::user("second question"),
            })
            .unwrap();

        let conversation = AppService::new(db).session_conversation(&sid).unwrap();
        let positions: Vec<(usize, &str)> = conversation
            .iter()
            .enumerate()
            .map(|(index, message)| (index, message.role.as_str()))
            .collect();
        let boundary_at = positions
            .iter()
            .position(|(_, role)| *role == "system")
            .unwrap_or_else(|| panic!("the boundary must replay as a row: {positions:?}"));
        assert_eq!(
            boundary_at, 2,
            "the row sits where it happened, between the two turns: {positions:?}"
        );
        assert!(
            matches!(
                conversation[boundary_at].parts.first(),
                Some(ConversationPartDto::CompactBoundary {
                    tokens_before: 9_000,
                    tokens_after: 2_000,
                    strategy,
                    summary,
                    first_kept_sequence: Some(2),
                }) if strategy == "session_memory"
                    && summary == "- decided: one vault"
            ),
            "{:?}",
            conversation[boundary_at].parts
        );
    }

    /// Every route that writes a durable boundary must reach the transcript row,
    /// including the mid-run ones: the projection keys on `summary: Some`, never on
    /// which compactor produced the boundary.
    #[test]
    fn every_boundary_route_replays_as_its_own_row() {
        use deepagent_core::compaction_strategy;
        use deepagent_core::event::EventPayload;
        use deepagent_core::message::Message;

        for strategy in [
            compaction_strategy::SESSION_MEMORY,
            compaction_strategy::MODEL,
            compaction_strategy::PROACTIVE_THRESHOLD,
            compaction_strategy::REACTIVE_MODEL,
            compaction_strategy::PREFIRE_PASS2,
        ] {
            let db = Database::open_in_memory().unwrap();
            let clock = FixedClock::new(1_000);
            let mut session = Session::create(&db, &clock, Some("route")).unwrap();
            let sid = session.id().to_string();
            session
                .append(EventPayload::MessageAppended {
                    message: Message::user("q"),
                })
                .unwrap();
            session
                .append(EventPayload::ContextCompacted {
                    tokens_before: 3_000,
                    tokens_after: 1_200,
                    strategy: strategy.to_string(),
                    summary: Some("- kept thread".to_string()),
                    first_kept_sequence: Some(1),
                })
                .unwrap();

            let conversation = AppService::new(db).session_conversation(&sid).unwrap();
            assert!(
                conversation.iter().any(|message| matches!(
                    message.parts.first(),
                    Some(ConversationPartDto::CompactBoundary { strategy: label, .. })
                        if label == strategy
                )),
                "a {strategy} boundary never reached the transcript: {conversation:?}"
            );
        }
    }

    #[test]
    fn metrics_only_compaction_records_render_no_row() {
        use deepagent_core::event::EventPayload;
        use deepagent_core::message::Message;

        let db = Database::open_in_memory().unwrap();
        let clock = FixedClock::new(1_000);
        let mut session = Session::create(&db, &clock, Some("micro")).unwrap();
        let sid = session.id().to_string();
        session
            .append(EventPayload::MessageAppended {
                message: Message::user("q"),
            })
            .unwrap();
        // Micro-compaction / stats records: `summary: None`.
        session
            .append(EventPayload::ContextCompacted {
                tokens_before: 500,
                tokens_after: 400,
                strategy: "micro_compact".to_string(),
                summary: None,
                first_kept_sequence: None,
            })
            .unwrap();

        let conversation = AppService::new(db).session_conversation(&sid).unwrap();
        assert!(
            !conversation.iter().any(|message| message.role == "system"),
            "internal accounting must not clutter the transcript: {conversation:?}"
        );
    }

    #[test]
    fn injected_memories_replay_as_their_own_row() {
        use crate::dto::ConversationPartDto;
        use deepagent_core::event::EventPayload;
        use deepagent_core::message::Message;

        let db = Database::open_in_memory().unwrap();
        let clock = FixedClock::new(1_000);
        let mut session = Session::create(&db, &clock, Some("memories")).unwrap();
        let sid = session.id().to_string();
        session
            .append(EventPayload::MessageAppended {
                message: Message::user("q"),
            })
            .unwrap();
        session
            .append(EventPayload::MemoriesInjected {
                ids: vec!["mem_1".to_string(), "mem_2".to_string()],
                count: 2,
                latency_ms: 37,
                trigger: "prefetch".to_string(),
            })
            .unwrap();

        let conversation = AppService::new(db).session_conversation(&sid).unwrap();
        let row = conversation
            .iter()
            .find(|message| message.role == "system")
            .expect("the injection must replay");
        assert!(
            matches!(
                row.parts.first(),
                Some(ConversationPartDto::Memories {
                    count: 2,
                    latency_ms: 37,
                    ..
                })
            ),
            "{:?}",
            row.parts
        );
        assert_eq!(
            row.parts.first().and_then(|part| match part {
                ConversationPartDto::Memories { ids, .. } => Some(ids.as_slice()),
                _ => None,
            }),
            Some(["mem_1".to_string(), "mem_2".to_string()].as_slice())
        );
    }

    #[test]
    fn session_conversation_projects_assistant_narration_between_tool_calls() {
        use deepagent_core::event::EventPayload;
        use deepagent_core::message::{Message, ToolCall};
        use deepagent_core::response_item::ResponseItem;

        let db = Database::open_in_memory().unwrap();
        let clock = FixedClock::new(1_000);
        let mut session = Session::create(&db, &clock, Some("narration")).unwrap();
        let sid = session.id().to_string();
        // `Session::append` also projects this into a provider message item;
        // that mirror must never open an assistant turn.
        session
            .append(EventPayload::MessageAppended {
                message: Message::user("weather?"),
            })
            .unwrap();
        // Narration before a tool call: persisted ONLY as a provider item.
        session
            .append_response_item(ResponseItem::Message {
                role: "assistant".into(),
                content: "which city? checking beijing".into(),
            })
            .unwrap();
        session
            .append_without_response_projection(EventPayload::ToolCallRequested {
                call: ToolCall {
                    id: "call_1".into(),
                    name: "web_search".into(),
                    arguments: serde_json::json!({"query": "北京 天气"}),
                },
            })
            .unwrap();
        session
            .append(EventPayload::ToolCallCompleted {
                call_id: "call_1".into(),
                ok: true,
                output: serde_json::json!({"count": 5}),
                duration_ms: 9,
            })
            .unwrap();
        session
            .append_response_item(ResponseItem::Message {
                role: "assistant".into(),
                content: "one moment".into(),
            })
            .unwrap();
        // The final answer exists as an item AND as the authoritative message.
        session
            .append_response_item(ResponseItem::Message {
                role: "assistant".into(),
                content: "cloudy today".into(),
            })
            .unwrap();
        session
            .append_without_response_projection(EventPayload::MessageAppended {
                message: Message::assistant("cloudy today"),
            })
            .unwrap();

        let conversation = AppService::new(db).session_conversation(&sid).unwrap();
        assert_eq!(conversation.len(), 2, "user turn + one assistant turn");
        let parts = &conversation[1].parts;
        assert!(
            matches!(
                &parts[..],
                [
                    ConversationPartDto::Text { text: before_tool },
                    ConversationPartDto::Tool { call_id, .. },
                    ConversationPartDto::Text { text: after_tool },
                    ConversationPartDto::Text { text: answer },
                ] if before_tool == "which city? checking beijing"
                    && call_id == "call_1"
                    && after_tool == "one moment"
                    && answer == "cloudy today"
            ),
            "parts: {parts:?}"
        );
        assert_eq!(conversation[1].content, "cloudy today");
    }

    #[test]
    fn session_conversation_projects_assistant_narration_after_a_projected_message() {
        use deepagent_core::event::EventPayload;
        use deepagent_core::message::Message;

        let db = Database::open_in_memory().unwrap();
        let clock = FixedClock::new(1_000);
        let mut session = Session::create(&db, &clock, Some("legacy order")).unwrap();
        let sid = session.id().to_string();
        session
            .append(EventPayload::MessageAppended {
                message: Message::user("hi"),
            })
            .unwrap();
        // The other write order: `Session::append` projects the message into an
        // item, so here the item FOLLOWS the message.
        session
            .append(EventPayload::MessageAppended {
                message: Message::assistant("final only"),
            })
            .unwrap();

        let conversation = AppService::new(db).session_conversation(&sid).unwrap();
        let parts = &conversation[1].parts;
        assert!(
            matches!(&parts[..], [ConversationPartDto::Text { text }] if text == "final only"),
            "parts: {parts:?}"
        );
    }

    #[test]
    fn session_conversation_parses_user_attachments() {
        let db = Database::open_in_memory().unwrap();
        let clock = FixedClock::new(1_000);
        let sid;
        {
            let mut s = Session::create(&db, &clock, Some("image")).unwrap();
            sid = s.id().to_string();
            s.append(deepagent_core::event::EventPayload::MessageAppended {
                message: deepagent_core::message::Message::user(
                    "check this\n\n<attachments>\n<attachment index=\"1\" name=\"shot.png\" type=\"image/png\" kind=\"image\">\nsource: paste\nsize: 42 bytes\npath: G:\\Temp\\shot.png\nsha256: abc123\n\nvisual text\n</attachment>\n</attachments>",
                ),
            })
            .unwrap();
        }

        let svc = AppService::new(db);
        let conversation = svc.session_conversation(&sid).unwrap();
        let user = conversation
            .iter()
            .find(|message| message.role == "user")
            .expect("user message with attachment");

        assert_eq!(user.attachments.len(), 1);
        let attachment = &user.attachments[0];
        assert_eq!(attachment.kind, "image");
        assert_eq!(attachment.name, "shot.png");
        assert_eq!(attachment.mime, "image/png");
        assert_eq!(attachment.size_bytes, 42);
        assert_eq!(
            attachment.original_path.as_deref(),
            Some("G:\\Temp\\shot.png")
        );
        assert_eq!(attachment.sha256.as_deref(), Some("abc123"));
    }

    #[test]
    fn unknown_session_errors() {
        let (svc, _) = seeded_service();
        let err = svc.session_detail("ses_00000000000000000000000000000000");
        assert!(err.is_err());
    }

    #[test]
    fn bad_id_errors() {
        let (svc, _) = seeded_service();
        assert!(svc.session_detail("not-a-valid-id-!!!").is_err());
    }

    #[test]
    fn commands_filterable() {
        let (svc, _) = seeded_service();
        assert!(!svc.commands("").is_empty());
        let hits = svc.commands("mcp");
        assert!(hits.iter().any(|c| c.id == "mcp.list"));
    }

    #[test]
    fn diff_reports_changes() {
        let (svc, _) = seeded_service();
        let d = svc.diff("a\nb\nc", "a\nB\nc");
        assert_eq!(d.added, 1);
        assert_eq!(d.removed, 1);
    }

    #[test]
    fn fork_creates_independent_branch() {
        let (svc, sid) = seeded_service();
        let before = svc.session_detail(&sid).unwrap();
        let last_seq = before.timeline.last().unwrap().sequence;

        let fork = svc.fork_session(&sid, last_seq).unwrap();
        assert_ne!(fork.new_session_id, sid);
        assert_eq!(fork.source_session_id, sid);

        // Both sessions now exist and the branch has the full prefix.
        let sessions = svc.list_sessions().unwrap();
        assert_eq!(sessions.len(), 2);
        let branch = svc.session_detail(&fork.new_session_id).unwrap();
        assert_eq!(branch.timeline.len(), before.timeline.len());
    }

    #[test]
    fn rewind_truncates_timeline() {
        let (svc, sid) = seeded_service();
        let before = svc.session_detail(&sid).unwrap();
        assert!(before.timeline.len() > 1);

        let res = svc.rewind_session(&sid, 0).unwrap();
        assert_eq!(res.kept_through, 0);
        assert!(res.events_removed > 0);

        let after = svc.session_detail(&sid).unwrap();
        assert_eq!(after.timeline.len(), 1);
    }

    #[test]
    #[ignore = "audit reproducer: rewinding must invalidate notes from the discarded branch"]
    fn audit_rewind_rejects_session_note_from_discarded_branch() {
        use crate::session_memory::{compaction_from_note, SessionMemoryState, SessionMemoryStore};
        use deepagent_core::{event::EventPayload, message::Message};

        let db = std::sync::Arc::new(Database::open_in_memory().unwrap());
        let clock = FixedClock::new(1_000);
        let mut session = Session::create(&db, &clock, Some("audit rewind notes")).unwrap();
        let id = session.id();
        for message in [
            Message::user("Stable task"),
            Message::assistant("Stable answer"),
        ] {
            session
                .append(EventPayload::MessageAppended { message })
                .unwrap();
        }
        let keep = EventStore::new(&db)
            .load_session(id)
            .unwrap()
            .last()
            .unwrap()
            .sequence;
        for message in [
            Message::user("Discarded decision: use RED"),
            Message::assistant("Old branch confirmed RED"),
        ] {
            session
                .append(EventPayload::MessageAppended { message })
                .unwrap();
        }
        let old_covered = EventStore::new(&db)
            .load_session(id)
            .unwrap()
            .last()
            .unwrap()
            .sequence;
        let notes = SessionMemoryStore::new(db.clone());
        // Seed a valid persisted note snapshot; this test concerns branch lineage,
        // not whether a model can produce a semantically correct summary.
        notes
            .save(
                id,
                &SessionMemoryState {
                    summary: "Discarded decision: use RED".into(),
                    last_summarized_sequence: Some(old_covered),
                    tool_cursor_sequence: Some(old_covered),
                    tokens_at_last_extraction: 20_000,
                    initialized: true,
                    updated_at: 1_000,
                },
            )
            .unwrap();
        drop(session);
        let service = AppService::from_shared(db.clone());
        service.rewind_session(&id.to_string(), keep).unwrap();
        let mut resumed = Session::recover(&db, &clock, id).unwrap();
        for message in [
            Message::user("Replacement decision: use BLUE"),
            Message::assistant("New branch confirmed BLUE"),
            Message::user("Continue only the BLUE branch"),
            Message::assistant("Ready to continue BLUE"),
        ] {
            resumed
                .append(EventPayload::MessageAppended { message })
                .unwrap();
        }
        let events = EventStore::new(&db).load_session(id).unwrap();
        let history = crate::input_runtime::model_history_from_events(&events);
        assert!(
            history.iter().all(|m| !m.content.contains("RED")),
            "rewind itself must have removed the discarded branch"
        );
        let choice = compaction_from_note(&notes, id, &events, &history);
        let dropped_new_content = choice
            .as_ref()
            .is_some_and(|(split, _)| history[..*split].iter().any(|m| m.content.contains("BLUE")));
        eprintln!("audit rewind note: keep={keep}, old_covered={old_covered}, current_tail={}, replacement={choice:?}, drops_new_branch={dropped_new_content}",
            events.last().unwrap().sequence);
        assert!(
            choice.is_none(),
            "stale note from the discarded branch was reused as current branch coverage"
        );
    }

    #[test]
    fn rewind_restores_files_from_checkpoints() {
        let db = std::sync::Arc::new(Database::open_in_memory().unwrap());
        let clock = FixedClock::new(1_000);
        let workspace = tempfile::tempdir().unwrap();
        let checkpoint_root = tempfile::tempdir().unwrap();
        let target = workspace.path().join("file.txt");
        std::fs::write(&target, "before").unwrap();

        let sid;
        let checkpoint_id;
        {
            let mut session =
                Session::create_in_project(&db, &clock, Some("restore"), Default::default(), None)
                    .unwrap();
            sid = session.id().to_string();
            session
                .append(deepagent_core::event::EventPayload::MessageAppended {
                    message: deepagent_core::message::Message::user("change file"),
                })
                .unwrap();
            let task = session.create_task("change file").unwrap();
            let sequence = deepagent_persistence::event_store::EventStore::new(&db)
                .load_session(session.id())
                .unwrap()
                .last()
                .unwrap()
                .sequence as i64;
            deepagent_persistence::run_store::RunStore::new(&db)
                .create("run_restore", &sid, Some(&task.to_string()), 1_000)
                .unwrap();
            let checkpoint = CheckpointManager::new(
                db.clone(),
                "run_restore",
                sequence,
                workspace.path(),
                checkpoint_root.path(),
            )
            .unwrap();
            checkpoint.capture_before(&target).unwrap();
            std::fs::write(&target, "after").unwrap();
            checkpoint_id = checkpoint.commit().unwrap();
        }

        let svc = AppService::from_shared(db);
        let result = svc.rewind_session(&sid, 0).unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "before");
        let target_display = target.to_string_lossy().to_string();
        assert!(result
            .restored_paths
            .iter()
            .any(|path| path == &target_display));
        assert!(!checkpoint_id.is_empty());
    }

    #[test]
    fn fork_restores_files_from_checkpoints() {
        let db = std::sync::Arc::new(Database::open_in_memory().unwrap());
        let clock = FixedClock::new(1_000);
        let workspace = tempfile::tempdir().unwrap();
        let checkpoint_root = tempfile::tempdir().unwrap();
        let target = workspace.path().join("forked.txt");
        std::fs::write(&target, "before fork point").unwrap();

        let sid;
        {
            let mut session =
                Session::create_in_project(&db, &clock, Some("fork"), Default::default(), None)
                    .unwrap();
            sid = session.id().to_string();
            session
                .append(deepagent_core::event::EventPayload::MessageAppended {
                    message: deepagent_core::message::Message::user("change file"),
                })
                .unwrap();
            let task = session.create_task("change file").unwrap();
            let sequence = deepagent_persistence::event_store::EventStore::new(&db)
                .load_session(session.id())
                .unwrap()
                .last()
                .unwrap()
                .sequence as i64;
            deepagent_persistence::run_store::RunStore::new(&db)
                .create("run_fork_restore", &sid, Some(&task.to_string()), 1_000)
                .unwrap();
            let checkpoint = CheckpointManager::new(
                db.clone(),
                "run_fork_restore",
                sequence,
                workspace.path(),
                checkpoint_root.path(),
            )
            .unwrap();
            checkpoint.capture_before(&target).unwrap();
            std::fs::write(&target, "after fork point").unwrap();
            checkpoint.commit().unwrap();
        }

        let svc = AppService::from_shared(db);
        let result = svc.fork_session(&sid, 0).unwrap();
        assert_ne!(result.new_session_id, sid);
        assert_eq!(
            std::fs::read_to_string(&target).unwrap(),
            "before fork point"
        );
        let target_display = target.to_string_lossy().to_string();
        assert!(result
            .restored_paths
            .iter()
            .any(|path| path == &target_display));

        let forked = svc.session_detail(&result.new_session_id).unwrap();
        assert_eq!(forked.timeline.len(), 1);
    }

    #[test]
    fn startup_recovery_finalizes_unfinished_runs_and_active_tasks() {
        let db = std::sync::Arc::new(Database::open_in_memory().unwrap());
        let clock = FixedClock::new(1_000);
        let sid;
        let task;
        {
            let mut session =
                Session::create_in_project(&db, &clock, Some("crash"), Default::default(), None)
                    .unwrap();
            sid = session.id().to_string();
            task = session.create_task("unfinished").unwrap();
            session.transition_task(task, TaskState::Running).unwrap();
        }
        let runs = RunStore::new(&db);
        runs.create("run_crashed", &sid, Some(&task.to_string()), 1_100)
            .unwrap();
        runs.transition("run_crashed", "running_turn", 1_200)
            .unwrap();

        // Seed control-plane rows that represent work interrupted by the
        // previous process. Startup recovery must reconcile these projections
        // in the same pass as the lifecycle run.
        let control = deepagent_persistence::run_control::RunControlStore::new(&db);
        control
            .create_action(&deepagent_persistence::run_control::NewRunAction {
                run_id: "run_crashed",
                turn_id: "turn_crashed",
                call_id: "call_running",
                sequence: 0,
                tool_name: "shell",
                arguments_hash: "hash-running",
                risk: "high",
                parent_action_id: None,
                now: 1_300,
            })
            .unwrap();
        control
            .transition_action(
                "run_crashed",
                "call_running",
                deepagent_persistence::run_control::RunActionState::Prepared,
                1_301,
                None,
                None,
            )
            .unwrap();
        control
            .transition_action(
                "run_crashed",
                "call_running",
                deepagent_persistence::run_control::RunActionState::Running,
                1_302,
                None,
                None,
            )
            .unwrap();
        control
            .create_action(&deepagent_persistence::run_control::NewRunAction {
                run_id: "run_crashed",
                turn_id: "turn_crashed",
                call_id: "call_approval",
                sequence: 1,
                tool_name: "filesystem.write",
                arguments_hash: "hash-approval",
                risk: "high",
                parent_action_id: None,
                now: 1_303,
            })
            .unwrap();
        control
            .transition_action(
                "run_crashed",
                "call_approval",
                deepagent_persistence::run_control::RunActionState::Prepared,
                1_304,
                None,
                None,
            )
            .unwrap();
        control
            .request_approval(&deepagent_persistence::run_control::NewRunApproval {
                approval_id: "approval_expired",
                run_id: "run_crashed",
                call_id: "call_approval",
                scope: "workspace",
                risk: "high",
                reason: Some("needs review"),
                policy_snapshot: None,
                expires_at: Some(1),
                now: 1_300,
            })
            .unwrap();
        control
            .acquire_lease(&deepagent_persistence::run_control::NewExecutionLease {
                lease_id: "lease_expired",
                resource_kind: "run",
                resource_id: "run_crashed",
                owner: "old-process",
                fencing_token_hash: "fence",
                expires_at: 1,
                now: 0,
            })
            .unwrap();

        let svc = AppService::from_shared(db.clone());
        let recovered = svc.recover_unfinished_runs().unwrap();
        assert_eq!(recovered.len(), 1);
        assert_eq!(recovered[0].run_id, "run_crashed");
        assert_eq!(recovered[0].terminal_kind, "failed");

        let actions = control.list_actions("run_crashed").unwrap();
        assert_eq!(
            actions[0].state,
            deepagent_persistence::run_control::RunActionState::Failed
        );
        let approvals = control.list_approvals("run_crashed").unwrap();
        assert_eq!(
            approvals[0].state,
            deepagent_persistence::run_control::ApprovalState::Expired
        );
        assert!(control
            .active_lease("run", "run_crashed", now_millis())
            .unwrap()
            .is_none());

        let record = RunStore::new(&db).get("run_crashed").unwrap().unwrap();
        assert_eq!(record.state, "terminal");
        assert_eq!(record.terminal_kind.as_deref(), Some("failed"));
        let events = RunStore::new(&db)
            .events_after("run_crashed", None)
            .unwrap();
        assert!(events
            .iter()
            .any(|event| event.event_type == "run_recovered_after_startup"));

        let session = Session::recover(&db, &clock, SessionId::from_str(&sid).unwrap()).unwrap();
        assert_eq!(session.state().task(task).unwrap().state, TaskState::Failed);
    }

    #[test]
    fn export_markdown_and_json() {
        let (svc, sid) = seeded_service();
        let md = svc.export_transcript(&sid, "markdown").unwrap();
        assert_eq!(md.extension, "md");
        assert!(md.content.starts_with("# demo"));

        let json = svc.export_transcript(&sid, "json").unwrap();
        assert_eq!(json.extension, "json");
        let parsed: serde_json::Value = serde_json::from_str(&json.content).unwrap();
        assert!(parsed.is_array());

        // Unknown format errors.
        assert!(svc.export_transcript(&sid, "pdf").is_err());
    }

    #[test]
    fn classify_interruption_detects_prompt_midturn_and_clean() {
        use deepagent_core::event::{Event, EventPayload};
        use deepagent_core::id::EventId;
        use deepagent_core::message::Message;
        let sid = SessionId::nil();
        let ev = |seq: u64, payload: EventPayload| Event {
            id: EventId::new(),
            session_id: sid,
            sequence: seq,
            timestamp: deepagent_core::clock::Timestamp::from_millis(seq as i64),
            payload,
        };

        // User message last → the prompt never got a reply.
        let events = vec![ev(
            0,
            EventPayload::MessageAppended {
                message: Message::user("hi"),
            },
        )];
        assert_eq!(classify_interruption(&events), Some("interrupted_prompt"));

        // Tool completion last → died mid-turn.
        let events = vec![
            ev(
                0,
                EventPayload::MessageAppended {
                    message: Message::assistant("calling a tool"),
                },
            ),
            ev(
                1,
                EventPayload::ToolCallCompleted {
                    call_id: "c1".into(),
                    ok: true,
                    output: serde_json::json!({ "ok": true }),
                    duration_ms: 1,
                },
            ),
        ];
        assert_eq!(classify_interruption(&events), Some("interrupted_turn"));

        // Assistant message last → ended cleanly between turns.
        let events = vec![ev(
            0,
            EventPayload::MessageAppended {
                message: Message::assistant("done"),
            },
        )];
        assert_eq!(classify_interruption(&events), None);

        // Empty log → unknown.
        assert_eq!(classify_interruption(&[]), None);
    }
}
