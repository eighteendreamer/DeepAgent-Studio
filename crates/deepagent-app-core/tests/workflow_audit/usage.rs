//! H-163: real workflow/kernel/finalizer consumers, synthetic local providers.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use deepagent_app_core::canvas_model_gateway::CanvasModelGateway;
use deepagent_app_core::canvas_provider_service::{
    CanvasModelConfig, CanvasProviderInput, CanvasProviderService, CanvasScenario,
};
use deepagent_app_core::{
    AppService, ChatService, CostService, MemorySecretStore, ModelPricing, SettingsService,
};
use deepagent_core::clock::SystemClock;
use deepagent_core::event::EventPayload;
use deepagent_models::{HttpTransport, MockTransport, ModelClient, ModelConfig, TransportRequest};
use deepagent_persistence::{event_store::EventStore, run_store::RunStore, Database};
use deepagent_runtime::workflow::{compile, NodeEventPublisher, WorkflowAgent, WorkflowRequest};
use deepagent_runtime::{AgentKernel, RunRequest, RuntimeConfig, RuntimeEvent};
use deepagent_session::Session;
use deepagent_tools::ToolRegistry;
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::Notify;
use tokio::task::JoinHandle;

#[path = "budget.rs"]
mod measured_budget;

struct Provider {
    base: String,
    calls: Arc<Mutex<Vec<Value>>>,
    server: JoinHandle<()>,
}

impl Provider {
    async fn start(responses: Vec<(u16, Option<Value>)>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}/v1", listener.local_addr().unwrap());
        let calls = Arc::new(Mutex::new(Vec::new()));
        let seen = calls.clone();
        let server = tokio::spawn(async move {
            for (status, response) in responses {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut wire = Vec::new();
                let (header_end, length) = loop {
                    let mut buf = [0; 4096];
                    let n = socket.read(&mut buf).await.unwrap();
                    assert!(n > 0);
                    wire.extend_from_slice(&buf[..n]);
                    if let Some(end) = wire.windows(4).position(|w| w == b"\r\n\r\n") {
                        let header = String::from_utf8_lossy(&wire[..end]);
                        let length = header
                            .lines()
                            .find_map(|line| {
                                let (key, value) = line.split_once(':')?;
                                key.eq_ignore_ascii_case("content-length")
                                    .then(|| value.trim().parse::<usize>().unwrap())
                            })
                            .unwrap();
                        break (end + 4, length);
                    }
                };
                while wire.len() < header_end + length {
                    let mut buf = [0; 4096];
                    let n = socket.read(&mut buf).await.unwrap();
                    assert!(n > 0);
                    wire.extend_from_slice(&buf[..n]);
                }
                seen.lock()
                    .unwrap()
                    .push(serde_json::from_slice(&wire[header_end..header_end + length]).unwrap());
                let Some(response) = response else {
                    std::future::pending::<()>().await;
                    unreachable!();
                };
                let body = response.to_string();
                let header = format!("HTTP/1.1 {status} Fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                socket.write_all(header.as_bytes()).await.unwrap();
                socket.write_all(body.as_bytes()).await.unwrap();
            }
        });
        Self {
            base,
            calls,
            server,
        }
    }

    async fn wait_calls(&self, count: usize) {
        tokio::time::timeout(Duration::from_secs(5), async {
            while self.calls.lock().unwrap().len() < count {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
    }
}

impl Drop for Provider {
    fn drop(&mut self) {
        self.server.abort();
    }
}

fn completion(prompt: u32, output: u32, hit: u32, reasoning: u32) -> Value {
    json!({"choices":[{"message":{"content":"answer","reasoning_content":"reasoning"},"finish_reason":"stop"}],
        "usage":{"prompt_tokens":prompt,"completion_tokens":output,"total_tokens":prompt+output,
            "prompt_cache_hit_tokens":hit,"prompt_cache_miss_tokens":prompt-hit,
            "completion_tokens_details":{"reasoning_tokens":reasoning},
            "untrusted_private_text":"h163-must-not-be-persisted"}})
}

fn workflow(models: &[&str]) -> WorkflowRequest {
    let mut nodes = vec![json!({"id":"start","kind":"start"})];
    let mut edges = Vec::new();
    let mut previous = "start".to_string();
    for (i, model) in models.iter().enumerate() {
        let id = format!("text-{i}");
        nodes.push(
            json!({"id":id,"kind":"text-gen","config":{"model":model,"prompt":"synthetic usage"}}),
        );
        edges.push(json!({"id":format!("edge-{i}"),"source":previous,"target":id}));
        previous = id;
    }
    nodes.push(json!({"id":"end","kind":"end"}));
    edges.push(json!({"id":"edge-end","source":previous,"target":"end"}));
    serde_json::from_value(json!({"definition":{"version":1,"nodes":nodes,"edges":edges}})).unwrap()
}

struct Fixture {
    root: tempfile::TempDir,
    db: Arc<Database>,
    chat: ChatService,
    providers: Arc<CanvasProviderService>,
    cost: Arc<CostService>,
    session: String,
}

impl Fixture {
    async fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let db = Arc::new(Database::open(root.path().join("usage.db")).unwrap());
        let secrets = Arc::new(MemorySecretStore::new());
        let settings = Arc::new(SettingsService::new(
            db.clone(),
            Arc::new(MockTransport::with_get_json(
                r#"{"data":[{"id":"deepseek-v4-flash","object":"model","owned_by":"deepseek"}]}"#,
            )),
            secrets.clone(),
        ));
        settings.initialize("sk-fixture-placeholder").await.unwrap();
        let providers = Arc::new(CanvasProviderService::new(db.clone(), secrets));
        let cost = Arc::new(CostService::new(db.clone()));
        let chat = ChatService::new(
            db.clone(),
            settings,
            Arc::new(MockTransport::default()),
            root.path(),
        )
        .with_canvas_model(Arc::new(CanvasModelGateway::new(providers.clone())))
        .with_cost(cost.clone());
        let session = chat.create_session(None).unwrap();
        Self {
            root,
            db,
            chat,
            providers,
            cost,
            session,
        }
    }

    fn register(&self, provider: &Provider, protocol: &str, model: &str, priority: i32) -> String {
        let saved = self
            .providers
            .save_provider(CanvasProviderInput {
                name: model.into(),
                protocol: protocol.into(),
                base_url: provider.base.clone(),
                enabled: true,
                api_key: Some("sk-fixture-placeholder".into()),
                models: vec![CanvasModelConfig {
                    id: model.into(),
                    name: model.into(),
                    description: String::new(),
                    enabled: true,
                    scenarios: vec![CanvasScenario::Text],
                    priority,
                    max_reference_images: None,
                }],
                ..Default::default()
            })
            .unwrap();
        // Synthetic differential prices prove model attribution, not real fees.
        let pricing = ["audit-a", "audit-b", "audit-anthropic", "audit-gemini"]
            .into_iter()
            .map(|id| {
                (
                    id.into(),
                    ModelPricing {
                        input_cache_hit_per_million: 1.0,
                        input_cache_miss_per_million: 2.0,
                        output_per_million: 3.0,
                    },
                )
            })
            .collect::<HashMap<_, _>>();
        self.cost.set_pricing(pricing);
        format!("{}::{model}", saved.id)
    }

    async fn run(&self, run: &str, request: WorkflowRequest) -> (bool, Vec<RuntimeEvent>) {
        let live = Arc::new(Mutex::new(Vec::new()));
        let capture = live.clone();
        let result = tokio::time::timeout(
            Duration::from_secs(10),
            self.chat.run_workflow(
                request,
                run.into(),
                self.session.clone(),
                move |event| capture.lock().unwrap().push(event),
                |_| {},
            ),
        )
        .await
        .expect("workflow must finish without external services");
        let events = live.lock().unwrap().clone();
        (result.is_ok(), events)
    }

    fn reopen(self) -> (tempfile::TempDir, Arc<Database>, String) {
        let path = self.root.path().join("usage.db");
        drop(self.chat);
        drop(self.providers);
        drop(self.cost);
        drop(self.db);
        let db = Arc::new(Database::open(path).unwrap());
        (self.root, db, self.session)
    }
}

fn recorded_usage(db: &Database, session: &str) -> Vec<Value> {
    EventStore::new(db).load_session(session.parse().unwrap()).unwrap().into_iter()
        .filter_map(|event| match event.payload {
            EventPayload::UsageRecorded {prompt_tokens, completion_tokens, reasoning_tokens,
                total_tokens, prompt_cache_hit_tokens, prompt_cache_miss_tokens, raw_responses_usage, ..} =>
                Some(json!({"prompt":prompt_tokens,"output":completion_tokens,"reasoning":reasoning_tokens,
                    "total":total_tokens,"hit":prompt_cache_hit_tokens,"miss":prompt_cache_miss_tokens,
                    "raw":raw_responses_usage})),
            _ => None,
        }).collect()
}

fn assert_terminal(db: &Database, run: &str, kind: &str) {
    let store = RunStore::new(db);
    assert_eq!(
        store.get(run).unwrap().unwrap().terminal_kind.as_deref(),
        Some(kind)
    );
    let events = store.events_after(run, None).unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_type == "run_terminal")
            .count(),
        1
    );
    let cursor = events[events.len() / 2].sequence;
    assert_eq!(
        store.events_after(run, Some(cursor)).unwrap(),
        events
            .into_iter()
            .filter(|e| e.sequence > cursor)
            .collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn multiple_models_reach_existing_ledger_ui_projection_and_reopened_sqlite_once() {
    let a = Provider::start(vec![(200, Some(completion(100, 20, 80, 12)))]).await;
    let b = Provider::start(vec![(200, Some(completion(50, 10, 0, 5)))]).await;
    let f = Fixture::new().await;
    let ra = f.register(&a, "openai", "audit-a", 0);
    let rb = f.register(&b, "openai", "audit-b", 0);
    let (ok, live) = f.run("usage-multi", workflow(&[&ra, &rb])).await;
    assert!(ok);
    assert_eq!(
        live.iter()
            .filter(|e| matches!(e, RuntimeEvent::Usage { .. }))
            .count(),
        2
    );
    let costs: Vec<_> = live
        .iter()
        .filter_map(|event| match event {
            RuntimeEvent::CostRecorded {
                model,
                cost_yuan,
                origin,
            } => Some((model, cost_yuan, origin)),
            _ => None,
        })
        .collect();
    assert_eq!(
        costs.len(),
        1,
        "existing UI needs one settled turn-cost receipt"
    );
    assert_eq!(costs[0].0, "workflow");
    assert!((costs[0].1.unwrap() - 0.000310).abs() < 0.000000001);
    assert_eq!(a.calls.lock().unwrap().len(), 1);
    assert_eq!(b.calls.lock().unwrap().len(), 1);
    let (_root, db, session) = f.reopen();
    let usage = recorded_usage(&db, &session);
    assert_eq!(usage.len(), 1);
    assert_eq!(
        (
            usage[0]["prompt"].as_u64(),
            usage[0]["output"].as_u64(),
            usage[0]["reasoning"].as_u64()
        ),
        (Some(50), Some(30), Some(17))
    );
    let raw = usage[0]["raw"].as_array().unwrap();
    assert_eq!(raw.len(), 2);
    assert_eq!(raw[0]["_deepagent"]["modelId"], "audit-a");
    assert_eq!(raw[1]["_deepagent"]["modelId"], "audit-b");
    assert!(!raw[0]["_deepagent"]["providerId"].is_null());
    assert!(!usage[0].to_string().contains("h163-must-not-be-persisted"));
    let totals = CostService::new(db.clone()).summary(&session).unwrap();
    assert_eq!(
        (
            totals.input_tokens,
            totals.output_tokens,
            totals.cache_hit_tokens,
            totals.cache_miss_tokens
        ),
        (150, 30, 80, 70)
    );
    let rows: Vec<(String,u32,u32,Option<f64>)> = db.with_conn(|conn| {
        let mut query=conn.prepare("SELECT model,input_tokens,output_tokens,cost_yuan FROM costs WHERE session_id=?1 ORDER BY model")
            .map_err(|e| deepagent_core::error::CoreError::other(e.to_string()))?;
        let rows=query.query_map([&session], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?)))
            .map_err(|e| deepagent_core::error::CoreError::other(e.to_string()))?;
        Ok(rows.map(|r|r.unwrap()).collect())
    }).unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!((&*rows[0].0, rows[0].1, rows[0].2), ("audit-a", 100, 20));
    assert_eq!((&*rows[1].0, rows[1].1, rows[1].2), ("audit-b", 50, 10));
    assert!(
        rows.iter().all(|r| r.3.is_some()),
        "exact model pricing keys must survive"
    );
    let detail = AppService::from_shared(db.clone())
        .session_detail(&session)
        .unwrap();
    let ui = detail
        .conversation
        .iter()
        .filter_map(|message| message.usage.as_ref())
        .last()
        .expect("existing UI usage DTO");
    assert_eq!(
        (ui.prompt_tokens, ui.completion_tokens, ui.reasoning_tokens),
        (50, 30, 17)
    );
    assert!(
        ui.cost_yuan
            .is_some_and(|cost| (cost - 0.000310).abs() < 0.000000001),
        "reloaded workflow footer must retain the whole run's multi-model cost, got {:?}",
        ui.cost_yuan
    );
    assert_terminal(&db, "usage-multi", "succeeded");
}

#[tokio::test]
async fn partly_unreported_workflow_usage_must_not_publish_a_complete_measured_cost() {
    let mut unknown = completion(50, 10, 0, 5);
    unknown.as_object_mut().unwrap().remove("usage");
    let provider = Provider::start(vec![
        (200, Some(completion(100, 20, 80, 12))),
        (200, Some(unknown)),
    ])
    .await;
    let f = Fixture::new().await;
    let model = f.register(&provider, "openai", "audit-a", 0);
    let (ok, live) = f
        .run("usage-partly-unreported", workflow(&[&model, &model]))
        .await;
    assert!(ok);
    let receipts: Vec<_> = live
        .iter()
        .filter_map(|event| match event {
            RuntimeEvent::CostRecorded { cost_yuan, .. } => Some(*cost_yuan),
            _ => None,
        })
        .collect();
    assert_eq!(
        receipts,
        vec![None],
        "unreported tokens do not mean a free call"
    );
    let totals = f.cost.summary(&f.session).unwrap();
    assert_eq!((totals.input_tokens, totals.output_tokens), (100, 20));
    let usage = recorded_usage(&f.db, &f.session);
    assert_eq!(usage[0]["raw"].as_array().unwrap().len(), 2);
    assert_eq!(usage[0]["raw"][1]["_deepagent"]["usageReported"], false);
    let detail = AppService::from_shared(f.db.clone())
        .session_detail(&f.session)
        .unwrap();
    let ui = detail
        .conversation
        .iter()
        .filter_map(|message| message.usage.as_ref())
        .last()
        .unwrap();
    assert_eq!(
        ui.cost_yuan, None,
        "reloaded partial usage must remain unknown"
    );
}

#[tokio::test]
async fn workflow_cost_receipt_survives_fork_and_does_not_shift_a_legacy_cost_row() {
    let a = Provider::start(vec![(200, Some(completion(100, 20, 80, 12)))]).await;
    let b = Provider::start(vec![(200, Some(completion(50, 10, 0, 5)))]).await;
    let f = Fixture::new().await;
    let ra = f.register(&a, "openai", "audit-a", 0);
    let rb = f.register(&b, "openai", "audit-b", 0);
    assert!(f.run("usage-receipt-source", workflow(&[&ra, &rb])).await.0);
    let events = EventStore::new(&f.db)
        .load_session(f.session.parse().unwrap())
        .unwrap();
    let receipts: Vec<_> = events
        .iter()
        .filter_map(|event| match &event.payload {
            EventPayload::RunCostRecorded {
                version,
                task_id,
                ledger_ids,
                ..
            } => Some((*version, task_id, ledger_ids)),
            _ => None,
        })
        .collect();
    assert_eq!(receipts.len(), 1);
    assert_eq!(receipts[0].0, 1);
    assert!(receipts[0].1.is_some());
    assert_eq!(receipts[0].2.len(), 2);
    let clock = SystemClock;
    let fork = deepagent_session::Session::fork(
        &f.db,
        &clock,
        f.session.parse().unwrap(),
        events.last().unwrap().sequence,
    )
    .unwrap();
    let fork_ui = AppService::from_shared(f.db.clone())
        .session_detail(&fork.id().to_string())
        .unwrap();
    let cost = fork_ui
        .conversation
        .iter()
        .filter_map(|message| message.usage.as_ref())
        .last()
        .unwrap()
        .cost_yuan
        .unwrap();
    assert!((cost - 0.000310).abs() < 0.000000001);
    assert_eq!(
        f.cost.summary(&fork.id().to_string()).unwrap().input_tokens,
        0,
        "historical receipt is not a second ledger charge"
    );
    drop(fork);

    // Legacy-log control: a usage and row without a typed receipt, appended
    // after the new multi-model run. It must not inherit either workflow row.
    let mut session =
        deepagent_session::Session::recover(&f.db, &clock, f.session.parse().unwrap()).unwrap();
    session
        .append(EventPayload::MessageAppended {
            message: deepagent_core::message::Message::user("legacy turn"),
        })
        .unwrap();
    session.create_task("legacy projection fixture").unwrap();
    session
        .append(EventPayload::MessageAppended {
            message: deepagent_core::message::Message::assistant("legacy answer"),
        })
        .unwrap();
    session
        .append(EventPayload::UsageRecorded {
            prompt_tokens: 5,
            completion_tokens: 2,
            reasoning_tokens: 0,
            total_tokens: 7,
            prompt_cache_hit_tokens: 0,
            prompt_cache_miss_tokens: 5,
            duration_ms: 1,
            raw_responses_usage: None,
        })
        .unwrap();
    let legacy_cost = f
        .cost
        .record(deepagent_app_core::cost_service::CostRecordRequest {
            session_id: f.session.clone(),
            model: "audit-a".into(),
            input_tokens: 5,
            output_tokens: 2,
            cache_hit_tokens: 0,
            cache_miss_tokens: 5,
            total_tokens: 7,
        })
        .unwrap()
        .unwrap();
    drop(session);
    let (_root, db, id) = f.reopen();
    let detail = AppService::from_shared(db).session_detail(&id).unwrap();
    let costs: Vec<_> = detail
        .conversation
        .iter()
        .filter_map(|message| message.usage.as_ref())
        .map(|usage| usage.cost_yuan)
        .collect();
    assert_eq!(costs.len(), 2);
    assert!((costs[0].unwrap() - 0.000310).abs() < 0.000000001);
    assert_eq!(costs[1], Some(legacy_cost));
}

#[tokio::test]
async fn partly_unpriced_workflow_keeps_usage_but_does_not_publish_a_partial_total_as_complete() {
    let a = Provider::start(vec![(200, Some(completion(100, 20, 80, 12)))]).await;
    let b = Provider::start(vec![(200, Some(completion(50, 10, 0, 5)))]).await;
    let f = Fixture::new().await;
    let ra = f.register(&a, "openai", "audit-a", 0);
    let rb = f.register(&b, "openai", "audit-unpriced", 0);
    let (ok, live) = f.run("usage-partly-unpriced", workflow(&[&ra, &rb])).await;
    assert!(ok);
    let receipts: Vec<_> = live
        .iter()
        .filter_map(|event| match event {
            RuntimeEvent::CostRecorded { cost_yuan, .. } => Some(*cost_yuan),
            _ => None,
        })
        .collect();
    assert_eq!(receipts, vec![None]);
    let (_root, db, session) = f.reopen();
    let totals = CostService::new(db.clone()).summary(&session).unwrap();
    assert_eq!((totals.input_tokens, totals.output_tokens), (150, 30));
    let unpriced: u32 = db
        .with_conn(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM costs WHERE session_id=?1 AND model='audit-unpriced' AND cost_yuan IS NULL",
                [&session],
                |row| row.get(0),
            )
            .map_err(|e| deepagent_core::error::CoreError::other(e.to_string()))
        })
        .unwrap();
    assert_eq!(unpriced, 1);
    assert_terminal(&db, "usage-partly-unpriced", "succeeded");
}

#[tokio::test]
async fn later_output_failure_keeps_all_answered_calls_and_raw_usage() {
    let mut invalid = completion(50, 10, 0, 5);
    invalid["choices"] = json!([]);
    let provider = Provider::start(vec![
        (200, Some(completion(100, 20, 80, 12))),
        (200, Some(invalid)),
    ])
    .await;
    let f = Fixture::new().await;
    let model = f.register(&provider, "openai", "audit-a", 0);
    let (ok, _) = f.run("usage-failed", workflow(&[&model, &model])).await;
    assert!(!ok);
    let (_root, db, session) = f.reopen();
    let usage = recorded_usage(&db, &session);
    assert_eq!(usage.len(), 1);
    assert_eq!(usage[0]["raw"].as_array().unwrap().len(), 2);
    assert_eq!(usage[0]["output"], 30);
    let ledger = CostService::new(db.clone()).summary(&session).unwrap();
    assert_eq!((ledger.input_tokens, ledger.output_tokens), (150, 30));
    assert_terminal(&db, "usage-failed", "failed");
}

#[tokio::test]
async fn transient_candidate_fallback_accounts_each_reported_attempt_once() {
    let a = Provider::start(vec![(503, Some(completion(100, 20, 80, 12)))]).await;
    let b = Provider::start(vec![(200, Some(completion(50, 10, 0, 5)))]).await;
    let f = Fixture::new().await;
    f.register(&a, "openai", "audit-a", 10);
    f.register(&b, "openai", "audit-b", 0);
    let (ok, _) = f
        // Empty is the only canvas value that means automatic scenario routing;
        // a non-empty bare model name must never silently fall through.
        .run("usage-fallback", workflow(&[""]))
        .await;
    assert!(ok);
    assert_eq!(a.calls.lock().unwrap().len(), 1);
    assert_eq!(b.calls.lock().unwrap().len(), 1);
    let (_root, db, session) = f.reopen();
    let usage = recorded_usage(&db, &session);
    assert_eq!(usage[0]["raw"].as_array().unwrap().len(), 2);
    let ledger = CostService::new(db.clone()).summary(&session).unwrap();
    assert_eq!((ledger.input_tokens, ledger.output_tokens), (150, 30));
    assert_terminal(&db, "usage-fallback", "succeeded");
}

#[tokio::test]
async fn unreported_usage_stays_unknown_and_does_not_create_a_measured_zero_cost() {
    let mut payload = completion(100, 20, 80, 12);
    payload.as_object_mut().unwrap().remove("usage");
    let provider = Provider::start(vec![(200, Some(payload))]).await;
    let f = Fixture::new().await;
    let model = f.register(&provider, "openai", "audit-a", 0);
    let (ok, live) = f.run("usage-unknown", workflow(&[&model])).await;
    assert!(ok);
    assert!(!live.iter().any(|e| matches!(
        e,
        RuntimeEvent::Usage { .. } | RuntimeEvent::CostRecorded { .. }
    )));
    let usage = recorded_usage(&f.db, &f.session);
    assert_eq!(usage[0]["raw"][0]["_deepagent"]["usageReported"], false);
    assert!(usage[0]["raw"][0].get("prompt_tokens").is_none());
    let count: i64 =
        f.db.with_conn(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM costs WHERE session_id=?1",
                [&f.session],
                |row| row.get(0),
            )
            .map_err(|e| deepagent_core::error::CoreError::other(e.to_string()))
        })
        .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn anthropic_and_gemini_usage_cross_the_real_bridge_without_chat_field_assumptions() {
    for (protocol, model, payload) in [
        (
            "anthropic",
            "audit-anthropic",
            json!({"content":[{"type":"text","text":"answer"}],
            "usage":{"input_tokens":10,"cache_read_input_tokens":80,"cache_creation_input_tokens":10,"output_tokens":20}}),
        ),
        (
            "gemini",
            "audit-gemini",
            json!({"candidates":[{"content":{"parts":[{"text":"answer"}]}}],
            "usageMetadata":{"promptTokenCount":90,"toolUsePromptTokenCount":10,"cachedContentTokenCount":80,
                "candidatesTokenCount":8,"thoughtsTokenCount":12,"totalTokenCount":120}}),
        ),
    ] {
        let provider = Provider::start(vec![(200, Some(payload))]).await;
        let f = Fixture::new().await;
        let selected = f.register(&provider, protocol, model, 0);
        let (ok, _) = f.run("usage-adapter", workflow(&[&selected])).await;
        assert!(ok, "{protocol}");
        let ledger = f.cost.summary(&f.session).unwrap();
        assert_eq!(
            (
                ledger.input_tokens,
                ledger.output_tokens,
                ledger.cache_hit_tokens,
                ledger.cache_miss_tokens
            ),
            (100, 20, 80, 20)
        );
        let usage = recorded_usage(&f.db, &f.session);
        assert_eq!(usage[0]["raw"][0]["_deepagent"]["modelId"], model);
        if protocol == "gemini" {
            assert_eq!(usage[0]["reasoning"], 12);
        } else {
            assert_eq!(usage[0]["raw"][0]["cache_creation_input_tokens"], 10);
        }
    }
}

#[tokio::test]
async fn cancellation_while_next_request_waits_keeps_previous_usage_and_no_late_completion() {
    let provider =
        Provider::start(vec![(200, Some(completion(100, 20, 80, 12))), (200, None)]).await;
    let f = Fixture::new().await;
    let model = f.register(&provider, "openai", "audit-a", 0);
    let run = f.run("usage-cancel", workflow(&[&model, &model]));
    let cancel = async {
        provider.wait_calls(2).await;
        assert!(f.chat.request_cancel(&f.session).unwrap().accepted);
    };
    let ((_, live), ()) = tokio::join!(run, cancel);
    assert!(!live.iter().any(|event|matches!(event,RuntimeEvent::WorkflowNode{event}
        if event.node_id=="text-1" && event.status==deepagent_runtime::workflow::NodeExecutionStatus::Completed)));
    let (_root, db, session) = f.reopen();
    let usage = recorded_usage(&db, &session);
    assert_eq!(usage[0]["raw"].as_array().unwrap().len(), 1);
    assert_eq!(
        CostService::new(db.clone())
            .summary(&session)
            .unwrap()
            .input_tokens,
        100
    );
    assert_terminal(&db, "usage-cancel", "cancelled");
}

#[tokio::test]
async fn run_level_budget_and_truncated_stream_preserve_usage_without_snapshot_double_counting() {
    for truncated in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let db = Arc::new(Database::open(root.path().join("kernel.db")).unwrap());
        let report = completion(100, 20, 80, 12)["usage"].clone();
        let mut chunks = vec![
            json!({"choices":[{"delta":{"content":"answer"},"finish_reason":null}]}).to_string(),
            json!({"choices":[],"usage":report}).to_string(),
            json!({"choices":[{"delta":{},"finish_reason":"stop"}],"usage":report}).to_string(),
        ];
        if !truncated {
            chunks.push("[DONE]".into());
        }
        let client = Arc::new(ModelClient::new(
            Arc::new(MockTransport::new(chunks)),
            ModelConfig::deepseek_chat("sk-fixture-placeholder"),
        ));
        let request = workflow(&["ignored"]);
        let mut definition = request.definition;
        definition.nodes[1].kind = "llm".into();
        definition.nodes[1].config =
            serde_json::from_value(json!({"llmPrompt":"synthetic direct usage"})).unwrap();
        let mut agent = WorkflowAgent::new(
            compile(definition).unwrap(),
            Default::default(),
            None,
            NodeEventPublisher::default(),
        )
        .expect("valid workflow inputs")
        .with_model(client, "audit-a".into());
        let mut session = Session::create(&db, &SystemClock, Some("usage kernel")).unwrap();
        let task = session.create_task("usage").unwrap();
        let registry = ToolRegistry::new();
        let kernel = AgentKernel::<SystemClock>::new(
            db.clone(),
            &registry,
            Default::default(),
            RuntimeConfig {
                max_total_tokens: Some(119),
                ..Default::default()
            },
            "usage-kernel",
        );
        let terminal = kernel
            .start(RunRequest::new(&mut session, task, &mut agent))
            .await
            .unwrap();
        assert_eq!(
            terminal.kind.label(),
            if truncated {
                "failed"
            } else {
                "budget_exceeded"
            }
        );
        let session_id = session.id().to_string();
        drop(session);
        drop(kernel);
        drop(agent);
        drop(db);
        let reopened = Database::open(root.path().join("kernel.db")).unwrap();
        let usage = recorded_usage(&reopened, &session_id);
        assert_eq!(usage.len(), 1);
        assert_eq!(usage[0]["raw"].as_array().unwrap().len(), 1);
        assert_eq!(
            (
                usage[0]["prompt"].as_u64(),
                usage[0]["output"].as_u64(),
                usage[0]["reasoning"].as_u64()
            ),
            (Some(100), Some(20), Some(12))
        );
        assert!(!usage[0].to_string().contains("h163-must-not-be-persisted"));
        assert_terminal(&reopened, "usage-kernel", terminal.kind.label());
    }
}

#[tokio::test]
async fn canvas_usage_budget_stops_before_dispatching_the_next_node() {
    let provider = Provider::start(vec![(200, Some(completion(100, 20, 80, 12)))]).await;
    let f = Fixture::new().await;
    let selected = f.register(&provider, "openai", "audit-a", 0);
    let request = workflow(&[&selected, &selected]);
    let mut agent = WorkflowAgent::new(
        compile(request.definition).unwrap(),
        Default::default(),
        None,
        NodeEventPublisher::default(),
    )
    .expect("valid workflow inputs")
    .with_canvas_bridge(Arc::new(CanvasModelGateway::new(f.providers.clone())));
    let mut session = Session::create(&f.db, &SystemClock, Some("canvas budget")).unwrap();
    let task = session.create_task("budget").unwrap();
    let registry = ToolRegistry::new();
    let kernel = AgentKernel::<SystemClock>::new(
        f.db.clone(),
        &registry,
        Default::default(),
        RuntimeConfig {
            max_total_tokens: Some(119),
            ..Default::default()
        },
        "usage-canvas-budget",
    );
    let terminal = tokio::time::timeout(
        Duration::from_secs(5),
        kernel.start(RunRequest::new(&mut session, task, &mut agent)),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(terminal.kind.label(), "budget_exceeded");
    assert_eq!(provider.calls.lock().unwrap().len(), 1);
    let usage = recorded_usage(&f.db, &session.id().to_string());
    assert_eq!(usage[0]["raw"].as_array().unwrap().len(), 1);
    assert_eq!(usage[0]["output"], 20);
    assert_terminal(&f.db, "usage-canvas-budget", "budget_exceeded");
}

/// A synthetic stream reports charged usage, then stalls before [DONE]. The
/// real runtime cancellation must drop its observer and settle once.
struct UsageThenPending {
    received: Arc<Notify>,
}

#[async_trait::async_trait]
impl HttpTransport for UsageThenPending {
    async fn stream(
        &self,
        _request: TransportRequest,
        sink: &mut dyn deepagent_models::transport::EventSink,
    ) -> deepagent_core::error::Result<()> {
        let report = json!({"choices":[], "usage": completion(100,20,80,12)["usage"]}).to_string();
        sink.on_event(&report)?;
        sink.on_event(&report)?;
        self.received.notify_one();
        std::future::pending().await
    }
}

#[tokio::test]
async fn cancellation_after_stream_usage_settles_reported_snapshot_once() {
    let root = tempfile::tempdir().unwrap();
    let db = Arc::new(Database::open(root.path().join("cancel.db")).unwrap());
    let received = Arc::new(Notify::new());
    let flag = Arc::new(AtomicBool::new(false));
    let client = Arc::new(ModelClient::new(
        Arc::new(UsageThenPending {
            received: received.clone(),
        }),
        ModelConfig::deepseek_chat("sk-fixture-placeholder"),
    ));
    let mut definition = workflow(&["ignored"]).definition;
    definition.nodes[1].kind = "llm".into();
    definition.nodes[1].config =
        serde_json::from_value(json!({"llmPrompt":"synthetic cancel"})).unwrap();
    let mut agent = WorkflowAgent::new(
        compile(definition).unwrap(),
        Default::default(),
        None,
        NodeEventPublisher::default(),
    )
    .expect("valid workflow inputs")
    .with_model(client, "audit-a".into())
    .with_cancel(flag.clone());
    let mut session = Session::create(&db, &SystemClock, Some("stream cancel")).unwrap();
    let task = session.create_task("cancel").unwrap();
    let registry = ToolRegistry::new();
    let kernel = AgentKernel::<SystemClock>::new(
        db.clone(),
        &registry,
        Default::default(),
        RuntimeConfig::default(),
        "usage-stream-cancel",
    )
    .with_cancellation_flag(flag.clone());
    let cancel = async {
        received.notified().await;
        flag.store(true, Ordering::SeqCst);
    };
    let (result, ()) = tokio::time::timeout(Duration::from_secs(5), async {
        tokio::join!(
            kernel.start(RunRequest::new(&mut session, task, &mut agent)),
            cancel
        )
    })
    .await
    .unwrap();
    assert_eq!(result.unwrap().kind.label(), "cancelled");
    let measured = agent.model_usage();
    assert_eq!(measured.len(), 1);
    assert_eq!(
        (
            measured[0].usage.billed_prompt_tokens,
            measured[0].usage.completion_tokens
        ),
        (100, 20)
    );
    let session_id = session.id().to_string();
    drop(session);
    drop(agent);
    drop(kernel);
    drop(db);
    let reopened = Database::open(root.path().join("cancel.db")).unwrap();
    let usage = recorded_usage(&reopened, &session_id);
    assert_eq!(usage.len(), 1);
    assert_eq!(usage[0]["raw"].as_array().unwrap().len(), 1);
    assert_eq!(usage[0]["reasoning"], 12);
    assert_terminal(&reopened, "usage-stream-cancel", "cancelled");
}
