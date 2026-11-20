//! Cost attribution and budget admission survive retries without model switching.

use super::*;
use deepagent_app_core::{BudgetConfig, CostSummary};
use deepagent_core::event::EventPayload;
use deepagent_persistence::cost_store::{CostStore, HistoricalCost};
use deepagent_persistence::event_store::EventStore;

const FLASH: &str = "deepseek-flash";
const PRO: &str = "deepseek-v4-pro";
const LIMIT: f64 = 0.002;

#[derive(Clone, Copy, Debug)]
enum RouteBudget {
    CompleteRoute,
    OriginalLimit,
    ExactFirstCall,
}

#[derive(Debug)]
struct RouteModel {
    retry_at: Option<usize>,
    tool_first: bool,
    requests: Mutex<Vec<String>>,
}

#[async_trait]
impl HttpTransport for RouteModel {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        let body: serde_json::Value = serde_json::from_str(&request.body)?;
        let call = {
            let mut requests = self.requests.lock().unwrap();
            let call = requests.len();
            requests.push(body["model"].as_str().unwrap().to_string());
            call
        };
        if self.retry_at == Some(call) {
            return Err(CoreError::provider(
                Some(503),
                Some("service_unavailable".into()),
                "AUDIT_CONTROLLED_TRANSIENT_FAILURE",
            ));
        }
        let mut events = if call == 0 && self.tool_first {
            response_function_call(
                "audit-cost-read",
                "read_file",
                serde_json::json!({"path":"fixture.txt"}),
            )
        } else {
            response_text("AUDIT_FALLBACK_COST_COMPLETE")
        };
        let output = serde_json::from_str::<serde_json::Value>(&events.pop().unwrap())?["response"]
            ["output"]
            .clone();
        events.push(
            serde_json::json!({
                "type":"response.completed",
                "response":{"id":"fixture-response","object":"response","created_at":1,"output":output,
                    "status":"completed",
                    "model":body["model"],
                    "usage":{
                        "input_tokens":1000,
                        "input_tokens_details":{"cached_tokens":800},
                        "output_tokens":100,
                        "total_tokens":1100
                    }
                }
            })
            .to_string(),
        );
        emit_responses_events(sink, &events)
    }
}

#[derive(Debug)]
struct RouteObservation {
    first_models: Vec<String>,
    ledger_models: Vec<String>,
    rows: Vec<HistoricalCost>,
    summary: CostSummary,
    cost_events: Vec<(String, Option<f64>)>,
    expected_cost: f64,
    limit: f64,
    second_error: Option<String>,
    second_dispatches: usize,
}

async fn observe_route(
    primary: &str,
    retry: bool,
    tool_first: bool,
    budget: RouteBudget,
) -> RouteObservation {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("fixture.txt"), "AUDIT_LOCAL_COST_FIXTURE").unwrap();
    let db = Arc::new(Database::open(root.path().join("state.sqlite")).unwrap());
    let secrets = Arc::new(MemorySecretStore::new());
    let settings = Arc::new(SettingsService::new(
        db.clone(),
        Arc::new(MockTransport::with_get_json(
            r#"{"data":[{"id":"deepseek-flash","object":"model","owned_by":"deepseek"},{"id":"deepseek-v4-pro","object":"model","owned_by":"deepseek"}]}"#,
        )),
        secrets.clone(),
    ));
    crate::registry_fixture::configure(&settings, "offline-audit-placeholder")
        .await
        .unwrap();
    settings
        .harness_models()
        .select_model("deepseek", primary)
        .unwrap();
    let prices = settings.load().unwrap().unwrap().pricing_catalog.models;
    assert_eq!(prices[FLASH].calculate(1000, 100, 800, 200), 0.000616);
    assert_eq!(prices[PRO].calculate(1000, 100, 800, 200), 0.002370);
    let first_call_cost = prices[primary].calculate(1000, 100, 800, 200);
    let stop_after_first = !matches!(budget, RouteBudget::CompleteRoute);
    let limit = match budget {
        // Route accounting needs enough budget to dispatch the continuation.
        // Keep the original low limit in separate explicit stop scenarios.
        RouteBudget::CompleteRoute => LIMIT + if tool_first { first_call_cost } else { 0.0 },
        RouteBudget::OriginalLimit => LIMIT,
        RouteBudget::ExactFirstCall => first_call_cost,
    };
    if stop_after_first {
        assert!(tool_first && first_call_cost >= limit);
    }
    let cost = Arc::new(CostService::from_settings(db.clone(), settings.clone()));
    cost.set_budget(BudgetConfig {
        daily_limit: Some(limit),
        monthly_limit: None,
    })
    .unwrap();
    let model = Arc::new(RouteModel {
        retry_at: retry.then_some(usize::from(tool_first)),
        tool_first,
        requests: Mutex::new(Vec::new()),
    });
    let chat =
        ChatService::new(db.clone(), settings, model.clone(), root.path()).with_cost(cost.clone());
    let session = chat.create_session(None).unwrap();
    let captured = Arc::new(Mutex::new(Vec::new()));
    let mut first = None;
    let mut second_error = None;
    for turn in 1..=2 {
        let events = captured.clone();
        let run_id = format!("audit-fallback-cost-{turn}");
        let result = chat
            .run_in_session(
                "Reply briefly; read fixture.txt if requested.",
                Some(&session),
                None,
                None,
                Vec::new(),
                None,
                false,
                Some(&run_id),
                move |event| {
                    if let deepagent_runtime::events::RuntimeEvent::CostRecorded {
                        model,
                        cost_yuan,
                        ..
                    } = event
                    {
                        events.lock().unwrap().push((model, cost_yuan));
                    }
                },
                |_| {},
            )
            .await;
        if turn == 1 {
            if stop_after_first {
                assert!(result
                    .unwrap_err()
                    .to_string()
                    .contains("daily budget exhausted"));
            } else {
                result.unwrap();
            }
            let first_models = model.requests.lock().unwrap().clone();
            let mut expected_models = vec![primary.to_string()];
            if tool_first && !stop_after_first {
                expected_models.push(primary.to_string());
            }
            if retry && !stop_after_first {
                expected_models.push(primary.to_string());
            }
            assert_eq!(first_models, expected_models);
            // H-219: compare the recorder to actual physical transport routes,
            // including the rejected and successful attempts of the same model.
            let run_events = chat.run_events(&run_id, None).unwrap();
            let snapshot = run_events
                .iter()
                .find(|event| event.event_type == "capability.snapshot")
                .unwrap();
            let wire_models = run_events
                .iter()
                .filter(|event| event.event_type == "capability_wire_snapshot")
                .map(|event| {
                    assert_eq!(event.data["snapshotId"], snapshot.data["snapshotId"]);
                    event.data["wireSchema"]["model"]
                        .as_str()
                        .unwrap()
                        .to_owned()
                })
                .collect::<Vec<_>>();
            let mut routes = first_models.clone();
            routes.dedup();
            assert_eq!(
                wire_models, routes,
                "persisted wire generations must follow real adapter dispatches"
            );
            let expected_cost = prices[primary].calculate(1000, 100, 800, 200)
                + if tool_first && !stop_after_first {
                    prices[primary].calculate(1000, 100, 800, 200)
                } else {
                    0.0
                };
            let ledger_models = db
                .with_conn(|conn| {
                    let mut stmt = conn
                        .prepare("SELECT model FROM costs WHERE session_id=?1 ORDER BY id")
                        .map_err(|e| CoreError::Persistence(e.to_string()))?;
                    let rows = stmt
                        .query_map([&session], |row| row.get::<_, String>(0))
                        .map_err(|e| CoreError::Persistence(e.to_string()))?;
                    rows.collect::<std::result::Result<Vec<_>, _>>()
                        .map_err(|e| CoreError::Persistence(e.to_string()))
                })
                .unwrap();
            let summary = cost.summary(&session).unwrap();
            let calls = 1 + u64::from(tool_first && !stop_after_first);
            assert_eq!(summary.input_tokens, 1000 * calls);
            assert_eq!(summary.output_tokens, 100 * calls);
            assert_eq!(summary.cache_hit_tokens, 800 * calls);
            assert_eq!(summary.cache_miss_tokens, 200 * calls);
            let rows = CostStore::new(&db)
                .session_costs_detailed(&session)
                .unwrap();
            let receipts: Vec<_> = EventStore::new(&db)
                .load_session(session.parse().unwrap())
                .unwrap()
                .into_iter()
                .filter_map(|event| match event.payload {
                    EventPayload::RunCostRecorded {
                        run_id,
                        ledger_ids,
                        cost_yuan,
                        ..
                    } => Some((run_id, ledger_ids, cost_yuan)),
                    _ => None,
                })
                .collect();
            assert_eq!(receipts.len(), 1);
            assert_eq!(receipts[0].0, run_id);
            assert_eq!(
                receipts[0].1,
                rows.iter().map(|row| row.id).collect::<Vec<_>>()
            );
            assert!(receipts[0]
                .2
                .is_some_and(|cost| (cost - expected_cost).abs() < 1e-12));
            first = Some((
                first_models,
                ledger_models,
                rows,
                summary,
                captured.lock().unwrap().clone(),
                expected_cost,
            ));
            if tool_first {
                assert!(chat
                    .run_events(&run_id, None)
                    .unwrap()
                    .iter()
                    .any(|event| event.event_type == "tool_completed"
                        && event.data["name"] == "read_file"
                        && event.data["ok"] == true));
            }
        } else {
            second_error = result.err().map(|error| error.to_string());
        }
        if turn == 1 || second_error.is_none() {
            let terminals: Vec<_> = chat
                .run_events(&run_id, None)
                .unwrap()
                .into_iter()
                .filter(|event| event.event_type == "run_terminal")
                .collect();
            assert_eq!(terminals.len(), 1);
            assert_eq!(
                terminals[0].data["kind"],
                if turn == 1 && stop_after_first {
                    "budget_exceeded"
                } else {
                    "succeeded"
                }
            );
        }
    }
    let (first_models, ledger_models, rows, summary, cost_events, expected_cost) = first.unwrap();
    let second_dispatches = model.requests.lock().unwrap().len() - first_models.len();
    let app = deepagent_app_core::AppService::from_shared(db.clone());
    let before_reopen = app.session_conversation(&session).unwrap();
    let turn_costs: Vec<_> = before_reopen
        .iter()
        .filter_map(|message| message.usage.as_ref())
        .map(|usage| usage.cost_yuan)
        .collect();
    assert_eq!(turn_costs.len(), 1 + usize::from(second_error.is_none()));
    assert!(turn_costs[0].is_some_and(|cost| (cost - expected_cost).abs() < 1e-12));
    if second_error.is_none() {
        assert!(turn_costs[1]
            .is_some_and(
                |cost| (cost - prices[primary].calculate(1000, 100, 800, 200)).abs() < 1e-12
            ));
    }
    let persisted_total = cost.summary(&session).unwrap().session_cost;
    drop(chat);
    drop(app);
    drop(cost);
    drop(db);
    let reopened = Arc::new(Database::open(root.path().join("state.sqlite")).unwrap());
    let settings = Arc::new(SettingsService::new(
        reopened.clone(),
        Arc::new(MockTransport::new(vec![])),
        secrets,
    ));
    let cost = Arc::new(CostService::from_settings(
        reopened.clone(),
        settings.clone(),
    ));
    let app = deepagent_app_core::AppService::from_shared(reopened.clone());
    let _chat = ChatService::new(reopened, settings, model, root.path()).with_cost(cost.clone());
    assert_eq!(app.session_conversation(&session).unwrap(), before_reopen);
    assert_eq!(
        cost.summary(&session).unwrap().session_cost,
        persisted_total
    );
    let observed = RouteObservation {
        first_models,
        ledger_models,
        rows,
        summary,
        cost_events,
        expected_cost,
        limit,
        second_error,
        second_dispatches,
    };
    eprintln!("route cost audit: retry={retry} tool_first={tool_first} observed={observed:?}");
    observed
}

fn assert_route_cost(observed: &RouteObservation) {
    assert_eq!(observed.cost_events.len(), 1, "one whole-run UI cost event");
    let ledger_total: Option<f64> = observed.rows.iter().map(|row| row.cost_yuan).sum();
    let event_total: Option<f64> = observed.cost_events.iter().map(|(_, cost)| *cost).sum();
    let cost_matches = (observed.summary.session_cost - observed.expected_cost).abs() < 1e-12
        && ledger_total.is_some_and(|cost| (cost - observed.expected_cost).abs() < 1e-12)
        && event_total.is_some_and(|cost| (cost - observed.expected_cost).abs() < 1e-12);
    let blocked = observed
        .second_error
        .as_deref()
        .is_some_and(|error| error.contains("daily budget exhausted"));
    assert!(
        cost_matches
            && blocked == (observed.expected_cost >= observed.limit)
            && observed.second_dispatches == usize::from(!blocked),
        "answered model route must own cost and next admission: {observed:?}"
    );
    // A single answered call also has an unambiguous expected ledger/event model.
    if observed.summary.input_tokens == 1000 {
        let answered = observed.first_models.last().unwrap();
        assert_eq!(
            observed.ledger_models.as_slice(),
            std::slice::from_ref(answered)
        );
        assert_eq!(
            &observed.cost_events,
            &[(answered.clone(), Some(observed.expected_cost))]
        );
        assert_eq!(observed.rows.len(), 1);
    } else {
        let answered = vec![
            observed.first_models[0].clone(),
            observed.first_models.last().unwrap().clone(),
        ];
        assert_eq!(observed.ledger_models, answered);
        assert_eq!(
            observed.rows.len(),
            2,
            "one durable row per answered physical request"
        );
        assert_eq!(
            observed.cost_events[0].0,
            if answered[0] == answered[1] {
                &answered[0]
            } else {
                "multi-model"
            }
        );
    }
}

#[tokio::test]
async fn unchanged_models_book_single_and_multiple_answered_calls_correctly() {
    for primary in [FLASH, PRO] {
        for tool_first in [false, true] {
            assert_route_cost(
                &observe_route(primary, false, tool_first, RouteBudget::CompleteRoute).await,
            );
        }
    }
}

#[tokio::test]
async fn transient_retry_keeps_each_selected_models_cost_and_next_run_admission() {
    for primary in [FLASH, PRO] {
        assert_route_cost(&observe_route(primary, true, false, RouteBudget::CompleteRoute).await);
    }
}

#[tokio::test]
async fn retry_and_tool_continuation_preserve_each_answered_calls_cost() {
    assert_route_cost(&observe_route(FLASH, true, true, RouteBudget::CompleteRoute).await);
    assert_route_cost(&observe_route(PRO, true, true, RouteBudget::CompleteRoute).await);
}

#[tokio::test]
async fn original_low_budget_stops_before_another_attempt() {
    for retry in [false, true] {
        assert_route_cost(&observe_route(PRO, retry, true, RouteBudget::OriginalLimit).await);
    }
}

#[tokio::test]
async fn exact_first_call_budget_stops_before_another_attempt() {
    for primary in [FLASH, PRO] {
        for retry in [false, true] {
            assert_route_cost(
                &observe_route(primary, retry, true, RouteBudget::ExactFirstCall).await,
            );
        }
    }
}
