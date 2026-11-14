//! H-297: the real child engine reaches its existing native compactor.
use super::*;
use deepagent_models::transport::{EventSink, HttpTransport, TransportRequest};
use deepagent_models::{ModelCapabilityResolver, ModelConfig};
use deepagent_persistence::subagent_store::SubagentRunStore;
use serde_json::{json, Value};
use std::sync::Mutex;

#[derive(Default)]
struct Recorder(Mutex<Vec<Value>>);

#[async_trait::async_trait]
impl HttpTransport for Recorder {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        let body: Value = serde_json::from_str(&request.body)?;
        let summary = body["instructions"]
            .as_str()
            .is_some_and(|text| text.starts_with("You compress an agent"));
        let mut requests = self.0.lock().unwrap();
        if summary {
            assert!(
                requests.is_empty(),
                "native summary must precede the child's answer request"
            );
            assert_eq!(body["max_output_tokens"], 1024);
        } else {
            assert_eq!(requests.len(), 1);
            assert!(body["input"]
                .to_string()
                .contains("Earlier conversation compacted to summary"));
            assert!(!body["input"]
                .to_string()
                .contains(&"请保留工具调用结果并核对每一步证据。".repeat(10000)));
        }
        assert_eq!(body["model"], "deepseek-flash");
        requests.push(body);
        drop(requests);
        let answer = if summary {
            json!({"goal":"continue synthetic work","completed":["preserved evidence"],"pending":[],"decisions":[],"failures":[]}).to_string()
        } else {
            "child finished".into()
        };
        let events = vec![json!({"type":"response.output_text.delta","delta":answer}).to_string(),json!({"type":"response.completed","response":{"id":"h297-child","object":"response","created_at":1,"model":"deepseek-flash","status":"completed","output":[]}}).to_string()];
        for event in crate::test_sequence::numbered(events) {
            if sink.on_event(&event)? {
                break;
            }
        }
        Ok(())
    }
}

#[tokio::test]
async fn h297_child_override_uses_offline_unicode_pressure_and_native_compaction() {
    let root = tempfile::tempdir().unwrap();
    let model = Arc::new(Recorder::default());
    let mut runner = super::tests::audit_child_runner(root.path(), model.clone());
    let mut config = ModelConfig::deepseek("synthetic");
    let mut cap = ModelCapabilityResolver::new().resolve_model_id("deepseek-flash");
    cap.context_window = 128000;
    cap.max_output_tokens = 8192;
    config.capabilities = vec![cap];
    runner.client = Arc::new(ModelClient::new(model.clone(), config));
    runner.model = "deepseek-v4-pro".into();
    let mut history = vec![ResponseInputItem::Message {
        role: "user".into(),
        content: "请保留工具调用结果并核对每一步证据。".repeat(10000),
    }];
    for index in 0..6 {
        history.push(ResponseInputItem::Message {
            role: if index % 2 == 0 { "assistant" } else { "user" }.into(),
            // The real keep window retains at least 10k estimated tail tokens;
            // give it a substantial recent tail so the older long item is a
            // valid compaction zone instead of protecting the entire history.
            content: format!("retained recent text {index} ").repeat(600),
        });
    }
    let request:deepagent_builtins::SubagentRequest=serde_json::from_value(json!({"description":"h297 child","prompt":"Continue the synthetic work.","model":"deepseek-flash","effort":"none","isolation":"shared"})).unwrap();
    let id = "h297-child".to_owned();
    let persist = runner.begin_tracking(&id, &request, false, None).unwrap();
    runner
        .run_tracked(
            id.clone(),
            SubagentInput { request, history },
            None,
            None,
            persist,
            false,
        )
        .await
        .unwrap();
    assert_eq!(model.0.lock().unwrap().len(), 2);
    let record = SubagentRunStore::new(&runner.db).get(&id).unwrap().unwrap();
    assert_eq!(record.state, "succeeded");
    let transcript: Value =
        serde_json::from_slice(&std::fs::read(record.transcript_path.unwrap()).unwrap()).unwrap();
    let items = transcript["context"]["items"].to_string();
    assert!(items.contains("Earlier conversation compacted to summary"));
    assert!(items.contains("retained recent text 5"));
}
