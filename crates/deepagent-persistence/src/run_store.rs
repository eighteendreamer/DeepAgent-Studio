//! Durable Agent Kernel v2 run state and ordered event ledger.

use deepagent_core::error::{CoreError, Result};
use rusqlite::{params, OptionalExtension, Transaction, TransactionBehavior};
use serde::{Deserialize, Serialize};

use crate::bounded_read::{
    self, HistoryCursor, HistoryPage, HistoryQuery, HistoryReadBudget, HistorySource,
};
use crate::{map_sqlite, Database};

#[cfg(test)]
#[path = "run_store/continuation_tests.rs"]
mod continuation_tests;

#[cfg(test)]
#[path = "run_store/capability_tests.rs"]
mod capability_tests;

#[cfg(test)]
#[path = "run_store/admission_tests.rs"]
mod admission_tests;

#[path = "run_store/scheduled.rs"]
mod scheduled;
pub use scheduled::ScheduledRunOrigin;

#[cfg(test)]
#[path = "run_store/scheduled_tests.rs"]
mod scheduled_tests;

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

/// The stable registry evidence carried by both legacy and richer run-owned
/// `capability.snapshot` events. Runtime computes it; storage compares it in
/// the same transaction that checks ownership and writes the first snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegistryCapabilityFingerprint {
    pub tool_count: usize,
    pub tool_schema_hash: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
/// Ordered ledger record. `run_terminal` is the unique committed outcome,
/// not necessarily the final record: a versioned `session_terminal_projected`
/// acknowledgement, model usage receipts and auxiliary lifecycles may follow
/// it. Replay consumers must keep those records and
/// use `event_type`, not the last array entry, to identify the outcome.
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

/// Borrowed input for the existing atomic terminal transaction. This is not a
/// wire DTO: the runtime owns terminal semantics and notification projection.
pub struct RunTerminalUpdate<'a> {
    pub kind: &'a str,
    pub reason: Option<&'a str>,
    pub data: &'a serde_json::Value,
}

/// One admission into the existing ledger. An optional replacement claim and
/// initial capability snapshot commit with the run, never after its ACK.
/// This is storage input, not a protocol DTO or execution authorization.
pub struct RunAdmission<'a> {
    pub run_id: &'a str,
    pub session_id: &'a str,
    pub task_id: Option<&'a str>,
    pub replaces_run_id: Option<&'a str>,
    pub capability_snapshot: Option<&'a serde_json::Value>,
    pub scheduled: Option<&'a ScheduledRunOrigin>,
}

/// Storage input for the two run-owned schema domains, not a wire DTO or
/// authorization grant. Each domain advances independently in the same ledger.
pub enum CapabilitySchema<'a> {
    Model(&'a serde_json::Value),
    Wire(&'a serde_json::Value),
}

// The canonical terminal doubles as a durable projection intent. Confirmation
// lives in the same ledger, never in a second store or a rebuildable file cursor.
const PENDING_TERMINAL_PROJECTION: &str = "r.finished_at IS NOT NULL
    AND EXISTS (SELECT 1 FROM run_events e WHERE e.run_id=r.id
                AND e.event_type IN ('run_terminal', 'run_recovered_after_startup'))
    AND NOT EXISTS (SELECT 1 FROM run_events e WHERE e.run_id=r.id
                    AND e.event_type='session_terminal_projected')";

fn insert_run(
    conn: &rusqlite::Connection,
    run_id: &str,
    session_id: &str,
    task_id: Option<&str>,
    now: i64,
) -> Result<()> {
    conn.execute(
        "INSERT INTO runs (id, session_id, task_id, state, created_at, updated_at) VALUES (?1, ?2, ?3, 'accepted', ?4, ?4)",
        params![run_id, session_id, task_id, now],
    ).map_err(map_sqlite)?;
    conn.execute(
        "INSERT INTO run_events (run_id, sequence, timestamp, phase, status, event_type, data) VALUES (?1, 0, ?2, 'accepted', 'completed', 'run_accepted', ?3)",
        params![run_id, now, serde_json::json!({"session_id":session_id,"task_id":task_id}).to_string()],
    ).map_err(map_sqlite)?;
    Ok(())
}

pub(crate) fn insert_run_event(
    conn: &rusqlite::Connection,
    run_id: &str,
    now: i64,
    phase: &str,
    event_type: &str,
    data: &serde_json::Value,
) -> Result<()> {
    conn.execute(
        "INSERT INTO run_events (run_id, sequence, timestamp, phase, status, event_type, data)
         SELECT ?1, COALESCE(MAX(sequence), -1) + 1, ?2, ?3, 'completed', ?4, ?5
         FROM run_events WHERE run_id=?1",
        params![run_id, now, phase, event_type, data.to_string()],
    )
    .map_err(map_sqlite)?;
    Ok(())
}

fn capability_state(
    conn: &rusqlite::Connection,
    run_id: &str,
) -> Result<(String, Option<serde_json::Value>)> {
    let owner: Option<(String, Option<i64>)> = conn
        .query_row(
            "SELECT session_id, finished_at FROM runs WHERE id=?1",
            [run_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(map_sqlite)?;
    let Some((session_id, None)) = owner else {
        return Err(CoreError::invalid(
            "capability snapshot requires an admitted nonterminal run",
        ));
    };
    let mut query = conn.prepare(
        "SELECT data FROM run_events WHERE run_id=?1 AND event_type='capability.snapshot' ORDER BY sequence LIMIT 2",
    ).map_err(map_sqlite)?;
    let data = query
        .query_map([run_id], |row| row.get::<_, String>(0))
        .map_err(map_sqlite)?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(map_sqlite)?;
    if data.len() > 1 {
        return Err(CoreError::invalid("run has duplicate capability snapshots"));
    }
    let snapshot = data
        .into_iter()
        .next()
        .map(|data| serde_json::from_str(&data))
        .transpose()?;
    Ok((session_id, snapshot))
}

fn validate_capability_identity(
    snapshot: &serde_json::Value,
    run_id: &str,
    session_id: &str,
) -> Result<()> {
    if snapshot.get("version").and_then(serde_json::Value::as_u64) != Some(1) {
        return Err(CoreError::invalid(
            "unsupported run capability snapshot version",
        ));
    }
    if snapshot.get("runId").and_then(serde_json::Value::as_str) != Some(run_id)
        || snapshot.get("threadId").and_then(serde_json::Value::as_str) != Some(session_id)
    {
        return Err(CoreError::invalid(
            "capability snapshot does not match the admitted owner",
        ));
    }
    Ok(())
}

fn validate_capability_generation(
    data: &serde_json::Value,
    snapshot_id: &str,
    projection: &str,
) -> Result<()> {
    if data.get("version").and_then(serde_json::Value::as_u64) != Some(1)
        || data.get("snapshotId").and_then(serde_json::Value::as_str) != Some(snapshot_id)
        || !data
            .get(projection)
            .is_some_and(serde_json::Value::is_object)
    {
        return Err(CoreError::invalid(
            "capability generation does not match its snapshot",
        ));
    }
    Ok(())
}

fn schema_generation_number(projection: &serde_json::Value) -> Result<u64> {
    if !projection
        .get("schemaHash")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|hash| !hash.is_empty())
    {
        return Err(CoreError::invalid("capability schema has no hash"));
    }
    projection
        .get("generation")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| CoreError::invalid("capability schema has no generation"))
}

fn bind_capability_snapshot(
    conn: &rusqlite::Connection,
    run_id: &str,
    snapshot: &serde_json::Value,
    now: i64,
) -> Result<()> {
    let (session_id, bound) = capability_state(conn, run_id)?;
    validate_capability_identity(snapshot, run_id, &session_id)?;
    if bound.is_some() {
        return Err(CoreError::invalid(
            "run capability snapshot is already bound",
        ));
    }
    insert_run_event(
        conn,
        run_id,
        now,
        "preparing",
        "capability.snapshot",
        snapshot,
    )
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
        self.admit(
            RunAdmission {
                run_id,
                session_id,
                task_id,
                replaces_run_id: None,
                capability_snapshot: None,
                scheduled: None,
            },
            now,
        )
        .map(|_| ())
    }

    /// False means the expected predecessor is missing, terminal, foreign or
    /// already replaced. All writes (including its claim) roll back on error.
    /// Existing create/continuation callers use this same transaction without
    /// a snapshot; prepared chat admissions supply the immutable initial one.
    pub fn admit(&self, admission: RunAdmission<'_>, now: i64) -> Result<bool> {
        self.db.with_conn(|conn| {
            let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)
                .map_err(map_sqlite)?;
            if let Some(old) = admission.replaces_run_id {
                let eligible: bool = tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM runs r WHERE r.id=?1 AND r.session_id=?2 AND r.finished_at IS NULL
                     AND NOT EXISTS(SELECT 1 FROM run_events e WHERE e.run_id=r.id AND e.event_type='continuation.created'))",
                    params![old, admission.session_id], |row| row.get(0),
                ).map_err(map_sqlite)?;
                if !eligible {
                    return Ok(false);
                }
            }
            insert_run(&tx, admission.run_id, admission.session_id, admission.task_id, now)?;
            if let Some(snapshot) = admission.capability_snapshot {
                bind_capability_snapshot(&tx, admission.run_id, snapshot, now)?;
            }
            if let Some(origin) = admission.scheduled {
                origin.validate(admission.run_id)?;
                if admission.replaces_run_id.is_some() {
                    return Err(CoreError::invalid("a scheduled admission cannot replace a run"));
                }
                insert_run_event(&tx, admission.run_id, now, "accepted", "schedule.accepted", &serde_json::to_value(origin)?)?;
            }
            if let Some(old) = admission.replaces_run_id {
                crate::run_control::append_control_event(
                    &tx, old, now, "continuation.created",
                    &serde_json::json!({"replacesTurnId":old,"newTurnId":admission.run_id,"disposition":"new_turn"}),
                )?;
            }
            tx.commit().map_err(map_sqlite)?;
            Ok(true)
        })
    }

    /// Bind the one immutable capability snapshot to an admitted, nonterminal
    /// owner. The eligibility check and append share the ledger transaction,
    /// including across independently opened database handles. The caller owns
    /// the capability semantics; storage only enforces identity and lifecycle.
    pub fn bind_capability_snapshot(
        &self,
        run_id: &str,
        snapshot: &serde_json::Value,
        now: i64,
    ) -> Result<()> {
        self.db.with_conn(|conn| {
            let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)
                .map_err(map_sqlite)?;
            bind_capability_snapshot(&tx, run_id, snapshot, now)?;
            tx.commit().map_err(map_sqlite)?;
            Ok(())
        })
    }

    /// Verify a richer admitted snapshot, or atomically write the legacy
    /// registry-only shape. No read/append window remains for direct kernels;
    /// concurrent identical callers reuse one event, conflicting callers fail.
    pub fn ensure_registry_capabilities(
        &self,
        run_id: &str,
        fingerprint: &RegistryCapabilityFingerprint,
        now: i64,
    ) -> Result<()> {
        self.db.with_conn(|conn| {
            let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)
                .map_err(map_sqlite)?;
            let (session_id, bound) = capability_state(&tx, run_id)?;
            if let Some(snapshot) = bound {
                // The only legacy contract is the original two-field shape.
                // A damaged rich snapshot must not silently lose its owner or
                // future version by being reinterpreted as legacy evidence.
                let legacy = snapshot.as_object().is_some_and(|fields| {
                    fields.len() == 2
                        && fields.contains_key("toolCount")
                        && fields.contains_key("toolSchemaHash")
                });
                if !legacy {
                    validate_capability_identity(&snapshot, run_id, &session_id)?;
                }
                let bound: RegistryCapabilityFingerprint = serde_json::from_value(snapshot)?;
                if &bound != fingerprint {
                    return Err(CoreError::invalid(
                        "admitted capability snapshot does not match execution registry",
                    ));
                }
            } else {
                insert_run_event(
                    &tx,
                    run_id,
                    now,
                    "preparing",
                    "capability.snapshot",
                    &serde_json::to_value(fingerprint)?,
                )?;
            }
            tx.commit().map_err(map_sqlite)?;
            Ok(())
        })
    }

    /// Publish or verify one capability generation under the admitted owner.
    /// Even an unchanged projection checks lifecycle inside this transaction;
    /// terminal commits and independent writers cannot race a stale success.
    /// Generic post-terminal receipts remain supported by append_event.
    pub fn record_capability_schema(
        &self,
        run_id: &str,
        snapshot_id: &str,
        schema: CapabilitySchema<'_>,
        now: i64,
    ) -> Result<()> {
        let (event_type, key, projection) = match schema {
            CapabilitySchema::Model(value) => ("capability_schema_snapshot", "modelSchema", value),
            CapabilitySchema::Wire(value) => ("capability_wire_snapshot", "wireSchema", value),
        };
        self.db.with_conn(|conn| {
            let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)
                .map_err(map_sqlite)?;
            let (session_id, bound) = capability_state(&tx, run_id)?;
            let snapshot = bound.ok_or_else(|| CoreError::invalid("run has no capability snapshot"))?;
            validate_capability_identity(&snapshot, run_id, &session_id)?;
            if snapshot_id.is_empty() || snapshot["snapshotId"].as_str() != Some(snapshot_id) {
                return Err(CoreError::invalid("capability schema does not match its admitted snapshot"));
            }
            let latest: Option<String> = tx.query_row(
                "SELECT data FROM run_events WHERE run_id=?1 AND event_type=?2 ORDER BY sequence DESC LIMIT 1",
                params![run_id, event_type], |row| row.get(0),
            ).optional().map_err(map_sqlite)?;
            let current = match latest {
                Some(data) => {
                    let data: serde_json::Value = serde_json::from_str(&data)?;
                    validate_capability_generation(&data, snapshot_id, key)?;
                    Some(data[key].clone())
                }
                None => snapshot.get(key).cloned(),
            };
            let number = schema_generation_number(projection)?;
            if let Some(current) = &current {
                schema_generation_number(current)?;
            }
            if current.as_ref() != Some(projection) {
                let expected = match current {
                    Some(current) => schema_generation_number(&current)?.checked_add(1)
                        .ok_or_else(|| CoreError::invalid("capability schema generation exhausted"))?,
                    None => 0,
                };
                if number != expected {
                    return Err(CoreError::invalid("stale or nonconsecutive capability schema generation"));
                }
                let mut data = serde_json::json!({"version":1, "snapshotId":snapshot_id});
                data[key] = projection.clone();
                tx.execute(
                    "INSERT INTO run_events (run_id, sequence, timestamp, phase, status, event_type, data)
                     SELECT ?1, COALESCE(MAX(sequence), -1) + 1, ?2, 'running_turn', 'ready', ?3, ?4
                     FROM run_events WHERE run_id=?1",
                    params![run_id, now, event_type, data.to_string()],
                ).map_err(map_sqlite)?;
            }
            tx.commit().map_err(map_sqlite)
        })
    }

    /// Claim one replacement and admit it in the existing ledger transaction.
    /// False means the expected run is missing, terminal, foreign or already
    /// replaced. No cancellation, admission or new continuation is then written.
    pub fn create_continuation(
        &self,
        replaces_run_id: &str,
        new_run_id: &str,
        session_id: &str,
        now: i64,
    ) -> Result<bool> {
        self.admit(
            RunAdmission {
                run_id: new_run_id,
                session_id,
                task_id: None,
                replaces_run_id: Some(replaces_run_id),
                capability_snapshot: None,
                scheduled: None,
            },
            now,
        )
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

    /// Bind a kernel task to an admitted run without changing its identity.
    pub fn bind_task(&self, run_id: &str, session_id: &str, task_id: &str) -> Result<()> {
        self.db.with_conn(|conn| {
            let changed = conn.execute(
                "UPDATE runs SET task_id=?3 WHERE id=?1 AND session_id=?2 AND finished_at IS NULL AND (task_id IS NULL OR task_id=?3)",
                params![run_id, session_id, task_id],
            ).map_err(map_sqlite)?;
            if changed != 1 {
                return Err(CoreError::invalid("run admission does not match the kernel task"));
            }
            Ok(())
        })
    }

    /// Commit cancellation audit, terminal event and terminal state atomically.
    /// Repeated finalizers cannot append a second terminal boundary.
    pub fn finish_with_event(
        &self,
        run_id: &str,
        terminal_kind: &str,
        reason: Option<&str>,
        terminal: &serde_json::Value,
        cancel_alias: Option<&str>,
        now: i64,
    ) -> Result<bool> {
        self.commit_terminal(
            run_id,
            RunTerminalUpdate {
                kind: terminal_kind,
                reason,
                data: terminal,
            },
            cancel_alias,
            None,
            None,
            now,
        )
    }

    /// A lifecycle notification cannot be replayed without its canonical
    /// outcome. Both rows and the run update commit, or all roll back.
    pub fn finish_with_notification(
        &self,
        run_id: &str,
        terminal: RunTerminalUpdate<'_>,
        notification: (&str, &serde_json::Value),
        cancel_alias: Option<&str>,
        now: i64,
    ) -> Result<bool> {
        self.commit_terminal(
            run_id,
            terminal,
            cancel_alias,
            None,
            Some(notification),
            now,
        )
    }

    /// Recovery uses the very same terminal transaction as normal execution.
    /// A failed UPDATE or event INSERT cannot leave an extra recovery marker.
    pub fn finish_recovered(
        &self,
        run_id: &str,
        reason: &str,
        recovery: &serde_json::Value,
        now: i64,
    ) -> Result<bool> {
        let terminal = serde_json::json!({"kind":"failed", "message":null, "reason":reason});
        self.commit_terminal(
            run_id,
            RunTerminalUpdate {
                kind: "failed",
                reason: Some(reason),
                data: &terminal,
            },
            None,
            Some(recovery),
            None,
            now,
        )
    }

    fn commit_terminal(
        &self,
        run_id: &str,
        terminal: RunTerminalUpdate<'_>,
        cancel_alias: Option<&str>,
        recovery: Option<&serde_json::Value>,
        notification: Option<(&str, &serde_json::Value)>,
        now: i64,
    ) -> Result<bool> {
        self.db.with_conn(|conn| {
            let tx = conn.unchecked_transaction().map_err(map_sqlite)?;
            let changed = tx.execute(
                "UPDATE runs SET state='terminal', terminal_kind=?2, terminal_reason=?3, updated_at=?4, finished_at=?4 WHERE id=?1 AND finished_at IS NULL",
                params![run_id, terminal.kind, terminal.reason, now],
            ).map_err(map_sqlite)?;
            if changed == 0 {
                return Ok(false);
            }
            crate::run_control::cancel_pending_approvals(&tx, run_id, now)?;
            if let Some(recovery) = recovery {
                let has_marker: bool = tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM run_events WHERE run_id=?1 AND event_type='run_recovered_after_startup')",
                    [run_id], |row| row.get(0),
                ).map_err(map_sqlite)?;
                if !has_marker {
                    insert_run_event(&tx, run_id, now, "finalizing", "run_recovered_after_startup", recovery)?;
                }
            }
            let mut events = Vec::new();
            if let Some(alias) = cancel_alias {
                events.push(("control", "cancel.requested", serde_json::json!({"alias": alias})));
                events.push(("control", "cancel.propagated", serde_json::json!({"alias": alias, "target": "active_run"})));
            }
            if let Some((event_type, data)) = notification {
                events.push(("finalizing", event_type, data.clone()));
            }
            events.push(("terminal", "run_terminal", terminal.data.clone()));
            for (phase, event_type, data) in events {
                insert_run_event(&tx, run_id, now, phase, event_type, &data)?;
            }
            tx.commit().map_err(map_sqlite)?;
            Ok(true)
        })
    }

    /// Repair only the old startup-recovery shape (finished row + recovery
    /// marker, no canonical event). Preserve the committed kind/reason/time.
    pub fn repair_legacy_recovered_terminal(&self, run_id: &str) -> Result<()> {
        self.db.with_conn(|conn| {
            let tx = conn.unchecked_transaction().map_err(map_sqlite)?;
            let legacy: Option<(String, Option<String>, i64)> = tx.query_row(
                "SELECT terminal_kind, terminal_reason, finished_at FROM runs r
                 WHERE id=?1 AND finished_at IS NOT NULL AND terminal_kind IS NOT NULL
                 AND EXISTS(SELECT 1 FROM run_events WHERE run_id=r.id AND event_type='run_recovered_after_startup')
                 AND NOT EXISTS(SELECT 1 FROM run_events WHERE run_id=r.id AND event_type='run_terminal')",
                [run_id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            ).optional().map_err(map_sqlite)?;
            if let Some((kind, reason, now)) = legacy {
                insert_run_event(&tx, run_id, now, "terminal", "run_terminal",
                    &serde_json::json!({"kind":kind, "message":null, "reason":reason}))?;
            }
            tx.commit().map_err(map_sqlite)
        })
    }

    /// Called only after the authoritative session file contains the outcome.
    /// Retrying after an acknowledgement failure does not duplicate the ack.
    pub(crate) fn confirm_terminal_projection(
        &self,
        run_id: &str,
        event: &deepagent_core::event::Event,
        now: i64,
    ) -> Result<()> {
        self.db.with_conn(|conn| {
            let tx = conn.unchecked_transaction().map_err(map_sqlite)?;
            let confirmed: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM run_events WHERE run_id=?1 AND event_type='session_terminal_projected')",
                [run_id], |row| row.get(0),
            ).map_err(map_sqlite)?;
            if !confirmed {
                insert_run_event(&tx, run_id, now, "terminal", "session_terminal_projected",
                    &serde_json::json!({"version":1, "session_id":event.session_id.to_string(),
                        "session_sequence":event.sequence, "session_event_id":event.id.to_string()}))?;
            }
            tx.commit().map_err(map_sqlite)
        })
    }

    pub fn pending_terminal_projections(&self, session_id: &str) -> Result<Vec<String>> {
        self.db.with_conn(|conn| {
            let sql = format!("SELECT r.id FROM runs r WHERE r.session_id=?1 AND ({PENDING_TERMINAL_PROJECTION}) ORDER BY r.created_at, r.id");
            let mut statement = conn.prepare(&sql).map_err(map_sqlite)?;
            let rows = statement.query_map([session_id], |row| row.get(0)).map_err(map_sqlite)?;
            rows.collect::<rusqlite::Result<Vec<_>>>().map_err(map_sqlite)
        })
    }

    /// Read only the terminal/outbox records, not every persisted model delta.
    pub(crate) fn terminal_to_project(&self, run_id: &str) -> Result<Option<serde_json::Value>> {
        self.db.with_conn(|conn| {
            let confirmed: bool = conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM run_events WHERE run_id=?1 AND event_type='session_terminal_projected')",
                [run_id], |row| row.get(0),
            ).map_err(map_sqlite)?;
            if confirmed { return Ok(None); }
            let mut statement = conn.prepare(
                "SELECT data FROM run_events WHERE run_id=?1 AND event_type='run_terminal' ORDER BY sequence"
            ).map_err(map_sqlite)?;
            let rows = statement.query_map([run_id], |row| row.get::<_, String>(0)).map_err(map_sqlite)?;
            let terminals = rows.collect::<rusqlite::Result<Vec<_>>>().map_err(map_sqlite)?;
            match terminals.as_slice() {
                [terminal] => Ok(Some(serde_json::from_str(terminal).map_err(|error| CoreError::Persistence(format!("run {run_id} invalid terminal: {error}")))?)),
                _ => Err(CoreError::Persistence(format!("run {run_id} requires exactly one canonical terminal, found {}", terminals.len()))),
            }
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
            let tx = conn.unchecked_transaction().map_err(map_sqlite)?;
            let changed = tx
                .execute(
                    "UPDATE runs SET state='terminal', terminal_kind=?2, terminal_reason=?3, updated_at=?4, finished_at=?4 WHERE id=?1 AND finished_at IS NULL",
                    params![run_id, terminal_kind, reason, now],
                )
                .map_err(map_sqlite)?;
            if changed == 1 {
                crate::run_control::cancel_pending_approvals(&tx, run_id, now)?;
            }
            tx.commit().map_err(map_sqlite)?;
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
        self.get_with_budget(run_id, None)
    }

    pub fn get_bounded(&self, run_id: &str, max_bytes: usize) -> Result<Option<RunRecord>> {
        self.get_with_budget(run_id, Some(max_bytes))
    }

    fn get_with_budget(&self, run_id: &str, max_bytes: Option<usize>) -> Result<Option<RunRecord>> {
        bounded_read::one(self.db,
                "SELECT id, session_id, task_id, state, terminal_kind, terminal_reason, created_at, updated_at, finished_at FROM runs WHERE id=?1",
                [run_id],
                max_bytes,|row| run_from_row(row).map_err(map_sqlite))
    }

    /// Read the run-owned capability projection and latest model/wire schema
    /// generations without reconstructing them from the current host.
    pub fn capability_snapshot(&self, run_id: &str) -> Result<Option<serde_json::Value>> {
        self.db.with_conn(|conn| {
            let session_id: Option<String> = conn
                .query_row("SELECT session_id FROM runs WHERE id=?1", [run_id], |row| {
                    row.get(0)
                })
                .optional()
                .map_err(map_sqlite)?;
            let Some(session_id) = session_id else {
                return Ok(None);
            };
            let mut statement = conn
                .prepare(
                    "SELECT event_type, data FROM run_events
                     WHERE run_id=?1 AND event_type IN
                       ('capability.snapshot','capability_schema_snapshot','capability_wire_snapshot')
                     ORDER BY sequence",
                )
                .map_err(map_sqlite)?;
            let rows = statement
                .query_map([run_id], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })
                .map_err(map_sqlite)?;
            let events = rows
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(map_sqlite)?;
            let mut snapshot = None;
            let mut schema_generation = None;
            let mut wire_generation = None;
            for (event_type, data) in events {
                let data: serde_json::Value = serde_json::from_str(&data)?;
                match event_type.as_str() {
                    "capability.snapshot" => {
                        if snapshot.replace(data).is_some() {
                            return Err(CoreError::invalid(
                                "run has duplicate capability snapshots",
                            ));
                        }
                    }
                    "capability_schema_snapshot" => schema_generation = Some(data),
                    "capability_wire_snapshot" => wire_generation = Some(data),
                    _ => unreachable!("query only selects capability event types"),
                }
            }
            let Some(mut snapshot) = snapshot else {
                return Ok(None);
            };
            let legacy = snapshot.as_object().is_some_and(|fields| {
                fields.len() == 2
                    && fields.contains_key("toolCount")
                    && fields.contains_key("toolSchemaHash")
            });
            if !legacy {
                validate_capability_identity(&snapshot, run_id, &session_id)?;
                let snapshot_id = snapshot
                    .get("snapshotId")
                    .and_then(serde_json::Value::as_str)
                    .map(ToOwned::to_owned)
                    .ok_or_else(|| CoreError::invalid("capability snapshot has no identity"))?;
                if let Some(data) = schema_generation {
                    validate_capability_generation(&data, &snapshot_id, "modelSchema")?;
                    snapshot["modelSchema"] = data["modelSchema"].clone();
                }
                if let Some(data) = wire_generation {
                    validate_capability_generation(&data, &snapshot_id, "wireSchema")?;
                    snapshot["wireSchema"] = data["wireSchema"].clone();
                }
            }
            Ok(Some(snapshot))
        })
    }

    /// Includes controls left behind after their parent became terminal. A
    /// snapshot is only a candidate list, never authorization to recover.
    pub fn recovery_sessions(&self, now: i64) -> Result<Vec<String>> {
        self.db.with_conn(|conn| {
            let sql = format!(
                "SELECT DISTINCT r.session_id FROM runs r
                 WHERE r.finished_at IS NULL
                    OR ({PENDING_TERMINAL_PROJECTION})
                    OR EXISTS (SELECT 1 FROM run_actions a WHERE a.run_id=r.id AND a.state='running')
                    OR EXISTS (SELECT 1 FROM run_approvals a WHERE a.run_id=r.id AND a.state='pending' AND a.expires_at<=?1)
                    OR EXISTS (SELECT 1 FROM subagent_runs c WHERE c.parent_run_id=r.id AND c.state='running' AND c.finished_at IS NULL)
                 ORDER BY r.session_id");
            let mut statement = conn.prepare(&sql).map_err(map_sqlite)?;
            let rows = statement.query_map([now], |row| row.get(0)).map_err(map_sqlite)?;
            rows.collect::<rusqlite::Result<Vec<_>>>().map_err(map_sqlite)
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
                .query_map([session_id], run_from_row)
                .map_err(map_sqlite)?;
            rows.collect::<std::result::Result<Vec<_>, _>>()
                .map_err(map_sqlite)
        })
    }

    /// Bounded membership-stable projection; the existing ledger remains the
    /// replay source. Equal timestamps have a deterministic identity tie-break.
    pub fn list_for_session_page(
        &self,
        session_id: &str,
        cursor: Option<&HistoryCursor>,
        budget: HistoryReadBudget,
    ) -> Result<HistoryPage<RunRecord>> {
        bounded_read::read(self.db,&HistoryQuery {
            source:HistorySource::Runs,columns:"id,session_id,task_id,state,terminal_kind,terminal_reason,created_at,updated_at,finished_at",
            table:"runs",scope_filter:"session_id=?1",number:"created_at",id:"id",descending:true,
        },session_id,cursor,budget,|row| run_from_row(row).map_err(map_sqlite))
    }

    pub fn events_page(
        &self,
        run_id: &str,
        after: Option<u64>,
        cursor: Option<&HistoryCursor>,
        budget: HistoryReadBudget,
    ) -> Result<HistoryPage<StoredRunEvent>> {
        bounded_read::read_after(
            self.db,
            &HistoryQuery {
                source: HistorySource::Events,
                columns: "run_id,sequence,timestamp,phase,status,event_type,data",
                table: "run_events",
                scope_filter: "run_id=?1",
                number: "sequence",
                id: "''",
                descending: false,
            },
            run_id,
            after.map(|value| value.min(i64::MAX as u64) as i64),
            cursor,
            budget,
            |row| run_event_from_row(row).map_err(map_sqlite),
        )
    }

    pub fn events_after(&self, run_id: &str, after: Option<u64>) -> Result<Vec<StoredRunEvent>> {
        self.db.with_conn(|conn| {
            let mut stmt = conn
                .prepare("SELECT run_id, sequence, timestamp, phase, status, event_type, data FROM run_events WHERE run_id=?1 AND sequence>?2 ORDER BY sequence")
                .map_err(map_sqlite)?;
            let rows = stmt
                .query_map(params![run_id, after.map(|v| v as i64).unwrap_or(-1)], run_event_from_row)
                .map_err(map_sqlite)?;
            rows.collect::<std::result::Result<Vec<_>, _>>()
                .map_err(map_sqlite)
        })
    }
}

fn run_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RunRecord> {
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
}

fn run_event_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredRunEvent> {
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
}

#[cfg(test)]
#[path = "run_store/terminal_notification_tests.rs"]
mod terminal_notification_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_and_cancel_audit_commit_once_and_replay_in_order() {
        let db = Database::open_in_memory().unwrap();
        db.with_conn(|conn| {
            conn.execute(
                "INSERT INTO sessions (id, created_at, updated_at) VALUES ('s', 1, 1)",
                [],
            )
            .unwrap();
            Ok(())
        })
        .unwrap();
        let store = RunStore::new(&db);
        store.create("r", "s", None, 1).unwrap();
        store.bind_task("r", "s", "t").unwrap();
        assert!(store.bind_task("r", "other", "t").is_err());
        assert!(store.bind_task("r", "s", "other").is_err());
        let terminal = serde_json::json!({"kind": "cancelled"});
        assert!(store
            .finish_with_event("r", "cancelled", None, &terminal, Some("r"), 2)
            .unwrap());
        assert!(!store
            .finish_with_event("r", "succeeded", None, &terminal, Some("r"), 3)
            .unwrap());
        let events = store.events_after("r", None).unwrap();
        assert_eq!(
            events
                .iter()
                .map(|e| e.event_type.as_str())
                .collect::<Vec<_>>(),
            [
                "run_accepted",
                "cancel.requested",
                "cancel.propagated",
                "run_terminal"
            ]
        );
        assert_eq!(
            store.events_after("r", Some(events[0].sequence)).unwrap(),
            events[1..]
        );
    }

    #[test]
    fn failed_terminal_event_rolls_back_state() {
        let db = Database::open_in_memory().unwrap();
        db.with_conn(|conn| {
            conn.execute_batch("INSERT INTO sessions (id, created_at, updated_at) VALUES ('s', 1, 1);
                CREATE TRIGGER fail_terminal BEFORE INSERT ON run_events WHEN NEW.event_type='run_terminal' BEGIN SELECT RAISE(ABORT, 'injected disk failure'); END;").unwrap();
            Ok(())
        }).unwrap();
        let store = RunStore::new(&db);
        store.create("r", "s", None, 1).unwrap();
        assert!(store
            .finish_with_event("r", "cancelled", None, &serde_json::json!({}), Some("r"), 2)
            .is_err());
        assert!(store.get("r").unwrap().unwrap().finished_at.is_none());
        let events = store.events_after("r", None).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, "run_accepted");
    }

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
            1
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
            2
        );
        assert!(store.finish("r1", "succeeded", None, 4).unwrap());
        assert!(!store.finish("r1", "failed", Some("late"), 5).unwrap());
        let record = store.get("r1").unwrap().unwrap();
        assert_eq!(record.terminal_kind.as_deref(), Some("succeeded"));
        assert_eq!(store.events_after("r1", None).unwrap().len(), 3);
        assert_eq!(store.list_for_session("s1").unwrap().len(), 1);
    }
}
