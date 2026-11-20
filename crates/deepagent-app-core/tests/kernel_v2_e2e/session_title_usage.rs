//! Offline accounting audit of the desktop's post-run title entry point.

use super::*;
use deepagent_app_core::{BudgetConfig, ModelPricing};
use deepagent_core::{clock::Timestamp, id::SessionId};
use deepagent_persistence::{cost_store::CostStore, event_store::EventStore};
use serde_json::{json, Value};
use tokio::sync::Notify;

const TITLE: &str = "离线标题计量验收";

#[path = "session_title_usage/lifecycle.rs"]
mod lifecycle;

#[derive(Clone, Default)]
enum TitleBehavior {
    #[default]
    Normal,
    Empty,
    FailedMeasured,
    FailedTerminal,
    WaitMeasured,
    Rename(Arc<Database>, SessionId),
}

#[derive(Default)]
struct TitleModel {
    requests: Mutex<Vec<Value>>,
    behavior: Mutex<TitleBehavior>,
    observed: Notify,
    release: Notify,
}

#[async_trait]
impl HttpTransport for TitleModel {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        let sink: &mut dyn deepagent_models::transport::EventSink =
            &mut crate::sequence_fixture::sink(sink);
        let body: Value = serde_json::from_str(&request.body)?;
        let is_title = body["instructions"]
            .as_str()
            .is_some_and(|text| text.contains("You generate concise conversation titles"));
        self.requests.lock().unwrap().push(body.clone());
        let behavior = self.behavior.lock().unwrap().clone();
        if is_title && matches!(behavior, TitleBehavior::FailedTerminal) {
            sink.on_event(&json!({"type":"response.failed","response":{"id":"fixture-response","object":"response","created_at":1,"model":"deepseek-flash","output":[],"status":"failed",
                "error":{"code":"offline_title_failed","message":"controlled title provider failure"},
                "usage":{"input_tokens":2000,"output_tokens":7,"total_tokens":2007,
                    "input_tokens_details":{"cached_tokens":1500}}}}).to_string())?;
            return Ok(());
        }
        let (text, usage) = if is_title {
            assert_eq!(body["max_output_tokens"], 48);
            assert_eq!(body["reasoning"]["effort"], "none");
            (
                if matches!(behavior, TitleBehavior::Empty) {
                    ""
                } else {
                    TITLE
                },
                json!({"input_tokens":2000,"output_tokens":7,"total_tokens":2007,
                    "input_tokens_details":{"cached_tokens":1500}}),
            )
        } else {
            (
                "Offline acknowledgement.",
                json!({"input_tokens":1000,"output_tokens":100,"total_tokens":1100,
                    "input_tokens_details":{"cached_tokens":800}}),
            )
        };
        let frames = super::responses_fixture::completed(
            if is_title {
                "audit-title"
            } else {
                "audit-main"
            },
            body["model"].as_str().unwrap(),
            super::responses_fixture::text("audit-text", text),
            usage,
        );
        emit_responses_events(sink, &frames)?;
        if is_title {
            // Repeated final snapshots must still book one physical attempt.
            // Give the distinct snapshot its own provider sequence; a replay
            // of the original event is an H-210 integrity error.
            let mut repeat: Value = serde_json::from_str(frames.last().unwrap()).unwrap();
            repeat["sequence_number"] = json!(frames.len());
            sink.on_event(&repeat.to_string())?;
            self.observed.notify_one();
            match behavior {
                TitleBehavior::FailedMeasured => {
                    return Err(CoreError::other("controlled title provider failure"))
                }
                TitleBehavior::WaitMeasured => self.release.notified().await,
                TitleBehavior::Rename(db, id) => {
                    EventStore::new(&db).rename_session(
                        id,
                        Some("用户在生成期间命名"),
                        Timestamp::from_millis(2),
                    )?;
                }
                _ => {}
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug)]
enum Case {
    Empty,
    Named,
    Untitled,
    ExhaustedBudget,
}

#[derive(Debug)]
struct Observation {
    case: Case,
    calls: usize,
    rows: usize,
    input: u64,
    output: u64,
    cached: u64,
    missed: u64,
    total_cost: f64,
    generated: Option<String>,
}

async fn services(
    db: Arc<Database>,
    workspace: &std::path::Path,
) -> (ChatService, Arc<CostService>, Arc<TitleModel>) {
    let settings = Arc::new(SettingsService::new(
        db.clone(),
        Arc::new(MockTransport::with_get_json(
            r#"{"data":[{"id":"deepseek-flash","object":"model","owned_by":"deepseek"}]}"#,
        )),
        Arc::new(MemorySecretStore::new()),
    ));
    crate::registry_fixture::configure(&settings, "offline-audit-placeholder")
        .await
        .unwrap();
    let cost = Arc::new(CostService::new(db.clone()));
    // Explicit synthetic price: one CNY per million for every category.
    cost.set_pricing(std::collections::HashMap::from([(
        "deepseek-flash".into(),
        ModelPricing {
            input_cache_hit_per_million: 1.0,
            input_cache_miss_per_million: 1.0,
            output_per_million: 1.0,
        },
    )]));
    let transport = Arc::new(TitleModel::default());
    let chat = ChatService::new(db, settings, transport.clone(), workspace).with_cost(cost.clone());
    (chat, cost, transport)
}

async fn run(chat: &ChatService, session: &str, prompt: &str, run_id: &str) -> Result<String> {
    chat.run_in_session(
        prompt,
        Some(session),
        None,
        None,
        Vec::new(),
        None,
        false,
        Some(run_id),
        |_| {},
        |_| {},
    )
    .await
}

async fn assert_exhausted_budget_blocks_ordinary_run(
    chat: &ChatService,
    cost: &CostService,
    session: &str,
) {
    cost.set_budget(BudgetConfig {
        daily_limit: Some(0.0005),
        monthly_limit: None,
    })
    .unwrap();
    assert!(cost
        .check_budget()
        .unwrap_err()
        .to_string()
        .contains("daily budget exhausted"));
    let error = run(
        chat,
        session,
        "This ordinary run must be blocked.",
        "audit-title-budget-control",
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("daily budget exhausted"));
}

async fn observe(case: Case) -> Observation {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("state.sqlite");
    let db = Arc::new(Database::open(&path).unwrap());
    let (chat, cost, transport) = services(db.clone(), root.path()).await;
    let session = chat.create_session(None).unwrap();
    let id = session.parse::<SessionId>().unwrap();
    if !matches!(case, Case::Empty) {
        run(
            &chat,
            &session,
            "Reply with a short acknowledgement.",
            "audit-title-main",
        )
        .await
        .unwrap();
        assert_eq!(transport.requests.lock().unwrap().len(), 1);
        assert!(EventStore::new(&db)
            .get_session(id)
            .unwrap()
            .unwrap()
            .title
            .is_none());
        let before = cost.summary(&session).unwrap();
        assert_eq!((before.input_tokens, before.output_tokens), (1000, 100));
        assert!((before.total_cost - 0.0011).abs() < 1e-12);
        let terminal: Vec<_> = chat
            .run_events("audit-title-main", None)
            .unwrap()
            .into_iter()
            .filter(|event| event.event_type == "run_terminal")
            .collect();
        assert_eq!(terminal.len(), 1);
        assert_eq!(terminal[0].data["kind"], "succeeded");
    }
    if matches!(case, Case::Named) {
        EventStore::new(&db)
            .rename_session(id, Some("用户标题"), Timestamp::from_millis(1))
            .unwrap();
    }
    if matches!(case, Case::ExhaustedBudget) {
        assert_exhausted_budget_blocks_ordinary_run(&chat, &cost, &session).await;
        assert_eq!(transport.requests.lock().unwrap().len(), 1);
    }
    let result = if matches!(case, Case::Empty) {
        chat.maybe_generate_session_title(&session).await
    } else {
        chat.maybe_generate_session_title_for_run(&session, "audit-title-main")
            .await
    };
    let generated = if matches!(case, Case::ExhaustedBudget) {
        // Optional title work skips an exhausted budget without dispatching;
        // malformed policy/storage errors still propagate from admission.
        assert!(result.unwrap().is_none());
        None
    } else {
        result.unwrap()
    };
    let title = EventStore::new(&db).get_session(id).unwrap().unwrap().title;
    match case {
        Case::Empty | Case::ExhaustedBudget => assert!(title.is_none()),
        Case::Named => assert_eq!(title.as_deref(), Some("用户标题")),
        _ => assert_eq!(title.as_deref(), Some(TITLE)),
    }
    let live = cost.summary(&session).unwrap();
    // A distinct connection reopens the file-backed store; not an in-memory gauge.
    let reopened = Database::open(&path).unwrap();
    let store = CostStore::new(&reopened);
    let usage = store.session_usage(&session).unwrap();
    assert_eq!(usage.input_tokens, live.input_tokens);
    assert_eq!(usage.output_tokens, live.output_tokens);
    assert_eq!(usage.cache_hit_tokens, live.cache_hit_tokens);
    assert_eq!(usage.cache_miss_tokens, live.cache_miss_tokens);
    assert_eq!(store.session_total(&session).unwrap(), live.session_cost);
    let observed = Observation {
        case,
        calls: transport.requests.lock().unwrap().len(),
        rows: store.session_costs_detailed(&session).unwrap().len(),
        input: usage.input_tokens,
        output: usage.output_tokens,
        cached: usage.cache_hit_tokens,
        missed: usage.cache_miss_tokens,
        total_cost: live.total_cost,
        generated,
    };
    eprintln!("session title accounting: {observed:?}");
    observed
}

#[tokio::test]
async fn empty_history_and_user_named_sessions_do_not_dispatch_title_requests() {
    for case in [Case::Empty, Case::Named] {
        let result = observe(case).await;
        let expected = usize::from(matches!(result.case, Case::Named));
        assert_eq!((result.calls, result.rows), (expected, expected));
        assert!(result.generated.is_none());
    }
}

#[tokio::test]
async fn audit_title_requests_obey_budget_and_book_returned_usage() {
    let ordinary = observe(Case::Untitled).await;
    let exhausted = observe(Case::ExhaustedBudget).await;
    assert!(
        ordinary.calls == 2 && ordinary.rows == 2 && ordinary.input == 3000
            && ordinary.output == 107 && ordinary.cached == 2300 && ordinary.missed == 700
            && (ordinary.total_cost - 0.003107).abs() < 1e-12
            && ordinary.generated.as_deref() == Some(TITLE)
            && exhausted.calls == 1 && exhausted.rows == 1 && exhausted.generated.is_none(),
        "title model calls must retain usage and share budget admission: {ordinary:?}; {exhausted:?}"
    );
}

struct TitleFixture {
    root: tempfile::TempDir,
    db: Arc<Database>,
    chat: ChatService,
    cost: Arc<CostService>,
    transport: Arc<TitleModel>,
    session: String,
}

impl TitleFixture {
    async fn new(reply: TitleBehavior) -> Self {
        let root = tempfile::tempdir().unwrap();
        let db = Arc::new(Database::open(root.path().join("state.sqlite")).unwrap());
        let (chat, cost, transport) = services(db.clone(), root.path()).await;
        let session = chat.create_session(None).unwrap();
        run(&chat, &session, "Acknowledge offline.", "audit-title-main")
            .await
            .unwrap();
        *transport.behavior.lock().unwrap() = reply;
        Self {
            root,
            db,
            chat,
            cost,
            transport,
            session,
        }
    }

    fn assert_settled(&self, outcome: &str, booked: bool) {
        let reopened = Database::open(self.root.path().join("state.sqlite")).unwrap();
        let events = deepagent_persistence::run_store::RunStore::new(&reopened)
            .events_after("audit-title-main", None)
            .unwrap();
        let started: Vec<_> = events
            .iter()
            .filter(|e| e.event_type == "model_side_call_started")
            .collect();
        let finished: Vec<_> = events
            .iter()
            .filter(|e| e.event_type == "model_side_call_finished")
            .collect();
        assert_eq!(started.len(), 1);
        assert_eq!(finished.len(), 1);
        assert_eq!(started[0].data["purpose"], "session_title");
        assert_eq!(
            started[0].data["invocation_id"],
            finished[0].data["invocation_id"]
        );
        assert_eq!(finished[0].data["outcome"], outcome);
        let receipts: Vec<_> = events
            .iter()
            .filter(|e| {
                e.event_type == "model_call_usage" && e.data["report"]["purpose"] == "session_title"
            })
            .collect();
        assert_eq!(receipts.len(), usize::from(booked));
        if booked {
            assert_eq!(receipts[0].data["report"]["purpose"], "session_title");
            assert_eq!(receipts[0].data["report"]["model_id"], "deepseek-flash");
            assert_eq!(
                receipts[0].data["report"]["usage"]["prompt_cache_hit_tokens"],
                1500
            );
            assert_eq!(
                receipts[0].data["report"]["invocation_id"],
                started[0].data["invocation_id"]
            );
        }
        let terminal: Vec<_> = events
            .iter()
            .filter(|e| e.event_type == "run_terminal")
            .collect();
        assert_eq!(terminal.len(), 1);
        assert_eq!(terminal[0].data["kind"], "succeeded");
        let usage = CostStore::new(&reopened)
            .session_usage(&self.session)
            .unwrap();
        assert_eq!(
            (usage.input_tokens, usage.output_tokens),
            if booked { (3000, 107) } else { (1000, 100) }
        );
        assert_eq!(
            CostStore::new(&reopened)
                .session_costs_detailed(&self.session)
                .unwrap()
                .len(),
            if booked { 2 } else { 1 }
        );
    }

    fn title(&self) -> Option<String> {
        EventStore::new(&self.db)
            .get_session(self.session.parse().unwrap())
            .unwrap()
            .unwrap()
            .title
    }
}

#[tokio::test]
async fn discarded_title_and_measured_transport_failure_still_book_usage() {
    for reply in [TitleBehavior::Empty, TitleBehavior::FailedMeasured] {
        let fixture = TitleFixture::new(reply.clone()).await;
        let result = fixture
            .chat
            .maybe_generate_session_title_for_run(&fixture.session, "audit-title-main")
            .await;
        let outcome = if matches!(reply, TitleBehavior::Empty) {
            assert!(result.unwrap().is_none());
            "empty"
        } else {
            assert!(result
                .unwrap_err()
                .to_string()
                .contains("controlled title provider failure"));
            "failed"
        };
        assert!(fixture.title().is_none());
        fixture.assert_settled(outcome, true);
    }
}

#[tokio::test]
async fn user_rename_during_title_request_wins_without_discarding_usage() {
    let fixture = TitleFixture::new(TitleBehavior::WaitMeasured).await;
    let title = fixture
        .chat
        .maybe_generate_session_title_for_run(&fixture.session, "audit-title-main");
    let rename = async {
        fixture.transport.observed.notified().await;
        EventStore::new(&fixture.db)
            .rename_session(
                fixture.session.parse().unwrap(),
                Some("用户新标题"),
                Timestamp::from_millis(2),
            )
            .unwrap();
        fixture.transport.release.notify_one();
    };
    let (result, ()) = tokio::join!(title, rename);
    assert!(result.unwrap().is_none());
    assert_eq!(fixture.title().as_deref(), Some("用户新标题"));
    fixture.assert_settled("superseded", true);
}

#[tokio::test]
async fn dropped_title_future_and_actual_timeout_keep_observed_usage() {
    // Exercise actual future ownership, not a synthetic Usage report.
    let fixture = TitleFixture::new(TitleBehavior::WaitMeasured).await;
    {
        let title = fixture
            .chat
            .maybe_generate_session_title_for_run(&fixture.session, "audit-title-main");
        tokio::pin!(title);
        tokio::select! {
            result = &mut title => panic!("waiting request completed early: {result:?}"),
            _ = fixture.transport.observed.notified() => {}
        }
    }
    fixture.assert_settled("cancelled", true);
    assert!(fixture.title().is_none());

    let fixture = TitleFixture::new(TitleBehavior::WaitMeasured).await;
    let error = fixture
        .chat
        .maybe_generate_session_title_for_run(&fixture.session, "audit-title-main")
        .await
        .unwrap_err();
    assert!(error
        .to_string()
        .contains("session title generation timed out"));
    fixture.assert_settled("timed_out", true);
    assert!(fixture.title().is_none());
}

#[tokio::test]
async fn explicit_title_owner_does_not_follow_a_newer_run_or_another_session() {
    let fixture = TitleFixture::new(TitleBehavior::Normal).await;
    let other = fixture.chat.create_session(None).unwrap();
    run(
        &fixture.chat,
        &other,
        "Other offline session.",
        "audit-title-other",
    )
    .await
    .unwrap();
    for invalid in ["missing-run", "audit-title-other"] {
        assert!(fixture
            .chat
            .maybe_generate_session_title_for_run(&fixture.session, invalid)
            .await
            .is_err());
    }
    assert_eq!(fixture.transport.requests.lock().unwrap().len(), 2);
    run(
        &fixture.chat,
        &fixture.session,
        "Continue offline.",
        "audit-title-newer",
    )
    .await
    .unwrap();
    assert_eq!(
        fixture
            .chat
            .maybe_generate_session_title_for_run(&fixture.session, "audit-title-main")
            .await
            .unwrap()
            .as_deref(),
        Some(TITLE)
    );
    let old = fixture.chat.run_events("audit-title-main", None).unwrap();
    assert_eq!(
        old.iter()
            .filter(|e| e.event_type == "model_call_usage"
                && e.data["report"]["purpose"] == "session_title")
            .count(),
        1
    );
    for run_id in ["audit-title-other", "audit-title-newer"] {
        let events = fixture.chat.run_events(run_id, None).unwrap();
        assert!(!events
            .iter()
            .any(|e| e.event_type.starts_with("model_side_call")
                || (e.event_type == "model_call_usage"
                    && e.data["report"]["purpose"] == "session_title")));
    }
}

#[tokio::test]
async fn corrupt_budget_is_an_error_not_an_optional_title_skip() {
    let fixture = TitleFixture::new(TitleBehavior::Normal).await;
    deepagent_persistence::document_store::DocumentStore::new(&fixture.db)
        .put(
            "settings",
            "cost-budget",
            "{",
            None,
            Timestamp::from_millis(1),
        )
        .unwrap();
    assert!(fixture.cost.check_budget().is_err());
    assert!(fixture
        .chat
        .maybe_generate_session_title_for_run(&fixture.session, "audit-title-main")
        .await
        .is_err());
    assert_eq!(fixture.transport.requests.lock().unwrap().len(), 1);
    assert!(fixture.title().is_none());
}

#[tokio::test]
async fn failed_title_ledger_write_is_explicit_without_rewriting_main_terminal() {
    let fixture = TitleFixture::new(TitleBehavior::Normal).await;
    fixture.db.with_conn(|connection| connection.execute_batch("CREATE TEMP TRIGGER fail_title_cost BEFORE INSERT ON costs BEGIN SELECT RAISE(FAIL,'controlled title cost failure'); END;").map_err(|e| CoreError::Persistence(e.to_string()))).unwrap();
    let error = fixture
        .chat
        .maybe_generate_session_title_for_run(&fixture.session, "audit-title-main")
        .await
        .unwrap_err();
    assert!(error.to_string().contains("controlled title cost failure"));
    let events = fixture.chat.run_events("audit-title-main", None).unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|e| e.event_type == "model_call_usage_failed")
            .count(),
        1
    );
    fixture.assert_settled("accounting_failed", false);
    assert!(fixture.title().is_none());
}

#[tokio::test]
async fn retry_is_a_distinct_paid_call_and_missing_pricing_does_not_erase_tokens() {
    let fixture = TitleFixture::new(TitleBehavior::FailedMeasured).await;
    assert!(fixture
        .chat
        .maybe_generate_session_title_for_run(&fixture.session, "audit-title-main")
        .await
        .is_err());
    fixture.assert_settled("failed", true);
    *fixture.transport.behavior.lock().unwrap() = TitleBehavior::Normal;
    fixture.cost.set_pricing(std::collections::HashMap::new());
    assert!(fixture
        .chat
        .maybe_generate_session_title_for_run(&fixture.session, "audit-title-main")
        .await
        .unwrap()
        .is_some());
    let reopened = Database::open(fixture.root.path().join("state.sqlite")).unwrap();
    let store = CostStore::new(&reopened);
    let usage = store.session_usage(&fixture.session).unwrap();
    assert_eq!(
        (
            usage.input_tokens,
            usage.output_tokens,
            usage.cache_hit_tokens
        ),
        (5000, 114, 3800)
    );
    let rows = store.session_costs_detailed(&fixture.session).unwrap();
    assert_eq!(rows.len(), 3);
    assert!(rows[0].cost_yuan.is_some() && rows[1].cost_yuan.is_some());
    assert!(rows[2].cost_yuan.is_none());
    let events = fixture.chat.run_events("audit-title-main", None).unwrap();
    let receipts: Vec<_> = events
        .iter()
        .filter(|e| {
            e.event_type == "model_call_usage" && e.data["report"]["purpose"] == "session_title"
        })
        .collect();
    assert_eq!(receipts.len(), 2);
    assert_ne!(receipts[0].data["call_id"], receipts[1].data["call_id"]);
    assert_ne!(
        receipts[0].data["report"]["invocation_id"],
        receipts[1].data["report"]["invocation_id"]
    );
}
