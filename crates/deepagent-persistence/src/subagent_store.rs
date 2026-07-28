//! Durable sub-agent run metadata.

use deepagent_core::error::{CoreError, Result};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::{map_sqlite, Database};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubagentRunRecord {
    pub id: String,
    pub parent_run_id: String,
    pub origin_parent_run_id: String,
    pub state: String,
    pub agent_type: String,
    pub transcript_path: Option<String>,
    pub worktree_path: Option<String>,
    pub summary: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub finished_at: Option<i64>,
    pub resume_count: u32,
}

pub struct SubagentRunStore<'db> {
    db: &'db Database,
}

impl<'db> SubagentRunStore<'db> {
    pub const fn new(db: &'db Database) -> Self {
        Self { db }
    }

    pub fn create(&self, record: &SubagentRunRecord) -> Result<()> {
        self.db.with_conn(|connection| {
            connection
                .execute(
                    "INSERT INTO subagent_runs (id, parent_run_id, origin_parent_run_id, state, agent_type, transcript_path, worktree_path, summary, created_at, updated_at, finished_at, resume_count) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                    params![
                        record.id,
                        record.parent_run_id,
                        record.origin_parent_run_id,
                        record.state,
                        record.agent_type,
                        record.transcript_path,
                        record.worktree_path,
                        record.summary,
                        record.created_at,
                        record.updated_at,
                        record.finished_at,
                        record.resume_count,
                    ],
                )
                .map_err(map_sqlite)?;
            Ok(())
        })
    }

    pub fn finish(&self, id: &str, state: &str, summary: Option<&str>, at: i64) -> Result<()> {
        if !matches!(state, "succeeded" | "failed" | "cancelled") {
            return Err(CoreError::invalid(format!(
                "invalid terminal subagent state: {state}"
            )));
        }
        self.db.with_conn(|connection| {
            let changed = connection
                .execute(
                    "UPDATE subagent_runs SET state=?2, summary=?3, updated_at=?4, finished_at=?4 WHERE id=?1 AND finished_at IS NULL",
                    params![id, state, summary, at],
                )
                .map_err(map_sqlite)?;
            if changed != 1 {
                return Err(CoreError::not_found(format!(
                    "active subagent run not found: {id}"
                )));
            }
            Ok(())
        })
    }

    pub fn get(&self, id: &str) -> Result<Option<SubagentRunRecord>> {
        self.db.with_conn(|connection| {
            connection
                .query_row(
                    "SELECT id, parent_run_id, origin_parent_run_id, state, agent_type, transcript_path, worktree_path, summary, created_at, updated_at, finished_at, resume_count FROM subagent_runs WHERE id=?1",
                    [id],
                    map_record,
                )
                .optional()
                .map_err(map_sqlite)
        })
    }

    pub fn list_for_parent(&self, parent_run_id: &str) -> Result<Vec<SubagentRunRecord>> {
        self.db.with_conn(|connection| {
            let mut statement = connection
                .prepare("SELECT id, parent_run_id, origin_parent_run_id, state, agent_type, transcript_path, worktree_path, summary, created_at, updated_at, finished_at, resume_count FROM subagent_runs WHERE parent_run_id=?1 OR origin_parent_run_id=?1 ORDER BY created_at, id")
                .map_err(map_sqlite)?;
            let rows = statement
                .query_map([parent_run_id], map_record)
                .map_err(map_sqlite)?;
            rows.collect::<rusqlite::Result<Vec<_>>>().map_err(map_sqlite)
        })
    }

    /// Close child runs left active by a previous process. This is called only
    /// when an on-disk database is opened, before a new runtime can start work.
    pub fn cancel_orphaned(&self, at: i64) -> Result<usize> {
        self.db.with_conn(|connection| {
            connection
                .execute(
                    "UPDATE subagent_runs
                     SET state='cancelled',
                         summary=COALESCE(summary, 'Interrupted because DeepAgent restarted'),
                         updated_at=?1,
                         finished_at=?1
                     WHERE state='running' AND finished_at IS NULL",
                    [at],
                )
                .map_err(map_sqlite)
        })
    }

    /// Reopen a terminal child under a later parent run. The caller is
    /// responsible for verifying both parent runs belong to the same session.
    pub fn resume(&self, id: &str, parent_run_id: &str, at: i64) -> Result<()> {
        self.db.with_conn(|connection| {
            let changed = connection
                .execute(
                    "UPDATE subagent_runs
                     SET parent_run_id=?2,
                         origin_parent_run_id=COALESCE(origin_parent_run_id, parent_run_id),
                         state='running', summary=NULL, updated_at=?3,
                         finished_at=NULL, resume_count=resume_count+1
                     WHERE id=?1 AND state IN ('succeeded', 'failed', 'cancelled')
                       AND finished_at IS NOT NULL",
                    params![id, parent_run_id, at],
                )
                .map_err(map_sqlite)?;
            if changed != 1 {
                return Err(CoreError::invalid(format!(
                    "sub-agent is not resumable: {id}"
                )));
            }
            Ok(())
        })
    }

    pub fn set_worktree_path(&self, id: &str, path: Option<&str>, at: i64) -> Result<()> {
        self.db.with_conn(|connection| {
            let changed = connection
                .execute(
                    "UPDATE subagent_runs SET worktree_path=?2, updated_at=?3 WHERE id=?1",
                    params![id, path, at],
                )
                .map_err(map_sqlite)?;
            if changed != 1 {
                return Err(CoreError::not_found(format!(
                    "sub-agent run not found: {id}"
                )));
            }
            Ok(())
        })
    }
}

fn map_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<SubagentRunRecord> {
    Ok(SubagentRunRecord {
        id: row.get(0)?,
        parent_run_id: row.get(1)?,
        origin_parent_run_id: row.get(2)?,
        state: row.get(3)?,
        agent_type: row.get(4)?,
        transcript_path: row.get(5)?,
        worktree_path: row.get(6)?,
        summary: row.get(7)?,
        created_at: row.get(8)?,
        updated_at: row.get(9)?,
        finished_at: row.get(10)?,
        resume_count: row.get(11)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::run_store::RunStore;

    #[test]
    fn creates_lists_and_finishes_subagent() {
        let db = Database::open_in_memory().unwrap();
        db.with_conn(|connection| {
            connection
                .execute(
                    "INSERT INTO sessions (id, created_at, updated_at) VALUES ('s1', 1, 1)",
                    [],
                )
                .map_err(map_sqlite)?;
            Ok(())
        })
        .unwrap();
        RunStore::new(&db).create("parent", "s1", None, 1).unwrap();
        let store = SubagentRunStore::new(&db);
        store
            .create(&SubagentRunRecord {
                id: "sub1".into(),
                parent_run_id: "parent".into(),
                origin_parent_run_id: "parent".into(),
                state: "running".into(),
                agent_type: "explore".into(),
                transcript_path: Some("sub1.json".into()),
                worktree_path: None,
                summary: None,
                created_at: 2,
                updated_at: 2,
                finished_at: None,
                resume_count: 0,
            })
            .unwrap();
        store.finish("sub1", "succeeded", Some("done"), 3).unwrap();

        let record = store.get("sub1").unwrap().unwrap();
        assert_eq!(record.state, "succeeded");
        assert_eq!(record.summary.as_deref(), Some("done"));
        assert_eq!(store.list_for_parent("parent").unwrap().len(), 1);
    }

    #[test]
    fn cancels_only_orphaned_running_subagents() {
        let db = Database::open_in_memory().unwrap();
        db.with_conn(|connection| {
            connection
                .execute(
                    "INSERT INTO sessions (id, created_at, updated_at) VALUES ('s1', 1, 1)",
                    [],
                )
                .map_err(map_sqlite)?;
            Ok(())
        })
        .unwrap();
        RunStore::new(&db).create("parent", "s1", None, 1).unwrap();
        let store = SubagentRunStore::new(&db);
        for (id, state, finished_at) in
            [("running", "running", None), ("done", "succeeded", Some(3))]
        {
            store
                .create(&SubagentRunRecord {
                    id: id.into(),
                    parent_run_id: "parent".into(),
                    origin_parent_run_id: "parent".into(),
                    state: state.into(),
                    agent_type: "general".into(),
                    transcript_path: None,
                    worktree_path: None,
                    summary: None,
                    created_at: 2,
                    updated_at: finished_at.unwrap_or(2),
                    finished_at,
                    resume_count: 0,
                })
                .unwrap();
        }

        assert_eq!(store.cancel_orphaned(10).unwrap(), 1);
        let orphan = store.get("running").unwrap().unwrap();
        assert_eq!(orphan.state, "cancelled");
        assert_eq!(orphan.finished_at, Some(10));
        assert!(orphan.summary.unwrap().contains("restarted"));
        assert_eq!(store.get("done").unwrap().unwrap().state, "succeeded");
    }

    #[test]
    fn resumes_terminal_child_under_a_later_parent_without_losing_origin() {
        let db = Database::open_in_memory().unwrap();
        db.with_conn(|connection| {
            connection
                .execute(
                    "INSERT INTO sessions (id, created_at, updated_at) VALUES ('s1', 1, 1)",
                    [],
                )
                .map_err(map_sqlite)?;
            Ok(())
        })
        .unwrap();
        let runs = RunStore::new(&db);
        runs.create("parent-a", "s1", None, 1).unwrap();
        runs.create("parent-b", "s1", None, 2).unwrap();
        let store = SubagentRunStore::new(&db);
        store
            .create(&SubagentRunRecord {
                id: "child".into(),
                parent_run_id: "parent-a".into(),
                origin_parent_run_id: "parent-a".into(),
                state: "running".into(),
                agent_type: "general".into(),
                transcript_path: None,
                worktree_path: None,
                summary: None,
                created_at: 3,
                updated_at: 3,
                finished_at: None,
                resume_count: 0,
            })
            .unwrap();
        store
            .finish("child", "succeeded", Some("first"), 4)
            .unwrap();
        store.resume("child", "parent-b", 5).unwrap();

        let child = store.get("child").unwrap().unwrap();
        assert_eq!(child.parent_run_id, "parent-b");
        assert_eq!(child.origin_parent_run_id, "parent-a");
        assert_eq!(child.state, "running");
        assert_eq!(child.resume_count, 1);
        assert!(child.summary.is_none());
        assert_eq!(store.list_for_parent("parent-a").unwrap().len(), 1);
        assert_eq!(store.list_for_parent("parent-b").unwrap().len(), 1);
    }
}
