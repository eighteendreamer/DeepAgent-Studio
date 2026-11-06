use super::*;

use deepagent_mcp::{adapters_for, McpClient, McpRegistry, MockTransport as McpMockTransport};
use deepagent_models::{ModelClient, ModelConfig, ResponseRequest, ToolSchema};
use deepagent_tools::{PermissionSet, ToolRegistry};

#[derive(Default)]
struct CountingModelTransport {
    requests: Mutex<Vec<serde_json::Value>>,
}

#[async_trait]
impl HttpTransport for CountingModelTransport {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        let sink: &mut dyn deepagent_models::transport::EventSink =
            &mut crate::sequence_fixture::sink(sink);
        self.requests
            .lock()
            .unwrap()
            .push(serde_json::from_str(&request.body)?);
        sink.on_event(r#"{"type":"response.completed","response":{"id":"fixture-response","object":"response","created_at":1,"model":"deepseek-flash","output":[],"status":"completed"}}"#)?;
        Ok(())
    }
}

#[tokio::test]
async fn invalid_mcp_tool_schemas_are_rejected_before_provider_transport() {
    let cases = [
        serde_json::json!({"name":"missing_schema"}),
        serde_json::json!({"name":"string_schema","inputSchema":"not-a-schema"}),
        serde_json::json!({"name":"array_schema","inputSchema":[]}),
    ];
    let mut violations = Vec::new();

    for (index, tool) in cases.into_iter().enumerate() {
        let mcp_transport =
            McpMockTransport::new().with_result("tools/list", serde_json::json!({"tools":[tool]}));
        let client = Arc::new(McpClient::new(Arc::new(mcp_transport)));
        let mut mcp_registry = McpRegistry::new();
        let mcp_result = mcp_registry
            .register(&format!("schema_audit_{index}"), client)
            .await;
        let mcp_registered = mcp_result.is_ok();
        assert_eq!(
            mcp_result.unwrap(),
            0,
            "no invalid definition is a capability"
        );
        assert!(mcp_registry.all_tools().is_empty());

        let model_transport = Arc::new(CountingModelTransport::default());
        let mut tool_registry = ToolRegistry::new();
        let mut tool_registered = false;
        if mcp_registered {
            for adapter in adapters_for(Arc::new(mcp_registry)) {
                tool_registered |= tool_registry.register(adapter).is_ok();
            }
            let tools = tool_registry
                .visible_to(&PermissionSet::developer())
                .into_iter()
                .map(|descriptor| {
                    ToolSchema::function(
                        descriptor.name,
                        descriptor.description,
                        descriptor.parameters,
                    )
                })
                .collect::<Vec<_>>();
            if !tools.is_empty() {
                let client = ModelClient::new(
                    model_transport.clone(),
                    ModelConfig::deepseek("audit-placeholder"),
                );
                client
                    .stream_response(
                        ResponseRequest::new(
                            "deepseek-v4-pro",
                            vec![deepagent_core::message::Message::user("Audit tool schema")],
                        )
                        .with_tools(tools),
                    )
                    .await
                    .unwrap();
            }
        }

        let requests = model_transport.requests.lock().unwrap();
        let parameters = requests
            .first()
            .and_then(|request| request["tools"].as_array())
            .and_then(|tools| tools.first())
            .and_then(|tool| tool.get("parameters"))
            .cloned();
        eprintln!(
            "MCP schema audit: mcp_registered={mcp_registered} tool_registered={tool_registered} parameters={parameters:?} transport_calls={}",
            requests.len()
        );
        if !mcp_registered || tool_registered || !requests.is_empty() {
            violations.push((index, parameters, requests.len()));
        }
    }

    assert!(
        violations.is_empty(),
        "invalid MCP schemas reached the model provider boundary: {violations:?}"
    );
}

#[tokio::test]
async fn invalid_mcp_schema_does_not_block_normal_request_with_healthy_tool() {
    let transport = McpMockTransport::new().with_result("tools/list", serde_json::json!({
        "tools":[
            {"name":"invalid","inputSchema":{"required":"not-an-array"}},
            {"name":"missing"},
            {"name":"healthy","inputSchema":{"type":"object","properties":{"q":{"type":"string"}}}}
        ]
    }));
    let mut mcp = McpRegistry::new();
    assert_eq!(
        mcp.register("mixed", Arc::new(McpClient::new(Arc::new(transport))))
            .await
            .unwrap(),
        1
    );
    let mut registry = ToolRegistry::new();
    for adapter in adapters_for(Arc::new(mcp)) {
        registry.register(adapter).unwrap();
    }
    let tools = registry
        .visible_to(&PermissionSet::developer())
        .into_iter()
        .map(|tool| ToolSchema::function(tool.name, tool.description, tool.parameters))
        .collect();
    let transport = Arc::new(CountingModelTransport::default());
    let client = ModelClient::new(
        transport.clone(),
        ModelConfig::deepseek("audit-placeholder"),
    );
    client
        .stream_response(
            ResponseRequest::new(
                "deepseek-flash",
                vec![deepagent_core::message::Message::user(
                    "normal text request",
                )],
            )
            .with_tools(tools),
        )
        .await
        .unwrap();
    let requests = transport.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0]["tools"].as_array().unwrap().len(), 1);
    assert_eq!(requests[0]["tools"][0]["name"], "mcp__mixed__healthy");
}
