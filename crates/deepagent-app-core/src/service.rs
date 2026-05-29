//! The application service: the single entry point the UI calls.
//!
//! [`AppService`] owns the [`Database`] and exposes high-level, DTO-returning
//! operations (list sessions, open a session with its timeline + stats). Tauri
//! commands (or a web handler) are thin wrappers over these methods.

use std::str::FromStr;

use deepagent_core::error::{CoreError, Result};
use deepagent_core::id::SessionId;
use deepagent_observation::{build_timeline, SessionStats};
use deepagent_persistence::event_store::EventStore;
use deepagent_persistence::Database;

use crate::commands::{builtin_commands, filter_commands};
use crate::diff::{diff_lines, DiffResult};
use crate::dto::{
    CommandDto, SessionDetailDto, SessionStatsDto, SessionSummaryDto, TimelineEntryDto,
};

/// The application service backing the UI.
pub struct AppService {
    db: Database,
}

impl AppService {
    /// Open the service over a database at `path` (created + migrated if new).
    pub fn open(path: impl AsRef<std::path::Path>) -> Result<Self> {
        Ok(Self {
            db: Database::open(path)?,
        })
    }

    /// Build over an existing database (e.g. in-memory for tests).
    pub fn new(db: Database) -> Self {
        Self { db }
    }

    /// Borrow the database (for callers that also run the runtime against it).
    pub fn database(&self) -> &Database {
        &self.db
    }

    /// List all sessions, newest first, as summaries for the sidebar.
    pub fn list_sessions(&self) -> Result<Vec<SessionSummaryDto>> {
        let store = EventStore::new(&self.db);
        let records = store.list_sessions()?;
        Ok(records
            .into_iter()
            .map(|r| SessionSummaryDto {
                id: r.id.to_string(),
                title: r.title,
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
                title: record.title,
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

    /// Compute a line-based diff between `old` and `new` for the Diff view.
    pub fn diff(&self, old: &str, new: &str) -> DiffResult {
        diff_lines(old, new)
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
}
