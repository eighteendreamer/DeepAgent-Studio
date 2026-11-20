//! Audit timestamps for application tables that previously had only domain clocks.
//! SQLite virtual/shadow/system tables are owned by SQLite and are never altered.
//! Existing writer-owned timestamps stay writer-owned. New fields are maintained
//! here so every insertion path, including projection repair, obeys one rule.

use rusqlite::Connection;

use crate::{map_sqlite, Result};

const NOW: &str = "(CAST(strftime('%s','now') AS INTEGER)*1000 + CAST(substr(strftime('%f','now'),4,3) AS INTEGER))";

// Table, original creation clock, original update clock. None means a new clock.
const TABLES: &[(&str, Option<&str>, Option<&str>)] = &[
    ("events", Some("timestamp"), None),
    ("costs", Some("timestamp"), None),
    ("run_events", Some("timestamp"), None),
    ("checkpoints", Some("created_at"), None),
    ("artifacts", Some("created_at"), None),
    ("migration_notices", Some("created_at"), None),
    ("memory_items_migration_ledger", Some("migrated_at"), None),
    ("secret_records", Some("updated_at"), Some("updated_at")),
    ("managed_files", Some("updated_at"), Some("updated_at")),
    (
        "session_search_cursors",
        Some("updated_at"),
        Some("updated_at"),
    ),
    (
        "terminal_session_cursors",
        Some("updated_at"),
        Some("updated_at"),
    ),
    ("session_file_state", Some("updated_at"), Some("updated_at")),
    ("harness_models", Some("updated_at"), Some("updated_at")),
    ("session_search_docs", Some("timestamp"), None),
    ("execution_leases", Some("acquired_at"), Some("renewed_at")),
    ("session_search_cjk_bigrams", None, None),
    ("session_catalog_state", None, None),
];

pub(super) fn install(conn: &Connection) -> Result<()> {
    for &(table, creation, update) in TABLES {
        install_table(conn, table, creation, update)?;
    }
    Ok(())
}

fn install_table(
    conn: &Connection,
    table: &str,
    creation: Option<&str>,
    update: Option<&str>,
) -> Result<()> {
    let columns = conn
        .prepare(&format!("PRAGMA table_info({table})"))
        .map_err(map_sqlite)?
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(map_sqlite)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(map_sqlite)?;
    for name in ["created_at", "updated_at"] {
        if !columns.iter().any(|column| column == name) {
            conn.execute_batch(&format!(
                "ALTER TABLE {table} ADD COLUMN {name} INTEGER NOT NULL DEFAULT 0;"
            ))
            .map_err(map_sqlite)?;
            let source = if name == "created_at" {
                creation.unwrap_or(NOW).to_owned()
            } else {
                format!(
                    "COALESCE({}, {})",
                    update.unwrap_or("NULL"),
                    creation.unwrap_or(NOW)
                )
            };
            conn.execute_batch(&format!("UPDATE {table} SET {name} = {source};"))
                .map_err(map_sqlite)?;
        }
    }
    let writer_owns_update = update == Some("updated_at");
    let creation = creation
        .map(|field| format!("NEW.{field}"))
        .unwrap_or(NOW.into());
    let update = format!(
        "COALESCE({}, {creation})",
        update
            .map(|field| format!("NEW.{field}"))
            .unwrap_or("NULL".into())
    );
    conn.execute_batch(&format!(
        "CREATE TRIGGER IF NOT EXISTS {table}_audit_insert AFTER INSERT ON {table}
         WHEN NEW.created_at = 0 OR NEW.updated_at = 0 BEGIN
            UPDATE {table} SET
                created_at = CASE WHEN NEW.created_at = 0 THEN {creation} ELSE NEW.created_at END,
                updated_at = CASE WHEN NEW.updated_at = 0 THEN {update} ELSE NEW.updated_at END
            WHERE rowid = NEW.rowid;
         END;"
    ))
    .map_err(map_sqlite)?;
    // Existing updated_at writers already preserve their domain contracts.
    // Maintain the newly introduced clock without recursively touching itself.
    if !writer_owns_update {
        let business: Vec<_> = columns
            .iter()
            .filter(|column| !matches!(column.as_str(), "created_at" | "updated_at"))
            .collect();
        let changed = business
            .iter()
            .map(|column| format!("OLD.{column} IS NOT NEW.{column}"))
            .collect::<Vec<_>>()
            .join(" OR ");
        conn.execute_batch(&format!(
            "CREATE TRIGGER IF NOT EXISTS {table}_audit_update AFTER UPDATE ON {table}
             WHEN NEW.updated_at IS OLD.updated_at AND ({changed}) BEGIN
                UPDATE {table} SET updated_at = max(OLD.updated_at + 1, {NOW})
                WHERE rowid = NEW.rowid;
             END;"
        ))
        .map_err(map_sqlite)?;
    }
    Ok(())
}
