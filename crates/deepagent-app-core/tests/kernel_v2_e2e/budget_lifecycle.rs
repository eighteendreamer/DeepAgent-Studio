//! Day/month limits must not silently become unlimited when their host is rebuilt.

use super::*;
use deepagent_app_core::{BudgetConfig, CostSummary};
use deepagent_core::clock::{Clock, SystemClock};
use deepagent_persistence::cost_store::CostStore;
use deepagent_persistence::document_store::DocumentStore;

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

async fn observe_rebuild(expected_budget: BudgetConfig, reapply: bool) -> LifecycleObservation {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("state.sqlite");
    let secrets = Arc::new(MemorySecretStore::new());
    let discovery = Arc::new(MockTransport::with_get_json(
        r#"{"data":[{"id":"deepseek-flash","object":"model","owned_by":"deepseek"}]}"#,
    ));
    let expected_error = if expected_budget.daily_limit.is_some() {
        "daily budget exhausted"
    } else {
        "monthly budget exhausted"
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
        crate::registry_fixture::configure(&settings, "offline-audit-placeholder")
            .await
            .unwrap();
        let pricing = settings.load().unwrap().unwrap().pricing_catalog.models;
        assert_eq!(pricing[MODEL].calculate(1000, 100, 0, 1000), CALL_COST);
        let cost = Arc::new(CostService::from_settings(db.clone(), settings.clone()));
        cost.set_budget(expected_budget.clone()).unwrap();
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
    let cost = Arc::new(CostService::from_settings(db.clone(), settings.clone()));
    assert_eq!(
        settings
            .harness_models()
            .selected_model()
            .unwrap()
            .unwrap()
            .model_name,
        MODEL
    );
    if reapply {
        cost.set_budget(expected_budget.clone()).unwrap();
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
    eprintln!("budget lifecycle audit: reapply={reapply} observed={observed:?}");
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
        let budget = BudgetConfig {
            daily_limit: (!monthly).then_some(0.001),
            monthly_limit: monthly.then_some(0.001),
        };
        assert_still_limited(observe_rebuild(budget, true).await);
    }
}

#[tokio::test]
async fn audit_daily_limit_survives_rebuilding_the_host_services() {
    assert_still_limited(
        observe_rebuild(
            BudgetConfig {
                daily_limit: Some(0.001),
                monthly_limit: None,
            },
            false,
        )
        .await,
    );
}

#[tokio::test]
async fn audit_monthly_limit_survives_rebuilding_the_host_services() {
    assert_still_limited(
        observe_rebuild(
            BudgetConfig {
                daily_limit: None,
                monthly_limit: Some(0.001),
            },
            false,
        )
        .await,
    );
}

#[tokio::test]
async fn both_limits_survive_rebuilding_the_host_services() {
    assert_still_limited(
        observe_rebuild(
            BudgetConfig {
                daily_limit: Some(0.001),
                monthly_limit: Some(0.001),
            },
            false,
        )
        .await,
    );
}

#[tokio::test]
async fn corrupt_policy_refuses_formal_admission_but_does_not_lose_incurred_usage() {
    let root = tempfile::tempdir().unwrap();
    let db = Arc::new(Database::open(root.path().join("state.sqlite")).unwrap());
    let settings = Arc::new(SettingsService::new(
        db.clone(),
        Arc::new(MockTransport::with_get_json(
            r#"{"data":[{"id":"deepseek-flash","object":"model","owned_by":"deepseek"}]}"#,
        )),
        Arc::new(MemorySecretStore::new()),
    ));
    crate::registry_fixture::configure(&settings, "synthetic-budget-fixture")
        .await
        .unwrap();
    let cost = Arc::new(CostService::from_settings(db.clone(), settings.clone()));
    cost.set_budget(BudgetConfig {
        daily_limit: Some(1.0),
        monthly_limit: None,
    })
    .unwrap();
    let model = Arc::new(UsageModel::default());
    let chat =
        ChatService::new(db.clone(), settings, model.clone(), root.path()).with_cost(cost.clone());
    let session = chat.create_session(None).unwrap();
    DocumentStore::new(&db)
        .put("settings", "cost-budget", "{", None, SystemClock.now())
        .unwrap();
    assert!(cost
        .summary(&session)
        .unwrap_err()
        .to_string()
        .contains("budget policy decode"));
    assert!(attempt(&chat, &session, "corrupt-budget-admission")
        .await
        .unwrap()
        .contains("budget policy decode"));
    assert_eq!(model.0.load(Ordering::SeqCst), 0);
    // In-flight usage may arrive after corruption. Recording must not depend
    // on successfully loading the admission policy, or real costs would vanish.
    assert_eq!(
        cost.record(deepagent_app_core::cost_service::CostRecordRequest {
            session_id: session.clone(),
            model: MODEL.into(),
            input_tokens: 1000,
            output_tokens: 100,
            cache_hit_tokens: 0,
            cache_miss_tokens: 1000,
            total_tokens: 1100,
        })
        .unwrap(),
        Some(CALL_COST)
    );
    assert_eq!(
        CostStore::new(&db).session_total(&session).unwrap(),
        CALL_COST
    );
    cost.set_budget(BudgetConfig::default()).unwrap();
    assert!(attempt(&chat, &session, "recovered-budget-admission")
        .await
        .is_none());
    assert_eq!(model.0.load(Ordering::SeqCst), 1);
    assert!((cost.summary(&session).unwrap().session_cost - CALL_COST * 2.0).abs() < 1e-12);
}

async fn run_budget_process_phase(root: &std::path::Path, phase: &str) {
    assert_eq!(
        std::fs::read_to_string(root.join("h227-process-fixture.marker")).unwrap(),
        "h227-isolated-fixture"
    );
    let db = Arc::new(Database::open(root.join("state.sqlite")).unwrap());
    let settings = Arc::new(SettingsService::new(
        db.clone(),
        Arc::new(MockTransport::with_get_json(
            r#"{"data":[{"id":"deepseek-flash","object":"model","owned_by":"deepseek"}]}"#,
        )),
        Arc::new(MemorySecretStore::new()),
    ));
    let cost = Arc::new(CostService::from_settings(db.clone(), settings.clone()));
    let restored_before_login = cost.budget().unwrap();
    // Each fresh process owns a new synthetic credential store and performs a
    // deliberate credential change. Ledger scope must not be reset by login.
    crate::registry_fixture::configure(&settings, &format!("synthetic-budget-{phase}"))
        .await
        .unwrap();
    assert_eq!(cost.budget().unwrap(), restored_before_login);
    let expected = BudgetConfig {
        daily_limit: Some(0.001),
        monthly_limit: Some(0.001),
    };
    let model = Arc::new(UsageModel::default());
    let chat =
        ChatService::new(db.clone(), settings.clone(), model.clone(), root).with_cost(cost.clone());
    match phase {
        "establish" => {
            assert_eq!(restored_before_login, BudgetConfig::default());
            cost.set_budget(expected).unwrap();
            let session = chat.create_session(None).unwrap();
            assert!(attempt(&chat, &session, "budget-process-first")
                .await
                .is_none());
            assert_eq!(model.0.load(Ordering::SeqCst), 1);
            assert_eq!(cost.summary(&session).unwrap().session_cost, CALL_COST);
            std::fs::write(root.join("session-id"), session).unwrap();
        }
        "enforce" => {
            assert_eq!(restored_before_login, expected);
            let session = std::fs::read_to_string(root.join("session-id")).unwrap();
            let summary = cost.summary(&session).unwrap();
            assert_eq!(summary.budget, expected);
            assert_eq!(summary.session_cost, CALL_COST);
            assert_eq!(summary.input_tokens, 1000);
            assert!(attempt(&chat, &session, "budget-process-blocked")
                .await
                .unwrap()
                .contains("daily budget exhausted"));
            assert_eq!(model.0.load(Ordering::SeqCst), 0);
            assert_eq!(
                CostStore::new(&db)
                    .session_costs_detailed(&session)
                    .unwrap()
                    .len(),
                1
            );
            cost.set_budget(BudgetConfig::default()).unwrap();
        }
        "cleared" => {
            assert_eq!(restored_before_login, BudgetConfig::default());
            let session = std::fs::read_to_string(root.join("session-id")).unwrap();
            assert_eq!(cost.summary(&session).unwrap().session_cost, CALL_COST);
            assert!(attempt(&chat, &session, "budget-process-after-clear")
                .await
                .is_none());
            assert_eq!(model.0.load(Ordering::SeqCst), 1);
            assert!((cost.summary(&session).unwrap().session_cost - CALL_COST * 2.0).abs() < 1e-12);
            let rows = CostStore::new(&db)
                .session_costs_detailed(&session)
                .unwrap();
            assert_eq!(rows.len(), 2);
            assert_eq!(rows[0].cost_yuan, Some(CALL_COST));
            assert_eq!(
                CostStore::new(&db)
                    .session_usage(&session)
                    .unwrap()
                    .input_tokens,
                2000
            );
        }
        _ => panic!("unexpected fixture phase"),
    }
    assert!(
        chat.wait_for_session_memory_passes(std::time::Duration::from_secs(1))
            .await
    );
    drop(chat);
    drop(cost);
    drop(settings);
    assert_eq!(Arc::strong_count(&db), 1);
}

#[tokio::test]
async fn budget_and_history_survive_real_host_process_restarts() {
    const PHASE: &str = "DEEPAGENT_H227_BUDGET_PHASE";
    if let Ok(phase) = std::env::var(PHASE) {
        run_budget_process_phase(&std::env::current_dir().unwrap(), &phase).await;
        return;
    }
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join("h227-process-fixture.marker"),
        "h227-isolated-fixture",
    )
    .unwrap();
    for phase in ["establish", "enforce", "cleared"] {
        let mut child = tokio::process::Command::new(std::env::current_exe().unwrap());
        child
            .args([
                "--exact",
                "budget_lifecycle::budget_and_history_survive_real_host_process_restarts",
                "--nocapture",
                "--test-threads=1",
            ])
            .env_clear()
            .current_dir(root.path())
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
        child.env(PHASE, phase);
        #[cfg(windows)]
        child.creation_flags(0x08000000);
        let output = tokio::time::timeout(std::time::Duration::from_secs(40), child.output())
            .await
            .expect("budget host fixture must exit within 40s")
            .unwrap();
        assert!(
            output.status.success(),
            "budget process phase {phase} failed: stdout={}, stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        eprintln!("budget host process {phase} exited successfully");
    }
}
