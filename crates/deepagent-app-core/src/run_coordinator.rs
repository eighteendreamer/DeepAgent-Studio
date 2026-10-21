//! Single control-plane boundary for active runs, approvals and interrupts.
//!
//! The coordinator deliberately does not execute a run. `AgentKernel` remains
//! the only runtime executor and terminal-state writer; this type owns the
//! cross-transport control signals that must survive a client disconnect.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use deepagent_core::error::{CoreError, Result};
use deepagent_persistence::run_control::RunControlStore;
use deepagent_persistence::run_store::RunStore;
use deepagent_persistence::Database;
use serde::Serialize;

use crate::approval_bridge::PendingApprovals;
use crate::question_bridge::PendingQuestions;

pub(crate) type CancellationMap =
    Arc<Mutex<HashMap<String, Arc<deepagent_runtime::CancellationTree>>>>;

#[derive(Clone)]
pub struct RunCoordinator {
    db: Arc<Database>,
    pending: PendingApprovals,
    pending_questions: PendingQuestions,
    cancellations: CancellationMap,
    run_ids: Arc<Mutex<HashMap<String, String>>>,
    admissions: Arc<Mutex<HashMap<String, ActiveRunRegistration>>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CancelRequest {
    pub accepted: bool,
}

/// Read-only readiness projection for the local coordinator. It reports
/// durable control pressure without creating a second runtime or event store.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CoordinatorReadiness {
    pub ready: bool,
    pub active_runs: usize,
    pub pending_approvals: u64,
    pub running_actions: u64,
    pub expired_leases: u64,
}

impl RunCoordinator {
    pub fn new(db: Arc<Database>) -> Self {
        Self {
            db,
            pending: PendingApprovals::new(),
            pending_questions: PendingQuestions::new(),
            cancellations: Arc::new(Mutex::new(HashMap::new())),
            run_ids: Arc::new(Mutex::new(HashMap::new())),
            admissions: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn pending(&self) -> PendingApprovals {
        self.pending.clone()
    }

    /// The shared pending-questions registry. The UI resolves an `ask_user_question`
    /// dialog through [`RunCoordinator::resolve_question`].
    pub fn pending_questions(&self) -> PendingQuestions {
        self.pending_questions.clone()
    }

    /// Answer a pending question with the user's map, or `None` when they
    /// declined. Returns `false` when no such question is pending.
    pub fn resolve_question(
        &self,
        session_id: &str,
        question_id: &str,
        answers: Option<std::collections::BTreeMap<String, String>>,
    ) -> bool {
        self.pending_questions
            .resolve(session_id, question_id, answers)
    }

    pub fn readiness(&self) -> Result<CoordinatorReadiness> {
        let active_runs = RunStore::new(&self.db).unfinished()?.len();
        let (pending_approvals, running_actions, expired_leases) =
            self.db.with_conn(|connection| {
                let pending: i64 = connection
                    .query_row(
                        "SELECT COUNT(*) FROM run_approvals WHERE state='pending'",
                        [],
                        |row| row.get(0),
                    )
                    .map_err(|error| CoreError::Persistence(error.to_string()))?;
                let running: i64 = connection
                    .query_row(
                        "SELECT COUNT(*) FROM run_actions WHERE state='running'",
                        [],
                        |row| row.get(0),
                    )
                    .map_err(|error| CoreError::Persistence(error.to_string()))?;
                let expired: i64 = connection.query_row(
                "SELECT COUNT(*) FROM execution_leases WHERE expires_at <= ?1 AND revoked_at IS NULL",
                [now_millis()],
                |row| row.get(0),
            ).map_err(|error| CoreError::Persistence(error.to_string()))?;
                Ok((
                    pending.max(0) as u64,
                    running.max(0) as u64,
                    expired.max(0) as u64,
                ))
            })?;
        Ok(CoordinatorReadiness {
            ready: true,
            active_runs,
            pending_approvals,
            running_actions,
            expired_leases,
        })
    }

    pub(crate) fn cancellation_map(&self) -> CancellationMap {
        self.cancellations.clone()
    }

    #[cfg(test)]
    pub(crate) fn register(
        &self,
        run_id: impl Into<String>,
        session_id: Option<&str>,
    ) -> ActiveRunRegistration {
        let run_id = run_id.into();
        if let Some(registration) = self
            .admissions
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&run_id)
        {
            return registration;
        }
        ActiveRunRegistration::new(
            self.cancellations.clone(),
            self.run_ids.clone(),
            run_id,
            session_id,
        )
    }

    /// Consume the cancellation registration created before the transport ACK.
    /// A run can be executed only once, even if two worker tasks race to resume
    /// the same durable admission.
    pub(crate) fn take_admission(&self, run_id: &str) -> Result<ActiveRunRegistration> {
        self.admissions
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(run_id)
            .ok_or_else(|| CoreError::invalid("run admission is missing or already consumed"))
    }

    /// Reserve both durable identity and cancellation before transport ACK.
    /// Session aliases are attached only after the input lease is acquired.
    pub fn admit(&self, run_id: &str, session_id: &str) -> Result<()> {
        let mut admissions = self.admissions.lock().unwrap_or_else(|p| p.into_inner());
        RunStore::new(&self.db).create(run_id, session_id, None, now_millis())?;
        let registration = ActiveRunRegistration::new(
            self.cancellations.clone(),
            self.run_ids.clone(),
            run_id.to_owned(),
            None,
        );
        admissions.insert(run_id.to_owned(), registration);
        Ok(())
    }

    /// Publish cancellation without waiting for storage. The run lifecycle
    /// persists the recorded intent together with its terminal boundary.
    /// A missing active alias is not an error: the request is acknowledged as
    /// not accepted and no durable event is fabricated.
    pub fn request_cancel(&self, alias: &str) -> Result<CancelRequest> {
        let cancellation = {
            let map = self.cancellations.lock().unwrap_or_else(|p| p.into_inner());
            let Some(cancellation) = map.get(alias).cloned() else {
                return Ok(CancelRequest { accepted: false });
            };
            cancellation
        };
        cancellation.request_cancel(alias);
        // `accepted` means the alias still addressed an active run. Repeated
        // clicks while it is already cancelling remain acknowledged so the UI
        // does not incorrectly report a transient not-found error.
        Ok(CancelRequest { accepted: true })
    }

    pub fn resolve_approval(
        &self,
        approval_id: &str,
        approved: bool,
        decided_by: &str,
    ) -> Result<bool> {
        self.resolve_approval_scoped(approval_id, approved, None, decided_by)
    }

    pub fn resolve_approval_scoped(
        &self,
        approval_id: &str,
        approved: bool,
        requested_scope: Option<&str>,
        decided_by: &str,
    ) -> Result<bool> {
        let controls = RunControlStore::new(&self.db);
        let record = controls.get_approval(approval_id)?;
        // The gate owns which scopes an answer may widen to, because the gate is
        // also what honors the resulting grant.
        self.pending.check_scope(approval_id, requested_scope)?;
        if record.is_some() {
            if let Err(error) = controls.respond_approval_scoped(
                approval_id,
                approved,
                decided_by,
                requested_scope,
                now_millis(),
            ) {
                if controls.get_approval(approval_id)?.is_some_and(|record| {
                    record.state == deepagent_persistence::run_control::ApprovalState::Expired
                }) {
                    let _ = self.pending.resolve_approved(approval_id, false);
                }
                return Err(error);
            }
        }
        // Only after the durable answer lands does a grant take effect, so an
        // expired or rejected answer can never leave a standing permission behind.
        let awakened = self
            .pending
            .resolve_scoped(approval_id, approved, requested_scope)?;
        Ok(awakened || record.is_some())
    }

    pub fn record_continuation(
        &self,
        run_alias: &str,
        replaces_turn_id: &str,
        new_turn_id: &str,
    ) -> Result<u64> {
        RunControlStore::new(&self.db).append_control_signal(
            &self
                .run_ids
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .get(run_alias)
                .cloned()
                .unwrap_or_else(|| run_alias.to_string()),
            "continuation.created",
            &serde_json::json!({
                "replacesTurnId": replaces_turn_id,
                "newTurnId": new_turn_id,
                "disposition": "new_turn",
            }),
        )
    }
}

pub(crate) struct ActiveRunRegistration {
    map: CancellationMap,
    run_ids: Arc<Mutex<HashMap<String, String>>>,
    run_id: String,
    cancellation: Arc<deepagent_runtime::CancellationTree>,
    keys: Mutex<Vec<String>>,
}

impl ActiveRunRegistration {
    fn new(
        map: CancellationMap,
        run_ids: Arc<Mutex<HashMap<String, String>>>,
        run_id: String,
        session_id: Option<&str>,
    ) -> Self {
        let registration = Self {
            map,
            run_ids,
            run_id: run_id.clone(),
            cancellation: Arc::new(deepagent_runtime::CancellationTree::new()),
            keys: Mutex::new(Vec::new()),
        };
        registration.add_alias(run_id);
        if let Some(session_id) = session_id {
            registration.add_alias(session_id.to_string());
        }
        registration
    }

    pub(crate) fn add_alias(&self, key: String) {
        let mut keys = self.keys.lock().unwrap_or_else(|p| p.into_inner());
        if keys.contains(&key) {
            return;
        }
        self.map
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(key.clone(), self.cancellation.clone());
        self.run_ids
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(key.clone(), self.run_id.clone());
        keys.push(key);
    }

    pub(crate) fn flag(&self) -> Arc<std::sync::atomic::AtomicBool> {
        self.cancellation.legacy_flag()
    }

    pub(crate) fn cancellation(&self) -> deepagent_runtime::CancellationTree {
        (*self.cancellation).clone()
    }
}

impl Drop for ActiveRunRegistration {
    fn drop(&mut self) {
        let keys = self.keys.lock().unwrap_or_else(|p| p.into_inner());
        let mut map = self.map.lock().unwrap_or_else(|p| p.into_inner());
        let mut run_ids = self.run_ids.lock().unwrap_or_else(|p| p.into_inner());
        for key in keys.iter() {
            if map
                .get(key)
                .is_some_and(|cancellation| Arc::ptr_eq(cancellation, &self.cancellation))
            {
                map.remove(key);
            }
            if run_ids
                .get(key)
                .is_some_and(|run_id| run_id == &self.run_id)
            {
                run_ids.remove(key);
            }
        }
    }
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
    use deepagent_core::clock::Timestamp;
    use deepagent_core::id::SessionId;
    use deepagent_persistence::event_store::EventStore;
    use deepagent_persistence::run_control::{NewRunAction, NewRunApproval, RunActionState};
    use deepagent_persistence::run_store::RunStore;

    #[test]
    fn cancellation_does_not_wait_for_a_durable_run() {
        let db = Arc::new(Database::open_in_memory().expect("db"));
        let coordinator = RunCoordinator::new(db);
        let registration = coordinator.register("preparing-run", None);
        assert!(
            coordinator
                .request_cancel("preparing-run")
                .expect("cancel")
                .accepted
        );
        assert!(registration.cancellation().is_cancelled());
    }

    #[test]
    fn cancel_does_not_acquire_the_database_lock() {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let coordinator = RunCoordinator::new(db.clone());
        let registration = coordinator.register("preparing", None);
        db.with_conn(|_| {
            assert!(coordinator.request_cancel("preparing")?.accepted);
            assert!(coordinator.request_cancel("preparing")?.accepted);
            assert!(registration.cancellation().is_cancelled());
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn readiness_reads_shared_durable_control_plane() {
        let db = Arc::new(Database::open_in_memory().expect("db"));
        let readiness = RunCoordinator::new(db).readiness().expect("readiness");
        assert!(readiness.ready);
        assert_eq!(readiness.active_runs, 0);
        assert_eq!(readiness.pending_approvals, 0);
        assert_eq!(readiness.running_actions, 0);
        assert_eq!(readiness.expired_leases, 0);
        let json = serde_json::to_value(readiness).expect("json");
        assert_eq!(json["activeRuns"], 0);
    }

    #[test]
    fn cancel_persists_against_canonical_run_for_session_alias() {
        let db = Arc::new(Database::open_in_memory().expect("db"));
        let session_id = SessionId::new();
        EventStore::new(&db)
            .create_session(session_id, Some("test"), Timestamp::from_millis(1))
            .expect("session");
        RunStore::new(&db)
            .create("run-1", &session_id.to_string(), None, 1)
            .expect("run");
        let coordinator = RunCoordinator::new(db.clone());
        let session_alias = session_id.to_string();
        let registration = coordinator.register("run-1", Some(&session_alias));

        let request = coordinator.request_cancel(&session_alias).expect("cancel");
        assert!(request.accepted);
        assert!(registration
            .flag()
            .load(std::sync::atomic::Ordering::Acquire));
        assert!(registration.cancellation().is_cancelled());

        let accepted = RunStore::new(&db).events_after("run-1", None).unwrap();
        assert_eq!(accepted.len(), 1);
        assert_eq!(accepted[0].event_type, "run_accepted");
        deepagent_runtime::kernel::persist_terminal(
            &db,
            "run-1",
            &deepagent_runtime::KernelTerminal {
                kind: deepagent_runtime::TerminalKind::Cancelled,
                message: None,
                reason: None,
            },
            &registration.cancellation(),
        )
        .unwrap();
        let events = RunStore::new(&db)
            .events_after("run-1", None)
            .expect("events");
        assert_eq!(events.len(), 4);
        assert_eq!(events[0].event_type, "run_accepted");
        assert_eq!(events[1].event_type, "cancel.requested");
        assert_eq!(events[2].event_type, "cancel.propagated");
        assert_eq!(events[1].data["alias"], session_alias);

        drop(registration);
        assert!(
            !coordinator
                .request_cancel(&session_alias)
                .expect("cancel after drop")
                .accepted
        );
    }

    #[test]
    fn cancellation_survives_audit_write_failure_and_terminal_retry_is_idempotent() {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let session_id = SessionId::new();
        EventStore::new(&db)
            .create_session(
                session_id,
                Some("cancel audit fault"),
                Timestamp::from_millis(1),
            )
            .unwrap();
        let coordinator = RunCoordinator::new(db.clone());
        coordinator
            .admit("audit-fault-run", &session_id.to_string())
            .unwrap();
        let registration = coordinator.take_admission("audit-fault-run").unwrap();
        let cancellation = registration.cancellation();
        let child = cancellation.child_token();
        db.with_conn(|conn| {
            conn.execute_batch(
                "CREATE TRIGGER fail_cancel_audit BEFORE INSERT ON run_events
                 WHEN NEW.event_type='cancel.requested'
                 BEGIN SELECT RAISE(ABORT, 'injected cancel audit write failure'); END;",
            )
            .map_err(|error| CoreError::Persistence(error.to_string()))?;
            Ok(())
        })
        .unwrap();

        assert!(
            coordinator
                .request_cancel("audit-fault-run")
                .unwrap()
                .accepted
        );
        assert!(
            coordinator
                .request_cancel("audit-fault-run")
                .unwrap()
                .accepted
        );
        assert!(cancellation.is_cancelled());
        assert!(child.is_cancelled());
        assert!(registration
            .flag()
            .load(std::sync::atomic::Ordering::Acquire));
        let terminal = deepagent_runtime::KernelTerminal {
            kind: deepagent_runtime::TerminalKind::Cancelled,
            message: None,
            reason: None,
        };
        let failure = deepagent_runtime::kernel::persist_terminal(
            &db,
            "audit-fault-run",
            &terminal,
            &cancellation,
        )
        .unwrap_err();
        assert!(failure
            .to_string()
            .contains("injected cancel audit write failure"));
        let store = RunStore::new(&db);
        assert!(store
            .get("audit-fault-run")
            .unwrap()
            .unwrap()
            .finished_at
            .is_none());
        assert_eq!(
            store.events_after("audit-fault-run", None).unwrap().len(),
            1
        );
        assert!(
            cancellation.is_cancelled(),
            "storage failure cannot undo the signal"
        );

        db.with_conn(|conn| {
            conn.execute_batch("DROP TRIGGER fail_cancel_audit")
                .map_err(|error| CoreError::Persistence(error.to_string()))?;
            Ok(())
        })
        .unwrap();
        assert!(deepagent_runtime::kernel::persist_terminal(
            &db,
            "audit-fault-run",
            &terminal,
            &cancellation,
        )
        .unwrap());
        assert!(!deepagent_runtime::kernel::persist_terminal(
            &db,
            "audit-fault-run",
            &terminal,
            &cancellation,
        )
        .unwrap());
        let events = store.events_after("audit-fault-run", None).unwrap();
        assert_eq!(
            events
                .iter()
                .map(|event| event.event_type.as_str())
                .collect::<Vec<_>>(),
            [
                "run_accepted",
                "cancel.requested",
                "cancel.propagated",
                "run_terminal"
            ]
        );
        assert_eq!(
            store
                .get("audit-fault-run")
                .unwrap()
                .unwrap()
                .terminal_kind
                .as_deref(),
            Some("cancelled")
        );
        drop(registration);
        assert!(
            !coordinator
                .request_cancel("audit-fault-run")
                .unwrap()
                .accepted
        );
    }

    #[test]
    fn approval_scope_mismatch_does_not_mutate_durable_state() {
        let db = Arc::new(Database::open_in_memory().expect("db"));
        let session_id = SessionId::new();
        EventStore::new(&db)
            .create_session(session_id, Some("test"), Timestamp::from_millis(1))
            .expect("session");
        RunStore::new(&db)
            .create("run-approval", &session_id.to_string(), None, 1)
            .expect("run");
        let controls = RunControlStore::new(&db);
        controls
            .create_action(&NewRunAction {
                run_id: "run-approval",
                turn_id: "turn-1",
                call_id: "call-1",
                sequence: 0,
                tool_name: "bash",
                arguments_hash: "hash",
                risk: "high",
                parent_action_id: None,
                now: 2,
            })
            .expect("action");
        controls
            .transition_action(
                "run-approval",
                "call-1",
                RunActionState::Prepared,
                3,
                None,
                None,
            )
            .expect("prepared");
        controls
            .request_approval(&NewRunApproval {
                approval_id: "approval-1",
                run_id: "run-approval",
                call_id: "call-1",
                scope: "single_call",
                risk: "high",
                reason: Some("test"),
                policy_snapshot: Some("always_ask"),
                expires_at: None,
                now: 4,
            })
            .expect("approval");

        let coordinator = RunCoordinator::new(db.clone());
        assert!(coordinator
            .resolve_approval_scoped("approval-1", true, Some("all_future_calls"), "test")
            .is_err());
        // A grant scope needs a live request in this process; the durable row
        // alone must not be enough to hand one out.
        assert!(coordinator
            .resolve_approval_scoped("approval-1", true, Some("tool_session"), "test")
            .is_err());
        assert_eq!(
            controls.get_approval("approval-1").unwrap().unwrap().scope,
            "single_call",
            "a refused answer never rewrites the persisted scope"
        );
        assert_eq!(
            controls.get_approval("approval-1").unwrap().unwrap().state,
            deepagent_persistence::run_control::ApprovalState::Pending
        );
        assert_eq!(
            controls
                .get_action("run-approval", "call-1")
                .unwrap()
                .unwrap()
                .state,
            RunActionState::Blocked
        );

        // The plain answer still works and records the applied scope.
        assert!(coordinator
            .resolve_approval_scoped("approval-1", true, Some("single_call"), "test")
            .unwrap());
        let resolved = controls.get_approval("approval-1").unwrap().unwrap();
        assert_eq!(
            resolved.state,
            deepagent_persistence::run_control::ApprovalState::Approved
        );
        assert_eq!(resolved.scope, "single_call");
    }

    #[test]
    fn continuation_records_explicit_new_turn_disposition() {
        let db = Arc::new(Database::open_in_memory().expect("db"));
        let session_id = SessionId::new();
        EventStore::new(&db)
            .create_session(session_id, Some("test"), Timestamp::from_millis(1))
            .expect("session");
        RunStore::new(&db)
            .create("run-cont", &session_id.to_string(), None, 1)
            .expect("run");
        RunCoordinator::new(db.clone())
            .record_continuation("run-cont", "turn-old", "turn-new")
            .expect("continuation");
        let events = RunStore::new(&db).events_after("run-cont", None).unwrap();
        assert_eq!(events[0].event_type, "run_accepted");
        assert_eq!(events[1].event_type, "continuation.created");
        assert_eq!(events[1].data["disposition"], "new_turn");
        assert_eq!(events[1].data["replacesTurnId"], "turn-old");
    }
}
