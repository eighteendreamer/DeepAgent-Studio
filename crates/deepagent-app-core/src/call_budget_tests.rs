//! Measured-spend admission uses the same durable costs and run receipts as
//! settlement; no second in-memory budget ledger or synthetic provider price.

use super::*;
use crate::cost_service::{BudgetConfig, ModelPricing};
use deepagent_models::{ModelClient, ModelConfig};
use std::sync::atomic::{AtomicUsize, Ordering};

fn price(cost: &CostService) {
    cost.set_pricing(std::collections::HashMap::from([(
        "flash".into(),
        ModelPricing {
            input_cache_hit_per_million: 100.0,
            input_cache_miss_per_million: 1000.0,
            output_per_million: 0.0,
        },
    )]));
}

fn settle(sink: &Arc<OwnedCallUsage>, measured: Option<Usage>) {
    let mut observer = CallUsageObserver::new(sink.clone(), "flash".into(), false);
    observer.completed(measured, None);
    drop(observer);
    sink.checkpoint().unwrap();
}

#[test]
fn main_and_side_attempts_share_each_period_gate_but_not_the_turn_projection() {
    for monthly in [false, true] {
        let (owner, events) = setup();
        let cost = owner.cost.as_ref().unwrap();
        price(cost);
        cost.set_budget(BudgetConfig {
            daily_limit: (!monthly).then_some(0.56),
            monthly_limit: monthly.then_some(0.56),
        })
        .unwrap();
        let main = owner.main_loop();
        let side = owner.invocation("summary".into(), "compaction");
        main.before_call().unwrap();
        settle(&main, Some(usage())); // cache-aware CNY 0.28
        side.before_call().unwrap();
        settle(&side, Some(usage()));
        for sink in [&main, &side] {
            let error = sink.before_call().unwrap_err().to_string();
            assert!(error.contains(if monthly {
                "monthly budget exhausted"
            } else {
                "daily budget exhausted"
            }));
            // Reaching a limit is not a reason to reject the already-paid
            // final answer. Only a subsequent paid request is refused.
            sink.checkpoint().unwrap();
        }
        let summary = cost.summary(&owner.session_id).unwrap();
        assert_eq!(summary.session_cost, 0.56);
        assert_eq!(
            (summary.input_tokens, summary.cache_hit_tokens),
            (2000, 1600)
        );
        assert_eq!(
            CostStore::new(&owner.db)
                .session_costs_detailed(&owner.session_id)
                .unwrap()
                .len(),
            2
        );
        let main_receipts = main.main_receipts().unwrap();
        assert_eq!(main_receipts.len(), 1);
        assert_eq!(main_receipts[0].cost_yuan, Some(0.28));
        let notifications = events.0.lock().unwrap();
        assert_eq!(
            notifications.len(),
            1,
            "only the side call notifies immediately"
        );
        assert!(matches!(
            notifications[0],
            RuntimeEvent::CostRecorded {
                origin: CostOrigin::SideCall,
                cost_yuan: Some(0.28),
                ..
            }
        ));
    }
}

#[test]
fn incomplete_usage_or_price_is_not_zero_spend_when_a_budget_is_configured() {
    for configured in [false, true] {
        for reported in [false, true] {
            let (owner, events) = setup();
            let cost = owner.cost.as_ref().unwrap();
            // No price: a measured call still owns its unknown-cost row.
            if configured {
                cost.set_budget(BudgetConfig {
                    daily_limit: Some(3.0),
                    monthly_limit: None,
                })
                .unwrap();
            }
            let main = owner.main_loop();
            main.before_call().unwrap();
            settle(&main, reported.then(usage));
            let result = main.before_call();
            if configured {
                assert!(result
                    .unwrap_err()
                    .to_string()
                    .contains("budget cannot be verified"));
            } else {
                result.unwrap();
            }
            let receipts = main.main_receipts().unwrap();
            assert_eq!(receipts.len(), 1);
            assert_eq!(receipts[0].cost_yuan, None);
            assert_eq!(receipts[0].ledger_id.is_some(), reported);
            assert_eq!(
                CostStore::new(&owner.db)
                    .session_costs_detailed(&owner.session_id)
                    .unwrap()
                    .len(),
                usize::from(reported)
            );
            assert!(events.0.lock().unwrap().is_empty());
        }
    }
}

#[test]
fn zero_budget_blocks_the_first_request_without_creating_a_receipt() {
    for monthly in [false, true] {
        let (owner, _) = setup();
        owner
            .cost
            .as_ref()
            .unwrap()
            .set_budget(BudgetConfig {
                daily_limit: (!monthly).then_some(0.0),
                monthly_limit: monthly.then_some(0.0),
            })
            .unwrap();
        let main = owner.main_loop();
        assert!(main
            .before_call()
            .unwrap_err()
            .to_string()
            .contains("budget exhausted"));
        assert!(main.main_receipts().unwrap().is_empty());
    }
}

#[test]
fn failed_side_booking_cannot_be_hidden_from_a_new_main_sink() {
    let (owner, _) = setup();
    owner
        .cost
        .as_ref()
        .unwrap()
        .set_budget(BudgetConfig {
            daily_limit: Some(3.0),
            monthly_limit: None,
        })
        .unwrap();
    owner.db.with_conn(|conn| conn.execute_batch("CREATE TEMP TRIGGER fail_cost BEFORE INSERT ON costs BEGIN SELECT RAISE(FAIL,'controlled budget cost failure'); END;").map_err(|error| CoreError::Persistence(error.to_string()))).unwrap();
    let side = owner.invocation("hook".into(), "hook_prompt");
    let mut observer = CallUsageObserver::new(side.clone(), "flash".into(), false);
    observer.completed(Some(usage()), None);
    drop(observer);
    assert!(side
        .checkpoint()
        .unwrap_err()
        .to_string()
        .contains("controlled budget cost failure"));
    assert!(owner
        .main_loop()
        .before_call()
        .unwrap_err()
        .to_string()
        .contains("budget cannot be verified"));
    assert!(CostStore::new(&owner.db)
        .session_costs_detailed(&owner.session_id)
        .unwrap()
        .is_empty());
}

#[test]
fn paid_usage_survives_owner_reopen_and_limits_other_runs_and_connections() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("state.sqlite");
    let (mut owner, _) = setup();
    owner.db = Arc::new(Database::open(&path).unwrap());
    EventStore::new(&owner.db)
        .create_session(owner.session_id.parse().unwrap(), None, SystemClock.now())
        .unwrap();
    RunStore::new(&owner.db)
        .create("run", &owner.session_id, None, 0)
        .unwrap();
    owner.cost = Some(Arc::new(CostService::new(owner.db.clone())));
    let cost = owner.cost.as_ref().unwrap();
    price(cost);
    cost.set_budget(BudgetConfig {
        daily_limit: Some(0.28),
        monthly_limit: None,
    })
    .unwrap();
    settle(&owner.main_loop(), Some(usage()));
    let sid = owner.session_id.clone();
    drop(owner);
    let db = Arc::new(Database::open(&path).unwrap());
    let cost = Arc::new(CostService::new(db.clone()));
    let events: Arc<dyn RuntimeEventSink> = Arc::new(Events::default());
    let reopened = RunCallUsageOwner {
        db: db.clone(),
        cost: Some(cost.clone()),
        session_id: sid.clone(),
        run_id: "run".into(),
        events: Arc::downgrade(&events),
    };
    assert_eq!(
        reopened.main_loop().main_receipts().unwrap()[0].cost_yuan,
        Some(0.28)
    );
    RunStore::new(&db)
        .create("other-run", &sid, None, 0)
        .unwrap();
    let other = RunCallUsageOwner {
        run_id: "other-run".into(),
        ..reopened
    };
    assert!(other
        .main_loop()
        .before_call()
        .unwrap_err()
        .to_string()
        .contains("daily budget exhausted"));
    assert!(other.main_loop().main_receipts().unwrap().is_empty());
    assert_eq!(cost.summary(&sid).unwrap().session_cost, 0.28);
}

#[test]
fn a_live_run_observes_another_connections_committed_spend_before_its_next_call() {
    for monthly in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("state.sqlite");
        let db = Arc::new(Database::open(&path).unwrap());
        let session = SessionId::new();
        EventStore::new(&db)
            .create_session(session, None, SystemClock.now())
            .unwrap();
        for run in ["writer", "reader"] {
            RunStore::new(&db)
                .create(run, &session.to_string(), None, 0)
                .unwrap();
        }
        let cost = Arc::new(CostService::new(db.clone()));
        price(&cost);
        cost.set_budget(BudgetConfig {
            daily_limit: (!monthly).then_some(0.28),
            monthly_limit: monthly.then_some(0.28),
        })
        .unwrap();
        let events: Arc<dyn RuntimeEventSink> = Arc::new(Events::default());
        let writer = RunCallUsageOwner {
            db,
            cost: Some(cost),
            session_id: session.to_string(),
            run_id: "writer".into(),
            events: Arc::downgrade(&events),
        };
        let reader_db = Arc::new(Database::open(&path).unwrap());
        let reader = RunCallUsageOwner {
            cost: Some(Arc::new(CostService::new(reader_db.clone()))),
            db: reader_db,
            run_id: "reader".into(),
            ..writer.clone()
        };
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let (committed_tx, committed_rx) = std::sync::mpsc::channel();
        let waiter = std::thread::spawn(move || {
            let main = reader.main_loop();
            main.before_call().unwrap();
            ready_tx.send(()).unwrap();
            committed_rx
                .recv_timeout(std::time::Duration::from_secs(10))
                .unwrap();
            assert!(main
                .before_call()
                .unwrap_err()
                .to_string()
                .contains(if monthly {
                    "monthly budget exhausted"
                } else {
                    "daily budget exhausted"
                }));
            assert!(main.main_receipts().unwrap().is_empty());
        });
        ready_rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .unwrap();
        // No finalizer or owner recreation: the other run is still alive.
        let main = writer.main_loop();
        main.before_call().unwrap();
        settle(&main, Some(usage()));
        committed_tx.send(()).unwrap();
        waiter.join().unwrap();
        assert_eq!(main.main_receipts().unwrap().len(), 1);
    }
}

struct SummaryTransport {
    calls: AtomicUsize,
    overflow_first: bool,
}

#[async_trait::async_trait]
impl deepagent_models::HttpTransport for SummaryTransport {
    async fn stream(
        &self,
        _request: deepagent_models::TransportRequest,
        sink: &mut dyn deepagent_models::transport::EventSink,
    ) -> Result<()> {
        let sink: &mut dyn deepagent_models::transport::EventSink =
            &mut crate::test_sequence::sink(sink);
        let index = self.calls.fetch_add(1, Ordering::AcqRel);
        for event in crate::responses_fixture::completed(
            &format!("summary-{index}"),
            "flash",
            crate::responses_fixture::text("summary", r#"{"goal":"g","completed":["ok"]}"#),
            json!({"input_tokens":1000,"output_tokens":100,"total_tokens":1100,
                "input_tokens_details":{"cached_tokens":800}}),
        ) {
            if sink.on_event(&event)? {
                break;
            }
        }
        if self.overflow_first && index == 0 {
            return Err(CoreError::provider(
                Some(413),
                Some("context_length_exceeded".into()),
                "controlled summary overflow",
            ));
        }
        Ok(())
    }
}

#[tokio::test]
async fn paid_compaction_is_committed_before_the_parent_next_request() {
    let (owner, events) = setup();
    let cost = owner.cost.as_ref().unwrap();
    price(cost);
    cost.set_budget(BudgetConfig {
        daily_limit: Some(0.28),
        monthly_limit: None,
    })
    .unwrap();
    let transport = Arc::new(SummaryTransport {
        calls: AtomicUsize::new(0),
        overflow_first: false,
    });
    let compactor = deepagent_context::ModelCompactor::new(
        Arc::new(ModelClient::new(
            transport.clone(),
            ModelConfig::deepseek("offline-placeholder"),
        )),
        "flash",
    )
    .with_call_usage_sink(owner.invocation("summary".into(), "compaction"));
    let (summary, observed) = compactor
        .summarize_accounted_cancelled("g", &Default::default(), &["old turn".into()], None)
        .await
        .unwrap();
    assert_eq!(summary.goal, "g");
    assert_eq!(observed.prompt_tokens, 1000);
    assert_eq!(transport.calls.load(Ordering::Acquire), 1);
    assert_eq!(cost.summary(&owner.session_id).unwrap().session_cost, 0.28);
    assert!(owner
        .main_loop()
        .before_call()
        .unwrap_err()
        .to_string()
        .contains("daily budget exhausted"));
    assert_eq!(events.0.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn measured_summary_overflow_cannot_bypass_the_gate_on_its_ptl_retry() {
    for limited in [false, true] {
        let (owner, _) = setup();
        let cost = owner.cost.as_ref().unwrap();
        price(cost);
        if limited {
            cost.set_budget(BudgetConfig {
                daily_limit: Some(0.28),
                monthly_limit: None,
            })
            .unwrap();
        }
        let transport = Arc::new(SummaryTransport {
            calls: AtomicUsize::new(0),
            overflow_first: true,
        });
        let compactor = deepagent_context::ModelCompactor::new(
            Arc::new(ModelClient::new(
                transport.clone(),
                ModelConfig::deepseek("offline-placeholder"),
            )),
            "flash",
        )
        .with_call_usage_sink(owner.invocation("summary".into(), "compaction"));
        let result = compactor
            .summarize_accounted_cancelled(
                "g",
                &Default::default(),
                &["a".into(), "b".into()],
                None,
            )
            .await;
        if limited {
            let failed = result.unwrap_err();
            assert!(failed.error.to_string().contains("daily budget exhausted"));
            assert_eq!(failed.usage.prompt_tokens, 1000);
        } else {
            let (summary, observed) = result.unwrap();
            assert_eq!(summary.goal, "g");
            // Usage keeps the latest prompt/context gauge; only completion
            // tokens accumulate here. The durable ledger below bills both
            // physical prompts, including the failed first attempt.
            assert_eq!(observed.prompt_tokens, 1000);
            assert_eq!(observed.completion_tokens, 200);
        }
        assert_eq!(
            transport.calls.load(Ordering::Acquire),
            if limited { 1 } else { 2 }
        );
        assert_eq!(
            cost.summary(&owner.session_id).unwrap().input_tokens,
            if limited { 1000 } else { 2000 }
        );
    }
}
