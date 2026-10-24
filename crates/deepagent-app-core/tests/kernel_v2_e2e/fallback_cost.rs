//! Cost attribution must follow answered requests, not the run's initial model.

use super::*;
use deepagent_app_core::{BudgetConfig, CostSummary};
use deepagent_models::ModelRole;
use deepagent_persistence::cost_store::{CostStore, HistoricalCost};

const FLASH: &str = "deepseek-flash";
const PRO: &str = "deepseek-v4-pro";
const LIMIT: f64 = 0.002;

#[derive(Debug)]
struct RouteModel {
    overload_at: Option<usize>,
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
        if self.overload_at == Some(call) {
            // Exercise the system's existing classifier, not a claim that DeepSeek
            // officially specifies this error code for its ordinary HTTP 503.
            return Err(CoreError::provider(
                Some(503),
                Some("overloaded_error".into()),
                "AUDIT_CONTROLLED_OVERLOAD",
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
        events.pop();
        events.push(
            serde_json::json!({
                "type":"response.completed",
                "response":{
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
    second_error: Option<String>,
    second_dispatches: usize,
}

async fn observe_route(primary: &str, switch: bool, tool_first: bool) -> RouteObservation {
    let fallback = if primary == FLASH { PRO } else { FLASH };
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("fixture.txt"), "AUDIT_LOCAL_COST_FIXTURE").unwrap();
    let db = Arc::new(Database::open(root.path().join("state.sqlite")).unwrap());
    let settings = Arc::new(SettingsService::new(
        db.clone(),
        Arc::new(MockTransport::with_get_json(
            r#"{"data":[{"id":"deepseek-flash","object":"model","owned_by":"deepseek"},{"id":"deepseek-v4-pro","object":"model","owned_by":"deepseek"}]}"#,
        )),
        Arc::new(MemorySecretStore::new()),
    ));
    settings
        .initialize("offline-audit-placeholder")
        .await
        .unwrap();
    settings.set_model(ModelRole::Chat, primary).unwrap();
    settings.set_model(ModelRole::Reasoner, fallback).unwrap();
    let prices = settings.load().unwrap().unwrap().pricing_catalog.models;
    assert_eq!(prices[FLASH].calculate(1000, 100, 800, 200), 0.000616);
    assert_eq!(prices[PRO].calculate(1000, 100, 800, 200), 0.002370);
    let cost = Arc::new(CostService::new(db.clone()));
    cost.set_pricing(prices.clone()); // Deliberately avoid the separate H-225 activation gap.
    cost.set_budget(BudgetConfig {
        daily_limit: Some(LIMIT),
        monthly_limit: None,
    });
    let model = Arc::new(RouteModel {
        overload_at: switch.then_some(usize::from(tool_first)),
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
            result.unwrap();
            let first_models = model.requests.lock().unwrap().clone();
            let mut expected_models = vec![primary.to_string()];
            if tool_first {
                expected_models.push(primary.to_string());
            }
            if switch {
                expected_models.push(fallback.to_string());
            }
            assert_eq!(first_models, expected_models);
            let answered = if switch { fallback } else { primary };
            let expected_cost = prices[answered].calculate(1000, 100, 800, 200)
                + if tool_first {
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
            let calls = 1 + u64::from(tool_first);
            assert_eq!(summary.input_tokens, 1000 * calls);
            assert_eq!(summary.output_tokens, 100 * calls);
            assert_eq!(summary.cache_hit_tokens, 800 * calls);
            assert_eq!(summary.cache_miss_tokens, 200 * calls);
            let rows = CostStore::new(&db)
                .session_costs_detailed(&session)
                .unwrap();
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
            assert_eq!(terminals[0].data["kind"], "succeeded");
        }
    }
    let (first_models, ledger_models, rows, summary, cost_events, expected_cost) = first.unwrap();
    let second_dispatches = model.requests.lock().unwrap().len() - first_models.len();
    let observed = RouteObservation {
        first_models,
        ledger_models,
        rows,
        summary,
        cost_events,
        expected_cost,
        second_error,
        second_dispatches,
    };
    eprintln!("fallback cost audit: switch={switch} tool_first={tool_first} observed={observed:?}");
    observed
}

fn assert_route_cost(observed: &RouteObservation) {
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
            && blocked == (observed.expected_cost >= LIMIT)
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
    }
}

#[tokio::test]
async fn unchanged_models_book_single_and_multiple_answered_calls_correctly() {
    for primary in [FLASH, PRO] {
        for tool_first in [false, true] {
            assert_route_cost(&observe_route(primary, false, tool_first).await);
        }
    }
}

#[tokio::test]
#[ignore = "audit H-226: fallback Pro usage is booked at the initial Flash price"]
async fn audit_fallback_to_pro_books_pro_cost_and_blocks_the_next_run() {
    assert_route_cost(&observe_route(FLASH, true, false).await);
}

#[tokio::test]
#[ignore = "audit H-226: fallback Flash usage is booked at the initial Pro price"]
async fn audit_fallback_to_flash_books_flash_cost_and_allows_the_next_run() {
    assert_route_cost(&observe_route(PRO, true, false).await);
}

#[tokio::test]
#[ignore = "audit H-226: mixed model usage cannot be priced under one initial model"]
async fn audit_mixed_model_run_preserves_each_answered_calls_cost_route() {
    assert_route_cost(&observe_route(FLASH, true, true).await);
}
