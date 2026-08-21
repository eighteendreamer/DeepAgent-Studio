//! Durable Agent Kernel v2 run state and ordered event ledger.

use deepagent_core::error::{CoreError, Result};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::{map_sqlite, Database};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunRecord {
    pub id: String,
    pub session_id: String,
    pub task_id: Option<String>,
    pub state: String,
    pub terminal_kind: Option<String>,
    pub terminal_reason: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub finished_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StoredRunEvent {
    pub run_id: String,
    pub sequence: u64,
    pub timestamp: i64,
    pub phase: String,
    pub status: String,
    pub event_type: String,
    pub data: serde_json::Value,
}

pub struct RunStore<'db> {
    db: &'db Database,
}

impl<'db> RunStore<'db> {
    pub const fn new(db: &'db Database) -> Self {
        Self { db }
    }

    pub fn create(
        &self,
        run_id: &str,
        session_id: &str,
        task_id: Option<&str>,
        now: i64,
    ) -> Result<()> {
        self.db.with_conn(|conn| {
            conn.execute(
                "INSERT INTO runs (id, session_id, task_id, state, created_at, updated_at) VALUES (?1, ?2, ?3, 'accepted', ?4, ?4)",
                params![run_id, session_id, task_id, now],
            )
            .map_err(map_sqlite)?;
            Ok(())
        })
    }

    pub fn transition(&self, run_id: &str, state: &str, now: i64) -> Result<()> {
        self.db.with_conn(|conn| {
            let changed = conn
                .execute(
                    "UPDATE runs SET state=?2, updated_at=?3 WHERE id=?1 AND finished_at IS NULL",
                    params![run_id, state, now],
                )
                .map_err(map_sqlite)?;
            if changed == 0 {
                return Err(CoreError::IllegalTransition {
                    from: "terminal_or_missing".into(),
                    to: state.into(),
                });
            }
            Ok(())
        })
    }

    pub fn finish(
        &self,
        run_id: &str,
        terminal_kind: &str,
        reason: Option<&str>,
        now: i64,
    ) -> Result<bool> {
        self.db.with_conn(|conn| {
            let changed = conn
                .execute(
                    "UPDATE runs SET state='terminal', terminal_kind=?2, terminal_reason=?3, updated_at=?4, finished_at=?4 WHERE id=?1 AND finished_at IS NULL",
                    params![run_id, terminal_kind, reason, now],
                )
                .map_err(map_sqlite)?;
            Ok(changed == 1)
        })
    }

    pub fn append_event(
        &self,
        run_id: &str,
        timestamp: i64,
        phase: &str,
        status: &str,
        event_type: &str,
        data: &serde_json::Value,
    ) -> Result<u64> {
        self.db.with_conn(|conn| {
            conn.execute_batch("BEGIN IMMEDIATE").map_err(map_sqlite)?;
            let result = (|| {
                let next: i64 = conn
                    .query_row(
                        "SELECT COALESCE(MAX(sequence), -1) + 1 FROM run_events WHERE run_id=?1",
                        [run_id],
                        |row| row.get(0),
                    )
                    .map_err(map_sqlite)?;
                conn.execute(
                    "INSERT INTO run_events (run_id, sequence, timestamp, phase, status, event_type, data) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    params![run_id, next, timestamp, phase, status, event_type, serde_json::to_string(data).map_err(|e| CoreError::Persistence(e.to_string()))?],
                )
                .map_err(map_sqlite)?;
                Ok(next as u64)
            })();
            match result {
                Ok(sequence) => {
                    conn.execute_batch("COMMIT").map_err(map_sqlite)?;
                    Ok(sequence)
                }
                Err(error) => {
                    let _ = conn.execute_batch("ROLLBACK");
                    Err(error)
                }
            }
        })
    }

    pub fn get(&self, run_id: &str) -> Result<Option<RunRecord>> {
        self.db.with_conn(|conn| {
            conn.query_row(
                "SELECT id, session_id, task_id, state, terminal_kind, terminal_reason, created_at, updated_at, finished_at FROM runs WHERE id=?1",
                [run_id],
                |row| {
                    Ok(RunRecord {
                        id: row.get(0)?,
                        session_id: row.get(1)?,
                        task_id: row.get(2)?,
                        state: row.get(3)?,
                        terminal_kind: row.get(4)?,
                        terminal_reason: row.get(5)?,
                        created_at: row.get(6)?,
                        updated_at: row.get(7)?,
                        finished_at: row.get(8)?,
                    })
                },
            )
            .optional()
            .map_err(map_sqlite)
        })
    }

    pub fn unfinished(&self) -> Result<Vec<RunRecord>> {
        self.db.with_conn(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT id, session_id, task_id, state, terminal_kind, terminal_reason, created_at, updated_at, finished_at
                     FROM runs
                     WHERE finished_at IS NULL
                     ORDER BY updated_at ASC, created_at ASC",
                )
                .map_err(map_sqlite)?;
            let rows = stmt
                .query_map([], |row| {
                    Ok(RunRecord {
                        id: row.get(0)?,
                        session_id: row.get(1)?,
                        task_id: row.get(2)?,
                        state: row.get(3)?,
                        terminal_kind: row.get(4)?,
                        terminal_reason: row.get(5)?,
                        created_at: row.get(6)?,
                        updated_at: row.get(7)?,
                        finished_at: row.get(8)?,
                    })
                })
                .map_err(map_sqlite)?;
            rows.collect::<std::result::Result<Vec<_>, _>>()
                .map_err(map_sqlite)
        })
    }

    /// List runs belonging to one persisted session, newest first.
    pub fn list_for_session(&self, session_id: &str) -> Result<Vec<RunRecord>> {
        self.db.with_conn(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT id, session_id, task_id, state, terminal_kind, terminal_reason,
                            created_at, updated_at, finished_at
                     FROM runs
                     WHERE session_id=?1
                     ORDER BY created_at DESC",
                )
                .map_err(map_sqlite)?;
            let rows = stmt
                .query_map([session_id], |row| {
                    Ok(RunRecord {
                        id: row.get(0)?,
                        session_id: row.get(1)?,
                        task_id: row.get(2)?,
                        state: row.get(3)?,
                        terminal_kind: row.get(4)?,
                        terminal_reason: row.get(5)?,
                        created_at: row.get(6)?,
                        updated_at: row.get(7)?,
                        finished_at: row.get(8)?,
                    })
                })
                .map_err(map_sqlite)?;
            rows.collect::<std::result::Result<Vec<_>, _>>()
                .map_err(map_sqlite)
        })
    }

    pub fn events_after(&self, run_id: &str, after: Option<u64>) -> Result<Vec<StoredRunEvent>> {
        self.db.with_conn(|conn| {
            let mut stmt = conn
                .prepare("SELECT run_id, sequence, timestamp, phase, status, event_type, data FROM run_events WHERE run_id=?1 AND sequence>?2 ORDER BY sequence")
                .map_err(map_sqlite)?;
            let rows = stmt
                .query_map(params![run_id, after.map(|v| v as i64).unwrap_or(-1)], |row| {
                    let data: String = row.get(6)?;
                    Ok(StoredRunEvent {
                        run_id: row.get(0)?,
                        sequence: row.get::<_, i64>(1)? as u64,
                        timestamp: row.get(2)?,
                        phase: row.get(3)?,
                        status: row.get(4)?,
                        event_type: row.get(5)?,
                        data: serde_json::from_str(&data).unwrap_or(serde_json::Value::Null),
                    })
                })
                .map_err(map_sqlite)?;
            rows.collect::<std::result::Result<Vec<_>, _>>()
                .map_err(map_sqlite)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_terminal_is_exactly_once_and_events_are_gapless() {
        let db = Database::open_in_memory().unwrap();
        db.with_conn(|conn| {
            conn.execute(
                "INSERT INTO sessions (id, created_at, updated_at) VALUES ('s1', 1, 1)",
                [],
            )
            .map_err(map_sqlite)?;
            Ok(())
        })
        .unwrap();
        let store = RunStore::new(&db);
        store.create("r1", "s1", Some("t1"), 1).unwrap();
        assert_eq!(
            store
                .append_event(
                    "r1",
                    2,
                    "preparing",
                    "started",
                    "phase",
                    &serde_json::json!({})
                )
                .unwrap(),
            0
        );
        assert_eq!(
            store
                .append_event(
                    "r1",
                    3,
                    "running_turn",
                    "started",
                    "phase",
                    &serde_json::json!({})
                )
                .unwrap(),
            1
        );
        assert!(store.finish("r1", "succeeded", None, 4).unwrap());
        assert!(!store.finish("r1", "failed", Some("late"), 5).unwrap());
        let record = store.get("r1").unwrap().unwrap();
        assert_eq!(record.terminal_kind.as_deref(), Some("succeeded"));
        assert_eq!(store.events_after("r1", None).unwrap().len(), 2);
        assert_eq!(store.list_for_session("s1").unwrap().len(), 1);
    }
}
