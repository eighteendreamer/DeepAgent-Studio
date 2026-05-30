//! The application service: the single entry point the UI calls.
//!
//! [`AppService`] owns the [`Database`] and exposes high-level, DTO-returning
//! operations (list sessions, open a session with its timeline + stats). Tauri
//! commands (or a web handler) are thin wrappers over these methods.

use std::str::FromStr;

use deepagent_core::clock::SystemClock;
use deepagent_core::error::{CoreError, Result};
use deepagent_core::id::SessionId;
use deepagent_observation::{build_timeline, export_transcript, SessionStats, TranscriptFormat};
use deepagent_persistence::event_store::EventStore;
use deepagent_persistence::Database;
use deepagent_session::Session;

use crate::commands::{builtin_commands, filter_commands};
use crate::diff::{diff_lines, DiffResult};
use crate::dto::{
    CommandDto, ForkResultDto, RewindResultDto, SessionDetailDto, SessionStatsDto,
    SessionSummaryDto, TimelineEntryDto, TranscriptDto,
};

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
        let records = store.list_sessions()?;
        Ok(records
            .into_iter()
            .map(|r| SessionSummaryDto {
                id: r.id.to_string(),
                project: r.project.as_deref().map(project_display_name),
                title: r.title,
                mode: r.mode.label().to_string(),
                created_at: r.created_at.as_millis(),
                updated_at: r.updated_at.as_millis(),
                ended: r.ended_at.is_some(),
            })
            .collect())
    }

    /// Open a session: its summary, full timeline, and aggregated stats.
    pub fn session_detail(&self, session_id: &str) -> Result<SessionDetailDto> {
        let id = SessionId::from_str(session_id)
            .map_err(|e| CoreError::invalid(format!("bad session id: {e}")))?;
        let store = EventStore::new(&self.db);
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
                project: record.project.as_deref().map(project_display_name),
                title: record.title,
                mode: record.mode.label().to_string(),
                created_at: record.created_at.as_millis(),
                updated_at: record.updated_at.as_millis(),
                ended: record.ended_at.is_some(),
            },
            timeline,
            stats,
        })
    }

    /// The command-palette commands, optionally filtered by a fuzzy `query`.
    pub fn commands(&self, query: &str) -> Vec<CommandDto> {
        filter_commands(query, &builtin_commands())
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
