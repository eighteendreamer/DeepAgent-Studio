//! H96 real child processes, synthetic SQLite/provider, never user data.
use super::delivery_tests::*;
use super::*;
use deepagent_persistence::run_store::RunStore;
use deepagent_runtime::schedule::cron::local_to_epoch_ms;
use std::process::{Command, Stdio};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

const CHILD: &str = "cron_service::process_tests::h96_process_child";
const ROOT: &str = "DEEPAGENT_H96_PROCESS_ROOT";
const PHASE: &str = "DEEPAGENT_H96_PROCESS_PHASE";

fn now() -> time::PrimitiveDateTime {
    time::macros::datetime!(2026-11-04 12:00:00)
}

fn park(root: &Path, run: &str) -> ! {
    std::fs::write(root.join("ready"), run).unwrap();
    loop {
        std::thread::sleep(Duration::from_secs(1));
    }
}

#[tokio::test]
#[ignore = "child entry, explicitly run by the default parent acceptance"]
async fn h96_process_child() {
    let root = std::path::PathBuf::from(std::env::var_os(ROOT).expect("parent fixture root"));
    let phase = std::env::var(PHASE).unwrap();
    let fixture = Fixture::open(&root).await;
    let scheduler = CronScheduler::for_workspace(&root);
    let mut prepared = None;
    let mut run_id = String::new();
    let result = scheduler.poll(now(), |task, occurrence| {
        if phase == "before" {
            park(&root, &occurrence.run_id);
        }
        run_id.clone_from(&occurrence.run_id);
        let (receipt, fresh) = fixture.chat.admit_scheduled(task, occurrence)?;
        prepared = fresh;
        if phase == "admitted" {
            park(&root, &occurrence.run_id);
        }
        set_readonly(scheduler.store().path(), true).unwrap();
        Ok(receipt)
    });
    assert!(result.is_err());
    fixture
        .chat
        .run_prepared(prepared.unwrap(), "H96 process control", |_| {}, |_| {})
        .await
        .unwrap();
    assert_eq!(fixture.transport.0.load(Ordering::SeqCst), 1);
    park(&root, &run_id);
}

struct Child(std::process::Child);
impl Drop for Child {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[tokio::test]
async fn h96_killed_primary_and_cold_standby_reconcile_without_reexecuting_admitted_runs() {
    for recurring in [false, true] {
        for phase in ["before", "admitted", "completed"] {
            let fixture = Fixture::new().await;
            let root = fixture.root.as_path();
            let scheduler = CronScheduler::for_workspace(root);
            let path = scheduler.store().path().to_path_buf();
            scheduler
                .store()
                .add(
                    "* * * * *".into(),
                    "H96 process control".into(),
                    recurring,
                    local_to_epoch_ms(now()) - 120_000,
                )
                .unwrap();
            let _restore = Restore(path.clone());
            let temp = root.join("child-temp");
            std::fs::create_dir(&temp).unwrap();
            let mut child = Child(
                Command::new(std::env::current_exe().unwrap())
                    .args(["--exact", CHILD, "--ignored", "--test-threads=1"])
                    .env(ROOT, root)
                    .env(PHASE, phase)
                    .env("TEMP", &temp)
                    .env("TMP", &temp)
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
                    .unwrap(),
            );
            let deadline = Instant::now() + Duration::from_secs(20);
            while !root.join("ready").exists() {
                assert!(
                    child.0.try_wait().unwrap().is_none(),
                    "child failed before {phase}"
                );
                assert!(Instant::now() < deadline, "child did not reach {phase}");
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            let run_id = std::fs::read_to_string(root.join("ready")).unwrap();
            // Before/admitted child is holding the real schedule lease. A
            // standby cannot start another model even with a different TEMP.
            let mut visits = 0;
            scheduler
                .poll(now(), |task, occurrence| {
                    visits += 1;
                    let (receipt, prepared) = fixture.chat.admit_scheduled(task, occurrence)?;
                    assert!(prepared.is_none());
                    Ok(receipt)
                })
                .unwrap_or_else(|error| {
                    assert_eq!(phase, "completed"); // read-only bookkeeping
                    assert!(matches!(
                        error,
                        deepagent_runtime::schedule::SchedulePollError::Bookkeeping(_)
                    ));
                    Vec::new()
                });
            assert_eq!(visits, usize::from(phase == "completed"));
            child.0.kill().unwrap();
            assert!(!child.0.wait().unwrap().success());
            set_readonly(&path, false).unwrap();

            let cold = Fixture::open(root).await;
            let app = crate::AppService::from_shared(cold.db.clone());
            let recovered = app.recover_unfinished_runs().unwrap();
            assert_eq!(recovered.len(), usize::from(phase == "admitted"));
            if phase == "admitted" {
                let run = RunStore::new(&cold.db).get(&run_id).unwrap().unwrap();
                assert_eq!(run.terminal_kind.as_deref(), Some("failed"));
                assert!(run
                    .terminal_reason
                    .as_deref()
                    .unwrap()
                    .contains("previous state"));
            }
            let mut fresh = None;
            CronScheduler::for_workspace(root)
                .poll(now(), |task, occurrence| {
                    assert_eq!(occurrence.run_id, run_id);
                    let (receipt, prepared) = cold.chat.admit_scheduled(task, occurrence)?;
                    fresh = prepared;
                    Ok(receipt)
                })
                .unwrap();
            assert_eq!(fresh.is_some(), phase == "before");
            if let Some(prepared) = fresh {
                cold.chat
                    .run_prepared(prepared, "H96 process control", |_| {}, |_| {})
                    .await
                    .unwrap();
            }
            assert_eq!(
                cold.transport.0.load(Ordering::SeqCst),
                usize::from(phase == "before")
            );
            let events = RunStore::new(&cold.db).events_after(&run_id, None).unwrap();
            for kind in ["run_accepted", "schedule.accepted", "run_terminal"] {
                assert_eq!(
                    events
                        .iter()
                        .filter(|event| event.event_type == kind)
                        .count(),
                    1,
                    "{phase}/{recurring}/{kind}"
                );
            }
            assert!(app.recover_unfinished_runs().unwrap().is_empty());
            eprintln!("H96 kill phase={phase} recurring={recurring}: one admission/terminal, cold model calls={}", cold.transport.0.load(Ordering::SeqCst));
        }
    }
}
