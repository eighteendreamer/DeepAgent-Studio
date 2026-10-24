//! Real stdio MCP discovery must survive clean service/database reconstruction.

use super::*;
use deepagent_app_core::{McpServerDto, McpService};
use deepagent_builtins::ToolSearchMode;
use deepagent_core::event::EventPayload;
use deepagent_persistence::event_store::EventStore;
use std::path::Path;
use std::time::Duration;

const INSPECT: &str = "mcp__discovery_audit__inspect";
const OTHER: &str = "mcp__discovery_audit__other";
const CANARY: &str = "AUDIT_DISCOVERY_CANARY";

#[derive(Default)]
struct DiscoveryModel {
    discover: bool,
    requests: Mutex<Vec<serde_json::Value>>,
}

#[async_trait]
impl HttpTransport for DiscoveryModel {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        let request: serde_json::Value = serde_json::from_str(&request.body)?;
        let model = request["model"].clone();
        let attempt = {
            let mut requests = self.requests.lock().unwrap();
            requests.push(request);
            requests.len()
        };
        if attempt > if self.discover { 4 } else { 2 } {
            return Err(CoreError::other("unexpected discovery audit request"));
        }
        let id = format!("discovery-item-{attempt}");
        let item = match (self.discover, attempt) {
            (true, 1) => serde_json::json!({
                "type":"function_call", "id":id, "call_id":"discovery-search",
                "name":"tool_search", "status":"completed",
                "arguments":serde_json::json!({"query":format!("select:{INSPECT}")}).to_string()
            }),
            (true, 2) => serde_json::json!({
                "type":"function_call", "id":id, "call_id":"discovery-inspect",
                "name":INSPECT, "status":"completed",
                "arguments":serde_json::json!({"value":CANARY}).to_string()
            }),
            _ => serde_json::json!({
                "type":"message", "id":id, "role":"assistant", "status":"completed",
                "content":[{"type":"output_text", "text":"Discovery audit complete.",
                    "annotations":[]}]
            }),
        };
        let events = [
            serde_json::json!({"type":"response.created", "sequence_number":0,
                "response":{"id":format!("discovery-response-{attempt}"),
                    "object":"response", "model":model, "status":"in_progress"}}),
            serde_json::json!({"type":"response.output_item.added", "sequence_number":1,
                "output_index":0, "item":item}),
            serde_json::json!({"type":"response.output_item.done", "sequence_number":2,
                "output_index":0, "item":item}),
            serde_json::json!({"type":"response.completed", "sequence_number":3,
                "response":{"id":format!("discovery-response-{attempt}"),
                    "object":"response", "model":model, "status":"completed", "output":[item]}}),
        ];
        emit_responses_events(
            sink,
            &events.iter().map(ToString::to_string).collect::<Vec<_>>(),
        )
    }
}

fn settings(db: &Arc<Database>, secrets: &Arc<MemorySecretStore>) -> Arc<SettingsService> {
    Arc::new(SettingsService::new(
        db.clone(),
        Arc::new(MockTransport::with_get_json(
            r#"{"data":[{"id":"deepseek-flash","object":"model","owned_by":"deepseek"}]}"#,
        )),
        secrets.clone(),
    ))
}

async fn prepare(mcp: &Arc<McpService>) {
    mcp.start_preparation();
    let ready = tokio::time::timeout(Duration::from_secs(5), async {
        while mcp.ready_registry().server_count() != 1 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    if ready.is_err() {
        mcp.ready_registry().close_all().await.unwrap();
        panic!("the discovery MCP fixture must become ready");
    }
}

async fn turn(chat: &ChatService, session: &str, run: &str) -> Result<()> {
    tokio::time::timeout(
        Duration::from_secs(5),
        chat.run_in_session(
            "Run the discovery audit.",
            Some(session),
            None,
            None,
            Vec::new(),
            None,
            false,
            Some(run),
            |_| {},
            |_| {},
        ),
    )
    .await
    .map_err(|_| CoreError::other("discovery audit run timed out"))??;
    Ok(())
}

fn assert_terminal(chat: &ChatService, run: &str) {
    let events = chat.run_events(run, None).unwrap();
    let terminals: Vec<_> = events
        .iter()
        .filter(|e| e.event_type == "run_terminal")
        .collect();
    assert_eq!(terminals.len(), 1);
    assert_eq!(terminals[0].data["kind"], "succeeded");
}

fn discovery_events(db: &Database, session: &str) -> Vec<Vec<String>> {
    EventStore::new(db)
        .load_session(session.parse().unwrap())
        .unwrap()
        .into_iter()
        .filter_map(|event| match event.payload {
            EventPayload::ToolsDiscovered { names } => Some(names),
            _ => None,
        })
        .collect()
}

fn advertised(request: &serde_json::Value) -> Vec<&str> {
    request["tools"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .filter(|name| name.starts_with("mcp__discovery_audit__"))
        .collect()
}

fn assert_replayed_call(request: &serde_json::Value) {
    let input = request["input"].as_array().unwrap();
    let calls: Vec<_> = input
        .iter()
        .filter(|item| item["type"] == "function_call" && item["call_id"] == "discovery-inspect")
        .collect();
    let outputs: Vec<_> = input
        .iter()
        .filter(|item| {
            item["type"] == "function_call_output" && item["call_id"] == "discovery-inspect"
        })
        .collect();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0]["name"], INSPECT);
    assert_eq!(outputs.len(), 1);
    assert!(outputs[0]["output"].to_string().contains(CANARY));
}

fn report(path: &Path) -> serde_json::Value {
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

#[tokio::test]
async fn deferred_mcp_discovery_survives_turns_and_database_reopen_without_session_leak() {
    let root = tempfile::tempdir().unwrap();
    let script = root.path().join("discovery-mcp.cjs");
    let report_path = root.path().join("discovery-report.json");
    std::fs::write(
        &script,
        r#"
const fs = require('node:fs');
const readline = require('node:readline');
const state = { initialize: 0, list: 0, calls: [] };
const report = process.env.AUDIT_DISCOVERY_REPORT;
function save() { fs.writeFileSync(report, JSON.stringify(state)); }
function reply(id, result) {
  process.stdout.write(JSON.stringify({jsonrpc: '2.0', id, result}) + '\n');
}
readline.createInterface({input: process.stdin}).on('line', line => {
  const request = JSON.parse(line);
  if (request.method === 'initialize') {
    state.initialize++; save();
    reply(request.id, {protocolVersion: request.params.protocolVersion,
      capabilities: {tools: {}}, serverInfo: {name: 'discovery-audit', version: '1'}});
  } else if (request.method === 'tools/list') {
    state.list++; save();
    reply(request.id, {tools: ['inspect', 'other'].map(name => ({
      name, description: 'Inspect the supplied audit value',
      inputSchema: {type: 'object', properties: {value: {type: 'string'}},
        required: ['value'], additionalProperties: false}
    }))});
  } else if (request.method === 'tools/call') {
    state.calls.push(request.params); save();
    reply(request.id, {content: [{type: 'text', text: request.params.arguments.value}],
      isError: false});
  } else if (request.id !== undefined) reply(request.id, {});
});
process.stdin.on('end', () => process.exit(0));
setTimeout(() => process.exit(2), 20000).unref();
"#,
    )
    .unwrap();
    let database_path = root.path().join("state.sqlite");
    let secrets = Arc::new(MemorySecretStore::new());
    let (session, previous_tools, previous_discovery) = {
        let db = Arc::new(Database::open(&database_path).unwrap());
        let settings = settings(&db, &secrets);
        settings
            .initialize("offline-audit-placeholder")
            .await
            .unwrap();
        settings
            .set_active_permission_preset(PermissionPreset::FullAccess)
            .unwrap();
        settings
            .set_tool_search_mode(ToolSearchMode::Enabled)
            .unwrap();
        let mcp = Arc::new(McpService::new(db.clone()));
        mcp.upsert(
            serde_json::from_value::<McpServerDto>(serde_json::json!({
                "name":"discovery_audit", "transport":"stdio", "enabled":true,
                "command":"node", "args":[script],
                "env":{"AUDIT_DISCOVERY_REPORT":report_path}
            }))
            .unwrap(),
        )
        .unwrap();
        prepare(&mcp).await;
        let model = Arc::new(DiscoveryModel {
            discover: true,
            ..Default::default()
        });
        let chat = ChatService::new(db.clone(), settings, model.clone(), root.path())
            .with_mcp(mcp.clone());
        let session = chat.create_session(None).unwrap();
        let first = turn(&chat, &session, "audit-discovery-first").await;
        let second = if first.is_ok() {
            turn(&chat, &session, "audit-discovery-next").await
        } else {
            Err(CoreError::other("first discovery run failed"))
        };
        // Close the real child before assertions, including defect assertions.
        mcp.ready_registry().close_all().await.unwrap();
        first.unwrap();
        second.unwrap();
        assert_terminal(&chat, "audit-discovery-first");
        assert_terminal(&chat, "audit-discovery-next");
        let requests = model.requests.lock().unwrap();
        assert_eq!(requests.len(), 4);
        assert!(advertised(&requests[0]).is_empty());
        assert!(requests[0]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["name"] == "tool_search"));
        for request in &requests[1..] {
            assert_eq!(advertised(request), [INSPECT]);
            assert_eq!(request["tools"], requests[1]["tools"]);
        }
        assert_replayed_call(&requests[2]);
        assert_replayed_call(&requests[3]);
        assert_eq!(chat.discovered_tool_names(&session), [INSPECT]);
        let discovered = discovery_events(&db, &session);
        assert_eq!(discovered, vec![vec![INSPECT.to_string()]]);
        let server = report(&report_path);
        assert_eq!(server["initialize"], 1);
        assert_eq!(server["list"], 1);
        assert_eq!(
            server["calls"],
            serde_json::json!([{"name":"inspect", "arguments":{"value":CANARY}}])
        );
        eprintln!(
            "discovery host 1: requests={}, server={}, discovery={discovered:?}",
            requests.len(),
            server
        );
        (session, requests[1]["tools"].clone(), discovered)
    };

    // All first-host service/database handles left scope; no injected discovery state.
    let db = Arc::new(Database::open(&database_path).unwrap());
    let settings = settings(&db, &secrets);
    let mcp = Arc::new(McpService::new(db.clone()));
    prepare(&mcp).await;
    let model = Arc::new(DiscoveryModel::default());
    let chat =
        ChatService::new(db.clone(), settings, model.clone(), root.path()).with_mcp(mcp.clone());
    let before_resume = chat.discovered_tool_names(&session);
    let restored = turn(&chat, &session, "audit-discovery-reopened").await;
    let restored_names = chat.discovered_tool_names(&session);
    let fresh = chat.create_session(None).unwrap();
    let fresh_result = if restored.is_ok() {
        turn(&chat, &fresh, "audit-discovery-fresh").await
    } else {
        Err(CoreError::other("restored discovery run failed"))
    };
    mcp.ready_registry().close_all().await.unwrap();
    restored.unwrap();
    fresh_result.unwrap();
    assert!(
        before_resume.is_empty(),
        "a reconstructed service has no in-memory discoveries"
    );
    assert_eq!(restored_names, [INSPECT]);
    assert!(chat.discovered_tool_names(&fresh).is_empty());
    assert_eq!(discovery_events(&db, &session), previous_discovery);
    assert!(discovery_events(&db, &fresh).is_empty());
    assert_terminal(&chat, "audit-discovery-reopened");
    assert_terminal(&chat, "audit-discovery-fresh");
    let requests = model.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(advertised(&requests[0]), [INSPECT]);
    assert_eq!(requests[0]["tools"], previous_tools);
    assert_replayed_call(&requests[0]);
    assert!(advertised(&requests[1]).is_empty());
    assert!(!requests[1]["input"].to_string().contains(CANARY));
    assert!(requests.iter().all(|r| !advertised(r).contains(&OTHER)));
    let server = report(&report_path);
    assert_eq!(server["initialize"], 1);
    assert_eq!(server["list"], 1);
    assert_eq!(server["calls"], serde_json::json!([]));
    eprintln!("discovery host 2: requests={}, server={}, restored={restored_names:?}, fresh=[]; tools snapshot unchanged", requests.len(), server);
}
