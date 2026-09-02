//! Metadata index for run-scoped tool output artifacts.

use deepagent_core::error::Result;
use rusqlite::params;
use serde::{Deserialize, Serialize};

use crate::{map_sqlite, Database};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolArtifactRecord {
    pub id: String,
    pub run_id: String,
    pub call_id: String,
    pub path: String,
    pub media_type: Option<String>,
    pub byte_size: i64,
    pub digest: Option<String>,
    pub created_at: i64,
}

pub struct ToolArtifactStore<'db> {
    db: &'db Database,
}

impl<'db> ToolArtifactStore<'db> {
    pub const fn new(db: &'db Database) -> Self {
        Self { db }
    }

    pub fn put(&self, record: &ToolArtifactRecord) -> Result<()> {
        self.db.with_conn(|conn| {
            conn.execute(
                "INSERT OR REPLACE INTO tool_artifacts (id, run_id, call_id, path, media_type, byte_size, digest, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    record.id,
                    record.run_id,
                    record.call_id,
                    record.path,
                    record.media_type,
                    record.byte_size,
                    record.digest,
                    record.created_at,
                ],
            )
            .map_err(map_sqlite)?;
            Ok(())
        })
    }

    pub fn list_for_run(&self, run_id: &str) -> Result<Vec<ToolArtifactRecord>> {
        self.db.with_conn(|conn| {
            let mut statement = conn
                .prepare("SELECT id, run_id, call_id, path, media_type, byte_size, digest, created_at FROM tool_artifacts WHERE run_id=?1 ORDER BY created_at, id")
                .map_err(map_sqlite)?;
            let rows = statement
                .query_map([run_id], |row| {
                    Ok(ToolArtifactRecord {
                        id: row.get(0)?,
                        run_id: row.get(1)?,
                        call_id: row.get(2)?,
                        path: row.get(3)?,
                        media_type: row.get(4)?,
                        byte_size: row.get(5)?,
                        digest: row.get(6)?,
                        created_at: row.get(7)?,
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
    use crate::run_store::RunStore;

    #[test]
    fn records_and_lists_artifacts_by_run() {
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
        RunStore::new(&db).create("r1", "s1", None, 1).unwrap();
        ToolArtifactStore::new(&db)
            .put(&ToolArtifactRecord {
                id: "a1".into(),
                run_id: "r1".into(),
                call_id: "c1".into(),
                path: "result.json".into(),
                media_type: Some("application/json".into()),
                byte_size: 42,
                digest: None,
                created_at: 2,
            })
            .unwrap();
        let records = ToolArtifactStore::new(&db).list_for_run("r1").unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].call_id, "c1");
        assert_eq!(records[0].byte_size, 42);
        let json = serde_json::to_value(&records[0]).unwrap();
        assert_eq!(json["runId"], "r1");
        assert_eq!(json["mediaType"], "application/json");
        assert!(json.get("run_id").is_none());
    }
}
