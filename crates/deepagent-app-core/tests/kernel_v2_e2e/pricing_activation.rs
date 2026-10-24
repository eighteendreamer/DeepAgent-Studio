//! First-login pricing activation through the same services the desktop owns.

use super::*;
use deepagent_app_core::{BudgetConfig, CostSummary};
use deepagent_persistence::cost_store::{CostStore, HistoricalCost};

const MODEL: &str = "deepseek-flash";
const EXPECTED_IDLE_COST: f64 = 0.000616;

#[derive(Debug, Clone, Copy)]
enum Activation {
    ConfiguredStartup,
    FirstLogin,
    RefreshAfterLogin,
}

#[derive(Debug, Default)]
struct PricedUsageModel(Mutex<Vec<String>>);

#[async_trait]
impl HttpTransport for PricedUsageModel {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        let body: serde_json::Value = serde_json::from_str(&request.body)?;
        self.0
            .lock()
            .unwrap()
            .push(body["model"].as_str().unwrap().to_string());
        emit_responses_events(
            sink,
            &[
                serde_json::json!({
                    "type": "response.output_text.delta",
                    "delta": "AUDIT_FIRST_LOGIN_PRICING_COMPLETE"
                })
                .to_string(),
                serde_json::json!({
                    "type": "response.completed",
                    "response": {
                        "status": "completed",
                        "usage": {
                            "input_tokens": 1000,
                            "input_tokens_details": {"cached_tokens": 800},
                            "output_tokens": 100,
                            "total_tokens": 1100
                        }
                    }
                })
                .to_string(),
            ],
        )
    }
}

#[derive(Debug)]
struct PricingObservation {
    first_summary: CostSummary,
    first_rows: Vec<HistoricalCost>,
    second_error: Option<String>,
    wire_models: Vec<String>,
    cost_events: Vec<Option<f64>>,
}

async fn observe_activation(activation: Activation) -> PricingObservation {
    let root = tempfile::tempdir().unwrap();
    let db = Arc::new(Database::open(root.path().join("state.sqlite")).unwrap());
    let settings = Arc::new(SettingsService::new(
        db.clone(),
        Arc::new(MockTransport::with_get_json(
            r#"{"data":[{"id":"deepseek-flash","object":"model","owned_by":"deepseek"}]}"#,
        )),
        Arc::new(MemorySecretStore::new()),
    ));
    if matches!(activation, Activation::ConfiguredStartup) {
        settings
            .initialize("offline-audit-placeholder")
            .await
            .unwrap();
    }

    // Mirror desktop startup: load existing pricing, then attempt its one-shot refresh.
    let cost = Arc::new(CostService::new(db.clone()));
    if let Some(view) = settings.view().unwrap() {
        cost.set_pricing(view.pricing_catalog.models);
    }
    match settings.refresh_pricing().await {
        Ok(catalog) => cost.set_pricing(catalog.models),
        Err(error) => {
            assert!(!matches!(activation, Activation::ConfiguredStartup));
            assert!(error.to_string().contains("settings not initialized"));
        }
    }

    // Mirror initialize_project: settings activate successfully, but cost is not notified.
    if !matches!(activation, Activation::ConfiguredStartup) {
        settings
            .initialize("offline-audit-placeholder")
            .await
            .unwrap();
    }
    let persisted = settings.load().unwrap().unwrap();
    assert_eq!(persisted.catalog.chat_model, MODEL);
    let price = persisted.pricing_catalog.models.get(MODEL).unwrap();
    assert!((price.calculate(1000, 100, 800, 200) - EXPECTED_IDLE_COST).abs() < 1e-12);
    if matches!(activation, Activation::RefreshAfterLogin) {
        // Positive control: the existing explicit-refresh bridge delivers the catalog.
        cost.set_pricing(settings.refresh_pricing().await.unwrap().models);
    }
    cost.set_budget(BudgetConfig {
        daily_limit: Some(0.0005),
        monthly_limit: None,
    });
    let model = Arc::new(PricedUsageModel::default());
    let chat =
        ChatService::new(db.clone(), settings, model.clone(), root.path()).with_cost(cost.clone());
    let cost_events = Arc::new(Mutex::new(Vec::new()));
    let session = chat.create_session(None).unwrap();
    let mut second_error = None;
    let mut first_summary = None;
    let mut first_rows = None;
    for turn in 1..=2 {
        let captured = cost_events.clone();
        let run_id = format!("audit-pricing-activation-{turn}");
        let result = chat
            .run_in_session(
                "Reply with a short acknowledgement.",
                Some(&session),
                None,
                None,
                Vec::new(),
                None,
                false,
                Some(&run_id),
                move |event| {
                    if let deepagent_runtime::events::RuntimeEvent::CostRecorded {
                        cost_yuan, ..
                    } = event
                    {
                        captured.lock().unwrap().push(cost_yuan);
                    }
                },
                |_| {},
            )
            .await;
        if turn == 1 {
            result.unwrap();
            first_summary = Some(cost.summary(&session).unwrap());
            first_rows = Some(
                CostStore::new(&db)
                    .session_costs_detailed(&session)
                    .unwrap(),
            );
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
    let observed = PricingObservation {
        first_summary: first_summary.unwrap(),
        first_rows: first_rows.unwrap(),
        second_error,
        wire_models: model.0.lock().unwrap().clone(),
        cost_events: cost_events.lock().unwrap().clone(),
    };
    assert_eq!(observed.first_summary.input_tokens, 1000);
    assert_eq!(observed.first_summary.output_tokens, 100);
    assert_eq!(observed.first_summary.cache_hit_tokens, 800);
    assert_eq!(observed.first_summary.cache_miss_tokens, 200);
    assert_eq!(observed.first_rows.len(), 1);
    assert!(observed.wire_models.iter().all(|model| model == MODEL));
    eprintln!("pricing activation audit: activation={activation:?} observed={observed:?}");
    observed
}

fn assert_priced_and_next_run_blocked(observed: &PricingObservation) {
    assert!(
        (observed.first_summary.session_cost - EXPECTED_IDLE_COST).abs() < 1e-12
            && observed.first_rows[0].cost_yuan == Some(EXPECTED_IDLE_COST)
            && observed.cost_events == [Some(EXPECTED_IDLE_COST)]
            && observed.wire_models == [MODEL]
            && observed
                .second_error
                .as_deref()
                .is_some_and(|error| error.contains("daily budget exhausted")),
        "successful settings activation did not reach the live cost/budget owner: {observed:?}"
    );
}

#[tokio::test]
async fn configured_startup_and_explicit_pricing_refresh_book_usage_and_enforce_budget() {
    for activation in [Activation::ConfiguredStartup, Activation::RefreshAfterLogin] {
        assert_priced_and_next_run_blocked(&observe_activation(activation).await);
    }
}

#[tokio::test]
#[ignore = "audit H-225: first login persists pricing without activating the existing CostService"]
async fn audit_first_login_activates_pricing_before_the_first_model_call() {
    assert_priced_and_next_run_blocked(&observe_activation(Activation::FirstLogin).await);
}
