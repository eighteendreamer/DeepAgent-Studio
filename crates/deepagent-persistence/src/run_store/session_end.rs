//! Session closing is an explicit admission in the existing run ledger.
use super::*;

pub(super) const REQUESTED: &str = "session_end.requested";

fn require_idle(conn: &rusqlite::Connection, session_id: &str, owner: Option<&str>) -> Result<()> {
    let busy: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM runs WHERE session_id=?1 AND finished_at IS NULL
         AND (?2 IS NULL OR id<>?2))",
            params![session_id, owner],
            |row| row.get(0),
        )
        .map_err(map_sqlite)?;
    if busy {
        return Err(CoreError::invalid(
            "session is busy; stop the active turn before ending it",
        ));
    }
    Ok(())
}

impl RunStore<'_> {
    /// One publication authority predicate for the standalone control and the
    /// admitted host agent. Snapshots and arbitrary ordinary runs confer none.
    pub fn validate_session_end_publication(
        &self,
        session_id: &str,
        owner: Option<&str>,
    ) -> Result<()> {
        self.db.with_conn(|conn| {
            if let Some(owner) = owner {
                let authorized: bool = conn.query_row(
                    "SELECT EXISTS(SELECT 1 FROM runs r JOIN run_events e ON e.run_id=r.id
                     WHERE r.id=?1 AND r.session_id=?2 AND r.finished_at IS NULL AND e.event_type=?3)",
                    params![owner, session_id, REQUESTED], |row| row.get(0),
                ).map_err(map_sqlite)?;
                if !authorized {
                    return Err(CoreError::invalid("run does not own an admitted session end"));
                }
            }
            require_idle(conn, session_id, owner)
        })
    }
}

pub(super) fn validate_admission(
    conn: &rusqlite::Connection,
    session_id: &str,
    ending: bool,
) -> Result<()> {
    let ended: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sessions WHERE id=?1 AND ended_at IS NOT NULL)",
            [session_id],
            |row| row.get(0),
        )
        .map_err(map_sqlite)?;
    let requested: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM runs r JOIN run_events e ON e.run_id=r.id
         WHERE r.session_id=?1 AND e.event_type=?2)",
            params![session_id, REQUESTED],
            |row| row.get(0),
        )
        .map_err(map_sqlite)?;
    // A durable close intent remains fail-closed after a crash/storage error.
    // /end may retry publication; normal turns must not reopen it implicitly.
    if !ending && (ended || requested) {
        return Err(CoreError::invalid(
            "session has ended or is ending; create or fork a session to continue",
        ));
    }
    if ending {
        require_idle(conn, session_id, None)?;
    }
    Ok(())
}
