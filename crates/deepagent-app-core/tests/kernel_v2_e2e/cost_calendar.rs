//! Calendar budget probes use historical synthetic ledger rows, never real billing.

use super::*;
use deepagent_app_core::{BudgetConfig, CostSummary};
use deepagent_persistence::cost_store::{CostEntry, CostStore};
use time::OffsetDateTime;

#[derive(Debug, Default)]
struct BudgetModel(AtomicUsize);

#[async_trait]
impl HttpTransport for BudgetModel {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        let body: serde_json::Value = serde_json::from_str(&request.body)?;
        assert_eq!(body["model"], "deepseek-v4-flash");
        self.0.fetch_add(1, Ordering::SeqCst);
        emit_responses_events(sink, &response_text("AUDIT_CALENDAR_BUDGET_COMPLETE"))
    }
}

#[derive(Debug)]
struct BudgetObservation {
    summary: CostSummary,
    budget_allowed: bool,
    run_error: Option<String>,
    model_calls: usize,
    terminals: Vec<String>,
}

async fn observe_budget(monthly: bool, rows: &[(i64, f64)]) -> BudgetObservation {
    let root = tempfile::tempdir().unwrap();
    let db = Arc::new(Database::open(root.path().join("state.sqlite")).unwrap());
    let settings = Arc::new(SettingsService::new(
        db.clone(),
        Arc::new(MockTransport::with_get_json(
            r#"{"data":[{"id":"deepseek-v4-flash","object":"model","owned_by":"deepseek"}]}"#,
        )),
        Arc::new(MemorySecretStore::new()),
    ));
    settings
        .initialize("offline-audit-placeholder")
        .await
        .unwrap();
    let cost = Arc::new(CostService::new(db.clone()));
    cost.set_budget(BudgetConfig {
        daily_limit: (!monthly).then_some(5.0),
        monthly_limit: monthly.then_some(5.0),
    });
    let model = Arc::new(BudgetModel::default());
    let chat =
        ChatService::new(db.clone(), settings, model.clone(), root.path()).with_cost(cost.clone());
    let session = chat.create_session(None).unwrap();
    for &(timestamp, amount) in rows {
        CostStore::new(&db)
            .insert(&CostEntry {
                session_id: &session,
                timestamp,
                model: "deepseek-v4-flash",
                input_tokens: 1,
                output_tokens: 0,
                cache_hit_tokens: 0,
                cache_miss_tokens: 1,
                total_tokens: 1,
                cost_yuan: Some(amount),
                input_cache_hit_price: None,
                input_cache_miss_price: None,
                output_price: None,
                pricing_source: Some("audit-synthetic-historical-cost"),
            })
            .unwrap();
    }
    let summary = cost.summary(&session).unwrap();
    let budget_allowed = cost.check_budget().is_ok();
    let run_error = chat
        .run_in_session(
            "Reply with a short acknowledgement.",
            Some(&session),
            None,
            None,
            Vec::new(),
            None,
            false,
            Some("audit-calendar-budget"),
            |_| {},
            |_| {},
        )
        .await
        .err()
        .map(|error| error.to_string());
    let terminals: Vec<String> = chat
        .run_events("audit-calendar-budget", None)
        .unwrap()
        .iter()
        .filter(|event| event.event_type == "run_terminal")
        .map(|event| event.data["kind"].as_str().unwrap().to_string())
        .collect();
    let model_calls = model.0.load(Ordering::SeqCst);
    if run_error.is_none() {
        assert_eq!(model_calls, 1);
        assert_eq!(terminals, ["succeeded"]);
    } else {
        assert_eq!(model_calls, 0, "budget rejection must precede dispatch");
    }
    BudgetObservation {
        summary,
        budget_allowed,
        run_error,
        model_calls,
        terminals,
    }
}

fn millis(at: OffsetDateTime) -> i64 {
    i64::try_from(at.unix_timestamp_nanos() / 1_000_000).unwrap()
}

async fn audit_boundary(monthly: bool) {
    // Use the actual clock; do not patch SystemClock or change the host timezone.
    let local = OffsetDateTime::now_local().expect("probe needs host local offset");
    let now = millis(local);
    let date = if monthly {
        local.date().replace_day(1).unwrap()
    } else {
        local.date()
    };
    let calendar_start = millis(date.midnight().assume_offset(local.offset()));
    let bucket_ms = 86_400_000 * if monthly { 30i64 } else { 1i64 };
    let bucket_start = now - now % bucket_ms;
    assert_ne!(
        calendar_start, bucket_start,
        "probe precondition: calendar/bucket boundaries must differ on this host/date"
    );
    let gap_timestamp =
        calendar_start.min(bucket_start) + (calendar_start - bucket_start).abs() / 2;
    let common_start = calendar_start.max(bucket_start);
    let retained_timestamp = common_start + (now - common_start) / 2;
    assert!(gap_timestamp < now && retained_timestamp <= now);

    let control = observe_budget(monthly, &[(retained_timestamp, 1.0)]).await;
    assert!(control.budget_allowed && control.run_error.is_none());
    let observed =
        observe_budget(monthly, &[(retained_timestamp, 1.0), (gap_timestamp, 12.0)]).await;
    assert_eq!(observed.summary.session_cost, 13.0);
    assert_eq!(observed.summary.total_cost, 13.0);
    let expected_cost = if gap_timestamp >= calendar_start {
        13.0
    } else {
        1.0
    };
    let actual_cost = if monthly {
        observed.summary.month_cost
    } else {
        observed.summary.today_cost
    };
    eprintln!(
        "calendar audit: monthly={monthly} now={local} calendar_start={} bucket_start={} gap={} expected={expected_cost} actual={actual_cost} budget_allowed={} calls={} terminals={:?} run_error={:?}",
        OffsetDateTime::from_unix_timestamp_nanos(i128::from(calendar_start) * 1_000_000).unwrap(),
        OffsetDateTime::from_unix_timestamp_nanos(i128::from(bucket_start) * 1_000_000).unwrap(),
        OffsetDateTime::from_unix_timestamp_nanos(i128::from(gap_timestamp) * 1_000_000).unwrap(),
        observed.budget_allowed, observed.model_calls, observed.terminals, observed.run_error,
    );
    assert!(
        actual_cost == expected_cost
            && observed.budget_allowed == (expected_cost < 5.0)
            && observed.run_error.is_none() == (expected_cost < 5.0),
        "calendar cost and real run admission disagree with historical ledger: {observed:?}"
    );
}

#[tokio::test]
async fn current_cost_blocks_daily_and_monthly_model_dispatch() {
    for monthly in [false, true] {
        let observed = observe_budget(monthly, &[(millis(OffsetDateTime::now_utc()), 12.0)]).await;
        assert!(!observed.budget_allowed);
        let error = observed.run_error.as_deref().expect("budget must reject");
        assert!(error.contains(if monthly {
            "monthly budget"
        } else {
            "daily budget"
        }));
        assert_eq!(observed.model_calls, 0);
    }
}

#[tokio::test]
#[ignore = "audit H-224: monthly calendar cost is replaced by an epoch-aligned 30-day bucket"]
async fn audit_monthly_budget_uses_calendar_month_on_real_run_admission() {
    audit_boundary(true).await;
}

#[tokio::test]
#[ignore = "audit H-224: daily cost ignores the documented host-local midnight"]
async fn audit_daily_budget_uses_local_midnight_on_real_run_admission() {
    audit_boundary(false).await;
}
