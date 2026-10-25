//! Calendar-totality audit through the normal tool, store and scheduler.
//! All workspaces and prompts are synthetic; no shell or network is used.

use super::*;
use crate::{MemorySecretStore, SettingsService};
use deepagent_builtins::cron_tools::{CronCreateTool, CronDeleteTool};
use deepagent_models::transport::MockTransport;
use deepagent_persistence::Database;
use deepagent_runtime::schedule::cron::{epoch_ms_to_local, parse_5_field};
use deepagent_tools::Tool;
use std::sync::Mutex;
use std::time::Duration;

async fn check_calendar_isolation(expression: &str, real_service_clock: bool) {
    // Calendar-impossible expressions are syntactically accepted. No malformed
    // JSON, direct store edit, time-zone change or out-of-range field is needed.
    parse_5_field(expression).unwrap();
    let root = tempfile::tempdir().unwrap();
    let db = Arc::new(Database::open_in_memory().unwrap());
    let transport = Arc::new(MockTransport::new(Vec::<String>::new()));
    let settings = Arc::new(SettingsService::new(
        db.clone(),
        transport.clone(),
        Arc::new(MemorySecretStore::new()),
    ));
    let chat = Arc::new(ChatService::new(db, settings, transport, root.path()));
    let cron = Arc::new(CronService::new(root.path(), chat));
    let create = CronCreateTool::new(cron.clone());
    let mut ids = Vec::new();
    for (expr, prompt) in [
        (expression, "AUDIT_CALENDAR_CANDIDATE_271"),
        ("* * * * *", "AUDIT_CALENDAR_HEALTHY_271"),
    ] {
        let output = create
            .invoke(serde_json::json!({"cron": expr, "prompt": prompt, "recurring": true}))
            .await
            .unwrap();
        assert!(output.ok && output.value["scheduled"] == true);
        ids.push(output.value["id"].as_str().unwrap().to_owned());
    }
    assert_eq!(cron.list().await.unwrap().len(), 2);
    let before = std::fs::read(cron.scheduler.store().path()).unwrap();
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    let due_at = epoch_ms_to_local(now_ms).unwrap() + time::Duration::minutes(2);
    let shutdown = CancellationToken::new();
    let fired = Arc::new(Mutex::new(Vec::<String>::new()));
    let mut runner = if real_service_clock {
        let service = cron.clone();
        let stop = shutdown.clone();
        // Real AppCore tick: the impossible first row panics before the
        // healthy row, so the fire->ChatService callback is never entered.
        tokio::spawn(async move { service.run_tick_loop(stop).await })
    } else {
        let scheduler = cron.scheduler.clone();
        let stop = shutdown.clone();
        let stop_after_fire = shutdown.clone();
        let observed = fired.clone();
        let healthy_id = ids[1].clone();
        // Production runtime loop with an injected clock; callbacks only
        // record ids and stop after the healthy task has really been visited.
        tokio::spawn(async move {
            run_tick_loop(
                scheduler,
                Duration::from_millis(10),
                move || due_at,
                move |task| {
                    observed.lock().unwrap().push(task.id.clone());
                    if task.id == healthy_id {
                        stop_after_fire.cancel();
                    }
                },
                stop,
            )
            .await
        })
    };
    let joined = match tokio::time::timeout(Duration::from_secs(5), &mut runner).await {
        Ok(joined) => joined,
        Err(_) => {
            shutdown.cancel();
            runner.await
        }
    };
    let panicked = joined.as_ref().err().is_some_and(|e| e.is_panic());
    if let Ok(result) = &joined {
        assert!(
            result.is_ok(),
            "loop error is not a calendar panic: {result:?}"
        );
    }
    if panicked {
        assert_eq!(
            std::fs::read(cron.scheduler.store().path()).unwrap(),
            before
        );
    }
    let healthy_fired = fired.lock().unwrap().contains(&ids[1]);
    assert_eq!(cron.list().await.unwrap().len(), 2);

    // The stored input survives the panic. A fresh scheduler meets the same
    // failure, rather than treating a fresh owner as a recovery mechanism.
    let reopened = CronScheduler::for_workspace(root.path());
    let replay = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        reopened.poll(due_at, |_| {})
    }));
    let reopen_panicked = replay.is_err();
    if let Ok(result) = replay {
        result.unwrap();
    }

    let deleted = CronDeleteTool::new(cron.clone())
        .invoke(serde_json::json!({"operation": "delete", "task_ids": [ids[0]]}))
        .await
        .unwrap();
    assert!(deleted.ok && deleted.value["removed"] == 1);
    let mut recovery = Vec::new();
    reopened
        .poll(due_at, |task| recovery.push(task.id.clone()))
        .unwrap();
    if panicked {
        assert_eq!(recovery, vec![ids[1].clone()]);
    }
    eprintln!(
        "expr={expression:?} real_service_clock={real_service_clock} create_ok=true loop_panicked={panicked} healthy_fired={healthy_fired} reopen_panicked={reopen_panicked} after_delete_callbacks={}",
        recovery.len(),
    );
    assert!(
        !panicked && !reopen_panicked && (real_service_clock || healthy_fired),
        "a calendar-impossible schedule must not terminate the scheduler or block healthy tasks"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn valid_april_date_keeps_scheduler_healthy() {
    check_calendar_isolation("0 0 30 4 *", false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn valid_leap_day_keeps_scheduler_healthy() {
    check_calendar_isolation("0 0 29 2 *", false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "audit: a normal cron_create for February 31 must not panic the runtime loop"]
async fn audit_february_31_does_not_terminate_scheduler() {
    check_calendar_isolation("0 0 31 2 *", false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "audit: a normal cron_create for April 31 must not poison healthy schedules"]
async fn audit_april_31_does_not_terminate_scheduler() {
    check_calendar_isolation("0 0 31 4 *", false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "audit: calendar-impossible input must not panic the real AppCore tick task"]
async fn audit_impossible_date_does_not_terminate_appcore_tick() {
    check_calendar_isolation("0 0 31 2 *", true).await;
}
