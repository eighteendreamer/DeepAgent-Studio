//! Derived search health, owned by EventStore, never a substitute event log.
use super::*;
use deepagent_core::clock::{Clock, SystemClock};
use serde::{Deserialize, Serialize};

const HEALTH_COLLECTION: &str = "session_file_health_v1";

/// Cooperative controls for optional file maintenance. This is not a second
/// cancellation tree: async callers supply their existing invocation signal.
pub struct SessionSearchControl<'a> {
    pub(super) cancelled: &'a (dyn Fn() -> bool + Sync),
    pub(super) timeout: std::time::Duration,
}

impl<'a> SessionSearchControl<'a> {
    pub fn new(timeout: std::time::Duration, cancelled: &'a (dyn Fn() -> bool + Sync)) -> Self {
        Self { cancelled, timeout }
    }

    pub(super) fn file_read(&self, deadline: std::time::Instant) -> session_files::ReadControl<'_> {
        session_files::ReadControl::new(
            self.cancelled,
            Some(deadline),
            32 * 1024 * 1024,
            64 * 1024 * 1024,
        )
    }

    pub(super) fn check_cancelled(&self) -> Result<()> {
        if (self.cancelled)() {
            Err(CoreError::other("session_search_cancelled"))
        } else {
            Ok(())
        }
    }
}

impl Default for SessionSearchControl<'static> {
    fn default() -> Self {
        Self::new(std::time::Duration::from_secs(5), &|| false)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionSearchIssueKind {
    Corrupt,
    Missing,
    Unreadable,
    Busy,
    Deferred,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionSearchIssue {
    pub session_id: SessionId,
    pub kind: SessionSearchIssueKind,
}

#[derive(Debug, Default)]
pub struct SessionSearchReport {
    pub hits: Vec<SessionSearchHit>,
    pub unavailable: Vec<SessionSearchIssue>,
}

impl SessionSearchReport {
    pub fn require_complete(self) -> Result<Vec<SessionSearchHit>> {
        if self.unavailable.is_empty() {
            Ok(self.hits)
        } else {
            Err(CoreError::Persistence(format!(
                "session_search_incomplete: {} unavailable histories; use search_with_status for explicit partial results",
                self.unavailable.len()
            )))
        }
    }
}

impl EventStore<'_> {
    // File failures are isolated only at a file boundary. SQLite/schema,
    // cursor parsing and projection transaction failures still propagate.
    pub(crate) fn repair_search_indexes(
        &self,
        scope: SessionSearchScope<'_>,
        control: &SessionSearchControl<'_>,
        deadline: std::time::Instant,
    ) -> Result<()> {
        self.repair_session_projections(scope, true, control, deadline)
    }

    /// Startup reconciles authority metadata, not optional search indexes.
    /// Explicit searches still fail on projection/SQLite transaction errors.
    pub(crate) fn repair_metadata_on_open(&self) -> Result<()> {
        let control = SessionSearchControl::default();
        self.repair_session_projections(
            SessionSearchScope::All,
            false,
            &control,
            std::time::Instant::now() + control.timeout,
        )
    }

    fn repair_session_projections(
        &self,
        scope: SessionSearchScope<'_>,
        include_search: bool,
        control: &SessionSearchControl<'_>,
        deadline: std::time::Instant,
    ) -> Result<()> {
        control.check_cancelled()?;
        let (project, no_project) = match scope {
            SessionSearchScope::All => (None, false),
            SessionSearchScope::Project(project) => (Some(project), false),
            SessionSearchScope::NoProject => (None, true),
        };
        let states: Vec<(SessionId, u64, u64)> = self.db.with_maintenance_conn(|c| {
            // A derived round-robin position prevents repeated short searches
            // from always spending their maintenance budget on the first files.
            // It contains only an opaque session ID, not project/path/content.
            let after = if include_search {
                c.query_row("SELECT body FROM documents WHERE collection='session_search_maintenance_v1' AND id='cursor' AND length(CAST(body AS BLOB)) <= 128", [], |r| r.get::<_, String>(0))
                    .optional().map_err(map_sqlite)?
                    .and_then(|s| serde_json::from_str::<SessionId>(&s).ok())
                    .map(|id| id.to_string())
            } else { None };
            let mut stmt = c
                .prepare(
                    "SELECT f.session_id, f.generation, f.byte_size
                 FROM session_file_state f
                 JOIN sessions s ON s.id=f.session_id
                 WHERE (?1 IS NULL OR s.project=?1) AND (?2=0 OR s.project IS NULL)
                 ORDER BY CASE WHEN ?3 IS NULL OR f.session_id > ?3 THEN 0 ELSE 1 END, f.session_id",
                )
                .map_err(map_sqlite)?;
            let rows = stmt
                .query_map(params![project, no_project, after], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, i64>(2)?,
                    ))
                })
                .map_err(map_sqlite)?;
            let mut states = Vec::new();
            for row in rows {
                let (id, generation, bytes) = row.map_err(map_sqlite)?;
                states.push((
                    id.parse::<SessionId>()
                        .map_err(|e| CoreError::Persistence(e.to_string()))?,
                    generation as u64,
                    bytes as u64,
                ));
            }
            Ok(states)
        })?;
        let mut last_attempted = None;
        for (id, generation, bytes) in states {
            control.check_cancelled()?;
            if std::time::Instant::now() >= deadline {
                self.mark_search_unavailable(id, SessionSearchIssueKind::Deferred)?;
                continue;
            }
            last_attempted = Some(id);
            self.repair_search_file(
                id,
                generation,
                bytes,
                include_search,
                &control.file_read(deadline),
            )?;
        }
        if include_search {
            if let Some(id) = last_attempted {
                let now = SystemClock.now().as_millis();
                self.db.with_maintenance_conn(|c| {
                    c.execute("INSERT INTO documents (collection,id,body,created_at,updated_at)
                     VALUES ('session_search_maintenance_v1','cursor',?1,?2,?2)
                     ON CONFLICT(collection,id) DO UPDATE SET body=excluded.body,updated_at=excluded.updated_at",
                     params![serde_json::to_string(&id)?,now]).map_err(map_sqlite)?;
                    Ok(())
                })?;
            }
        }
        Ok(())
    }

    fn repair_search_file(
        &self,
        id: SessionId,
        generation: u64,
        bytes: u64,
        include_search: bool,
        control: &session_files::ReadControl<'_>,
    ) -> Result<()> {
        // A poisoned process-wide gate is not a fault of this authority file.
        let Some(_io) = self.db.try_lock_session_io()? else {
            return self.mark_search_unavailable(id, SessionSearchIssueKind::Busy);
        };
        let _file = match session_files::try_lock_session_scope(
            &self.db.session_files_root(),
            id,
            "lock",
            true,
        ) {
            Ok(Some(guard)) => guard,
            Ok(None) => return self.mark_search_unavailable(id, SessionSearchIssueKind::Busy),
            Err(error) => return self.quarantine_search_file(id, &error),
        };
        // Startup only reconciles authority metadata. Search must validate
        // actual frames even when length/generation/mtime are unchanged;
        // filesystem metadata is not an integrity proof.
        if !include_search {
            match session_files::current_cursor(&self.db.session_files_root(), id) {
                Ok(Some(c)) if c.generation == generation && c.byte_len == bytes => return Ok(()),
                Ok(_) => {}
                Err(error) => return self.quarantine_search_file(id, &error),
            }
        }
        // Stream each group into the existing projection transaction. Nothing
        // becomes visible until the complete authority (including final CRC)
        // validates; any source/visitor failure rolls this transaction back.
        let projection = self.db.with_maintenance_conn(|c| {
            control.checkpoint()?;
            let tx = c.unchecked_transaction().map_err(map_sqlite)?;
            if include_search {
                replace_search_index_controlled(&tx, id, &[], &|| control.checkpoint())?;
            }
            let loaded = match session_files::try_visit_for_maintenance(
                &self.db.session_files_root(),
                id,
                control,
                &mut |group, _| {
                    if include_search {
                        for event in group {
                            index_search_event_controlled(&tx, &event, &|| control.checkpoint())?;
                        }
                    }
                    Ok(())
                },
            ) {
                Ok(Some(loaded)) => loaded,
                // Drop the transaction before recording file-local health or
                // checking recycle state: both use this same connection owner.
                other => return Ok(other),
            };
            self.commit_search_projection(&tx, id, &loaded, include_search, control)?;
            tx.commit().map_err(map_sqlite)?;
            Ok(Ok(Some(loaded)))
        });
        let loaded = match projection {
            Ok(result) => result,
            Err(error) => {
                if let Some(reason) = control.stopped() {
                    return self.quarantine_maintenance_read(
                        id,
                        &session_files::MaintenanceReadError::Stopped(reason),
                    );
                }
                return Err(error);
            }
        };
        let loaded = match loaded {
            Ok(Some(loaded)) => loaded,
            Ok(None) => {
                match session_files::latest_trash_timestamp(&self.db.session_files_root(), id) {
                    Ok(Some(_)) => {
                        if include_search {
                            self.clear_recycled_search_projection(id)?;
                        }
                        return Ok(());
                    }
                    Ok(None) => {
                        return self.quarantine_search_file(
                            id,
                            &CoreError::not_found("session authority file unavailable"),
                        )
                    }
                    Err(error) => return self.quarantine_search_file(id, &error),
                }
            }
            Err(error) => return self.quarantine_maintenance_read(id, &error),
        };
        if let Err(error) = session_files::prune_older_generations(
            &self.db.session_files_root(),
            id,
            loaded.generation,
        ) {
            tracing::warn!(session_id=%id,%error,"could not remove obsolete session generations");
        }
        Ok(())
    }

    fn commit_search_projection(
        &self,
        tx: &rusqlite::Transaction<'_>,
        id: SessionId,
        loaded: &session_files::ScannedSessionFile,
        include_search: bool,
        control: &session_files::ReadControl<'_>,
    ) -> Result<()> {
        let updated_at = loaded.updated_at.as_millis();
        let ended_at = loaded.ended_at.map(|t| t.as_millis());
        tx.execute(
            "UPDATE session_file_state SET generation=?2, last_sequence=?3,
                    byte_size=?4, updated_at=?5 WHERE session_id=?1",
            params![
                id.to_string(),
                loaded.generation as i64,
                loaded.last_sequence.map(|s| s as i64),
                loaded.byte_len as i64,
                updated_at
            ],
        )
        .map_err(map_sqlite)?;
        tx.execute(
            "UPDATE sessions SET updated_at=?2, ended_at=?3 WHERE id=?1",
            params![id.to_string(), updated_at, ended_at],
        )
        .map_err(map_sqlite)?;
        if !include_search {
            // A successfully recovered authority frame invalidates, but
            // does not synchronously rebuild, the optional search cursor.
            tx.execute(
                "UPDATE session_search_cursors SET dirty=1 WHERE session_id=?1",
                [id.to_string()],
            )
            .map_err(map_sqlite)?;
            return control.checkpoint();
        }
        tx.execute(
            "INSERT INTO session_search_cursors
                    (session_id, generation, last_sequence, dirty, updated_at)
                 VALUES (?1, ?2, ?3, 0, ?4)
                 ON CONFLICT(session_id) DO UPDATE SET
                    generation=excluded.generation, last_sequence=excluded.last_sequence,
                    dirty=0, updated_at=excluded.updated_at",
            params![
                id.to_string(),
                loaded.generation as i64,
                loaded.last_sequence.map(|s| s as i64),
                updated_at
            ],
        )
        .map_err(map_sqlite)?;
        tx.execute(
            "DELETE FROM documents WHERE collection=?1 AND id=?2",
            params![HEALTH_COLLECTION, id.to_string()],
        )
        .map_err(map_sqlite)?;
        control.checkpoint()
    }

    pub(super) fn quarantine_search_file(&self, id: SessionId, error: &CoreError) -> Result<()> {
        let kind = match error {
            CoreError::EventLog(_) => SessionSearchIssueKind::Corrupt,
            CoreError::NotFound(_) => SessionSearchIssueKind::Missing,
            _ => SessionSearchIssueKind::Unreadable,
        };
        self.mark_search_unavailable(id, kind)
    }

    pub(super) fn quarantine_maintenance_read(
        &self,
        id: SessionId,
        error: &session_files::MaintenanceReadError,
    ) -> Result<()> {
        match error {
            session_files::MaintenanceReadError::Busy => {
                self.mark_search_unavailable(id, SessionSearchIssueKind::Busy)
            }
            session_files::MaintenanceReadError::Source(error) => {
                self.quarantine_search_file(id, error)
            }
            session_files::MaintenanceReadError::Stopped(session_files::ReadStop::Budget) => {
                self.mark_search_unavailable(id, SessionSearchIssueKind::Deferred)
            }
            session_files::MaintenanceReadError::Stopped(session_files::ReadStop::Cancelled) => {
                Err(CoreError::other("session_search_cancelled"))
            }
            session_files::MaintenanceReadError::Projection(error) => Err(CoreError::Persistence(
                format!("session search projection failed: {error}"),
            )),
        }
    }

    fn clear_recycled_search_projection(&self, id: SessionId) -> Result<()> {
        // Intentional recycle is a visibility transition, not missing/corrupt
        // authority. Retain the cursor dirty so restore revalidates the file.
        self.db.with_maintenance_conn(|c| {
            let tx = c.unchecked_transaction().map_err(map_sqlite)?;
            replace_search_index(&tx, id, &[])?;
            tx.execute(
                "DELETE FROM documents WHERE collection=?1 AND id=?2",
                params![HEALTH_COLLECTION, id.to_string()],
            )
            .map_err(map_sqlite)?;
            tx.execute(
                "UPDATE session_search_cursors SET dirty=1 WHERE session_id=?1",
                [id.to_string()],
            )
            .map_err(map_sqlite)?;
            tx.commit().map_err(map_sqlite)
        })
    }

    pub(super) fn mark_search_unavailable(
        &self,
        id: SessionId,
        kind: SessionSearchIssueKind,
    ) -> Result<()> {
        let issue = SessionSearchIssue {
            session_id: id,
            kind,
        };
        let body = serde_json::to_string(&issue)?;
        let now = SystemClock.now().as_millis();
        self.db.with_maintenance_conn(|c| {
            let tx = c.unchecked_transaction().map_err(map_sqlite)?;
            tx.execute(
                "INSERT INTO documents (collection, id, body, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?4)
                 ON CONFLICT(collection, id) DO UPDATE SET
                    body=excluded.body, updated_at=excluded.updated_at",
                params![HEALTH_COLLECTION, id.to_string(), body, now],
            )
            .map_err(map_sqlite)?;
            tx.execute(
                "UPDATE session_search_cursors SET dirty=1 WHERE session_id=?1",
                [id.to_string()],
            )
            .map_err(map_sqlite)?;
            tx.commit().map_err(map_sqlite)
        })?;
        // Never persist/log transcript contents, file paths or parser excerpts.
        tracing::warn!(session_id=%id,?kind,"session authority unavailable; search projection quarantined without changing source bytes");
        Ok(())
    }

    pub(super) fn search_file_issues(
        &self,
        scope: SessionSearchScope<'_>,
    ) -> Result<Vec<SessionSearchIssue>> {
        let (project, no_project) = match scope {
            SessionSearchScope::All => (None, false),
            SessionSearchScope::Project(p) => (Some(p), false),
            SessionSearchScope::NoProject => (None, true),
        };
        self.db.with_maintenance_conn(|c| {
            let mut stmt = c
                .prepare(
                    "SELECT h.body FROM documents h LEFT JOIN sessions s ON s.id=h.id
                 WHERE h.collection=?1 AND (?2 IS NULL OR s.project=?2)
                    AND (?3=0 OR (s.id IS NOT NULL AND s.project IS NULL)) ORDER BY h.id",
                )
                .map_err(map_sqlite)?;
            let rows = stmt
                .query_map(params![HEALTH_COLLECTION, project, no_project], |row| {
                    row.get::<_, String>(0)
                })
                .map_err(map_sqlite)?;
            let mut issues = Vec::new();
            for row in rows {
                issues.push(serde_json::from_str(&row.map_err(map_sqlite)?)?);
            }
            Ok(issues)
        })
    }

    /// Search-only best-effort source read. Ordinary replay remains strict.
    /// File failures are isolated here for both snippets and tool context;
    /// process-wide coordination and quarantine transaction failures propagate.
    pub(super) fn read_search_context(
        &self,
        session_id: SessionId,
        from_sequence: Sequence,
        through_sequence: Sequence,
        control: &session_files::ReadControl<'_>,
    ) -> Result<Option<Vec<Event>>> {
        let mut context = Vec::new();
        let available = self.visit_available_authority(session_id, control, &mut |group, _| {
            context.extend(
                group
                    .into_iter()
                    .filter(|event| (from_sequence..=through_sequence).contains(&event.sequence)),
            );
            Ok(())
        })?;
        Ok(available.then_some(context))
    }

    /// One file-local isolation boundary for optional readers and owned startup
    /// recovery. Keep a visitor's results private until the entire walk passes.
    pub(super) fn visit_available_authority(
        &self,
        session_id: SessionId,
        control: &session_files::ReadControl<'_>,
        visitor: &mut session_files::scan::GroupVisitor<'_>,
    ) -> Result<bool> {
        let Some(_io) = self.db.try_lock_session_io()? else {
            self.mark_search_unavailable(session_id, SessionSearchIssueKind::Busy)?;
            return Ok(false);
        };
        let _file = match session_files::try_lock_session_scope(
            &self.db.session_files_root(),
            session_id,
            "lock",
            true,
        ) {
            Ok(Some(guard)) => guard,
            Ok(None) => {
                self.mark_search_unavailable(session_id, SessionSearchIssueKind::Busy)?;
                return Ok(false);
            }
            Err(error) => {
                self.quarantine_search_file(session_id, &error)?;
                return Ok(false);
            }
        };
        let loaded = (|| {
            let loaded = match session_files::try_visit_for_maintenance(
                &self.db.session_files_root(),
                session_id,
                control,
                visitor,
            )? {
                Some(loaded) => loaded,
                None if session_files::latest_trash_timestamp(
                    &self.db.session_files_root(),
                    session_id,
                )
                .map_err(session_files::MaintenanceReadError::Source)?
                .is_some() =>
                {
                    return Ok(None)
                }
                None => {
                    return Err(session_files::MaintenanceReadError::Source(
                        CoreError::not_found("session authority file unavailable"),
                    ))
                }
            };
            Ok(Some(loaded))
        })();
        match loaded {
            Ok(None) => Ok(false),
            Ok(Some(_)) => Ok(true),
            Err(error) => {
                self.quarantine_maintenance_read(session_id, &error)?;
                Ok(false)
            }
        }
    }
}
