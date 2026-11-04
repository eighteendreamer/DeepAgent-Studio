//! Test consumer of the same durable RunStore receipt contract as AppCore.
use deepagent_core::{clock::Timestamp, error::Result, id::SessionId};
use deepagent_persistence::{
    event_store::EventStore,
    run_store::{RunAdmission, RunStore, ScheduledRunOrigin},
    Database,
};
use deepagent_runtime::schedule::{DispatchReceipt, ScheduledOccurrence, ScheduledTask};

pub fn accept(
    db: &Database,
    task: &ScheduledTask,
    occurrence: &ScheduledOccurrence,
    delivered: impl FnOnce(),
) -> Result<DispatchReceipt> {
    let store = RunStore::new(db);
    if let Some(origin) = store.scheduled_origin(&occurrence.run_id)? {
        return Ok(DispatchReceipt {
            accepted_at_ms: origin.accepted_at_ms,
        });
    }
    let session = SessionId::new();
    EventStore::new(db).create_session(
        session,
        None,
        Timestamp::from_millis(occurrence.polled_at_ms),
    )?;
    let origin = ScheduledRunOrigin {
        version: 1,
        occurrence_id: occurrence.run_id.clone(),
        task_id: task.id.clone(),
        schedule_anchor_ms: occurrence.schedule_anchor_ms,
        scheduled_for: occurrence.scheduled_for.clone(),
        accepted_at_ms: occurrence.polled_at_ms,
    };
    assert!(store.admit(
        RunAdmission {
            run_id: &occurrence.run_id,
            session_id: &session.to_string(),
            task_id: None,
            replaces_run_id: None,
            capability_snapshot: None,
            scheduled: Some(&origin),
        },
        occurrence.polled_at_ms
    )?);
    delivered();
    Ok(DispatchReceipt {
        accepted_at_ms: origin.accepted_at_ms,
    })
}
