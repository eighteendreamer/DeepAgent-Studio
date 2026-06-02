//! Cost ledger over the `costs` table (gap-closure spec, Phase 1B).
//!
//! Records one row per completed model call so the UI can show per-session and
//! cumulative spend, and the runtime can enforce optional budget limits. This
//! layer owns the SQL; `deepagent-app-core`'s `CostService` calls it without
//! touching `rusqlite` directly (keeping the dependency direction clean:
//! app-core -> persistence).

use deepagent_core::error::Result;
use rusqlite::params;

use crate::{map_sqlite, Database};

/// A single cost ledger row to insert.
#[derive(Debug, Clone, PartialEq)]
pub struct CostEntry<'a> {
    /// Session this cost belongs to.
    pub session_id: &'a str,
    /// Unix-millis timestamp of the call.
    pub timestamp: i64,
    /// Model identifier (e.g. `deepseek-v4-flash`).
    pub model: &'a str,
    /// Prompt tokens.
    pub input_tokens: u32,
    /// Completion tokens.
    pub output_tokens: u32,
    /// Cache-hit tokens (subset of input billed at the discounted rate).
    pub cache_hit_tokens: u32,
    /// Total tokens reported by the provider.
    pub total_tokens: u32,
    /// Computed cost in USD (column kept as `cost_yuan` for schema compatibility).
    pub cost_yuan: f64,
}

/// Repository over the `costs` table.
pub struct CostStore<'db> {
    db: &'db Database,
}

impl<'db> CostStore<'db> {
    /// Wrap a database handle.
    pub fn new(db: &'db Database) -> Self {
        Self { db }
    }

    /// Insert one cost record.
    pub fn insert(&self, entry: &CostEntry<'_>) -> Result<()> {
        self.db.with_conn(|c| {
            c.execute(
                "INSERT INTO costs (session_id, timestamp, model, input_tokens, output_tokens, \
                 cache_hit_tokens, total_tokens, cost_yuan) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    entry.session_id,
                    entry.timestamp,
                    entry.model,
                    entry.input_tokens,
                    entry.output_tokens,
                    entry.cache_hit_tokens,
                    entry.total_tokens,
                    entry.cost_yuan,
                ],
            )
            .map_err(map_sqlite)?;
            Ok(())
        })
    }

    /// Sum of `cost_yuan` for a single session.
    pub fn session_total(&self, session_id: &str) -> Result<f64> {
        self.db.with_conn(|c| {
            c.query_row(
                "SELECT COALESCE(SUM(cost_yuan), 0.0) FROM costs WHERE session_id = ?1",
                params![session_id],
                |row| row.get(0),
            )
            .map_err(map_sqlite)
        })
    }

    /// Sum of `cost_yuan` for all records at or after `since` (unix millis).
    pub fn total_since(&self, since: i64) -> Result<f64> {
        self.db.with_conn(|c| {
            c.query_row(
                "SELECT COALESCE(SUM(cost_yuan), 0.0) FROM costs WHERE timestamp >= ?1",
                params![since],
                |row| row.get(0),
            )
            .map_err(map_sqlite)
        })
    }

    /// All-time sum of `cost_yuan`.
    pub fn total(&self) -> Result<f64> {
        self.db.with_conn(|c| {
            c.query_row(
                "SELECT COALESCE(SUM(cost_yuan), 0.0) FROM costs",
                [],
                |row| row.get(0),
            )
            .map_err(map_sqlite)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry<'a>(session: &'a str, ts: i64, cost: f64) -> CostEntry<'a> {
        CostEntry {
            session_id: session,
            timestamp: ts,
            model: "deepseek-v4-flash",
            input_tokens: 100,
            output_tokens: 50,
            cache_hit_tokens: 0,
            total_tokens: 150,
            cost_yuan: cost,
        }
    }

    #[test]
    fn insert_and_session_total() {
        let db = Database::open_in_memory().unwrap();
        // Need a session row for the FK.
        db.with_conn(|c| {
            c.execute(
                "INSERT INTO sessions (id, title, mode, created_at, updated_at) \
                 VALUES ('ses_1', 't', 'normal', 0, 0)",
                [],
            )
            .map_err(map_sqlite)?;
            Ok(())
        })
        .unwrap();

        let store = CostStore::new(&db);
        store.insert(&entry("ses_1", 1_000, 0.01)).unwrap();
        store.insert(&entry("ses_1", 2_000, 0.02)).unwrap();

        let total = store.session_total("ses_1").unwrap();
        assert!((total - 0.03).abs() < 1e-9);
    }

    #[test]
    fn total_since_filters_by_timestamp() {
        let db = Database::open_in_memory().unwrap();
        db.with_conn(|c| {
            c.execute(
                "INSERT INTO sessions (id, title, mode, created_at, updated_at) \
                 VALUES ('ses_1', 't', 'normal', 0, 0)",
                [],
            )
            .map_err(map_sqlite)?;
            Ok(())
        })
        .unwrap();

        let store = CostStore::new(&db);
        store.insert(&entry("ses_1", 1_000, 0.01)).unwrap();
        store.insert(&entry("ses_1", 5_000, 0.05)).unwrap();

        assert!((store.total_since(3_000).unwrap() - 0.05).abs() < 1e-9);
        assert!((store.total().unwrap() - 0.06).abs() < 1e-9);
    }
}
