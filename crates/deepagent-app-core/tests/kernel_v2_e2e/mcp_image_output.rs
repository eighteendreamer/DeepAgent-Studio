//! MCP result projection must preserve model content and separate client metadata.

use super::*;
use deepagent_app_core::{McpServerDto, McpService, PermissionPreset};
use deepagent_builtins::ToolSearchMode;
use std::time::Duration;

struct McpOutputModel {
    calls: AtomicUsize,
    requests: Mutex<Vec<serde_json::Value>>,
}

#[async_trait]
impl HttpTransport for McpOutputModel {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        self.requests
            .lock()
            .unwrap()
            .push(serde_json::from_str(&request.body).unwrap());
        let events = match self.calls.fetch_add(1, Ordering::AcqRel) {
            0 => response_function_call(
                "audit-image-call",
                "mcp__image_output__get_image",
                serde_json::json!({}),
            ),
            1 => response_text("MCP result received."),
            _ => return Err(CoreError::other("unexpected model request")),
        };
        emit_responses_events(sink, &events)
    }
}

#[tokio::test]
#[ignore = "audit reproducer: MCP image results must reach DeepSeek Responses as input_image parts"]
async fn audit_mcp_image_result_is_not_serialized_as_text_json() {
    const IMAGE_DATA: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jZh0AAAAASUVORK5CYII=";
    let requests = mcp_result_requests(serde_json::json!({
        "content": [{ "type": "image", "data": IMAGE_DATA, "mimeType": "image/png" }],
        "isError": false
    }))
    .await;
    let output = requests[1]["input"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["type"] == "function_call_output")
        .expect("the tool call and its result must remain paired")["output"]
        .clone();
    let output_is_typed_image = output.as_array().is_some_and(|parts| {
        parts
            .iter()
            .any(|part| part["type"] == "input_image" && part["image_url"].is_string())
    });
    let output_is_json_string = output.as_str().is_some_and(|value| {
        serde_json::from_str::<serde_json::Value>(value)
            .is_ok_and(|parsed| parsed["result"]["content"][0]["type"] == "image")
    });
    let raw_data_present = requests[1].to_string().contains(IMAGE_DATA);
    eprintln!(
        "MCP image output audit: wire_output_kind={}, typed_input_image={output_is_typed_image}, raw_json_string={output_is_json_string}, raw_image_data_present={raw_data_present}",
        if output.is_string() { "string" } else if output.is_array() { "array" } else { "other" }
    );
    assert!(
        output_is_typed_image,
        "MCP image result must be emitted as typed input_image content parts, not a JSON string"
    );
}

#[tokio::test]
async fn mcp_public_text_and_structured_result_reach_the_model() {
    assert_metadata_projection(false).await;
}

#[tokio::test]
#[ignore = "audit: client-only MCP result _meta must not become model-visible tool output"]
async fn audit_mcp_client_metadata_is_not_sent_to_model() {
    assert_metadata_projection(true).await;
}

async fn assert_metadata_projection(include_meta: bool) {
    const PUBLIC: &str = "audit-public-mcp-result";
    const STRUCTURED: &str = "audit-public-structured-value";
    const PRIVATE: &str = "audit-client-only-metadata-canary";
    let mut result = serde_json::json!({
        "content": [{ "type": "text", "text": PUBLIC }],
        "structuredContent": { "visible": STRUCTURED },
        "isError": false
    });
    if include_meta {
        result["_meta"] = serde_json::json!({
            "audit.example/clientState": { "privateDetail": PRIVATE }
        });
    }
    let requests = mcp_result_requests(result).await;
    let first_wire = requests[0].to_string();
    let next_wire = requests[1].to_string();
    let private_before = first_wire.contains(PRIVATE);
    let private_after = next_wire.contains(PRIVATE);
    eprintln!(
        "MCP result metadata audit: include_meta={include_meta}, model_requests={}, public_text_present={}, structured_content_present={}, private_before_tool={private_before}, private_after_tool={private_after}",
        requests.len(), next_wire.contains(PUBLIC), next_wire.contains(STRUCTURED)
    );
    assert!(
        !private_before,
        "the canary must originate only in the MCP result"
    );
    assert!(
        next_wire.contains(PUBLIC),
        "public text must remain visible"
    );
    assert!(
        next_wire.contains(STRUCTURED),
        "structured content must remain visible"
    );
    assert!(
        !private_after,
        "client-only MCP result metadata must not enter the provider request"
    );
}

async fn mcp_result_requests(result: serde_json::Value) -> Vec<serde_json::Value> {
    let root = tempfile::tempdir().unwrap();
    let script = root.path().join("image-result-mcp.cjs");
    std::fs::write(
        &script,
        format!(
            r#"
const readline = require('node:readline');
const result = {result};
function reply(id, result) {{
  process.stdout.write(JSON.stringify({{ jsonrpc: '2.0', id, result }}) + '\n');
}}
readline.createInterface({{ input: process.stdin }}).on('line', line => {{
  const request = JSON.parse(line);
  if (request.method === 'initialize') reply(request.id, {{
    protocolVersion: request.params.protocolVersion,
    capabilities: {{ tools: {{}} }},
    serverInfo: {{ name: 'mcp-image-audit', version: '1' }}
  }});
  else if (request.method === 'tools/list') reply(request.id, {{ tools: [{{
    name: 'get_image', description: 'Return a small image', inputSchema: {{ type: 'object' }}
  }}] }});
  else if (request.method === 'tools/call') reply(request.id, result);
  else if (request.id !== undefined) reply(request.id, {{}});
}});
process.stdin.on('end', () => process.exit(0));
setTimeout(() => process.exit(2), 15000).unref();
"#,
            result = serde_json::to_string(&result).unwrap()
        ),
    )
    .unwrap();

    let db = Arc::new(Database::open_in_memory().unwrap());
    let settings = Arc::new(SettingsService::new(
        db.clone(),
        Arc::new(MockTransport::with_get_json(
            r#"{"data":[{"id":"deepseek-v4-flash","object":"model","owned_by":"deepseek"}]}"#,
        )),
        Arc::new(MemorySecretStore::new()),
    ));
    settings
        .initialize("offline-audit-placeholder")
        .await
        .unwrap();
    settings
        .set_active_permission_preset(PermissionPreset::FullAccess)
        .unwrap();
    settings
        .set_tool_search_mode(ToolSearchMode::Disabled)
        .unwrap();

    let mcp = Arc::new(McpService::new(db.clone()));
    let dto: McpServerDto = serde_json::from_value(serde_json::json!({
        "name": "image_output", "transport": "stdio", "enabled": true,
        "command": "node", "args": [script]
    }))
    .unwrap();
    mcp.upsert(dto).unwrap();
    mcp.start_preparation();
    let prepared = tokio::time::timeout(Duration::from_secs(5), async {
        while mcp.ready_registry().server_count() != 1 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    let ready = mcp.ready_registry();
    if prepared.is_err() {
        ready.close_all().await.unwrap();
        panic!("the local MCP image fixture must be ready before the model run");
    }

    let model = Arc::new(McpOutputModel {
        calls: AtomicUsize::new(0),
        requests: Mutex::new(Vec::new()),
    });
    let chat = ChatService::new(db, settings, model.clone(), root.path()).with_mcp(mcp);
    let outcome = tokio::time::timeout(
        Duration::from_secs(5),
        chat.run_in_session(
            "Use the MCP tool and inspect its result.",
            None,
            None,
            None,
            Vec::new(),
            None,
            false,
            Some("audit-mcp-image-result"),
            |_| {},
            |_| {},
        ),
    )
    .await;
    ready.close_all().await.unwrap();
    outcome.expect("the MCP result run must finish").unwrap();

    let requests = model.requests.lock().unwrap().clone();
    assert_eq!(
        requests.len(),
        2,
        "the tool result must be followed by a model request"
    );
    requests
}
