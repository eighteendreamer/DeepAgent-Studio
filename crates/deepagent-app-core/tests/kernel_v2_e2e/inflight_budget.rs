//! Offline measured-usage budget probes; prices are synthetic, not provider quotes.

use super::responses_fixture::{call, completed, text};
use super::*;
use deepagent_app_core::cost_service::CostRecordRequest;
use deepagent_app_core::{BudgetConfig, CostSummary, ModelPricing};
use deepagent_core::{event::EventPayload, id::SessionId};
use deepagent_persistence::cost_store::CostStore;
use deepagent_persistence::event_store::EventStore;
use deepagent_runtime::events::RuntimeEvent;
use serde_json::{json, Value};
use std::str::FromStr;

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
    final_answer: bool,
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
        let item = if index == 0 && !self.final_answer {
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
    completed_tools: usize,
    followup_error: Option<String>,
    followup_dispatches: usize,
}

async fn observe(monthly: bool, limit: f64, prebook: bool) -> Observation {
    observe_case(monthly, limit, prebook, false).await
}

async fn budget_services(
    root: &std::path::Path,
    monthly: bool,
    limit: f64,
) -> (Arc<Database>, Arc<SettingsService>, Arc<CostService>) {
    let db = Arc::new(Database::open(root.join("state.sqlite")).unwrap());
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
    })
    .unwrap();
    (db, settings, cost)
}

async fn observe_case(monthly: bool, limit: f64, prebook: bool, final_answer: bool) -> Observation {
    let root = tempfile::tempdir().unwrap();
    let (db, settings, cost) = budget_services(root.path(), monthly, limit).await;
    let usage_events = Arc::new(AtomicUsize::new(0));
    let model = Arc::new(BudgetLoopModel {
        final_answer,
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
    let session_id = SessionId::from_str(&session).unwrap();
    let history = EventStore::new(&db).load_session(session_id).unwrap();
    let completed_tools = history.iter().filter(|event| matches!(
        &event.payload,
        EventPayload::ToolCallCompleted { call_id, ok: true, .. } if call_id == "budget-list"
    )).count();
    // A second real connection must see the committed tool result, even when
    // budget admission prevents a model from consuming it in this turn.
    let reopened = Database::open(root.path().join("state.sqlite")).unwrap();
    assert_eq!(
        EventStore::new(&reopened).load_session(session_id).unwrap(),
        history
    );
    let before_followup = model.dispatches.lock().unwrap().len();
    // Only low-limit groups make a follow-up: it should fail before dispatch.
    let followup_error = if limit <= 1.0 {
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
        completed_tools,
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
    assert_eq!(observed.completed_tools, 1);
    // UI callbacks may lag the runtime's provider-response processing. Their
    // delivery count is diagnostic, not a dispatch ordering contract.
    assert!(observed.dispatches[1].prior_usage_events <= 1);
    // Each measured request must be committed before another paid request,
    // independently of the asynchronous UI event pump.
    assert_eq!(observed.dispatches[1].ledger_cost, 1.0);
    assert!(observed.dispatches[1].budget_allowed);
    assert_eq!(observed.summary.session_cost, 2.0);
    assert_eq!(observed.summary.input_tokens, 2000);
    assert_eq!(observed.summary.output_tokens, 20);
    assert_eq!(observed.summary.cache_hit_tokens, 0);
    assert_eq!(observed.summary.cache_miss_tokens, 2000);
    assert_eq!(observed.ledger_rows, 2);
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

async fn assert_no_dispatch_after_measured_limit(monthly: bool, limit: f64) {
    let observed = observe(monthly, limit, false).await;
    let category = if monthly {
        "monthly budget exhausted"
    } else {
        "daily budget exhausted"
    };
    assert_eq!(
        observed.dispatches.len(),
        1,
        "first response already measured CNY 1 >= limit {limit}; do not dispatch another paid call"
    );
    assert!(observed.first_error.as_deref().unwrap().contains(category));
    assert_eq!(observed.terminals, ["budget_exceeded"]);
    assert_eq!(observed.completed_tools, 1);
    assert_eq!(observed.summary.session_cost, 1.0);
    assert_eq!(observed.summary.input_tokens, 1000);
    assert_eq!(observed.summary.output_tokens, 10);
    assert_eq!(observed.ledger_rows, 1);
    assert_eq!(observed.usage_events, 1);
    assert_eq!(observed.cost_events, [Some(1.0)]);
    assert!(observed
        .followup_error
        .as_deref()
        .unwrap()
        .contains(category));
    assert_eq!(observed.followup_dispatches, 0);
}

#[tokio::test]
async fn audit_daily_budget_stops_after_measured_usage_reaches_limit() {
    assert_no_dispatch_after_measured_limit(false, 0.5).await;
}

#[tokio::test]
async fn audit_monthly_budget_stops_after_measured_usage_reaches_limit() {
    assert_no_dispatch_after_measured_limit(true, 0.5).await;
}

#[tokio::test]
async fn exactly_reaching_either_period_limit_also_stops_the_next_paid_request() {
    for monthly in [false, true] {
        assert_no_dispatch_after_measured_limit(monthly, 1.0).await;
    }
}

#[tokio::test]
async fn a_final_answer_that_itself_exceeds_the_limit_is_not_retroactively_failed() {
    for monthly in [false, true] {
        let observed = observe_case(monthly, 0.5, false, true).await;
        assert!(observed.first_error.is_none());
        assert_eq!(observed.dispatches.len(), 1);
        assert_eq!(observed.completed_tools, 0);
        assert_eq!(observed.terminals, ["succeeded"]);
        assert_eq!(observed.summary.session_cost, 1.0);
        assert_eq!(observed.ledger_rows, 1);
        assert_eq!(observed.cost_events, [Some(1.0)]);
        assert!(observed
            .followup_error
            .as_deref()
            .unwrap()
            .contains("budget exhausted"));
        assert_eq!(observed.followup_dispatches, 0);
    }
}

const CHILD: &str = "h272-private-child";

struct DelegatedBudgetModel {
    child_loop: bool,
    cost: Arc<CostService>,
    parent_calls: AtomicUsize,
    child_calls: AtomicUsize,
    dispatches: Mutex<Vec<(bool, f64)>>,
}

#[async_trait]
impl HttpTransport for DelegatedBudgetModel {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        let body: Value = serde_json::from_str(&request.body)?;
        assert_eq!(body["model"], MODEL);
        // Match only user input, not the parent's task call arguments/result.
        let child = request_contains_user_input(&body, CHILD);
        let index = if child {
            self.child_calls.fetch_add(1, Ordering::AcqRel)
        } else {
            self.parent_calls.fetch_add(1, Ordering::AcqRel)
        };
        assert!(index < 2, "unexpected delegated model retry or extra turn");
        self.dispatches
            .lock()
            .unwrap()
            .push((child, self.cost.summary("")?.total_cost));
        let item = match (child, index, self.child_loop) {
            (false, 0, _) => call(
                "budget-delegate",
                "task",
                json!({"description":"offline budget child", "prompt":CHILD,
                    "allowed_tools":["task_list"]}),
            ),
            (true, 0, true) => call("budget-child-list", "task_list", json!({})),
            (true, _, _) => text("budget-child-answer", "Synthetic private child result."),
            _ => text("budget-parent-answer", "Synthetic delegation complete."),
        };
        emit_responses_events(
            sink,
            &completed(
                &format!("delegated-{child}-{index}"),
                MODEL,
                item,
                json!({"input_tokens":1000,"output_tokens":10,"total_tokens":1010,
                    "input_tokens_details":{"cached_tokens":0}}),
            ),
        )
    }
}

async fn assert_delegated_budget(child_loop: bool) {
    use deepagent_runtime::events::CostOrigin;

    for monthly in [false, true] {
        for limited in [true, false] {
            let root = tempfile::tempdir().unwrap();
            let (db, settings, cost) =
                budget_services(root.path(), monthly, if limited { 2.0 } else { 5.0 }).await;
            let model = Arc::new(DelegatedBudgetModel {
                child_loop,
                cost: cost.clone(),
                parent_calls: AtomicUsize::new(0),
                child_calls: AtomicUsize::new(0),
                dispatches: Mutex::new(Vec::new()),
            });
            let chat = ChatService::new(db.clone(), settings, model.clone(), root.path())
                .with_cost(cost.clone());
            let session = chat.create_session(None).unwrap();
            let live = Arc::new(Mutex::new(Vec::new()));
            let capture = live.clone();
            let result = chat
                .run_in_session(
                    "Delegate one synthetic task, then acknowledge its result.",
                    Some(&session),
                    None,
                    None,
                    Vec::new(),
                    None,
                    false,
                    Some(RUN),
                    move |event| capture.lock().unwrap().push(event),
                    |_| {},
                )
                .await;
            if limited {
                assert!(result.unwrap_err().to_string().contains(if monthly {
                    "monthly budget exhausted"
                } else {
                    "daily budget exhausted"
                }));
            } else {
                result.unwrap();
            }
            let parents = if limited { 1 } else { 2 };
            let children = if !limited && child_loop { 2 } else { 1 };
            assert_eq!(model.parent_calls.load(Ordering::Acquire), parents);
            assert_eq!(model.child_calls.load(Ordering::Acquire), children);
            let mut expected = vec![(false, 0.0), (true, 1.0)];
            if !limited {
                if child_loop {
                    expected.push((true, 2.0));
                }
                expected.push((false, 1.0 + children as f64));
            }
            assert_eq!(*model.dispatches.lock().unwrap(), expected);

            let ledger = CostStore::new(&db)
                .session_costs_detailed(&session)
                .unwrap();
            assert_eq!(ledger.len(), parents + children);
            let summary = cost.summary(&session).unwrap();
            assert_eq!(summary.session_cost, (parents + children) as f64);
            assert_eq!(summary.input_tokens, ((parents + children) * 1000) as u64);
            let receipts = CostStore::new(&db)
                .run_call_receipts(&session, RUN)
                .unwrap();
            assert_eq!(receipts.len(), parents + children);
            assert_eq!(
                receipts
                    .iter()
                    .filter(|r| r.report["origin"] == "run")
                    .count(),
                parents
            );
            assert_eq!(
                receipts
                    .iter()
                    .filter(|r| r.report["purpose"] == "subagent")
                    .count(),
                children
            );
            assert!(receipts
                .iter()
                .all(|r| r.ledger_id.is_some() && r.cost_yuan == Some(1.0)));
            let events = live.lock().unwrap();
            assert_eq!(
                events
                    .iter()
                    .filter(|e| matches!(e, RuntimeEvent::Usage { .. }))
                    .count(),
                parents
            );
            let main_costs: Vec<_> = events
                .iter()
                .filter_map(|e| match e {
                    RuntimeEvent::CostRecorded {
                        origin: CostOrigin::Run,
                        cost_yuan,
                        ..
                    } => Some(*cost_yuan),
                    _ => None,
                })
                .collect();
            assert_eq!(main_costs, [Some(parents as f64)]);
            assert_eq!(
                events
                    .iter()
                    .filter(|e| matches!(
                        e,
                        RuntimeEvent::CostRecorded {
                            origin: CostOrigin::SideCall,
                            cost_yuan: Some(1.0),
                            ..
                        }
                    ))
                    .count(),
                children
            );

            let session_id = session.parse().unwrap();
            let history = EventStore::new(&db).load_session(session_id).unwrap();
            let task_results: Vec<_> = history
                .iter()
                .filter_map(|event| match &event.payload {
                    EventPayload::ToolCallCompleted {
                        call_id,
                        ok,
                        output,
                        ..
                    } if call_id == "budget-delegate" => Some((*ok, output)),
                    _ => None,
                })
                .collect();
            assert_eq!(task_results.len(), 1);
            assert_eq!(task_results[0].0, !(limited && child_loop));
            let projected: Vec<_> = history
                .iter()
                .filter_map(|event| match &event.payload {
                    EventPayload::RunCostRecorded {
                        ledger_ids,
                        cost_yuan,
                        ..
                    } => Some((ledger_ids.len(), *cost_yuan)),
                    _ => None,
                })
                .collect();
            assert_eq!(projected, [(parents, Some(parents as f64))]);
            let runs = chat.subagent_runs(RUN).unwrap();
            assert_eq!(runs.len(), 1);
            assert_eq!(
                runs[0].state,
                if limited && child_loop {
                    "failed"
                } else {
                    "succeeded"
                }
            );
            let terminals: Vec<_> = chat
                .run_events(RUN, None)
                .unwrap()
                .into_iter()
                .filter(|e| e.event_type == "run_terminal")
                .map(|e| e.data["kind"].clone())
                .collect();
            assert_eq!(
                terminals,
                [json!(if limited {
                    "budget_exceeded"
                } else {
                    "succeeded"
                })]
            );
            let reopened = Database::open(root.path().join("state.sqlite")).unwrap();
            assert_eq!(
                EventStore::new(&reopened).load_session(session_id).unwrap(),
                history
            );
            assert_eq!(
                CostStore::new(&reopened)
                    .run_call_receipts(&session, RUN)
                    .unwrap()
                    .len(),
                parents + children
            );
        }
    }
}

#[tokio::test]
async fn child_final_usage_blocks_the_parent_continuation_with_sufficient_budget_control() {
    assert_delegated_budget(false).await;
}

#[tokio::test]
async fn child_tool_usage_blocks_both_child_and_parent_continuations_with_normal_control() {
    assert_delegated_budget(true).await;
}
