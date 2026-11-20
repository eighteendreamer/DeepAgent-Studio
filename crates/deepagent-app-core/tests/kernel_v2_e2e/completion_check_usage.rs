//! H-63: production ChatService assembly, real tool mutation and file SQLite.
use super::*;
use deepagent_app_core::{ApprovalPolicy, BudgetConfig, ExecutionFeatures, ModelPricing};
use deepagent_persistence::{cost_store::CostStore, run_store::RunStore};
use deepagent_runtime::events::{CostOrigin, RuntimeEvent};
use serde_json::{json, Value};

const RUN: &str = "completion-check-usage";

#[derive(Default)]
struct Model {
    main: AtomicUsize,
    panel: AtomicUsize,
    stall: AtomicUsize,
}

#[async_trait]
impl HttpTransport for Model {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        let body: Value = serde_json::from_str(&request.body)?;
        let instructions = body["instructions"].as_str().unwrap_or_default();
        let (item, input, output, cached) = if instructions
            == deepagent_app_core::verification_panel::GOAL_COVERAGE_SKEPTIC_PROMPT
        {
            self.panel.fetch_add(1, Ordering::SeqCst);
            (
                super::responses_fixture::text("skeptic-text", "VERDICT: PASS"),
                101,
                7,
                80,
            )
        } else if instructions == deepagent_runtime::stall_detector::STALL_CLASSIFIER_PROMPT {
            self.stall.fetch_add(1, Ordering::SeqCst);
            (
                super::responses_fixture::text(
                    "stall-text",
                    r#"{"category":"not_stalled_complete","confidence":0.9,"evidence":"offline"}"#,
                ),
                101,
                7,
                80,
            )
        } else {
            let index = self.main.fetch_add(1, Ordering::SeqCst);
            assert!(index < 3, "unexpected extra main request");
            let item = if index == 0 {
                super::responses_fixture::call(
                    "write-answer",
                    "write_file",
                    json!({"path":"answer.txt","content":"offline evidence"}),
                )
            } else {
                super::responses_fixture::text("main-text", "Created answer.txt.")
            };
            (item, 11, 3, 2)
        };
        emit_responses_events(
            sink,
            &super::responses_fixture::completed(
                "completion-usage",
                body["model"].as_str().unwrap(),
                item,
                json!({"input_tokens":input,"output_tokens":output,"total_tokens":input+output,
                "input_tokens_details":{"cached_tokens":cached}}),
            ),
        )
    }
}

async fn observe(panel: bool, stall: bool, exhausted_after_answer: bool) {
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
    crate::registry_fixture::configure(&settings, "offline-placeholder")
        .await
        .unwrap();
    settings
        .set_approval_policy(ApprovalPolicy::FullAccess)
        .unwrap();
    settings
        .set_execution_features(ExecutionFeatures {
            adversarial_verify: panel,
            stall_detector: stall,
            ..Default::default()
        })
        .unwrap();
    let cost = Arc::new(CostService::new(db.clone()));
    cost.set_pricing(std::collections::HashMap::from([(
        "deepseek-flash".into(),
        ModelPricing {
            input_cache_hit_per_million: 1.0,
            input_cache_miss_per_million: 1.0,
            output_per_million: 1.0,
        },
    )]));
    if exhausted_after_answer {
        cost.set_budget(BudgetConfig {
            daily_limit: Some(0.000028),
            monthly_limit: None,
        })
        .unwrap();
    }
    let model = Arc::new(Model::default());
    let chat =
        ChatService::new(db.clone(), settings, model.clone(), root.path()).with_cost(cost.clone());
    let captured = Arc::new(Mutex::new(Vec::new()));
    let events = captured.clone();
    let result = chat
        .run_in_session(
            "Create answer.txt containing offline evidence, then finish.",
            None,
            None,
            None,
            Vec::new(),
            None,
            false,
            Some(RUN),
            move |event| {
                events.lock().unwrap().push(event);
            },
            |_| {},
        )
        .await;
    assert!(
        result.is_ok(),
        "optional checks must not hard-fail a paid final answer: {result:?}"
    );
    let session = result.unwrap();
    assert_eq!(
        std::fs::read_to_string(root.path().join("answer.txt")).unwrap(),
        "offline evidence"
    );
    assert_eq!(model.main.load(Ordering::SeqCst), 2);
    let expected_panel = if panel && !exhausted_after_answer {
        3
    } else {
        0
    };
    let expected_stall = usize::from(stall && !exhausted_after_answer);
    assert_eq!(model.panel.load(Ordering::SeqCst), expected_panel);
    assert_eq!(model.stall.load(Ordering::SeqCst), expected_stall);
    let side = expected_panel + expected_stall;
    let summary = cost.summary(&session).unwrap();
    assert_eq!(
        (
            summary.input_tokens,
            summary.output_tokens,
            summary.cache_hit_tokens
        ),
        (
            22 + side as u64 * 101,
            6 + side as u64 * 7,
            4 + side as u64 * 80
        )
    );
    let reopened = Database::open(&path).unwrap();
    let store = CostStore::new(&reopened);
    assert_eq!(
        store.session_costs_detailed(&session).unwrap().len(),
        2 + side
    );
    assert_eq!(
        store.session_usage(&session).unwrap().input_tokens,
        summary.input_tokens
    );
    let runs = RunStore::new(&reopened);
    assert_eq!(
        runs.get(RUN).unwrap().unwrap().terminal_kind.as_deref(),
        Some("succeeded")
    );
    let durable = runs.events_after(RUN, None).unwrap();
    assert_eq!(
        durable
            .iter()
            .filter(|event| event.event_type == "run_terminal")
            .count(),
        1
    );
    let live = captured.lock().unwrap();
    assert_eq!(
        live.iter()
            .filter(|event| matches!(
                event,
                RuntimeEvent::CostRecorded {
                    origin: CostOrigin::Run,
                    ..
                }
            ))
            .count(),
        1
    );
    assert_eq!(
        live.iter()
            .filter(|event| matches!(
                event,
                RuntimeEvent::CostRecorded {
                    origin: CostOrigin::SideCall,
                    ..
                }
            ))
            .count(),
        side
    );
    let receipts = store.run_call_receipts(&session, RUN).unwrap();
    assert_eq!(
        receipts
            .iter()
            .filter(|receipt| receipt.report["origin"] == "run")
            .count(),
        2
    );
    assert_eq!(
        receipts
            .iter()
            .filter(|receipt| receipt.report["origin"] == "side_call")
            .count(),
        side
    );
}

#[tokio::test]
async fn disabled_panel_classifier_and_combined_enabled_consumers_share_the_production_owner() {
    for (panel, stall) in [(false, false), (true, false), (false, true), (true, true)] {
        observe(panel, stall, false).await;
    }
}

#[tokio::test]
async fn optional_check_admission_rejection_does_not_refute_the_already_paid_answer() {
    for (panel, stall) in [(false, true), (true, false), (true, true)] {
        observe(panel, stall, true).await;
    }
}
