//! Actual hook/model parser boundaries, using the same offline Responses
//! fixture as the production-policy and cancellation regressions.
use super::*;
use crate::hook_runtime::AppHookActionExecutor;
use crate::responses_fixture;
use async_trait::async_trait;
use deepagent_hooks::{HookAction, HookActionExecutor, HookActionType, HookRegistry};
use deepagent_hooks::{HookContext, HookData, HookDefinitions, HookPoint, SystemHookRunner};
use deepagent_models::transport::{EventSink, HttpTransport, TransportRequest};
use deepagent_models::{ModelClient, ModelConfig, ReasoningEffort};
use deepagent_tools::{PermissionSet, ToolExecutionContext, ToolRegistry};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::sync::Notify;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Answer {
    Valid,
    InvalidJson,
    FailedMeasured,
    FailedUnmeasured,
    RetryMeasured,
    WaitMeasured,
}

struct Transport {
    answer: Answer,
    calls: AtomicUsize,
    observed: Notify,
}

#[async_trait]
impl HttpTransport for Transport {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        let sink: &mut dyn deepagent_models::transport::EventSink =
            &mut crate::test_sequence::sink(sink);
        let index = self.calls.fetch_add(1, Ordering::AcqRel);
        tokio::task::yield_now().await;
        let fail = || CoreError::Provider {
            status: Some(503),
            code: None,
            message: "controlled offline failure".into(),
            retry_after_ms: Some(1),
            request_id: None,
        };
        if self.answer == Answer::FailedUnmeasured {
            return Err(fail());
        }
        let body: serde_json::Value = serde_json::from_str(&request.body)?;
        let model = body["model"].as_str().unwrap();
        assert!(matches!(model, "flash" | "pro"));
        let text = if self.answer == Answer::InvalidJson {
            "not JSON"
        } else {
            r#"{"ok":true}"#
        };
        for event in responses_fixture::completed(
            &format!("hook-response-{index}"),
            model,
            responses_fixture::text(&format!("decision-{index}"), text),
            json!({"input_tokens":1000,"output_tokens":100,"total_tokens":1100,
                "input_tokens_details":{"cached_tokens":800},
                "output_tokens_details":{"reasoning_tokens":0}}),
        ) {
            if sink.on_event(&event)? {
                break;
            }
        }
        self.observed.notify_one();
        if self.answer == Answer::WaitMeasured {
            return std::future::pending().await;
        }
        if self.answer == Answer::FailedMeasured
            || (self.answer == Answer::RetryMeasured && index == 0)
        {
            return Err(fail());
        }
        Ok(())
    }
}

fn executor(owner: RunCallUsageOwner, answer: Answer) -> (AppHookActionExecutor, Arc<Transport>) {
    let transport = Arc::new(Transport {
        answer,
        calls: AtomicUsize::new(0),
        observed: Notify::new(),
    });
    let host = AppHookActionExecutor {
        client: Arc::new(ModelClient::new(
            transport.clone(),
            ModelConfig::deepseek("offline-placeholder"),
        )),
        model: "flash".into(),
        reasoning_effort: ReasoningEffort::None,
        mcp: None,
        agent_registry: Arc::new(ToolRegistry::new()),
        agent_policy: Arc::new(HookRegistry::new()),
        agent_permissions: PermissionSet::empty(),
        events: owner.events.clone(),
        usage_owner: Some(owner),
    };
    (host, transport)
}

fn action(kind: HookActionType) -> HookAction {
    HookAction {
        action_type: kind,
        prompt: "Controlled offline review".into(),
        timeout: Some(5),
        ..Default::default()
    }
}

fn registered(
    host: AppHookActionExecutor,
    action: &HookAction,
    execution: ToolExecutionContext,
) -> HookRegistry {
    let definitions =
        HookDefinitions::parse(&json!({"hooks":{"SessionStart":[{"hooks":[action]}]}}).to_string())
            .unwrap();
    let mut hooks = HookRegistry::new().with_execution_context(execution);
    assert_eq!(
        definitions.register_into_with_host(&mut hooks, Arc::new(SystemHookRunner), Arc::new(host)),
        1
    );
    hooks
}

#[tokio::test]
async fn invalid_json_does_not_erase_prompt_or_agent_usage() {
    for kind in [HookActionType::Prompt, HookActionType::Agent] {
        let (owner, _events) = setup();
        let (host, transport) = executor(owner.clone(), Answer::InvalidJson);
        assert!(host
            .execute(&action(kind), json!({"hook_event_name":"PreToolUse"}))
            .await
            .is_err());
        assert_eq!(transport.calls.load(Ordering::Acquire), 1);
        assert_eq!(
            owner
                .cost
                .as_ref()
                .unwrap()
                .summary(&owner.session_id)
                .unwrap()
                .input_tokens,
            1000
        );
    }
}

#[tokio::test]
async fn failed_prompt_keeps_measured_usage_but_does_not_invent_unmeasured_usage() {
    for (answer, expected) in [
        (Answer::FailedMeasured, 1000),
        (Answer::FailedUnmeasured, 0),
    ] {
        let (owner, _events) = setup();
        let (host, _) = executor(owner.clone(), answer);
        assert!(host
            .execute(&action(HookActionType::Prompt), json!({}))
            .await
            .is_err());
        assert_eq!(
            owner
                .cost
                .as_ref()
                .unwrap()
                .summary(&owner.session_id)
                .unwrap()
                .input_tokens,
            expected
        );
        assert_eq!(
            RunStore::new(&owner.db)
                .events_after("run", Some(0))
                .unwrap()
                .len(),
            usize::from(expected > 0)
        );
    }
}

#[tokio::test]
async fn actual_agent_retry_and_prompt_override_price_each_physical_route_once() {
    let (owner, _events) = setup();
    use crate::cost_service::ModelPricing;
    let price = |factor| ModelPricing {
        input_cache_hit_per_million: factor,
        input_cache_miss_per_million: factor,
        output_per_million: factor,
    };
    owner
        .cost
        .as_ref()
        .unwrap()
        .set_pricing(std::collections::HashMap::from([
            ("flash".into(), price(1.0)),
            ("pro".into(), price(10.0)),
        ]));
    let (agent, requests) = executor(owner.clone(), Answer::RetryMeasured);
    agent
        .execute(&action(HookActionType::Agent), json!({}))
        .await
        .unwrap();
    assert_eq!(requests.calls.load(Ordering::Acquire), 2);
    let (prompt, _) = executor(owner.clone(), Answer::Valid);
    let mut config = action(HookActionType::Prompt);
    config.model = Some(" pro ".into());
    prompt.execute(&config, json!({})).await.unwrap();
    let summary = owner
        .cost
        .as_ref()
        .unwrap()
        .summary(&owner.session_id)
        .unwrap();
    assert_eq!(summary.input_tokens, 3000);
    assert!((summary.session_cost - 0.0132).abs() < 1e-12);
    let records = RunStore::new(&owner.db)
        .events_after("run", Some(0))
        .unwrap();
    assert_eq!(records.len(), 3);
    assert_eq!(records[2].data["report"]["model_id"], "pro");
    assert_eq!(
        records[0].data["report"]["invocation_id"],
        records[1].data["report"]["invocation_id"]
    );
    assert_ne!(records[0].data["call_id"], records[1].data["call_id"]);
}

#[tokio::test]
async fn measured_agent_failure_reaches_the_budget_before_any_retry() {
    let (owner, _events) = setup();
    let cost = owner.cost.as_ref().unwrap();
    cost.set_pricing(std::collections::HashMap::from([(
        "flash".into(),
        crate::cost_service::ModelPricing {
            input_cache_hit_per_million: 1.0,
            input_cache_miss_per_million: 1.0,
            output_per_million: 1.0,
        },
    )]));
    cost.set_budget(crate::cost_service::BudgetConfig {
        daily_limit: Some(0.001),
        monthly_limit: None,
    })
    .unwrap();
    let (agent, requests) = executor(owner.clone(), Answer::RetryMeasured);
    let error = agent
        .execute(&action(HookActionType::Agent), json!({}))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("daily budget exhausted"));
    assert_eq!(requests.calls.load(Ordering::Acquire), 1);
    let summary = cost.summary(&owner.session_id).unwrap();
    assert_eq!(summary.input_tokens, 1000);
    assert!((summary.session_cost - 0.0011).abs() < 1e-12);
    assert!(owner
        .main_loop()
        .before_call()
        .unwrap_err()
        .to_string()
        .contains("daily budget exhausted"));
}

#[tokio::test]
async fn cancellation_after_usage_is_accounted_for_prompt_and_private_agent() {
    for kind in [HookActionType::Prompt, HookActionType::Agent] {
        let (owner, _events) = setup();
        let (host, transport) = executor(owner.clone(), Answer::WaitMeasured);
        let execution = ToolExecutionContext::default();
        let cancel = async {
            transport.observed.notified().await;
            execution.cancellation_token().cancel();
        };
        // The registered resource owner bridges hierarchical cancellation to
        // the private agent's legacy model flag. Do not bypass that owner.
        let hooks = registered(host, &action(kind), execution.clone());
        let context = HookContext::new(
            owner.session_id.parse().unwrap(),
            HookPoint::SessionStart,
            HookData::None,
        );
        let run = hooks.dispatch(&context);
        let (result, ()) =
            tokio::time::timeout(Duration::from_secs(2), async { tokio::join!(run, cancel) })
                .await
                .unwrap();
        assert!(result.is_err());
        assert_eq!(transport.calls.load(Ordering::Acquire), 1);
        assert_eq!(
            owner
                .cost
                .as_ref()
                .unwrap()
                .summary(&owner.session_id)
                .unwrap()
                .input_tokens,
            1000
        );
    }
}

#[tokio::test]
async fn registered_hook_timeout_keeps_prompt_and_agent_usage() {
    for kind in [HookActionType::Prompt, HookActionType::Agent] {
        let (owner, _events) = setup();
        let (host, _) = executor(owner.clone(), Answer::WaitMeasured);
        let mut config = action(kind);
        config.timeout = Some(1);
        let hooks = registered(host, &config, ToolExecutionContext::default());
        let context = HookContext::new(
            owner.session_id.parse().unwrap(),
            HookPoint::SessionStart,
            HookData::None,
        );
        let result = tokio::time::timeout(Duration::from_secs(3), hooks.dispatch(&context))
            .await
            .unwrap();
        assert!(result.is_err());
        assert_eq!(
            owner
                .cost
                .as_ref()
                .unwrap()
                .summary(&owner.session_id)
                .unwrap()
                .input_tokens,
            1000
        );
    }
}

#[tokio::test]
async fn dropping_an_inflight_hook_after_usage_preserves_its_receipt() {
    for kind in [HookActionType::Prompt, HookActionType::Agent] {
        let (owner, _events) = setup();
        let (host, transport) = executor(owner.clone(), Answer::WaitMeasured);
        let config = action(kind);
        let mut run = Box::pin(host.execute(&config, json!({})));
        tokio::select! {
            result = &mut run => panic!("hook must still be pending: {result:?}"),
            _ = transport.observed.notified() => {}
        }
        // Explicitly drop the actual waiting future, not just an observation
        // handle. The observer's RAII settlement is the timeout/drop seam.
        drop(run);
        assert_eq!(
            owner
                .cost
                .as_ref()
                .unwrap()
                .summary(&owner.session_id)
                .unwrap()
                .input_tokens,
            1000
        );
        assert_eq!(
            RunStore::new(&owner.db)
                .events_after("run", Some(0))
                .unwrap()
                .len(),
            1
        );
    }
}

#[tokio::test]
async fn concurrent_model_hooks_keep_distinct_invocations_and_call_receipts() {
    let (owner, _events) = setup();
    let (host, transport) = executor(owner.clone(), Answer::Valid);
    let prompt = action(HookActionType::Prompt);
    let agent = action(HookActionType::Agent);
    let (first, second) = tokio::join!(
        host.execute(&prompt, json!({})),
        host.execute(&agent, json!({}))
    );
    first.unwrap();
    second.unwrap();
    assert_eq!(transport.calls.load(Ordering::Acquire), 2);
    assert_eq!(
        owner
            .cost
            .as_ref()
            .unwrap()
            .summary(&owner.session_id)
            .unwrap()
            .input_tokens,
        2000
    );
    let records = RunStore::new(&owner.db)
        .events_after("run", Some(0))
        .unwrap();
    assert_eq!(records.len(), 2);
    assert_ne!(
        records[0].data["report"]["invocation_id"],
        records[1].data["report"]["invocation_id"]
    );
    assert_ne!(records[0].data["call_id"], records[1].data["call_id"]);
}

#[tokio::test]
async fn accounting_write_failure_is_an_explicit_hook_error_not_a_success() {
    let (owner, events) = setup();
    owner.db.with_conn(|conn| conn.execute_batch("CREATE TEMP TRIGGER fail_cost BEFORE INSERT ON costs BEGIN SELECT RAISE(FAIL,'controlled cost failure'); END;").map_err(|e| CoreError::Persistence(e.to_string()))).unwrap();
    let (host, _) = executor(owner.clone(), Answer::Valid);
    let result = host
        .execute(
            &action(HookActionType::Prompt),
            json!({"hook_event_name":"PreToolUse"}),
        )
        .await;
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("model call accounting failed"));
    let captured = events.0.lock().unwrap();
    assert!(!captured
        .iter()
        .any(|event| matches!(event, RuntimeEvent::CostRecorded { .. })));
    assert!(captured.iter().any(
        |event| matches!(event, RuntimeEvent::HookCompleted { outcome, .. } if outcome == "error")
    ));
    assert_eq!(
        RunStore::new(&owner.db)
            .events_after("run", Some(0))
            .unwrap()[0]
            .event_type,
        "model_call_usage_failed"
    );
}
