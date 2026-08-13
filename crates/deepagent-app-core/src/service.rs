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
use deepagent_persistence::event_store::EventStore;
use deepagent_persistence::run_store::RunStore;
use deepagent_persistence::Database;
use deepagent_runtime::{tool_ui_metadata, CheckpointManager};
use deepagent_session::Session;

use crate::commands::{builtin_commands, commands_from_roots, filter_commands};
use crate::diff::{diff_lines, DiffResult};
use crate::dto::{
    AttachmentDto, CommandDto, ConversationMessageDto, ConversationPartDto, ConversationUsageDto,
    ForkResultDto, RewindResultDto, RunRecoveryDto, SessionDetailDto, SessionStatsDto,
    SessionSummaryDto, TimelineEntryDto, TranscriptDto,
};
use crate::{ArchiveService, ProjectService, SessionStateService};

/// The application service backing the UI.
pub struct AppService {
    db: std::sync::Arc<Database>,
}

impl AppService {
    /// Open the service over a database at `path` (created + migrated if new).
    pub fn open(path: impl AsRef<std::path::Path>) -> Result<Self> {
        Ok(Self {
            db: std::sync::Arc::new(Database::open(path)?),
        })
    }

    /// Build over an existing database (e.g. in-memory for tests).
    pub fn new(db: Database) -> Self {
        Self {
            db: std::sync::Arc::new(db),
        }
    }

    /// Build over a shared database handle (so settings + sessions share one DB).
    pub fn from_shared(db: std::sync::Arc<Database>) -> Self {
        Self { db }
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
    pub fn recover_unfinished_runs(&self) -> Result<Vec<RunRecoveryDto>> {
        let clock = SystemClock;
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
            if let Ok(session_id) = SessionId::from_str(&run.session_id) {
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
                now_millis(),
                "finalizing",
                "completed",
                "run_recovered_after_startup",
                &serde_json::json!({
                    "session_id": run.session_id.clone(),
                    "task_id": run.task_id.clone(),
                    "previous_state": run.state.clone(),
                    "terminal_kind": "failed",
                    "reason": reason,
                    // Phase D: incremental checkpoint commits keep backups
                    // reachable across a crash. Surface them so the UI (or a
                    // later auto-resume) can offer rolling files back to the
                    // pre-run state via the standard rewind path.
                    "checkpoint_ids": checkpoint_ids,
                    "has_file_backups": !checkpoint_ids.is_empty(),
                }),
            )?;
            store.finish(&run.id, "failed", Some(&reason), now_millis())?;
            recovered.push(RunRecoveryDto {
                run_id: run.id,
                session_id: run.session_id,
                task_id: run.task_id,
                previous_state: run.state,
                terminal_kind: "failed".to_string(),
                terminal_reason: reason,
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
        Ok(records
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
            .map(|r| SessionSummaryDto {
                id: r.id.to_string(),
                project: r.project.as_deref().map(|path| {
                    project_names
                        .get(path)
                        .filter(|name| !name.trim().is_empty())
                        .cloned()
                        .unwrap_or_else(|| project_display_name(path))
                }),
                title: r.title,
                mode: r.mode.label().to_string(),
                created_at: r.created_at.as_millis(),
                updated_at: r.updated_at.as_millis(),
                ended: r.ended_at.is_some(),
                pinned: pinned.contains(&r.id.to_string()),
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

        Ok(SessionDetailDto {
            summary: SessionSummaryDto {
                id: record.id.to_string(),
                project: record.project.as_deref().map(|path| {
                    projects
                        .display_name(path)
                        .unwrap_or_else(|_| project_display_name(path))
                }),
                title: record.title,
                mode: record.mode.label().to_string(),
                created_at: record.created_at.as_millis(),
                updated_at: record.updated_at.as_millis(),
                ended: record.ended_at.is_some(),
                pinned: pinned.contains(&record.id.to_string()),
            },
            timeline,
            stats,
        })
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
    /// message turns (assistant text + its reasoning), and
    /// `ToolCallRequested`/`ToolCallCompleted` fold into a tool card attached
    /// (in chronological position) to the current assistant turn.
    pub fn session_conversation(&self, session_id: &str) -> Result<Vec<ConversationMessageDto>> {
        use deepagent_core::event::EventPayload;
        use deepagent_core::message::Role;

        let id = SessionId::from_str(session_id)
            .map_err(|e| CoreError::invalid(format!("bad session id: {e}")))?;
        let store = EventStore::new(&self.db);
        let events = store.load_session(id)?;
        let mut recorded_costs = CostStore::new(&self.db)
            .session_costs(session_id)?
            .into_iter();

        let mut messages: Vec<ConversationMessageDto> = Vec::new();
        // Index of tool parts by call_id within the current assistant turn, so a
        // completion updates the request card in place.
        let mut tool_pos: std::collections::HashMap<String, (usize, usize)> =
            std::collections::HashMap::new();

        // Ensure there is a trailing assistant turn to attach tool cards to.
        let ensure_assistant = |messages: &mut Vec<ConversationMessageDto>| -> usize {
            if let Some(last) = messages.last() {
                if last.role == "assistant" {
                    return messages.len() - 1;
                }
            }
            messages.push(ConversationMessageDto {
                role: "assistant".to_string(),
                content: String::new(),
                attachments: Vec::new(),
                parts: Vec::new(),
                usage: None,
            });
            messages.len() - 1
        };

        for ev in &events {
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
                        let attachments = parse_conversation_attachments(&text, session_id);
                        messages.push(ConversationMessageDto {
                            role: "user".to_string(),
                            content: text.clone(),
                            attachments,
                            parts: vec![ConversationPartDto::Text { text }],
                            usage: None,
                        });
                    }
                    Role::Assistant => {
                        let content = message.content.trim();
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
                        if let Some(r) = reasoning {
                            messages[idx].parts.push(ConversationPartDto::Reasoning {
                                text: r.to_string(),
                            });
                        }
                        if !content.is_empty() {
                            messages[idx].parts.push(ConversationPartDto::Text {
                                text: content.to_string(),
                            });
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
                EventPayload::ToolCallRequested { call } => {
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
                        cost_yuan: recorded_costs.next(),
                    });
                }
                _ => {}
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
                ConversationPartDto::Tool { .. } => true,
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
    fn lists_sessions() {
        let (svc, sid) = seeded_service();
        let sessions = svc.list_sessions().unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, sid);
        assert_eq!(sessions[0].title.as_deref(), Some("demo"));
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
    fn session_detail_has_timeline_and_stats() {
        let (svc, sid) = seeded_service();
        let detail = svc.session_detail(&sid).unwrap();
        assert_eq!(detail.summary.id, sid);
        assert!(!detail.timeline.is_empty());
        assert_eq!(detail.stats.tool_successes, 1);
        assert_eq!(detail.stats.tool_success_rate, Some(1.0));
        // Timeline is ordered by sequence.
        for w in detail.timeline.windows(2) {
            assert!(w[0].sequence < w[1].sequence);
        }
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

        let svc = AppService::from_shared(db.clone());
        let recovered = svc.recover_unfinished_runs().unwrap();
        assert_eq!(recovered.len(), 1);
        assert_eq!(recovered[0].run_id, "run_crashed");
        assert_eq!(recovered[0].terminal_kind, "failed");

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
}
