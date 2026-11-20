//! Failed Responses accounting through the real chat/kernel/file-SQLite boundary.

use super::*;
use deepagent_app_core::{BudgetConfig, ModelPricing};
use deepagent_core::{event::EventPayload, id::SessionId};
use deepagent_persistence::{cost_store::CostStore, event_store::EventStore, run_store::RunStore};
use deepagent_runtime::events::RuntimeEvent;
use serde_json::{json, Value};
use std::str::FromStr;

const MODEL: &str = "deepseek-flash";
const RUN: &str = "h133-failed-response";

struct TerminalTransport {
    events: Vec<String>,
    calls: AtomicUsize,
}

#[async_trait]
impl HttpTransport for TerminalTransport {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        let body: Value = serde_json::from_str(&request.body)?;
        assert_eq!(body["model"], MODEL);
        assert_eq!(
            self.calls.fetch_add(1, Ordering::SeqCst),
            0,
            "unexpected retry"
        );
        emit_responses_events(sink, &self.events)
    }
}

fn usage() -> Value {
    json!({"input_tokens":9,"output_tokens":2,"total_tokens":11,
        "input_tokens_details":{"cached_tokens":4},
        "output_tokens_details":{"reasoning_tokens":1}})
}

async fn observe(sample: Option<Value>, success: bool, limit: Option<f64>) {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("state.sqlite");
    let db = Arc::new(Database::open(&path).unwrap());
    let settings = Arc::new(SettingsService::new(
        db.clone(),
        Arc::new(MockTransport::with_get_json(
            r#"{"data":[{"id":"deepseek-flash","object":"model","owned_by":"deepseek"}]}"#,
        )),
        Arc::new(MemorySecretStore::new()),
    ));
    crate::registry_fixture::configure(&settings, "offline-audit-placeholder")
        .await
        .unwrap();
    let cost = Arc::new(CostService::new(db.clone()));
    // Synthetic rates; the complete fixture costs exactly CNY 0.00544.
    cost.set_pricing(std::collections::HashMap::from([(
        MODEL.into(),
        ModelPricing {
            input_cache_hit_per_million: 100.0,
            input_cache_miss_per_million: 1000.0,
            output_per_million: 20.0,
        },
    )]));
    cost.set_budget(BudgetConfig {
        daily_limit: limit,
        monthly_limit: None,
    })
    .unwrap();
    let measured = sample.as_ref().is_some_and(|value| !value.is_null());
    let events = if success {
        super::responses_fixture::completed(
            "h133-response",
            MODEL,
            super::responses_fixture::text("h133-answer", "offline complete"),
            sample.clone().unwrap_or(Value::Null),
        )
    } else {
        let mut response = json!({"id":"h133-response","object":"response",
            "created_at":1,"model":MODEL,"status":"failed","output":[],
            "error":{"code":"audit_failed","message":"controlled failure"}});
        if let Some(sample) = sample {
            response["usage"] = sample;
        }
        vec![
            json!({"type":"response.created","sequence_number":0,
            "response":{"id":"h133-response","status":"in_progress","output":[]}})
            .to_string(),
            json!({"type":"response.failed","sequence_number":1,"response":response}).to_string(),
        ]
    };
    let transport = Arc::new(TerminalTransport {
        events,
        calls: AtomicUsize::new(0),
    });
    let chat = ChatService::new(db.clone(), settings, transport.clone(), root.path())
        .with_cost(cost.clone());
    let session = chat.create_session(None).unwrap();
    let live = Arc::new(Mutex::new(Vec::new()));
    let callback = live.clone();
    let result = chat
        .run_in_session(
            "Reply once.",
            Some(&session),
            None,
            None,
            Vec::new(),
            None,
            false,
            Some(RUN),
            move |event| {
                if matches!(&event, RuntimeEvent::Usage { .. }) {
                    callback
                        .lock()
                        .unwrap()
                        .push(serde_json::to_value(event).unwrap());
                }
            },
            |_| {},
        )
        .await;
    if success {
        assert!(result.is_ok(), "{result:?}");
    } else {
        assert!(result.unwrap_err().to_string().contains("audit_failed"));
    }
    assert_eq!(transport.calls.load(Ordering::SeqCst), 1);
    let live = live.lock().unwrap().clone();
    assert_eq!(live.len(), usize::from(measured));
    if measured {
        assert_eq!(live[0]["prompt_tokens"], 9);
        assert_eq!(live[0]["completion_tokens"], 2);
        assert_eq!(live[0]["reasoning_tokens"], 1);
        assert_eq!(live[0]["raw_responses_usage"], usage());
    }
    let receipts = CostStore::new(&db)
        .run_call_receipts(&session, RUN)
        .unwrap();
    assert_eq!(receipts.len(), usize::from(measured || success));
    if measured {
        assert!(receipts[0].ledger_id.is_some());
        assert_eq!(receipts[0].report["model_id"], MODEL);
        assert_eq!(receipts[0].report["usage"]["reasoning_tokens"], 1);
        assert_eq!(receipts[0].report["raw_usage"], usage());
        assert_eq!(receipts[0].report["usage_complete"], true);
    } else if success {
        assert!(receipts[0].ledger_id.is_none());
        assert_eq!(receipts[0].report["usage_complete"], false);
    }
    let summary = cost.summary(&session).unwrap();
    assert_eq!(
        (summary.input_tokens, summary.output_tokens),
        if measured { (9, 2) } else { (0, 0) }
    );
    assert_eq!(
        (summary.cache_hit_tokens, summary.cache_miss_tokens),
        if measured { (4, 5) } else { (0, 0) }
    );
    assert!((summary.session_cost - if measured { 0.00544 } else { 0.0 }).abs() < 1e-12);
    let rows = CostStore::new(&db)
        .session_costs_detailed(&session)
        .unwrap();
    assert_eq!(
        rows.len(),
        usize::from(measured),
        "finalizer must not charge twice"
    );
    let run_events = chat.run_events(RUN, None).unwrap();
    assert!(run_events
        .windows(2)
        .all(|pair| pair[0].sequence < pair[1].sequence));
    let terminals: Vec<_> = run_events
        .iter()
        .filter(|event| event.event_type == "run_terminal")
        .collect();
    assert_eq!(terminals.len(), 1);
    assert_eq!(
        terminals[0].data["kind"],
        if success { "succeeded" } else { "failed" }
    );
    if measured {
        let receipt_event = run_events
            .iter()
            .find(|event| event.event_type == "model_call_usage")
            .unwrap();
        assert!(receipt_event.sequence < terminals[0].sequence);
    }
    // Continuation reads the same measured spending, not only successful answers.
    if limit.is_some() {
        assert!(cost.check_budget().is_err());
        let next = chat
            .run_in_session(
                "Reply once.",
                Some(&session),
                None,
                None,
                Vec::new(),
                None,
                false,
                Some("h133-followup"),
                |_| {},
                |_| {},
            )
            .await;
        assert!(next.is_err());
        assert_eq!(transport.calls.load(Ordering::SeqCst), 1);
    }
    let history = EventStore::new(&db)
        .load_session(SessionId::from_str(&session).unwrap())
        .unwrap();
    let recorded: Vec<_> = history
        .iter()
        .filter_map(|event| match &event.payload {
            EventPayload::UsageRecorded {
                reasoning_tokens,
                raw_responses_usage,
                ..
            } if raw_responses_usage.is_some() => {
                Some((*reasoning_tokens, raw_responses_usage.clone()))
            }
            _ => None,
        })
        .collect();
    assert_eq!(recorded.len(), usize::from(measured));
    if measured {
        assert_eq!(recorded[0], (1, Some(json!([usage()]))));
    }
    drop(chat);
    drop(cost);
    drop(db);
    let reopened = Arc::new(Database::open(&path).unwrap());
    assert_eq!(
        RunStore::new(&reopened).events_after(RUN, None).unwrap(),
        run_events
    );
    assert_eq!(
        EventStore::new(&reopened)
            .load_session(SessionId::from_str(&session).unwrap())
            .unwrap(),
        history
    );
    assert_eq!(
        CostStore::new(&reopened)
            .session_costs_detailed(&session)
            .unwrap(),
        rows
    );
    let cold_receipts = CostStore::new(&reopened)
        .run_call_receipts(&session, RUN)
        .unwrap();
    assert_eq!(
        serde_json::to_value(cold_receipts).unwrap(),
        serde_json::to_value(receipts).unwrap()
    );
    assert_eq!(
        CostService::new(reopened).summary(&session).unwrap(),
        summary
    );
}

#[tokio::test]
async fn failed_usage_is_durable_once_and_blocks_the_next_paid_run() {
    observe(Some(usage()), false, Some(0.001)).await;
}

#[tokio::test]
async fn failed_missing_and_null_usage_do_not_fabricate_a_charge() {
    for sample in [None, Some(Value::Null)] {
        observe(sample, false, None).await;
    }
}

#[tokio::test]
async fn completed_usage_and_unreported_success_keep_the_existing_contract() {
    observe(Some(usage()), true, None).await;
    observe(Some(Value::Null), true, None).await;
}
