//! Deterministic in-process model seam tests. No provider or OS execution.
use super::*;
use crate::workflow::{
    compile, CanvasCompletionResponse, CanvasEmbeddingResponse, CanvasRouteOutcome,
};
use std::sync::atomic::AtomicUsize;
use std::sync::Mutex;
use std::time::Duration;

#[derive(Default)]
struct ProbeBridge {
    live: Arc<AtomicUsize>,
    maximum: Mutex<usize>,
    finished: Mutex<Vec<String>>,
    output_bytes: AtomicUsize,
}

struct ActiveCall(Arc<AtomicUsize>);
impl Drop for ActiveCall {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

#[async_trait]
impl CanvasModelBridge for ProbeBridge {
    async fn complete(&self, request: CanvasCompletionRequest) -> Result<CanvasCompletionResponse> {
        let count = self.live.fetch_add(1, Ordering::SeqCst) + 1;
        let _active = ActiveCall(self.live.clone());
        {
            let mut max = self.maximum.lock().unwrap();
            *max = (*max).max(count);
        }
        if request.prompt == "boom" {
            return Err(CoreError::other("synthetic item failure"));
        }
        let delay = if request.prompt == "slow" { 100 } else { 5 };
        tokio::time::sleep(Duration::from_millis(delay)).await;
        self.finished.lock().unwrap().push(request.prompt.clone());
        Ok(CanvasCompletionResponse {
            text: if self.output_bytes.load(Ordering::SeqCst) > 0 {
                "x".repeat(self.output_bytes.load(Ordering::SeqCst))
            } else {
                request.prompt
            },
            reasoning: None,
            provider_id: "synthetic-provider".into(),
            model_id: "synthetic-model".into(),
            usage: Some(deepagent_models::Usage {
                prompt_tokens: 10,
                completion_tokens: 2,
                total_tokens: 12,
                ..Default::default()
            }),
            raw_usage: None,
        })
    }
    async fn generate_image(&self, _: CanvasImageRequest) -> Result<CanvasImageResponse> {
        unreachable!()
    }
    async fn run_audio(
        &self,
        _: CanvasAudioRequest,
    ) -> Result<super::super::canvas::CanvasAudioResponse> {
        unreachable!()
    }
    async fn generate_video(&self, _: CanvasVideoRequest) -> Result<CanvasVideoResponse> {
        unreachable!()
    }
    async fn embed(&self, _: CanvasEmbeddingRequest) -> Result<CanvasEmbeddingResponse> {
        unreachable!()
    }
    fn route_operation(&self, _: CanvasRouteRequest) -> Result<CanvasRouteOutcome> {
        unreachable!()
    }
    fn inspect_input_kinds(&self, _: &[String]) -> Result<Vec<String>> {
        unreachable!()
    }
}

#[derive(Default)]
struct Capture(Mutex<Vec<NodeExecutionEvent>>);
impl crate::events::RuntimeEventSink for Capture {
    fn emit(&self, event: crate::events::RuntimeEvent) {
        if let crate::events::RuntimeEvent::WorkflowNode { event } = event {
            self.0.lock().unwrap().push(event);
        }
    }
}

fn definition(parallel: bool, concurrency: usize) -> super::super::graph::WorkflowDefinition {
    serde_json::from_value(json!({"version":1,"nodes":[
        {"id":"start","kind":"start","config":{"inputVariables":[{"name":"items","variable":"items","type":"array"}]}},
        {"id":"iter","kind":"iteration","config":{"inputVariable":"{{#start.items#}}","outputVariable":"{{#body.text#}}","parallel":parallel,"maxConcurrency":concurrency}},
        {"id":"entry","kind":"iteration-start"},
        {"id":"body","kind":"llm","config":{"llmPrompt":"{{#entry.item#}}"}},
        {"id":"end","kind":"end"}
    ],"edges":[
        {"id":"a","source":"start","target":"iter"},
        {"id":"b","source":"iter","source_handle":"item","target":"entry"},
        {"id":"c","source":"entry","target":"body"},
        {"id":"d","source":"iter","source_handle":"done","target":"end"}
    ]})).unwrap()
}

fn fixture(
    definition: super::super::graph::WorkflowDefinition,
    items: Value,
) -> (WorkflowAgent, Arc<ProbeBridge>, Arc<Capture>) {
    let bridge = Arc::new(ProbeBridge::default());
    let capture = Arc::new(Capture::default());
    let agent = WorkflowAgent::new(
        compile(definition).unwrap(),
        json!({"items":items}).as_object().unwrap().clone(),
        None,
        NodeEventPublisher::new(capture.clone()),
    )
    .with_canvas_bridge(bridge.clone());
    (agent, bridge, capture)
}

async fn drive(agent: &mut WorkflowAgent) -> Result<()> {
    for step in 0..2000 {
        match agent.think(step, &[]).await? {
            AgentDecision::Complete(_) => return Ok(()),
            AgentDecision::Continue => {}
            other => panic!("no tools in this fixture: {other:?}"),
        }
    }
    panic!("bounded fixture did not terminate")
}

#[tokio::test]
async fn parallel_overlaps_real_item_futures_but_aggregates_in_input_order_and_accounts_once() {
    let (mut agent, bridge, capture) =
        fixture(definition(true, 2), json!(["slow", "fast", "third"]));
    drive(&mut agent).await.unwrap();
    assert_eq!(*bridge.maximum.lock().unwrap(), 2);
    assert_eq!(bridge.finished.lock().unwrap()[0], "fast");
    assert_eq!(bridge.live.load(Ordering::SeqCst), 0);
    assert_eq!(
        agent.outputs["iter"],
        json!({"output":["slow","fast","third"]})
    );
    let usage = agent.cumulative_usage().unwrap();
    assert_eq!(usage.billed_prompt_tokens, 30);
    assert_eq!(usage.completion_tokens, 6);
    assert_eq!(agent.take_pending_raw_usage().len(), 3);
    assert!(agent.take_pending_raw_usage().is_empty());
    assert_eq!(*agent.item_output_budget.lock().unwrap(), 0);
    let events = capture.0.lock().unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|event| event.node_id == "iter" && event.status.is_terminal())
            .count(),
        1
    );
    assert!(events
        .iter()
        .filter(|event| event.node_id == "body")
        .all(|event| event.scope.len() == 2));
}

#[tokio::test]
async fn serial_configuration_never_overlaps_even_with_larger_concurrency_setting() {
    let (mut agent, bridge, _) =
        fixture(definition(false, 50), json!(["first", "second", "third"]));
    drive(&mut agent).await.unwrap();
    assert_eq!(*bridge.maximum.lock().unwrap(), 1);
    assert_eq!(
        *bridge.finished.lock().unwrap(),
        vec!["first", "second", "third"]
    );
}

#[tokio::test]
async fn parallel_continue_and_remove_keep_mixed_results_ordered_and_account_only_real_usage() {
    for (policy, expected) in [
        ("continue", json!(["slow", "fast", null, "third"])),
        ("remove", json!(["slow", "fast", "third"])),
    ] {
        let mut definition = definition(true, 3);
        definition.nodes[1]
            .config
            .insert("errorHandling".into(), json!(policy));
        let (mut agent, bridge, capture) =
            fixture(definition, json!(["slow", "fast", "boom", "third"]));
        drive(&mut agent).await.unwrap();
        assert_eq!(*bridge.maximum.lock().unwrap(), 3);
        assert_eq!(bridge.finished.lock().unwrap()[0], "fast");
        assert_eq!(bridge.live.load(Ordering::SeqCst), 0);
        assert_eq!(agent.outputs["iter"], json!({"output":expected}));
        assert_eq!(agent.cumulative_usage().unwrap().billed_prompt_tokens, 30);
        assert_eq!(agent.take_pending_raw_usage().len(), 3);
        assert!(agent.take_pending_raw_usage().is_empty());
        assert_eq!(*agent.item_output_budget.lock().unwrap(), 0);
        let events = capture.0.lock().unwrap();
        let failures: Vec<_> = events
            .iter()
            .filter(|event| event.node_id == "body" && event.status == NodeExecutionStatus::Failed)
            .collect();
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].scope, vec!["iter", "2"]);
        assert_eq!(
            events
                .iter()
                .filter(|event| event.node_id == "iter" && event.status.is_terminal())
                .count(),
            1
        );
    }
}

#[tokio::test]
async fn terminate_drops_inflight_sibling_and_never_admits_later_item_or_done() {
    let (mut agent, bridge, capture) = fixture(
        definition(true, 2),
        json!(["slow", "boom", "must-not-start"]),
    );
    assert!(drive(&mut agent).await.is_err());
    agent.finish_background_tasks().await;
    assert_eq!(bridge.live.load(Ordering::SeqCst), 0);
    assert!(bridge.finished.lock().unwrap().is_empty());
    let events = capture.0.lock().unwrap();
    assert!(!events
        .iter()
        .any(|event| event.scope == vec!["iter", "2"] || event.node_id == "end"));
    assert_eq!(
        events
            .iter()
            .filter(|event| event.node_id == "body"
                && event.scope == vec!["iter", "0"]
                && event.status == NodeExecutionStatus::Cancelled)
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| event.node_id == "iter" && event.status == NodeExecutionStatus::Failed)
            .count(),
        1
    );
}

#[tokio::test]
async fn run_cancellation_drops_item_futures_and_closes_each_started_scope_once() {
    let (mut agent, bridge, capture) =
        fixture(definition(true, 2), json!(["slow", "slow", "not-started"]));
    let flag = Arc::new(AtomicBool::new(false));
    agent.cancel = Some(flag.clone());
    let work = async {
        for step in 0..100 {
            if agent
                .think_cancelled(step, &[], Some(flag.clone()))
                .await
                .is_err()
            {
                return;
            }
        }
        panic!("cancellation was not observed")
    };
    let cancel = async {
        for _ in 0..100 {
            if bridge.live.load(Ordering::SeqCst) == 2 {
                flag.store(true, Ordering::Release);
                return;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        panic!("actual item futures did not start")
    };
    tokio::join!(work, cancel);
    agent.finish_background_tasks().await;
    agent.finish_background_tasks().await;
    assert_eq!(*agent.item_output_budget.lock().unwrap(), 0);
    assert_eq!(bridge.live.load(Ordering::SeqCst), 0);
    let events = capture.0.lock().unwrap();
    assert_eq!(
        events
            .iter()
            .filter(
                |event| event.node_id == "body" && event.status == NodeExecutionStatus::Cancelled
            )
            .count(),
        2
    );
    assert_eq!(
        events
            .iter()
            .filter(
                |event| event.node_id == "iter" && event.status == NodeExecutionStatus::Cancelled
            )
            .count(),
        1
    );
    assert!(!events
        .iter()
        .any(|event| event.scope == vec!["iter", "2"] || event.node_id == "end"));
}

#[tokio::test]
async fn nested_parallel_scopes_divide_fifty_lanes_instead_of_multiplying_them() {
    let mut definition = definition(true, 50);
    definition.nodes[1]
        .config
        .insert("outputVariable".into(), json!("{{#inner.output#}}"));
    definition.nodes[3]
        .config
        .insert("llmPrompt".into(), json!("{{#inner-entry.item#}}"));
    definition.nodes.extend(serde_json::from_value::<Vec<super::super::graph::WorkflowNodeSpec>>(json!([
        {"id":"inner","kind":"iteration","config":{"inputVariable":"{{#entry.item#}}","outputVariable":"{{#body.text#}}","parallel":true,"maxConcurrency":50}},
        {"id":"inner-entry","kind":"iteration-start"}
    ])).unwrap());
    definition.edges.retain(|edge| edge.id != "c");
    definition.edges.extend(
        serde_json::from_value::<Vec<super::super::graph::WorkflowEdgeSpec>>(json!([
            {"id":"c","source":"entry","target":"inner"},
            {"id":"e","source":"inner","source_handle":"item","target":"inner-entry"},
            {"id":"f","source":"inner-entry","target":"body"}
        ]))
        .unwrap(),
    );
    let items = Value::Array((0..50).map(|_| json!(["first", "second"])).collect());
    let (mut agent, bridge, capture) = fixture(definition, items);
    drive(&mut agent).await.unwrap();
    assert_eq!(*bridge.maximum.lock().unwrap(), 50);
    assert_eq!(
        agent.outputs["iter"]["output"],
        Value::Array(vec![json!(["first", "second"]); 50])
    );
    let events = capture.0.lock().unwrap();
    let body: Vec<_> = events
        .iter()
        .filter(|event| event.node_id == "body" && event.status == NodeExecutionStatus::Completed)
        .collect();
    assert_eq!(body.len(), 100);
    assert!(body.iter().all(|event| event.scope.len() == 4
        && event.scope[0] == "iter"
        && event.scope[2] == "inner"));
    assert_eq!(
        body.iter()
            .map(|event| &event.scope)
            .collect::<BTreeSet<_>>()
            .len(),
        100
    );
}

#[test]
fn output_store_lease_is_shared_bounded_and_released_on_failed_reservation_or_drop() {
    let budget = Arc::new(Mutex::new(0));
    let mut first = OutputLease::reserve(budget.clone(), values::MAX_BYTES).unwrap();
    let second = OutputLease::reserve(budget.clone(), 49 * values::MAX_BYTES).unwrap();
    assert!(first.resize(2 * values::MAX_BYTES).is_err());
    assert!(OutputLease::reserve(budget.clone(), 1).is_err());
    assert_eq!(*budget.lock().unwrap(), 50 * values::MAX_BYTES);
    drop(second);
    first.resize(2 * values::MAX_BYTES).unwrap();
    assert_eq!(*budget.lock().unwrap(), 2 * values::MAX_BYTES);
    drop(first);
    assert_eq!(*budget.lock().unwrap(), 0);
}

#[tokio::test]
async fn growing_aggregate_fails_boundedly_before_admitting_later_items_even_with_continue() {
    let mut definition = definition(false, 1);
    definition.nodes[1]
        .config
        .insert("errorHandling".into(), json!("continue"));
    let (mut agent, bridge, capture) =
        fixture(definition, json!(["first", "second", "not-started"]));
    bridge.output_bytes.store(600_000, Ordering::SeqCst);
    let error = drive(&mut agent).await.unwrap_err();
    assert!(
        error.to_string().contains("aggregate exceeds 1 MiB"),
        "{error}"
    );
    agent.finish_background_tasks().await;
    assert_eq!(*agent.item_output_budget.lock().unwrap(), 0);
    assert_eq!(bridge.finished.lock().unwrap().len(), 2);
    assert!(!capture
        .0
        .lock()
        .unwrap()
        .iter()
        .any(|event| event.scope == vec!["iter", "2"] || event.node_id == "end"));
}
