//! H271: creation rejection and legacy calendar isolation through real owners.
//! Only synthetic workspaces, file SQLite and an offline model transport.

use super::*;
use deepagent_builtins::cron_tools::{CronCreateTool, CronDeleteTool};
use deepagent_persistence::run_store::RunStore;
use deepagent_runtime::schedule::cron::{epoch_ms_to_local, local_to_epoch_ms, parse_5_field};
use deepagent_runtime::schedule::ScheduledTask;
use deepagent_tools::Tool;
use std::sync::atomic::Ordering;
use std::sync::Mutex;
use std::time::Duration;

async fn candidate_task(cron: &Arc<CronService>, expression: &str, valid: bool) -> ScheduledTask {
    parse_5_field(expression).unwrap();
    let output = CronCreateTool::new(cron.clone())
        .invoke(serde_json::json!({
            "cron": expression, "prompt": "H271 synthetic candidate", "recurring": true
        }))
        .await
        .unwrap();
    if valid {
        assert!(output.ok && output.value["scheduled"] == true);
        cron.scheduler.store().read().unwrap().pop().unwrap()
    } else {
        assert!(!output.ok);
        assert!(output.value["error"]
            .as_str()
            .unwrap()
            .contains("no_calendar_match_in_supported_range"));
        assert!(!cron.scheduler.store().path().exists());
        // Deliberately the old-version persistence route, not a claim that the
        // new production tool still accepts the impossible plan.
        ScheduledTask {
            id: "h271bad1".into(),
            cron: expression.into(),
            prompt: "H271 synthetic legacy candidate".into(),
            created_at: 0,
            last_fired_at: None,
            recurring: Some(true),
            permanent: None,
        }
    }
}

async fn healthy_task(cron: &Arc<CronService>) -> ScheduledTask {
    let output = CronCreateTool::new(cron.clone())
        .invoke(serde_json::json!({
            "cron": "* * * * *", "prompt": "H271 synthetic healthy", "recurring": true
        }))
        .await
        .unwrap();
    assert!(output.ok && output.value["scheduled"] == true);
    cron.scheduler
        .store()
        .read()
        .unwrap()
        .into_iter()
        .find(|task| task.id == output.value["id"].as_str().unwrap())
        .unwrap()
}

async fn check_calendar_isolation(expression: &str, valid: bool) {
    // Both file orders must work: a late bad row must not roll back healthy
    // bookkeeping, and an early one must not prevent visiting healthy rows.
    for candidate_first in [true, false] {
        let fixture = delivery_tests::Fixture::new().await;
        let cron = Arc::new(CronService::new(&fixture.root, fixture.chat.clone()));
        let mut candidate = candidate_task(&cron, expression, valid).await;
        let mut healthy = healthy_task(&cron).await;
        let now = time::macros::datetime!(2026-11-05 12:00);
        let now_ms = local_to_epoch_ms(now);
        candidate.created_at = now_ms - 120_000;
        healthy.created_at = candidate.created_at;
        let mut tasks = vec![candidate.clone(), healthy.clone()];
        if !candidate_first {
            tasks.reverse();
        }
        cron.scheduler.store().write(&tasks).unwrap();
        let shutdown = CancellationToken::new();
        let fired = Arc::new(Mutex::new(Vec::new()));
        let observed = fired.clone();
        let scheduler = cron.scheduler.clone();
        let stop = shutdown.clone();
        let stop_after_fire = shutdown.clone();
        let mut worker = tokio::spawn(async move {
            run_tick_loop(
                scheduler,
                Duration::from_millis(10),
                move || now,
                move |task, occurrence| {
                    observed.lock().unwrap().push(task.id.clone());
                    stop_after_fire.cancel();
                    Ok(deepagent_runtime::schedule::DispatchReceipt {
                        accepted_at_ms: occurrence.polled_at_ms,
                    })
                },
                stop,
            )
            .await
        });
        let completed = tokio::time::timeout(Duration::from_secs(5), &mut worker).await;
        shutdown.cancel();
        match completed {
            Ok(result) => result.unwrap().unwrap(),
            Err(error) => {
                worker.await.unwrap().unwrap();
                panic!("healthy callback must run: {error}");
            }
        }
        assert_eq!(*fired.lock().unwrap(), vec![healthy.id.clone()]);
        let reopened = CronScheduler::for_workspace(&fixture.root);
        let stored = reopened.store().read().unwrap();
        assert_eq!(stored.len(), 2);
        assert_eq!(
            stored.iter().find(|t| t.id == candidate.id),
            Some(&candidate)
        );
        assert_eq!(
            stored
                .iter()
                .find(|t| t.id == healthy.id)
                .unwrap()
                .last_fired_at,
            Some(now_ms)
        );
        assert!(reopened
            .poll(now, |_, _| panic!("same slot must not dispatch again"))
            .unwrap()
            .is_empty());
        let deleted = CronDeleteTool::new(cron.clone())
            .invoke(serde_json::json!({"operation": "delete", "task_ids": [candidate.id]}))
            .await
            .unwrap();
        assert!(deleted.ok && deleted.value["removed"] == 1);
        assert_eq!(cron.list().await.unwrap().len(), 1);
        eprintln!("expr={expression:?} new_create_accepted={valid} candidate_first={candidate_first} healthy_callbacks=1 reopen_callbacks=0 legacy_retained=true delete_ok=true");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn valid_april_date_keeps_scheduler_healthy() {
    check_calendar_isolation("0 0 30 4 *", true).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn valid_leap_day_keeps_scheduler_healthy() {
    check_calendar_isolation("0 0 29 2 *", true).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn audit_february_31_does_not_terminate_scheduler() {
    check_calendar_isolation("0 0 31 2 *", false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn audit_april_31_does_not_terminate_scheduler() {
    check_calendar_isolation("0 0 31 4 *", false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn audit_impossible_date_does_not_terminate_appcore_tick() {
    let root = tempfile::tempdir().unwrap();
    let fixture = delivery_tests::Fixture::open(root.path()).await;
    let cron = Arc::new(CronService::new(root.path(), fixture.chat.clone()));
    let mut candidate = candidate_task(&cron, "0 0 31 2 *", false).await;
    let mut healthy = healthy_task(&cron).await;
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    candidate.created_at = now_ms - 120_000;
    healthy.created_at = candidate.created_at;
    cron.scheduler
        .store()
        .write(&[candidate.clone(), healthy.clone()])
        .unwrap();
    let shutdown = CancellationToken::new();
    let owner = cron.clone();
    let stop = shutdown.clone();
    let worker = tokio::spawn(async move { owner.run_tick_loop(stop).await });
    let completed = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let done = fixture
                .db
                .with_conn(|connection| {
                    connection
                        .query_row(
                            "SELECT count(*) FROM runs WHERE finished_at IS NOT NULL",
                            [],
                            |row| row.get::<_, usize>(0),
                        )
                        .map_err(|error| deepagent_core::CoreError::Persistence(error.to_string()))
                })
                .unwrap();
            if done == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    let loop_alive = !worker.is_finished();
    shutdown.cancel();
    worker.await.unwrap().unwrap();
    completed.expect("real healthy Kernel run must reach durable terminal");
    assert!(loop_alive);
    assert_eq!(fixture.transport.0.load(Ordering::SeqCst), 1);
    let stored = cron.scheduler.store().read().unwrap();
    assert_eq!(
        stored.iter().find(|t| t.id == candidate.id),
        Some(&candidate)
    );
    let accepted_at = stored
        .iter()
        .find(|t| t.id == healthy.id)
        .unwrap()
        .last_fired_at
        .unwrap();
    drop(cron);
    drop(fixture);

    // Close and reopen the actual file database and ChatService. The accepted
    // slot remains accounted for; the legacy input is neither lost nor retried.
    let reopened = delivery_tests::Fixture::open(root.path()).await;
    let cron = CronService::new(root.path(), reopened.chat.clone());
    assert!(cron
        .scheduler
        .poll(epoch_ms_to_local(accepted_at).unwrap(), |_, _| {
            panic!("reopened same slot must not enter admission")
        })
        .unwrap()
        .is_empty());
    assert_eq!(reopened.transport.0.load(Ordering::SeqCst), 0);
    let ids = reopened
        .db
        .with_conn(|connection| {
            let mut query = connection
                .prepare("SELECT id FROM runs")
                .map_err(|error| deepagent_core::CoreError::Persistence(error.to_string()))?;
            let ids = query
                .query_map([], |row| row.get::<_, String>(0))
                .and_then(|rows| rows.collect::<std::result::Result<Vec<_>, _>>())
                .map_err(|error| deepagent_core::CoreError::Persistence(error.to_string()))?;
            Ok(ids)
        })
        .unwrap();
    assert_eq!(ids.len(), 1);
    let ledger = RunStore::new(&reopened.db);
    assert_eq!(
        ledger
            .get(&ids[0])
            .unwrap()
            .unwrap()
            .terminal_kind
            .as_deref(),
        Some("succeeded")
    );
    let events = ledger.events_after(&ids[0], None).unwrap();
    for kind in ["run_accepted", "schedule.accepted", "run_terminal"] {
        assert_eq!(events.iter().filter(|e| e.event_type == kind).count(), 1);
    }
    assert_eq!(cron.scheduler.store().read().unwrap().len(), 2);
    eprintln!("new_impossible_create_rejected=true legacy_retained=true actual_kernel_requests=1 loop_alive_before_shutdown=true file_sqlite_reopen=true same_slot_requests=0 terminals=1");
}
