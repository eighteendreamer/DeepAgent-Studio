//! SQLite metadata inventory for large files kept in managed directories.

use std::path::Path;

use deepagent_core::error::Result;
use rusqlite::params;
use serde::{Deserialize, Serialize};

use crate::{map_sqlite, Database};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManagedFileRecord {
    pub category: String,
    pub relative_path: String,
    pub root_path: String,
    pub byte_size: u64,
    pub modified_at: Option<i64>,
    pub digest: Option<String>,
    pub status: String,
    pub updated_at: i64,
}

pub struct ManagedFileStore<'db> {
    db: &'db Database,
}

impl<'db> ManagedFileStore<'db> {
    pub const fn new(db: &'db Database) -> Self {
        Self { db }
    }

    /// Replace one category inventory atomically. Files remain on disk; this
    /// only refreshes their SQLite metadata projection.
    pub fn replace_category(
        &self,
        category: &str,
        root: &Path,
        records: &[ManagedFileRecord],
    ) -> Result<()> {
        let root = root.to_string_lossy();
        self.db.with_conn(|connection| {
            connection.execute_batch("BEGIN IMMEDIATE").map_err(map_sqlite)?;
            let result = (|| {
                connection
                    .execute("DELETE FROM managed_files WHERE category=?1", [category])
                    .map_err(map_sqlite)?;
                for record in records {
                    connection
                        .execute(
                            "INSERT INTO managed_files (category,relative_path,root_path,byte_size,modified_at,digest,status,updated_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
                            params![
                                category,
                                record.relative_path,
                                root.as_ref(),
                                record.byte_size.min(i64::MAX as u64) as i64,
                                record.modified_at,
                                record.digest,
                                record.status,
                                record.updated_at,
                            ],
                        )
                        .map_err(map_sqlite)?;
                }
                Ok(())
            })();
            match result {
                Ok(()) => {
                    connection.execute_batch("COMMIT").map_err(map_sqlite)?;
                    Ok(())
                }
                Err(error) => {
                    let _ = connection.execute_batch("ROLLBACK");
                    Err(error)
                }
            }
        })
    }

    pub fn list(&self, category: Option<&str>) -> Result<Vec<ManagedFileRecord>> {
        self.db.with_conn(|connection| {
            let (sql, value) = match category {
                Some(value) => (
                    "SELECT category,relative_path,root_path,byte_size,modified_at,digest,status,updated_at FROM managed_files WHERE category=?1 ORDER BY relative_path",
                    value,
                ),
                None => (
                    "SELECT category,relative_path,root_path,byte_size,modified_at,digest,status,updated_at FROM managed_files WHERE ?1='' ORDER BY category,relative_path",
                    "",
                ),
            };
            let mut statement = connection.prepare(sql).map_err(map_sqlite)?;
            let rows = statement
                .query_map([value], |row| {
                    Ok(ManagedFileRecord {
                        category: row.get(0)?,
                        relative_path: row.get(1)?,
                        root_path: row.get(2)?,
                        byte_size: row.get::<_, i64>(3)?.max(0) as u64,
                        modified_at: row.get(4)?,
                        digest: row.get(5)?,
                        status: row.get(6)?,
                        updated_at: row.get(7)?,
                    })
                })
                .map_err(map_sqlite)?;
            rows.collect::<rusqlite::Result<Vec<_>>>().map_err(map_sqlite)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_only_selected_category() {
        let db = Database::open_in_memory().unwrap();
        let store = ManagedFileStore::new(&db);
        let record = |category: &str, path: &str| ManagedFileRecord {
            category: category.into(),
            relative_path: path.into(),
            root_path: "ignored-by-replace".into(),
            byte_size: 12,
            modified_at: Some(2),
            digest: None,
            status: "present".into(),
            updated_at: 3,
        };
        store
            .replace_category(
                "attachments",
                Path::new("C:/data/attachments"),
                &[record("attachments", "a.txt")],
            )
            .unwrap();
        store
            .replace_category(
                "cache",
                Path::new("C:/data/cache"),
                &[record("cache", "b.bin")],
            )
            .unwrap();
        store
            .replace_category("attachments", Path::new("C:/data/attachments"), &[])
            .unwrap();
        assert!(store.list(Some("attachments")).unwrap().is_empty());
        assert_eq!(store.list(Some("cache")).unwrap().len(), 1);
    }
}
