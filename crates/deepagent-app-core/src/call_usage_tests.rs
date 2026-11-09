use super::*;
use deepagent_core::id::SessionId;
use deepagent_models::{DeltaObserver, Usage};
use deepagent_persistence::event_store::EventStore;
use deepagent_runtime::usage::CallUsageObserver;
use serde_json::json;

#[path = "hook_usage_boundary_tests.rs"]
mod hook_boundaries;

#[path = "call_budget_tests.rs"]
mod budget_boundaries;

#[path = "side_call_usage_tests.rs"]
mod side_calls;

#[path = "chat_filter_usage_tests.rs"]
mod chat_filter;

#[path = "chat_interrupt_usage_tests.rs"]
mod chat_interrupt;

#[derive(Default)]
struct Events(Mutex<Vec<RuntimeEvent>>);
impl RuntimeEventSink for Events {
    fn emit(&self, event: RuntimeEvent) {
        self.0.lock().unwrap().push(event);
    }
}

fn setup() -> (RunCallUsageOwner, Arc<Events>) {
    let db = Arc::new(Database::open_in_memory().unwrap());
    let session = SessionId::new();
    EventStore::new(&db)
        .create_session(session, None, SystemClock.now())
        .unwrap();
    RunStore::new(&db)
        .create("run", &session.to_string(), None, 0)
        .unwrap();
    let events = Arc::new(Events::default());
    let sink: Arc<dyn RuntimeEventSink> = events.clone();
    let owner = RunCallUsageOwner {
        cost: Some(Arc::new(CostService::new(db.clone()))),
        db,
        session_id: session.to_string(),
        run_id: "run".into(),
        events: Arc::downgrade(&sink),
    };
    (owner, events)
}

fn usage() -> Usage {
    Usage {
        prompt_tokens: 1000,
        completion_tokens: 100,
        total_tokens: 1100,
        prompt_cache_hit_tokens: 800,
        prompt_cache_miss_tokens: 200,
        ..Default::default()
    }
}

#[test]
fn failed_cancelled_and_successful_attempts_share_ownership_without_raw_secrets() {
    let (owner, events) = setup();
    let sink = owner.invocation("hook-1".into(), "hook_agent");
    for model in ["flash", "pro"] {
        let mut observer = CallUsageObserver::new(sink.clone(), model.into(), false);
        observer.on_usage(usage(), &json!({"input_tokens":1000,"input_tokens_details":{"cached_tokens":800,"text":"secret-canary"},"output_tokens_details":{"reasoning_tokens":0},"secret":"secret-canary"}));
        observer.on_usage(usage(), &json!({"input_tokens":1000,"input_tokens_details":{"cached_tokens":800},"output_tokens_details":{"reasoning_tokens":0},"secret":"secret-canary"}));
        if model == "pro" {
            observer.completed(Some(usage()), None);
        }
        // flash models a measured failure/cancellation, with no completed().
    }
    sink.checkpoint().unwrap();
    let summary = owner
        .cost
        .as_ref()
        .unwrap()
        .summary(&owner.session_id)
        .unwrap();
    assert_eq!(
        (summary.input_tokens, summary.cache_hit_tokens),
        (2000, 1600)
    );
    let records = RunStore::new(&owner.db)
        .events_after("run", Some(0))
        .unwrap();
    assert_eq!(records.len(), 2);
    assert_ne!(records[0].data["call_id"], records[1].data["call_id"]);
    assert_eq!(records[0].data["report"]["model_id"], "flash");
    assert_eq!(records[1].data["report"]["model_id"], "pro");
    assert_eq!(
        records[0].data["report"]["raw_usage"]["input_tokens_details"]["cached_tokens"],
        800
    );
    assert!(!serde_json::to_string(&records)
        .unwrap()
        .contains("secret-canary"));
    let events = events.0.lock().unwrap();
    assert_eq!(events.len(), 2);
    assert!(events.iter().all(|event| matches!(
        event,
        RuntimeEvent::CostRecorded {
            origin: CostOrigin::SideCall,
            cost_yuan: None,
            ..
        }
    )));
}

#[test]
fn unreported_success_is_incomplete_without_a_zero_charge() {
    let (owner, events) = setup();
    let sink = owner.invocation("hook-1".into(), "hook_prompt");
    let mut observer = CallUsageObserver::new(sink.clone(), "flash".into(), false);
    observer.completed(None, None);
    drop(observer);
    sink.checkpoint().unwrap();
    assert!(CostStore::new(&owner.db)
        .session_costs_detailed(&owner.session_id)
        .unwrap()
        .is_empty());
    let records = RunStore::new(&owner.db)
        .events_after("run", Some(0))
        .unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].data["report"]["usage_complete"], false);
    assert!(records[0].data["ledger_id"].is_null());
    assert!(events.0.lock().unwrap().is_empty());
}

#[test]
fn failed_cost_write_has_explicit_failed_receipt_and_no_success_notification() {
    let (owner, events) = setup();
    owner.db.with_conn(|conn| conn.execute_batch("CREATE TEMP TRIGGER fail_cost BEFORE INSERT ON costs BEGIN SELECT RAISE(FAIL,'controlled cost failure'); END;").map_err(|e| CoreError::Persistence(e.to_string()))).unwrap();
    let sink = owner.invocation("hook-1".into(), "hook_prompt");
    let mut observer = CallUsageObserver::new(sink.clone(), "flash".into(), false);
    observer.completed(Some(usage()), None);
    drop(observer);
    assert!(sink
        .checkpoint()
        .unwrap_err()
        .to_string()
        .contains("controlled cost failure"));
    assert!(CostStore::new(&owner.db)
        .session_costs_detailed(&owner.session_id)
        .unwrap()
        .is_empty());
    let records = RunStore::new(&owner.db)
        .events_after("run", Some(0))
        .unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].event_type, "model_call_usage_failed");
    assert_eq!(records[0].data["report"]["usage"]["prompt_tokens"], 1000);
    assert!(events.0.lock().unwrap().is_empty());
}

#[test]
fn parallel_invocations_do_not_share_private_agent_usage_or_drop_receipts() {
    let (owner, events) = setup();
    std::thread::scope(|scope| {
        for index in 0..8 {
            let sink = owner.invocation(format!("hook-{index}"), "hook_agent");
            scope.spawn(move || {
                let mut observer = CallUsageObserver::new(sink.clone(), "flash".into(), false);
                observer.completed(Some(usage()), None);
                drop(observer);
                sink.checkpoint().unwrap();
            });
        }
    });
    assert_eq!(
        owner
            .cost
            .as_ref()
            .unwrap()
            .summary(&owner.session_id)
            .unwrap()
            .input_tokens,
        8000
    );
    let records = RunStore::new(&owner.db)
        .events_after("run", Some(0))
        .unwrap();
    assert_eq!(
        records
            .iter()
            .map(|r| r.data["call_id"].as_str().unwrap())
            .collect::<std::collections::HashSet<_>>()
            .len(),
        8
    );
    assert_eq!(events.0.lock().unwrap().len(), 8);
}

#[test]
fn owned_hook_rows_do_not_steal_the_next_legacy_turn_cost_after_reopen() {
    use crate::cost_service::ModelPricing;
    use deepagent_core::event::EventPayload;
    use deepagent_core::message::Message;
    use deepagent_session::Session;
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("state.sqlite");
    let db = Arc::new(Database::open(&path).unwrap());
    let cost = Arc::new(CostService::new(db.clone()));
    cost.set_pricing(std::collections::HashMap::from([(
        "flash".into(),
        ModelPricing {
            input_cache_hit_per_million: 1.0,
            input_cache_miss_per_million: 1.0,
            output_per_million: 1.0,
        },
    )]));
    let mut session = Session::create(&db, &SystemClock, Some("legacy replay")).unwrap();
    let sid = session.id().to_string();
    RunStore::new(&db).create("run", &sid, None, 1).unwrap();
    let events: Arc<dyn RuntimeEventSink> = Arc::new(Events::default());
    let owner = RunCallUsageOwner {
        db: db.clone(),
        cost: Some(cost.clone()),
        session_id: sid.clone(),
        run_id: "run".into(),
        events: Arc::downgrade(&events),
    };
    for (prompt, completion) in [(100, 10), (200, 20)] {
        session
            .append(EventPayload::MessageAppended {
                message: Message::user("legacy question"),
            })
            .unwrap();
        session
            .append(EventPayload::MessageAppended {
                message: Message::assistant("legacy answer"),
            })
            .unwrap();
        session
            .append(EventPayload::UsageRecorded {
                prompt_tokens: prompt,
                completion_tokens: completion,
                reasoning_tokens: 0,
                total_tokens: prompt + completion,
                prompt_cache_hit_tokens: 0,
                prompt_cache_miss_tokens: prompt,
                duration_ms: 1,
                raw_responses_usage: None,
            })
            .unwrap();
        if prompt == 200 {
            // Insert the owned side row BETWEEN the two legacy ledger rows.
            let sink = owner.invocation("hook".into(), "hook_prompt");
            let mut observer = CallUsageObserver::new(sink.clone(), "flash".into(), false);
            observer.completed(Some(usage()), None);
            drop(observer);
            sink.checkpoint().unwrap();
        }
        cost.record(cost_request_for(
            &sid,
            "flash",
            &RunUsage::from(Usage {
                prompt_tokens: prompt,
                completion_tokens: completion,
                total_tokens: prompt + completion,
                prompt_cache_miss_tokens: prompt,
                ..Default::default()
            }),
        ))
        .unwrap();
    }
    drop(session);
    drop(owner);
    drop(cost);
    drop(db);
    let service = crate::service::AppService::new(Database::open(&path).unwrap());
    let costs: Vec<_> = service
        .session_conversation(&sid)
        .unwrap()
        .into_iter()
        .filter_map(|message| message.usage.map(|usage| usage.cost_yuan))
        .collect();
    assert_eq!(costs, vec![Some(0.000110), Some(0.000220)]);
}

#[test]
fn no_configured_ledger_keeps_an_explicit_unbooked_receipt() {
    let (mut owner, events) = setup();
    owner.cost = None;
    let sink = owner.invocation("hook".into(), "hook_prompt");
    let mut observer = CallUsageObserver::new(sink.clone(), "flash".into(), false);
    observer.completed(Some(usage()), None);
    drop(observer);
    sink.checkpoint().unwrap();
    let records = RunStore::new(&owner.db)
        .events_after("run", Some(0))
        .unwrap();
    assert_eq!(records[0].data["report"]["ledger_available"], false);
    assert_eq!(records[0].data["report"]["usage"]["prompt_tokens"], 1000);
    assert!(records[0].data["ledger_id"].is_null());
    assert!(events.0.lock().unwrap().is_empty());
}
