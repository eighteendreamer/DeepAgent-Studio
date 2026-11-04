//! H96: temporary schedules and real SQLite admission receipts; no user data.
#![cfg(windows)]

#[path = "fixtures/scheduled_admission.rs"]
mod scheduled_admission;
use deepagent_persistence::Database;
use deepagent_runtime::schedule::{cron::local_to_epoch_ms, run_tick_loop, CronScheduler};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

struct WritableOnDrop(std::path::PathBuf, std::fs::Permissions);

impl WritableOnDrop {
    fn new(path: std::path::PathBuf) -> Self {
        let permissions = std::fs::metadata(&path).unwrap().permissions();
        Self(path, permissions)
    }
}

impl Drop for WritableOnDrop {
    fn drop(&mut self) {
        let _ = std::fs::set_permissions(&self.0, self.1.clone());
    }
}

fn readonly(path: &std::path::Path, value: bool) {
    let mut permissions = std::fs::metadata(path).unwrap().permissions();
    permissions.set_readonly(value);
    std::fs::set_permissions(path, permissions).unwrap();
}

#[test]
fn h96_post_dispatch_write_failure_does_not_repeat_the_same_occurrence_after_reopen() {
    let now = time::macros::datetime!(2026-11-04 12:00:00);
    let mut repeats = Vec::new();
    for recurring in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let scheduler = CronScheduler::for_workspace(root.path());
        scheduler
            .store()
            .add(
                "* * * * *".into(),
                "H96 synthetic delivery".into(),
                recurring,
                local_to_epoch_ms(now) - 120_000,
            )
            .unwrap();
        let path = scheduler.store().path().to_owned();
        let _restore = WritableOnDrop::new(path.clone());
        let db = Database::open(root.path().join("runs.sqlite")).unwrap();
        let mut deliveries = 0;
        let first = scheduler.poll(now, |task, occurrence| {
            let receipt = scheduled_admission::accept(&db, task, occurrence, || deliveries += 1)?;
            readonly(&path, true);

            Ok(receipt)
        });
        assert!(
            first.is_err(),
            "real schedule write must fail after delivery"
        );
        assert_eq!(deliveries, 1);
        readonly(&path, false);
        let reopened = CronScheduler::for_workspace(root.path());
        let db = Database::open(root.path().join("runs.sqlite")).unwrap();
        reopened
            .poll(now, |task, occurrence| {
                scheduled_admission::accept(&db, task, occurrence, || deliveries += 1)
            })
            .unwrap();
        if deliveries != 1 {
            repeats.push(recurring);
        }
        eprintln!("H96 recurring={recurring} deliveries={deliveries}");
    }
    assert!(
        repeats.is_empty(),
        "same occurrence redispatched: recurring={repeats:?}"
    );
}

#[tokio::test]
async fn h96_transient_post_dispatch_error_does_not_end_the_tick_owner() {
    let now = time::macros::datetime!(2026-11-04 12:00:00);
    let root = tempfile::tempdir().unwrap();
    let scheduler = CronScheduler::for_workspace(root.path());
    scheduler
        .store()
        .add(
            "* * * * *".into(),
            "H96 tick recovery".into(),
            false,
            local_to_epoch_ms(now) - 120_000,
        )
        .unwrap();
    let path = scheduler.store().path().to_owned();
    let _restore = WritableOnDrop::new(path.clone());
    let shutdown = CancellationToken::new();
    let stop = shutdown.clone();
    let calls = Arc::new(Mutex::new(0));
    let observed = calls.clone();
    let db = Database::open(root.path().join("runs.sqlite")).unwrap();
    let mut ticks = 0;
    let loop_result = tokio::time::timeout(
        Duration::from_secs(3),
        run_tick_loop(
            scheduler,
            Duration::from_millis(5),
            || {
                ticks += 1;
                if ticks == 2 {
                    readonly(&path, false);
                }
                if ticks >= 4 {
                    stop.cancel();
                }
                now
            },
            |task, occurrence| {
                scheduled_admission::accept(&db, task, occurrence, || {
                    *observed.lock().unwrap() += 1;
                    readonly(&path, true);
                })
            },
            shutdown,
        ),
    )
    .await
    .unwrap();
    readonly(&path, false);
    assert_eq!(*calls.lock().unwrap(), 1);
    assert!(CronScheduler::for_workspace(root.path())
        .store()
        .read()
        .unwrap()
        .is_empty());
    assert!(
        loop_result.is_ok(),
        "recoverable bookkeeping error killed the tick owner"
    );
    assert!(
        ticks >= 4,
        "tick owner must remain alive after the failed write"
    );
}

#[test]
fn h96_successful_one_shot_and_recurring_delivery_controls() {
    let now = time::macros::datetime!(2026-11-04 12:00:00);
    for recurring in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let scheduler = CronScheduler::for_workspace(root.path());
        scheduler
            .store()
            .add(
                "* * * * *".into(),
                "H96 successful control".into(),
                recurring,
                local_to_epoch_ms(now) - 120_000,
            )
            .unwrap();
        let db = Database::open(root.path().join("runs.sqlite")).unwrap();
        let mut deliveries = 0;
        scheduler
            .poll(now, |task, occurrence| {
                scheduled_admission::accept(&db, task, occurrence, || deliveries += 1)
            })
            .unwrap();
        scheduler
            .poll(now, |task, occurrence| {
                scheduled_admission::accept(&db, task, occurrence, || deliveries += 1)
            })
            .unwrap();
        assert_eq!(deliveries, 1);
        scheduler
            .poll(now + time::Duration::minutes(1), |task, occurrence| {
                scheduled_admission::accept(&db, task, occurrence, || deliveries += 1)
            })
            .unwrap();
        assert_eq!(deliveries, if recurring { 2 } else { 1 });
    }
}

#[test]
fn h96_late_reconciliation_preserves_the_next_recurring_occurrence() {
    let now = time::macros::datetime!(2026-11-04 12:00:00);
    let root = tempfile::tempdir().unwrap();
    let scheduler = CronScheduler::for_workspace(root.path());
    scheduler
        .store()
        .add(
            "* * * * *".into(),
            "H96 late retry".into(),
            true,
            local_to_epoch_ms(now) - 120_000,
        )
        .unwrap();
    let db = Database::open(root.path().join("runs.sqlite")).unwrap();
    let path = scheduler.store().path().to_path_buf();
    let _restore = WritableOnDrop::new(path.clone());
    let mut deliveries = 0;
    let mut first_id = String::new();
    assert!(scheduler
        .poll(now, |task, occurrence| {
            first_id.clone_from(&occurrence.run_id);
            let receipt = scheduled_admission::accept(&db, task, occurrence, || deliveries += 1)?;
            readonly(&path, true);
            Ok(receipt)
        })
        .is_err());
    readonly(&path, false);
    let later = now + time::Duration::minutes(5);
    scheduler
        .poll(later, |task, occurrence| {
            assert_eq!(occurrence.run_id, first_id);
            scheduled_admission::accept(&db, task, occurrence, || deliveries += 1)
        })
        .unwrap();
    assert_eq!(deliveries, 1);
    assert_eq!(
        scheduler.store().read().unwrap()[0].last_fired_at,
        Some(local_to_epoch_ms(now))
    );
    scheduler
        .poll(later, |task, occurrence| {
            assert_ne!(occurrence.run_id, first_id);
            scheduled_admission::accept(&db, task, occurrence, || deliveries += 1)
        })
        .unwrap();
    assert_eq!(
        deliveries, 2,
        "the next pending slot cannot be silently lost to retry time"
    );
    assert_eq!(
        scheduler.store().read().unwrap()[0].last_fired_at,
        Some(local_to_epoch_ms(later))
    );
}

#[test]
fn h96_one_failed_admission_does_not_block_or_redispatch_healthy_tasks() {
    let now = time::macros::datetime!(2026-11-04 12:00:00);
    let root = tempfile::tempdir().unwrap();
    let scheduler = CronScheduler::for_workspace(root.path());
    let db = Database::open(root.path().join("runs.sqlite")).unwrap();
    let mut ids = Vec::new();
    for prompt in ["fails", "one-shot", "recurring"] {
        ids.push(
            scheduler
                .store()
                .add(
                    "* * * * *".into(),
                    prompt.into(),
                    prompt == "recurring",
                    local_to_epoch_ms(now) - 120_000,
                )
                .unwrap()
                .id,
        );
    }
    let mut deliveries = std::collections::BTreeMap::<String, usize>::new();
    assert!(scheduler
        .poll(now, |task, occurrence| {
            if task.id == ids[0] {
                return Err(deepagent_core::CoreError::other(
                    "H96 synthetic admission failure",
                ));
            }
            scheduled_admission::accept(&db, task, occurrence, || {
                *deliveries.entry(task.id.clone()).or_default() += 1
            })
        })
        .is_err());
    assert_eq!(scheduler.store().read().unwrap().len(), 2);
    scheduler
        .poll(now, |task, occurrence| {
            scheduled_admission::accept(&db, task, occurrence, || {
                *deliveries.entry(task.id.clone()).or_default() += 1
            })
        })
        .unwrap();
    assert!(ids.iter().all(|id| deliveries.get(id) == Some(&1)));
    assert_eq!(scheduler.store().read().unwrap().len(), 1);
}

#[tokio::test]
async fn h96_repeated_admission_failure_backs_off_and_remains_cancellable() {
    let now = time::macros::datetime!(2026-11-04 12:00:00);
    let root = tempfile::tempdir().unwrap();
    let scheduler = CronScheduler::for_workspace(root.path());
    scheduler
        .store()
        .add(
            "* * * * *".into(),
            "H96 repeated failure".into(),
            false,
            local_to_epoch_ms(now) - 120_000,
        )
        .unwrap();
    let token = CancellationToken::new();
    let stop = token.clone();
    let cancel = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(80)).await;
        stop.cancel();
    });
    let mut calls = 0;
    tokio::time::timeout(
        Duration::from_secs(3),
        run_tick_loop(
            scheduler,
            Duration::from_millis(10),
            || now,
            |_, _| {
                calls += 1;
                Err(deepagent_core::CoreError::other(
                    "H96 unavailable admission",
                ))
            },
            token,
        ),
    )
    .await
    .unwrap()
    .unwrap();
    cancel.await.unwrap();
    assert!(
        (1..=4).contains(&calls),
        "retry spun without backoff: {calls}"
    );
    assert_eq!(
        CronScheduler::for_workspace(root.path())
            .store()
            .read()
            .unwrap()
            .len(),
        1
    );
}
