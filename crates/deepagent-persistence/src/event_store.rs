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
//! 2. Events are written to a per-session compressed JSONL file; SQLite keeps
//!    only queryable session metadata and a rebuildable file cursor.
//! 3. Appending and computing the next sequence are serialized per database,
//!    while one stable per-session OS lock covers file and projection commits
//!    across processes.

use deepagent_core::clock::Timestamp;
use deepagent_core::error::{CoreError, Result};
use deepagent_core::event::{Event, EventPayload, Sequence};
use deepagent_core::id::{EventId, SessionId};
use deepagent_core::message::Role;
use deepagent_core::session_mode::SessionMode;
use rusqlite::{params, OptionalExtension};
use sha2::{Digest, Sha256};

use crate::session_files::{self, SessionFileCursor, SessionFileHeader};
use crate::{map_sqlite, Database};

/// Existing repository facade over session metadata and file-backed events.
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

/// One conversation-body match projected from the rebuildable SQLite index.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionSearchHit {
    /// Existing session summary for the matched conversation.
    pub session: SessionRecord,
    /// Exact event position in the authoritative session file.
    pub sequence: Sequence,
    /// Message role when the indexed event is a conversation message.
    pub role: Option<Role>,
    /// Timestamp of the matched event.
    pub timestamp: Timestamp,
    /// Short excerpt reconstructed from the source event, not the FTS table.
    pub snippet: String,
}

/// Scope for a session-body query. `NoProject` is distinct from `All` so an
/// unscoped desktop conversation cannot search registered projects by default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionSearchScope<'a> {
    All,
    Project(&'a str),
    NoProject,
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
        let _io = self.db.lock_session_io()?;
        let _file_lock = session_files::lock_session(&self.db.session_files_root(), id)?;
        let created = session_files::create(
            &self.db.session_files_root(),
            SessionFileHeader {
                id,
                title: title.map(str::to_owned),
                mode,
                project: project.map(str::to_owned),
                created_at: now,
            },
        )?;
        let result =
            self.db.with_conn(|c| {
                let tx = c.unchecked_transaction().map_err(map_sqlite)?;
                tx.execute(
                "INSERT INTO sessions (id, title, mode, project, created_at, updated_at, ended_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?5, NULL)",
                params![id.to_string(), title, mode.label(), project, now.as_millis()],
            )
            .map_err(map_sqlite)?;
                tx.execute(
                    "INSERT INTO session_file_state
                    (session_id, generation, last_sequence, byte_size, updated_at)
                 VALUES (?1, ?2, NULL, ?3, ?4)",
                    params![
                        id.to_string(),
                        created.generation as i64,
                        created.byte_len as i64,
                        now.as_millis()
                    ],
                )
                .map_err(map_sqlite)?;
                tx.commit().map_err(map_sqlite)
            });
        if let Err(error) = &result {
            tracing::error!(%id, %error, "session file committed but metadata insert failed; retaining source for startup repair");
        }
        result
    }

    /// Append `payload` to `session_id`'s stream, returning the full [`Event`]
    /// (with its assigned sequence number and id).
    pub fn append(
        &self,
        session_id: SessionId,
        payload: EventPayload,
        now: Timestamp,
    ) -> Result<Event> {
        let _io = self.db.lock_session_io()?;
        let _file_lock = session_files::lock_session(&self.db.session_files_root(), session_id)?;
        if self.get_session(session_id)?.is_none() {
            return Err(CoreError::not_found(format!(
                "session {session_id} does not exist"
            )));
        }
        let cursor = self.projected_cursor(session_id)?;
        let search_cursor = self.db.with_conn(|c| {
            c.query_row(
                "SELECT generation, last_sequence, dirty
                 FROM session_search_cursors WHERE session_id = ?1",
                params![session_id.to_string()],
                |row| {
                    let generation: i64 = row.get(0)?;
                    let last_sequence: Option<i64> = row.get(1)?;
                    let dirty: i64 = row.get(2)?;
                    Ok((
                        generation as u64,
                        last_sequence.map(|value| value as u64),
                        dirty != 0,
                    ))
                },
            )
            .optional()
            .map_err(map_sqlite)
        })?;
        let candidate_sequence = cursor
            .and_then(|cursor| cursor.last_sequence)
            .map_or(0, |sequence| sequence + 1);
        let event = Event {
            id: EventId::new(),
            session_id,
            sequence: candidate_sequence,
            timestamp: now,
            payload,
        };
        let committed =
            session_files::append(&self.db.session_files_root(), session_id, event, cursor)?;
        let event = committed.event;
        if committed.recovered_tail_bytes > 0 {
            tracing::warn!(
                %session_id,
                recovered_bytes = committed.recovered_tail_bytes,
                "discarded an incomplete session-file tail before append"
            );
        }
        let search_is_incremental = match search_cursor {
            None => event.sequence == 0,
            Some((generation, last_sequence, dirty)) => {
                !dirty
                    && generation == committed.generation
                    && last_sequence.map_or(event.sequence == 0, |sequence| {
                        sequence + 1 == event.sequence
                    })
            }
        };
        let rebuild_events = if search_is_incremental {
            None
        } else {
            Some(
                session_files::load(&self.db.session_files_root(), session_id)?
                    .ok_or_else(|| {
                        CoreError::EventLog(format!(
                            "session {session_id} disappeared after event append"
                        ))
                    })?
                    .events,
            )
        };

        let projection_result = self.db.with_conn(|c| {
            let tx = c.unchecked_transaction().map_err(map_sqlite)?;
            tx.execute(
                "UPDATE sessions SET updated_at = ?2 WHERE id = ?1",
                params![session_id.to_string(), now.as_millis()],
            )
            .map_err(map_sqlite)?;
            if matches!(&event.payload, EventPayload::SessionEnded { .. }) {
                tx.execute(
                    "UPDATE sessions SET ended_at = ?2 WHERE id = ?1",
                    params![session_id.to_string(), now.as_millis()],
                )
                .map_err(map_sqlite)?;
            }
            tx.execute(
                "INSERT INTO session_file_state
                    (session_id, generation, last_sequence, byte_size, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(session_id) DO UPDATE SET
                    generation=excluded.generation,
                    last_sequence=excluded.last_sequence,
                    byte_size=excluded.byte_size,
                    updated_at=excluded.updated_at",
                params![
                    session_id.to_string(),
                    committed.generation as i64,
                    event.sequence as i64,
                    committed.byte_len as i64,
                    now.as_millis()
                ],
            )
            .map_err(map_sqlite)?;
            if let Some(events) = rebuild_events.as_deref() {
                replace_search_index(&tx, session_id, events)?;
            } else {
                index_search_event(&tx, &event)?;
            }
            tx.execute(
                "INSERT INTO session_search_cursors
                    (session_id, generation, last_sequence, dirty, updated_at)
                 VALUES (?1, ?2, ?3, 0, ?4)
                 ON CONFLICT(session_id) DO UPDATE SET
                    generation=excluded.generation,
                    last_sequence=excluded.last_sequence,
                    dirty=0,
                    updated_at=excluded.updated_at",
                params![
                    session_id.to_string(),
                    committed.generation as i64,
                    event.sequence as i64,
                    now.as_millis()
                ],
            )
            .map_err(map_sqlite)?;
            tx.commit().map_err(map_sqlite)
        });
        if let Err(error) = projection_result {
            tracing::error!(
                %session_id,
                sequence = event.sequence,
                %error,
                "session event committed but SQLite projection update failed"
            );
            if let Err(mark_error) = self.mark_search_dirty(
                session_id,
                committed.generation,
                search_cursor.and_then(|(_, sequence, _)| sequence),
                now,
            ) {
                tracing::error!(
                    %session_id,
                    %mark_error,
                    "failed to mark conversation search projection dirty"
                );
            }
        }
        Ok(event)
    }

    fn projected_cursor(&self, session_id: SessionId) -> Result<Option<SessionFileCursor>> {
        self.db.with_conn(|c| {
            c.query_row(
                "SELECT generation, last_sequence, byte_size
                 FROM session_file_state WHERE session_id = ?1",
                params![session_id.to_string()],
                |row| {
                    let generation: i64 = row.get(0)?;
                    let last_sequence: Option<i64> = row.get(1)?;
                    let byte_len: i64 = row.get(2)?;
                    Ok(SessionFileCursor {
                        generation: generation as u64,
                        last_sequence: last_sequence.map(|value| value as u64),
                        byte_len: byte_len as u64,
                    })
                },
            )
            .optional()
            .map_err(map_sqlite)
        })
    }

    /// Load all events for a session, ordered by sequence (for replay).
    pub fn load_session(&self, session_id: SessionId) -> Result<Vec<Event>> {
        self.read_from(session_id, 0)
    }

    /// Read events for a session starting at `from_sequence` (inclusive).
    /// Useful for incremental replay / tailing.
    pub fn read_from(&self, session_id: SessionId, from_sequence: Sequence) -> Result<Vec<Event>> {
        let _io = self.db.lock_session_io()?;
        let _file_lock = session_files::lock_session(&self.db.session_files_root(), session_id)?;
        let Some(loaded) = session_files::load(&self.db.session_files_root(), session_id)? else {
            return Ok(Vec::new());
        };
        verify_contiguous(&loaded.events, session_id, 0)?;
        if loaded.recovered_tail_bytes > 0 {
            tracing::warn!(
                %session_id,
                recovered_bytes = loaded.recovered_tail_bytes,
                "discarded an incomplete session-file tail after an interrupted write"
            );
        }
        Ok(loaded
            .events
            .into_iter()
            .filter(|event| event.sequence >= from_sequence)
            .collect())
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

    /// List active file-backed sessions, most recently updated first. Legacy
    /// rows without a source file and recycled sessions remain hidden from
    /// every caller, including CLI and harness thread listings.
    pub fn list_sessions(&self) -> Result<Vec<SessionRecord>> {
        let records = self.db.with_conn(|c| {
            let mut stmt = c
                .prepare(
                    "SELECT s.id, s.title, s.mode, s.project, s.created_at,
                            s.updated_at, s.ended_at
                     FROM sessions s
                     JOIN session_file_state f ON f.session_id = s.id
                     ORDER BY s.updated_at DESC",
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
        })?;
        let root = self.db.session_files_root();
        let mut visible = Vec::with_capacity(records.len());
        for record in records {
            if session_files::current_cursor(&root, record.id)?.is_some() {
                visible.push(record);
            }
        }
        Ok(visible)
    }

    /// Count of events in a session.
    pub fn event_count(&self, session_id: SessionId) -> Result<u64> {
        let _io = self.db.lock_session_io()?;
        let _file_lock = session_files::lock_session(&self.db.session_files_root(), session_id)?;
        let Some(physical) =
            session_files::current_cursor(&self.db.session_files_root(), session_id)?
        else {
            return Ok(0);
        };
        if let Some(projected) = self.projected_cursor(session_id)? {
            if projected.generation == physical.generation
                && projected.byte_len == physical.byte_len
            {
                return Ok(projected.last_sequence.map_or(0, |sequence| sequence + 1));
            }
        }
        Ok(
            session_files::load(&self.db.session_files_root(), session_id)?
                .map(|loaded| loaded.events.len() as u64)
                .unwrap_or(0),
        )
    }

    /// Atomically move a session's authoritative files into the managed
    /// recycle area. Metadata remains intact so archive restore stays on the
    /// existing session id and projection chain.
    pub fn trash_session(&self, session_id: SessionId, deleted_at: Timestamp) -> Result<bool> {
        let _io = self.db.lock_session_io()?;
        let _file_lock = session_files::lock_session(&self.db.session_files_root(), session_id)?;
        if self.get_session(session_id)?.is_none() {
            return Err(CoreError::not_found(format!(
                "session {session_id} does not exist"
            )));
        }
        session_files::move_to_trash(
            &self.db.session_files_root(),
            session_id,
            deleted_at.as_millis(),
        )
    }

    /// Ensure the authoritative file is active, restoring its newest recycled
    /// generation if needed. Returns false only when neither location has it.
    pub fn restore_trashed_session(&self, session_id: SessionId) -> Result<bool> {
        let _io = self.db.lock_session_io()?;
        let _file_lock = session_files::lock_session(&self.db.session_files_root(), session_id)?;
        if session_files::current_cursor(&self.db.session_files_root(), session_id)?.is_some() {
            return Ok(true);
        }
        session_files::restore_from_trash(&self.db.session_files_root(), session_id)
    }

    /// Inspect the recycle area for a session without altering its files.
    pub fn trashed_session_deleted_at(&self, session_id: SessionId) -> Result<Option<Timestamp>> {
        let _io = self.db.lock_session_io()?;
        let _file_lock = session_files::lock_session(&self.db.session_files_root(), session_id)?;
        if session_files::current_cursor(&self.db.session_files_root(), session_id)?.is_some() {
            return Ok(None);
        }
        Ok(
            session_files::latest_trash_timestamp(&self.db.session_files_root(), session_id)?
                .map(Timestamp::from_millis),
        )
    }

    /// Permanently remove recycled session directories older than `cutoff`.
    pub fn purge_trashed_sessions_before(&self, cutoff: Timestamp) -> Result<u64> {
        let _io = self.db.lock_session_io()?;
        session_files::purge_trash_before(&self.db.session_files_root(), cutoff.as_millis())
    }

    /// Remove the SQLite metadata and rebuildable projections for a session
    /// whose recycled source file has passed its retention window.
    pub fn purge_deleted_session_metadata(&self, session_id: SessionId) -> Result<bool> {
        self.db.with_conn(|c| {
            let tx = c.unchecked_transaction().map_err(map_sqlite)?;
            tx.execute(
                "DELETE FROM session_search_fts
                 WHERE CAST(doc_id AS INTEGER) IN
                    (SELECT id FROM session_search_docs WHERE session_id = ?1)",
                params![session_id.to_string()],
            )
            .map_err(map_sqlite)?;
            let deleted = tx
                .execute(
                    "DELETE FROM sessions WHERE id = ?1",
                    params![session_id.to_string()],
                )
                .map_err(map_sqlite)?;
            tx.commit().map_err(map_sqlite)?;
            Ok(deleted > 0)
        })
    }

    /// Search settled conversation content through the rebuildable FTS index.
    /// Exact snippets are reconstructed from the authoritative session file.
    pub fn search(
        &self,
        query: &str,
        scope: SessionSearchScope<'_>,
        limit: usize,
    ) -> Result<Vec<SessionSearchHit>> {
        let query = query.trim();
        if query.is_empty() {
            return Ok(Vec::new());
        }
        self.repair_search_indexes()?;
        let limit = limit.clamp(1, 100) as i64;
        let char_count = query.chars().count();
        let is_cjk_bigram = char_count == 2 && query.chars().all(is_cjk);
        let (project, no_project) = match scope {
            SessionSearchScope::All => (None, false),
            SessionSearchScope::Project(project) => (Some(project), false),
            SessionSearchScope::NoProject => (None, true),
        };
        let candidates: Vec<(SessionId, Sequence, Option<Role>, Timestamp)> =
            self.db.with_conn(|c| {
                let sql = if is_cjk_bigram {
                    "SELECT d.session_id, d.sequence, d.role, d.timestamp
                     FROM session_search_cjk_bigrams b
                     JOIN session_search_docs d ON d.id = b.doc_id
                     JOIN sessions s ON s.id = d.session_id
                     WHERE b.gram = ?1 AND (?2 IS NULL OR s.project = ?2)
                       AND (?4 = 0 OR s.project IS NULL)
                     ORDER BY d.timestamp DESC LIMIT ?3"
                } else if char_count >= 3 {
                    "SELECT d.session_id, d.sequence, d.role, d.timestamp
                     FROM session_search_fts f
                     JOIN session_search_docs d ON d.id = CAST(f.doc_id AS INTEGER)
                     JOIN sessions s ON s.id = d.session_id
                     WHERE session_search_fts MATCH ?1
                       AND (?2 IS NULL OR s.project = ?2)
                       AND (?4 = 0 OR s.project IS NULL)
                     ORDER BY bm25(session_search_fts), d.timestamp DESC LIMIT ?3"
                } else {
                    "SELECT d.session_id, d.sequence, d.role, d.timestamp
                     FROM session_search_fts f
                     JOIN session_search_docs d ON d.id = CAST(f.doc_id AS INTEGER)
                     JOIN sessions s ON s.id = d.session_id
                     WHERE f.content LIKE ?1 ESCAPE '\\'
                       AND (?2 IS NULL OR s.project = ?2)
                       AND (?4 = 0 OR s.project IS NULL)
                     ORDER BY d.timestamp DESC LIMIT ?3"
                };
                let search_term = if is_cjk_bigram {
                    query.to_owned()
                } else if char_count >= 3 {
                    format!("\"{}\"", query.replace('"', "\"\""))
                } else {
                    format!("%{}%", escape_like(query))
                };
                let mut stmt = c.prepare(sql).map_err(map_sqlite)?;
                let rows = stmt
                    .query_map(params![search_term, project, limit, no_project], |row| {
                        let id: String = row.get(0)?;
                        let sequence: i64 = row.get(1)?;
                        let role: Option<String> = row.get(2)?;
                        let timestamp: i64 = row.get(3)?;
                        Ok((id, sequence, role, timestamp))
                    })
                    .map_err(map_sqlite)?;
                let mut candidates = Vec::new();
                for row in rows {
                    let (id, sequence, role, timestamp) = row.map_err(map_sqlite)?;
                    candidates.push((
                        id.parse::<SessionId>()
                            .map_err(|error| CoreError::Persistence(error.to_string()))?,
                        sequence as Sequence,
                        role.as_deref().and_then(parse_role),
                        Timestamp::from_millis(timestamp),
                    ));
                }
                Ok(candidates)
            })?;

        let mut hits = Vec::with_capacity(candidates.len());
        for (session_id, sequence, role, timestamp) in candidates {
            let Some(session) = self.get_session(session_id)? else {
                continue;
            };
            let Some(event) = self.read_from(session_id, sequence)?.into_iter().next() else {
                continue;
            };
            if event.sequence != sequence {
                continue;
            }
            let Some((_, content)) = search_document(&event) else {
                continue;
            };
            hits.push(SessionSearchHit {
                session,
                sequence,
                role,
                timestamp,
                snippet: make_snippet(&content, query, 240),
            });
        }
        Ok(hits)
    }

    fn mark_search_dirty(
        &self,
        session_id: SessionId,
        generation: u64,
        last_sequence: Option<u64>,
        now: Timestamp,
    ) -> Result<()> {
        self.db.with_conn(|c| {
            c.execute(
                "INSERT INTO session_search_cursors
                    (session_id, generation, last_sequence, dirty, updated_at)
                 VALUES (?1, ?2, ?3, 1, ?4)
                 ON CONFLICT(session_id) DO UPDATE SET dirty=1, updated_at=excluded.updated_at",
                params![
                    session_id.to_string(),
                    generation as i64,
                    last_sequence.map(|value| value as i64),
                    now.as_millis()
                ],
            )
            .map_err(map_sqlite)?;
            Ok(())
        })
    }

    pub(crate) fn recover_orphaned_session_files(&self) -> Result<()> {
        for session_id in session_files::list_active_session_ids(&self.db.session_files_root())? {
            let _io = self.db.lock_session_io()?;
            let _file_lock =
                session_files::lock_session(&self.db.session_files_root(), session_id)?;
            if self.projected_cursor(session_id)?.is_some() {
                continue;
            }
            let Some(loaded) = session_files::load(&self.db.session_files_root(), session_id)?
            else {
                continue;
            };
            let updated_at = loaded
                .events
                .last()
                .map_or(loaded.header.created_at.as_millis(), |event| {
                    event.timestamp.as_millis()
                });
            let ended_at = loaded.events.iter().rev().find_map(|event| {
                matches!(event.payload, EventPayload::SessionEnded { .. })
                    .then_some(event.timestamp.as_millis())
            });
            self.db.with_conn(|c| {
                let tx = c.unchecked_transaction().map_err(map_sqlite)?;
                tx.execute(
                    "INSERT INTO sessions
                        (id, title, mode, project, created_at, updated_at, ended_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                     ON CONFLICT(id) DO UPDATE SET
                        title=excluded.title,
                        mode=excluded.mode,
                        project=excluded.project,
                        created_at=excluded.created_at,
                        updated_at=excluded.updated_at,
                        ended_at=excluded.ended_at",
                    params![
                        session_id.to_string(),
                        loaded.header.title,
                        loaded.header.mode.label(),
                        loaded.header.project,
                        loaded.header.created_at.as_millis(),
                        updated_at,
                        ended_at,
                    ],
                )
                .map_err(map_sqlite)?;
                tx.execute(
                    "INSERT INTO session_file_state
                        (session_id, generation, last_sequence, byte_size, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![
                        session_id.to_string(),
                        loaded.generation as i64,
                        loaded.events.last().map(|event| event.sequence as i64),
                        loaded.byte_len as i64,
                        updated_at,
                    ],
                )
                .map_err(map_sqlite)?;
                tx.commit().map_err(map_sqlite)
            })?;
            tracing::warn!(%session_id, "recovered session metadata from authoritative file");
        }
        Ok(())
    }

    pub(crate) fn repair_search_indexes(&self) -> Result<()> {
        let states: Vec<(SessionId, u64, u64, bool)> = self.db.with_conn(|c| {
            let mut stmt = c
                .prepare(
                    "SELECT f.session_id, f.generation, f.byte_size,
                            CASE WHEN c.session_id IS NULL OR c.dirty <> 0
                                OR c.generation <> f.generation
                                OR c.last_sequence IS NOT f.last_sequence
                            THEN 1 ELSE 0 END
                     FROM session_file_state f
                     LEFT JOIN session_search_cursors c ON c.session_id = f.session_id",
                )
                .map_err(map_sqlite)?;
            let rows = stmt
                .query_map([], |row| {
                    let id: String = row.get(0)?;
                    let generation: i64 = row.get(1)?;
                    let byte_size: i64 = row.get(2)?;
                    let dirty: i64 = row.get(3)?;
                    Ok((id, generation, byte_size, dirty))
                })
                .map_err(map_sqlite)?;
            let mut states = Vec::new();
            for row in rows {
                let (id, generation, byte_size, dirty) = row.map_err(map_sqlite)?;
                states.push((
                    id.parse::<SessionId>()
                        .map_err(|error| CoreError::Persistence(error.to_string()))?,
                    generation as u64,
                    byte_size as u64,
                    dirty != 0,
                ));
            }
            Ok(states)
        })?;
        for (session_id, projected_generation, projected_bytes, dirty) in states {
            let _io = self.db.lock_session_io()?;
            let _file_lock =
                session_files::lock_session(&self.db.session_files_root(), session_id)?;
            let physical =
                session_files::current_cursor(&self.db.session_files_root(), session_id)?;
            let projection_matches_file = physical.as_ref().is_some_and(|cursor| {
                cursor.generation == projected_generation && cursor.byte_len == projected_bytes
            });
            if !dirty && projection_matches_file {
                continue;
            }
            let Some(loaded) = session_files::load(&self.db.session_files_root(), session_id)?
            else {
                continue;
            };
            let updated_at = loaded
                .events
                .last()
                .map_or(loaded.header.created_at.as_millis(), |event| {
                    event.timestamp.as_millis()
                });
            let ended_at = loaded.events.iter().rev().find_map(|event| {
                matches!(event.payload, EventPayload::SessionEnded { .. })
                    .then_some(event.timestamp.as_millis())
            });
            self.db.with_conn(|c| {
                let tx = c.unchecked_transaction().map_err(map_sqlite)?;
                tx.execute(
                    "UPDATE session_file_state SET
                        generation = ?2, last_sequence = ?3, byte_size = ?4, updated_at = ?5
                     WHERE session_id = ?1",
                    params![
                        session_id.to_string(),
                        loaded.generation as i64,
                        loaded.events.last().map(|event| event.sequence as i64),
                        loaded.byte_len as i64,
                        updated_at,
                    ],
                )
                .map_err(map_sqlite)?;
                tx.execute(
                    "UPDATE sessions SET updated_at = ?2, ended_at = ?3 WHERE id = ?1",
                    params![session_id.to_string(), updated_at, ended_at],
                )
                .map_err(map_sqlite)?;
                replace_search_index(&tx, session_id, &loaded.events)?;
                tx.execute(
                    "INSERT INTO session_search_cursors
                        (session_id, generation, last_sequence, dirty, updated_at)
                     VALUES (?1, ?2, ?3, 0, ?4)
                     ON CONFLICT(session_id) DO UPDATE SET
                        generation=excluded.generation,
                        last_sequence=excluded.last_sequence,
                        dirty=0,
                        updated_at=excluded.updated_at",
                    params![
                        session_id.to_string(),
                        loaded.generation as i64,
                        loaded.events.last().map(|event| event.sequence as i64),
                        updated_at
                    ],
                )
                .map_err(map_sqlite)?;
                tx.commit().map_err(map_sqlite)
            })?;
            if let Err(error) = session_files::prune_older_generations(
                &self.db.session_files_root(),
                session_id,
                loaded.generation,
            ) {
                tracing::warn!(%session_id, %error, "could not remove obsolete session generations");
            }
        }
        Ok(())
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
        if events.is_empty() {
            return Err(CoreError::not_found(format!("session {source_id}")));
        }

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
        let _io = self.db.lock_session_io()?;
        let _file_lock = session_files::lock_session(&self.db.session_files_root(), session_id)?;
        if self.get_session(session_id)?.is_none() {
            return Err(CoreError::not_found(format!(
                "session {session_id} does not exist"
            )));
        }
        let (rewritten, removed) =
            session_files::rewrite_prefix(&self.db.session_files_root(), session_id, keep_through)?;
        let tail_ts = rewritten
            .events
            .last()
            .map(|event| event.timestamp.as_millis());
        let updated_at = tail_ts.unwrap_or(rewritten.header.created_at.as_millis());
        let last_sequence = rewritten.events.last().map(|event| event.sequence as i64);
        let projection_result = self.db.with_conn(|c| {
            let tx = c.unchecked_transaction().map_err(map_sqlite)?;
            tx.execute(
                "UPDATE sessions SET ended_at = NULL, updated_at = ?2 WHERE id = ?1",
                params![session_id.to_string(), updated_at],
            )
            .map_err(map_sqlite)?;
            tx.execute(
                "UPDATE session_file_state
                 SET generation = ?2, last_sequence = ?3, byte_size = ?4, updated_at = ?5
                 WHERE session_id = ?1",
                params![
                    session_id.to_string(),
                    rewritten.generation as i64,
                    last_sequence,
                    rewritten.byte_len as i64,
                    updated_at
                ],
            )
            .map_err(map_sqlite)?;
            replace_search_index(&tx, session_id, &rewritten.events)?;
            tx.execute(
                "INSERT INTO session_search_cursors
                    (session_id, generation, last_sequence, dirty, updated_at)
                 VALUES (?1, ?2, ?3, 0, ?4)
                 ON CONFLICT(session_id) DO UPDATE SET
                    generation=excluded.generation,
                    last_sequence=excluded.last_sequence,
                    dirty=0,
                    updated_at=excluded.updated_at",
                params![
                    session_id.to_string(),
                    rewritten.generation as i64,
                    last_sequence,
                    updated_at
                ],
            )
            .map_err(map_sqlite)?;
            tx.commit().map_err(map_sqlite)
        });
        if let Err(error) = projection_result {
            tracing::error!(
                %session_id,
                %error,
                "session rewind committed but SQLite projection update failed"
            );
        } else if let Err(error) = session_files::prune_older_generations(
            &self.db.session_files_root(),
            session_id,
            rewritten.generation,
        ) {
            tracing::warn!(%session_id, %error, "could not remove obsolete session generations");
        }
        Ok(removed)
    }
}

fn replace_search_index(
    connection: &rusqlite::Connection,
    session_id: SessionId,
    events: &[Event],
) -> Result<()> {
    connection
        .execute(
            "DELETE FROM session_search_fts
             WHERE CAST(doc_id AS INTEGER) IN
                (SELECT id FROM session_search_docs WHERE session_id = ?1)",
            params![session_id.to_string()],
        )
        .map_err(map_sqlite)?;
    connection
        .execute(
            "DELETE FROM session_search_cjk_bigrams
             WHERE doc_id IN
                (SELECT id FROM session_search_docs WHERE session_id = ?1)",
            params![session_id.to_string()],
        )
        .map_err(map_sqlite)?;
    connection
        .execute(
            "DELETE FROM session_search_docs WHERE session_id = ?1",
            params![session_id.to_string()],
        )
        .map_err(map_sqlite)?;
    for event in events {
        index_search_event(connection, event)?;
    }
    Ok(())
}

fn index_search_event(connection: &rusqlite::Connection, event: &Event) -> Result<()> {
    let Some((role, content)) = search_document(event) else {
        return Ok(());
    };
    if content.trim().is_empty() {
        return Ok(());
    }
    let content_hash = format!("{:x}", Sha256::digest(content.as_bytes()));
    let inserted = connection
        .execute(
            "INSERT OR IGNORE INTO session_search_docs
                (session_id, sequence, role, timestamp, content_hash)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                event.session_id.to_string(),
                event.sequence as i64,
                role.map(|value| value.as_str()),
                event.timestamp.as_millis(),
                content_hash
            ],
        )
        .map_err(map_sqlite)?;
    if inserted == 0 {
        return Ok(());
    }
    let doc_id: i64 = connection
        .query_row(
            "SELECT id FROM session_search_docs WHERE session_id = ?1 AND sequence = ?2",
            params![event.session_id.to_string(), event.sequence as i64],
            |row| row.get(0),
        )
        .map_err(map_sqlite)?;
    connection
        .execute(
            "INSERT INTO session_search_fts(doc_id, content) VALUES (?1, ?2)",
            params![doc_id, content],
        )
        .map_err(map_sqlite)?;
    let mut bigrams = std::collections::BTreeSet::new();
    let chars: Vec<char> = content.chars().collect();
    for pair in chars.windows(2) {
        if pair.iter().all(|character| is_cjk(*character)) {
            bigrams.insert(pair.iter().collect::<String>());
        }
    }
    for gram in bigrams {
        connection
            .execute(
                "INSERT OR IGNORE INTO session_search_cjk_bigrams(doc_id, gram)
                 VALUES (?1, ?2)",
                params![doc_id, gram],
            )
            .map_err(map_sqlite)?;
    }
    Ok(())
}

fn search_document(event: &Event) -> Option<(Option<Role>, String)> {
    match &event.payload {
        EventPayload::MessageAppended { message }
            if matches!(message.role, Role::User | Role::Assistant)
                || (message.role == Role::System
                    && message
                        .content
                        .starts_with("[Earlier conversation compacted")) =>
        {
            Some((Some(message.role), message.content.clone()))
        }
        EventPayload::ToolCallRequested { call } => {
            let arguments = json_shape(&call.arguments);
            Some((None, format!("tool {} requested {arguments}", call.name)))
        }
        EventPayload::ToolCallCompleted {
            call_id,
            ok,
            output,
            ..
        } => Some((
            None,
            format!(
                "tool call {call_id} {} {}{}",
                if *ok { "completed" } else { "failed" },
                json_shape(output),
                tool_output_preview(output)
                    .map(|preview| format!(" preview {preview}"))
                    .unwrap_or_default()
            ),
        )),
        _ => None,
    }
}

/// Only explicit status/summary fields are eligible for the FTS projection.
/// In particular, file contents, stdout, patches, and arbitrary result bodies
/// remain solely in the authoritative session file.
fn tool_output_preview(output: &serde_json::Value) -> Option<String> {
    let object = output.as_object()?;
    let text = ["summary", "message", "status"]
        .iter()
        .filter_map(|key| object.get(*key).and_then(serde_json::Value::as_str))
        .find(|value| !value.trim().is_empty())?;
    let mut redacted = String::new();
    let mut redact_next = false;
    for token in text.split_whitespace() {
        if !redacted.is_empty() {
            redacted.push(' ');
        }
        let lower = token.to_ascii_lowercase();
        let sensitive_label = [
            "api_key",
            "apikey",
            "password",
            "secret",
            "authorization",
            "token",
        ]
        .iter()
        .any(|secret| lower.contains(secret));
        if redact_next || lower.starts_with("sk-") || sensitive_label {
            redacted.push_str("<redacted>");
        } else {
            redacted.push_str(token);
        }
        redact_next = lower == "bearer"
            || (sensitive_label && (lower.ends_with(':') || lower.ends_with('=')));
    }
    let preview: String = redacted.chars().take(160).collect();
    (!preview.is_empty()).then_some(preview)
}

fn json_shape(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Object(map) => {
            // Arbitrary JSON keys can themselves contain credentials or private
            // data. Only fixed schema labels are safe to duplicate into FTS.
            const SAFE_KEYS: &[&str] = &[
                "path", "status", "summary", "message", "content", "error", "result", "ok",
                "count", "query", "limit",
            ];
            let keys: Vec<&str> = map
                .keys()
                .map(String::as_str)
                .filter(|key| SAFE_KEYS.contains(key))
                .collect();
            format!("object with {} fields [{}]", map.len(), keys.join(", "))
        }
        serde_json::Value::Array(values) => format!("array length {}", values.len()),
        serde_json::Value::String(_) => "string output".into(),
        serde_json::Value::Number(_) => "number output".into(),
        serde_json::Value::Bool(_) => "boolean output".into(),
        serde_json::Value::Null => "null output".into(),
    }
}

fn parse_role(role: &str) -> Option<Role> {
    match role {
        "system" => Some(Role::System),
        "user" => Some(Role::User),
        "assistant" => Some(Role::Assistant),
        "tool" => Some(Role::Tool),
        _ => None,
    }
}

fn is_cjk(character: char) -> bool {
    matches!(
        character,
        '\u{3400}'..='\u{4dbf}'
            | '\u{4e00}'..='\u{9fff}'
            | '\u{f900}'..='\u{faff}'
            | '\u{20000}'..='\u{2fa1f}'
    )
}

fn escape_like(query: &str) -> String {
    query
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

fn make_snippet(content: &str, query: &str, max_chars: usize) -> String {
    let content_chars: Vec<char> = content.chars().collect();
    if content_chars.len() <= max_chars {
        return content.to_owned();
    }
    let query_chars: Vec<char> = query.chars().collect();
    let lower_content: Vec<char> = content.to_lowercase().chars().collect();
    let lower_query: Vec<char> = query.to_lowercase().chars().collect();
    let match_start = if lower_query.is_empty() || lower_query.len() > lower_content.len() {
        0
    } else {
        lower_content
            .windows(lower_query.len())
            .position(|window| window == lower_query.as_slice())
            .unwrap_or(0)
    };
    let padding = max_chars.saturating_sub(query_chars.len()) / 2;
    let start = match_start.saturating_sub(padding);
    let end = (start + max_chars).min(content_chars.len());
    let mut snippet: String = content_chars[start..end].iter().collect();
    if start > 0 {
        snippet.insert(0, '…');
    }
    if end < content_chars.len() {
        snippet.push('…');
    }
    snippet
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
    fn disk_database_reopens_from_session_file_without_sqlite_event_bodies() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("deepagent.db");
        let session_id = SessionId::new();
        {
            let db = Database::open(&db_path).unwrap();
            let store = EventStore::new(&db);
            store
                .create_session(session_id, Some("disk"), Timestamp::from_millis(10))
                .unwrap();
            store
                .append(
                    session_id,
                    EventPayload::Note {
                        text: "persisted in file".into(),
                    },
                    Timestamp::from_millis(20),
                )
                .unwrap();
            let sqlite_events: i64 = db
                .with_conn(|connection| {
                    connection
                        .query_row("SELECT count(*) FROM events", [], |row| row.get(0))
                        .map_err(map_sqlite)
                })
                .unwrap();
            assert_eq!(sqlite_events, 0);
        }

        let reopened = Database::open(&db_path).unwrap();
        let events = EventStore::new(&reopened).load_session(session_id).unwrap();
        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0].payload,
            EventPayload::Note { text } if text == "persisted in file"
        ));
        assert!(temp.path().join("files").join("sessions").exists());
    }

    #[test]
    fn disk_databases_share_the_per_session_lock_for_concurrent_appends() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("state.sqlite3");
        let first = std::sync::Arc::new(Database::open(&db_path).unwrap());
        let session_id = SessionId::new();
        EventStore::new(&first)
            .create_session(session_id, Some("concurrent"), Timestamp::from_millis(1))
            .unwrap();
        let second = std::sync::Arc::new(Database::open(&db_path).unwrap());
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));

        let workers: Vec<_> = [first.clone(), second]
            .into_iter()
            .enumerate()
            .map(|(worker, db)| {
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    for index in 0..25 {
                        EventStore::new(&db)
                            .append(
                                session_id,
                                EventPayload::MessageAppended {
                                    message: Message::user(format!("worker-{worker}-{index}")),
                                },
                                Timestamp::from_millis(10 + index),
                            )
                            .unwrap();
                    }
                })
            })
            .collect();
        barrier.wait();
        for worker in workers {
            worker.join().unwrap();
        }

        let events = EventStore::new(&first).load_session(session_id).unwrap();
        assert_eq!(events.len(), 50);
        assert!(events
            .iter()
            .enumerate()
            .all(|(index, event)| event.sequence == index as u64));
    }

    #[test]
    fn reopen_repairs_projection_after_file_commit_before_sqlite_commit() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("state.sqlite3");
        let session_id = SessionId::new();
        {
            let db = Database::open(&db_path).unwrap();
            let store = EventStore::new(&db);
            store
                .create_session(session_id, Some("crash window"), Timestamp::from_millis(1))
                .unwrap();
            store
                .append(
                    session_id,
                    EventPayload::MessageAppended {
                        message: Message::user("before crash"),
                    },
                    Timestamp::from_millis(2),
                )
                .unwrap();
            let cursor = db
                .with_conn(|connection| {
                    connection
                        .query_row(
                            "SELECT generation, last_sequence, byte_size
                             FROM session_file_state WHERE session_id = ?1",
                            params![session_id.to_string()],
                            |row| {
                                Ok(SessionFileCursor {
                                    generation: row.get::<_, i64>(0)? as u64,
                                    last_sequence: row
                                        .get::<_, Option<i64>>(1)?
                                        .map(|value| value as u64),
                                    byte_len: row.get::<_, i64>(2)? as u64,
                                })
                            },
                        )
                        .map_err(map_sqlite)
                })
                .unwrap();
            let event = Event {
                id: EventId::new(),
                session_id,
                sequence: 0,
                timestamp: Timestamp::from_millis(3),
                payload: EventPayload::MessageAppended {
                    message: Message::assistant("projection crash marker"),
                },
            };
            {
                let _file_lock =
                    session_files::lock_session(&db.session_files_root(), session_id).unwrap();
                session_files::append(&db.session_files_root(), session_id, event, Some(cursor))
                    .unwrap();
            }
            assert_eq!(store.event_count(session_id).unwrap(), 2);
            // Deliberately omit every SQLite update, matching a process exit
            // after the authoritative frame fsync and before projection commit.
        }

        let reopened = Database::open(&db_path).unwrap();
        let store = EventStore::new(&reopened);
        assert_eq!(store.load_session(session_id).unwrap().len(), 2);
        assert_eq!(
            store.get_session(session_id).unwrap().unwrap().updated_at,
            Timestamp::from_millis(3)
        );
        let hits = store
            .search("projection crash marker", SessionSearchScope::All, 10)
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].sequence, 1);
    }

    #[test]
    fn reopen_recovers_session_created_before_metadata_commit() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("state.sqlite3");
        let session_id = SessionId::new();
        {
            let db = Database::open(&db_path).unwrap();
            let root = db.session_files_root();
            let _file_lock = session_files::lock_session(&root, session_id).unwrap();
            session_files::create(
                &root,
                SessionFileHeader {
                    id: session_id,
                    title: Some("orphaned creation".into()),
                    mode: SessionMode::Normal,
                    project: Some("G:/workspace/orphan".into()),
                    created_at: Timestamp::from_millis(42),
                },
            )
            .unwrap();
            assert!(EventStore::new(&db)
                .get_session(session_id)
                .unwrap()
                .is_none());
            // Simulate a process exit after the file fsync and before SQLite
            // inserts the session and its cursor.
        }

        let reopened = Database::open(&db_path).unwrap();
        let store = EventStore::new(&reopened);
        let record = store.get_session(session_id).unwrap().unwrap();
        assert_eq!(record.title.as_deref(), Some("orphaned creation"));
        assert_eq!(record.project.as_deref(), Some("G:/workspace/orphan"));
        assert_eq!(record.created_at, Timestamp::from_millis(42));
        assert_eq!(store.event_count(session_id).unwrap(), 0);
        let appended = store
            .append(
                session_id,
                EventPayload::MessageAppended {
                    message: Message::user("found after orphan recovery"),
                },
                Timestamp::from_millis(43),
            )
            .unwrap();
        assert_eq!(appended.sequence, 0);
        assert_eq!(
            store
                .search("orphan recovery", SessionSearchScope::All, 10)
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn failed_session_metadata_insert_keeps_file_and_repairs_existing_row_on_reopen() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("state.sqlite3");
        let session_id = SessionId::new();
        {
            let db = Database::open(&db_path).unwrap();
            db.with_conn(|connection| {
                connection
                    .execute(
                        "INSERT INTO sessions
                            (id, title, mode, project, created_at, updated_at, ended_at)
                         VALUES (?1, 'stale row', 'normal', NULL, 1, 1, NULL)",
                        params![session_id.to_string()],
                    )
                    .map_err(map_sqlite)?;
                Ok(())
            })
            .unwrap();
            let store = EventStore::new(&db);
            assert!(store
                .create_session(session_id, Some("file truth"), Timestamp::from_millis(2))
                .is_err());
            assert!(
                session_files::current_cursor(&db.session_files_root(), session_id)
                    .unwrap()
                    .is_some()
            );
        }

        let reopened = Database::open(&db_path).unwrap();
        let store = EventStore::new(&reopened);
        assert_eq!(
            store
                .get_session(session_id)
                .unwrap()
                .unwrap()
                .title
                .as_deref(),
            Some("file truth")
        );
        assert_eq!(
            store
                .append(
                    session_id,
                    EventPayload::Note {
                        text: "recovered".into()
                    },
                    Timestamp::from_millis(3),
                )
                .unwrap()
                .sequence,
            0
        );
    }

    #[test]
    fn trash_and_restore_preserve_the_same_session_history() {
        let (db, session_id, clock) = store_with_session();
        let store = EventStore::new(&db);
        store
            .append(
                session_id,
                EventPayload::MessageAppended {
                    message: Message::user("recover me"),
                },
                clock.now(),
            )
            .unwrap();
        assert!(store
            .trash_session(session_id, Timestamp::from_millis(2_000))
            .unwrap());
        assert!(store.load_session(session_id).unwrap().is_empty());
        assert!(store.restore_trashed_session(session_id).unwrap());
        let restored = store.load_session(session_id).unwrap();
        assert_eq!(restored.len(), 1);
        assert!(matches!(
            &restored[0].payload,
            EventPayload::MessageAppended { message } if message.content == "recover me"
        ));
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
    fn search_matches_chinese_english_paths_and_code_identifiers() {
        let db = Database::open_in_memory().unwrap();
        let store = EventStore::new(&db);
        let session_id = SessionId::new();
        store
            .create_session_full(
                session_id,
                Some("search"),
                SessionMode::Normal,
                Some("G:/workspace/project-a"),
                Timestamp::from_millis(1),
            )
            .unwrap();
        store
            .append(
                session_id,
                EventPayload::MessageAppended {
                    message: Message::user(
                        "请检查缓存稳定性，并查看 src/runtime/cache.rs 里的 parse_config 函数",
                    ),
                },
                Timestamp::from_millis(2),
            )
            .unwrap();
        store
            .append(
                session_id,
                EventPayload::MessageAppended {
                    message: Message::assistant("Cache prefix remains stable after the fix."),
                },
                Timestamp::from_millis(3),
            )
            .unwrap();

        for query in ["缓存", "稳定性", "prefix", "src/runtime", "parse_config"] {
            let hits = store
                .search(
                    query,
                    SessionSearchScope::Project("G:/workspace/project-a"),
                    10,
                )
                .unwrap();
            assert!(!hits.is_empty(), "query {query:?} should match");
            assert_eq!(hits[0].session.id, session_id);
            assert!(hits[0].snippet.contains(query) || query == "prefix");
        }
        assert!(store
            .search(
                "缓存",
                SessionSearchScope::Project("G:/workspace/other"),
                10
            )
            .unwrap()
            .is_empty());
    }

    #[test]
    fn tool_search_indexes_only_bounded_redacted_summary_not_result_body() {
        let db = Database::open_in_memory().unwrap();
        let store = EventStore::new(&db);
        let session_id = SessionId::new();
        store
            .create_session(session_id, Some("tool preview"), Timestamp::from_millis(1))
            .unwrap();
        store
            .append(
                session_id,
                EventPayload::ToolCallCompleted {
                    call_id: "call-1".into(),
                    ok: true,
                    output: serde_json::json!({
                        "summary": "review-marker Bearer secretvalue password=hunter2 api_key: keyvalue",
                        "content": "private-result-marker",
                        "sk-private-key-marker": "value"
                    }),
                    duration_ms: 1,
                },
                Timestamp::from_millis(2),
            )
            .unwrap();
        let hits = store
            .search("review-marker", SessionSearchScope::All, 10)
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert!(hits[0].snippet.contains("review-marker"));
        for secret in [
            "secretvalue",
            "hunter2",
            "keyvalue",
            "private-result-marker",
            "private-key-marker",
        ] {
            assert!(
                store
                    .search(secret, SessionSearchScope::All, 10)
                    .unwrap()
                    .is_empty(),
                "{secret} must not enter the search projection"
            );
        }
    }

    #[test]
    fn no_project_scope_applies_before_limit_for_all_search_query_lengths() {
        let db = Database::open_in_memory().unwrap();
        let store = EventStore::new(&db);
        for (index, project) in [None, Some("G:/workspace/project-a")]
            .into_iter()
            .enumerate()
        {
            let session_id = SessionId::new();
            store
                .create_session_full(
                    session_id,
                    Some("search"),
                    SessionMode::Normal,
                    project,
                    Timestamp::from_millis(index as i64 + 1),
                )
                .unwrap();
            store
                .append(
                    session_id,
                    EventPayload::MessageAppended {
                        message: Message::user("缓存 alpha z"),
                    },
                    Timestamp::from_millis(index as i64 + 3),
                )
                .unwrap();
        }
        for query in ["缓存", "alpha", "z"] {
            let hits = store
                .search(query, SessionSearchScope::NoProject, 1)
                .unwrap();
            assert_eq!(hits.len(), 1, "query {query:?}");
            assert_eq!(hits[0].session.project, None, "query {query:?}");
            assert_eq!(
                store
                    .search(query, SessionSearchScope::All, 10)
                    .unwrap()
                    .len(),
                2,
                "query {query:?}"
            );
        }
    }

    #[test]
    fn v20_rebuilds_tool_previews_from_files_without_rewriting_the_source() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("state.sqlite3");
        let session_id = SessionId::new();
        let source_bytes;
        {
            let db = Database::open(&db_path).unwrap();
            let store = EventStore::new(&db);
            store
                .create_session(session_id, Some("tool preview"), Timestamp::from_millis(1))
                .unwrap();
            store
                .append(
                    session_id,
                    EventPayload::ToolCallCompleted {
                        call_id: "call-1".into(),
                        ok: true,
                        output: serde_json::json!({"summary": "migration-preview-marker"}),
                        duration_ms: 1,
                    },
                    Timestamp::from_millis(2),
                )
                .unwrap();
            source_bytes = std::fs::read(
                session_files::session_dir(&db.session_files_root(), session_id)
                    .join("session.g000001.v1.jsonl.zst"),
            )
            .unwrap();
            db.with_conn(|connection| {
                connection
                    .execute_batch(
                        "UPDATE session_search_fts SET content = 'old tool shape';
                         PRAGMA user_version = 19;",
                    )
                    .map_err(map_sqlite)?;
                Ok(())
            })
            .unwrap();
        }

        let db = Database::open(&db_path).unwrap();
        let hits = EventStore::new(&db)
            .search("migration-preview-marker", SessionSearchScope::All, 10)
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].session.id, session_id);
        assert_eq!(
            std::fs::read(
                session_files::session_dir(&db.session_files_root(), session_id)
                    .join("session.g000001.v1.jsonl.zst")
            )
            .unwrap(),
            source_bytes
        );
    }

    #[test]
    fn deleted_search_projection_is_rebuilt_from_session_file() {
        let (db, session_id, clock) = store_with_session();
        let store = EventStore::new(&db);
        store
            .append(
                session_id,
                EventPayload::MessageAppended {
                    message: Message::user("rebuildable needle text"),
                },
                clock.now(),
            )
            .unwrap();
        assert_eq!(
            store
                .search("needle", SessionSearchScope::All, 10)
                .unwrap()
                .len(),
            1
        );
        db.with_conn(|connection| {
            connection
                .execute_batch(
                    "DELETE FROM session_search_fts;
                     DELETE FROM session_search_cjk_bigrams;
                     DELETE FROM session_search_docs;
                     DELETE FROM session_search_cursors;",
                )
                .map_err(map_sqlite)?;
            Ok(())
        })
        .unwrap();

        let rebuilt = store.search("needle", SessionSearchScope::All, 10).unwrap();
        assert_eq!(rebuilt.len(), 1);
        assert_eq!(rebuilt[0].session.id, session_id);
    }

    #[test]
    fn dropped_fts_table_is_recreated_and_reindexed_on_open() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("state.sqlite3");
        let session_id = SessionId::new();
        {
            let db = Database::open(&db_path).unwrap();
            let store = EventStore::new(&db);
            store
                .create_session(session_id, Some("search reset"), Timestamp::from_millis(1))
                .unwrap();
            store
                .append(
                    session_id,
                    EventPayload::MessageAppended {
                        message: Message::user("rebuild this search projection"),
                    },
                    Timestamp::from_millis(2),
                )
                .unwrap();
            assert_eq!(
                store
                    .search("projection", SessionSearchScope::All, 10)
                    .unwrap()
                    .len(),
                1
            );
            db.with_conn(|connection| {
                connection
                    .execute_batch("DROP TABLE session_search_fts;")
                    .map_err(map_sqlite)
            })
            .unwrap();
        }

        let reopened = Database::open(&db_path).unwrap();
        let hits = EventStore::new(&reopened)
            .search("projection", SessionSearchScope::All, 10)
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].session.id, session_id);
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
    fn list_sessions_excludes_legacy_rows_and_recycled_files() {
        let (db, active, clock) = store_with_session();
        let store = EventStore::new(&db);
        let legacy = SessionId::new();
        db.with_conn(|connection| {
            connection
                .execute(
                    "INSERT INTO sessions
                        (id, title, mode, project, created_at, updated_at, ended_at)
                     VALUES (?1, 'legacy', 'normal', NULL, 1, 1, NULL)",
                    params![legacy.to_string()],
                )
                .map_err(map_sqlite)?;
            Ok(())
        })
        .unwrap();
        let recycled = SessionId::new();
        store
            .create_session(recycled, Some("recycled"), clock.now())
            .unwrap();
        store.trash_session(recycled, clock.now()).unwrap();

        let listed = store.list_sessions().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, active);
        assert!(store.get_session(legacy).unwrap().is_some());
        assert!(store.get_session(recycled).unwrap().is_some());
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
        let dir = session_files::session_dir(&db.session_files_root(), sid);
        assert!(!dir.join("session.g000001.v1.jsonl.zst").exists());
        assert!(dir.join("session.g000002.v1.jsonl.zst").exists());

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
