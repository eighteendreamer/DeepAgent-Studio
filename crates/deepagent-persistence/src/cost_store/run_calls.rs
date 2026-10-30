//! Request ownership in the existing run_events ledger. The cost row and its
//! receipt commit together; no session-file write is needed for accounting.

use std::collections::HashSet;

use deepagent_core::error::{CoreError, Result};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{insert_entry, CostEntry, CostStore};
use crate::{map_sqlite, run_store::insert_run_event};

const EVENT_TYPE: &str = "model_call_usage";

/// Host-owned attribution, not a provider response or a new cost ledger.
pub struct RunCallUsage<'a> {
    pub session_id: &'a str,
    pub run_id: &'a str,
    pub call_id: &'a str,
    pub timestamp: i64,
    /// Versioned, sanitized model/usage/purpose data supplied by the host.
    pub report: &'a Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RecordedCallCost {
    pub ledger_id: Option<i64>,
    pub cost_yuan: Option<f64>,
    pub inserted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunCallReceipt {
    version: u32,
    pub session_id: String,
    pub call_id: String,
    pub report: Value,
    pub ledger_id: Option<i64>,
    pub cost_yuan: Option<f64>,
}

fn decode(body: &str) -> Result<RunCallReceipt> {
    let receipt: RunCallReceipt = serde_json::from_str(body)
        .map_err(|_| CoreError::Persistence("invalid model call usage receipt".into()))?;
    if receipt.version != 1 {
        return Err(CoreError::Persistence(
            "unsupported model call usage receipt".into(),
        ));
    }
    Ok(receipt)
}

impl CostStore<'_> {
    /// Read the existing durable accounting receipts, including incomplete or
    /// failed bookings. A missing/wrong parent cannot authorize a paid call.
    pub fn run_call_receipts(&self, session_id: &str, run_id: &str) -> Result<Vec<RunCallReceipt>> {
        self.db.with_conn(|conn| {
            let owner: Option<String> = conn
                .query_row("SELECT session_id FROM runs WHERE id=?1", [run_id], |row| {
                    row.get(0)
                })
                .optional()
                .map_err(map_sqlite)?;
            if owner.as_deref() != Some(session_id) {
                return Err(CoreError::invalid("model call parent run/session mismatch"));
            }
            let mut statement = conn
                .prepare(
                    "SELECT data FROM run_events WHERE run_id=?1
                     AND event_type IN (?2, 'model_call_usage_failed') ORDER BY sequence",
                )
                .map_err(map_sqlite)?;
            let rows = statement
                .query_map(params![run_id, EVENT_TYPE], |row| row.get::<_, String>(0))
                .map_err(map_sqlite)?;
            rows.map(|row| {
                let receipt = decode(&row.map_err(map_sqlite)?)?;
                if receipt.session_id != session_id {
                    return Err(CoreError::Persistence(
                        "model call receipt session mismatch".into(),
                    ));
                }
                Ok(receipt)
            })
            .collect()
        })
    }

    /// Atomically book an answered/measured request and its parent ownership.
    /// A repeated identical identity is a no-op (including after reopen);
    /// conflicting attribution is an error. None means unreported usage or
    /// no configured cost owner, never a fabricated zero-token cost row.
    pub fn record_run_call(
        &self,
        call: &RunCallUsage<'_>,
        entry: Option<&CostEntry<'_>>,
    ) -> Result<RecordedCallCost> {
        if call.call_id.is_empty() || entry.is_some_and(|entry| entry.session_id != call.session_id)
        {
            return Err(CoreError::invalid("model call attribution mismatch"));
        }
        self.db.with_conn(|conn| {
            // Match RunStore's serialized append boundary, across independent
            // SQLite connections/processes as well as this Database mutex.
            conn.execute_batch("BEGIN IMMEDIATE").map_err(map_sqlite)?;
            let result = (|| {
                let owner: Option<(String, String)> = conn.query_row(
                    "SELECT session_id, state FROM runs WHERE id=?1", [call.run_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                ).optional().map_err(map_sqlite)?;
                let (owner, phase) = owner.ok_or_else(|| CoreError::invalid("model call parent run missing"))?;
                if owner != call.session_id {
                    return Err(CoreError::invalid("model call parent run/session mismatch"));
                }
                let mut stmt = conn.prepare(
                    "SELECT data FROM run_events WHERE run_id=?1 AND event_type=?2 ORDER BY sequence",
                ).map_err(map_sqlite)?;
                let rows = stmt.query_map(params![call.run_id, EVENT_TYPE], |row| row.get::<_, String>(0))
                    .map_err(map_sqlite)?;
                for row in rows {
                    let old = decode(&row.map_err(map_sqlite)?)?;
                    if old.call_id == call.call_id {
                        if old.session_id != call.session_id || old.report != *call.report {
                            return Err(CoreError::invalid("conflicting model call receipt"));
                        }
                        return Ok(RecordedCallCost {
                            ledger_id: old.ledger_id, cost_yuan: old.cost_yuan, inserted: false,
                        });
                    }
                }
                drop(stmt);
                let ledger_id = entry.map(|entry| insert_entry(conn, entry)).transpose()?;
                let cost_yuan = entry.and_then(|entry| entry.cost_yuan);
                let receipt = serde_json::json!({
                    "version":1, "session_id":call.session_id, "call_id":call.call_id,
                    "report":call.report, "ledger_id":ledger_id, "cost_yuan":cost_yuan,
                });
                insert_run_event(conn, call.run_id, call.timestamp, &phase, EVENT_TYPE, &receipt)?;
                Ok(RecordedCallCost { ledger_id, cost_yuan, inserted: true })
            })();
            match result {
                Ok(value) => {
                    if let Err(error) = conn.execute_batch("COMMIT") {
                        let _ = conn.execute_batch("ROLLBACK");
                        return Err(map_sqlite(error));
                    }
                    Ok(value)
                }
                Err(error) => {
                    let _ = conn.execute_batch("ROLLBACK");
                    Err(error)
                }
            }
        })
    }

    /// Exclude explicitly owned physical-call rows from legacy positional turn
    /// replay. Modern turn receipts reference their identities explicitly;
    /// side calls are session spend, not the main context/turn cost gauge.
    pub fn run_call_ledger_ids(&self, session_id: &str) -> Result<HashSet<i64>> {
        self.db.with_conn(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT e.data FROM run_events e JOIN runs r ON r.id=e.run_id
                 WHERE r.session_id=?1 AND e.event_type=?2",
                )
                .map_err(map_sqlite)?;
            let rows = stmt
                .query_map(params![session_id, EVENT_TYPE], |row| {
                    row.get::<_, String>(0)
                })
                .map_err(map_sqlite)?;
            let mut ids = HashSet::new();
            for row in rows {
                let receipt = decode(&row.map_err(map_sqlite)?)?;
                if receipt.session_id != session_id {
                    return Err(CoreError::Persistence(
                        "model call receipt session mismatch".into(),
                    ));
                }
                ids.extend(receipt.ledger_id);
            }
            Ok(ids)
        })
    }
}

#[cfg(test)]
#[path = "run_calls_tests.rs"]
mod tests;
