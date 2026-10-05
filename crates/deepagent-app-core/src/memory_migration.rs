//! One-release migration: legacy `memory_items` JSON rows → vault entries.
//!
//! Before the storage convergence, `memory_write` persisted memories as JSON in
//! the shared `documents` table under the `memory_items` collection, while the
//! retrieval track that injects `relevant memories` only reads Markdown files
//! from the knowledge vault. Any memory written by an older build is therefore
//! stranded: `memory_recall` can still list it, but the model never sees it in
//! context. This module moves those rows into the vault once at startup.
//!
//! Layering: the ledger table itself is schema (V22 in `deepagent-persistence`),
//! but the data move lives here because only app-core knows both the document
//! store and the vault paths — `deepagent-persistence` stays free of upward
//! dependencies (see the V3 comment in `migrations.rs`).
//!
//! Crash-safety and reversibility:
//! - every attempt is written to `memory_items_migration_ledger` together with
//!   the original JSON body, so the move can be undone even after the source row
//!   is deleted;
//! - a row is only deleted after its vault file was written, and a ledger row
//!   with `vault_path IS NOT NULL` counts as already migrated, so an interrupted
//!   run resumes without duplicating entries;
//! - an item that cannot be written keeps its source row and is recorded with
//!   `vault_path IS NULL`, so it is retried on the next start instead of being
//!   silently dropped.

use std::collections::HashMap;
use std::sync::Arc;

use deepagent_core::clock::{Clock, SystemClock, Timestamp};
use deepagent_core::error::{CoreError, Result};
use deepagent_memory::MemoryItem;
use deepagent_persistence::document_store::DocumentStore;
use deepagent_persistence::Database;
use rusqlite::OptionalExtension;

use crate::knowledge_service::{KnowledgeDraftDto, KnowledgeService};
use crate::memory_service::memory_title;

/// Collection the legacy rows live in. Kept local so the string cannot drift
/// from what older builds wrote.
const LEGACY_COLLECTION: &str = "memory_items";

/// Outcome of one migration pass, logged at startup and asserted in tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MemoryMigrationReport {
    /// Rows written to the vault in this pass.
    pub migrated: usize,
    /// Rows already recorded as migrated (no-op on re-run).
    pub skipped: usize,
    /// Rows that could not be written; their source rows remain.
    pub failed: usize,
}

impl MemoryMigrationReport {
    /// True when this pass wrote nothing and failed nothing.
    pub fn is_no_op(&self) -> bool {
        self.migrated == 0 && self.failed == 0
    }
}

/// Migrate every legacy `memory_items` row into the knowledge vault.
///
/// Idempotent: safe to call on every startup. Short-circuits without touching
/// the ledger when the legacy collection is empty.
pub fn migrate_legacy_memory_items(
    db: &Arc<Database>,
    knowledge: &Arc<KnowledgeService>,
) -> Result<MemoryMigrationReport> {
    let store = DocumentStore::new(db);
    let rows = store.list(LEGACY_COLLECTION)?;
    if rows.is_empty() {
        return Ok(MemoryMigrationReport::default());
    }

    let already = ledger_state(db)?;
    let now = SystemClock.now();
    let mut report = MemoryMigrationReport::default();

    for doc in rows {
        if let Some(Some(_path)) = already.get(&doc.id) {
            // A previous pass wrote the file; only the source-row delete was
            // interrupted. Finish it and count as skipped, not migrated.
            store.delete(LEGACY_COLLECTION, &doc.id)?;
            report.skipped += 1;
            continue;
        }

        let item: MemoryItem = match serde_json::from_str(&doc.body) {
            Ok(item) => item,
            Err(error) => {
                // Corrupt payload: keep the row, record the failure, retry later.
                tracing::warn!(
                    item_id = %doc.id,
                    error = %error,
                    "legacy memory row is not a MemoryItem; leaving it in place"
                );
                record_ledger(db, &doc.id, None, now, &doc.body)?;
                report.failed += 1;
                continue;
            }
        };

        let saved = knowledge.save(KnowledgeDraftDto {
            title: memory_title(&item.content),
            body: item.content.clone(),
            kind: Some("memory".to_string()),
            tags: item.tags.clone(),
            // Legacy rows predate scope selection and were app-global in
            // practice: recall read them across every project.
            scope: Some("global".to_string()),
            source_session: None,
            tier: Some(tier_label(&item).to_string()),
            importance: Some(item.importance),
        });

        match saved {
            Ok(dto) => {
                record_ledger(db, &doc.id, Some(&dto.id), now, &doc.body)?;
                store.delete(LEGACY_COLLECTION, &doc.id)?;
                report.migrated += 1;
            }
            Err(error) => {
                tracing::warn!(
                    item_id = %doc.id,
                    error = %error,
                    "could not write legacy memory into the vault; keeping the row"
                );
                record_ledger(db, &doc.id, None, now, &doc.body)?;
                report.failed += 1;
            }
        }
    }

    tracing::info!(
        migrated = report.migrated,
        skipped = report.skipped,
        failed = report.failed,
        "legacy memory_items migration pass finished"
    );
    Ok(report)
}

/// Read the ledger: `item_id -> vault entry id`. A `None` value marks a failed
/// attempt, which must be retried rather than treated as done.
pub fn ledger_state(db: &Arc<Database>) -> Result<HashMap<String, Option<String>>> {
    db.with_conn(|conn| {
        let mut stmt = conn
            .prepare("SELECT item_id, vault_path FROM memory_items_migration_ledger")
            .map_err(sql_err)?;
        let mut rows = stmt.query([]).map_err(sql_err)?;
        let mut out = HashMap::new();
        while let Some(row) = rows.next().map_err(sql_err)? {
            let id: String = row.get(0).map_err(sql_err)?;
            let path: Option<String> = row.get(1).map_err(sql_err)?;
            out.insert(id, path);
        }
        Ok(out)
    })
}

/// Record (or overwrite) one ledger row. Exposed for the crash-safety test and
/// for any future rollback tooling.
pub fn record_ledger(
    db: &Arc<Database>,
    item_id: &str,
    vault_entry_id: Option<&str>,
    now: Timestamp,
    source_body: &str,
) -> Result<()> {
    db.with_conn(|conn| {
        conn.execute(
            "INSERT INTO memory_items_migration_ledger (item_id, vault_path, migrated_at, source_body)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(item_id) DO UPDATE SET
                 vault_path = excluded.vault_path,
                 migrated_at = excluded.migrated_at,
                 source_body = excluded.source_body",
            rusqlite::params![item_id, vault_entry_id, now.as_millis(), source_body],
        )
        .map_err(sql_err)?;
        Ok(())
    })
}

/// The original JSON body kept for rollback, by item id. `None` when the
/// migration never saw that item.
pub fn ledger_source_body(db: &Arc<Database>, item_id: &str) -> Result<Option<String>> {
    db.with_conn(|conn| {
        let body: Option<String> = conn
            .query_row(
                "SELECT source_body FROM memory_items_migration_ledger WHERE item_id = ?1",
                rusqlite::params![item_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(sql_err)?;
        Ok(body)
    })
}

/// `deepagent-persistence` keeps SQL errors at its own boundary, so app-core
/// maps them the same way instead of assuming a `From` impl exists.
fn sql_err(error: rusqlite::Error) -> CoreError {
    CoreError::Persistence(error.to_string())
}

/// Legacy tier label; `MemoryItem.tier` is the typed source of truth.
fn tier_label(item: &MemoryItem) -> &'static str {
    use deepagent_memory::MemoryTier;
    match item.tier {
        MemoryTier::Semantic => "semantic",
        MemoryTier::Episodic => "episodic",
        MemoryTier::Procedural => "procedural",
        MemoryTier::Workspace => "workspace",
        MemoryTier::Failure => "failure",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use deepagent_builtins::MemoryBackend;
    use deepagent_memory::MemoryTier;

    fn knowledge(tmp: &std::path::Path) -> Arc<KnowledgeService> {
        Arc::new(
            KnowledgeService::open(&tmp.join("proj"), &tmp.join("glob")).expect("open knowledge"),
        )
    }

    fn legacy_body(tier: MemoryTier, content: &str, importance: f32) -> String {
        let item = MemoryItem::new(tier, content.to_string(), importance, SystemClock.now())
            .with_tags(vec!["legacy".into()]);
        serde_json::to_string(&item).unwrap()
    }

    fn seed(db: &Arc<Database>, id: &str, body: &str) {
        DocumentStore::new(db)
            .put(LEGACY_COLLECTION, id, body, None, SystemClock.now())
            .unwrap();
    }

    fn memory_entries(svc: &KnowledgeService) -> usize {
        svc.list()
            .iter()
            .filter(|entry| entry.kind == "memory")
            .count()
    }

    #[test]
    fn legacy_rows_move_into_the_vault_and_leave_the_collection() {
        let tmp = tempfile::tempdir().unwrap();
        let db = Arc::new(Database::open_in_memory().unwrap());
        let svc = knowledge(tmp.path());

        seed(
            &db,
            "m-a",
            &legacy_body(MemoryTier::Failure, "cargo needs -j1 on this machine", 0.9),
        );
        seed(
            &db,
            "m-b",
            &legacy_body(MemoryTier::Procedural, "user prefers concise commits", 0.4),
        );

        let report = migrate_legacy_memory_items(&db, &svc).unwrap();
        assert_eq!(
            report,
            MemoryMigrationReport {
                migrated: 2,
                skipped: 0,
                failed: 0
            }
        );
        assert!(DocumentStore::new(&db)
            .list(LEGACY_COLLECTION)
            .unwrap()
            .is_empty());
        assert_eq!(memory_entries(&svc), 2);

        // The whole point of the move: knowledge retrieval now sees them, which
        // the old JSON rows could never reach.
        let hits = svc.search("concise commits", None, 5);
        assert!(
            hits.iter().any(|hit| hit.kind == "memory"),
            "migrated memory must surface through knowledge search: {hits:?}"
        );

        // Tier and importance survived as frontmatter, so recall still ranks.
        let ledger = ledger_state(&db);
        assert_eq!(ledger.unwrap().len(), 2);
        assert!(
            ledger_source_body(&db, "m-a").unwrap().is_some(),
            "body kept for rollback"
        );
    }

    #[tokio::test]
    async fn migrated_memories_are_recallable_through_the_memory_tool_backend() {
        let tmp = tempfile::tempdir().unwrap();
        let db = Arc::new(Database::open_in_memory().unwrap());
        let svc = knowledge(tmp.path());
        seed(
            &db,
            "m-a",
            &legacy_body(MemoryTier::Failure, "cargo needs -j1 on this machine", 0.9),
        );
        seed(
            &db,
            "m-b",
            &legacy_body(MemoryTier::Procedural, "user prefers concise commits", 0.4),
        );
        migrate_legacy_memory_items(&db, &svc).unwrap();

        let backend = crate::memory_service::MemoryService::new(svc);
        let all = backend.recall(None, 10).await.unwrap();
        assert_eq!(all.len(), 2, "recall must read the migrated entries");
        assert_eq!(all[0].tier, "failure", "importance 0.9 outranks 0.4");

        let failures = backend.recall(Some("failure".into()), 10).await.unwrap();
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].content, "cargo needs -j1 on this machine");
    }

    #[test]
    fn a_second_pass_is_a_no_op_and_creates_no_duplicates() {
        let tmp = tempfile::tempdir().unwrap();
        let db = Arc::new(Database::open_in_memory().unwrap());
        let svc = knowledge(tmp.path());
        seed(
            &db,
            "m-a",
            &legacy_body(MemoryTier::Semantic, "fact one", 0.5),
        );

        let first = migrate_legacy_memory_items(&db, &svc).unwrap();
        assert_eq!(first.migrated, 1);
        let second = migrate_legacy_memory_items(&db, &svc).unwrap();
        assert!(
            second.is_no_op(),
            "re-run must move nothing, got {second:?}"
        );
        assert_eq!(memory_entries(&svc), 1);
    }

    #[test]
    fn interrupted_delete_after_write_is_finished_without_rewriting() {
        let tmp = tempfile::tempdir().unwrap();
        let db = Arc::new(Database::open_in_memory().unwrap());
        let svc = knowledge(tmp.path());
        let body = legacy_body(MemoryTier::Workspace, "repo layout note", 0.5);

        // Simulate a crash between "vault file written" and "source row deleted".
        let dto = svc
            .save(KnowledgeDraftDto {
                title: memory_title("repo layout note"),
                body: "repo layout note".to_string(),
                kind: Some("memory".to_string()),
                tags: vec!["legacy".into()],
                scope: Some("global".to_string()),
                source_session: None,
                tier: Some("workspace".to_string()),
                importance: Some(0.5),
            })
            .unwrap();
        record_ledger(&db, "m-x", Some(&dto.id), SystemClock.now(), &body).unwrap();
        seed(&db, "m-x", &body);

        let report = migrate_legacy_memory_items(&db, &svc).unwrap();
        assert_eq!(report.migrated, 0, "must not write the entry a second time");
        assert_eq!(report.skipped, 1);
        assert!(DocumentStore::new(&db)
            .list(LEGACY_COLLECTION)
            .unwrap()
            .is_empty());
        assert_eq!(memory_entries(&svc), 1);
    }

    #[test]
    fn a_corrupt_row_is_kept_and_retried_not_skipped() {
        let tmp = tempfile::tempdir().unwrap();
        let db = Arc::new(Database::open_in_memory().unwrap());
        let svc = knowledge(tmp.path());
        seed(&db, "broken-1", "this is not a MemoryItem");

        let report = migrate_legacy_memory_items(&db, &svc).unwrap();
        assert_eq!(report.failed, 1);
        assert_eq!(report.migrated, 0);
        // Failure never costs data: the row survives for a later retry.
        assert_eq!(
            DocumentStore::new(&db)
                .list(LEGACY_COLLECTION)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            ledger_state(&db).unwrap(),
            HashMap::from([("broken-1".to_string(), None)])
        );
        assert_eq!(memory_entries(&svc), 0);

        let again = migrate_legacy_memory_items(&db, &svc).unwrap();
        assert_eq!(
            again.failed, 1,
            "a failed attempt must be retried, not skipped"
        );
    }
}
