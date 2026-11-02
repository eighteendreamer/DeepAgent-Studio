//! Protocol/lifecycle tests. The synthetic bridge and receipt are not proof of
//! interpreter execution or OS isolation; that requires real SDK acceptance.
use super::*;
use crate::agent::{Agent, AgentDecision, Observation};
use crate::events::{RuntimeEvent, RuntimeEventSink};
use crate::workflow::{
    compile, NodeEventPublisher, NodeExecutionEvent, NodeExecutionStatus, WorkflowAgent,
};
use deepagent_tools::ToolInvocation;
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct Bridge {
    calls: Mutex<Vec<CodeRequest>>,
    released: Mutex<Vec<String>>,
}

impl CodeBridge for Bridge {
    fn prepare(&self, request: CodeRequest) -> Result<CodeInvocation> {
        self.calls.lock().unwrap().push(request);
        Ok(CodeInvocation {
            request_id: "synthetic-handle".into(),
            language: CodeLanguage::Javascript,
            script_sha256: "a".repeat(64),
        })
    }
    fn release(&self, id: &str) {
        self.released.lock().unwrap().push(id.into());
    }
}

#[derive(Default)]
struct Sink(Mutex<Vec<NodeExecutionEvent>>);
impl RuntimeEventSink for Sink {
    fn emit(&self, event: RuntimeEvent) {
        if let RuntimeEvent::WorkflowNode { event } = event {
            self.0.lock().unwrap().push(event);
        }
    }
}

fn fixture() -> (WorkflowAgent, Arc<Bridge>, Arc<Sink>) {
    let definition = serde_json::from_value(json!({"version":1,"nodes":[
        {"id":"start","kind":"start"},
        {"id":"code","kind":"code","config":{
            "codeLanguage":"javascript","codeScript":"function main(x) { return {result: x.input}; }",
            "codeInputVariables":[{"name":"input","value":6}],
            "codeOutputVariables":[{"name":"result","type":"number"}]
        }},
        {"id":"end","kind":"end"}
    ],"edges":[{"id":"a","source":"start","target":"code"},{"id":"b","source":"code","target":"end"}]})).unwrap();
    let bridge = Arc::new(Bridge::default());
    let sink = Arc::new(Sink::default());
    let agent = WorkflowAgent::new(
        compile(definition).unwrap(),
        Map::new(),
        None,
        NodeEventPublisher::new(sink.clone()),
    )
    .with_code_bridge(bridge.clone());
    (agent, bridge, sink)
}

async fn defer(agent: &mut WorkflowAgent) -> ToolInvocation {
    assert_eq!(agent.think(0, &[]).await.unwrap(), AgentDecision::Continue);
    let AgentDecision::CallTool(call) = agent.think(1, &[]).await.unwrap() else {
        panic!("must defer into Kernel")
    };
    assert_eq!(call.name, CODE_EXECUTE_TOOL);
    assert_eq!(call.arguments.as_object().unwrap().len(), 3);
    assert!(!call.arguments.to_string().contains("function main"));
    call
}

fn receipt() -> Value {
    json!({
        "version":CODE_VERSION,"outputs":{"result":42},"receipt":{
            "language":"javascript","script_sha256":"a".repeat(64),
            "runtime_version":"24.21.0","isolation":"windows_appcontainer_v1","exit_code":0,
            "stdout":"","stderr":"","stdout_truncated":false,"stderr_truncated":false
        }
    })
}

fn terminal(sink: &Sink) -> Vec<NodeExecutionEvent> {
    sink.0
        .lock()
        .unwrap()
        .iter()
        .filter(|event| event.node_id == "code" && event.status.is_terminal())
        .cloned()
        .collect()
}

#[tokio::test]
async fn code_requires_correlated_raw_typed_result_and_emits_receipt_separately() {
    let (mut agent, bridge, sink) = fixture();
    let call = defer(&mut agent).await;
    let mut observation =
        Observation::new(CODE_EXECUTE_TOOL, true, json!({"truncated":true})).with_call_id(call.id);
    observation.raw_output = Some(receipt());
    assert_eq!(
        agent.think(2, &[observation]).await.unwrap(),
        AgentDecision::Continue
    );
    let events = terminal(&sink);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].outputs, Some(json!({"result":42})));
    assert_eq!(
        events[0].updates["codeExecution"]["runtime_version"],
        "24.21.0"
    );
    assert_eq!(bridge.calls.lock().unwrap().len(), 1);
    assert_eq!(*bridge.released.lock().unwrap(), vec!["synthetic-handle"]);
}

#[tokio::test]
async fn wrong_tool_wrong_call_missing_receipt_or_wrong_output_never_completes() {
    for case in 0..5 {
        let (mut agent, bridge, sink) = fixture();
        let call = defer(&mut agent).await;
        let mut observation =
            Observation::new(CODE_EXECUTE_TOOL, true, receipt()).with_call_id(call.id);
        match case {
            0 => observation.tool = "some_other_tool".into(),
            1 => observation.call_id = Some("unrelated".into()),
            2 => observation.output = json!({"outputs":{"result":42}}),
            3 => observation.output["receipt"]["script_sha256"] = json!("b".repeat(64)),
            _ => observation.output["outputs"]["result"] = json!("42"),
        }
        assert!(agent.think(2, &[observation]).await.is_err());
        let events = terminal(&sink);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].status, NodeExecutionStatus::Failed);
        assert!(events[0].outputs.is_none());
        assert_eq!(bridge.released.lock().unwrap().len(), 1);
    }
}

#[tokio::test]
async fn run_stopping_before_result_delivery_releases_once_and_fails_once() {
    let (mut agent, bridge, sink) = fixture();
    defer(&mut agent).await;
    agent.finish_background_tasks().await;
    agent.finish_background_tasks().await;
    assert_eq!(bridge.released.lock().unwrap().len(), 1);
    let events = terminal(&sink);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].status, NodeExecutionStatus::Failed);
}

#[tokio::test]
async fn cancellation_before_delivery_does_not_publish_successful_outputs() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let (agent, bridge, sink) = fixture();
    let cancel = Arc::new(AtomicBool::new(false));
    let mut agent = agent.with_cancel(cancel.clone());
    let call = defer(&mut agent).await;
    cancel.store(true, Ordering::SeqCst);
    let observation = Observation::new(CODE_EXECUTE_TOOL, true, receipt()).with_call_id(call.id);
    assert!(agent.think(2, &[observation]).await.is_err());
    assert_eq!(bridge.released.lock().unwrap().len(), 1);
    let events = terminal(&sink);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].status, NodeExecutionStatus::Cancelled);
    assert!(events[0].outputs.is_none());
}
