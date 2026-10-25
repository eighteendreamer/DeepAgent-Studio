//! Offline measured-usage budget probes; prices are synthetic, not provider quotes.

use super::responses_fixture::{call, completed, text};
use super::*;
use deepagent_app_core::cost_service::CostRecordRequest;
use deepagent_app_core::{BudgetConfig, CostSummary, ModelPricing};
use deepagent_persistence::cost_store::CostStore;
use deepagent_runtime::events::RuntimeEvent;
use serde_json::{json, Value};

const MODEL: &str = "deepseek-flash";
const RUN: &str = "audit-inflight-budget";

#[derive(Debug)]
struct Dispatch {
    prior_usage_events: usize,
    ledger_cost: f64,
    budget_allowed: bool,
    tool_output_paired: bool,
}

struct BudgetLoopModel {
    cost: Arc<CostService>,
    usage_events: Arc<AtomicUsize>,
    dispatches: Mutex<Vec<Dispatch>>,
}

#[async_trait]
impl HttpTransport for BudgetLoopModel {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        let body: Value = serde_json::from_str(&request.body)?;
        assert_eq!(body["model"], MODEL);
        let index = {
            let mut dispatches = self.dispatches.lock().unwrap();
            let index = dispatches.len();
            assert!(
                index < 2,
                "unexpected retry, fallback or extra model request"
            );
            let input = body["input"].as_array().unwrap();
            let tool_output_paired = input.iter().any(|item| {
                item["type"] == "function_call_output" && item["call_id"] == "budget-list"
            }) && input.iter().any(|item| {
                item["type"] == "function_call"
                    && item["call_id"] == "budget-list"
                    && item["name"] == "task_list"
            });
            dispatches.push(Dispatch {
                prior_usage_events: self.usage_events.load(Ordering::SeqCst),
                ledger_cost: self.cost.summary("")?.total_cost,
                budget_allowed: self.cost.check_budget().is_ok(),
                tool_output_paired,
            });
            index
        };
        let item = if index == 0 {
            call("budget-list", "task_list", json!({}))
        } else {
            text("budget-answer", "Offline budget audit complete.")
        };
        emit_responses_events(
            sink,
            &completed(
                &format!("budget-response-{index}"),
                MODEL,
                item,
                json!({"input_tokens":1000,"output_tokens":10,"total_tokens":1010,
                    "input_tokens_details":{"cached_tokens":0},
                    "output_tokens_details":{"reasoning_tokens":0}}),
            ),
        )
    }
}

#[derive(Debug)]
struct Observation {
    first_error: Option<String>,
    dispatches: Vec<Dispatch>,
    summary: CostSummary,
    ledger_rows: usize,
    usage_events: usize,
    cost_events: Vec<Option<f64>>,
    terminals: Vec<String>,
    followup_error: Option<String>,
    followup_dispatches: usize,
}

async fn observe(monthly: bool, limit: f64, prebook: bool) -> Observation {
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
    // Exactly CNY 1 per fixture response, not a claim about official prices.
    cost.set_pricing(std::collections::HashMap::from([(
        MODEL.to_string(),
        ModelPricing {
            input_cache_hit_per_million: 100.0,
            input_cache_miss_per_million: 1000.0,
            output_per_million: 0.0,
        },
    )]));
    cost.set_budget(BudgetConfig {
        daily_limit: (!monthly).then_some(limit),
        monthly_limit: monthly.then_some(limit),
    });
    let usage_events = Arc::new(AtomicUsize::new(0));
    let model = Arc::new(BudgetLoopModel {
        cost: cost.clone(),
        usage_events: usage_events.clone(),
        dispatches: Mutex::new(Vec::new()),
    });
    let chat =
        ChatService::new(db.clone(), settings, model.clone(), root.path()).with_cost(cost.clone());
    let session = chat.create_session(None).unwrap();
    if prebook {
        assert_eq!(
            cost.record(CostRecordRequest {
                session_id: session.clone(),
                model: MODEL.into(),
                input_tokens: 1000,
                output_tokens: 10,
                cache_hit_tokens: 0,
                cache_miss_tokens: 1000,
                total_tokens: 1010,
            })
            .unwrap(),
            Some(1.0)
        );
    }
    let cost_events = Arc::new(Mutex::new(Vec::new()));
    let captured = cost_events.clone();
    let counter = usage_events.clone();
    let first_error = chat
        .run_in_session(
            "List tasks once, then briefly acknowledge completion.",
            Some(&session),
            None,
            None,
            Vec::new(),
            None,
            false,
            Some(RUN),
            move |event| match event {
                RuntimeEvent::Usage {
                    prompt_tokens,
                    completion_tokens,
                    ..
                } => {
                    assert_eq!((prompt_tokens, completion_tokens), (1000, 10));
                    counter.fetch_add(1, Ordering::SeqCst);
                }
                RuntimeEvent::CostRecorded { cost_yuan, .. } => {
                    captured.lock().unwrap().push(cost_yuan);
                }
                _ => {}
            },
            |_| {},
        )
        .await
        .err()
        .map(|error| error.to_string());
    let summary = cost.summary(&session).unwrap();
    let ledger_rows = CostStore::new(&db)
        .session_costs_detailed(&session)
        .unwrap()
        .len();
    let terminals = chat
        .run_events(RUN, None)
        .unwrap()
        .into_iter()
        .filter(|event| event.event_type == "run_terminal")
        .map(|event| event.data["kind"].as_str().unwrap().to_string())
        .collect();
    let before_followup = model.dispatches.lock().unwrap().len();
    // Only low-limit groups make a follow-up: it should fail before dispatch.
    let followup_error = if limit < 1.0 {
        chat.run_in_session(
            "Briefly acknowledge completion.",
            Some(&session),
            None,
            None,
            Vec::new(),
            None,
            false,
            Some("audit-inflight-budget-followup"),
            |_| {},
            |_| {},
        )
        .await
        .err()
        .map(|error| error.to_string())
    } else {
        None
    };
    let dispatches = std::mem::take(&mut *model.dispatches.lock().unwrap());
    let observed = Observation {
        first_error,
        followup_dispatches: dispatches.len() - before_followup,
        dispatches,
        summary,
        ledger_rows,
        usage_events: usage_events.load(Ordering::SeqCst),
        cost_events: cost_events.lock().unwrap().clone(),
        terminals,
        followup_error,
    };
    eprintln!(
        "inflight budget: monthly={monthly} limit={limit} prebook={prebook} observed={observed:?}"
    );
    observed
}

fn assert_successful_two_calls(observed: &Observation) {
    assert!(observed.first_error.is_none());
    assert_eq!(observed.dispatches.len(), 2);
    assert_eq!(observed.usage_events, 2);
    assert!(observed.dispatches[1].tool_output_paired);
    // UI callbacks may lag the runtime's provider-response processing. Their
    // delivery count is diagnostic, not a dispatch ordering contract.
    assert!(observed.dispatches[1].prior_usage_events <= 1);
    assert_eq!(observed.dispatches[1].ledger_cost, 0.0);
    assert!(observed.dispatches[1].budget_allowed);
    assert_eq!(observed.summary.session_cost, 2.0);
    assert_eq!(observed.summary.input_tokens, 2000);
    assert_eq!(observed.summary.output_tokens, 20);
    assert_eq!(observed.summary.cache_hit_tokens, 0);
    assert_eq!(observed.summary.cache_miss_tokens, 2000);
    assert_eq!(observed.ledger_rows, 1);
    assert_eq!(observed.cost_events, [Some(2.0)]);
    assert_eq!(observed.terminals, ["succeeded"]);
}

#[tokio::test]
async fn sufficient_budget_allows_the_normal_tool_continuation() {
    for monthly in [false, true] {
        assert_successful_two_calls(&observe(monthly, 3.0, false).await);
    }
}

#[tokio::test]
async fn already_booked_spend_blocks_dispatch_for_both_periods() {
    for monthly in [false, true] {
        let observed = observe(monthly, 0.5, true).await;
        let category = if monthly {
            "monthly budget"
        } else {
            "daily budget"
        };
        assert!(observed.first_error.as_deref().unwrap().contains(category));
        assert!(observed
            .followup_error
            .as_deref()
            .unwrap()
            .contains(category));
        assert!(observed.dispatches.is_empty());
        assert_eq!(observed.followup_dispatches, 0);
        assert_eq!(observed.summary.total_cost, 1.0);
        assert_eq!(observed.ledger_rows, 1);
        assert_eq!(observed.usage_events, 0);
        assert!(observed.cost_events.is_empty());
    }
}

async fn assert_no_dispatch_after_measured_limit(monthly: bool) {
    let observed = observe(monthly, 0.5, false).await;
    // Complete all evidence and cleanup before the intended failing invariant.
    assert_successful_two_calls(&observed);
    assert!(observed
        .followup_error
        .as_deref()
        .unwrap()
        .contains(if monthly {
            "monthly budget exhausted"
        } else {
            "daily budget exhausted"
        }));
    assert_eq!(observed.followup_dispatches, 0);
    assert_eq!(
        observed.dispatches.len(),
        1,
        "first response already measured CNY 1 >= limit 0.5; do not dispatch another paid call"
    );
}

#[tokio::test]
#[ignore = "audit H-272: daily budget is not rechecked after measured in-run model usage"]
async fn audit_daily_budget_stops_after_measured_usage_reaches_limit() {
    assert_no_dispatch_after_measured_limit(false).await;
}

#[tokio::test]
#[ignore = "audit H-272: monthly budget is not rechecked after measured in-run model usage"]
async fn audit_monthly_budget_stops_after_measured_usage_reaches_limit() {
    assert_no_dispatch_after_measured_limit(true).await;
}
