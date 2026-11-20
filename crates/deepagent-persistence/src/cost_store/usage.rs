use deepagent_core::error::Result;
use rusqlite::params;

use super::CostStore;
use crate::map_sqlite;

/// Project the existing cost rows and their optional physical-request receipt.
/// Old rows remain readable, without inferring which legacy turn owned them.
pub struct UsageLedgerRow {
    pub id: i64,
    pub timestamp: i64,
    pub model: String,
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub cache_write_hour: u64,
}

impl CostStore<'_> {
    pub fn usage_rows(
        &self,
        session: Option<&str>,
        start: i64,
        end: i64,
        ids: Option<&[i64]>,
    ) -> Result<Vec<UsageLedgerRow>> {
        let ids = ids.map(|ids| serde_json::json!(ids).to_string());
        self.db.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "WITH receipts AS (
                     SELECT r.session_id, CAST(json_extract(e.data,'$.ledger_id') AS INTEGER) AS ledger_id,
                       MAX(r.created_at) AS turn_start,
                       MAX(COALESCE(json_extract(e.data,'$.report.raw_usage.cache_creation_input_tokens'),0)) AS written,
                       MAX(COALESCE(json_extract(e.data,'$.report.raw_usage.cache_creation.ephemeral_1h_input_tokens'),0)) AS written_hour
                     FROM run_events e JOIN runs r ON r.id=e.run_id
                     WHERE e.event_type='model_call_usage' AND json_valid(e.data)
                       AND (?1 IS NULL OR r.session_id=?1)
                       AND json_extract(e.data,'$.ledger_id') IS NOT NULL
                       AND (?4 IS NULL OR json_extract(e.data,'$.ledger_id') IN (SELECT value FROM json_each(?4)))
                     GROUP BY r.session_id, ledger_id
                 )
                 SELECT c.id, COALESCE(r.turn_start,c.timestamp), c.model,
                   c.input_tokens, c.output_tokens, c.cache_hit_tokens,
                   COALESCE(r.written,0), COALESCE(r.written_hour,0)
                 FROM costs c LEFT JOIN receipts r ON r.ledger_id=c.id AND r.session_id=c.session_id
                 WHERE (?1 IS NULL OR c.session_id=?1)
                   AND (?4 IS NULL OR c.id IN (SELECT value FROM json_each(?4)))
                   AND COALESCE(r.turn_start,c.timestamp)>=?2 AND COALESCE(r.turn_start,c.timestamp)<?3"
            ).map_err(map_sqlite)?;
            let rows = stmt.query_map(params![session,start,end,ids], |row| Ok(UsageLedgerRow {
                id:row.get(0)?, timestamp:row.get(1)?, model:row.get(2)?, input:row.get(3)?,
                output:row.get(4)?, cache_read:row.get(5)?, cache_write:row.get(6)?, cache_write_hour:row.get(7)?,
            })).map_err(map_sqlite)?;
            rows.map(|row| row.map_err(map_sqlite)).collect()
        })
    }
}
