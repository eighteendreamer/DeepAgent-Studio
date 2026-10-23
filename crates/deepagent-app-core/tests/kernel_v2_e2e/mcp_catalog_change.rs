//! A server's tool-list change should reach the next run's ready snapshot and wire schema.

use super::*;
use deepagent_app_core::{McpServerDto, McpService};
use deepagent_builtins::ToolSearchMode;
use std::time::Duration;

struct CatalogChangeModel {
    requests: Mutex<Vec<serde_json::Value>>,
}

#[async_trait]
impl HttpTransport for CatalogChangeModel {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        let request: serde_json::Value = serde_json::from_str(&request.body)?;
        let call = {
            let mut requests = self.requests.lock().unwrap();
            requests.push(request);
            requests.len()
        };
        let events = match call {
            1 => response_function_call(
                "audit-catalog-call",
                "mcp__catalog_audit__initial_tool",
                serde_json::json!({}),
            ),
            2 | 3 => response_text("Catalog audit turn completed."),
            _ => return Err(CoreError::other("unexpected catalog audit model request")),
        };
        emit_responses_events(sink, &events)
    }
}

#[tokio::test]
async fn unchanged_mcp_catalog_is_preserved_between_runs() {
    exercise_catalog_change(false).await;
}

#[tokio::test]
#[ignore = "audit: tools/list_changed must refresh the next run's MCP snapshot and schema"]
async fn audit_mcp_list_changed_reaches_next_run() {
    exercise_catalog_change(true).await;
}

fn advertised_catalog_tools(request: &serde_json::Value) -> Vec<String> {
    request["tools"]
        .as_array()
        .expect("the real provider request must contain tool schemas")
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .filter(|name| name.starts_with("mcp__catalog_audit__"))
        .map(str::to_owned)
        .collect()
}

async fn exercise_catalog_change(change: bool) {
    let root = tempfile::tempdir().unwrap();
    let script = root.path().join("catalog-change-mcp.cjs");
    let report = root.path().join("catalog-report.json");
    std::fs::write(
        &script,
        r#"
const fs = require('node:fs');
const readline = require('node:readline');
const change = process.env.AUDIT_CHANGE_CATALOG === '1';
const report = process.env.AUDIT_CATALOG_REPORT;
const state = {
  initialize_requests: 0, list_requests: 0, tool_call_names: [],
  notifications_sent: 0, current_tools: ['initial_tool']
};
function save() {
  fs.writeFileSync(report + '.tmp', JSON.stringify(state));
  fs.renameSync(report + '.tmp', report);
}
function reply(id, result) {
  process.stdout.write(JSON.stringify({ jsonrpc: '2.0', id, result }) + '\n');
}
readline.createInterface({ input: process.stdin }).on('line', line => {
  const request = JSON.parse(line);
  if (request.method === 'initialize') {
    state.initialize_requests++;
    save();
    reply(request.id, {
      protocolVersion: request.params.protocolVersion,
      capabilities: { tools: { listChanged: true } },
      serverInfo: { name: 'catalog-audit', version: '1' }
    });
  } else if (request.method === 'tools/list') {
    state.list_requests++;
    save();
    reply(request.id, { tools: state.current_tools.map(name => ({
      name, description: 'Catalog audit fixture tool',
      inputSchema: { type: 'object', properties: {} }
    })) });
  } else if (request.method === 'tools/call') {
    state.tool_call_names.push(request.params.name);
    if (change) {
      state.current_tools = ['updated_tool'];
      state.notifications_sent++;
      // The notification precedes the matching response on the same pipe.
      // Completing this call therefore exercises the production reader past it.
      process.stdout.write(JSON.stringify({
        jsonrpc: '2.0', method: 'notifications/tools/list_changed'
      }) + '\n');
    }
    save();
    reply(request.id, {
      content: [{ type: 'text', text: 'Catalog fixture call completed' }], isError: false
    });
  } else if (request.id !== undefined) reply(request.id, {});
});
process.stdin.on('end', () => process.exit(0));
setTimeout(() => process.exit(2), 15000).unref();
"#,
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
        "name": "catalog_audit", "transport": "stdio", "enabled": true,
        "command": "node", "args": [script],
        "env": {
            "AUDIT_CHANGE_CATALOG": if change { "1" } else { "0" },
            "AUDIT_CATALOG_REPORT": report
        }
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
    let initial_ready = mcp.ready_registry();
    if prepared.is_err() {
        initial_ready.close_all().await.unwrap();
        panic!("the catalog MCP fixture must be ready before the model run");
    }

    let model = Arc::new(CatalogChangeModel {
        requests: Mutex::new(Vec::new()),
    });
    let chat = ChatService::new(db, settings, model.clone(), root.path()).with_mcp(mcp.clone());
    let session = chat.create_session(None).unwrap();
    let mut outcomes = Vec::new();
    let mut ready_after_change = Vec::new();
    for (index, prompt) in [
        "Call the initial MCP tool.",
        "Inspect the current tool catalog.",
    ]
    .into_iter()
    .enumerate()
    {
        let outcome = tokio::time::timeout(
            Duration::from_secs(5),
            chat.run_in_session(
                prompt,
                Some(&session),
                None,
                None,
                Vec::new(),
                None,
                false,
                Some(&format!("audit-mcp-catalog-{index}")),
                |_| {},
                |_| {},
            ),
        )
        .await;
        let succeeded = matches!(&outcome, Ok(Ok(_)));
        outcomes.push(outcome);
        if !succeeded {
            break;
        }
        if index == 0 {
            if change {
                // This is a bounded local observation window, not an MCP
                // protocol deadline. A stable catalog needs no such wait.
                let _ = tokio::time::timeout(Duration::from_secs(1), async {
                    while mcp
                        .ready_registry()
                        .find_tool("mcp__catalog_audit__updated_tool")
                        .is_none()
                    {
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                })
                .await;
            }
            ready_after_change = mcp
                .ready_registry()
                .all_tools()
                .into_iter()
                .map(|tool| tool.namespaced_name)
                .collect();
        }
    }
    // Close the real child before any expected defect assertion can fail.
    initial_ready.close_all().await.unwrap();
    let server: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&report).unwrap()).unwrap();
    for outcome in outcomes {
        outcome.expect("the catalog audit run must finish").unwrap();
    }
    let requests = model.requests.lock().unwrap();
    assert_eq!(
        requests.len(),
        3,
        "two runs must complete across one MCP call"
    );
    let first_wire = advertised_catalog_tools(&requests[0]);
    let next_run_wire = advertised_catalog_tools(&requests[2]);
    eprintln!(
        "MCP catalog change audit: change={change}, server={server}, ready_after_change={ready_after_change:?}, first_wire={first_wire:?}, next_run_wire={next_run_wire:?}, model_requests={}",
        requests.len()
    );
    assert_eq!(server["initialize_requests"], 1);
    assert_eq!(
        server["tool_call_names"],
        serde_json::json!(["initial_tool"])
    );
    assert_eq!(server["notifications_sent"], usize::from(change));
    assert_eq!(first_wire, ["mcp__catalog_audit__initial_tool"]);
    let expected = if change {
        "updated_tool"
    } else {
        "initial_tool"
    };
    assert_eq!(server["current_tools"], serde_json::json!([expected]));
    assert_eq!(
        ready_after_change,
        [format!("mcp__catalog_audit__{expected}")],
        "the host must publish the server's updated catalog before the next run"
    );
    assert_eq!(
        next_run_wire,
        [format!("mcp__catalog_audit__{expected}")],
        "the next run's real provider schema must match the server's current tools"
    );
}
