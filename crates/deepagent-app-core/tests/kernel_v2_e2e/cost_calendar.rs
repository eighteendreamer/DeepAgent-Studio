//! Calendar budget probes use historical synthetic ledger rows, never real billing.

use super::*;
use deepagent_app_core::{BudgetConfig, CostSummary};
use deepagent_persistence::cost_store::{CostEntry, CostStore};
use jiff::{tz::TimeZone, Timestamp};
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
    summary: Result<CostSummary>,
    budget_error: Option<String>,
    run_error: Option<String>,
    model_calls: usize,
}

fn period_budget(monthly: bool) -> BudgetConfig {
    BudgetConfig {
        daily_limit: (!monthly).then_some(5.0),
        monthly_limit: monthly.then_some(5.0),
    }
}

async fn observe_budget(budget: BudgetConfig, rows: &[(i64, f64)]) -> BudgetObservation {
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
    cost.set_budget(budget);
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
    let summary = cost.summary(&session);
    let budget_error = cost.check_budget().err().map(|error| error.to_string());
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
        assert_eq!(
            terminals,
            ["failed"],
            "admission failure must finalize once"
        );
    }
    BudgetObservation {
        summary,
        budget_error,
        run_error,
        model_calls,
    }
}

fn millis(at: OffsetDateTime) -> i64 {
    i64::try_from(at.unix_timestamp_nanos() / 1_000_000).unwrap()
}

async fn audit_boundary(monthly: bool) {
    // The original differential probe's real red/green evidence is preserved
    // in the audit. The default regression tests the contract on any host/date,
    // including UTC, without requiring the former wrong bucket to differ.
    let now = millis(OffsetDateTime::now_utc());
    let local = Timestamp::from_millisecond(now)
        .unwrap()
        .to_zoned(TimeZone::try_system().unwrap());
    let date = if monthly {
        local.date().first_of_month()
    } else {
        local.date()
    };
    let calendar_start = date
        .to_zoned(local.time_zone().clone())
        .unwrap()
        .timestamp()
        .as_millisecond();
    assert!(calendar_start <= now);
    let period_cost = |summary: &CostSummary| {
        if monthly {
            summary.month_cost
        } else {
            summary.today_cost
        }
    };
    // Prior-period spend is visible in the ledger but cannot exhaust this period.
    let control = observe_budget(
        period_budget(monthly),
        &[(calendar_start - 1, 12.0), (calendar_start, 1.0)],
    )
    .await;
    let summary = control.summary.as_ref().unwrap();
    assert_eq!(summary.session_cost, 13.0);
    assert_eq!(summary.total_cost, 13.0);
    assert_eq!(period_cost(summary), 1.0);
    assert!(control.budget_error.is_none() && control.run_error.is_none());

    // Include the boundary itself, aggregate historical/current spend, and
    // reject equality with the limit before issuing any model request.
    let observed =
        observe_budget(period_budget(monthly), &[(calendar_start, 4.0), (now, 1.0)]).await;
    let summary = observed.summary.as_ref().unwrap();
    assert_eq!(summary.session_cost, 5.0);
    assert_eq!(summary.total_cost, 5.0);
    assert_eq!(period_cost(summary), 5.0);
    let expected = if monthly {
        "monthly budget exhausted"
    } else {
        "daily budget exhausted"
    };
    assert!(observed.budget_error.as_deref().unwrap().contains(expected));
    assert!(observed.run_error.as_deref().unwrap().contains(expected));
    assert_eq!(observed.model_calls, 0);
}

#[tokio::test]
async fn current_cost_blocks_daily_and_monthly_model_dispatch() {
    for monthly in [false, true] {
        let observed = observe_budget(
            period_budget(monthly),
            &[(millis(OffsetDateTime::now_utc()), 12.0)],
        )
        .await;
        assert!(observed.summary.is_ok());
        assert!(observed.budget_error.is_some());
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
async fn audit_monthly_budget_uses_calendar_month_on_real_run_admission() {
    audit_boundary(true).await;
}

#[tokio::test]
async fn audit_daily_budget_uses_local_midnight_on_real_run_admission() {
    audit_boundary(false).await;
}

#[tokio::test]
async fn invalid_timezone_refuses_finite_budgets_before_model_dispatch() {
    const CHILD: &str = "DEEPAGENT_H224_INVALID_TZ_CHILD";
    const INVALID_TZ: &str = "DeepAgent/H224_Missing_Zone";
    if std::env::var_os(CHILD).is_some() {
        assert_eq!(std::env::var("TZ").unwrap(), INVALID_TZ);
        for monthly in [false, true] {
            let observed = observe_budget(period_budget(monthly), &[]).await;
            assert!(observed
                .summary
                .unwrap_err()
                .to_string()
                .contains("cost calendar system timezone"));
            assert!(observed
                .budget_error
                .as_deref()
                .unwrap()
                .contains("cost calendar system timezone"));
            assert!(observed
                .run_error
                .as_deref()
                .unwrap()
                .contains("cost calendar system timezone"));
            assert_eq!(observed.model_calls, 0);
        }
        // Preserve the existing no-budget fast path; failure to render a
        // summary must not silently introduce a new limit for unlimited runs.
        let unlimited = observe_budget(BudgetConfig::default(), &[]).await;
        assert!(unlimited.summary.is_err());
        assert!(unlimited.budget_error.is_none() && unlimited.run_error.is_none());
        assert_eq!(unlimited.model_calls, 1);
        return;
    }

    // Never mutate process-global TZ or the host clock. A fresh child also
    // avoids Jiff's process-local system-timezone cache masking invalid TZ.
    let root = tempfile::tempdir().unwrap();
    let mut child = tokio::process::Command::new(std::env::current_exe().unwrap());
    child
        .args([
            "--exact",
            "cost_calendar::invalid_timezone_refuses_finite_budgets_before_model_dispatch",
            "--nocapture",
            "--test-threads=1",
        ])
        .env_clear()
        .kill_on_drop(true);
    for name in [
        "PATH",
        "SystemRoot",
        "WINDIR",
        "SystemDrive",
        "PATHEXT",
        "COMSPEC",
    ] {
        if let Some(value) = std::env::var_os(name) {
            child.env(name, value);
        }
    }
    child.env(CHILD, "1").env("TZ", INVALID_TZ);
    for name in [
        "HOME",
        "USERPROFILE",
        "TEMP",
        "TMP",
        "TMPDIR",
        "APPDATA",
        "LOCALAPPDATA",
        "XDG_CONFIG_HOME",
        "DEEPAGENT_HOME",
        "DEEPAGENT_DATA_ROOT",
        "DEEPAGENT_MANAGED_SETTINGS_DIR",
    ] {
        child.env(name, root.path());
    }
    #[cfg(windows)]
    child.creation_flags(0x08000000);
    let output = tokio::time::timeout(std::time::Duration::from_secs(20), child.output())
        .await
        .expect("isolated timezone child must exit within 20s")
        .unwrap();
    assert!(
        output.status.success(),
        "isolated timezone child failed: stdout={}, stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
