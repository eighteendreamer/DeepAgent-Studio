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

    /// Create a new session row. The caller still appends a
    /// `SessionStarted` event to the stream.
    pub fn create_session(&self, id: SessionId, title: Option<&str>, now: Timestamp) -> Result<()> {
        self.db.with_conn(|c| {
            c.execute(
                "INSERT INTO sessions (id, title, created_at, updated_at, ended_at)
                 VALUES (?1, ?2, ?3, ?3, NULL)",
                params![id.to_string(), title, now.as_millis()],
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
                "SELECT id, title, created_at, updated_at, ended_at
                 FROM sessions WHERE id = ?1",
                params![session_id.to_string()],
                |row| {
                    let id_str: String = row.get(0)?;
                    let title: Option<String> = row.get(1)?;
                    let created: i64 = row.get(2)?;
                    let updated: i64 = row.get(3)?;
                    let ended: Option<i64> = row.get(4)?;
                    Ok((id_str, title, created, updated, ended))
                },
            )
            .optional()
            .map_err(map_sqlite)?
            .map(|(id_str, title, created, updated, ended)| {
                let id = id_str
                    .parse::<SessionId>()
                    .map_err(|e| CoreError::Persistence(e.to_string()))?;
                Ok(SessionRecord {
                    id,
                    title,
                    created_at: Timestamp::from_millis(created),
                    updated_at: Timestamp::from_millis(updated),
                    ended_at: ended.map(Timestamp::from_millis),
                })
            })
            .transpose()
        })
    }

    /// List all sessions, most recently updated first.
    pub fn list_sessions(&self) -> Result<Vec<SessionRecord>> {
        self.db.with_conn(|c| {
            let mut stmt = c
                .prepare(
                    "SELECT id, title, created_at, updated_at, ended_at
                     FROM sessions ORDER BY updated_at DESC",
                )
                .map_err(map_sqlite)?;
            let rows = stmt
                .query_map([], |row| {
                    let id_str: String = row.get(0)?;
                    let title: Option<String> = row.get(1)?;
                    let created: i64 = row.get(2)?;
                    let updated: i64 = row.get(3)?;
                    let ended: Option<i64> = row.get(4)?;
                    Ok((id_str, title, created, updated, ended))
                })
                .map_err(map_sqlite)?;

            let mut out = Vec::new();
            for row in rows {
                let (id_str, title, created, updated, ended) = row.map_err(map_sqlite)?;
                let id = id_str
                    .parse::<SessionId>()
                    .map_err(|e| CoreError::Persistence(e.to_string()))?;
                out.push(SessionRecord {
                    id,
                    title,
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
                EventPayload::SessionStarted { title: None },
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
}
