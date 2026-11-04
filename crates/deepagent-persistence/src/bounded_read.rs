//! Shared query mechanics for existing repositories' bounded projections.
//! This owns no history, schema, registry or mutable state. Repository-owned
//! static SQL descriptors and their existing row decoders remain authoritative.

use deepagent_core::error::{CoreError, Result};
use rusqlite::{params, types::ValueRef, Row};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::Write;

use crate::{map_sqlite, Database};

pub const MAX_HISTORY_ITEMS: usize = 500;
pub const MAX_HISTORY_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum HistorySource {
    Runs,
    Events,
    Actions,
    Approvals,
    Artifacts,
    Children,
}

impl HistorySource {
    const ALL: [Self; 6] = [
        Self::Runs,
        Self::Events,
        Self::Actions,
        Self::Approvals,
        Self::Artifacts,
        Self::Children,
    ];

    fn anchor_columns(self) -> (&'static str, &'static str, &'static str) {
        match self {
            Self::Runs => ("runs", "created_at", "id"),
            Self::Events => ("run_events", "sequence", "CAST(id AS TEXT)"),
            Self::Actions => ("run_actions", "sequence", "call_id,run_id"),
            Self::Approvals => ("run_approvals", "created_at", "approval_id"),
            Self::Artifacts => ("artifacts", "created_at", "id"),
            Self::Children => ("subagent_runs", "created_at", "id"),
        }
    }
}

fn anchor_digest(
    connection: &rusqlite::Connection,
    source: HistorySource,
    through_rowid: i64,
) -> Result<[u8; 32]> {
    let (table, number, id) = source.anchor_columns();
    let mut digest = Sha256::new();
    digest.update(table.as_bytes());
    digest.update([0]);
    digest.update(through_rowid.to_le_bytes());
    if through_rowid == 0 {
        digest.update([0]);
        return Ok(digest.finalize().into());
    }

    let mut statement = connection
        .prepare(&format!("SELECT {number},{id} FROM {table} WHERE rowid=?1"))
        .map_err(map_sqlite)?;
    let mut rows = statement.query([through_rowid]).map_err(map_sqlite)?;
    let row = rows
        .next()
        .map_err(map_sqlite)?
        .ok_or_else(stale_snapshot)?;
    let number: i64 = row.get(0).map_err(map_sqlite)?;
    digest.update([1]);
    digest.update(number.to_le_bytes());
    for column in 1..row.as_ref().column_count() {
        let id = row.get_ref(column).map_err(map_sqlite)?;
        let id = id
            .as_str()
            .map_err(|_| CoreError::EventLog("invalid history anchor identity".into()))?;
        if id.len() > 1024 {
            return Err(CoreError::invalid(
                "history_item_too_large: history anchor identity exceeds cursor budget",
            ));
        }
        digest.update((id.len() as u64).to_le_bytes());
        digest.update(id.as_bytes());
    }
    Ok(digest.finalize().into())
}

fn stale_snapshot() -> CoreError {
    CoreError::invalid("stale history snapshot: high-water row identity changed")
}

/// Insertion cuts captured once in the original database, not a second store.
/// Identity anchors reject high-water deletion/reuse. Mutable projections and
/// relationship predicates remain live, not an immutable MVCC snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistorySnapshot {
    runs: i64,
    events: i64,
    actions: i64,
    approvals: i64,
    artifacts: i64,
    children: i64,
    run_anchor: [u8; 32],
    event_anchor: [u8; 32],
    action_anchor: [u8; 32],
    approval_anchor: [u8; 32],
    artifact_anchor: [u8; 32],
    child_anchor: [u8; 32],
}

impl HistorySnapshot {
    pub fn capture(db: &Database) -> Result<Self> {
        db.with_conn(|connection| {
            let tx = connection.unchecked_transaction().map_err(map_sqlite)?;
            let mut snapshot = tx
                .query_row(
                    "SELECT (SELECT COALESCE(MAX(rowid),0) FROM runs),
                        (SELECT COALESCE(MAX(rowid),0) FROM run_events),
                        (SELECT COALESCE(MAX(rowid),0) FROM run_actions),
                        (SELECT COALESCE(MAX(rowid),0) FROM run_approvals),
                        (SELECT COALESCE(MAX(rowid),0) FROM artifacts),
                        (SELECT COALESCE(MAX(rowid),0) FROM subagent_runs)",
                    [],
                    |row| {
                        Ok(Self {
                            runs: row.get(0)?,
                            events: row.get(1)?,
                            actions: row.get(2)?,
                            approvals: row.get(3)?,
                            artifacts: row.get(4)?,
                            children: row.get(5)?,
                            run_anchor: [0; 32],
                            event_anchor: [0; 32],
                            action_anchor: [0; 32],
                            approval_anchor: [0; 32],
                            artifact_anchor: [0; 32],
                            child_anchor: [0; 32],
                        })
                    },
                )
                .map_err(map_sqlite)?;
            for source in HistorySource::ALL {
                let through = snapshot.through(source);
                *snapshot.anchor_mut(source) = anchor_digest(&tx, source, through)?;
            }
            tx.commit().map_err(map_sqlite)?;
            Ok(snapshot)
        })
    }

    fn through(self, source: HistorySource) -> i64 {
        match source {
            HistorySource::Runs => self.runs,
            HistorySource::Events => self.events,
            HistorySource::Actions => self.actions,
            HistorySource::Approvals => self.approvals,
            HistorySource::Artifacts => self.artifacts,
            HistorySource::Children => self.children,
        }
    }

    fn anchor(self, source: HistorySource) -> [u8; 32] {
        match source {
            HistorySource::Runs => self.run_anchor,
            HistorySource::Events => self.event_anchor,
            HistorySource::Actions => self.action_anchor,
            HistorySource::Approvals => self.approval_anchor,
            HistorySource::Artifacts => self.artifact_anchor,
            HistorySource::Children => self.child_anchor,
        }
    }

    fn anchor_mut(&mut self, source: HistorySource) -> &mut [u8; 32] {
        match source {
            HistorySource::Runs => &mut self.run_anchor,
            HistorySource::Events => &mut self.event_anchor,
            HistorySource::Actions => &mut self.action_anchor,
            HistorySource::Approvals => &mut self.approval_anchor,
            HistorySource::Artifacts => &mut self.artifact_anchor,
            HistorySource::Children => &mut self.child_anchor,
        }
    }

    fn validate_source(
        self,
        connection: &rusqlite::Connection,
        source: HistorySource,
    ) -> Result<()> {
        if self.anchor(source) != anchor_digest(connection, source, self.through(source))? {
            return Err(stale_snapshot());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistoryCursor {
    source: HistorySource,
    scope: String,
    through_rowid: i64,
    after_number: Option<i64>,
    after_id: String,
    anchor: [u8; 32],
}

#[derive(Debug, Clone, Copy)]
pub struct HistoryReadBudget {
    pub max_rows: usize,
    pub max_bytes: usize,
    /// Use the same membership cut across all repositories and runs of one
    /// hydration. None retains independent repository-page reads.
    pub snapshot: Option<HistorySnapshot>,
}

#[derive(Debug)]
pub struct HistoryRow<T, Cursor = HistoryCursor> {
    pub value: T,
    pub cursor: Cursor,
}

#[derive(Debug)]
pub struct HistoryPage<T, Cursor = HistoryCursor> {
    /// Checkpoint before the first row, including its frozen membership cut.
    pub start_cursor: Cursor,
    pub rows: Vec<HistoryRow<T, Cursor>>,
    pub has_more: bool,
}

/// Only repositories can specify SQL identifiers; request text is never SQL.
pub(crate) struct HistoryQuery {
    pub source: HistorySource,
    pub columns: &'static str,
    pub table: &'static str,
    pub scope_filter: &'static str,
    pub number: &'static str,
    pub id: &'static str,
    pub descending: bool,
}

impl HistoryQuery {
    fn keyset_sql(&self) -> String {
        let compare = if self.descending { "<" } else { ">" };
        let direction = if self.descending { "DESC" } else { "ASC" };
        format!(
            "SELECT rowid,{},{} FROM {} WHERE ({}) AND rowid<=?2 AND
             (?3 IS NULL OR {} {compare} ?3 OR ({}=?3 AND {} {compare} ?4))
             ORDER BY {} {direction},{} {direction} LIMIT ?5",
            self.number,
            self.id,
            self.table,
            self.scope_filter,
            self.number,
            self.number,
            self.id,
            self.number,
            self.id,
        )
    }

    fn initial_cursor(
        &self,
        connection: &rusqlite::Connection,
        scope: &str,
        initial_after: Option<i64>,
        snapshot: Option<HistorySnapshot>,
    ) -> Result<HistoryCursor> {
        let (through_rowid, anchor) = match snapshot {
            Some(snapshot) => (snapshot.through(self.source), snapshot.anchor(self.source)),
            None => {
                let through_rowid = connection
                    .query_row(
                        &format!(
                            "SELECT COALESCE(MAX(rowid),0) FROM {} WHERE {}",
                            self.table, self.scope_filter
                        ),
                        [scope],
                        |row| row.get(0),
                    )
                    .map_err(map_sqlite)?;
                (
                    through_rowid,
                    anchor_digest(connection, self.source, through_rowid)?,
                )
            }
        };
        Ok(HistoryCursor {
            source: self.source,
            scope: scope.into(),
            through_rowid,
            after_number: initial_after,
            after_id: String::new(),
            anchor,
        })
    }
}

pub(crate) fn read<T: Serialize>(
    db: &Database,
    query: &HistoryQuery,
    scope: &str,
    cursor: Option<&HistoryCursor>,
    budget: HistoryReadBudget,
    decode: impl FnMut(&Row<'_>) -> Result<T>,
) -> Result<HistoryPage<T>> {
    read_after(db, query, scope, None, cursor, budget, decode)
}

pub(crate) fn read_after<T: Serialize>(
    db: &Database,
    query: &HistoryQuery,
    scope: &str,
    initial_after: Option<i64>,
    cursor: Option<&HistoryCursor>,
    budget: HistoryReadBudget,
    mut decode: impl FnMut(&Row<'_>) -> Result<T>,
) -> Result<HistoryPage<T>> {
    if !(1..=MAX_HISTORY_ITEMS).contains(&budget.max_rows)
        || !(1..=MAX_HISTORY_BYTES).contains(&budget.max_bytes)
        || scope.len() > 1024
    {
        return Err(CoreError::invalid("invalid history query budget or scope"));
    }
    if cursor.is_some_and(|cursor| {
        cursor.source != query.source
            || cursor.scope != scope
            || cursor.through_rowid < 0
            || cursor.after_id.len() > 1024
            || budget.snapshot.is_some_and(|snapshot| {
                cursor.through_rowid != snapshot.through(query.source)
                    || cursor.anchor != snapshot.anchor(query.source)
            })
    }) {
        return Err(CoreError::invalid(
            "invalid or foreign history query cursor",
        ));
    }
    db.with_conn(|conn| {
        // Freeze the high-water lookup and reads for this call, including other
        // Database instances/processes. Across pages only membership is frozen;
        // mutable control states are current projections, not a historical MVCC
        // snapshot. The immutable run ledger remains the replay authority.
        let tx = conn.unchecked_transaction().map_err(map_sqlite)?;
        if let Some(snapshot) = budget.snapshot {
            snapshot.validate_source(&tx, query.source)?;
        } else if let Some(cursor) = cursor {
            if cursor.anchor != anchor_digest(&tx, query.source, cursor.through_rowid)? {
                return Err(stale_snapshot());
            }
        }
        let mut position = match cursor {
            Some(cursor) => cursor.clone(),
            None => query.initial_cursor(&tx, scope, initial_after, budget.snapshot)?,
        };
        let start_cursor = position.clone();
        // Sort only the small identity keys, never the payloads. Hydrate one
        // selected row at a time, guarding borrowed SQLite values before the
        // repository decoder can clone strings or deserialize JSON.
        let mut keys = tx.prepare(&query.keyset_sql()).map_err(map_sqlite)?;
        let mut key_rows = keys
            .query(params![
                scope,
                position.through_rowid,
                position.after_number,
                position.after_id,
                budget.max_rows as i64 + 1
            ])
            .map_err(map_sqlite)?;
        let mut hydrate = tx
            .prepare(&format!(
                "SELECT {} FROM {} WHERE rowid=?1",
                query.columns, query.table
            ))
            .map_err(map_sqlite)?;
        let mut rows = Vec::new();
        let mut bytes = 2;
        let mut has_more = false;
        while let Some(key_row) = key_rows.next().map_err(map_sqlite)? {
            if rows.len() == budget.max_rows {
                has_more = true;
                break;
            }
            let rowid: i64 = key_row.get(0).map_err(map_sqlite)?;
            let number: i64 = key_row.get(1).map_err(map_sqlite)?;
            let id = key_row
                .get_ref(2)
                .map_err(map_sqlite)?
                .as_str()
                .map_err(|_| CoreError::EventLog("invalid history key".into()))?;
            if id.len() > 1024 {
                return Err(CoreError::invalid(
                    "history_item_too_large: history identity exceeds cursor budget",
                ));
            }
            let mut selected = hydrate.query([rowid]).map_err(map_sqlite)?;
            let row = selected.next().map_err(map_sqlite)?.ok_or_else(|| {
                CoreError::EventLog("history row disappeared inside snapshot".into())
            })?;
            check_raw_row(row, budget.max_bytes)?;
            let value = decode(row)?;
            let encoded = json_len(&value, budget.max_bytes)?;
            let next_bytes = bytes + encoded + usize::from(!rows.is_empty());
            if next_bytes > budget.max_bytes {
                if rows.is_empty() {
                    return Err(oversized());
                }
                has_more = true;
                break;
            }
            bytes = next_bytes;
            position.after_number = Some(number);
            position.after_id = id.to_owned();
            rows.push(HistoryRow {
                value,
                cursor: position.clone(),
            });
        }
        drop(key_rows);
        drop(keys);
        drop(hydrate);
        tx.commit().map_err(map_sqlite)?;
        Ok(HistoryPage {
            start_cursor,
            rows,
            has_more,
        })
    })
}

pub(crate) fn check_raw_row(row: &Row<'_>, max_bytes: usize) -> Result<()> {
    let mut bytes = 0usize;
    for index in 0..row.as_ref().column_count() {
        let size = match row.get_ref(index).map_err(map_sqlite)? {
            ValueRef::Text(value) | ValueRef::Blob(value) => value.len(),
            _ => 8,
        };
        bytes = bytes.checked_add(size).ok_or_else(oversized)?;
        if bytes > max_bytes {
            return Err(oversized());
        }
    }
    Ok(())
}

/// Bounded single-row lookup used by metadata/root/lease paths too. No string
/// or blob is cloned by a decoder before the borrowed-value budget check.
pub(crate) fn one<T>(
    db: &Database,
    sql: &str,
    params: impl rusqlite::Params,
    max_bytes: Option<usize>,
    decode: impl FnOnce(&Row<'_>) -> Result<T>,
) -> Result<Option<T>> {
    db.with_conn(|connection| one_on_connection(connection, sql, params, max_bytes, decode))
}

pub(crate) fn one_on_connection<T>(
    connection: &rusqlite::Connection,
    sql: &str,
    params: impl rusqlite::Params,
    max_bytes: Option<usize>,
    decode: impl FnOnce(&Row<'_>) -> Result<T>,
) -> Result<Option<T>> {
    if max_bytes.is_some_and(|limit| !(1..=MAX_HISTORY_BYTES).contains(&limit)) {
        return Err(CoreError::invalid("invalid history metadata byte budget"));
    }
    let mut statement = connection.prepare(sql).map_err(map_sqlite)?;
    let mut rows = statement.query(params).map_err(map_sqlite)?;
    let Some(row) = rows.next().map_err(map_sqlite)? else {
        return Ok(None);
    };
    if let Some(limit) = max_bytes {
        check_raw_row(row, limit)?;
    }
    decode(row).map(Some)
}

fn oversized() -> CoreError {
    CoreError::invalid("history_item_too_large: persisted projection exceeds page budget")
}

/// Count JSON bytes without allocating another serialized copy of a payload.
pub fn json_len(value: &impl Serialize, max_bytes: usize) -> Result<usize> {
    struct Counter {
        bytes: usize,
        max: usize,
    }
    impl Write for Counter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if bytes.len() > self.max - self.bytes {
                return Err(std::io::Error::other("history JSON byte budget exceeded"));
            }
            self.bytes += bytes.len();
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut counter = Counter {
        bytes: 0,
        max: max_bytes,
    };
    serde_json::to_writer(&mut counter, value).map_err(|_| oversized())?;
    Ok(counter.bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{event_store::EventStore, run_store::RunStore};
    use deepagent_core::{clock::Timestamp, id::SessionId};
    use serde_json::{json, Value};

    const EVENTS: HistoryQuery = HistoryQuery {
        source: HistorySource::Events,
        columns: "sequence,data",
        table: "run_events",
        scope_filter: "run_id=?1",
        number: "sequence",
        id: "''",
        descending: false,
    };

    fn fixture(count: usize) -> Database {
        let db = Database::open_in_memory().unwrap();
        let session = SessionId::new();
        EventStore::new(&db)
            .create_session(session, None, Timestamp::from_millis(1))
            .unwrap();
        RunStore::new(&db)
            .create("r", &session.to_string(), None, 1)
            .unwrap();
        for index in 0..count {
            RunStore::new(&db)
                .append_event(
                    "r",
                    2,
                    "running",
                    "completed",
                    "synthetic",
                    &json!({"text":format!("row-{index}")}),
                )
                .unwrap();
        }
        db
    }

    fn decode(row: &Row<'_>) -> Result<Value> {
        let sequence: i64 = row.get(0).map_err(map_sqlite)?;
        let data: String = row.get(1).map_err(map_sqlite)?;
        Ok(json!({"sequence":sequence,"data":serde_json::from_str::<Value>(&data)?}))
    }

    fn budget(max_rows: usize) -> HistoryReadBudget {
        HistoryReadBudget {
            snapshot: None,
            max_rows,
            max_bytes: 2048,
        }
    }

    #[test]
    fn h218_sql_keyset_roundtrip_frozen_append_and_retry() {
        let db = fixture(17);
        let expected: Vec<_> = RunStore::new(&db)
            .events_after("r", None)
            .unwrap()
            .into_iter()
            .map(|event| json!({"sequence":event.sequence,"data":event.data}))
            .collect();
        let mut cursor = None;
        let mut actual = Vec::new();
        for index in 0..15 {
            let page = read(&db, &EVENTS, "r", cursor.as_ref(), budget(3), decode).unwrap();
            assert!(page.rows.len() <= 3);
            let retry = read(&db, &EVENTS, "r", cursor.as_ref(), budget(3), decode).unwrap();
            assert_eq!(
                retry.rows.iter().map(|row| &row.value).collect::<Vec<_>>(),
                page.rows.iter().map(|row| &row.value).collect::<Vec<_>>()
            );
            cursor = page.rows.last().map(|row| row.cursor.clone());
            actual.extend(page.rows.into_iter().map(|row| row.value));
            if index == 0 {
                RunStore::new(&db)
                    .append_event("r", 3, "running", "completed", "late", &json!({}))
                    .unwrap();
            }
            if !page.has_more {
                break;
            }
        }
        assert_eq!(actual, expected);
    }

    #[test]
    fn h218_sql_does_not_decode_the_limit_probe_or_oversized_borrowed_text() {
        let db = fixture(5);
        let mut decoded = 0;
        let page = read(&db, &EVENTS, "r", None, budget(2), |row| {
            decoded += 1;
            decode(row)
        })
        .unwrap();
        assert_eq!(decoded, 2);
        assert!(page.has_more);
        db.with_conn(|conn| {
            conn.execute(
                "UPDATE run_events SET data=?1 WHERE run_id='r' AND sequence=0",
                ["x".repeat(16 * 1024)],
            )
            .map_err(map_sqlite)?;
            Ok(())
        })
        .unwrap();
        let mut attempted_decode = false;
        let error = read(&db, &EVENTS, "r", None, budget(2), |row| {
            attempted_decode = true;
            decode(row)
        })
        .unwrap_err();
        assert!(
            !attempted_decode,
            "oversized text was cloned before the budget guard"
        );
        assert!(error.to_string().contains("history_item_too_large"));
    }

    #[test]
    fn h218_sql_cursors_bind_repository_scope_and_reject_invalid_limits() {
        let db = fixture(5);
        let first = read(&db, &EVENTS, "r", None, budget(2), decode).unwrap();
        let cursor = &first.rows.last().unwrap().cursor;
        assert!(read(&db, &EVENTS, "other", Some(cursor), budget(2), decode).is_err());
        let query = HistoryQuery {
            source: HistorySource::Runs,
            ..EVENTS
        };
        assert!(read(&db, &query, "r", Some(cursor), budget(2), decode).is_err());
        for limit in [0, 501] {
            assert!(read(&db, &EVENTS, "r", None, budget(limit), decode).is_err());
        }
    }

    #[test]
    fn h218_json_budget_counts_utf8_and_escaping_without_payload_buffer() {
        let value = json!({"text":"中文\n\"\\".repeat(32)});
        let exact = serde_json::to_vec(&value).unwrap().len();
        assert_eq!(json_len(&value, exact).unwrap(), exact);
        assert!(json_len(&value, exact - 1).is_err());
    }

    #[test]
    fn h218_shared_snapshot_excludes_append_before_a_later_run_view_is_opened() {
        let db = fixture(5);
        let runs = RunStore::new(&db);
        let session = runs.get("r").unwrap().unwrap().session_id;
        runs.create("later-view", &session, None, 2).unwrap();
        let cut = HistorySnapshot::capture(&db).unwrap();
        runs.append_event(
            "later-view",
            3,
            "running",
            "completed",
            "late",
            &json!({"marker":"late"}),
        )
        .unwrap();
        runs.create("late-run", &session, None, 3).unwrap();
        let frozen = HistoryReadBudget {
            snapshot: Some(cut),
            ..budget(3)
        };
        let page = runs.events_page("later-view", None, None, frozen).unwrap();
        assert_eq!(page.rows.len(), 1);
        assert_eq!(page.rows[0].value.event_type, "run_accepted");
        let run_page = runs.list_for_session_page(&session, None, frozen).unwrap();
        assert_eq!(run_page.rows.len(), 2);
        assert!(run_page.rows.iter().all(|row| row.value.id != "late-run"));
        let checkpoint = run_page.start_cursor;
        let retry = runs
            .list_for_session_page(&session, Some(&checkpoint), frozen)
            .unwrap();
        assert_eq!(retry.rows.len(), 2);
    }

    fn artifact_record(id: &str) -> crate::artifact_store::ArtifactRecord {
        use crate::artifact_store::{ArtifactKind, ArtifactRecord};
        ArtifactRecord {
            id: id.into(),
            kind: ArtifactKind::ToolResult,
            run_id: Some("r".into()),
            call_id: None,
            workspace_id: None,
            path: format!("synthetic-{id}"),
            media_type: None,
            byte_size: 0,
            digest: None,
            created_at: 2,
        }
    }

    fn assert_reused_high_water_rowid_is_rejected(use_shared_snapshot: bool) {
        use crate::artifact_store::ArtifactStore;

        let db = fixture(0);
        let artifacts = ArtifactStore::new(&db);
        let put = |id: &str| {
            artifacts.put_new(&artifact_record(id)).unwrap();
        };
        put("a");
        put("b");
        put("c");

        let snapshot = HistorySnapshot::capture(&db).unwrap();
        let budget = HistoryReadBudget {
            snapshot: use_shared_snapshot.then_some(snapshot),
            ..budget(1)
        };
        let page = read(
            &db,
            &HistoryQuery {
                source: HistorySource::Artifacts,
                columns: "id",
                table: "artifacts",
                scope_filter: "run_id=?1",
                number: "created_at",
                id: "id",
                descending: false,
            },
            "r",
            None,
            budget,
            |row| row.get::<_, String>(0).map_err(map_sqlite),
        )
        .unwrap();
        assert_eq!(page.rows[0].value, "a");
        assert!(page.has_more);

        assert!(artifacts.delete("c").unwrap());
        put("d");

        let error = read(
            &db,
            &HistoryQuery {
                source: HistorySource::Artifacts,
                columns: "id",
                table: "artifacts",
                scope_filter: "run_id=?1",
                number: "created_at",
                id: "id",
                descending: false,
            },
            "r",
            page.rows.last().map(|row| &row.cursor),
            budget,
            |row| row.get::<_, String>(0).map_err(map_sqlite),
        )
        .unwrap_err();
        assert!(error.to_string().contains("stale"), "{error}");
    }

    #[test]
    fn h218_snapshot_and_cursor_reject_reused_high_water_rowid_after_delete() {
        assert_reused_high_water_rowid_is_rejected(true);
        assert_reused_high_water_rowid_is_rejected(false);
    }

    #[test]
    fn h218_action_anchor_uses_the_complete_composite_identity() {
        use crate::run_control::{NewRunAction, RunControlStore};
        let db = fixture(0);
        let events = EventStore::new(&db);
        let runs = RunStore::new(&db);
        let old_session: SessionId = runs.get("r").unwrap().unwrap().session_id.parse().unwrap();
        let session = SessionId::new();
        events
            .create_session(session, None, Timestamp::from_millis(1))
            .unwrap();
        runs.create("later", &session.to_string(), None, 2).unwrap();
        let controls = RunControlStore::new(&db);
        let create = |run| {
            controls
                .create_action(&NewRunAction {
                    run_id: run,
                    turn_id: run,
                    call_id: "same-call",
                    sequence: 0,
                    tool_name: "synthetic",
                    arguments_hash: "synthetic",
                    risk: "low",
                    parent_action_id: None,
                    now: 3,
                })
                .unwrap()
        };
        create("r");
        let frozen = HistoryReadBudget {
            snapshot: Some(HistorySnapshot::capture(&db).unwrap()),
            ..budget(2)
        };
        let first = controls.list_actions_page("later", None, frozen).unwrap();
        assert!(first.rows.is_empty());
        events
            .trash_session(old_session, Timestamp::from_millis(4))
            .unwrap();
        events
            .purge_trashed_sessions_before(Timestamp::from_millis(5))
            .unwrap();
        assert!(events.purge_deleted_session_metadata(old_session).unwrap());
        create("later");
        let result = controls.list_actions_page("later", Some(&first.start_cursor), frozen);
        assert!(
            result.is_err(),
            "new run/call identity entered the old cut: {result:?}"
        );
        assert!(result.unwrap_err().to_string().contains("stale"));
    }

    #[test]
    fn h218_artifact_cut_survives_middle_delete_replace_reopen_and_vacuum() {
        use crate::artifact_store::ArtifactStore;
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("mutations.sqlite");
        let db = Database::open(&path).unwrap();
        let session = SessionId::new();
        EventStore::new(&db)
            .create_session(session, None, Timestamp::from_millis(1))
            .unwrap();
        RunStore::new(&db)
            .create("r", &session.to_string(), None, 1)
            .unwrap();
        let store = ArtifactStore::new(&db);
        for id in ["a", "b", "c", "d"] {
            store.put_new(&artifact_record(id)).unwrap();
        }
        let cut = HistorySnapshot::capture(&db).unwrap();
        let frozen = HistoryReadBudget {
            snapshot: Some(cut),
            ..budget(1)
        };
        let first = store.list_for_run_page("r", None, frozen).unwrap();
        assert_eq!(first.rows[0].value.id, "a");
        let cursor = first.rows[0].cursor.clone();
        assert!(store.delete("b").unwrap());
        store.put(&artifact_record("c")).unwrap();
        store.put_new(&artifact_record("late")).unwrap();
        let newer = HistorySnapshot::capture(&db).unwrap();
        assert!(store
            .list_for_run_page(
                "r",
                Some(&cursor),
                HistoryReadBudget {
                    snapshot: Some(newer),
                    ..frozen
                }
            )
            .is_err());
        drop(db);

        let db = Database::open(&path).unwrap();
        let remaining = ArtifactStore::new(&db)
            .list_for_run_page("r", Some(&cursor), frozen)
            .unwrap();
        assert_eq!(
            remaining
                .rows
                .iter()
                .map(|row| row.value.id.as_str())
                .collect::<Vec<_>>(),
            ["d"]
        );
        assert!(!remaining.has_more);

        db.with_conn(|connection| connection.execute_batch("VACUUM").map_err(map_sqlite))
            .unwrap();
        // A maintenance renumbering must either preserve the old identity cut
        // or fail closed; it must never admit a newly inserted identity.
        match ArtifactStore::new(&db).list_for_run_page("r", Some(&cursor), frozen) {
            Ok(page) => assert_eq!(
                page.rows
                    .iter()
                    .map(|row| row.value.id.as_str())
                    .collect::<Vec<_>>(),
                ["d"]
            ),
            Err(error) => assert!(error.to_string().contains("stale"), "{error}"),
        }
    }

    #[test]
    fn h218_subagent_resume_keeps_live_ownership_and_excludes_late_identity() {
        use crate::subagent_store::{SubagentRunRecord, SubagentRunStore};
        let db = fixture(0);
        let runs = RunStore::new(&db);
        let session = runs.get("r").unwrap().unwrap().session_id;
        runs.create("later", &session, None, 2).unwrap();
        let store = SubagentRunStore::new(&db);
        let record = |id: &str, parent: &str| SubagentRunRecord {
            id: id.into(),
            parent_run_id: parent.into(),
            origin_parent_run_id: parent.into(),
            state: "succeeded".into(),
            agent_type: "general".into(),
            transcript_path: None,
            worktree_path: None,
            summary: Some("synthetic".into()),
            created_at: 2,
            updated_at: 2,
            finished_at: Some(2),
            resume_count: 0,
        };
        store.create(&record("a", "r")).unwrap();
        store.create(&record("b", "r")).unwrap();
        let frozen = HistoryReadBudget {
            snapshot: Some(HistorySnapshot::capture(&db).unwrap()),
            ..budget(1)
        };
        let first = store.list_for_parent_page("r", None, frozen).unwrap();
        let new_parent_before = store.list_for_parent_page("later", None, frozen).unwrap();
        assert!(new_parent_before.rows.is_empty());
        store.resume_from_attempt("b", "later", 0, 3).unwrap();
        store.create(&record("late", "later")).unwrap();
        let origin = store
            .list_for_parent_page("r", Some(&first.rows[0].cursor), frozen)
            .unwrap();
        let current = store
            .list_for_parent_page("later", Some(&new_parent_before.start_cursor), frozen)
            .unwrap();
        for page in [origin, current] {
            assert_eq!(page.rows.len(), 1);
            assert!(!page.has_more);
            let child = &page.rows[0].value;
            assert_eq!(child.id, "b");
            assert_eq!(child.parent_run_id, "later");
            assert_eq!(child.origin_parent_run_id, "r");
            assert_eq!(child.state, "running");
            assert_eq!(child.resume_count, 1);
        }
    }

    #[test]
    fn h218_single_row_metadata_rejects_oversize_before_decoder() {
        let db = fixture(0);
        let mut decoded = false;
        db.with_conn(|conn| {
            conn.execute(
                "UPDATE runs SET terminal_reason=?1 WHERE id='r'",
                ["x".repeat(10000)],
            )
            .map_err(map_sqlite)?;
            Ok(())
        })
        .unwrap();
        let result = one(
            &db,
            "SELECT id,terminal_reason FROM runs WHERE id=?1",
            ["r"],
            Some(1024),
            |_| {
                decoded = true;
                Ok(())
            },
        );
        assert!(result.is_err());
        assert!(!decoded);
    }

    fn collect<T>(mut fetch: impl FnMut(Option<&HistoryCursor>) -> HistoryPage<T>) -> Vec<T> {
        let mut cursor = None;
        let mut items = Vec::new();
        for _ in 0..50 {
            let page = fetch(cursor.as_ref());
            assert!(page.rows.len() <= 3);
            if page.has_more {
                assert!(!page.rows.is_empty());
                let next = page.rows.last().unwrap().cursor.clone();
                assert_ne!(Some(&next), cursor.as_ref());
                cursor = Some(next);
            }
            items.extend(page.rows.into_iter().map(|row| row.value));
            if !page.has_more {
                return items;
            }
        }
        panic!("history keyset failed to terminate");
    }

    #[test]
    fn h218_real_repository_pages_preserve_all_projection_rows_and_tie_order() {
        use crate::artifact_store::{ArtifactKind, ArtifactRecord, ArtifactStore};
        use crate::run_control::{NewRunAction, NewRunApproval, RunControlStore};
        use crate::subagent_store::{SubagentRunRecord, SubagentRunStore};
        let db = fixture(10);
        let runs = RunStore::new(&db);
        let session = runs.get("r").unwrap().unwrap().session_id;
        let controls = RunControlStore::new(&db);
        let artifacts = ArtifactStore::new(&db);
        let children = SubagentRunStore::new(&db);
        for index in 0..10 {
            let call = format!("call-{index:02}");
            runs.create(&format!("run-{index:02}"), &session, None, 1)
                .unwrap();
            controls
                .create_action(&NewRunAction {
                    run_id: "r",
                    turn_id: "r",
                    call_id: &call,
                    sequence: index,
                    tool_name: "synthetic",
                    arguments_hash: "hash",
                    risk: "low",
                    parent_action_id: None,
                    now: 2,
                })
                .unwrap();
            controls
                .request_approval(&NewRunApproval {
                    approval_id: &format!("approval-{index:02}"),
                    run_id: "r",
                    call_id: &call,
                    scope: "once",
                    risk: "low",
                    reason: None,
                    policy_snapshot: None,
                    expires_at: None,
                    now: 2,
                })
                .unwrap();
            artifacts
                .put_new(&ArtifactRecord {
                    id: format!("artifact-{index:02}"),
                    kind: ArtifactKind::ToolResult,
                    run_id: Some("r".into()),
                    call_id: Some(call),
                    workspace_id: None,
                    path: format!("synthetic-not-created-{index}"),
                    media_type: None,
                    byte_size: 0,
                    digest: None,
                    created_at: 2,
                })
                .unwrap();
            children
                .create(&SubagentRunRecord {
                    id: format!("child-{index:02}"),
                    parent_run_id: "r".into(),
                    origin_parent_run_id: "r".into(),
                    state: "succeeded".into(),
                    agent_type: "general".into(),
                    transcript_path: None,
                    worktree_path: None,
                    summary: None,
                    created_at: 2,
                    updated_at: 2,
                    finished_at: Some(2),
                    resume_count: 0,
                })
                .unwrap();
        }
        let mut expected_runs = runs.list_for_session(&session).unwrap();
        expected_runs.sort_by(|a, b| (b.created_at, &b.id).cmp(&(a.created_at, &a.id)));
        assert_eq!(
            collect(|cursor| runs
                .list_for_session_page(&session, cursor, budget(3))
                .unwrap()),
            expected_runs
        );
        assert_eq!(
            collect(|cursor| runs.events_page("r", None, cursor, budget(3)).unwrap()),
            runs.events_after("r", None).unwrap()
        );
        assert_eq!(
            collect(|cursor| controls.list_actions_page("r", cursor, budget(3)).unwrap()),
            controls.list_actions("r").unwrap()
        );
        assert_eq!(
            collect(|cursor| controls
                .list_approvals_page("r", cursor, budget(3))
                .unwrap()),
            controls.list_approvals("r").unwrap()
        );
        assert_eq!(
            collect(|cursor| artifacts.list_for_run_page("r", cursor, budget(3)).unwrap()),
            artifacts.list_for_run("r").unwrap()
        );
        assert_eq!(
            collect(|cursor| children
                .list_for_parent_page("r", cursor, budget(3))
                .unwrap()),
            children.list_for_parent("r").unwrap()
        );
        assert!(runs
            .events_page("r", Some(u64::MAX), None, budget(3))
            .unwrap()
            .rows
            .is_empty());
        assert_eq!(
            collect(|cursor| runs.events_page("r", Some(5), cursor, budget(3)).unwrap()),
            runs.events_after("r", Some(5)).unwrap()
        );
    }
}
