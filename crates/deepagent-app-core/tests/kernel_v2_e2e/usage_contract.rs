//! Responses usage invariants at the provider-to-cost boundary.

use super::responses_fixture::{completed, text};
use super::*;
use deepagent_app_core::{CostSummary, ModelPricing};
use deepagent_persistence::cost_store::CostStore;
use deepagent_runtime::events::RuntimeEvent;
use serde_json::{json, Value};

const MODEL: &str = "deepseek-flash";

#[derive(Debug)]
struct UsageTransport {
    usage: Value,
    calls: AtomicUsize,
}

#[async_trait]
impl HttpTransport for UsageTransport {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        let body: Value = serde_json::from_str(&request.body)?;
        assert_eq!(body["model"], MODEL);
        self.calls.fetch_add(1, Ordering::SeqCst);
        emit_responses_events(
            sink,
            &completed(
                "usage-contract-response",
                MODEL,
                text("usage-contract-answer", "offline usage contract audit"),
                self.usage.clone(),
            ),
        )
    }
}

#[derive(Debug)]
struct Observation {
    result_error: Option<String>,
    summary: CostSummary,
    ledger_rows: usize,
    usage_events: usize,
    costs: Vec<Option<f64>>,
    calls: usize,
}

async fn observe(usage: Value) -> Observation {
    let root = tempfile::tempdir().unwrap();
    let db = Arc::new(Database::open(root.path().join("state.sqlite")).unwrap());
    let settings = Arc::new(SettingsService::new(
        db.clone(),
        Arc::new(MockTransport::with_get_json(
            r#"{"data":[{"id":"deepseek-flash","object":"model","owned_by":"deepseek"}]}"#,
        )),
        Arc::new(MemorySecretStore::new()),
    ));
    settings
        .initialize("offline-audit-placeholder")
        .await
        .unwrap();
    let cost = Arc::new(CostService::new(db.clone()));
    cost.set_pricing(std::collections::HashMap::from([(
        MODEL.to_string(),
        ModelPricing {
            input_cache_hit_per_million: 100.0,
            input_cache_miss_per_million: 1000.0,
            output_per_million: 0.0,
        },
    )]));
    let transport = Arc::new(UsageTransport {
        usage,
        calls: AtomicUsize::new(0),
    });
    let chat = ChatService::new(db.clone(), settings, transport.clone(), root.path())
        .with_cost(cost.clone());
    let session = chat.create_session(None).unwrap();
    let usage_events = Arc::new(AtomicUsize::new(0));
    let costs = Arc::new(Mutex::new(Vec::new()));
    let usage_counter = usage_events.clone();
    let captured_costs = costs.clone();
    let result_error = chat
        .run_in_session(
            "Reply once.",
            Some(&session),
            None,
            None,
            Vec::new(),
            None,
            false,
            Some("audit-usage-contract"),
            move |event| match event {
                RuntimeEvent::Usage { .. } => {
                    usage_counter.fetch_add(1, Ordering::SeqCst);
                }
                RuntimeEvent::CostRecorded { cost_yuan, .. } => {
                    captured_costs.lock().unwrap().push(cost_yuan);
                }
                _ => {}
            },
            |_| {},
        )
        .await
        .err()
        .map(|error| error.to_string());
    let captured_cost_values = costs.lock().unwrap().clone();
    Observation {
        result_error,
        summary: cost.summary(&session).unwrap(),
        ledger_rows: CostStore::new(&db)
            .session_costs_detailed(&session)
            .unwrap()
            .len(),
        usage_events: usage_events.load(Ordering::SeqCst),
        costs: captured_cost_values,
        calls: transport.calls.load(Ordering::SeqCst),
    }
}

#[tokio::test]
async fn consistent_cached_usage_reaches_the_existing_cost_owner() {
    let observed = observe(json!({
        "input_tokens": 3,
        "input_tokens_details": {"cached_tokens": 2},
        "output_tokens": 0,
        "total_tokens": 3
    }))
    .await;
    eprintln!("usage contract control: {observed:?}");
    assert!(observed.result_error.is_none());
    assert_eq!(observed.calls, 1);
    assert_eq!(observed.summary.input_tokens, 3);
    assert_eq!(observed.summary.cache_hit_tokens, 2);
    assert_eq!(observed.summary.cache_miss_tokens, 1);
    assert_eq!(observed.summary.session_cost, 0.0012);
    assert_eq!(observed.ledger_rows, 1);
    assert_eq!(observed.costs, [Some(0.0012)]);
    assert_eq!(observed.usage_events, 1);
}

#[tokio::test]
#[ignore = "audit H-273: cached_tokens greater than input_tokens must not be accepted as billable usage"]
async fn audit_inconsistent_cached_usage_is_rejected_before_cost_accounting() {
    let observed = observe(json!({
        "input_tokens": 3,
        "input_tokens_details": {"cached_tokens": 8},
        "output_tokens": 0,
        "total_tokens": 3
    }))
    .await;
    eprintln!("usage contract malformed response: {observed:?}");
    assert!(
        observed.result_error.is_some(),
        "provider usage with cached_tokens > input_tokens was accepted: {observed:?}"
    );
    assert_eq!(observed.calls, 1);
    assert_eq!(observed.ledger_rows, 0);
    assert!(observed.costs.is_empty());
}
