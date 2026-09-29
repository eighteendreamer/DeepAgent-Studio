//! Scheduled-task evaluation loop.
//!
//! The decision core — which tasks are due at a given time and what firing
//! does to them (delete a one-shot, reschedule a recurring, expire an old
//! recurring) — is a pure, time-injectable function. "Advancing time" in a test
//! is just calling [`CronScheduler::poll`] with a later `now`; real runs get a
//! thin [`tokio::time`] tick wrapper rooted on the same poll.
//!
//! The `now` the caller injects is a naive *local* calendar minute, produced by
//! the workspace clock; cron expressions are evaluated in the process's local
//! timezone (Claude Code parity). [`cron::epoch_ms_to_local`] translates a
//! persisted anchor into that same calendar view so one monotonic wall clock
//! drives both.

use std::path::Path;

use deepagent_core::error::Result;
use time::PrimitiveDateTime;

use super::cron::{epoch_ms_to_local, next_local_match, parse_5_field};
use super::store::{ScheduledTask, ScheduledTaskStore, RECURRING_MAX_AGE_MS};

/// What happened to a task on a poll.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FireOutcome {
    /// The task's prompt was handed to the fire callback.
    Fired { task_id: String },
    /// The task had fired but is now past its recurring auto-expiry window and
    /// was removed without firing.
    Expired { task_id: String, reason: String },
}

/// Whether a task's next run (anchored at `last_fired_at ?? created_at`) is at
/// or before `now`. Shared by [`CronScheduler::poll`] and the missed-task scan.
pub fn is_due(task: &ScheduledTask, now: PrimitiveDateTime) -> bool {
    let Some(from) = epoch_ms_to_local(task.schedule_anchor_ms()) else {
        return false;
    };
    let Ok(fields) = parse_5_field(&task.cron) else {
        return false;
    };
    next_local_match(&fields, from).is_some_and(|next| next <= now)
}

/// Tasks whose first run (from their creation anchor) is already in the past —
/// e.g. a session that was down across the scheduled slot. Surfaced to the user
/// at startup (CC `findMissedTasks`).
pub fn missed_tasks(tasks: &[ScheduledTask], now: PrimitiveDateTime) -> Vec<&ScheduledTask> {
    tasks.iter().filter(|t| is_due(t, now)).collect()
}

/// File-backed scheduler. Each [`CronScheduler::poll`] is one read-modify-write
/// over the store; fire side effects happen inside the caller's callback.
#[derive(Debug, Clone)]
pub struct CronScheduler {
    store: ScheduledTaskStore,
}

impl CronScheduler {
    pub fn new(store: ScheduledTaskStore) -> Self {
        Self { store }
    }

    /// Store rooted at `workspace` (used by the app-core integration).
    pub fn for_workspace(workspace: &Path) -> Self {
        Self::new(ScheduledTaskStore::new(workspace))
    }

    pub fn store(&self) -> &ScheduledTaskStore {
        &self.store
    }

    /// Evaluate every stored task against `now`. Due tasks are handed to `fire`
    /// (the caller decides what "running the prompt" means — enqueue, inject
    /// into a session, etc.). Bookkeeping is applied afterwards in one batched
    /// write per kind:
    /// - one-shot → removed after firing,
    /// - recurring → `last_fired_at` stamped (reschedule anchor survives restarts),
    /// - recurring past the auto-expiry window → removed **without** firing.
    pub fn poll(
        &self,
        now: PrimitiveDateTime,
        mut fire: impl FnMut(&ScheduledTask),
    ) -> Result<Vec<FireOutcome>> {
        let now_ms = super::cron::local_to_epoch_ms(now);
        let tasks = self.store.read()?;
        let mut outcomes = Vec::new();
        let mut to_delete: Vec<String> = Vec::new();
        let mut to_mark_fired: Vec<String> = Vec::new();

        for task in &tasks {
            if !is_due(task, now) {
                continue;
            }
            let age_ms = now_ms.saturating_sub(task.created_at);
            if task.is_recurring()
                && !task.permanent.unwrap_or(false)
                && RECURRING_MAX_AGE_MS > 0
                && age_ms > RECURRING_MAX_AGE_MS
            {
                to_delete.push(task.id.clone());
                outcomes.push(FireOutcome::Expired {
                    task_id: task.id.clone(),
                    reason: format!("recurring task older than {RECURRING_MAX_AGE_MS}ms"),
                });
                continue;
            }
            fire(task);
            if task.is_recurring() {
                to_mark_fired.push(task.id.clone());
            } else {
                to_delete.push(task.id.clone());
            }
            outcomes.push(FireOutcome::Fired {
                task_id: task.id.clone(),
            });
        }

        if !to_delete.is_empty() {
            self.store.remove(&to_delete)?;
        }
        if !to_mark_fired.is_empty() {
            self.store.mark_fired(&to_mark_fired, now_ms)?;
        }
        Ok(outcomes)
    }
}

/// Run the scheduler loop at a fixed tick until `shutdown` fires. One poll per
/// tick. `clock` yields the current local calendar minute (the app-core bound
/// plugs the real wall clock here; tests plug a deterministic one).
pub async fn run_tick_loop(
    scheduler: CronScheduler,
    tick: std::time::Duration,
    mut clock: impl FnMut() -> PrimitiveDateTime + Send,
    mut fire: impl FnMut(&ScheduledTask) + Send,
    shutdown: tokio_util::sync::CancellationToken,
) -> Result<()> {
    let mut interval = tokio::time::interval(tick);
    loop {
        tokio::select! {
            _ = shutdown.cancelled() => return Ok(()),
            _ = interval.tick() => {
                let outcomes = scheduler.poll(clock(), &mut fire)?;
                if !outcomes.is_empty() {
                    tracing::info!(n = outcomes.len(), "scheduled tasks fired");
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schedule::store::ScheduledTaskStore;

    #[test]
    fn one_shot_fires_once_then_is_removed() {
        let dir = tempfile::tempdir().unwrap();
        let store = ScheduledTaskStore::new_at(dir.path().join("scheduled-tasks.json"));
        let one_shot = store
            .add("0 9 * * *".into(), "once".into(), false, 1_700_000_000_000)
            .unwrap();
        let sched = CronScheduler::new(store);

        let fired_at = time::macros::datetime!(2026-09-15 09:00:00);
        let mut fired: Vec<String> = Vec::new();
        let outcomes = sched.poll(fired_at, |t| fired.push(t.id.clone())).unwrap();
        assert_eq!(outcomes.len(), 1);
        assert_eq!(fired, vec![one_shot.id]);
        assert!(sched.store().read().unwrap().is_empty(), "one-shot removed");
    }

    #[test]
    fn recurring_reschedules_from_fire_time() {
        let dir = tempfile::tempdir().unwrap();
        let store = ScheduledTaskStore::new_at(dir.path().join("scheduled-tasks.json"));
        let created_ms =
            crate::schedule::cron::local_to_epoch_ms(time::macros::datetime!(2026-09-14 00:00:00));
        let recurring = store
            .add("0 9 * * *".into(), "daily".into(), true, created_ms)
            .unwrap();
        let sched = CronScheduler::new(store);

        let day1 = time::macros::datetime!(2026-09-15 09:00:00);
        let mut fired: Vec<String> = Vec::new();
        let outcomes = sched.poll(day1, |t| fired.push(t.id.clone())).unwrap();
        assert_eq!(
            outcomes,
            vec![FireOutcome::Fired {
                task_id: recurring.id.clone()
            }]
        );
        assert_eq!(fired, vec![recurring.id.clone()]);
        // Still present, stamped as fired at day1 (same inverse conversion the
        // poll used, so the assertion is tz-consistent with the write).
        let stored = sched.store().read().unwrap();
        assert_eq!(stored.len(), 1);
        let fired_ms = crate::schedule::cron::local_to_epoch_ms(day1);
        assert_eq!(stored[0].last_fired_at, Some(fired_ms));
        // Same day later: NOT due again (next is strictly after the last fire).
        let noon = time::macros::datetime!(2026-09-15 12:00:00);
        let outcomes = sched.poll(noon, |t| fired.push(t.id.clone())).unwrap();
        assert!(outcomes.is_empty());
        assert_eq!(fired.len(), 1);
        // Next day at 09:00 exactly: due again (strictly after means 09:00
        // counts once its anchor is the previous 09:00).
    }

    #[test]
    fn recurring_fires_again_next_schedule_slot() {
        let dir = tempfile::tempdir().unwrap();
        let store = ScheduledTaskStore::new_at(dir.path().join("scheduled-tasks.json"));
        let created_ms =
            crate::schedule::cron::local_to_epoch_ms(time::macros::datetime!(2026-09-14 00:00:00));
        store
            .add("0 9 * * *".into(), "daily".into(), true, created_ms)
            .unwrap();
        let sched = CronScheduler::new(store);
        let day1 = time::macros::datetime!(2026-09-15 09:00:00);
        let day2 = time::macros::datetime!(2026-09-16 09:00:00);
        let mut fired: Vec<String> = Vec::new();
        let o1 = sched.poll(day1, |t| fired.push(t.id.clone())).unwrap();
        let o2 = sched.poll(day2, |t| fired.push(t.id.clone())).unwrap();
        assert_eq!(o1.len(), 1);
        assert_eq!(o2.len(), 1);
        assert_eq!(fired.len(), 2);
    }

    #[test]
    fn recurring_expires_after_max_age_without_firing() {
        let dir = tempfile::tempdir().unwrap();
        let store = ScheduledTaskStore::new_at(dir.path().join("scheduled-tasks.json"));
        // Created a long time ago (well past the 7-day window).
        let old_ms = 1_500_000_000_000i64; // 2017-07-14
        let recurring = store
            .add("0 9 * * *".into(), "old".into(), true, old_ms)
            .unwrap();
        let sched = CronScheduler::new(store);
        let now = time::macros::datetime!(2026-09-15 09:00:00);
        let mut fired: Vec<String> = Vec::new();
        let outcomes = sched.poll(now, |t| fired.push(t.id.clone())).unwrap();
        assert!(fired.is_empty(), "expired task must NOT fire");
        assert_eq!(
            outcomes,
            vec![FireOutcome::Expired {
                task_id: recurring.id.clone(),
                reason: "recurring task older than 604800000ms".into(),
            }]
        );
        assert!(
            sched.store().read().unwrap().is_empty(),
            "expired task removed"
        );
    }

    #[test]
    fn permanent_recurring_never_expires() {
        let dir = tempfile::tempdir().unwrap();
        let store = ScheduledTaskStore::new_at(dir.path().join("scheduled-tasks.json"));
        let old_ms = 1_500_000_000_000i64;
        let task = store
            .add("0 9 * * *".into(), "perm".into(), true, old_ms)
            .unwrap();
        let mut permanent = task.clone();
        permanent.permanent = Some(true);
        store.write(&[permanent]).unwrap();

        let sched = CronScheduler::new(store);
        let now = time::macros::datetime!(2026-09-15 09:00:00);
        let mut fired: Vec<String> = Vec::new();
        let outcomes = sched.poll(now, |t| fired.push(t.id.clone())).unwrap();
        assert_eq!(fired, vec![task.id.clone()]);
        assert_eq!(outcomes, vec![FireOutcome::Fired { task_id: task.id }]);
        assert_eq!(sched.store().read().unwrap().len(), 1);
    }

    #[test]
    fn missed_tasks_reports_ones_whose_slot_passed() {
        let dir = tempfile::tempdir().unwrap();
        let store = ScheduledTaskStore::new_at(dir.path().join("scheduled-tasks.json"));
        let past = store
            .add(
                "0 9 * * *".into(),
                "should have run".into(),
                false,
                1_700_000_000_000,
            )
            .unwrap();
        // Created AFTER `now` below → its first run is in the future, not missed.
        let future = store
            .add(
                "0 9 * * *".into(),
                "tomorrow".into(),
                false,
                1_800_000_000_000,
            )
            .unwrap();
        // `past` was created 2026-09-15 00:00 → its 09:00 slot is behind now.
        let now = time::macros::datetime!(2026-09-15 10:00:00);
        let stored = store.read().unwrap();
        let missed = missed_tasks(&stored, now);
        let ids: Vec<&str> = missed.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, vec![past.id.as_str()]);
        assert!(!ids.contains(&future.id.as_str()));
    }
}
