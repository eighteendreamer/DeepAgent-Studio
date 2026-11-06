//! H-236 private agent boundaries after a completed model item and during a tool wait.

use super::*;
use deepagent_models::transport::EventSink;
use deepagent_models::{HttpTransport, ModelConfig, TransportRequest};
use deepagent_tools::{Tool, ToolDescriptor, ToolOutput};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, AtomicUsize};
use tokio::sync::Notify;

use crate::responses_fixture;

struct DropFlag(Arc<AtomicBool>);

impl Drop for DropFlag {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

struct WaitingRead {
    entered: Notify,
    invoked: AtomicUsize,
    dropped: Arc<AtomicBool>,
    complete: bool,
}

#[async_trait]
impl Tool for WaitingRead {
    fn read_only_projection(self: Arc<Self>) -> Option<Arc<dyn Tool>> {
        // Only synthetic wait instrumentation is shared, no parent state.
        Some(self)
    }

    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "h236_read_gate".into(),
            description: "A test-only read-only wait".into(),
            parameters: json!({"type":"object"}),
            risk: RiskLevel::Safe,
            required_permissions: PermissionSet::empty(),
        }
    }

    async fn invoke(&self, _: Value) -> Result<ToolOutput> {
        self.invoked.fetch_add(1, Ordering::AcqRel);
        let _guard = DropFlag(self.dropped.clone());
        self.entered.notify_one();
        if self.complete {
            Ok(ToolOutput::success(json!({"read":"completed"})))
        } else {
            std::future::pending().await
        }
    }
}

struct CompletedToolCall {
    requests: AtomicUsize,
    delivered: AtomicBool,
    cancel_after_parse: Option<ToolExecutionContext>,
    complete: bool,
}

#[async_trait]
impl HttpTransport for CompletedToolCall {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        let sink: &mut dyn deepagent_models::transport::EventSink =
            &mut crate::test_sequence::sink(sink);
        let index = self.requests.fetch_add(1, Ordering::AcqRel);
        assert!(index <= usize::from(self.complete));
        let item = if index == 0 {
            responses_fixture::call("h236-private-read", "h236_read_gate", json!({}))
        } else {
            let body: Value = serde_json::from_str(&request.body)?;
            let results: Vec<_> = body["input"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|item| {
                    item["type"] == "function_call_output" && item["call_id"] == "h236-private-read"
                })
                .collect();
            assert_eq!(results.len(), 1);
            let output: Value = serde_json::from_str(results[0]["output"].as_str().unwrap())?;
            assert_eq!(output["status"], "ok");
            responses_fixture::text("h236-private-decision", r#"{"ok":true}"#)
        };
        let events = responses_fixture::completed(
            &format!("h236-private-response-{index}"),
            "deepseek-flash",
            item,
            json!({"input_tokens":12,"output_tokens":6,"total_tokens":18}),
        );
        for event in events {
            if sink.on_event(&event)? {
                self.delivered.store(true, Ordering::Release);
                break;
            }
        }
        if let Some(parent) = &self.cancel_after_parse {
            // The real accumulator has accepted the complete tool call. Stop
            // before RuntimeEngine is allowed to dispatch the resulting tool.
            parent.cancellation_token().cancel();
        }
        Ok(())
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum StopPoint {
    ParsedCall,
    ToolWait,
    Complete,
}

async fn exercise(point: StopPoint) {
    let parent = ToolExecutionContext::default();
    let tool = Arc::new(WaitingRead {
        entered: Notify::new(),
        invoked: AtomicUsize::new(0),
        dropped: Arc::new(AtomicBool::new(false)),
        complete: point == StopPoint::Complete,
    });
    let mut tools = ToolRegistry::new();
    tools.register(tool.clone()).unwrap();
    let transport = Arc::new(CompletedToolCall {
        requests: AtomicUsize::new(0),
        delivered: AtomicBool::new(false),
        cancel_after_parse: (point == StopPoint::ParsedCall).then(|| parent.clone()),
        complete: point == StopPoint::Complete,
    });
    let sink: Arc<dyn RuntimeEventSink> = Arc::new(NoopEventSink);
    let host = Arc::new(AppHookActionExecutor {
        client: Arc::new(ModelClient::new(
            transport.clone(),
            ModelConfig::deepseek("offline-test-placeholder"),
        )),
        model: "deepseek-flash".into(),
        reasoning_effort: ReasoningEffort::None,
        mcp: None,
        agent_registry: Arc::new(tools),
        agent_policy: Arc::new(HookRegistry::new()),
        agent_permissions: PermissionSet::empty(),
        events: Arc::downgrade(&sink),
        usage_owner: None,
    });
    let definitions = HookDefinitions::parse(
        r#"{"hooks":{"SessionStart":[{"hooks":[{"type":"agent","prompt":"Review","timeout":5}]}]}}"#,
    )
    .unwrap();
    let mut hooks = HookRegistry::new().with_execution_context(parent.clone());
    definitions.register_into_with_host(&mut hooks, Arc::new(SystemHookRunner), host);
    let ctx = HookContext::new(
        deepagent_core::id::SessionId::nil(),
        deepagent_hooks::HookPoint::SessionStart,
        HookData::None,
    );
    let stop = async {
        if point == StopPoint::ToolWait {
            tool.entered.notified().await;
            parent.cancellation_token().cancel();
        }
    };
    let (result, ()) = tokio::time::timeout(Duration::from_secs(3), async {
        tokio::join!(hooks.dispatch(&ctx), stop)
    })
    .await
    .expect("private runtime must stop with its owning hook");
    if point == StopPoint::Complete {
        assert_eq!(result.unwrap(), HookOutcome::Continue);
    } else {
        assert!(result.unwrap_err().to_string().contains("cancelled"));
    }
    assert!(transport.delivered.load(Ordering::Acquire));
    assert_eq!(
        transport.requests.load(Ordering::Acquire),
        if point == StopPoint::Complete { 2 } else { 1 }
    );
    assert_eq!(
        tool.invoked.load(Ordering::Acquire),
        usize::from(point != StopPoint::ParsedCall)
    );
    assert_eq!(
        tool.dropped.load(Ordering::Acquire),
        point != StopPoint::ParsedCall
    );
}

#[tokio::test]
async fn h236_cancel_after_parsing_a_tool_call_prevents_its_dispatch() {
    exercise(StopPoint::ParsedCall).await;
}

#[tokio::test]
async fn h236_cancel_during_a_child_tool_wait_drops_the_wait_without_a_model_followup() {
    exercise(StopPoint::ToolWait).await;
}

#[tokio::test]
async fn h236_uncancelled_child_tool_completes_and_reaches_its_decision() {
    exercise(StopPoint::Complete).await;
}
