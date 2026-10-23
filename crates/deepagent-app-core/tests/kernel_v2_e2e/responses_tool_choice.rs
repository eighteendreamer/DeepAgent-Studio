use super::*;

use deepagent_app_core::settings::ResponsesApiSettings;
use deepagent_models::{ModelClient, ModelConfig, ResponseRequest, ToolSchema};

#[derive(Default)]
struct CountingTransport {
    requests: Mutex<Vec<serde_json::Value>>,
}

#[async_trait]
impl HttpTransport for CountingTransport {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        self.requests
            .lock()
            .unwrap()
            .push(serde_json::from_str(&request.body)?);
        sink.on_event(r#"{"type":"response.completed","response":{"status":"completed"}}"#)?;
        Ok(())
    }
}

#[tokio::test]
#[ignore = "audit reproducer: DeepSeek Responses named tool_choice only supports function"]
async fn unsupported_responses_named_tool_choices_are_rejected_before_transport() {
    let cases = [
        (
            r#"{"tool_choice":{"type":"custom","name":"apply_patch"}}"#,
            ToolSchema::function(
                "apply_patch",
                "Apply a patch.",
                serde_json::json!({"type":"object","properties":{}}),
            ),
        ),
        (
            r#"{"tool_choice":{"type":"web_search"}}"#,
            ToolSchema::function(
                "web_search",
                "Search the web.",
                serde_json::json!({"type":"object","properties":{}}),
            ),
        ),
    ];

    let mut violations = Vec::new();
    for (raw, tool) in cases {
        let parsed = ResponsesApiSettings::from_developer_json(raw);
        let accepted = parsed.is_ok();
        let transport = Arc::new(CountingTransport::default());

        if let Ok(settings) = parsed {
            let mut config = ModelConfig::deepseek("audit-placeholder");
            config.defaults.native_web_search = tool.function.name == "web_search";
            let client = ModelClient::new(transport.clone(), config);
            let request = ResponseRequest::new(
                "deepseek-v4-pro",
                vec![deepagent_core::message::Message::user("Audit tool choice")],
            )
            .with_tools(vec![tool])
            .with_tool_choice(settings.effective_tool_choice().unwrap());
            client.stream_response(request).await.unwrap();
        }

        let requests = transport.requests.lock().unwrap();
        eprintln!(
            "Responses tool_choice audit: accepted={accepted} wire={:?}",
            requests.first()
        );
        if accepted || !requests.is_empty() {
            violations.push((raw.to_string(), requests.first().cloned()));
        }
    }

    assert!(
        violations.is_empty(),
        "unsupported named tool_choice reached provider transport: {violations:?}"
    );
}
