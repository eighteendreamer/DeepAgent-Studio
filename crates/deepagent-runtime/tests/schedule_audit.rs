//! Opt-in scheduler ownership audit; real independent processes and file store.
//! Fire callbacks only record task ids. No model, shell tool or network runs.

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use deepagent_runtime::schedule::{cron::local_to_epoch_ms, CronScheduler};

const TEST: &str = "audit_two_scheduler_processes_cannot_fire_the_same_occurrence";
const CHILD_ROOT: &str = "DEEPAGENT_AUDIT_CRON_CHILD_ROOT";
const LOCK_TEST: &str = "audit_scheduler_lock_is_released_after_owner_exit_or_kill";
const LOCK_CHILD_ROOT: &str = "DEEPAGENT_AUDIT_CRON_LOCK_CHILD_ROOT";
const LOCK_CHILD_MODE: &str = "DEEPAGENT_AUDIT_CRON_LOCK_CHILD_MODE";

#[test]
fn audit_two_scheduler_processes_cannot_fire_the_same_occurrence() {
    let now = time::macros::datetime!(2026-10-21 12:00:00);
    if let Some(root) = std::env::var_os(CHILD_ROOT) {
        let root = PathBuf::from(root);
        let scheduler = CronScheduler::for_workspace(&root);
        let outcomes = scheduler
            .poll(now, |task| {
                std::fs::write(root.join("child-fired"), &task.id).unwrap();
            })
            .unwrap();
        std::fs::write(root.join("child-outcomes"), outcomes.len().to_string()).unwrap();
        return;
    }

    let mut duplicates = Vec::new();
    for recurring in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let scheduler = CronScheduler::for_workspace(root);
        let task = scheduler
            .store()
            .add(
                "* * * * *".into(),
                "audit scheduler ownership only".into(),
                recurring,
                local_to_epoch_ms(now) - 120_000,
            )
            .unwrap();
        let mut callback_count = 0;
        let outcomes = scheduler
            .poll(now, |fired| {
                callback_count += 1;
                assert_eq!(fired.id, task.id);
                // Hold the parent immediately after its real due evaluation
                // but before poll persists fire bookkeeping. This fixes a
                // legal two-process interleaving without replacing store IO.
                std::fs::write(root.join("parent-fired"), &fired.id).unwrap();
                let mut child = Command::new(std::env::current_exe().unwrap())
                    .args(["--exact", TEST, "--ignored", "--test-threads=1"])
                    .env(CHILD_ROOT, root)
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
                    .unwrap();
                let deadline = Instant::now() + Duration::from_secs(5);
                let child_status = loop {
                    if let Some(status) = child.try_wait().unwrap() {
                        break Some(status);
                    }
                    if Instant::now() >= deadline {
                        child.kill().unwrap();
                        child.wait().unwrap();
                        break None;
                    }
                    std::thread::sleep(Duration::from_millis(10));
                };
                assert!(
                    child_status.is_some_and(|status| status.success()),
                    "audit child must exit cleanly"
                );
            })
            .unwrap();
        let parent_id = std::fs::read_to_string(root.join("parent-fired")).unwrap();
        let child_id = std::fs::read_to_string(root.join("child-fired")).ok();
        let same_occurrence_fired_twice = child_id.as_deref() == Some(parent_id.as_str());
        let remaining = scheduler.store().read().unwrap();
        let mut sequential_refires = 0;
        scheduler.poll(now, |_| sequential_refires += 1).unwrap();
        assert_eq!(callback_count, 1);
        assert_eq!(outcomes.len(), 1);
        assert_eq!(
            sequential_refires, 0,
            "single owner sequential bookkeeping is a working control"
        );
        eprintln!("recurring={recurring} same_task_fired_in_two_processes={same_occurrence_fired_twice} remaining_tasks={} sequential_refires={sequential_refires}", remaining.len());
        if same_occurrence_fired_twice {
            duplicates.push(recurring);
        }
    }
    assert!(
        duplicates.is_empty(),
        "one occurrence was dispatched twice for recurring={duplicates:?}"
    );
}

#[test]
fn audit_scheduler_lock_is_released_after_owner_exit_or_kill() {
    let now = time::macros::datetime!(2026-10-21 12:00:00);
    if let Some(root) = std::env::var_os(LOCK_CHILD_ROOT) {
        let root = PathBuf::from(root);
        let scheduler = CronScheduler::for_workspace(&root);
        let mode = std::env::var(LOCK_CHILD_MODE).unwrap_or_default();
        let child_now = now;
        scheduler
            .poll(child_now, |_| {
                std::fs::write(root.join("lock-ready"), "ready").unwrap();
                if mode == "crash" {
                    std::thread::sleep(Duration::from_secs(30));
                } else {
                    std::thread::sleep(Duration::from_millis(250));
                }
            })
            .unwrap();
        return;
    }

    for mode in ["normal", "crash"] {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let scheduler = CronScheduler::for_workspace(root);
        let task = scheduler
            .store()
            .add(
                "* * * * *".into(),
                "audit scheduler lock lifecycle only".into(),
                true,
                local_to_epoch_ms(now) - 120_000,
            )
            .unwrap();
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", LOCK_TEST, "--test-threads=1"])
            .env(LOCK_CHILD_ROOT, root)
            .env(LOCK_CHILD_MODE, mode)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let ready_deadline = Instant::now() + Duration::from_secs(5);
        while !root.join("lock-ready").exists() {
            assert!(
                Instant::now() < ready_deadline,
                "{mode} lock owner did not acquire the scheduler lock"
            );
            std::thread::sleep(Duration::from_millis(10));
        }

        let mut fired = 0;
        let blocked = scheduler.poll(now, |_| fired += 1).unwrap();
        assert!(
            blocked.is_empty(),
            "{mode} owner must block a competing poll"
        );
        assert_eq!(fired, 0);

        if mode == "crash" {
            child.kill().unwrap();
        }
        let status = child.wait().unwrap();
        if mode == "normal" {
            assert!(status.success(), "normal lock owner must exit cleanly");
        } else {
            assert!(
                !status.success(),
                "killed lock owner must not report success"
            );
        }

        let after_now = if mode == "normal" {
            now + time::Duration::minutes(1)
        } else {
            now
        };
        let after_release = scheduler.poll(after_now, |_| fired += 1).unwrap();
        assert_eq!(fired, 1, "{mode} owner must not strand the occurrence");
        assert_eq!(after_release.len(), 1);
        assert!(matches!(
            &after_release[0],
            deepagent_runtime::schedule::FireOutcome::Fired { task_id } if task_id == &task.id
        ));
    }
}

#[cfg(windows)]
#[test]
#[ignore = "audit H-96: post-fire schedule bookkeeping failure must not replay the same occurrence"]
fn audit_post_fire_bookkeeping_failure_replays_same_occurrence() {
    let now = time::macros::datetime!(2026-10-21 12:00:00);

    for recurring in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let scheduler = CronScheduler::for_workspace(directory.path());
        let task = scheduler
            .store()
            .add(
                "* * * * *".into(),
                "audit post-fire bookkeeping only".into(),
                recurring,
                local_to_epoch_ms(now) - 120_000,
            )
            .unwrap();
        let schedule_path = scheduler.store().path().to_path_buf();
        let mut fire_count = 0;

        let first_poll = scheduler.poll(now, |_| {
            fire_count += 1;
            let mut permissions = std::fs::metadata(&schedule_path).unwrap().permissions();
            permissions.set_readonly(true);
            std::fs::set_permissions(&schedule_path, permissions).unwrap();
        });
        assert!(
            first_poll.is_err(),
            "post-fire bookkeeping should fail for recurring={recurring}"
        );
        assert_eq!(fire_count, 1);
        let pending = scheduler.store().read().unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].id, task.id);
        assert_eq!(pending[0].last_fired_at, None);
        drop(scheduler);

        let mut permissions = std::fs::metadata(&schedule_path).unwrap().permissions();
        permissions.set_readonly(false);
        std::fs::set_permissions(&schedule_path, permissions).unwrap();

        let scheduler = CronScheduler::for_workspace(directory.path());
        let retry = scheduler
            .poll(now, |_| {
                fire_count += 1;
            })
            .unwrap();
        assert_eq!(retry.len(), 1);
        assert_eq!(fire_count, 2, "the same occurrence fired again");
        if recurring {
            assert_eq!(
                scheduler.store().read().unwrap()[0].last_fired_at,
                Some(local_to_epoch_ms(now))
            );
        } else {
            assert!(scheduler.store().read().unwrap().is_empty());
        }
        eprintln!("recurring={recurring} fire_count={fire_count}");
    }
}
