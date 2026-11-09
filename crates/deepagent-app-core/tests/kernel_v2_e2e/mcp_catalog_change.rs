//! A server's tool-list change should reach the next run's ready snapshot and wire schema.

use super::*;
use deepagent_app_core::{McpServerDto, McpService};
use deepagent_builtins::ToolSearchMode;
use std::time::Duration;

struct CatalogChangeModel {
    requests: Mutex<Vec<serde_json::Value>>,
    next_call: Option<(&'static str, serde_json::Value)>,
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
            3 if self.next_call.is_some() => {
                let (name, arguments) = self.next_call.as_ref().unwrap();
                response_function_call("h137-next-call", name, arguments.clone())
            }
            2..=4 => response_text("Catalog audit turn completed."),
            _ => return Err(CoreError::other("unexpected catalog audit model request")),
        };
        emit_responses_events(sink, &events)
    }
}

#[tokio::test]
async fn unchanged_mcp_catalog_is_preserved_between_runs() {
    exercise_catalog_change("unchanged").await;
}

#[tokio::test]
async fn audit_mcp_list_changed_reaches_next_run() {
    exercise_catalog_change("replace").await;
}

#[tokio::test]
async fn h137_catalog_add_remove_and_schema_updates_share_the_actual_dispatch_snapshot() {
    for mode in ["add", "remove", "schema"] {
        exercise_catalog_change(mode).await;
    }
}

#[tokio::test]
async fn h137_current_run_keeps_its_frozen_schema_after_host_catalog_publication() {
    exercise_catalog_change("inflight").await;
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

async fn exercise_catalog_change(mode: &str) {
    let change = mode != "unchanged";
    let expected_tools: Vec<&str> = match mode {
        "replace" | "inflight" => vec!["updated_tool"],
        "add" => vec!["initial_tool", "added_tool"],
        "remove" => Vec::new(),
        _ => vec!["initial_tool"],
    };
    let next_call = match mode {
        "replace" | "inflight" => Some((
            "mcp__catalog_audit__updated_tool",
            serde_json::json!({"value":7}),
        )),
        "add" => Some((
            "mcp__catalog_audit__added_tool",
            serde_json::json!({"value":7}),
        )),
        "schema" => Some((
            "mcp__catalog_audit__initial_tool",
            serde_json::json!({"value":7}),
        )),
        "unchanged" => Some(("mcp__catalog_audit__initial_tool", serde_json::json!({}))),
        _ => None,
    };
    let root = tempfile::tempdir().unwrap();
    let script = root.path().join("catalog-change-mcp.cjs");
    let report = root.path().join("catalog-report.json");
    std::fs::write(
        &script,
        r#"
const fs = require('node:fs');
const readline = require('node:readline');
const mode = process.env.AUDIT_CHANGE_CATALOG;
const change = mode !== 'unchanged';
const report = process.env.AUDIT_CATALOG_REPORT;
const state = {
  initialize_requests: 0, list_requests: 0, tool_call_names: [],
  notifications_sent: 0, current_tools: ['initial_tool'], changed: false,
  tool_arguments: []
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
      inputSchema: state.changed && (mode === 'schema' || name !== 'initial_tool')
        ? {type:'object', required:['value'], properties:{value:{type:'integer'}}, additionalProperties:false}
        : {type:'object', properties:{}, additionalProperties:false}
    })) });
  } else if (request.method === 'tools/call') {
    state.tool_call_names.push(request.params.name);
    state.tool_arguments.push(request.params.arguments);
    if (!state.current_tools.includes(request.params.name)) {
      save();
      process.stdout.write(JSON.stringify({jsonrpc:'2.0',id:request.id,error:{code:-32602,message:'unknown fixture tool'}})+'\n');
      return;
    }
    if (change && !state.changed) {
      state.changed = true;
      state.current_tools = (mode === 'replace'||mode==='inflight') ? ['updated_tool']
        : mode === 'add' ? ['initial_tool','added_tool'] : mode === 'remove' ? [] : ['initial_tool'];
      state.notifications_sent++;
      // The notification precedes the matching response on the same pipe.
      // Completing this call therefore exercises the production reader past it.
      process.stdout.write(JSON.stringify({
        jsonrpc: '2.0', method: 'notifications/tools/list_changed'
      }) + '\n');
    }
    save();
    const finish=()=>reply(request.id, {
      content: [{ type: 'text', text: 'Catalog fixture call completed' }], isError: false
    });
    if(mode==='inflight'&&state.tool_call_names.length===1){
      // Only the host observer writes this synthetic marker, after publishing
      // the new catalog. No timing guess about an async task winning the race.
      const gate=setInterval(()=>{if(!fs.existsSync(report+'.release'))return;
        clearInterval(gate);finish();},5);
    }else finish();
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
            "AUDIT_CHANGE_CATALOG": mode,
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
        next_call: next_call.clone(),
    });
    let chat = ChatService::new(db, settings, model.clone(), root.path()).with_mcp(mcp.clone());
    let session = chat.create_session(None).unwrap();
    let mut outcomes = Vec::new();
    let mut ready_after_change = Vec::new();
    let mut prior_config: Option<serde_json::Value> = None;
    for (index, prompt) in [
        "Call the initial MCP tool.",
        "Inspect the current tool catalog.",
    ]
    .into_iter()
    .enumerate()
    {
        let overrides = deepagent_app_core::HarnessRunOverrides::default();
        let config = chat.read_execution_config(&overrides).unwrap();
        if let Some(prior) = &prior_config {
            if change {
                assert_ne!(prior["mcpGeneration"], config["mcpGeneration"]);
                let stale = chat.admit_prepared_input_with_config(
                    "h137-stale-admission",
                    &session,
                    prompt,
                    overrides.clone(),
                    prior["configSnapshotId"].as_str(),
                );
                assert!(
                    matches!(stale, Err(CoreError::ExecutionConfigStale)),
                    "a superseded catalog cannot admit the previous config identity"
                );
            } else {
                assert_eq!(prior["configSnapshotId"], config["configSnapshotId"]);
            }
        }
        let prepared = chat
            .admit_prepared_input_with_config(
                &format!("audit-mcp-catalog-{index}"),
                &session,
                prompt,
                overrides,
                config["configSnapshotId"].as_str(),
            )
            .expect("config/read and actual frozen runtime must use the same catalog identity");
        prior_config = Some(config);
        let run = chat.run_prepared(prepared, prompt, |_| {}, |_| {});
        let outcome = tokio::time::timeout(Duration::from_secs(5), async {
            if mode == "inflight" && index == 0 {
                let observer = async {
                    while mcp
                        .ready_registry()
                        .find_tool("mcp__catalog_audit__updated_tool")
                        .is_none()
                    {
                        tokio::time::sleep(Duration::from_millis(5)).await;
                    }
                    std::fs::write(report.with_extension("json.release"), "host published")
                        .unwrap();
                };
                let (result, ()) = tokio::join!(run, observer);
                result
            } else {
                run.await
            }
        })
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
                        .all_tools()
                        .iter()
                        .map(|tool| tool.local_name.as_str())
                        .collect::<Vec<_>>()
                        != expected_tools
                        || (mode == "schema"
                            && mcp.ready_registry().all_tools()[0]
                                .input_schema
                                .get("required")
                                .is_none())
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
    // Snapshot the metadata while the peer is still discoverable. Closing the
    // shared client must hide tools even on a borrowed host view (H-48); that
    // lifecycle gate is not a catalog mutation and must not be weakened.
    let initial_tools_before_close = initial_ready.all_tools();
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
        if next_call.is_some() { 4 } else { 3 },
        "two runs must complete across one MCP call"
    );
    let first_wire = advertised_catalog_tools(&requests[0]);
    let mut next_run_wire = advertised_catalog_tools(&requests[2]);
    next_run_wire.sort();
    ready_after_change.sort();
    eprintln!(
        "MCP catalog change audit: change={change}, server={server}, ready_after_change={ready_after_change:?}, first_wire={first_wire:?}, next_run_wire={next_run_wire:?}, model_requests={}",
        requests.len()
    );
    assert_eq!(server["initialize_requests"], 1);
    assert_eq!(
        server["tool_call_names"],
        if let Some((name, _)) = &next_call {
            serde_json::json!([
                "initial_tool",
                name.strip_prefix("mcp__catalog_audit__").unwrap()
            ])
        } else {
            serde_json::json!(["initial_tool"])
        }
    );
    assert_eq!(server["notifications_sent"], usize::from(change));
    assert_eq!(first_wire, ["mcp__catalog_audit__initial_tool"]);
    assert_eq!(
        advertised_catalog_tools(&requests[1]),
        first_wire,
        "the already-running turn must not switch its schema after host publication"
    );
    assert_eq!(server["current_tools"], serde_json::json!(expected_tools));
    let mut expected_names: Vec<_> = expected_tools
        .iter()
        .map(|name| format!("mcp__catalog_audit__{name}"))
        .collect();
    expected_names.sort();
    assert_eq!(
        initial_tools_before_close.len(),
        1,
        "borrowed host catalog must not gain or lose entries"
    );
    assert_eq!(
        initial_tools_before_close[0].local_name, "initial_tool",
        "previously borrowed host snapshots remain immutable"
    );
    assert!(initial_tools_before_close[0]
        .input_schema
        .get("required")
        .is_none());
    assert_eq!(
        ready_after_change, expected_names,
        "the host must publish the server's updated catalog before the next run"
    );
    assert_eq!(
        next_run_wire, expected_names,
        "the next run's real provider schema must match the server's current tools"
    );
    if let Some((name, args)) = next_call {
        assert_eq!(
            server["tool_arguments"][1], args,
            "new route must actually execute with the newly admitted arguments"
        );
        let events = chat.run_events("audit-mcp-catalog-1", None).unwrap();
        assert!(
            events
                .iter()
                .any(|event| event.event_type == "tool_completed"
                    && event.data["call_id"] == "h137-next-call"
                    && event.data["ok"] == true),
            "{name} must not only be advertised; it must succeed through the actual pipeline"
        );
        if change {
            let schema = requests[2]["tools"]
                .as_array()
                .unwrap()
                .iter()
                .find(|tool| tool["name"] == name)
                .unwrap();
            assert_eq!(
                schema["parameters"]["required"],
                serde_json::json!(["value"])
            );
        }
    }
}
