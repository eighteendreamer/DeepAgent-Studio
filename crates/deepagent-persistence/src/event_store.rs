//! The append-only event store.
//!
//! This is the durable backbone of the runtime (开发提示词.md §3). It exposes a
//! deliberately small surface:
//! - [`EventStore::create_session`] — register a session row.
//! - [`EventStore::append`] — append an event, assigning the next gapless
//!   sequence number atomically.
//! - [`EventStore::load_session`] / [`EventStore::read_from`] — read the stream
//!   back for replay.
//!
//! Invariants enforced here:
//! 1. Sequence numbers are contiguous and start at 0 per session.
//! 2. Events are never updated or deleted (append-only).
//! 3. Appending and computing the next sequence happen in one transaction, so
//!    concurrent appends cannot produce duplicate or gapped sequences.

use deepagent_core::clock::Timestamp;
use deepagent_core::error::{CoreError, Result};
use deepagent_core::event::{Event, EventPayload, Sequence};
use deepagent_core::id::{EventId, SessionId};
use deepagent_core::session_mode::SessionMode;
use rusqlite::{params, OptionalExtension};

use crate::{map_sqlite, Database};

/// Repository over the `sessions` + `events` tables.
pub struct EventStore<'db> {
    db: &'db Database,
}

/// A summary row describing a persisted session.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionRecord {
    /// Session id.
    pub id: SessionId,
    /// Optional title.
    pub title: Option<String>,
    /// The session run mode.
    pub mode: SessionMode,
    /// The project this session belongs to (absolute folder path), if any.
    pub project: Option<String>,
    /// Creation time.
    pub created_at: Timestamp,
    /// Last-updated time (time of most recent appended event).
    pub updated_at: Timestamp,
    /// Set once the session is ended.
    pub ended_at: Option<Timestamp>,
}

impl<'db> EventStore<'db> {
    /// Wrap a database handle.
    pub fn new(db: &'db Database) -> Self {
        Self { db }
    }

    /// Create a new session row in [`SessionMode::Normal`]. The caller still
    /// appends a `SessionStarted` event to the stream.
    pub fn create_session(&self, id: SessionId, title: Option<&str>, now: Timestamp) -> Result<()> {
        self.create_session_with_mode(id, title, SessionMode::Normal, now)
    }

    /// Create a new session row with an explicit run mode (no project).
    pub fn create_session_with_mode(
        &self,
        id: SessionId,
        title: Option<&str>,
        mode: SessionMode,
        now: Timestamp,
    ) -> Result<()> {
        self.create_session_full(id, title, mode, None, now)
    }

    /// Create a new session row with an explicit run mode and project. The
    /// caller still appends a `SessionStarted` event to the stream.
    pub fn create_session_full(
        &self,
        id: SessionId,
        title: Option<&str>,
        mode: SessionMode,
        project: Option<&str>,
        now: Timestamp,
    ) -> Result<()> {
        self.db.with_conn(|c| {
            c.execute(
                "INSERT INTO sessions (id, title, mode, project, created_at, updated_at, ended_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?5, NULL)",
                params![
                    id.to_string(),
                    title,
                    mode.label(),
                    project,
                    now.as_millis()
                ],
            )
            .map_err(map_sqlite)?;
            Ok(())
        })
    }

    /// Append `payload` to `session_id`'s stream, returning the full [`Event`]
    /// (with its assigned sequence number and id).
    pub fn append(
        &self,
        session_id: SessionId,
        payload: EventPayload,
        now: Timestamp,
    ) -> Result<Event> {
        let kind = payload.kind();
        let payload_json = serde_json::to_string(&payload)?;
        let event_id = EventId::new();

        self.db.with_conn(|c| {
            let tx = c.unchecked_transaction().map_err(map_sqlite)?;

            // Ensure the session exists.
            let exists: bool = tx
                .query_row(
                    "SELECT 1 FROM sessions WHERE id = ?1",
                    params![session_id.to_string()],
                    |_| Ok(true),
                )
                .optional()
                .map_err(map_sqlite)?
                .unwrap_or(false);
            if !exists {
                return Err(CoreError::not_found(format!(
                    "session {session_id} does not exist"
                )));
            }

            // Next gapless sequence = max(sequence)+1, or 0 if none.
            let next_seq: Sequence = tx
                .query_row(
                    "SELECT COALESCE(MAX(sequence) + 1, 0) FROM events WHERE session_id = ?1",
                    params![session_id.to_string()],
                    |row| row.get::<_, i64>(0),
                )
                .map_err(map_sqlite)? as Sequence;

            tx.execute(
                "INSERT INTO events (id, session_id, sequence, kind, timestamp, payload)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    event_id.to_string(),
                    session_id.to_string(),
                    next_seq as i64,
                    kind,
                    now.as_millis(),
                    payload_json,
                ],
            )
            .map_err(map_sqlite)?;

            // Touch the session's updated_at / ended_at.
            tx.execute(
                "UPDATE sessions SET updated_at = ?2 WHERE id = ?1",
                params![session_id.to_string(), now.as_millis()],
            )
            .map_err(map_sqlite)?;
            if matches!(payload, EventPayload::SessionEnded { .. }) {
                tx.execute(
                    "UPDATE sessions SET ended_at = ?2 WHERE id = ?1",
                    params![session_id.to_string(), now.as_millis()],
                )
                .map_err(map_sqlite)?;
            }

            tx.commit().map_err(map_sqlite)?;

            Ok(Event {
                id: event_id,
                session_id,
                sequence: next_seq,
                timestamp: now,
                payload,
            })
        })
    }

    /// Load all events for a session, ordered by sequence (for replay).
    pub fn load_session(&self, session_id: SessionId) -> Result<Vec<Event>> {
        self.read_from(session_id, 0)
    }

    /// Read events for a session starting at `from_sequence` (inclusive).
    /// Useful for incremental replay / tailing.
    pub fn read_from(&self, session_id: SessionId, from_sequence: Sequence) -> Result<Vec<Event>> {
        self.db.with_conn(|c| {
            let mut stmt = c
                .prepare(
                    "SELECT id, sequence, timestamp, payload
                     FROM events
                     WHERE session_id = ?1 AND sequence >= ?2
                     ORDER BY sequence ASC",
                )
                .map_err(map_sqlite)?;

            let rows = stmt
                .query_map(
                    params![session_id.to_string(), from_sequence as i64],
                    |row| {
                        let id_str: String = row.get(0)?;
                        let sequence: i64 = row.get(1)?;
                        let ts: i64 = row.get(2)?;
                        let payload_json: String = row.get(3)?;
                        Ok((id_str, sequence, ts, payload_json))
                    },
                )
                .map_err(map_sqlite)?;

            let mut events = Vec::new();
            for row in rows {
                let (id_str, sequence, ts, payload_json) = row.map_err(map_sqlite)?;
                let id = id_str
                    .parse::<EventId>()
                    .map_err(|e| CoreError::EventLog(e.to_string()))?;
                let payload: EventPayload = serde_json::from_str(&payload_json)?;
                events.push(Event {
                    id,
                    session_id,
                    sequence: sequence as Sequence,
                    timestamp: Timestamp::from_millis(ts),
                    payload,
                });
            }

            verify_contiguous(&events, session_id, from_sequence)?;
            Ok(events)
        })
    }

    /// Fetch the session record.
    pub fn get_session(&self, session_id: SessionId) -> Result<Option<SessionRecord>> {
        self.db.with_conn(|c| {
            c.query_row(
                "SELECT id, title, mode, project, created_at, updated_at, ended_at
                 FROM sessions WHERE id = ?1",
                params![session_id.to_string()],
                |row| {
                    let id_str: String = row.get(0)?;
                    let title: Option<String> = row.get(1)?;
                    let mode: String = row.get(2)?;
                    let project: Option<String> = row.get(3)?;
                    let created: i64 = row.get(4)?;
                    let updated: i64 = row.get(5)?;
                    let ended: Option<i64> = row.get(6)?;
                    Ok((id_str, title, mode, project, created, updated, ended))
                },
            )
            .optional()
            .map_err(map_sqlite)?
            .map(|(id_str, title, mode, project, created, updated, ended)| {
                let id = id_str
                    .parse::<SessionId>()
                    .map_err(|e| CoreError::Persistence(e.to_string()))?;
                Ok(SessionRecord {
                    id,
                    title,
                    mode: parse_mode(&mode),
                    project,
                    created_at: Timestamp::from_millis(created),
                    updated_at: Timestamp::from_millis(updated),
                    ended_at: ended.map(Timestamp::from_millis),
                })
            })
            .transpose()
        })
    }

    /// Update a session's display title.
    pub fn rename_session(
        &self,
        session_id: SessionId,
        title: Option<&str>,
        now: Timestamp,
    ) -> Result<bool> {
        self.db.with_conn(|c| {
            let changed = c
                .execute(
                    "UPDATE sessions SET title = ?2, updated_at = ?3 WHERE id = ?1",
                    params![session_id.to_string(), title, now.as_millis()],
                )
                .map_err(map_sqlite)?;
            Ok(changed > 0)
        })
    }

    /// List all sessions, most recently updated first.
    pub fn list_sessions(&self) -> Result<Vec<SessionRecord>> {
        self.db.with_conn(|c| {
            let mut stmt = c
                .prepare(
                    "SELECT id, title, mode, project, created_at, updated_at, ended_at
                     FROM sessions ORDER BY updated_at DESC",
                )
                .map_err(map_sqlite)?;
            let rows = stmt
                .query_map([], |row| {
                    let id_str: String = row.get(0)?;
                    let title: Option<String> = row.get(1)?;
                    let mode: String = row.get(2)?;
                    let project: Option<String> = row.get(3)?;
                    let created: i64 = row.get(4)?;
                    let updated: i64 = row.get(5)?;
                    let ended: Option<i64> = row.get(6)?;
                    Ok((id_str, title, mode, project, created, updated, ended))
                })
                .map_err(map_sqlite)?;

            let mut out = Vec::new();
            for row in rows {
                let (id_str, title, mode, project, created, updated, ended) =
                    row.map_err(map_sqlite)?;
                let id = id_str
                    .parse::<SessionId>()
                    .map_err(|e| CoreError::Persistence(e.to_string()))?;
                out.push(SessionRecord {
                    id,
                    title,
                    mode: parse_mode(&mode),
                    project,
                    created_at: Timestamp::from_millis(created),
                    updated_at: Timestamp::from_millis(updated),
                    ended_at: ended.map(Timestamp::from_millis),
                });
            }
            Ok(out)
        })
    }

    /// Count of events in a session.
    pub fn event_count(&self, session_id: SessionId) -> Result<u64> {
        self.db.with_conn(|c| {
            let n: i64 = c
                .query_row(
                    "SELECT count(*) FROM events WHERE session_id = ?1",
                    params![session_id.to_string()],
                    |r| r.get(0),
                )
                .map_err(map_sqlite)?;
            Ok(n as u64)
        })
    }

    /// Distinct project paths that have at least one session, each with its
    /// most-recent session update time (for ordering projects in the sidebar).
    /// Sessions with no project are excluded.
    pub fn distinct_projects(&self) -> Result<Vec<(String, Timestamp)>> {
        self.db.with_conn(|c| {
            let mut stmt = c
                .prepare(
                    "SELECT project, MAX(updated_at) AS last_updated
                     FROM sessions
                     WHERE project IS NOT NULL AND project <> ''
                     GROUP BY project
                     ORDER BY last_updated DESC",
                )
                .map_err(map_sqlite)?;
            let rows = stmt
                .query_map([], |row| {
                    let project: String = row.get(0)?;
                    let last: i64 = row.get(1)?;
                    Ok((project, last))
                })
                .map_err(map_sqlite)?;
            let mut out = Vec::new();
            for row in rows {
                let (project, last) = row.map_err(map_sqlite)?;
                out.push((project, Timestamp::from_millis(last)));
            }
            Ok(out)
        })
    }

    /// Copy events `0..=until_seq` of `source_id` into a freshly-created
    /// session `new_id`, preserving payloads, ordering, and original
    /// timestamps. The new session carries the source's title + run mode.
    ///
    /// This is the **non-destructive** storage primitive behind session
    /// *fork* (branching) and *rewind-to-new-branch*: the source stream is
    /// never touched, so the full history remains replayable.
    pub fn fork_session(
        &self,
        source_id: SessionId,
        new_id: SessionId,
        until_seq: Sequence,
        now: Timestamp,
    ) -> Result<()> {
        let record = self
            .get_session(source_id)?
            .ok_or_else(|| CoreError::not_found(format!("session {source_id} does not exist")))?;
        let events = self.load_session(source_id)?;

        // Create the new session row first (carry title + mode + project forward).
        self.create_session_full(
            new_id,
            record.title.as_deref(),
            record.mode,
            record.project.as_deref(),
            now,
        )?;

        // Copy the prefix, re-appending each payload with its original
        // timestamp so the forked timeline reads identically. `append` assigns
        // gapless sequences starting at 0, which mirrors the source ordering.
        for ev in events.iter().filter(|e| e.sequence <= until_seq) {
            self.append(new_id, ev.payload.clone(), ev.timestamp)?;
        }
        Ok(())
    }

    /// Discard every event of `session_id` with sequence strictly greater than
    /// `keep_through`, returning the number of events removed. Sequences
    /// `0..=keep_through` stay contiguous, so later appends continue gaplessly.
    ///
    /// This is the one deliberate exception to the append-only rule and is
    /// reachable only through an explicit, user-initiated **rewind**. The
    /// session's `ended_at` is cleared (the session is "reopened") and
    /// `updated_at` is reset to the timestamp of the new tail event.
    pub fn truncate_after(&self, session_id: SessionId, keep_through: Sequence) -> Result<u64> {
        self.db.with_conn(|c| {
            let tx = c.unchecked_transaction().map_err(map_sqlite)?;

            let exists: bool = tx
                .query_row(
                    "SELECT 1 FROM sessions WHERE id = ?1",
                    params![session_id.to_string()],
                    |_| Ok(true),
                )
                .optional()
                .map_err(map_sqlite)?
                .unwrap_or(false);
            if !exists {
                return Err(CoreError::not_found(format!(
                    "session {session_id} does not exist"
                )));
            }

            let deleted = tx
                .execute(
                    "DELETE FROM events WHERE session_id = ?1 AND sequence > ?2",
                    params![session_id.to_string(), keep_through as i64],
                )
                .map_err(map_sqlite)?;

            // The new tail timestamp (None if the session is now empty).
            let tail_ts: Option<i64> = tx
                .query_row(
                    "SELECT MAX(timestamp) FROM events WHERE session_id = ?1",
                    params![session_id.to_string()],
                    |row| row.get::<_, Option<i64>>(0),
                )
                .map_err(map_sqlite)?;

            if let Some(ts) = tail_ts {
                tx.execute(
                    "UPDATE sessions SET ended_at = NULL, updated_at = ?2 WHERE id = ?1",
                    params![session_id.to_string(), ts],
                )
                .map_err(map_sqlite)?;
            } else {
                tx.execute(
                    "UPDATE sessions SET ended_at = NULL WHERE id = ?1",
                    params![session_id.to_string()],
                )
                .map_err(map_sqlite)?;
            }

            tx.commit().map_err(map_sqlite)?;
            Ok(deleted as u64)
        })
    }
}

/// Parse a stored mode label back into [`SessionMode`], defaulting to Normal
/// for unknown/legacy values.
fn parse_mode(s: &str) -> SessionMode {
    match s {
        "resumed" => SessionMode::Resumed,
        "remote" => SessionMode::Remote,
        "direct_connect" => SessionMode::DirectConnect,
        "assistant_viewer" => SessionMode::AssistantViewer,
        "coordinator" => SessionMode::Coordinator,
        "background_task" => SessionMode::BackgroundTask,
        _ => SessionMode::Normal,
    }
}

/// Defensive check that the loaded stream has contiguous sequences starting
/// at `from`. Guards against corruption / partial writes.
fn verify_contiguous(events: &[Event], session_id: SessionId, from: Sequence) -> Result<()> {
    for (i, e) in events.iter().enumerate() {
        let expected = from + i as Sequence;
        if e.sequence != expected {
            return Err(CoreError::EventLog(format!(
                "sequence gap in session {session_id}: expected {expected}, found {}",
                e.sequence
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use deepagent_core::clock::{Clock, FixedClock};
    use deepagent_core::message::Message;

    fn store_with_session() -> (Database, SessionId, FixedClock) {
        let db = Database::open_in_memory().unwrap();
        let clock = FixedClock::new(1_000);
        let sid = SessionId::new();
        {
            let store = EventStore::new(&db);
            store
                .create_session(sid, Some("test"), clock.now())
                .unwrap();
        }
        (db, sid, clock)
    }

    #[test]
    fn append_assigns_gapless_sequences() {
        let (db, sid, clock) = store_with_session();
        let store = EventStore::new(&db);

        let e0 = store
            .append(
                sid,
                EventPayload::SessionStarted {
                    title: None,
                    mode: deepagent_core::session_mode::SessionMode::Normal,
                },
                clock.now(),
            )
            .unwrap();
        clock.advance(10);
        let e1 = store
            .append(
                sid,
                EventPayload::MessageAppended {
                    message: Message::user("hi"),
                },
                clock.now(),
            )
            .unwrap();

        assert_eq!(e0.sequence, 0);
        assert_eq!(e1.sequence, 1);
        assert_eq!(store.event_count(sid).unwrap(), 2);
    }

    #[test]
    fn load_returns_events_in_order() {
        let (db, sid, clock) = store_with_session();
        let store = EventStore::new(&db);
        for i in 0..5 {
            store
                .append(
                    sid,
                    EventPayload::Note {
                        text: format!("n{i}"),
                    },
                    clock.now(),
                )
                .unwrap();
            clock.advance(1);
        }
        let events = store.load_session(sid).unwrap();
        assert_eq!(events.len(), 5);
        for (i, e) in events.iter().enumerate() {
            assert_eq!(e.sequence, i as u64);
        }
    }

    #[test]
    fn read_from_offset() {
        let (db, sid, clock) = store_with_session();
        let store = EventStore::new(&db);
        for i in 0..5 {
            store
                .append(
                    sid,
                    EventPayload::Note {
                        text: format!("n{i}"),
                    },
                    clock.now(),
                )
                .unwrap();
        }
        let tail = store.read_from(sid, 3).unwrap();
        assert_eq!(tail.len(), 2);
        assert_eq!(tail[0].sequence, 3);
    }

    #[test]
    fn append_to_missing_session_fails() {
        let db = Database::open_in_memory().unwrap();
        let store = EventStore::new(&db);
        let err = store
            .append(
                SessionId::new(),
                EventPayload::Note { text: "x".into() },
                Timestamp::from_millis(1),
            )
            .unwrap_err();
        assert!(matches!(err, CoreError::NotFound(_)));
    }

    #[test]
    fn session_ended_sets_ended_at() {
        let (db, sid, clock) = store_with_session();
        let store = EventStore::new(&db);
        clock.advance(50);
        store
            .append(
                sid,
                EventPayload::SessionEnded {
                    reason: Some("done".into()),
                },
                clock.now(),
            )
            .unwrap();
        let rec = store.get_session(sid).unwrap().unwrap();
        assert!(rec.ended_at.is_some());
        assert_eq!(rec.title.as_deref(), Some("test"));
    }

    #[test]
    fn list_sessions_returns_created() {
        let (db, sid, _clock) = store_with_session();
        let store = EventStore::new(&db);
        let all = store.list_sessions().unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].id, sid);
    }

    #[test]
    fn fork_copies_prefix_into_new_session() {
        let (db, sid, clock) = store_with_session();
        let store = EventStore::new(&db);
        for i in 0..5 {
            store
                .append(
                    sid,
                    EventPayload::Note {
                        text: format!("n{i}"),
                    },
                    clock.now(),
                )
                .unwrap();
            clock.advance(1);
        }

        let new_id = SessionId::new();
        store.fork_session(sid, new_id, 2, clock.now()).unwrap();

        // Source untouched.
        assert_eq!(store.event_count(sid).unwrap(), 5);
        // Forked copy has events 0..=2 (3 events), contiguous from 0.
        let forked = store.load_session(new_id).unwrap();
        assert_eq!(forked.len(), 3);
        for (i, e) in forked.iter().enumerate() {
            assert_eq!(e.sequence, i as u64);
        }
        // Payloads preserved.
        assert!(matches!(
            &forked[0].payload,
            EventPayload::Note { text } if text == "n0"
        ));
        // Title + mode carried forward.
        let rec = store.get_session(new_id).unwrap().unwrap();
        assert_eq!(rec.title.as_deref(), Some("test"));
        // New session can keep appending gaplessly.
        let next = store
            .append(
                new_id,
                EventPayload::Note {
                    text: "branch".into(),
                },
                clock.now(),
            )
            .unwrap();
        assert_eq!(next.sequence, 3);
    }

    #[test]
    fn truncate_after_removes_tail_and_reopens() {
        let (db, sid, clock) = store_with_session();
        let store = EventStore::new(&db);
        for i in 0..5 {
            store
                .append(
                    sid,
                    EventPayload::Note {
                        text: format!("n{i}"),
                    },
                    clock.now(),
                )
                .unwrap();
            clock.advance(1);
        }
        // End it so we can verify ended_at gets cleared on rewind.
        store
            .append(
                sid,
                EventPayload::SessionEnded { reason: None },
                clock.now(),
            )
            .unwrap();
        assert!(store.get_session(sid).unwrap().unwrap().ended_at.is_some());

        let removed = store.truncate_after(sid, 2).unwrap();
        // Removed events 3,4,5 (the SessionEnded plus n3,n4).
        assert_eq!(removed, 3);

        let remaining = store.load_session(sid).unwrap();
        assert_eq!(remaining.len(), 3);
        for (i, e) in remaining.iter().enumerate() {
            assert_eq!(e.sequence, i as u64);
        }
        // Reopened.
        assert!(store.get_session(sid).unwrap().unwrap().ended_at.is_none());

        // Appending continues gaplessly from the kept tail.
        let next = store
            .append(
                sid,
                EventPayload::Note {
                    text: "after".into(),
                },
                clock.now(),
            )
            .unwrap();
        assert_eq!(next.sequence, 3);
    }

    #[test]
    fn truncate_after_missing_session_fails() {
        let db = Database::open_in_memory().unwrap();
        let store = EventStore::new(&db);
        let err = store.truncate_after(SessionId::new(), 0).unwrap_err();
        assert!(matches!(err, CoreError::NotFound(_)));
    }

    #[test]
    fn project_is_stored_and_grouped() {
        let db = Database::open_in_memory().unwrap();
        let clock = FixedClock::new(1_000);
        let store = EventStore::new(&db);

        let a = SessionId::new();
        let b = SessionId::new();
        let c = SessionId::new();
        store
            .create_session_full(
                a,
                Some("s1"),
                SessionMode::Normal,
                Some("/proj/x"),
                clock.now(),
            )
            .unwrap();
        clock.advance(10);
        store
            .create_session_full(
                b,
                Some("s2"),
                SessionMode::Normal,
                Some("/proj/x"),
                clock.now(),
            )
            .unwrap();
        clock.advance(10);
        store
            .create_session_full(
                c,
                Some("s3"),
                SessionMode::Normal,
                Some("/proj/y"),
                clock.now(),
            )
            .unwrap();

        // Each record carries its project.
        assert_eq!(
            store.get_session(a).unwrap().unwrap().project.as_deref(),
            Some("/proj/x")
        );

        // Distinct projects, most-recently-updated first → y (newer) before x.
        let projects = store.distinct_projects().unwrap();
        let names: Vec<&str> = projects.iter().map(|(p, _)| p.as_str()).collect();
        assert_eq!(names, vec!["/proj/y", "/proj/x"]);
    }

    #[test]
    fn legacy_session_has_no_project() {
        let (db, sid, clock) = store_with_session();
        let store = EventStore::new(&db);
        let _ = clock;
        // store_with_session used create_session (no project) → None.
        assert!(store.get_session(sid).unwrap().unwrap().project.is_none());
    }
}
