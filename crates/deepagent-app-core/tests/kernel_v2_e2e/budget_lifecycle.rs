//! Day/month limits must not silently become unlimited when their host is rebuilt.

use super::*;
use deepagent_app_core::{BudgetConfig, CostSummary};
use deepagent_persistence::cost_store::CostStore;

const MODEL: &str = "deepseek-flash";
const CALL_COST: f64 = 0.0014;

#[derive(Debug, Default)]
struct UsageModel(AtomicUsize);

#[async_trait]
impl HttpTransport for UsageModel {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        let body: serde_json::Value = serde_json::from_str(&request.body)?;
        assert_eq!(body["model"], MODEL);
        self.0.fetch_add(1, Ordering::SeqCst);
        emit_responses_events(
            sink,
            &answer_with_usage("AUDIT_BUDGET_LIFECYCLE_COMPLETE", 1000, 100),
        )
    }
}

async fn attempt(chat: &ChatService, session: &str, run: &str) -> Option<String> {
    let result = chat
        .run_in_session(
            "Reply briefly.",
            Some(session),
            None,
            None,
            Vec::new(),
            None,
            false,
            Some(run),
            |_| {},
            |_| {},
        )
        .await;
    if result.is_ok() {
        let terminals: Vec<_> = chat
            .run_events(run, None)
            .unwrap()
            .into_iter()
            .filter(|event| event.event_type == "run_terminal")
            .collect();
        assert_eq!(terminals.len(), 1);
        assert_eq!(terminals[0].data["kind"], "succeeded");
    }
    result.err().map(|error| error.to_string())
}

#[derive(Debug)]
struct LifecycleObservation {
    expected_budget: BudgetConfig,
    restored: CostSummary,
    next_error: Option<String>,
    next_dispatches: usize,
}

async fn observe_rebuild(monthly: bool, reapply: bool) -> LifecycleObservation {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("state.sqlite");
    let secrets = Arc::new(MemorySecretStore::new());
    let discovery = Arc::new(MockTransport::with_get_json(
        r#"{"data":[{"id":"deepseek-flash","object":"model","owned_by":"deepseek"}]}"#,
    ));
    let expected_budget = BudgetConfig {
        daily_limit: (!monthly).then_some(0.001),
        monthly_limit: monthly.then_some(0.001),
    };
    let expected_error = if monthly {
        "monthly budget exhausted"
    } else {
        "daily budget exhausted"
    };

    // First host: real completed usage populates the ledger, then a second run
    // is refused at admission. No manual SQL insertion or clock changes.
    let session = {
        let db = Arc::new(Database::open(&path).unwrap());
        let settings = Arc::new(SettingsService::new(
            db.clone(),
            discovery.clone(),
            secrets.clone(),
        ));
        settings
            .initialize("offline-audit-placeholder")
            .await
            .unwrap();
        let pricing = settings.load().unwrap().unwrap().pricing_catalog.models;
        assert_eq!(pricing[MODEL].calculate(1000, 100, 0, 1000), CALL_COST);
        let cost = Arc::new(CostService::new(db.clone()));
        cost.set_pricing(pricing);
        cost.set_budget(expected_budget.clone());
        assert_eq!(cost.summary("").unwrap().budget, expected_budget);
        let model = Arc::new(UsageModel::default());
        let chat = ChatService::new(db.clone(), settings.clone(), model.clone(), root.path())
            .with_cost(cost.clone());
        let session = chat.create_session(None).unwrap();
        assert!(attempt(&chat, &session, "audit-budget-before-first")
            .await
            .is_none());
        assert!(attempt(&chat, &session, "audit-budget-before-blocked")
            .await
            .is_some_and(|error| error.contains(expected_error)));
        assert_eq!(model.0.load(Ordering::SeqCst), 1);
        let summary = cost.summary(&session).unwrap();
        assert_eq!(summary.session_cost, CALL_COST);
        assert_eq!(summary.today_cost, CALL_COST);
        assert_eq!(summary.month_cost, CALL_COST);
        assert_eq!(summary.input_tokens, 1000);
        assert_eq!(summary.output_tokens, 100);
        assert!(
            chat.wait_for_session_memory_passes(std::time::Duration::from_secs(1))
                .await
        );
        drop(chat);
        drop(settings);
        drop(cost);
        // Ensure the old Database owner actually closes before reopening it.
        assert_eq!(Arc::strong_count(&db), 1);
        drop(db);
        session
    };

    // Second host: recreate services from the same file, just as startup does.
    // The fixture secret backend outlives both service compositions (like an
    // external credential store); no old settings/cost/database handle is reused.
    let db = Arc::new(Database::open(&path).unwrap());
    let settings = Arc::new(SettingsService::new(db.clone(), discovery, secrets));
    let cost = Arc::new(CostService::new(db.clone()));
    let view = settings.view().unwrap().unwrap();
    assert_eq!(view.chat_model, MODEL);
    cost.set_pricing(view.pricing_catalog.models);
    if reapply {
        cost.set_budget(expected_budget.clone());
    }
    let restored = cost.summary(&session).unwrap();
    assert_eq!(restored.session_cost, CALL_COST);
    assert_eq!(restored.today_cost, CALL_COST);
    assert_eq!(restored.month_cost, CALL_COST);
    assert_eq!(restored.input_tokens, 1000);
    assert_eq!(
        CostStore::new(&db)
            .session_costs_detailed(&session)
            .unwrap()
            .len(),
        1
    );
    let model = Arc::new(UsageModel::default());
    let chat =
        ChatService::new(db.clone(), settings, model.clone(), root.path()).with_cost(cost.clone());
    let next_error = attempt(&chat, &session, "audit-budget-after-rebuild").await;
    let next_dispatches = model.0.load(Ordering::SeqCst);
    let expected_total = CALL_COST * (1 + next_dispatches) as f64;
    assert!((cost.summary(&session).unwrap().session_cost - expected_total).abs() < 1e-12);
    let observed = LifecycleObservation {
        expected_budget,
        restored,
        next_error,
        next_dispatches,
    };
    eprintln!("budget lifecycle audit: monthly={monthly} reapply={reapply} observed={observed:?}");
    observed
}

fn assert_still_limited(observed: LifecycleObservation) {
    assert!(
        observed.restored.budget == observed.expected_budget
            && observed
                .next_error
                .as_deref()
                .is_some_and(|error| error.contains("budget exhausted"))
            && observed.next_dispatches == 0,
        "retained spend must still be subject to the configured period limit: {observed:?}"
    );
}

#[tokio::test]
async fn explicitly_reapplying_limits_after_rebuild_restores_admission_controls() {
    for monthly in [false, true] {
        assert_still_limited(observe_rebuild(monthly, true).await);
    }
}

#[tokio::test]
#[ignore = "audit H-227: a configured daily limit is lost when the cost owner is rebuilt"]
async fn audit_daily_limit_survives_rebuilding_the_host_services() {
    assert_still_limited(observe_rebuild(false, false).await);
}

#[tokio::test]
#[ignore = "audit H-227: a configured monthly limit is lost when the cost owner is rebuilt"]
async fn audit_monthly_limit_survives_rebuilding_the_host_services() {
    assert_still_limited(observe_rebuild(true, false).await);
}
