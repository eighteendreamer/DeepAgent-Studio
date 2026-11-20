//! Opt-in validation of the final-request budget gate against a real provider.

use super::HookedReactiveContextCompactor;
use crate::input_runtime::response_items_from_events;
use deepagent_context::ContextPolicy;
use deepagent_core::clock::SystemClock;
use deepagent_core::error::CoreError;
use deepagent_core::event::EventPayload;
use deepagent_core::response_item::ResponseInputItem;
use deepagent_hooks::HookRegistry;
use deepagent_models::transport::{EventSink, HttpTransport, TransportRequest};
use deepagent_models::usage::{CallUsageReport, CallUsageSink};
use deepagent_models::{ModelClient, ModelConfig, ReasoningEffort, ReqwestTransport};
use deepagent_persistence::{event_store::EventStore, Database};
use deepagent_runtime::{Agent, AgentDecision, ModelAgent};
use deepagent_session::Session;
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct Receipts(Mutex<Vec<CallUsageReport>>);

impl CallUsageSink for Receipts {
    fn record(&self, report: CallUsageReport) {
        self.0.lock().unwrap().push(report);
    }
}

impl Receipts {
    fn take_one(&self, phase: &str) -> deepagent_models::Usage {
        let mut reports = self.0.lock().unwrap();
        assert_eq!(reports.len(), 1, "{phase}: expected one physical request");
        let report = reports.pop().unwrap();
        let usage = report.usage.expect("provider did not report usage");
        let created = report
            .raw_usage
            .as_ref()
            .and_then(|raw| raw["cache_creation_input_tokens"].as_u64());
        println!(
            "phase={phase} input={} hit={} miss={} created={created:?}",
            usage.prompt_tokens, usage.prompt_cache_hit_tokens, usage.prompt_cache_miss_tokens
        );
        usage
    }
}

struct RecordingTransport {
    inner: ReqwestTransport,
    inputs: Mutex<Vec<Vec<ResponseInputItem>>>,
}

#[async_trait::async_trait]
impl HttpTransport for RecordingTransport {
    async fn stream(
        &self,
        request: TransportRequest,
        sink: &mut dyn EventSink,
    ) -> deepagent_core::error::Result<()> {
        let wire: serde_json::Value = serde_json::from_str(&request.body).unwrap();
        self.inputs
            .lock()
            .unwrap()
            .push(serde_json::from_value(wire["input"].clone()).unwrap());
        self.inner.stream(request, sink).await
    }
}

fn text(role: &str, content: impl Into<String>) -> ResponseInputItem {
    ResponseInputItem::Message {
        role: role.into(),
        content: content.into(),
    }
}

#[tokio::test]
#[ignore = "uses provider quota; set DEEPAGENT_CACHE_TEST_KEY, _URL, and _MODEL for Responses"]
async fn real_request_budget_compaction_survives_reopen_and_reports_cache() {
    let env = |name| std::env::var(name).expect("set the cache probe configuration");
    let model = env("DEEPAGENT_CACHE_TEST_MODEL");
    let mut config = ModelConfig::deepseek(env("DEEPAGENT_CACHE_TEST_KEY"));
    config.base_url = env("DEEPAGENT_CACHE_TEST_URL");
    let mut capability = config.capability(&model);
    // The synthetic local window drives the production budget gate without
    // needing to send enough data to fill the provider's real context window.
    capability.context_window = 24_000;
    capability.max_output_tokens = 512;
    config.capabilities.push(capability.clone());
    let transport = Arc::new(RecordingTransport {
        inner: ReqwestTransport::new(),
        inputs: Mutex::default(),
    });
    let client = Arc::new(ModelClient::new(transport.clone(), config));
    let policy = ContextPolicy::for_capability(&capability, ReasoningEffort::None);
    // The summary names the transcript directory. Keep this path relative so
    // the fixture never sends an absolute host path to the provider.
    let dir = tempfile::Builder::new()
        .prefix("p8-fixture-")
        .tempdir_in(".")
        .unwrap();
    let cwd = std::env::current_dir().unwrap();
    let path = dir
        .path()
        .strip_prefix(&cwd)
        .unwrap_or(dir.path())
        .join("cache.sqlite3");
    assert!(path.is_relative());
    let mut db = Arc::new(Database::open(&path).unwrap());
    assert!(db.data_root().is_relative());
    let id = Session::create(&db, &SystemClock, None).unwrap().id();
    let rules = format!(
        "Synthetic fixture {id}. Follow the latest user request and reply with its marker only.\n{}",
        (0..320)
            .map(|n| format!("Archive guide {n}: garden meadow river archive.\n"))
            .collect::<String>()
    );
    let mut session = Session::recover(&db, &SystemClock, id).unwrap();
    for index in 0..12 {
        session
            .append_response_item(text(
                "user",
                format!(
                    "Synthetic archived entry {index}: {}",
                    "garden meadow river archive ".repeat(150)
                ),
            ))
            .unwrap();
        session
            .append_response_item(text("assistant", format!("Archived entry {index}.")))
            .unwrap();
    }
    drop(session);

    let main = Arc::new(Receipts::default());
    let summary = Arc::new(Receipts::default());
    let mut main_usage = Vec::new();
    for (index, phase) in ["cold", "before", "compacted", "reopened", "follow-up"]
        .into_iter()
        .enumerate()
    {
        let mut session = Session::recover(&db, &SystemClock, id).unwrap();
        let store = EventStore::new(&db);
        let history = response_items_from_events(&store.load_session(id).unwrap());
        let marker = format!("CACHE-AUDIT-{index}");
        let padding = if phase == "compacted" {
            "garden meadow river archive ".repeat(720)
        } else {
            String::new()
        };
        let goal = format!("Synthetic context: {padding}\nReply only: {marker}");
        session.append_response_item(text("user", &goal)).unwrap();
        let compactor = HookedReactiveContextCompactor::new(
            client.clone(),
            model.clone(),
            Arc::new(HookRegistry::new()),
            id,
            db.clone(),
        )
        .with_call_usage_sink(summary.clone());
        let decision = ModelAgent::new(client.clone(), &model, &rules, &goal, vec![])
            .with_response_history(history)
            .with_snip_tool(deepagent_builtins::SNIP_HISTORY_TOOL_NAME)
            .with_context_policy(policy.clone())
            .with_reasoning_effort(ReasoningEffort::None)
            .with_cache_session(id)
            .with_max_model_attempts(1)
            .with_reactive_compactor(Arc::new(compactor))
            .with_call_usage_sink(main.clone())
            .think(0, &[])
            .await
            .unwrap_or_else(|error| {
                let status = match error {
                    CoreError::Provider { status, .. } => status,
                    _ => None,
                };
                panic!("{phase}: model step failed (HTTP status={status:?})");
            });
        let AgentDecision::CompleteItems { message, items } = decision else {
            panic!("{phase}: expected a completed model step");
        };
        main_usage.push(main.take_one(phase));
        if phase == "compacted" {
            summary.take_one("summary");
        } else {
            assert!(
                summary.0.lock().unwrap().is_empty(),
                "{phase}: unexpected summary request"
            );
        }
        let events = store.load_session(id).unwrap();
        let boundaries: Vec<_> = events
            .iter()
            .filter_map(|event| match &event.payload {
                EventPayload::ContextCompacted {
                    tokens_before,
                    tokens_after,
                    ..
                } => Some((*tokens_before, *tokens_after)),
                _ => None,
            })
            .collect();
        assert_eq!(
            boundaries.len(),
            usize::from(index >= 2),
            "{phase}: wrong boundary count"
        );
        let replay = response_items_from_events(&events);
        {
            let requests = transport.inputs.lock().unwrap();
            assert!(
                requests.last() == Some(&replay),
                "{phase}: replay differs from wire input"
            );
        }
        assert!(
            replay.last() == Some(&text("user", format!("{goal}\n[id:u{}]", 13 + index))),
            "{phase}: current user turn changed"
        );
        assert_eq!(
            message.content.trim(),
            marker,
            "{phase}: incorrect visible response"
        );
        if phase == "compacted" {
            let (before, after) = boundaries[0];
            assert!(after < before);
            println!(
                "compaction estimated_before={before} estimated_after={after} durable_boundaries=1"
            );
        }
        for item in items {
            session.append_response_item(item).unwrap();
        }
        if phase == "compacted" {
            let committed = response_items_from_events(&store.load_session(id).unwrap());
            drop(session);
            db = Arc::new(Database::open(&path).unwrap());
            let recovered =
                response_items_from_events(&EventStore::new(&db).load_session(id).unwrap());
            assert!(recovered == committed, "reopen altered compacted history");
        }
    }
    assert!(
        main_usage[1].prompt_cache_hit_tokens > 0,
        "pre-compaction warm request missed"
    );
    assert!(
        main_usage[3..]
            .iter()
            .any(|usage| usage.prompt_cache_hit_tokens > 0),
        "cache never recovered after compaction"
    );
}
