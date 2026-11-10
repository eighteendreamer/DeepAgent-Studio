use super::*;
use crate::transport::{EventSink, TransportRequest};
use deepagent_core::error::CoreError;
use std::sync::Mutex;

#[derive(Default)]
struct Capture(Mutex<Vec<serde_json::Value>>);

#[async_trait::async_trait]
impl HttpTransport for Capture {
    async fn stream(&self, request: TransportRequest, _: &mut dyn EventSink) -> Result<()> {
        self.0
            .lock()
            .unwrap()
            .push(serde_json::from_str(&request.body)?);
        Err(CoreError::other("synthetic stop after capturing wire"))
    }
}

#[tokio::test]
async fn h219_tool_projection_matches_actual_adapter_for_both_protocols_and_defaults() {
    for mode in [WireMode::Responses, WireMode::ChatCompletions] {
        for cancelled in [false, true] {
            for empty in [false, true] {
                let transport = Arc::new(Capture::default());
                let client = ModelClient::new(
                    transport.clone(),
                    ModelConfig::deepseek("synthetic-placeholder")
                        .with_wire_mode(mode)
                        .with_defaults(ResponseDefaults::default()),
                );
                let tools = if empty {
                    Vec::new()
                } else {
                    ["read_file", "apply_patch", "web_search"]
                        .map(|name| {
                            crate::ToolSchema::function(
                                name,
                                "private description",
                                serde_json::json!({"type":"object"}),
                            )
                        })
                        .to_vec()
                };
                let projection = client.project_tool_schemas("deepseek-v4-pro", &tools);
                assert!(transport.0.lock().unwrap().is_empty());
                let request = ResponseRequest::new(
                    "deepseek-v4-pro",
                    vec![deepagent_core::message::Message::user("private prompt")],
                )
                .with_tools(tools.clone());
                let result = if cancelled {
                    client
                        .stream_response_cancelled(request, Arc::new(AtomicBool::new(false)))
                        .await
                } else {
                    client.stream_response(request).await
                };
                assert!(result.is_err());
                let requests = transport.0.lock().unwrap();
                assert_eq!(requests.len(), 1);
                let wire = requests[0]
                    .get("tools")
                    .cloned()
                    .unwrap_or_else(|| serde_json::json!([]));
                assert_eq!(serde_json::json!(projection.tools), wire);
                assert_eq!(projection.model, requests[0]["model"]);
                assert_eq!(projection.wire_mode, mode);
                assert!(
                    tools.iter().all(|tool| tool.kind == "function"),
                    "projection must not mutate the host schema"
                );
                if !empty {
                    if mode == WireMode::Responses {
                        assert_eq!(wire[1]["type"], "custom");
                        assert!(wire[1].get("parameters").is_none());
                        assert_eq!(wire[2]["type"], "function");
                    } else {
                        assert!(wire
                            .as_array()
                            .unwrap()
                            .iter()
                            .all(|tool| tool["type"] == "function"
                                && tool["function"]["parameters"].is_object()));
                    }
                }
            }
        }
    }
}
