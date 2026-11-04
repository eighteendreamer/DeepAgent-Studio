//! A schedule occurrence is one normal prepared admission, not another executor.
use super::*;
use deepagent_persistence::run_store::{RunStore, ScheduledRunOrigin};
use deepagent_runtime::schedule::{DispatchReceipt, ScheduledOccurrence, ScheduledTask};

impl ChatService {
    /// Caller holds the scheduler's stable file lease. An existing admission
    /// (active, interrupted, failed or completed) is reconciled, never executed
    /// again. Startup recovery remains the existing RunStore/Kernel boundary.
    pub(crate) fn admit_scheduled(
        &self,
        task: &ScheduledTask,
        occurrence: &ScheduledOccurrence,
    ) -> Result<(
        DispatchReceipt,
        Option<crate::run_assembler::PreparedChatRun>,
    )> {
        if let Some(origin) = RunStore::new(&self.db).scheduled_origin(&occurrence.run_id)? {
            if origin.task_id != task.id
                || origin.schedule_anchor_ms != occurrence.schedule_anchor_ms
                || origin.scheduled_for != occurrence.scheduled_for
            {
                return Err(CoreError::Persistence(
                    "scheduled admission identity conflict; do not redispatch".into(),
                ));
            }
            tracing::info!(run_id = %occurrence.run_id, task_id = %task.id, "reconciled existing scheduled admission without redispatch");
            return Ok((
                DispatchReceipt {
                    accepted_at_ms: origin.accepted_at_ms,
                },
                None,
            ));
        }
        let session_id = self.create_session(None)?;
        let origin = ScheduledRunOrigin {
            version: 1,
            occurrence_id: occurrence.run_id.clone(),
            task_id: task.id.clone(),
            schedule_anchor_ms: occurrence.schedule_anchor_ms,
            scheduled_for: occurrence.scheduled_for.clone(),
            accepted_at_ms: occurrence.polled_at_ms,
        };
        let prepared = self
            .admit_prepared_chat(
                &occurrence.run_id,
                &session_id,
                Some(&task.prompt),
                None,
                Some(&origin),
                HarnessRunOverrides::default(),
            )?
            .ok_or_else(|| CoreError::invalid("scheduled prepared admission was rejected"))?;
        Ok((
            DispatchReceipt {
                accepted_at_ms: origin.accepted_at_ms,
            },
            Some(prepared),
        ))
    }
}
