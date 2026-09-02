//! Durable control-plane projections for a persisted run.
//!
//! `runs` and `run_events` remain the lifecycle projection and ordered ledger.
//! This module adds queryable action, approval and lease records and always
//! appends their state-change event in the same SQLite transaction.

use deepagent_core::error::{CoreError, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::{map_sqlite, Database};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunActionState {
    Received,
    Prepared,
    Queued,
    Blocked,
    Running,
    Completed,
    Failed,
    Cancelled,
    Expired,
    Denied,
}

impl RunActionState {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Received => "received",
            Self::Prepared => "prepared",
            Self::Queued => "queued",
            Self::Blocked => "blocked",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Expired => "expired",
            Self::Denied => "denied",
        }
    }

    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Failed | Self::Cancelled | Self::Expired | Self::Denied
        )
    }

    fn parse(value: &str) -> Result<Self> {
        match value {
            "received" => Ok(Self::Received),
            "prepared" => Ok(Self::Prepared),
            "queued" => Ok(Self::Queued),
            "blocked" => Ok(Self::Blocked),
            "running" => Ok(Self::Running),
            "completed" => Ok(Self::Completed),
            "failed" => Ok(Self::Failed),
            "cancelled" => Ok(Self::Cancelled),
            "expired" => Ok(Self::Expired),
            "denied" => Ok(Self::Denied),
            other => Err(CoreError::EventLog(format!(
                "unknown persisted run action state '{other}'"
            ))),
        }
    }

    fn can_transition_to(self, next: Self) -> bool {
        use RunActionState::*;
        match self {
            Received => matches!(next, Prepared | Failed | Cancelled),
            Prepared => matches!(next, Queued | Blocked | Running | Failed | Cancelled),
            Queued => matches!(next, Blocked | Running | Failed | Cancelled),
            Blocked => matches!(
                next,
                Queued | Running | Denied | Expired | Cancelled | Failed
            ),
            Running => matches!(next, Completed | Failed | Cancelled),
            Completed | Failed | Cancelled | Expired | Denied => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunActionRecord {
    pub run_id: String,
    pub turn_id: String,
    pub call_id: String,
    pub sequence: u64,
    pub tool_name: String,
    pub arguments_hash: String,
    pub state: RunActionState,
    pub risk: String,
    pub approval_id: Option<String>,
    pub attempt: u32,
    pub lease_owner: Option<String>,
    pub lease_expires_at: Option<i64>,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
    pub result_ref: Option<String>,
    pub blocked_reason: Option<String>,
    pub parent_action_id: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone)]
pub struct NewRunAction<'a> {
    pub run_id: &'a str,
    pub turn_id: &'a str,
    pub call_id: &'a str,
    pub sequence: u64,
    pub tool_name: &'a str,
    pub arguments_hash: &'a str,
    pub risk: &'a str,
    pub parent_action_id: Option<&'a str>,
    pub now: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlMutation {
    Applied,
    Unchanged,
}

enum ApprovalResponseOutcome {
    Mutation(ControlMutation),
    Expired { requested: ApprovalState },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalState {
    Pending,
    Approved,
    Denied,
    Expired,
    Cancelled,
}

impl ApprovalState {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Approved => "approved",
            Self::Denied => "denied",
            Self::Expired => "expired",
            Self::Cancelled => "cancelled",
        }
    }

    fn parse(value: &str) -> Result<Self> {
        match value {
            "pending" => Ok(Self::Pending),
            "approved" => Ok(Self::Approved),
            "denied" => Ok(Self::Denied),
            "expired" => Ok(Self::Expired),
            "cancelled" => Ok(Self::Cancelled),
            other => Err(CoreError::EventLog(format!(
                "unknown persisted approval state '{other}'"
            ))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunApprovalRecord {
    pub approval_id: String,
    pub run_id: String,
    pub call_id: String,
    pub state: ApprovalState,
    pub scope: String,
    pub risk: String,
    pub reason: Option<String>,
    pub policy_snapshot: Option<String>,
    pub expires_at: Option<i64>,
    pub decided_at: Option<i64>,
    pub decided_by: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone)]
pub struct NewRunApproval<'a> {
    pub approval_id: &'a str,
    pub run_id: &'a str,
    pub call_id: &'a str,
    pub scope: &'a str,
    pub risk: &'a str,
    pub reason: Option<&'a str>,
    pub policy_snapshot: Option<&'a str>,
    pub expires_at: Option<i64>,
    pub now: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionLeaseRecord {
    pub lease_id: String,
    pub resource_kind: String,
    pub resource_id: String,
    pub owner: String,
    pub epoch: u64,
    pub fencing_token_hash: String,
    pub acquired_at: i64,
    pub expires_at: i64,
    pub renewed_at: Option<i64>,
    pub revoked_at: Option<i64>,
    pub revoke_reason: Option<String>,
}

#[derive(Debug, Clone)]
pub struct NewExecutionLease<'a> {
    pub lease_id: &'a str,
    pub resource_kind: &'a str,
    pub resource_id: &'a str,
    pub owner: &'a str,
    pub fencing_token_hash: &'a str,
    pub expires_at: i64,
    pub now: i64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunControlRecovery {
    pub failed_running_actions: u64,
    pub expired_approvals: u64,
    pub revoked_leases: u64,
}

pub struct RunControlStore<'db> {
    db: &'db Database,
}

impl<'db> RunControlStore<'db> {
    pub const fn new(db: &'db Database) -> Self {
        Self { db }
    }

    pub fn next_action_sequence(&self, run_id: &str) -> Result<u64> {
        self.db.with_conn(|connection| {
            let next: i64 = connection
                .query_row(
                    "SELECT COALESCE(MAX(sequence),-1)+1 FROM run_actions WHERE run_id=?1",
                    [run_id],
                    |row| row.get(0),
                )
                .map_err(map_sqlite)?;
            to_u64(next, "action sequence")
        })
    }

    /// Append a control-plane signal to the canonical run event ledger.
    /// Transports use this for durable interrupt/continuation intent; the
    /// kernel remains responsible for observed cancellation and terminal state.
    pub fn append_control_signal(
        &self,
        run_id: &str,
        event_type: &str,
        data: &serde_json::Value,
    ) -> Result<u64> {
        crate::run_store::RunStore::new(self.db).append_event(
            run_id,
            now_millis(),
            "accepted",
            "progress",
            event_type,
            data,
        )
    }

    pub fn create_action(&self, action: &NewRunAction<'_>) -> Result<ControlMutation> {
        self.db.with_conn(|connection| {
            in_immediate_transaction(connection, || {
                let changed = connection
                    .execute(
                        "INSERT INTO run_actions (run_id,turn_id,call_id,sequence,tool_name,arguments_hash,state,risk,parent_action_id,created_at,updated_at) VALUES (?1,?2,?3,?4,?5,?6,'received',?7,?8,?9,?9) ON CONFLICT(run_id,call_id) DO NOTHING",
                        params![
                            action.run_id,
                            action.turn_id,
                            action.call_id,
                            to_i64(action.sequence)?,
                            action.tool_name,
                            action.arguments_hash,
                            action.risk,
                            action.parent_action_id,
                            action.now,
                        ],
                    )
                    .map_err(map_sqlite)?;
                if changed == 0 {
                    let existing = get_action_from(connection, action.run_id, action.call_id)?
                        .ok_or_else(|| CoreError::not_found("conflicting run action vanished"))?;
                    if existing.turn_id == action.turn_id
                        && existing.sequence == action.sequence
                        && existing.tool_name == action.tool_name
                        && existing.arguments_hash == action.arguments_hash
                    {
                        return Ok(ControlMutation::Unchanged);
                    }
                    return Err(CoreError::invalid(format!(
                        "idempotency conflict for action {}/{}",
                        action.run_id, action.call_id
                    )));
                }
                append_control_event(
                    connection,
                    action.run_id,
                    action.now,
                    "action.received",
                    &serde_json::json!({
                        "turnId": action.turn_id,
                        "callId": action.call_id,
                        "sequence": action.sequence,
                        "tool": action.tool_name,
                        "risk": action.risk,
                    }),
                )?;
                Ok(ControlMutation::Applied)
            })
        })
    }

    pub fn transition_action(
        &self,
        run_id: &str,
        call_id: &str,
        next: RunActionState,
        now: i64,
        blocked_reason: Option<&str>,
        result_ref: Option<&str>,
    ) -> Result<ControlMutation> {
        self.db.with_conn(|connection| {
            in_immediate_transaction(connection, || {
                let current = get_action_from(connection, run_id, call_id)?
                    .ok_or_else(|| CoreError::not_found(format!("run action {run_id}/{call_id}")))?;
                if current.state == next {
                    return Ok(ControlMutation::Unchanged);
                }
                if !current.state.can_transition_to(next) {
                    return Err(CoreError::IllegalTransition {
                        from: current.state.label().to_string(),
                        to: next.label().to_string(),
                    });
                }
                let started_at = (next == RunActionState::Running).then_some(now);
                let finished_at = next.is_terminal().then_some(now);
                let attempt_increment = i64::from(next == RunActionState::Running);
                connection
                    .execute(
                        "UPDATE run_actions SET state=?3, attempt=attempt+?4, started_at=COALESCE(started_at,?5), finished_at=?6, blocked_reason=?7, result_ref=?8, updated_at=?9 WHERE run_id=?1 AND call_id=?2 AND state=?10",
                        params![
                            run_id,
                            call_id,
                            next.label(),
                            attempt_increment,
                            started_at,
                            finished_at,
                            blocked_reason,
                            result_ref,
                            now,
                            current.state.label(),
                        ],
                    )
                    .map_err(map_sqlite)?;
                append_control_event(
                    connection,
                    run_id,
                    now,
                    &format!("action.{}", next.label()),
                    &serde_json::json!({
                        "callId": call_id,
                        "from": current.state.label(),
                        "state": next.label(),
                        "blockedReason": blocked_reason,
                        "resultRef": result_ref,
                    }),
                )?;
                Ok(ControlMutation::Applied)
            })
        })
    }

    pub fn get_action(&self, run_id: &str, call_id: &str) -> Result<Option<RunActionRecord>> {
        self.db
            .with_conn(|connection| get_action_from(connection, run_id, call_id))
    }

    pub fn list_actions(&self, run_id: &str) -> Result<Vec<RunActionRecord>> {
        self.db.with_conn(|connection| {
            let mut statement = connection
                .prepare(ACTION_SELECT_WITH_FILTER)
                .map_err(map_sqlite)?;
            let rows = statement
                .query_map([run_id], action_from_row)
                .map_err(map_sqlite)?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(map_sqlite)?
                .into_iter()
                .map(parse_action)
                .collect()
        })
    }

    pub fn request_approval(&self, approval: &NewRunApproval<'_>) -> Result<ControlMutation> {
        self.db.with_conn(|connection| {
            in_immediate_transaction(connection, || {
                let changed = connection
                    .execute(
                        "INSERT INTO run_approvals (approval_id,run_id,call_id,state,scope,risk,reason,policy_snapshot,expires_at,created_at,updated_at) VALUES (?1,?2,?3,'pending',?4,?5,?6,?7,?8,?9,?9) ON CONFLICT(approval_id) DO NOTHING",
                        params![
                            approval.approval_id,
                            approval.run_id,
                            approval.call_id,
                            approval.scope,
                            approval.risk,
                            approval.reason,
                            approval.policy_snapshot,
                            approval.expires_at,
                            approval.now,
                        ],
                    )
                    .map_err(map_sqlite)?;
                if changed == 0 {
                    let existing = get_approval_from(connection, approval.approval_id)?
                        .ok_or_else(|| CoreError::not_found("conflicting approval vanished"))?;
                    if existing.run_id == approval.run_id
                        && existing.call_id == approval.call_id
                        && existing.scope == approval.scope
                        && existing.risk == approval.risk
                    {
                        return Ok(ControlMutation::Unchanged);
                    }
                    return Err(CoreError::invalid(format!(
                        "idempotency conflict for approval {}",
                        approval.approval_id
                    )));
                }
                connection
                    .execute(
                        "UPDATE run_actions SET approval_id=?3, state='blocked', blocked_reason=?4, updated_at=?5 WHERE run_id=?1 AND call_id=?2 AND state IN ('prepared','queued','blocked')",
                        params![
                            approval.run_id,
                            approval.call_id,
                            approval.approval_id,
                            approval.reason,
                            approval.now,
                        ],
                    )
                    .map_err(map_sqlite)?;
                append_control_event(
                    connection,
                    approval.run_id,
                    approval.now,
                    "approval.requested",
                    &serde_json::json!({
                        "approvalId": approval.approval_id,
                        "callId": approval.call_id,
                        "scope": approval.scope,
                        "risk": approval.risk,
                        "expiresAt": approval.expires_at,
                    }),
                )?;
                Ok(ControlMutation::Applied)
            })
        })
    }

    pub fn respond_approval(
        &self,
        approval_id: &str,
        approved: bool,
        decided_by: &str,
        now: i64,
    ) -> Result<ControlMutation> {
        let outcome = self.db.with_conn(|connection| {
            in_immediate_transaction(connection, || {
                let current = get_approval_from(connection, approval_id)?
                    .ok_or_else(|| CoreError::not_found(format!("approval {approval_id}")))?;
                let next = if approved {
                    ApprovalState::Approved
                } else {
                    ApprovalState::Denied
                };
                if current.state == ApprovalState::Pending
                    && current.expires_at.is_some_and(|expiry| expiry <= now)
                {
                    expire_approval(connection, &current, now)?;
                    return Ok(ApprovalResponseOutcome::Expired {
                        requested: next,
                    });
                }
                if current.state == next {
                    return Ok(ApprovalResponseOutcome::Mutation(
                        ControlMutation::Unchanged,
                    ));
                }
                if current.state != ApprovalState::Pending {
                    return Err(CoreError::IllegalTransition {
                        from: current.state.label().to_string(),
                        to: next.label().to_string(),
                    });
                }
                connection
                    .execute(
                        "UPDATE run_approvals SET state=?2,decided_at=?3,decided_by=?4,updated_at=?3 WHERE approval_id=?1 AND state='pending'",
                        params![approval_id, next.label(), now, decided_by],
                    )
                    .map_err(map_sqlite)?;
                let action_state = if approved { "queued" } else { "denied" };
                connection
                    .execute(
                        "UPDATE run_actions SET state=?3, finished_at=CASE WHEN ?3='denied' THEN ?4 ELSE finished_at END, updated_at=?4 WHERE run_id=?1 AND call_id=?2 AND state='blocked'",
                        params![current.run_id, current.call_id, action_state, now],
                    )
                    .map_err(map_sqlite)?;
                append_control_event(
                    connection,
                    &current.run_id,
                    now,
                    "approval.responded",
                    &serde_json::json!({
                        "approvalId": approval_id,
                        "callId": current.call_id,
                        "state": next.label(),
                        "decidedBy": decided_by,
                    }),
                )?;
                Ok(ApprovalResponseOutcome::Mutation(ControlMutation::Applied))
            })
        })?;
        match outcome {
            ApprovalResponseOutcome::Mutation(mutation) => Ok(mutation),
            ApprovalResponseOutcome::Expired { requested } => Err(CoreError::IllegalTransition {
                from: ApprovalState::Expired.label().to_string(),
                to: requested.label().to_string(),
            }),
        }
    }

    pub fn get_approval(&self, approval_id: &str) -> Result<Option<RunApprovalRecord>> {
        self.db
            .with_conn(|connection| get_approval_from(connection, approval_id))
    }

    pub fn list_approvals(&self, run_id: &str) -> Result<Vec<RunApprovalRecord>> {
        self.db.with_conn(|connection| {
            let mut statement = connection
                .prepare("SELECT approval_id,run_id,call_id,state,scope,risk,reason,policy_snapshot,expires_at,decided_at,decided_by,created_at,updated_at FROM run_approvals WHERE run_id=?1 ORDER BY created_at, approval_id")
                .map_err(map_sqlite)?;
            let rows = statement.query_map([run_id], approval_row).map_err(map_sqlite)?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(map_sqlite)?
                .into_iter()
                .map(parse_approval)
                .collect()
        })
    }

    pub fn acquire_lease(&self, lease: &NewExecutionLease<'_>) -> Result<ExecutionLeaseRecord> {
        if lease.expires_at <= lease.now {
            return Err(CoreError::invalid("lease expiry must be in the future"));
        }
        self.db.with_conn(|connection| {
            in_immediate_transaction(connection, || {
                if let Some(active) = active_lease_from(
                    connection,
                    lease.resource_kind,
                    lease.resource_id,
                )? {
                    if active.expires_at > lease.now {
                        if active.owner == lease.owner
                            && active.fencing_token_hash == lease.fencing_token_hash
                        {
                            return Ok(active);
                        }
                        return Err(CoreError::IllegalTransition {
                            from: format!("leased_by:{}@{}", active.owner, active.epoch),
                            to: format!("leased_by:{}", lease.owner),
                        });
                    }
                    connection
                        .execute(
                            "UPDATE execution_leases SET revoked_at=?2,revoke_reason='expired' WHERE lease_id=?1 AND revoked_at IS NULL",
                            params![active.lease_id, lease.now],
                        )
                        .map_err(map_sqlite)?;
                }
                let epoch: i64 = connection
                    .query_row(
                        "SELECT COALESCE(MAX(epoch),0)+1 FROM execution_leases WHERE resource_kind=?1 AND resource_id=?2",
                        params![lease.resource_kind, lease.resource_id],
                        |row| row.get(0),
                    )
                    .map_err(map_sqlite)?;
                connection
                    .execute(
                        "INSERT INTO execution_leases (lease_id,resource_kind,resource_id,owner,epoch,fencing_token_hash,acquired_at,expires_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
                        params![
                            lease.lease_id,
                            lease.resource_kind,
                            lease.resource_id,
                            lease.owner,
                            epoch,
                            lease.fencing_token_hash,
                            lease.now,
                            lease.expires_at,
                        ],
                    )
                    .map_err(map_sqlite)?;
                lease_from(connection, lease.lease_id)?.ok_or_else(|| {
                    CoreError::Persistence("inserted execution lease vanished".to_string())
                })
            })
        })
    }

    pub fn renew_lease(
        &self,
        lease_id: &str,
        owner: &str,
        epoch: u64,
        new_expiry: i64,
        now: i64,
    ) -> Result<ControlMutation> {
        if new_expiry <= now {
            return Err(CoreError::invalid("lease expiry must be in the future"));
        }
        self.db.with_conn(|connection| {
            let changed = connection
                .execute(
                    "UPDATE execution_leases SET expires_at=?4,renewed_at=?5 WHERE lease_id=?1 AND owner=?2 AND epoch=?3 AND revoked_at IS NULL AND expires_at>?5",
                    params![lease_id, owner, to_i64(epoch)?, new_expiry, now],
                )
                .map_err(map_sqlite)?;
            if changed == 1 {
                Ok(ControlMutation::Applied)
            } else {
                Err(CoreError::IllegalTransition {
                    from: "stale_or_missing_lease".to_string(),
                    to: "renewed".to_string(),
                })
            }
        })
    }

    pub fn validate_fence(
        &self,
        resource_kind: &str,
        resource_id: &str,
        owner: &str,
        epoch: u64,
        now: i64,
    ) -> Result<bool> {
        self.db.with_conn(|connection| {
            let valid: i64 = connection
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM execution_leases WHERE resource_kind=?1 AND resource_id=?2 AND owner=?3 AND epoch=?4 AND revoked_at IS NULL AND expires_at>?5)",
                    params![resource_kind, resource_id, owner, to_i64(epoch)?, now],
                    |row| row.get(0),
                )
                .map_err(map_sqlite)?;
            Ok(valid == 1)
        })
    }

    pub fn recover(&self, now: i64) -> Result<RunControlRecovery> {
        self.db.with_conn(|connection| {
            in_immediate_transaction(connection, || {
                let running = collect_pairs(
                    connection,
                    "SELECT run_id,call_id FROM run_actions WHERE state='running'",
                )?;
                for (run_id, call_id) in &running {
                    connection
                        .execute(
                            "UPDATE run_actions SET state='failed',finished_at=?3,blocked_reason='process_restarted_while_running',updated_at=?3 WHERE run_id=?1 AND call_id=?2 AND state='running'",
                            params![run_id, call_id, now],
                        )
                        .map_err(map_sqlite)?;
                    append_control_event(
                        connection,
                        run_id,
                        now,
                        "action.failed",
                        &serde_json::json!({
                            "callId": call_id,
                            "reason": "process_restarted_while_running",
                            "replayed": false,
                        }),
                    )?;
                }

                let expiring = collect_approval_ids(connection, now)?;
                for approval_id in &expiring {
                    if let Some(record) = get_approval_from(connection, approval_id)? {
                        expire_approval(connection, &record, now)?;
                    }
                }
                let revoked = connection
                    .execute(
                        "UPDATE execution_leases SET revoked_at=?1,revoke_reason='expired' WHERE revoked_at IS NULL AND expires_at<=?1",
                        [now],
                    )
                    .map_err(map_sqlite)?;
                Ok(RunControlRecovery {
                    failed_running_actions: running.len() as u64,
                    expired_approvals: expiring.len() as u64,
                    revoked_leases: revoked as u64,
                })
            })
        })
    }
}

const ACTION_SELECT: &str = "SELECT run_id,turn_id,call_id,sequence,tool_name,arguments_hash,state,risk,approval_id,attempt,lease_owner,lease_expires_at,started_at,finished_at,result_ref,blocked_reason,parent_action_id,created_at,updated_at FROM run_actions";
const ACTION_SELECT_WITH_FILTER: &str = "SELECT run_id,turn_id,call_id,sequence,tool_name,arguments_hash,state,risk,approval_id,attempt,lease_owner,lease_expires_at,started_at,finished_at,result_ref,blocked_reason,parent_action_id,created_at,updated_at FROM run_actions WHERE run_id=?1 ORDER BY sequence";

type RawAction = (
    String,
    String,
    String,
    i64,
    String,
    String,
    String,
    String,
    Option<String>,
    i64,
    Option<String>,
    Option<i64>,
    Option<i64>,
    Option<i64>,
    Option<String>,
    Option<String>,
    Option<String>,
    i64,
    i64,
);

fn action_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawAction> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
        row.get(7)?,
        row.get(8)?,
        row.get(9)?,
        row.get(10)?,
        row.get(11)?,
        row.get(12)?,
        row.get(13)?,
        row.get(14)?,
        row.get(15)?,
        row.get(16)?,
        row.get(17)?,
        row.get(18)?,
    ))
}

fn parse_action(raw: RawAction) -> Result<RunActionRecord> {
    Ok(RunActionRecord {
        run_id: raw.0,
        turn_id: raw.1,
        call_id: raw.2,
        sequence: to_u64(raw.3, "action sequence")?,
        tool_name: raw.4,
        arguments_hash: raw.5,
        state: RunActionState::parse(&raw.6)?,
        risk: raw.7,
        approval_id: raw.8,
        attempt: to_u32(raw.9, "action attempt")?,
        lease_owner: raw.10,
        lease_expires_at: raw.11,
        started_at: raw.12,
        finished_at: raw.13,
        result_ref: raw.14,
        blocked_reason: raw.15,
        parent_action_id: raw.16,
        created_at: raw.17,
        updated_at: raw.18,
    })
}

fn get_action_from(
    connection: &Connection,
    run_id: &str,
    call_id: &str,
) -> Result<Option<RunActionRecord>> {
    let sql = format!("{ACTION_SELECT} WHERE run_id=?1 AND call_id=?2");
    connection
        .query_row(&sql, params![run_id, call_id], action_from_row)
        .optional()
        .map_err(map_sqlite)?
        .map(parse_action)
        .transpose()
}

fn get_approval_from(
    connection: &Connection,
    approval_id: &str,
) -> Result<Option<RunApprovalRecord>> {
    let raw = connection
        .query_row(
            "SELECT approval_id,run_id,call_id,state,scope,risk,reason,policy_snapshot,expires_at,decided_at,decided_by,created_at,updated_at FROM run_approvals WHERE approval_id=?1",
            [approval_id],
            approval_row,
        )
        .optional()
        .map_err(map_sqlite)?;
    raw.map(parse_approval).transpose()
}

type RawApproval = (
    String,
    String,
    String,
    String,
    String,
    String,
    Option<String>,
    Option<String>,
    Option<i64>,
    Option<i64>,
    Option<String>,
    i64,
    i64,
);

fn approval_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawApproval> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
        row.get(7)?,
        row.get(8)?,
        row.get(9)?,
        row.get(10)?,
        row.get(11)?,
        row.get(12)?,
    ))
}

fn parse_approval(raw: RawApproval) -> Result<RunApprovalRecord> {
    Ok(RunApprovalRecord {
        approval_id: raw.0,
        run_id: raw.1,
        call_id: raw.2,
        state: ApprovalState::parse(&raw.3)?,
        scope: raw.4,
        risk: raw.5,
        reason: raw.6,
        policy_snapshot: raw.7,
        expires_at: raw.8,
        decided_at: raw.9,
        decided_by: raw.10,
        created_at: raw.11,
        updated_at: raw.12,
    })
}

fn lease_from(connection: &Connection, lease_id: &str) -> Result<Option<ExecutionLeaseRecord>> {
    lease_query(
        connection,
        "SELECT lease_id,resource_kind,resource_id,owner,epoch,fencing_token_hash,acquired_at,expires_at,renewed_at,revoked_at,revoke_reason FROM execution_leases WHERE lease_id=?1",
        [lease_id],
    )
}

fn active_lease_from(
    connection: &Connection,
    resource_kind: &str,
    resource_id: &str,
) -> Result<Option<ExecutionLeaseRecord>> {
    lease_query(
        connection,
        "SELECT lease_id,resource_kind,resource_id,owner,epoch,fencing_token_hash,acquired_at,expires_at,renewed_at,revoked_at,revoke_reason FROM execution_leases WHERE resource_kind=?1 AND resource_id=?2 AND revoked_at IS NULL",
        [resource_kind, resource_id],
    )
}

fn lease_query<const N: usize>(
    connection: &Connection,
    sql: &str,
    values: [&str; N],
) -> Result<Option<ExecutionLeaseRecord>> {
    type RawLease = (
        String,
        String,
        String,
        String,
        i64,
        String,
        i64,
        i64,
        Option<i64>,
        Option<i64>,
        Option<String>,
    );
    let mut statement = connection.prepare(sql).map_err(map_sqlite)?;
    let raw = statement
        .query_row(
            rusqlite::params_from_iter(values),
            |row| -> rusqlite::Result<RawLease> {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                    row.get(8)?,
                    row.get(9)?,
                    row.get(10)?,
                ))
            },
        )
        .optional()
        .map_err(map_sqlite)?;
    raw.map(|raw| {
        Ok(ExecutionLeaseRecord {
            lease_id: raw.0,
            resource_kind: raw.1,
            resource_id: raw.2,
            owner: raw.3,
            epoch: to_u64(raw.4, "lease epoch")?,
            fencing_token_hash: raw.5,
            acquired_at: raw.6,
            expires_at: raw.7,
            renewed_at: raw.8,
            revoked_at: raw.9,
            revoke_reason: raw.10,
        })
    })
    .transpose()
}

fn expire_approval(connection: &Connection, approval: &RunApprovalRecord, now: i64) -> Result<()> {
    connection
        .execute(
            "UPDATE run_approvals SET state='expired',updated_at=?2 WHERE approval_id=?1 AND state='pending'",
            params![approval.approval_id, now],
        )
        .map_err(map_sqlite)?;
    connection
        .execute(
            "UPDATE run_actions SET state='expired',finished_at=?3,updated_at=?3 WHERE run_id=?1 AND call_id=?2 AND state='blocked'",
            params![approval.run_id, approval.call_id, now],
        )
        .map_err(map_sqlite)?;
    append_control_event(
        connection,
        &approval.run_id,
        now,
        "approval.expired",
        &serde_json::json!({
            "approvalId": approval.approval_id,
            "callId": approval.call_id,
        }),
    )?;
    Ok(())
}

fn append_control_event(
    connection: &Connection,
    run_id: &str,
    timestamp: i64,
    event_type: &str,
    data: &serde_json::Value,
) -> Result<u64> {
    let next: i64 = connection
        .query_row(
            "SELECT COALESCE(MAX(sequence), -1) + 1 FROM run_events WHERE run_id=?1",
            [run_id],
            |row| row.get(0),
        )
        .map_err(map_sqlite)?;
    connection
        .execute(
            "INSERT INTO run_events (run_id,sequence,timestamp,phase,status,event_type,data) VALUES (?1,?2,?3,'control',?4,?5,?6)",
            params![
                run_id,
                next,
                timestamp,
                event_type.rsplit('.').next().unwrap_or("updated"),
                event_type,
                serde_json::to_string(data).map_err(CoreError::from)?,
            ],
        )
        .map_err(map_sqlite)?;
    to_u64(next, "run event sequence")
}

fn in_immediate_transaction<T>(
    connection: &Connection,
    operation: impl FnOnce() -> Result<T>,
) -> Result<T> {
    connection
        .execute_batch("BEGIN IMMEDIATE")
        .map_err(map_sqlite)?;
    match operation() {
        Ok(value) => {
            connection.execute_batch("COMMIT").map_err(map_sqlite)?;
            Ok(value)
        }
        Err(error) => {
            let _ = connection.execute_batch("ROLLBACK");
            Err(error)
        }
    }
}

fn collect_pairs(connection: &Connection, sql: &str) -> Result<Vec<(String, String)>> {
    let mut statement = connection.prepare(sql).map_err(map_sqlite)?;
    let rows = statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .map_err(map_sqlite)?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(map_sqlite)
}

fn collect_approval_ids(connection: &Connection, now: i64) -> Result<Vec<String>> {
    let mut statement = connection
        .prepare(
            "SELECT approval_id FROM run_approvals WHERE state='pending' AND expires_at IS NOT NULL AND expires_at<=?1",
        )
        .map_err(map_sqlite)?;
    let rows = statement
        .query_map([now], |row| row.get(0))
        .map_err(map_sqlite)?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(map_sqlite)
}

fn to_i64(value: u64) -> Result<i64> {
    i64::try_from(value).map_err(|_| CoreError::invalid("integer exceeds SQLite i64 range"))
}

fn to_u64(value: i64, label: &str) -> Result<u64> {
    u64::try_from(value).map_err(|_| CoreError::EventLog(format!("negative {label}")))
}

fn to_u32(value: i64, label: &str) -> Result<u32> {
    u32::try_from(value).map_err(|_| CoreError::EventLog(format!("invalid {label}")))
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::run_store::RunStore;

    fn database_with_run() -> Database {
        let db = Database::open_in_memory().unwrap();
        db.with_conn(|connection| {
            connection
                .execute(
                    "INSERT INTO sessions (id,created_at,updated_at) VALUES ('session-1',1,1)",
                    [],
                )
                .map_err(map_sqlite)?;
            Ok(())
        })
        .unwrap();
        RunStore::new(&db)
            .create("run-1", "session-1", Some("turn-1"), 1)
            .unwrap();
        db
    }

    fn action<'a>(call_id: &'a str, sequence: u64) -> NewRunAction<'a> {
        NewRunAction {
            run_id: "run-1",
            turn_id: "turn-1",
            call_id,
            sequence,
            tool_name: "bash",
            arguments_hash: "sha256:arguments",
            risk: "high",
            parent_action_id: None,
            now: 2,
        }
    }

    #[test]
    fn action_transition_and_event_commit_together() {
        let db = database_with_run();
        let store = RunControlStore::new(&db);
        assert_eq!(
            store.create_action(&action("call-1", 0)).unwrap(),
            ControlMutation::Applied
        );
        assert_eq!(
            store.create_action(&action("call-1", 0)).unwrap(),
            ControlMutation::Unchanged
        );
        store
            .transition_action("run-1", "call-1", RunActionState::Prepared, 3, None, None)
            .unwrap();
        store
            .transition_action("run-1", "call-1", RunActionState::Running, 4, None, None)
            .unwrap();
        store
            .transition_action(
                "run-1",
                "call-1",
                RunActionState::Completed,
                5,
                None,
                Some("document:result-1"),
            )
            .unwrap();
        let record = store.get_action("run-1", "call-1").unwrap().unwrap();
        assert_eq!(record.state, RunActionState::Completed);
        assert_eq!(record.attempt, 1);
        assert_eq!(record.result_ref.as_deref(), Some("document:result-1"));
        assert_eq!(
            RunStore::new(&db)
                .events_after("run-1", None)
                .unwrap()
                .len(),
            4
        );
        assert!(store
            .transition_action("run-1", "call-1", RunActionState::Running, 6, None, None)
            .is_err());
    }

    #[test]
    fn action_sequence_conflict_rolls_back_projection_and_event() {
        let db = database_with_run();
        let store = RunControlStore::new(&db);
        store.create_action(&action("call-1", 0)).unwrap();

        let error = store.create_action(&action("call-2", 0)).unwrap_err();
        assert!(error.to_string().contains("UNIQUE constraint failed"));
        assert!(store.get_action("run-1", "call-2").unwrap().is_none());
        let events = RunStore::new(&db).events_after("run-1", None).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, "action.received");
    }

    #[test]
    fn approval_response_is_idempotent_and_expiry_is_enforced() {
        let db = database_with_run();
        let store = RunControlStore::new(&db);
        store.create_action(&action("call-1", 0)).unwrap();
        store
            .transition_action("run-1", "call-1", RunActionState::Prepared, 3, None, None)
            .unwrap();
        store
            .request_approval(&NewRunApproval {
                approval_id: "approval-1",
                run_id: "run-1",
                call_id: "call-1",
                scope: "single_call",
                risk: "high",
                reason: Some("requires user approval"),
                policy_snapshot: Some("always_ask"),
                expires_at: Some(20),
                now: 4,
            })
            .unwrap();
        assert_eq!(
            store
                .respond_approval("approval-1", true, "user:test", 5)
                .unwrap(),
            ControlMutation::Applied
        );
        assert_eq!(
            store
                .respond_approval("approval-1", true, "user:test", 6)
                .unwrap(),
            ControlMutation::Unchanged
        );
        assert!(store
            .respond_approval("approval-1", false, "user:test", 7)
            .is_err());
        assert_eq!(
            store.get_action("run-1", "call-1").unwrap().unwrap().state,
            RunActionState::Queued
        );

        store.create_action(&action("call-2", 1)).unwrap();
        store
            .transition_action("run-1", "call-2", RunActionState::Prepared, 8, None, None)
            .unwrap();
        store
            .request_approval(&NewRunApproval {
                approval_id: "approval-2",
                run_id: "run-1",
                call_id: "call-2",
                scope: "single_call",
                risk: "high",
                reason: None,
                policy_snapshot: None,
                expires_at: Some(9),
                now: 8,
            })
            .unwrap();
        assert!(store
            .respond_approval("approval-2", true, "user:test", 10)
            .is_err());
        assert_eq!(
            store.get_approval("approval-2").unwrap().unwrap().state,
            ApprovalState::Expired
        );
    }

    #[test]
    fn lease_epoch_fences_stale_owners() {
        let db = database_with_run();
        let store = RunControlStore::new(&db);
        let first = store
            .acquire_lease(&NewExecutionLease {
                lease_id: "lease-1",
                resource_kind: "run",
                resource_id: "run-1",
                owner: "worker-a",
                fencing_token_hash: "hash-a",
                expires_at: 10,
                now: 2,
            })
            .unwrap();
        assert_eq!(first.epoch, 1);
        assert!(store
            .acquire_lease(&NewExecutionLease {
                lease_id: "lease-conflict",
                resource_kind: "run",
                resource_id: "run-1",
                owner: "worker-b",
                fencing_token_hash: "hash-b",
                expires_at: 11,
                now: 3,
            })
            .is_err());
        let second = store
            .acquire_lease(&NewExecutionLease {
                lease_id: "lease-2",
                resource_kind: "run",
                resource_id: "run-1",
                owner: "worker-b",
                fencing_token_hash: "hash-b",
                expires_at: 20,
                now: 11,
            })
            .unwrap();
        assert_eq!(second.epoch, 2);
        assert!(!store
            .validate_fence("run", "run-1", "worker-a", first.epoch, 12)
            .unwrap());
        assert!(store
            .validate_fence("run", "run-1", "worker-b", second.epoch, 12)
            .unwrap());
    }

    #[test]
    fn recovery_preserves_prepared_and_blocked_but_never_replays_running() {
        let db = database_with_run();
        let store = RunControlStore::new(&db);
        for (call_id, sequence) in [("prepared", 0), ("blocked", 1), ("running", 2)] {
            store.create_action(&action(call_id, sequence)).unwrap();
            store
                .transition_action("run-1", call_id, RunActionState::Prepared, 3, None, None)
                .unwrap();
        }
        store
            .transition_action(
                "run-1",
                "blocked",
                RunActionState::Blocked,
                4,
                Some("wait"),
                None,
            )
            .unwrap();
        store
            .transition_action("run-1", "running", RunActionState::Running, 4, None, None)
            .unwrap();
        let recovery = store.recover(50).unwrap();
        assert_eq!(recovery.failed_running_actions, 1);
        assert_eq!(
            store
                .get_action("run-1", "prepared")
                .unwrap()
                .unwrap()
                .state,
            RunActionState::Prepared
        );
        assert_eq!(
            store.get_action("run-1", "blocked").unwrap().unwrap().state,
            RunActionState::Blocked
        );
        let running = store.get_action("run-1", "running").unwrap().unwrap();
        assert_eq!(running.state, RunActionState::Failed);
        assert_eq!(
            running.blocked_reason.as_deref(),
            Some("process_restarted_while_running")
        );
    }
}
