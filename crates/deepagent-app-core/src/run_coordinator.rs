//! Single control-plane boundary for active runs, approvals and interrupts.
//!
//! The coordinator deliberately does not execute a run. `AgentKernel` remains
//! the only runtime executor and terminal-state writer; this type owns the
//! cross-transport control signals that must survive a client disconnect.

use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use deepagent_core::error::Result;
use deepagent_persistence::run_control::RunControlStore;
use deepagent_persistence::Database;

use crate::approval_bridge::PendingApprovals;

pub(crate) type CancellationMap = Arc<Mutex<HashMap<String, Arc<AtomicBool>>>>;

#[derive(Clone)]
pub struct RunCoordinator {
    db: Arc<Database>,
    pending: PendingApprovals,
    cancellations: CancellationMap,
    run_ids: Arc<Mutex<HashMap<String, String>>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CancelRequest {
    pub accepted: bool,
}

impl RunCoordinator {
    pub fn new(db: Arc<Database>) -> Self {
        Self {
            db,
            pending: PendingApprovals::new(),
            cancellations: Arc::new(Mutex::new(HashMap::new())),
            run_ids: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn pending(&self) -> PendingApprovals {
        self.pending.clone()
    }

    pub(crate) fn cancellation_map(&self) -> CancellationMap {
        self.cancellations.clone()
    }

    pub(crate) fn register(
        &self,
        run_id: impl Into<String>,
        session_id: Option<&str>,
    ) -> ActiveRunRegistration {
        ActiveRunRegistration::new(
            self.cancellations.clone(),
            self.run_ids.clone(),
            run_id.into(),
            session_id,
        )
    }

    /// Persist the cancellation request before publishing the in-memory flag.
    /// A missing active alias is not an error: the request is acknowledged as
    /// not accepted and no durable event is fabricated.
    pub fn request_cancel(&self, alias: &str) -> Result<CancelRequest> {
        let (run_id, flag) = {
            let map = self.cancellations.lock().unwrap_or_else(|p| p.into_inner());
            let Some(flag) = map.get(alias).cloned() else {
                return Ok(CancelRequest { accepted: false });
            };
            let run_id = self
                .run_ids
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .get(alias)
                .cloned()
                .unwrap_or_else(|| alias.to_string());
            (run_id, flag)
        };
        // `alias` is normally the run id. For session/turn aliases, the event
        // is still safely attached by the active registration's canonical key.
        RunControlStore::new(&self.db).append_control_signal(
            &run_id,
            "cancel.requested",
            &serde_json::json!({ "alias": alias }),
        )?;
        flag.store(true, std::sync::atomic::Ordering::Release);
        Ok(CancelRequest { accepted: true })
    }

    pub fn resolve_approval(
        &self,
        approval_id: &str,
        approved: bool,
        decided_by: &str,
    ) -> Result<bool> {
        let controls = RunControlStore::new(&self.db);
        if controls.get_approval(approval_id)?.is_some() {
            if let Err(error) =
                controls.respond_approval(approval_id, approved, decided_by, now_millis())
            {
                if controls.get_approval(approval_id)?.is_some_and(|record| {
                    record.state == deepagent_persistence::run_control::ApprovalState::Expired
                }) {
                    let _ = self.pending.resolve_approved(approval_id, false);
                }
                return Err(error);
            }
            let _ = self.pending.resolve_approved(approval_id, approved);
            return Ok(true);
        }
        Ok(self.pending.resolve_approved(approval_id, approved))
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
                "replaces_turn_id": replaces_turn_id,
                "new_turn_id": new_turn_id,
            }),
        )
    }
}

pub(crate) struct ActiveRunRegistration {
    map: CancellationMap,
    run_ids: Arc<Mutex<HashMap<String, String>>>,
    run_id: String,
    flag: Arc<AtomicBool>,
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
            flag: Arc::new(AtomicBool::new(false)),
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
            .insert(key.clone(), self.flag.clone());
        self.run_ids
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(key.clone(), self.run_id.clone());
        keys.push(key);
    }

    pub(crate) fn flag(&self) -> Arc<AtomicBool> {
        self.flag.clone()
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
                .is_some_and(|flag| Arc::ptr_eq(flag, &self.flag))
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
    use deepagent_persistence::run_store::RunStore;

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

        let events = RunStore::new(&db)
            .events_after("run-1", None)
            .expect("events");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, "cancel.requested");
        assert_eq!(events[0].data["alias"], session_alias);

        drop(registration);
        assert!(
            !coordinator
                .request_cancel(&session_alias)
                .expect("cancel after drop")
                .accepted
        );
    }
}
