//! H144: real Kernel cancellation, tool effects and the original durable ledger.
use super::*;
use crate::cost_service::{BudgetConfig, ModelPricing};
use crate::run_finalizer::{AppRunFinalizer, AppRunFinalizerRequest};
use deepagent_builtins::{file_tools::WriteFileTool, WorkspaceRoot};
use deepagent_core::{event::EventPayload, response_item::ResponseItem};
use deepagent_models::{ModelClient, ModelConfig, ToolSchema};
use deepagent_runtime::{
    Agent, AgentKernel, ModelAgent, RunHandle, RunRequest, RuntimeConfig, TerminalKind,
};
use deepagent_session::Session;
use deepagent_tools::ToolRegistry;
use deepagent_tracing::metrics::Metrics;
use serde_json::Value;
use std::collections::VecDeque;

#[derive(Clone, Copy, Debug)]
enum Scenario {
    Abort,
    ResourceExhausted,
    TransportRetry,
    LocalCancel,
    RetryBudget,
    Normal,
}

impl Scenario {
    fn expected(self) -> (TerminalKind, usize, bool) {
        match self {
            Self::Abort => (TerminalKind::Failed, 1, false),
            Self::ResourceExhausted => (TerminalKind::Failed, 1, false),
            Self::TransportRetry => (TerminalKind::Succeeded, 3, true),
            Self::LocalCancel => (TerminalKind::Cancelled, 1, false),
            Self::RetryBudget => (TerminalKind::BudgetExceeded, 1, false),
            Self::Normal => (TerminalKind::Succeeded, 2, true),
        }
    }

    fn program(self, sample: Option<Value>) -> VecDeque<Vec<String>> {
        let aborted = || {
            chunk(
                "aborted",
                Some(("rejected-call", "forbidden.txt")),
                sample.clone(),
            )
        };
        let resource = || {
            chunk(
                "insufficient_system_resource",
                Some(("rejected-call", "forbidden.txt")),
                sample.clone(),
            )
        };
        let approved = || {
            chunk(
                "tool_calls",
                Some(("approved-call", "effect.txt")),
                sample.clone(),
            )
        };
        let interrupted = || {
            let mut event: Value = serde_json::from_str(
                &chunk(
                    "tool_calls",
                    Some(("rejected-call", "forbidden.txt")),
                    sample.clone(),
                )[0],
            )
            .unwrap();
            event["choices"][0]["finish_reason"] = Value::Null;
            vec![event.to_string(), "__CONNECTION_ERROR__".into()]
        };
        let normal = || chunk("stop", None, sample.clone());
        match self {
            Self::Abort | Self::LocalCancel => vec![aborted(), normal()],
            Self::ResourceExhausted => vec![resource(), resource()],
            Self::TransportRetry => vec![interrupted(), approved(), normal()],
            Self::RetryBudget => vec![interrupted(), normal()],
            Self::Normal => vec![approved(), normal()],
        }
        .into()
    }
}

fn measured() -> Value {
    json!({"prompt_tokens":9,"completion_tokens":2,"total_tokens":11,
        "prompt_cache_hit_tokens":4,"prompt_cache_miss_tokens":5,
        "completion_tokens_details":{"reasoning_tokens":1}})
}

fn chunk(reason: &str, call: Option<(&str, &str)>, sample: Option<Value>) -> Vec<String> {
    let delta = if let Some((id, path)) = call {
        json!({"tool_calls":[{"index":0,"id":id,"type":"function","function":{"name":"write_file","arguments":json!({"path":path,"content":"synthetic effect"}).to_string()}}]})
    } else {
        json!({"content":"synthetic done"})
    };
    let mut event = json!({"choices":[{"index":0,"delta":delta,"finish_reason":reason}]});
    if let Some(sample) = sample {
        event["usage"] = sample;
    }
    vec![event.to_string(), "[DONE]".into()]
}

pub(super) struct Transport {
    program: Mutex<VecDeque<Vec<String>>>,
    requests: Mutex<Vec<Value>>,
    cancel: Option<RunHandle>,
}

#[async_trait::async_trait]
impl deepagent_models::HttpTransport for Transport {
    async fn stream(
        &self,
        request: deepagent_models::TransportRequest,
        sink: &mut dyn deepagent_models::transport::EventSink,
    ) -> Result<()> {
        assert!(request.url.ends_with("/chat/completions"));
        let body: Value = serde_json::from_str(&request.body)?;
        assert!(
            !request.body.contains("rejected-call"),
            "an interrupted invocation must never enter subsequent history"
        );
        self.requests.lock().unwrap().push(body);
        let events = self
            .program
            .lock()
            .unwrap()
            .pop_front()
            .expect("bounded synthetic program");
        let mut result = Ok(());
        for event in events {
            if event == "__CONNECTION_ERROR__" {
                result = Err(CoreError::other("synthetic connection interrupted"));
                break;
            }
            match sink.on_event(&event) {
                Ok(true) => break,
                Ok(false) => {}
                Err(error) => {
                    result = Err(error);
                    break;
                }
            }
        }
        if let Some(handle) = &self.cancel {
            assert!(handle.cancel());
        }
        result
    }
}

async fn observe(scenario: Scenario, sample: Option<Value>) {
    let measured = sample.as_ref().is_some_and(|sample| !sample.is_null());
    let (expected, calls, writes) = scenario.expected();
    let temp = tempfile::tempdir().unwrap();
    let workspace = temp.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let path = temp.path().join("state.sqlite");
    let db = Arc::new(Database::open(&path).unwrap());
    let clock = SystemClock;
    let mut session = Session::create(&db, &clock, None).unwrap();
    let sid = session.id().to_string();
    let task = session
        .create_task("synthetic interrupted request")
        .unwrap();
    let cost = Arc::new(CostService::new(db.clone()));
    cost.set_pricing(
        ["deepseek-flash", "deepseek-v4-pro"]
            .into_iter()
            .map(|model| {
                (
                    model.into(),
                    ModelPricing {
                        input_cache_hit_per_million: 100.0,
                        input_cache_miss_per_million: 1000.0,
                        output_per_million: 20.0,
                    },
                )
            })
            .collect(),
    );
    if matches!(scenario, Scenario::RetryBudget) {
        cost.set_budget(BudgetConfig {
            daily_limit: Some(0.001),
            monthly_limit: None,
        })
        .unwrap();
    }
    let mut registry = ToolRegistry::new();
    registry
        .register(Arc::new(WriteFileTool::new(WorkspaceRoot::new(&workspace))))
        .unwrap();
    let descriptor = &registry.get("write_file").unwrap().descriptor;
    let schemas = vec![ToolSchema::function(
        &descriptor.name,
        &descriptor.description,
        descriptor.parameters.clone(),
    )];
    let live = Arc::new(Events::default());
    let checkpoint = Arc::new(
        deepagent_runtime::CheckpointManager::new(
            db.clone(),
            "h144",
            0,
            &workspace,
            temp.path().join("checkpoints"),
        )
        .unwrap(),
    );
    let kernel = AgentKernel::new(
        db.clone(),
        &registry,
        Metrics::new(),
        RuntimeConfig {
            max_steps: Some(4),
            checkpoint: Some(checkpoint),
            action_persistence: Some(
                deepagent_runtime::ToolActionPersistence::new(db.clone(), "h144", "h144").unwrap(),
            ),
            ..Default::default()
        },
        "h144",
    )
    .with_events(live);
    let sink = kernel.event_sink();
    let owner = RunCallUsageOwner {
        db: db.clone(),
        cost: Some(cost.clone()),
        session_id: sid.clone(),
        run_id: "h144".into(),
        events: Arc::downgrade(&sink),
    };
    let booked = owner.main_loop();
    let transport = Arc::new(Transport {
        program: Mutex::new(scenario.program(sample)),
        requests: Mutex::new(Vec::new()),
        cancel: matches!(scenario, Scenario::LocalCancel).then(|| kernel.handle()),
    });
    let client = Arc::new(ModelClient::new(
        transport.clone(),
        ModelConfig::deepseek_chat("synthetic"),
    ));
    let mut agent = ModelAgent::new(
        client.clone(),
        "deepseek-flash",
        "rules",
        "synthetic",
        schemas,
    )
    .with_call_usage_sink(booked.clone())
    .with_max_model_attempts(2);
    let terminal = tokio::time::timeout(
        std::time::Duration::from_secs(6),
        kernel.start(RunRequest::new(&mut session, task, &mut agent)),
    )
    .await
    .unwrap()
    .unwrap();
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
            run_id: "h144",
            discovered_before_run: &Default::default(),
            discovered_tools: &Default::default(),
            usage: agent.cumulative_usage(),
            model_usage: Some(&routes),
            booked_calls: Some(&booked),
            multi_model_label: "synthetic",
            side_usage: None,
            model_name: "deepseek-flash",
            sink: sink.as_ref(),
            run_succeeded: terminal.kind == TerminalKind::Succeeded,
            capture_client: None,
            capture_model: None,
        },
    )
    .unwrap();
    let receipts = CostStore::new(&db).run_call_receipts(&sid, "h144").unwrap();
    let rows = CostStore::new(&db).session_costs_detailed(&sid).unwrap();
    let summary = cost.summary(&sid).unwrap();
    let requests = transport.requests.lock().unwrap().clone();
    eprintln!("H144 {scenario:?}: terminal={:?} calls={} receipt={} cost_rows={} paid_input={} reason={:?}",terminal.kind,requests.len(),receipts.len(),rows.len(),summary.input_tokens,terminal.reason);
    assert_eq!(terminal.kind, expected);
    assert_eq!(requests.len(), calls);
    assert_eq!(
        kernel.handle().is_cancelled(),
        matches!(scenario, Scenario::LocalCancel)
    );
    if matches!(scenario, Scenario::Abort | Scenario::ResourceExhausted) {
        let reason = terminal.reason.as_deref().unwrap();
        assert!(!reason.contains("unsupported"), "{reason}");
        assert!(reason.contains(if matches!(scenario, Scenario::Abort) {
            "aborted"
        } else {
            "insufficient_system_resource"
        }));
    }
    assert_eq!(receipts.len(), calls);
    assert!(receipts
        .iter()
        .all(|receipt| receipt.report["usage_complete"] == measured
            && receipt.ledger_id.is_some() == measured));
    assert_eq!(rows.len(), if measured { calls } else { 0 });
    assert_eq!(
        (summary.input_tokens, summary.output_tokens),
        (
            if measured { 9 * calls as u64 } else { 0 },
            if measured { 2 * calls as u64 } else { 0 }
        )
    );
    assert!(
        (summary.session_cost
            - if measured {
                0.00544 * calls as f64
            } else {
                0.0
            })
        .abs()
            < 1e-12
    );
    assert!(
        receipts
            .iter()
            .all(|r| r.report["model_id"] == "deepseek-flash"),
        "overload must not route calls to another model"
    );
    assert!(requests.iter().all(|r| r["model"] == "deepseek-flash"));
    assert!(!workspace.join("forbidden.txt").exists());
    assert_eq!(workspace.join("effect.txt").exists(), writes);
    if writes {
        assert_eq!(
            std::fs::read_to_string(workspace.join("effect.txt")).unwrap(),
            "synthetic effect"
        );
    }
    let events = RunStore::new(&db).events_after("h144", None).unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|e| e.event_type == "run_terminal")
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|e| e.event_type == "tool_started")
            .count(),
        usize::from(writes)
    );
    assert_eq!(
        events
            .iter()
            .filter(|e| e.event_type == "tool_completed")
            .count(),
        usize::from(writes)
    );
    assert!(events
        .windows(2)
        .all(|pair| pair[0].sequence < pair[1].sequence));
    let history = EventStore::new(&db).load_session(session.id()).unwrap();
    assert!(!serde_json::to_string(&history)
        .unwrap()
        .contains("rejected-call"));
    assert_eq!(history.iter().filter(|e|matches!(&e.payload,EventPayload::ResponseItemAppended{item:ResponseItem::FunctionCall{call_id,..}, .. }if call_id=="approved-call")).count(),usize::from(writes));
    assert_eq!(
        history
            .iter()
            .filter(|e| matches!(&e.payload, EventPayload::UsageRecorded { .. }))
            .count(),
        usize::from(measured)
    );
    let record = RunStore::new(&db).get("h144").unwrap().unwrap();
    let actions = deepagent_persistence::run_control::RunControlStore::new(&db)
        .list_actions("h144")
        .unwrap();
    assert_eq!(actions.len(), usize::from(writes));
    for action in &actions {
        assert_eq!(action.call_id, "approved-call");
        assert_eq!(
            action.state,
            deepagent_persistence::run_control::RunActionState::Completed
        );
    }
    assert!(
        deepagent_persistence::run_control::RunControlStore::new(&db)
            .list_approvals("h144")
            .unwrap()
            .is_empty()
    );
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
        RunStore::new(&reopened).get("h144").unwrap().unwrap(),
        record
    );
    assert_eq!(
        RunStore::new(&reopened).events_after("h144", None).unwrap(),
        events
    );
    assert_eq!(
        EventStore::new(&reopened)
            .load_session(sid.parse().unwrap())
            .unwrap(),
        history
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
                .run_call_receipts(&sid, "h144")
                .unwrap()
        )
        .unwrap(),
        serde_json::to_value(receipts).unwrap()
    );
    assert_eq!(
        deepagent_persistence::run_control::RunControlStore::new(&reopened)
            .list_actions("h144")
            .unwrap(),
        actions
    );
    assert!(
        deepagent_persistence::run_control::RunControlStore::new(&reopened)
            .list_approvals("h144")
            .unwrap()
            .is_empty()
    );
    assert_eq!(CostService::new(reopened).summary(&sid).unwrap(), summary);
    assert_eq!(
        transport.requests.lock().unwrap().len(),
        calls,
        "cold reopen must not dispatch"
    );
}

#[tokio::test]
async fn h144_provider_abort_and_local_cancel_keep_distinct_durable_outcomes_and_usage() {
    for sample in [Some(measured()), None, Some(Value::Null)] {
        observe(Scenario::Abort, sample).await;
    }
    observe(Scenario::LocalCancel, Some(measured())).await;
}

#[tokio::test]
async fn h144_terminal_overload_transport_retry_budget_and_real_write_survive_reopen() {
    for sample in [Some(measured()), None, Some(Value::Null)] {
        observe(Scenario::ResourceExhausted, sample).await;
    }
    for scenario in [
        Scenario::TransportRetry,
        Scenario::RetryBudget,
        Scenario::Normal,
    ] {
        observe(scenario, Some(measured())).await;
    }
}

pub(super) fn text_client(reason: &str, text: &str) -> (Arc<ModelClient>, Arc<Transport>) {
    text_client_with_usage(reason, text, Some(measured()))
}

fn text_client_with_usage(
    reason: &str,
    text: &str,
    sample: Option<Value>,
) -> (Arc<ModelClient>, Arc<Transport>) {
    let mut terminal = json!({"choices":[{"delta":{},"finish_reason":reason}]});
    if let Some(sample) = sample {
        terminal["usage"] = sample;
    }
    let transport = Arc::new(Transport {
        program: Mutex::new(VecDeque::from([vec![
            json!({"choices":[{"delta":{"content":text},"finish_reason":null}]}).to_string(),
            terminal.to_string(),
            "[DONE]".into(),
        ]])),
        requests: Mutex::new(Vec::new()),
        cancel: None,
    });
    (
        Arc::new(ModelClient::new(
            transport.clone(),
            ModelConfig::deepseek_chat("synthetic"),
        )),
        transport,
    )
}

#[tokio::test]
async fn h144_side_classifier_does_not_adopt_interrupted_valid_json_and_still_books_once() {
    use deepagent_runtime::stall_detector::StallClassifier;
    for reason in [
        "aborted",
        "insufficient_system_resource",
        "content_filter",
        "stop",
    ] {
        let (owner, _) = setup();
        let (client, transport) = text_client(
            reason,
            r#"{"category":"not_stalled_complete","confidence":0.9,"evidence":"synthetic"}"#,
        );
        let classifier =
            crate::stall_classifier::ModelStallClassifier::new(client, "deepseek-flash")
                .with_call_usage(owner.clone());
        let result = classifier.classify("synthetic transcript").await;
        assert_eq!(result.is_some(), reason == "stop", "{reason}");
        assert_eq!(transport.requests.lock().unwrap().len(), 1);
        let receipts = CostStore::new(&owner.db)
            .run_call_receipts(&owner.session_id, "run")
            .unwrap();
        assert_eq!(receipts.len(), 1);
        assert_eq!(receipts[0].report["usage"]["prompt_tokens"], 9);
        let events = RunStore::new(&owner.db).events_after("run", None).unwrap();
        let finished: Vec<_> = events
            .iter()
            .filter(|event| event.event_type == "model_side_call_finished")
            .collect();
        assert_eq!(finished.len(), 1);
        assert_eq!(
            finished[0].data["outcome"],
            if reason == "stop" {
                "completed"
            } else {
                "failed"
            }
        );
    }
}

#[tokio::test]
async fn h144_planning_does_not_adopt_interrupted_json() {
    for reason in [
        "aborted",
        "insufficient_system_resource",
        "content_filter",
        "stop",
    ] {
        let (client, _) = text_client(
            reason,
            r#"[{"id":"one","goal":"synthetic task","depends_on":[]}]"#,
        );
        let plan =
            crate::dag_orchestration::plan_dag_via_model(&client, "deepseek-flash", "synthetic")
                .await;
        assert_eq!(plan.is_ok(), reason == "stop", "{reason}");
    }
}

#[tokio::test]
async fn h144_advisory_command_classification_does_not_adopt_interrupted_text() {
    use crate::command_guard_llm::{CommandClassifier, CommandVerdict, ModelCommandClassifier};
    for reason in [
        "aborted",
        "insufficient_system_resource",
        "content_filter",
        "stop",
    ] {
        let (client, _) = text_client(reason, "INJECTION: synthetic control");
        let verdict = ModelCommandClassifier::new(client, "deepseek-flash")
            .classify("synthetic command text; never executed")
            .await;
        assert_eq!(
            matches!(verdict, CommandVerdict::Injection(_)),
            reason == "stop",
            "{reason}"
        );
    }
}

#[tokio::test]
async fn h144_legacy_workflow_chat_cannot_complete_a_node_from_interrupted_text_and_reopens_once() {
    use deepagent_runtime::workflow::{
        compile, NodeEventPublisher, NodeExecutionEvent, NodeExecutionStatus, WorkflowAgent,
    };
    for reason in [
        "aborted",
        "insufficient_system_resource",
        "content_filter",
        "stop",
    ] {
        for sample in [Some(measured()), None, Some(Value::Null)] {
            let has_usage = sample.as_ref().is_some_and(|sample| !sample.is_null());
            let successful = reason == "stop";
            let temp = tempfile::tempdir().unwrap();
            let path = temp.path().join("workflow.sqlite");
            let db = Arc::new(Database::open(&path).unwrap());
            let clock = SystemClock;
            let mut session = Session::create(&db, &clock, None).unwrap();
            let sid = session.id().to_string();
            let task = session
                .create_task("synthetic native workflow chat")
                .unwrap();
            let registry = ToolRegistry::new();
            let kernel = AgentKernel::new(
                db.clone(),
                &registry,
                Metrics::new(),
                RuntimeConfig {
                    max_steps: Some(4),
                    ..Default::default()
                },
                "h144-workflow",
            );
            let sink = kernel.event_sink();
            let cost = Arc::new(CostService::new(db.clone()));
            cost.set_pricing(std::collections::HashMap::from([(
                "deepseek-flash".into(),
                ModelPricing {
                    input_cache_hit_per_million: 100.0,
                    input_cache_miss_per_million: 1000.0,
                    output_per_million: 20.0,
                },
            )]));
            let owner = RunCallUsageOwner {
                db: db.clone(),
                cost: Some(cost.clone()),
                session_id: sid.clone(),
                run_id: "h144-workflow".into(),
                events: Arc::downgrade(&sink),
            };
            let booked = owner.main_loop();
            let (client, transport) =
                text_client_with_usage(reason, "H144_WORKFLOW_PARTIAL", sample);
            let compiled=compile(serde_json::from_value(json!({"version":1,"nodes":[
                {"id":"start","kind":"start"},
                {"id":"llm","kind":"llm","config":{"llmPrompt":"synthetic"}},
                {"id":"end","kind":"end"}
            ],"edges":[{"id":"a","source":"start","target":"llm"},{"id":"b","source":"llm","target":"end"}]})).unwrap()).unwrap();
            let mut agent = WorkflowAgent::new(
                compiled,
                serde_json::Map::new(),
                None,
                NodeEventPublisher::default(),
            )
            .unwrap()
            .with_model(client.clone(), "deepseek-flash".into())
            .with_call_usage_sink(booked.clone());
            let terminal = kernel
                .start(RunRequest::new(&mut session, task, &mut agent))
                .await
                .unwrap();
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
                    run_id: "h144-workflow",
                    discovered_before_run: &Default::default(),
                    discovered_tools: &Default::default(),
                    usage: agent.cumulative_usage(),
                    model_usage: Some(&routes),
                    booked_calls: Some(&booked),
                    multi_model_label: "synthetic",
                    side_usage: None,
                    model_name: "deepseek-flash",
                    sink: sink.as_ref(),
                    run_succeeded: successful,
                    capture_client: None,
                    capture_model: None,
                },
            )
            .unwrap();
            let events = RunStore::new(&db)
                .events_after("h144-workflow", None)
                .unwrap();
            if !successful {
                assert!(!serde_json::to_string(&events)
                    .unwrap()
                    .contains("H144_WORKFLOW_PARTIAL"));
            }
            let nodes: Vec<NodeExecutionEvent> = events
                .iter()
                .filter(|e| e.event_type == "workflow_node")
                .map(|e| serde_json::from_value(e.data["event"].clone()).unwrap())
                .collect();
            let receipts = CostStore::new(&db)
                .run_call_receipts(&sid, "h144-workflow")
                .unwrap();
            let rows = CostStore::new(&db).session_costs_detailed(&sid).unwrap();
            let summary = cost.summary(&sid).unwrap();
            eprintln!(
                "H144 workflow {reason}: terminal={:?} nodes={:?} receipt={} rows={} input={}",
                terminal.kind,
                nodes
                    .iter()
                    .filter(|n| n.status.is_terminal())
                    .map(|n| (&n.node_id, n.status))
                    .collect::<Vec<_>>(),
                receipts.len(),
                rows.len(),
                summary.input_tokens
            );
            assert_eq!(
                terminal.kind,
                if successful {
                    TerminalKind::Succeeded
                } else {
                    TerminalKind::Failed
                }
            );
            assert_eq!(
                transport.requests.lock().unwrap().len(),
                1,
                "workflow must not replay provider interruptions"
            );
            let llm: Vec<_> = nodes
                .iter()
                .filter(|n| n.node_id == "llm" && n.status.is_terminal())
                .collect();
            assert_eq!(llm.len(), 1);
            assert_eq!(
                llm[0].status,
                if successful {
                    NodeExecutionStatus::Completed
                } else {
                    NodeExecutionStatus::Failed
                }
            );
            if successful {
                assert_eq!(
                    llm[0].outputs.as_ref().unwrap()["text"],
                    "H144_WORKFLOW_PARTIAL"
                );
            } else {
                assert!(llm[0].outputs.is_none());
                assert!(llm[0].error.as_deref().unwrap().contains(reason));
            }
            assert!(nodes.iter().any(|n| n.node_id == "end"
                && n.status
                    == if successful {
                        NodeExecutionStatus::Completed
                    } else {
                        NodeExecutionStatus::Skipped
                    }));
            assert_eq!(receipts.len(), 1);
            assert_eq!(receipts[0].report["usage_complete"], has_usage);
            assert_eq!(rows.len(), usize::from(has_usage));
            assert_eq!(summary.input_tokens, if has_usage { 9 } else { 0 });
            assert!((summary.session_cost - if has_usage { 0.00544 } else { 0.0 }).abs() < 1e-12);
            assert_eq!(
                events
                    .iter()
                    .filter(|e| e.event_type == "run_terminal")
                    .count(),
                1
            );
            let record = RunStore::new(&db).get("h144-workflow").unwrap().unwrap();
            let history = EventStore::new(&db).load_session(session.id()).unwrap();
            drop(agent);
            drop(client);
            drop(booked);
            drop(owner);
            drop(sink);
            drop(kernel);
            drop(cost);
            drop(session);
            drop(db);
            let reopened = Arc::new(Database::open(&path).unwrap());
            assert_eq!(
                RunStore::new(&reopened)
                    .get("h144-workflow")
                    .unwrap()
                    .unwrap(),
                record
            );
            assert_eq!(
                RunStore::new(&reopened)
                    .events_after("h144-workflow", None)
                    .unwrap(),
                events
            );
            assert_eq!(
                EventStore::new(&reopened)
                    .load_session(sid.parse().unwrap())
                    .unwrap(),
                history
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
                        .run_call_receipts(&sid, "h144-workflow")
                        .unwrap()
                )
                .unwrap(),
                serde_json::to_value(receipts).unwrap()
            );
            assert_eq!(CostService::new(reopened).summary(&sid).unwrap(), summary);
            assert_eq!(transport.requests.lock().unwrap().len(), 1);
        }
    }
}
