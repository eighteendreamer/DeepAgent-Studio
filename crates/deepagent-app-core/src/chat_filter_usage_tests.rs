//! H132: public Chat client -> original kernel/finalizer -> one durable ledger.
use super::*;
use crate::cost_service::{BudgetConfig, ModelPricing};
use crate::run_finalizer::{AppRunFinalizer, AppRunFinalizerRequest};
use deepagent_core::event::EventPayload;
use deepagent_models::{ModelClient, ModelConfig};
use deepagent_runtime::{Agent, AgentKernel, ModelAgent, RunRequest, RuntimeConfig, TerminalKind};
use deepagent_session::Session;
use deepagent_tools::ToolRegistry;
use deepagent_tracing::metrics::Metrics;
use serde_json::Value;
use std::sync::atomic::{AtomicUsize, Ordering};

struct Transport {
    events: Vec<String>,
    calls: AtomicUsize,
}
#[async_trait::async_trait]
impl deepagent_models::HttpTransport for Transport {
    async fn stream(
        &self,
        request: deepagent_models::TransportRequest,
        sink: &mut dyn deepagent_models::transport::EventSink,
    ) -> Result<()> {
        assert!(request.url.ends_with("/chat/completions"));
        assert_eq!(
            serde_json::from_str::<Value>(&request.body)?["model"],
            "deepseek-flash"
        );
        self.calls.fetch_add(1, Ordering::SeqCst);
        for event in &self.events {
            if sink.on_event(event)? {
                break;
            }
        }
        Ok(())
    }
}

fn measured() -> Value {
    json!({"prompt_tokens":9,"completion_tokens":2,"total_tokens":11,"prompt_cache_hit_tokens":4,"prompt_cache_miss_tokens":5,"completion_tokens_details":{"reasoning_tokens":1}})
}

async fn observe(sample: Option<Value>, filtered: bool) {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("state.sqlite");
    let db = Arc::new(Database::open(&path).unwrap());
    let clock = SystemClock;
    let mut session = Session::create(&db, &clock, None).unwrap();
    let sid = session.id().to_string();
    let task = session.create_task("synthetic filtered request").unwrap();
    let cost = Arc::new(CostService::new(db.clone()));
    cost.set_pricing(std::collections::HashMap::from([(
        "deepseek-flash".into(),
        ModelPricing {
            input_cache_hit_per_million: 100.0,
            input_cache_miss_per_million: 1000.0,
            output_per_million: 20.0,
        },
    )]));
    let has_usage = sample.as_ref().is_some_and(|value| !value.is_null());
    if has_usage {
        cost.set_budget(BudgetConfig {
            daily_limit: Some(0.001),
            monthly_limit: None,
        })
        .unwrap();
    }
    let mut chunk = json!({"id":"h132","object":"chat.completion.chunk","model":"deepseek-flash","choices":[{"index":0,"delta":if filtered{json!({})}else{json!({"content":"normal answer"})},"finish_reason":if filtered{"content_filter"}else{"stop"}}]});
    if let Some(sample) = sample {
        chunk["usage"] = sample;
    }
    let transport = Arc::new(Transport {
        events: vec![chunk.to_string(), "[DONE]".into()],
        calls: AtomicUsize::new(0),
    });
    let registry = ToolRegistry::new();
    let live = Arc::new(Events::default());
    let kernel = AgentKernel::new(
        db.clone(),
        &registry,
        Metrics::new(),
        RuntimeConfig::default(),
        "h132",
    )
    .with_events(live.clone());
    let sink = kernel.event_sink();
    let owner = RunCallUsageOwner {
        db: db.clone(),
        cost: Some(cost.clone()),
        session_id: sid.clone(),
        run_id: "h132".into(),
        events: Arc::downgrade(&sink),
    };
    let booked = owner.main_loop();
    let client = Arc::new(ModelClient::new(
        transport.clone(),
        ModelConfig::deepseek_chat("synthetic"),
    ));
    let mut agent = ModelAgent::new(
        client.clone(),
        "deepseek-flash",
        "rules",
        "synthetic",
        vec![],
    )
    .with_call_usage_sink(booked.clone())
    .with_max_model_attempts(3)
    .with_fallback_model(Some("other-model".into()));
    let terminal = kernel
        .start(RunRequest::new(&mut session, task, &mut agent))
        .await
        .unwrap();
    assert_eq!(
        terminal.kind,
        if filtered {
            TerminalKind::Failed
        } else {
            TerminalKind::Succeeded
        }
    );
    let routes = agent.model_usage();
    AppRunFinalizer::new(
        db.clone(),
        Some(cost.clone()),
        None,
        Default::default(),
        Default::default(),
    )
    .finalize_after_kernel(
        &mut session,
        AppRunFinalizerRequest {
            session_id: &sid,
            run_id: "h132",
            discovered_before_run: &Default::default(),
            discovered_tools: &Default::default(),
            usage: agent.cumulative_usage(),
            model_usage: Some(&routes),
            booked_calls: Some(&booked),
            multi_model_label: "synthetic",
            side_usage: None,
            model_name: "deepseek-flash",
            sink: sink.as_ref(),
            run_succeeded: !filtered,
            capture_client: None,
            capture_model: None,
        },
    )
    .unwrap();
    assert_eq!(
        transport.calls.load(Ordering::SeqCst),
        1,
        "filtered completion must not retry or switch route"
    );
    let receipts = CostStore::new(&db).run_call_receipts(&sid, "h132").unwrap();
    let rows = CostStore::new(&db).session_costs_detailed(&sid).unwrap();
    let summary = cost.summary(&sid).unwrap();
    assert_eq!(rows.len(), usize::from(has_usage));
    assert_eq!(summary.input_tokens, if has_usage { 9 } else { 0 });
    assert_eq!(summary.output_tokens, if has_usage { 2 } else { 0 });
    assert!((summary.session_cost - if has_usage { 0.00544 } else { 0.0 }).abs() < 1e-12);
    eprintln!(
        "H132 original accounting: receipts={}, rows={}, paid_input={}, filtered={}, reason={:?}",
        receipts.len(),
        rows.len(),
        summary.input_tokens,
        filtered,
        terminal.reason
    );
    if filtered {
        assert!(
            terminal
                .reason
                .as_deref()
                .unwrap()
                .contains("model stopped due to content filter"),
            "{:?}",
            terminal.reason
        );
    }
    assert_eq!(receipts.len(), 1);
    assert_eq!(receipts[0].report["usage_complete"], has_usage);
    assert_eq!(receipts[0].ledger_id.is_some(), has_usage);
    let events = RunStore::new(&db).events_after("h132", None).unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_type == "run_terminal")
            .count(),
        1
    );
    assert!(!events
        .iter()
        .any(|event| matches!(event.event_type.as_str(), "tool_started" | "tool_completed")));
    let history = EventStore::new(&db).load_session(session.id()).unwrap();
    assert_eq!(
        history
            .iter()
            .filter(|event| matches!(event.payload, EventPayload::UsageRecorded { .. }))
            .count(),
        usize::from(has_usage)
    );
    if has_usage {
        let receipt = events
            .iter()
            .find(|event| event.event_type == "model_call_usage")
            .unwrap();
        let end = events
            .iter()
            .find(|event| event.event_type == "run_terminal")
            .unwrap();
        assert!(receipt.sequence < end.sequence);
        let next_task = session.create_task("budget control").unwrap();
        let next_owner = RunCallUsageOwner {
            run_id: "h132-next".into(),
            ..owner.clone()
        };
        let mut next = ModelAgent::new(client.clone(), "deepseek-flash", "rules", "next", vec![])
            .with_call_usage_sink(next_owner.main_loop());
        let next_kernel = AgentKernel::new(
            db.clone(),
            &registry,
            Metrics::new(),
            RuntimeConfig::default(),
            "h132-next",
        );
        let stopped = next_kernel
            .start(RunRequest::new(&mut session, next_task, &mut next))
            .await
            .unwrap();
        assert_eq!(stopped.kind, TerminalKind::BudgetExceeded);
        assert_eq!(transport.calls.load(Ordering::SeqCst), 1);
        assert!(CostStore::new(&db)
            .run_call_receipts(&sid, "h132-next")
            .unwrap()
            .is_empty());
    }
    let final_history = EventStore::new(&db).load_session(session.id()).unwrap();
    drop(agent);
    drop(client);
    drop(booked);
    drop(owner);
    drop(sink);
    drop(kernel);
    drop(session);
    drop(cost);
    drop(db);
    let reopened = Arc::new(Database::open(&path).unwrap());
    assert_eq!(
        RunStore::new(&reopened).events_after("h132", None).unwrap(),
        events
    );
    assert_eq!(
        EventStore::new(&reopened)
            .load_session(sid.parse().unwrap())
            .unwrap(),
        final_history
    );
    assert_eq!(
        CostStore::new(&reopened)
            .session_costs_detailed(&sid)
            .unwrap(),
        rows
    );
    assert_eq!(
        serde_json::to_value(
            CostStore::new(&reopened)
                .run_call_receipts(&sid, "h132")
                .unwrap()
        )
        .unwrap(),
        serde_json::to_value(receipts).unwrap()
    );
    assert_eq!(CostService::new(reopened).summary(&sid).unwrap(), summary);
}

#[tokio::test]
async fn h132_filtered_usage_and_unreported_controls_survive_kernel_finalizer_and_reopen() {
    for sample in [Some(measured()), None, Some(Value::Null)] {
        observe(sample, true).await;
    }
    observe(Some(measured()), false).await;
}
