use super::*;

// UserPromptSubmit returns before AppRunFinalizer. Its answered requests must
// still be durable, even though no main-model request or UsageRecorded exists.
async fn assert_rejected_prompt_is_billed(kind: HookKind, expected_calls: usize) {
    let temporary = tempfile::tempdir().unwrap();
    let workspace = temporary.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let path = temporary.path().join("state.sqlite");
    let db = Arc::new(Database::open(&path).unwrap());
    let settings = hook_settings(db.clone(), HookKind::None).await;
    settings
        .set_hooks_json(
            &json!({"hooks":{"UserPromptSubmit":[{"hooks":[{
                "type":if kind.is_agent() {"agent"} else {"prompt"},
                "prompt":MARKER,"timeout":5
            }]}]}})
            .to_string(),
        )
        .unwrap();
    let unit = settings.load().unwrap().unwrap().pricing_catalog.models[MODEL]
        .calculate(1000, 100, 800, 200);
    let cost = Arc::new(CostService::from_settings(db.clone(), settings.clone()));
    cost.set_budget(BudgetConfig {
        daily_limit: Some(unit * (expected_calls as f64 - 0.5)),
        monthly_limit: None,
    })
    .unwrap();
    let model = Arc::new(UsageModel {
        kind,
        parent: Mutex::new(Vec::new()),
        hooks: Mutex::new(Vec::new()),
    });
    let chat = ChatService::new(db.clone(), settings.clone(), model.clone(), &workspace)
        .with_cost(cost.clone());
    let captured = Arc::new(Mutex::new(Vec::new()));
    let events = captured.clone();
    let session = chat
        .run_in_session(
            "Controlled early rejection.",
            None,
            None,
            None,
            Vec::new(),
            None,
            false,
            Some("h235-early-gate"),
            move |event| events.lock().unwrap().push(event),
            |_| {},
        )
        .await
        .unwrap();
    assert!(model.parent.lock().unwrap().is_empty());
    assert_eq!(model.hooks.lock().unwrap().len(), expected_calls);
    let events = captured.lock().unwrap().clone();
    assert!(events.iter().any(|event| matches!(event,
        RuntimeEvent::HookCompleted { event, outcome, .. }
            if event == "UserPromptSubmit" && outcome == "blocked"
    )));
    assert_eq!(durable_totals(&db, &session), (0, 0));
    let summary = cost.summary(&session).unwrap();
    assert_eq!(summary.input_tokens, expected_calls as u64 * 1000);
    assert!((summary.session_cost - unit * expected_calls as f64).abs() < 1e-12);
    assert!((live_totals(&events).0 - summary.session_cost).abs() < 1e-12);

    // Rebuild all owners over the actual file SQLite and check the next
    // admission, rather than merely inspecting an in-memory accumulator.
    drop(chat);
    drop(cost);
    drop(settings);
    drop(db);
    let db = Arc::new(Database::open(&path).unwrap());
    let settings = hook_settings(db.clone(), HookKind::None).await;
    let cost = Arc::new(CostService::from_settings(db.clone(), settings.clone()));
    assert_eq!(
        cost.summary(&session).unwrap().input_tokens,
        summary.input_tokens
    );
    let chat = ChatService::new(db, settings, model.clone(), workspace).with_cost(cost);
    let error = chat
        .run_in_session(
            "Must fail budget admission before any new request.",
            Some(&session),
            None,
            None,
            Vec::new(),
            None,
            false,
            Some("h235-early-gate-next"),
            |_| {},
            |_| {},
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("daily budget exhausted"));
    assert!(model.parent.lock().unwrap().is_empty());
    assert_eq!(model.hooks.lock().unwrap().len(), expected_calls);
}

#[tokio::test]
async fn prompt_early_denial_is_billed_and_survives_reopen() {
    assert_rejected_prompt_is_billed(HookKind::PromptDeny, 1).await;
}

#[tokio::test]
async fn agent_early_denial_is_billed_and_survives_reopen() {
    assert_rejected_prompt_is_billed(HookKind::AgentDeny, 2).await;
}
