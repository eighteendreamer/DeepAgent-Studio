//! Offline accounting audit of the desktop's post-run title entry point.

use super::*;
use deepagent_app_core::{BudgetConfig, ModelPricing};
use deepagent_core::{clock::Timestamp, id::SessionId};
use deepagent_persistence::{cost_store::CostStore, event_store::EventStore};
use serde_json::{json, Value};

const TITLE: &str = "离线标题计量验收";

#[derive(Default)]
struct TitleModel(Mutex<Vec<Value>>);

#[async_trait]
impl HttpTransport for TitleModel {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        let body: Value = serde_json::from_str(&request.body)?;
        let is_title = body["instructions"]
            .as_str()
            .is_some_and(|text| text.contains("You generate concise conversation titles"));
        self.0.lock().unwrap().push(body.clone());
        let (text, usage) = if is_title {
            assert_eq!(body["max_output_tokens"], 48);
            assert_eq!(body["reasoning"]["effort"], "none");
            (
                TITLE,
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
        emit_responses_events(
            sink,
            &super::responses_fixture::completed(
                if is_title {
                    "audit-title"
                } else {
                    "audit-main"
                },
                body["model"].as_str().unwrap(),
                super::responses_fixture::text("audit-text", text),
                usage,
            ),
        )
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
    settings
        .initialize("offline-audit-placeholder")
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
    });
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
        assert_eq!(transport.0.lock().unwrap().len(), 1);
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
        assert_eq!(transport.0.lock().unwrap().len(), 1);
    }
    let generated = chat.maybe_generate_session_title(&session).await.unwrap();
    let title = EventStore::new(&db).get_session(id).unwrap().unwrap().title;
    match case {
        Case::Empty => assert!(title.is_none()),
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
        calls: transport.0.lock().unwrap().len(),
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
#[ignore = "audit H-63 extension: successful post-run title usage lacks the shared cost owner"]
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
