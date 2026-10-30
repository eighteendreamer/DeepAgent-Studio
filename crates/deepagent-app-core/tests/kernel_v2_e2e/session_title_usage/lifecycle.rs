use super::*;
use deepagent_core::{event::EventPayload, message::Message};
use deepagent_persistence::run_store::{RunRecord, RunStore};
use std::time::Duration;

const RUN: &str = "audit-title-lifecycle";

async fn prepared(
    root: &std::path::Path,
) -> (
    Arc<Database>,
    ChatService,
    Arc<CostService>,
    Arc<TitleModel>,
    String,
    RunRecord,
) {
    let db = Arc::new(Database::open(root.join("state.sqlite")).unwrap());
    let (chat, cost, model) = services(db.clone(), root).await;
    let session = chat.create_session(None).unwrap();
    run(&chat, &session, "Reply briefly.", RUN).await.unwrap();
    let parent = RunStore::new(&db).get(RUN).unwrap().unwrap();
    assert_eq!(parent.terminal_kind.as_deref(), Some("succeeded"));
    (db, chat, cost, model, session, parent)
}

fn assert_lifecycle(
    db: &Database,
    session: &str,
    original: &RunRecord,
    outcomes: &[&str],
    measured: usize,
) {
    let store = RunStore::new(db);
    assert_eq!(store.get(RUN).unwrap().as_ref(), Some(original));
    let events = store.events_after(RUN, None).unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_type == "run_terminal")
            .count(),
        1
    );
    assert!(events
        .windows(2)
        .all(|pair| pair[0].sequence + 1 == pair[1].sequence));
    let ended: Vec<_> = events
        .iter()
        .filter(|event| event.event_type == "model_side_call_finished")
        .collect();
    assert_eq!(ended.len(), outcomes.len());
    for (event, expected) in ended.iter().zip(outcomes) {
        assert_eq!(event.data["version"], 1);
        assert_eq!(event.data["outcome"], *expected);
        assert_eq!(event.data["session_id"], session);
        assert_eq!(event.data["purpose"], "session_title");
        let started = events
            .iter()
            .find(|start| {
                start.event_type == "model_side_call_started"
                    && start.data["invocation_id"] == event.data["invocation_id"]
            })
            .unwrap();
        assert!(started.sequence < event.sequence);
    }
    let receipts = CostStore::new(db).run_call_receipts(session, RUN).unwrap();
    assert_eq!(
        receipts
            .iter()
            .filter(|receipt| receipt.report["purpose"] == "session_title")
            .count(),
        measured
    );
    for receipt in receipts
        .iter()
        .filter(|receipt| receipt.report["purpose"] == "session_title")
    {
        assert_eq!(receipt.report["model_id"], "deepseek-flash");
        assert_eq!(receipt.report["origin"], "side_call");
        assert_eq!(receipt.report["usage"]["prompt_cache_hit_tokens"], 1500);
        assert!(ended
            .iter()
            .any(|end| end.data["invocation_id"] == receipt.report["invocation_id"]));
    }
}

#[tokio::test]
async fn accepted_empty_and_superseded_titles_share_durable_accounting_without_reterminalizing() {
    for mode in 0..3 {
        let root = tempfile::tempdir().unwrap();
        let (db, chat, cost, model, session, parent) = prepared(root.path()).await;
        *model.behavior.lock().unwrap() = match mode {
            0 => TitleBehavior::Normal,
            1 => TitleBehavior::Empty,
            _ => TitleBehavior::Rename(db.clone(), session.parse().unwrap()),
        };
        let generated = chat
            .maybe_generate_session_title_for_run(&session, RUN)
            .await
            .unwrap();
        assert_eq!(generated.as_deref(), (mode == 0).then_some(TITLE));
        let summary = cost.summary(&session).unwrap();
        assert_eq!(
            (
                summary.input_tokens,
                summary.output_tokens,
                summary.cache_hit_tokens
            ),
            (3000, 107, 2300)
        );
        assert!((summary.total_cost - 0.003107).abs() < 1e-12);
        if mode == 2 {
            assert_eq!(
                EventStore::new(&db)
                    .get_session(session.parse().unwrap())
                    .unwrap()
                    .unwrap()
                    .title
                    .as_deref(),
                Some("用户在生成期间命名")
            );
        }
        let reopened = Database::open(root.path().join("state.sqlite")).unwrap();
        assert_lifecycle(
            &reopened,
            &session,
            &parent,
            &[match mode {
                0 => "applied",
                1 => "empty",
                _ => "superseded",
            }],
            1,
        );
        assert_eq!(
            CostStore::new(&reopened)
                .session_usage(&session)
                .unwrap()
                .input_tokens,
            3000
        );
    }
}

#[tokio::test]
async fn measured_failed_title_attempt_can_retry_without_losing_or_rebooking_usage() {
    let root = tempfile::tempdir().unwrap();
    let (db, chat, cost, model, session, parent) = prepared(root.path()).await;
    *model.behavior.lock().unwrap() = TitleBehavior::FailedMeasured;
    assert!(chat
        .maybe_generate_session_title_for_run(&session, RUN)
        .await
        .unwrap_err()
        .to_string()
        .contains("controlled title provider failure"));
    assert_lifecycle(&db, &session, &parent, &["failed"], 1);
    *model.behavior.lock().unwrap() = TitleBehavior::Normal;
    assert_eq!(
        chat.maybe_generate_session_title_for_run(&session, RUN)
            .await
            .unwrap()
            .as_deref(),
        Some(TITLE)
    );
    assert_lifecycle(&db, &session, &parent, &["failed", "applied"], 2);
    let summary = cost.summary(&session).unwrap();
    assert_eq!(
        (
            summary.input_tokens,
            summary.output_tokens,
            summary.cache_hit_tokens
        ),
        (5000, 114, 3800)
    );
    assert!((summary.total_cost - 0.005114).abs() < 1e-12);
    assert_eq!(
        CostStore::new(&db)
            .session_costs_detailed(&session)
            .unwrap()
            .len(),
        3
    );
}

#[tokio::test]
async fn native_failed_title_terminal_retains_reported_usage_before_propagating_the_error() {
    let root = tempfile::tempdir().unwrap();
    let (db, chat, cost, model, session, parent) = prepared(root.path()).await;
    *model.behavior.lock().unwrap() = TitleBehavior::FailedTerminal;
    assert!(chat
        .maybe_generate_session_title_for_run(&session, RUN)
        .await
        .unwrap_err()
        .to_string()
        .contains("controlled title provider failure"));
    assert_lifecycle(&db, &session, &parent, &["failed"], 1);
    assert_eq!(cost.summary(&session).unwrap().input_tokens, 3000);
}

#[tokio::test]
async fn externally_cancelled_or_timed_out_title_future_settles_measured_usage_after_main_terminal()
{
    for timed_out in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let (db, chat, cost, model, session, parent) = prepared(root.path()).await;
        *model.behavior.lock().unwrap() = TitleBehavior::WaitMeasured;
        let mut future = Box::pin(chat.maybe_generate_session_title_for_run(&session, RUN));
        tokio::select! {
            result = &mut future => panic!("measured request must still wait: {result:?}"),
            _ = model.observed.notified() => {}
        }
        if timed_out {
            assert!(tokio::time::timeout(Duration::from_millis(30), future)
                .await
                .is_err());
        } else {
            drop(future);
        }
        assert_lifecycle(&db, &session, &parent, &["cancelled"], 1);
        assert_eq!(cost.summary(&session).unwrap().input_tokens, 3000);
        assert!(EventStore::new(&db)
            .get_session(session.parse().unwrap())
            .unwrap()
            .unwrap()
            .title
            .is_none());
    }
}

#[tokio::test]
async fn title_failed_ledger_write_never_saves_title_or_rewrites_main_success() {
    let root = tempfile::tempdir().unwrap();
    let (db, chat, cost, _model, session, parent) = prepared(root.path()).await;
    db.with_conn(|conn| conn.execute_batch("CREATE TEMP TRIGGER fail_title_cost BEFORE INSERT ON costs BEGIN SELECT RAISE(FAIL,'controlled title cost failure'); END;").map_err(|e| CoreError::Persistence(e.to_string()))).unwrap();
    assert!(chat
        .maybe_generate_session_title_for_run(&session, RUN)
        .await
        .unwrap_err()
        .to_string()
        .contains("controlled title cost failure"));
    assert_lifecycle(&db, &session, &parent, &["accounting_failed"], 1);
    let receipts = CostStore::new(&db)
        .run_call_receipts(&session, RUN)
        .unwrap();
    let failed_receipt = receipts
        .iter()
        .find(|receipt| receipt.report["purpose"] == "session_title")
        .unwrap();
    assert!(failed_receipt.ledger_id.is_none());
    assert_eq!(cost.summary(&session).unwrap().input_tokens, 1000);
    let events = chat.run_events(RUN, None).unwrap();
    let failed = events
        .iter()
        .find(|event| event.event_type == "model_call_usage_failed")
        .unwrap();
    assert_eq!(failed.data["report"]["usage"]["prompt_tokens"], 2000);
    assert!(EventStore::new(&db)
        .get_session(session.parse().unwrap())
        .unwrap()
        .unwrap()
        .title
        .is_none());
}

#[tokio::test]
async fn missing_price_keeps_title_tokens_and_unknown_price_blocks_the_next_optional_dispatch() {
    let root = tempfile::tempdir().unwrap();
    let (db, chat, cost, model, session, parent) = prepared(root.path()).await;
    cost.set_pricing(std::collections::HashMap::new());
    *model.behavior.lock().unwrap() = TitleBehavior::Empty;
    assert!(chat
        .maybe_generate_session_title_for_run(&session, RUN)
        .await
        .unwrap()
        .is_none());
    let summary = cost.summary(&session).unwrap();
    assert_eq!((summary.input_tokens, summary.output_tokens), (3000, 107));
    let calls = model.requests.lock().unwrap().len();
    cost.set_budget(BudgetConfig {
        daily_limit: Some(1.0),
        monthly_limit: None,
    })
    .unwrap();
    let error = chat
        .maybe_generate_session_title_for_run(&session, RUN)
        .await
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("prior model call usage or price is unavailable"),
        "{error}"
    );
    assert_eq!(model.requests.lock().unwrap().len(), calls);
    assert_lifecycle(&db, &session, &parent, &["empty", "rejected"], 1);
}

#[tokio::test]
async fn explicit_parent_does_not_guess_latest_and_legacy_ambiguity_or_missing_provenance_never_dispatches(
) {
    let root = tempfile::tempdir().unwrap();
    let (db, chat, _cost, model, session, parent) = prepared(root.path()).await;
    let other = chat.create_session(None).unwrap();
    let before = model.requests.lock().unwrap().len();
    assert!(chat
        .maybe_generate_session_title_for_run(&session, "absent")
        .await
        .is_err());
    let runs = RunStore::new(&db);
    runs.create("foreign", &other, None, 1).unwrap();
    assert!(chat
        .maybe_generate_session_title_for_run(&session, "foreign")
        .await
        .unwrap_err()
        .to_string()
        .contains("session mismatch"));
    runs.create("concurrent-newer", &session, None, parent.created_at + 1)
        .unwrap();
    assert!(chat
        .maybe_generate_session_title(&session)
        .await
        .unwrap_err()
        .to_string()
        .contains("explicit parent"));
    assert!(chat
        .maybe_generate_session_title_for_run(&session, "concurrent-newer")
        .await
        .unwrap()
        .is_none());
    assert_eq!(model.requests.lock().unwrap().len(), before);
    // Accurate causal run remains valid even with a newer active run.
    assert_eq!(
        chat.maybe_generate_session_title_for_run(&session, RUN)
            .await
            .unwrap()
            .as_deref(),
        Some(TITLE)
    );
    assert_lifecycle(&db, &session, &parent, &["applied"], 1);
    // Separate session with no runs: imported history cannot invent provenance.
    let legacy = chat.create_session(None).unwrap();
    let clock = deepagent_core::clock::SystemClock;
    let mut history =
        deepagent_session::Session::recover(&db, &clock, legacy.parse().unwrap()).unwrap();
    history
        .append(EventPayload::MessageAppended {
            message: Message::user("Imported history"),
        })
        .unwrap();
    let before = model.requests.lock().unwrap().len();
    assert!(chat
        .maybe_generate_session_title(&legacy)
        .await
        .unwrap()
        .is_none());
    assert_eq!(model.requests.lock().unwrap().len(), before);
}
