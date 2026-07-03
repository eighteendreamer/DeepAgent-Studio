//! The application service: the single entry point the UI calls.
//!
//! [`AppService`] owns the [`Database`] and exposes high-level, DTO-returning
//! operations (list sessions, open a session with its timeline + stats). Tauri
//! commands (or a web handler) are thin wrappers over these methods.

use std::str::FromStr;

use deepagent_core::clock::{Clock, SystemClock};
use deepagent_core::error::{CoreError, Result};
use deepagent_core::id::SessionId;
use deepagent_observation::{build_timeline, export_transcript, SessionStats, TranscriptFormat};
use deepagent_persistence::cost_store::CostStore;
use deepagent_persistence::event_store::EventStore;
use deepagent_persistence::Database;
use deepagent_session::Session;

use crate::commands::{builtin_commands, commands_from_roots, filter_commands};
use crate::diff::{diff_lines, DiffResult};
use crate::dto::{
    CommandDto, ConversationMessageDto, ConversationPartDto, ConversationUsageDto, ForkResultDto,
    RewindResultDto, SessionDetailDto, SessionStatsDto, SessionSummaryDto, TimelineEntryDto,
    TranscriptDto,
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

    /// List all sessions, newest first, as summaries for the sidebar.
    pub fn list_sessions(&self) -> Result<Vec<SessionSummaryDto>> {
        let store = EventStore::new(&self.db);
        let archived = ArchiveService::new(self.db.clone()).archived_ids()?;
        let pinned = SessionStateService::new(self.db.clone()).pinned_ids()?;
        let projects = ProjectService::new(self.db.clone());
        let registered_projects = projects.registered_paths()?;
        let records = store.list_sessions()?;
        Ok(records
            .into_iter()
            .filter(|r| !archived.contains(&r.id.to_string()))
            .filter(|r| {
                r.project
                    .as_deref()
                    .map(|path| registered_projects.contains(path))
                    .unwrap_or(true)
            })
            .map(|r| SessionSummaryDto {
                id: r.id.to_string(),
                project: r.project.as_deref().map(|path| {
                    projects
                        .display_name(path)
                        .unwrap_or_else(|_| project_display_name(path))
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
                        messages.push(ConversationMessageDto {
                            role: "user".to_string(),
                            content: text.clone(),
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
                    messages[idx].parts.push(ConversationPartDto::Tool {
                        call_id: call.id.clone(),
                        name: call.name.clone(),
                        args: serde_json::to_string_pretty(&call.arguments)
                            .unwrap_or_else(|_| call.arguments.to_string()),
                        status: "running".to_string(),
                        duration_ms: None,
                        detail: None,
                        output: None,
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
                            duration_ms: dm,
                            detail,
                            output: tool_output,
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
                        }
                    }
                }
                EventPayload::UsageRecorded {
                    prompt_tokens,
                    completion_tokens,
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

    /// **Fork** a session at sequence `at_seq`: create a new sibling branch
    /// whose stream copies events `0..=at_seq` from the source. The source is
    /// left untouched (non-destructive branching). Returns the new branch id.
    pub fn fork_session(&self, session_id: &str, at_seq: u64) -> Result<ForkResultDto> {
        let id = SessionId::from_str(session_id)
            .map_err(|e| CoreError::invalid(format!("bad session id: {e}")))?;
        let clock = SystemClock;
        let forked = Session::fork(&self.db, &clock, id, at_seq)?;
        Ok(ForkResultDto {
            new_session_id: forked.id().to_string(),
            source_session_id: session_id.to_string(),
            forked_at: at_seq,
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
        let removed = session.rewind(to_seq)?;
        Ok(RewindResultDto {
            session_id: session_id.to_string(),
            kept_through: to_seq,
            events_removed: removed,
        })
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
