//! MCP result projection must preserve model content and separate client metadata.

use super::responses_fixture::{call, completed, text};
use super::*;
use deepagent_app_core::{McpServerDto, McpService, PermissionPreset};
use deepagent_builtins::ToolSearchMode;
use deepagent_core::event::EventPayload;
use deepagent_persistence::event_store::EventStore;
use deepagent_persistence::run_control::{RunActionState, RunControlStore};
use deepagent_persistence::run_store::RunStore;
use std::time::Duration;

const CALL: &str = "audit-image-call";
const RUN: &str = "audit-mcp-image-result";

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
        let index = self.calls.fetch_add(1, Ordering::AcqRel);
        let item = match index {
            0 => call(CALL, "mcp__image_output__get_image", serde_json::json!({})),
            1 => text("audit-mcp-final", "MCP result received."),
            _ => return Err(CoreError::other("unexpected model request")),
        };
        let events = completed(
            &format!("audit-mcp-response-{index}"),
            "deepseek-v4-flash",
            item,
            serde_json::json!({"input_tokens":20,"output_tokens":5,"total_tokens":25}),
        );
        emit_responses_events(sink, &events)
    }
}

#[tokio::test]
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
async fn h127_mixed_mcp_images_keep_order_without_inline_bytes_in_history() {
    const DATA: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jZh0AAAAASUVORK5CYII=";
    let observed = mcp_result_observation(serde_json::json!({
        "content": [
            {"type":"text", "text":"before-image"},
            {"type":"image", "data":DATA, "mimeType":"image/png"},
            {"type":"text", "text":"between-images"},
            {"type":"image", "data":DATA, "mimeType":"image/png"},
            {"type":"text", "text":"after-images"}
        ],
        "isError":false
    }))
    .await;
    let output = observed.requests[1]["input"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["type"] == "function_call_output")
        .unwrap()["output"]
        .clone();
    let parts = output
        .as_array()
        .expect("mixed tool output must stay typed");
    let images: Vec<_> = parts
        .iter()
        .enumerate()
        .filter(|(_, part)| part["type"] == "input_image")
        .collect();
    assert_eq!(images.len(), 2);
    let text_position = |marker: &str| {
        parts
            .iter()
            .position(|part| {
                part["type"] == "input_text"
                    && part["text"]
                        .as_str()
                        .is_some_and(|text| text.contains(marker))
            })
            .unwrap()
    };
    assert!(text_position("before-image") < images[0].0);
    assert!(images[0].0 < text_position("between-images"));
    assert!(text_position("between-images") < images[1].0);
    assert!(images[1].0 < text_position("after-images"));
    assert!(
        !observed.history_json.contains(DATA),
        "raw image bytes must not enter session events"
    );
    assert!(
        !observed.run_events_json.contains(DATA),
        "raw image bytes must not enter run events"
    );
    assert!(
        observed.run_events_json.contains("sha256"),
        "run events must retain the same typed image refs"
    );
    assert!(
        observed.history_json.contains("sha256"),
        "durable image identity must survive reopen"
    );
    assert_eq!(
        observed.replayed_output, output,
        "a real cold continuation must materialize the same ordered tool images without redispatch"
    );
    assert!(observed.session_ok);
}

#[tokio::test]
async fn h127_invalid_mcp_images_return_paired_visible_failure_without_raw_data() {
    for (mime, data) in [
        ("text/plain", "aDEyNy1jYW5hcnk="),
        ("image/png", "not-base64-h127-canary"),
    ] {
        let observed = mcp_result_observation(serde_json::json!({
            "content":[{"type":"image", "data":data, "mimeType":mime}], "isError":false
        }))
        .await;
        assert!(
            !observed.session_ok,
            "invalid tool image must not look successful: {mime}"
        );
        let outputs: Vec<_> = observed.requests[1]["input"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|item| item["type"] == "function_call_output" && item["call_id"] == CALL)
            .collect();
        assert_eq!(outputs.len(), 1);
        assert!(outputs[0]["output"].to_string().contains("image"));
        assert!(!observed.requests[1].to_string().contains(data));
        assert!(!observed.history_json.contains(data));
        assert!(!observed.run_events_json.contains(data));
    }
}

#[tokio::test]
async fn h127_failed_mcp_image_result_preserves_error_and_image_evidence() {
    const DATA: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jZh0AAAAASUVORK5CYII=";
    let observed = mcp_result_observation(serde_json::json!({
        "content":[{"type":"text","text":"image-diagnostic"},{"type":"image","data":DATA,"mimeType":"image/png"}],
        "isError":true, "structuredContent":{"reason":"synthetic-failure"}
    })).await;
    assert!(!observed.session_ok);
    assert!(!observed.event_ok);
    assert_eq!(observed.action_state, RunActionState::Failed);
    assert!(!observed.history_json.contains(DATA));
    let output = observed.requests[1]["input"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["type"] == "function_call_output")
        .unwrap()["output"]
        .clone();
    let parts = output.as_array().unwrap();
    assert_eq!(
        parts
            .iter()
            .filter(|part| part["type"] == "input_image")
            .count(),
        1
    );
    let first: serde_json::Value =
        serde_json::from_str(parts[0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(first["status"], "error");
    assert_eq!(first["error"]["isError"], true);
    assert_eq!(
        first["error"]["structuredContent"]["reason"],
        "synthetic-failure"
    );
    assert!(parts.iter().any(|part| part["text"] == "image-diagnostic"));
}

#[tokio::test]
async fn mcp_public_text_and_structured_result_reach_the_model() {
    assert_metadata_projection(false).await;
}

#[tokio::test]
async fn mcp_execution_error_is_preserved_through_events_storage_and_model_input() {
    for flag in [Some(false), Some(true), None] {
        const MARKER: &str = "audit-mcp-execution-result";
        let mut result = serde_json::json!({
            "content": [{"type":"text", "text":MARKER}],
            "structuredContent": {"audit":"retained"}
        });
        if let Some(flag) = flag {
            result["isError"] = flag.into();
        }
        let observed = mcp_result_observation(result).await;
        let expected_ok = flag != Some(true);
        assert_eq!(observed.event_ok, expected_ok);
        assert_eq!(observed.session_ok, expected_ok);
        assert_eq!(
            observed.action_state,
            if expected_ok {
                RunActionState::Completed
            } else {
                RunActionState::Failed
            }
        );
        assert_eq!(observed.session_output["isError"], !expected_ok);
        assert_eq!(observed.session_output["content"][0]["text"], MARKER);
        assert_eq!(
            observed.session_output["structuredContent"]["audit"],
            "retained"
        );
        let outputs: Vec<_> = observed.requests[1]["input"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|item| item["type"] == "function_call_output" && item["call_id"] == CALL)
            .collect();
        assert_eq!(
            outputs.len(),
            1,
            "one result must remain paired with the call"
        );
        let wire: serde_json::Value =
            serde_json::from_str(outputs[0]["output"].as_str().unwrap()).unwrap();
        assert_eq!(wire["status"], if expected_ok { "ok" } else { "error" });
        let payload = &wire[if expected_ok { "result" } else { "error" }];
        assert_eq!(payload["isError"], !expected_ok);
        assert_eq!(payload["content"][0]["text"], MARKER);
        assert_eq!(payload["structuredContent"]["audit"], "retained");
        if !expected_ok {
            assert_eq!(payload["error"], MARKER);
        }
        assert_eq!(
            observed.terminal_kind, "succeeded",
            "the model may acknowledge a failed tool and finish the turn"
        );
        eprintln!("MCP error semantics: isError={flag:?} model_requests={} event_ok={} session_ok={} action={:?} wire_status={} terminal={} sqlite_reopened=true",
            observed.requests.len(), observed.event_ok, observed.session_ok, observed.action_state, wire["status"], observed.terminal_kind);
    }
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
    mcp_result_observation(result).await.requests
}

struct McpResultObservation {
    requests: Vec<serde_json::Value>,
    event_ok: bool,
    session_ok: bool,
    session_output: serde_json::Value,
    action_state: RunActionState,
    terminal_kind: String,
    history_json: String,
    run_events_json: String,
    replayed_output: serde_json::Value,
}

async fn mcp_result_observation(result: serde_json::Value) -> McpResultObservation {
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

    let db_path = root.path().join("mcp-result.sqlite");
    let db = Arc::new(Database::open(&db_path).unwrap());
    let secrets = Arc::new(MemorySecretStore::new());
    let settings = Arc::new(SettingsService::new(
        db.clone(),
        Arc::new(MockTransport::with_get_json(
            r#"{"data":[{"id":"deepseek-v4-flash","object":"model","owned_by":"deepseek","input_modalities":["text","image"],"output_modalities":["text"]}]}"#,
        )),
        secrets.clone(),
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
    let chat = ChatService::new(db.clone(), settings, model.clone(), root.path()).with_mcp(mcp);
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
            Some(RUN),
            |_| {},
            |_| {},
        ),
    )
    .await;
    ready.close_all().await.unwrap();
    let session = outcome.expect("the MCP result run must finish").unwrap();
    let live = chat.run_events(RUN, None).unwrap();
    let live_tools: Vec<_> = live
        .iter()
        .filter(|event| event.event_type == "tool_completed" && event.data["call_id"] == CALL)
        .collect();
    assert_eq!(live_tools.len(), 1);
    let event_ok = live_tools[0].data["ok"].as_bool().unwrap();
    drop(chat);
    drop(db);
    let reopened = Arc::new(Database::open(&db_path).unwrap());
    let events = RunStore::new(&reopened).events_after(RUN, None).unwrap();
    let stored_tools: Vec<_> = events
        .iter()
        .filter(|event| event.event_type == "tool_completed" && event.data["call_id"] == CALL)
        .collect();
    assert_eq!(stored_tools.len(), 1);
    assert_eq!(stored_tools[0].data["ok"], event_ok);
    let terminals: Vec<_> = events
        .iter()
        .filter(|event| event.event_type == "run_terminal")
        .collect();
    assert_eq!(terminals.len(), 1);
    let terminal_kind = terminals[0].data["kind"].as_str().unwrap().to_owned();
    let actions = RunControlStore::new(&reopened).list_actions(RUN).unwrap();
    assert_eq!(actions.len(), 1);
    assert_eq!(actions[0].call_id, CALL);
    let history = EventStore::new(&reopened)
        .load_session(session.parse().unwrap())
        .unwrap();
    let completions: Vec<_> = history
        .iter()
        .filter_map(|event| match &event.payload {
            EventPayload::ToolCallCompleted {
                call_id,
                ok,
                output,
                ..
            } if call_id == CALL => Some((*ok, output.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(completions.len(), 1);
    let (session_ok, session_output) = completions.into_iter().next().unwrap();

    // No MCP service is attached after reopen. A continuation must use the
    // original admitted refs, not rerun the tool to reconstruct its evidence.
    let resumed_settings = Arc::new(SettingsService::new(
        reopened.clone(),
        Arc::new(MockTransport::with_get_json(
            r#"{"data":[{"id":"deepseek-v4-flash","object":"model","owned_by":"deepseek","input_modalities":["text","image"],"output_modalities":["text"]}]}"#,
        )),
        secrets,
    ));
    let replay_model = Arc::new(McpOutputModel {
        calls: AtomicUsize::new(1),
        requests: Mutex::new(Vec::new()),
    });
    let resumed = ChatService::new(
        reopened.clone(),
        resumed_settings,
        replay_model.clone(),
        root.path(),
    );
    tokio::time::timeout(
        Duration::from_secs(5),
        resumed.run_in_session(
            "Inspect the retained evidence without repeating the tool.",
            Some(&session),
            None,
            None,
            Vec::new(),
            None,
            false,
            Some("h127-cold-replay"),
            |_| {},
            |_| {},
        ),
    )
    .await
    .expect("cold continuation must finish")
    .unwrap();
    let replay_requests = replay_model.requests.lock().unwrap();
    assert_eq!(replay_requests.len(), 1);
    let replayed_output = replay_requests[0]["input"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["type"] == "function_call_output" && item["call_id"] == CALL)
        .expect("cold continuation retains original paired output")["output"]
        .clone();
    let after = EventStore::new(&reopened)
        .load_session(session.parse().unwrap())
        .unwrap();
    assert_eq!(after.iter().filter(|event|matches!(&event.payload,EventPayload::ToolCallRequested{call} if call.id==CALL)).count(),1,"cold continuation must not redispatch the MCP call");

    let requests = model.requests.lock().unwrap().clone();
    assert_eq!(
        requests.len(),
        2,
        "the tool result must be followed by a model request"
    );
    McpResultObservation {
        requests,
        event_ok,
        session_ok,
        session_output,
        action_state: actions[0].state,
        terminal_kind,
        history_json: serde_json::to_string(&history).unwrap(),
        run_events_json: serde_json::to_string(&events).unwrap(),
        replayed_output,
    }
}
