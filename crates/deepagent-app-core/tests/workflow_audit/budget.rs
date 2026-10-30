//! H-272: production workflow is a direct consumer of the shared accounting
//! seam. Only loopback providers and synthetic prices are used here.

use super::*;
use deepagent_app_core::BudgetConfig;
use deepagent_persistence::cost_store::CostStore;

#[tokio::test]
async fn measured_daily_and_monthly_spend_stops_the_next_workflow_node() {
    for monthly in [false, true] {
        let provider = Provider::start(vec![
            (200, Some(completion(100, 20, 80, 12))),
            (200, Some(completion(100, 20, 80, 12))),
        ])
        .await;
        let f = Fixture::new().await;
        let selected = f.register(&provider, "openai", "audit-a", 0);
        f.cost
            .set_budget(BudgetConfig {
                daily_limit: (!monthly).then_some(0.0001),
                monthly_limit: monthly.then_some(0.0001),
            })
            .unwrap();
        let (ok, events) = f
            .run("measured-budget", workflow(&[&selected, &selected]))
            .await;
        assert!(!ok, "a budget stop is not a successful final answer");
        assert_eq!(provider.calls.lock().unwrap().len(), 1);
        assert_terminal(&f.db, "measured-budget", "budget_exceeded");
        assert_eq!(
            CostStore::new(&f.db)
                .session_costs_detailed(&f.session)
                .unwrap()
                .len(),
            1
        );
        let summary = f.cost.summary(&f.session).unwrap();
        assert_eq!(
            (
                summary.input_tokens,
                summary.output_tokens,
                summary.cache_hit_tokens
            ),
            (100, 20, 80)
        );
        assert!((summary.session_cost - 0.00018).abs() < 1e-12);
        let notices: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                RuntimeEvent::CostRecorded { cost_yuan, .. } => Some(*cost_yuan),
                _ => None,
            })
            .collect();
        assert_eq!(notices.len(), 1);
        assert!(notices[0].is_some_and(|value| (value - 0.00018).abs() < 1e-12));
        let (_root, db, session) = f.reopen();
        assert_terminal(&db, "measured-budget", "budget_exceeded");
        assert_eq!(
            recorded_usage(&db, &session)[0]["raw"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
    }
}

#[tokio::test]
async fn measured_transient_failure_cannot_dispatch_another_paid_candidate_after_limit() {
    for limited in [false, true] {
        let failed = Provider::start(vec![(503, Some(completion(100, 20, 80, 12)))]).await;
        let fallback = Provider::start(vec![(200, Some(completion(100, 20, 80, 12)))]).await;
        let f = Fixture::new().await;
        // Existing scenario routing sorts larger priorities first.
        f.register(&failed, "openai", "audit-a", 10);
        f.register(&fallback, "openai", "audit-b", 0);
        f.cost
            .set_budget(BudgetConfig {
                daily_limit: Some(if limited { 0.0001 } else { 1.0 }),
                monthly_limit: None,
            })
            .unwrap();
        let (ok, _) = f.run("fallback-budget", workflow(&[""])).await;
        assert_eq!(failed.calls.lock().unwrap().len(), 1);
        assert_eq!(ok, !limited);
        assert_eq!(fallback.calls.lock().unwrap().len(), usize::from(!limited));
        assert_terminal(
            &f.db,
            "fallback-budget",
            if limited {
                "budget_exceeded"
            } else {
                "succeeded"
            },
        );
        assert_eq!(
            CostStore::new(&f.db)
                .session_costs_detailed(&f.session)
                .unwrap()
                .len(),
            if limited { 1 } else { 2 }
        );
        assert_eq!(
            f.cost.summary(&f.session).unwrap().input_tokens,
            if limited { 100 } else { 200 }
        );
    }
}
