//! Check request-time pressure against version-pinned offline text counts.
//! The provider and compactor are deterministic seams; no real model is called.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use deepagent_context::ContextPolicy;
use deepagent_core::error::Result;
use deepagent_core::message::Message;
use deepagent_models::transport::{EventSink, HttpTransport, TransportRequest};
use deepagent_models::{
    CapabilitySource, ModelCapability, ModelClient, ModelConfig, ReasoningEffort,
};
use deepagent_runtime::agent::Agent;
use deepagent_runtime::model_agent::{
    CompactionTrigger, ModelAgent, ReactiveCompaction, ReactiveContextCompactor,
};
use serde_json::{json, Value};

#[derive(Default)]
struct CountingCompactor(AtomicUsize);

#[async_trait]
impl ReactiveContextCompactor for CountingCompactor {
    async fn compact(
        &self,
        _messages: &[Message],
        trigger: CompactionTrigger,
    ) -> Result<Option<ReactiveCompaction>> {
        assert_eq!(trigger, CompactionTrigger::AutoCompactThreshold);
        self.0.fetch_add(1, Ordering::AcqRel);
        // Measure whether the gate was reached, without simulating an adopted
        // summary or its effectiveness. Returning None retains the real input.
        Ok(None)
    }
}

struct RecordingModel {
    compactor: Arc<CountingCompactor>,
    calls: AtomicUsize,
    compactions_before_dispatch: AtomicUsize,
    expected_input: String,
}

#[async_trait]
impl HttpTransport for RecordingModel {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        assert_eq!(self.calls.fetch_add(1, Ordering::AcqRel), 0);
        let body: Value = serde_json::from_str(&request.body).unwrap();
        assert_eq!(body["model"], "audit-offline-tokenizer");
        assert_eq!(body["max_output_tokens"], 8192);
        let user = body["input"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["role"] == "user")
            .unwrap();
        assert_eq!(user["content"], self.expected_input);
        self.compactions_before_dispatch
            .store(self.compactor.0.load(Ordering::Acquire), Ordering::Release);
        let item = json!({"type":"message","id":"audit-message","role":"assistant",
            "status":"completed","content":[{"type":"output_text","text":"received","annotations":[]}]});
        let events = [
            json!({"type":"response.created","response":{"id":"audit-response","object":"response","created_at":1,"model":"audit-offline-tokenizer","status":"in_progress","output":[]}}),
            json!({"type":"response.output_item.added","output_index":0,"item":{"type":"message","id":"audit-message","role":"assistant","status":"in_progress","content":[]}}),
            json!({"type":"response.output_item.done","output_index":0,"item":item}),
            json!({"type":"response.completed","response":{"id":"audit-response","object":"response","created_at":1,"model":"audit-offline-tokenizer","status":"completed","output":[item],"usage":{"input_tokens":1,"output_tokens":1,"total_tokens":2},"error":null,"incomplete_details":null}}),
        ];
        for (sequence, mut event) in events.into_iter().enumerate() {
            event["sequence_number"] = sequence.into();
            if sink.on_event(&event.to_string())? {
                break;
            }
        }
        Ok(())
    }
}

async fn observe(sample: &Value) -> (usize, usize) {
    let capability = ModelCapability {
        input_modalities: None,
        model_id: "audit-offline-tokenizer".into(),
        context_window: 128_000,
        max_output_tokens: 8192,
        supports_tools: true,
        supports_thinking: true,
        supports_json_output: true,
        capability_source: CapabilitySource::ProviderMetadata,
        fallback_reason: None,
    };
    let policy = ContextPolicy::for_capability(&capability, ReasoningEffort::None);
    let threshold = policy.autocompact_threshold_tokens(None, None);
    let mut config = ModelConfig::deepseek("offline-audit-placeholder");
    config.capabilities = vec![capability];
    config.defaults.max_output_tokens = Some(8192);
    let content = sample["text"]
        .as_str()
        .unwrap()
        .repeat(sample["repeat"].as_u64().unwrap() as usize);
    let compactor = Arc::new(CountingCompactor::default());
    let transport = Arc::new(RecordingModel {
        compactor: compactor.clone(),
        calls: AtomicUsize::new(0),
        compactions_before_dispatch: AtomicUsize::new(0),
        expected_input: content.clone(),
    });
    let client = Arc::new(ModelClient::new(transport.clone(), config));
    let mut agent = ModelAgent::new(
        client,
        "audit-offline-tokenizer",
        "system",
        &content,
        vec![],
    )
    .with_reasoning_effort(ReasoningEffort::None)
    .with_context_policy(policy, None, None)
    .with_reactive_compactor(compactor);
    tokio::time::timeout(std::time::Duration::from_secs(5), agent.think(0, &[]))
        .await
        .expect("bounded offline model request")
        .unwrap();
    assert_eq!(transport.calls.load(Ordering::Acquire), 1);
    let calls = transport
        .compactions_before_dispatch
        .load(Ordering::Acquire);
    eprintln!("ModelAgent offline pressure: sample={} offline_text={} threshold={threshold} compactor_calls_before_dispatch={calls} requests=1", sample["name"], sample["offline_tokens"]);
    (calls, threshold)
}

fn samples() -> Vec<Value> {
    let spec: Value = serde_json::from_str(include_str!(
        "../../deepagent-context/tests/fixtures/deepseek_tokenizer_samples.json"
    ))
    .unwrap();
    spec["samples"].as_array().unwrap().clone()
}

#[tokio::test]
async fn english_above_threshold_and_code_below_threshold_reach_the_expected_gate() {
    for sample in samples().into_iter().filter(|sample| {
        matches!(
            sample["name"].as_str(),
            Some("english_prose" | "rust_with_chinese_comments")
        )
    }) {
        let (calls, threshold) = observe(&sample).await;
        assert_eq!(
            calls,
            usize::from(sample["offline_tokens"].as_u64().unwrap() >= threshold as u64)
        );
    }
}

#[tokio::test]
#[ignore = "audit: ModelAgent must reach pressure handling before dispatching offline over-threshold text"]
async fn audit_offline_over_threshold_unicode_reaches_pressure_before_dispatch() {
    let mut missed = Vec::new();
    for sample in samples().into_iter().filter(|sample| {
        matches!(
            sample["name"].as_str(),
            Some("chinese_prose" | "unicode_symbols")
        )
    }) {
        let (calls, threshold) = observe(&sample).await;
        assert!(sample["offline_tokens"].as_u64().unwrap() >= threshold as u64);
        if calls == 0 {
            missed.push(sample["name"].clone());
        }
    }
    assert!(
        missed.is_empty(),
        "ModelAgent missed offline pressure: {missed:?}"
    );
}
