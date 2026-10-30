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

struct ToolLoopUsageTransport {
    samples: [Value; 2],
    requests: Mutex<Vec<Value>>,
}

#[async_trait]
impl HttpTransport for ToolLoopUsageTransport {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        let body: Value = serde_json::from_str(&request.body)?;
        assert_eq!(body["model"], MODEL);
        let index = {
            let mut requests = self.requests.lock().unwrap();
            let index = requests.len();
            assert!(index < 2, "unexpected retry, fallback or extra model call");
            requests.push(body.clone());
            index
        };
        let item = if index == 0 {
            super::responses_fixture::call("usage-task-list", "task_list", json!({}))
        } else {
            let input = body["input"].as_array().unwrap();
            assert_eq!(
                input
                    .iter()
                    .filter(|item| {
                        item["type"] == "function_call_output"
                            && item["call_id"] == "usage-task-list"
                    })
                    .count(),
                1
            );
            text(
                "tool-loop-answer",
                "Offline main-loop usage audit complete.",
            )
        };
        emit_responses_events(
            sink,
            &completed(
                &format!("tool-loop-usage-{index}"),
                MODEL,
                item,
                self.samples[index].clone(),
            ),
        )
    }
}

fn assert_tool_loop_live_usage(live: &[Value], samples: &[Value; 2], with_details: bool) {
    let usages: Vec<_> = live
        .iter()
        .filter(|event| event["type"] == "usage")
        .collect();
    assert_eq!(usages.len(), 2);
    for (index, usage) in usages.iter().enumerate() {
        assert_eq!(usage["raw_responses_usage"], samples[index]);
        assert_eq!(usage["prompt_tokens"], samples[index]["input_tokens"]);
        assert_eq!(usage["completion_tokens"], samples[index]["output_tokens"]);
        assert_eq!(usage["total_tokens"], samples[index]["total_tokens"]);
        assert_eq!(
            usage["reasoning_tokens"],
            if with_details { [12, 25][index] } else { 0 }
        );
        assert_eq!(
            usage["prompt_cache_hit_tokens"],
            if with_details { [80, 250][index] } else { 0 }
        );
        assert_eq!(
            usage["prompt_cache_miss_tokens"],
            if with_details {
                [20, 50][index]
            } else {
                [100, 300][index]
            }
        );
    }
    let expected_cost = if with_details { 0.00101 } else { 0.002 };
    let costs: Vec<_> = live
        .iter()
        .filter(|event| event["type"] == "cost_recorded")
        .collect();
    assert_eq!(costs.len(), 1);
    assert_eq!(costs[0]["cost_yuan"], expected_cost);
}

fn assert_tool_loop_session_usage(
    db: &Database,
    session: &str,
    samples: &[Value; 2],
    with_details: bool,
) {
    use deepagent_core::event::EventPayload;
    use deepagent_persistence::event_store::EventStore;

    let events = EventStore::new(db)
        .load_session(session.parse().unwrap())
        .unwrap();
    let persisted: Vec<_> = events
        .iter()
        .filter_map(|event| match &event.payload {
            EventPayload::UsageRecorded {
                prompt_tokens,
                completion_tokens,
                reasoning_tokens,
                total_tokens,
                prompt_cache_hit_tokens,
                prompt_cache_miss_tokens,
                raw_responses_usage,
                ..
            } => Some((
                *prompt_tokens,
                *completion_tokens,
                *reasoning_tokens,
                *total_tokens,
                *prompt_cache_hit_tokens,
                *prompt_cache_miss_tokens,
                raw_responses_usage,
            )),
            _ => None,
        })
        .collect();
    assert_eq!(persisted.len(), 1);
    let (prompt, output, reasoning, total, hit, miss, raw) = persisted[0];
    assert_eq!(
        (prompt, output, reasoning, total, hit, miss),
        (
            300,
            50,
            if with_details { 37 } else { 0 },
            330,
            if with_details { 250 } else { 0 },
            if with_details { 50 } else { 300 }
        )
    );
    assert_eq!(raw.as_ref().unwrap(), &json!(samples));
}

fn assert_tool_loop_run_usage(
    events: &[deepagent_persistence::run_store::StoredRunEvent],
    samples: &[Value; 2],
) {
    let usages: Vec<_> = events
        .iter()
        .filter(|event| event.event_type == "usage")
        .collect();
    assert_eq!(usages.len(), 2);
    for (index, event) in usages.iter().enumerate() {
        assert_eq!(event.data["raw_responses_usage"], samples[index]);
    }
    let tools: Vec<_> = events
        .iter()
        .filter(|event| event.event_type == "tool_completed")
        .collect();
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].data["name"], "task_list");
    assert_eq!(tools[0].data["call_id"], "usage-task-list");
    assert_eq!(tools[0].data["ok"], true);
    let terminals: Vec<_> = events
        .iter()
        .filter(|event| event.event_type == "run_terminal")
        .collect();
    assert_eq!(terminals.len(), 1);
    assert_eq!(terminals[0].data["kind"], "succeeded");
}

async fn observe_tool_loop_usage(with_details: bool) {
    use deepagent_persistence::run_store::RunStore;

    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let path = root.path().join("state.sqlite");
    let db = Arc::new(Database::open(&path).unwrap());
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
    // Synthetic, deliberately distinct rates make bucket mistakes observable.
    cost.set_pricing(std::collections::HashMap::from([(
        MODEL.to_string(),
        ModelPricing {
            input_cache_hit_per_million: 1.0,
            input_cache_miss_per_million: 4.0,
            output_per_million: 8.0,
        },
    )]));
    let mut samples = [
        json!({"input_tokens":100,"output_tokens":20,"total_tokens":120}),
        json!({"input_tokens":300,"output_tokens":30,"total_tokens":330}),
    ];
    if with_details {
        for (sample, (cached, reasoning)) in samples.iter_mut().zip([(80, 12), (250, 25)]) {
            sample["input_tokens_details"] = json!({"cached_tokens":cached});
            sample["output_tokens_details"] = json!({"reasoning_tokens":reasoning});
        }
    }
    let transport = Arc::new(ToolLoopUsageTransport {
        samples: samples.clone(),
        requests: Mutex::new(Vec::new()),
    });
    let chat = ChatService::new(db.clone(), settings, transport.clone(), &workspace)
        .with_cost(cost.clone());
    let captured = Arc::new(Mutex::new(Vec::new()));
    let callback = captured.clone();
    let session = chat
        .run_in_session(
            "List tasks once, then briefly acknowledge completion.",
            None,
            None,
            None,
            Vec::new(),
            None,
            false,
            Some("main-tool-loop-usage"),
            move |event| {
                if matches!(
                    &event,
                    RuntimeEvent::Usage { .. } | RuntimeEvent::CostRecorded { .. }
                ) {
                    callback
                        .lock()
                        .unwrap()
                        .push(serde_json::to_value(event).unwrap());
                }
            },
            |_| {},
        )
        .await
        .unwrap();
    assert_eq!(transport.requests.lock().unwrap().len(), 2);
    assert_tool_loop_live_usage(&captured.lock().unwrap(), &samples, with_details);
    let before = RunStore::new(&db)
        .events_after("main-tool-loop-usage", None)
        .unwrap();
    assert_tool_loop_run_usage(&before, &samples);
    let expected_cost = if with_details { 0.00101 } else { 0.002 };
    let summary = cost.summary(&session).unwrap();
    assert_eq!((summary.input_tokens, summary.output_tokens), (400, 50));
    assert_eq!(summary.cache_hit_tokens, if with_details { 330 } else { 0 });
    assert_eq!(
        summary.cache_miss_tokens,
        if with_details { 70 } else { 400 }
    );
    assert_eq!(summary.session_cost, expected_cost);
    assert_eq!(
        CostStore::new(&db)
            .session_costs_detailed(&session)
            .unwrap()
            .len(),
        1
    );
    drop(chat);
    drop(cost);
    drop(transport);
    drop(db);
    let reopened = Database::open(&path).unwrap();
    assert_eq!(
        RunStore::new(&reopened)
            .events_after("main-tool-loop-usage", None)
            .unwrap(),
        before
    );
    assert_tool_loop_session_usage(&reopened, &session, &samples, with_details);
    let cold_cost = CostService::new(Arc::new(reopened));
    assert_eq!(cold_cost.summary(&session).unwrap(), summary);
    eprintln!("main tool-loop usage: details={with_details}, requests=2, live_usage=2, durable_usage=2, raw_samples=2, ledger_rows=1, billed_input=400, output=50, cost={expected_cost}, file_reopen=equal");
}

#[tokio::test]
async fn main_tool_loop_preserves_reasoning_and_cache_usage_without_double_charge() {
    observe_tool_loop_usage(true).await;
}

#[tokio::test]
async fn main_tool_loop_defaults_absent_usage_details_without_losing_raw_samples() {
    observe_tool_loop_usage(false).await;
}

#[tokio::test]
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
    assert_eq!(observed.usage_events, 0);
    assert!(observed.costs.is_empty());
    assert_eq!(observed.summary.session_cost, 0.0);
}

#[tokio::test]
#[ignore = "audit H-274: provider token counts wider than the host Usage type must not be truncated"]
async fn audit_oversized_usage_counts_are_rejected_before_cost_accounting() {
    let oversized_input = u64::from(u32::MAX) + 1;
    let observed = observe(json!({
        "input_tokens": oversized_input,
        "input_tokens_details": {"cached_tokens": 0},
        "output_tokens": 1,
        "total_tokens": oversized_input + 1
    }))
    .await;
    eprintln!("usage contract oversized response: {observed:?}");
    assert!(
        observed.result_error.is_some(),
        "provider usage wider than host u32 was silently truncated: {observed:?}"
    );
    assert_eq!(observed.calls, 1);
    assert_eq!(observed.ledger_rows, 0);
    assert!(observed.costs.is_empty());
}
