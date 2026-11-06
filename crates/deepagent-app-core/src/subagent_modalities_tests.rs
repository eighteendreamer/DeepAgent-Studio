//! H-71: tracked children use the parent's frozen model capability boundary.
use super::*;
use deepagent_models::transport::{EventSink, HttpTransport, TransportRequest};
use deepagent_models::{ModelCatalog, ModelConfig, ModelRole};
use deepagent_persistence::{run_store::RunStore, subagent_store::SubagentRunStore};
use serde_json::{json, Value};
use std::sync::Mutex;

#[derive(Default)]
struct Model(Mutex<Vec<Value>>);
#[async_trait::async_trait]
impl HttpTransport for Model {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        self.0
            .lock()
            .unwrap()
            .push(serde_json::from_str(&request.body)?);
        let item = json!({"type":"message","id":"h71-child","role":"assistant",
            "content":[{"type":"output_text","text":"synthetic child answer"}]});
        sink.on_event(&json!({"type":"response.output_item.done","item":item}).to_string())?;
        sink.on_event(&json!({"type":"response.completed","response":{"id":"h71-child",
            "object":"response","created_at":1,"model":"h71-text","status":"completed","output":[item]}}).to_string())?;
        Ok(())
    }
}

#[tokio::test]
async fn h71_child_override_checks_retained_images_and_preserves_normal_text() {
    for (selected, with_image) in [
        ("h71-text", true),
        ("h71-unknown", true),
        ("h71-text", false),
    ] {
        let root = tempfile::tempdir().unwrap();
        let model = Arc::new(Model::default());
        let mut runner = super::tests::audit_child_runner(root.path(), model.clone());
        let catalog:ModelCatalog=serde_json::from_value(json!({
            "base_url":"https://api.deepseek.com", "chat_model":"h71-vision","reasoner_model":"h71-text",
            "available":[{"id":"h71-vision","input_modalities":["text","image"]},
                {"id":"h71-text","input_modalities":["text"]}]})).unwrap();
        runner.client = Arc::new(ModelClient::new(
            model.clone(),
            ModelConfig::from_catalog("synthetic-placeholder", &catalog, ModelRole::Chat),
        ));
        runner.model = "h71-vision".into();
        let request: deepagent_builtins::SubagentRequest = serde_json::from_value(json!({
            "description":"modality control","prompt":"Continue the synthetic context.",
            "model":selected,"isolation":"shared"}))
        .unwrap();
        let history = if with_image {
            vec![serde_json::from_value(
                json!({"type":"input_message","role":"user","content":"retained image",
                "attachments":[{"id":"h71-image","kind":"image","media_type":"image/png"}]}),
            )
            .unwrap()]
        } else {
            vec![ResponseInputItem::Message {
                role: "user".into(),
                content: "retained text".into(),
            }]
        };
        let saved_history = history.clone();
        let id = "h71-child".to_owned();
        let persist = runner.begin_tracking(&id, &request, false, None).unwrap();
        assert!(persist);
        let outcome = runner
            .run_tracked(
                id.clone(),
                SubagentInput { request, history },
                None,
                None,
                persist,
                false,
            )
            .await;
        let store = SubagentRunStore::new(&runner.db);
        let record = store.get(&id).unwrap().unwrap();
        if with_image {
            assert!(outcome.unwrap_err().to_string().contains("image_input_"));
            assert!(model.0.lock().unwrap().is_empty());
            assert_eq!(record.state, "failed");
            let transcript: Value =
                serde_json::from_slice(&std::fs::read(record.transcript_path.unwrap()).unwrap())
                    .unwrap();
            // Durable replay assigns stable snip tags to user content. Apply
            // that existing projection to the expected item; references must
            // still match exactly, not merely appear somewhere as text.
            let mut expected_history = saved_history.clone();
            deepagent_runtime::tag_response_user_items(&mut expected_history);
            assert!(transcript["context"]["items"]
                .as_array()
                .unwrap()
                .iter()
                .any(|item| item == &serde_json::to_value(&expected_history[0]).unwrap()));
        } else {
            outcome.unwrap();
            assert_eq!(record.state, "succeeded");
            assert_eq!(model.0.lock().unwrap().len(), 1);
        }
        let events = RunStore::new(&runner.db)
            .events_after("audit-parent", None)
            .unwrap();
        let terminal: Vec<_> = events
            .iter()
            .filter(|event| event.event_type == "subagent_completed")
            .collect();
        assert_eq!(terminal.len(), 1);
    }
}
