//! MCP initialization must be validated before publishing a callable Ready snapshot.

use super::responses_fixture::{call, completed, text};
use super::*;
use deepagent_app_core::{McpServerDto, McpService};
use deepagent_builtins::ToolSearchMode;
use deepagent_persistence::run_store::RunStore;
use serde_json::{json, Value};
use std::time::Duration;

const MODEL: &str = "deepseek-flash";
const RUN: &str = "audit-mcp-initialize";
const TOOL: &str = "mcp__initialize_audit__inspect";
const MARKER: &str = "SYNTHETIC_MCP_TOOL_AFTER_INITIALIZE";
const SERVER: &str = r#"
const fs = require('node:fs');
const readline = require('node:readline');
const state = { initialize:0, lists:0, calls:0 };
const mode = process.env.AUDIT_MODE;
function save() {
  fs.writeFileSync(process.env.AUDIT_REPORT + '.tmp', JSON.stringify(state));
  fs.renameSync(process.env.AUDIT_REPORT + '.tmp', process.env.AUDIT_REPORT);
}
function reply(id, result) {
  process.stdout.write(JSON.stringify({jsonrpc:'2.0', id, result}) + '\n');
}
readline.createInterface({input:process.stdin}).on('line', line => {
  const req = JSON.parse(line);
  if (req.method === 'initialize') {
    state.initialize++;
    state.requestedVersion = req.params.protocolVersion;
    if (mode === 'error') {
      save();
      process.stdout.write(JSON.stringify({jsonrpc:'2.0', id:req.id,
        error:{code:-32602,message:'synthetic handshake rejection'}}) + '\n');
      return;
    }
    let result = {protocolVersion:req.params.protocolVersion, capabilities:{tools:{}},
      serverInfo:{name:'initialize-contract-fixture',version:'1'}};
    if (mode === 'empty') result = {};
    if (mode === 'missing-version') delete result.protocolVersion;
    if (mode === 'missing-capabilities') delete result.capabilities;
    if (mode === 'missing-info') delete result.serverInfo;
    if (mode === 'wrong-version-type') result.protocolVersion = 42;
    if (mode === 'unsupported') result.protocolVersion = '2099-01-01';
    state.initializeResult = result;
    save();
    reply(req.id, result);
  } else if (req.method === 'tools/list') {
    state.lists++;
    save();
    // Do not require initialized here: this probe isolates validation from H-08.
    reply(req.id, {tools:[{name:'inspect',description:'Synthetic handshake audit',
      inputSchema:{type:'object',properties:{},additionalProperties:false}}]});
  } else if (req.method === 'tools/call') {
    state.calls++;
    state.toolName = req.params.name;
    save();
    reply(req.id, {content:[{type:'text',text:'SYNTHETIC_MCP_TOOL_AFTER_INITIALIZE'}],
      isError:false});
  } else if (req.id !== undefined) reply(req.id, {});
});
process.stdin.on('end', () => process.exit(0));
setTimeout(() => process.exit(2), 15000).unref();
"#;

struct ParentModel {
    requests: Mutex<Vec<Value>>,
}

#[async_trait]
impl HttpTransport for ParentModel {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        let body: Value = serde_json::from_str(&request.body)?;
        assert_eq!(body["model"], MODEL);
        let index = {
            let mut requests = self.requests.lock().unwrap();
            requests.push(body.clone());
            requests.len() - 1
        };
        let advertised = body["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|tool| tool["name"] == TOOL);
        let item = if index == 0 && advertised {
            call("initialize-tool-call", TOOL, json!({}))
        } else {
            text("initialize-final", "Offline initialization audit complete.")
        };
        assert!(index < 2);
        emit_responses_events(
            sink,
            &completed(
                &format!("initialize-response-{index}"),
                MODEL,
                item,
                json!({"input_tokens":12,"output_tokens":6,"total_tokens":18}),
            ),
        )
    }
}

#[derive(Debug)]
struct Observation {
    mode: &'static str,
    probe_status: String,
    host_status: String,
    ready_servers: usize,
    advertised: bool,
    lists: u64,
    calls: u64,
    model_received_result: bool,
}

async fn observe(mode: &'static str) -> Observation {
    let root = tempfile::tempdir().unwrap();
    let script = root.path().join("initialize-contract-mcp.cjs");
    let report_path = root.path().join("report.json");
    std::fs::write(&script, SERVER).unwrap();
    let db = Arc::new(Database::open(root.path().join("state.sqlite")).unwrap());
    let settings = Arc::new(SettingsService::new(
        db.clone(),
        Arc::new(MockTransport::with_get_json(
            r#"{"data":[{"id":"deepseek-flash","object":"model","owned_by":"deepseek"}]}"#,
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
    let dto: McpServerDto = serde_json::from_value(json!({
        "name":"initialize_audit","transport":"stdio","enabled":true,
        "command":"node","args":[script],"env":{
            "AUDIT_MODE":mode,"AUDIT_REPORT":report_path
        }
    }))
    .unwrap();
    // The formal settings-page probe owns and closes its separate process.
    let probe = mcp.test_server(dto.clone()).await.unwrap();
    let probe_report: Value =
        serde_json::from_str(&std::fs::read_to_string(&report_path).unwrap()).unwrap();
    assert_eq!(probe_report["initialize"], 1);
    assert_eq!(probe_report["calls"], 0);
    mcp.upsert(dto).unwrap();
    mcp.start_preparation();
    let published = tokio::time::timeout(Duration::from_secs(4), async {
        loop {
            let statuses = mcp.connection_status().await.unwrap();
            assert_eq!(statuses.len(), 1);
            if statuses[0].status != "preparing" {
                break statuses.into_iter().next().unwrap();
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await;
    let ready = mcp.ready_registry();
    let status = match published {
        Ok(status) => status,
        Err(_) => {
            ready.close_all().await.unwrap();
            panic!("host preparation must publish a bounded startup outcome");
        }
    };
    let model = Arc::new(ParentModel {
        requests: Mutex::new(Vec::new()),
    });
    let chat = ChatService::new(db.clone(), settings, model.clone(), root.path()).with_mcp(mcp);
    let run = tokio::time::timeout(
        Duration::from_secs(8),
        chat.run_in_session(
            "Use the synthetic MCP inspect tool if it is available, otherwise reply briefly.",
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
    run.expect("parent run must settle").unwrap();
    let report: Value =
        serde_json::from_str(&std::fs::read_to_string(report_path).unwrap()).unwrap();
    assert_eq!(report["initialize"], 1);
    assert_eq!(
        report["requestedVersion"],
        deepagent_mcp::protocol::MCP_PROTOCOL_VERSION
    );
    if mode != "error" {
        assert_eq!(report["initializeResult"], probe_report["initializeResult"]);
    }
    assert_eq!(
        RunStore::new(&db)
            .get(RUN)
            .unwrap()
            .unwrap()
            .terminal_kind
            .as_deref(),
        Some("succeeded")
    );
    assert_eq!(
        chat.run_events(RUN, None)
            .unwrap()
            .iter()
            .filter(|event| event.event_type == "run_terminal")
            .count(),
        1
    );
    let requests = model.requests.lock().unwrap();
    let advertised = requests[0]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .any(|tool| tool["name"] == TOOL);
    assert_eq!(requests.len(), if advertised { 2 } else { 1 });
    let mut model_received_result = false;
    if advertised {
        let outputs: Vec<_> = requests[1]["input"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|item| {
                item["type"] == "function_call_output" && item["call_id"] == "initialize-tool-call"
            })
            .collect();
        assert_eq!(outputs.len(), 1);
        let output: Value = serde_json::from_str(outputs[0]["output"].as_str().unwrap()).unwrap();
        assert_eq!(output["status"], "ok");
        model_received_result = output.to_string().contains(MARKER);
        assert_eq!(report["toolName"], "inspect");
    }
    let observed = Observation {
        mode,
        probe_status: probe.status,
        host_status: status.status,
        ready_servers: ready.server_count(),
        advertised,
        lists: report["lists"].as_u64().unwrap(),
        calls: report["calls"].as_u64().unwrap(),
        model_received_result,
    };
    eprintln!("MCP initialization contract: {observed:?}");
    assert_eq!(observed.mode, mode);
    observed
}

#[tokio::test]
async fn valid_mcp_initialize_publishes_and_executes_the_tool() {
    let observed = observe("valid").await;
    assert_eq!(observed.probe_status, "connected");
    assert_eq!(observed.host_status, "connected");
    assert_eq!(observed.ready_servers, 1);
    assert_eq!(observed.lists, 1);
    assert_eq!(observed.calls, 1);
    assert!(observed.advertised && observed.model_received_result);
}

#[tokio::test]
async fn explicit_mcp_initialize_error_is_contained_before_discovery() {
    let observed = observe("error").await;
    assert_eq!(observed.probe_status, "failed");
    assert_eq!(observed.host_status, "failed");
    assert_eq!(observed.ready_servers, 0);
    assert_eq!(observed.lists, 0);
    assert_eq!(observed.calls, 0);
    assert!(!observed.advertised && !observed.model_received_result);
}

#[tokio::test]
async fn audit_invalid_mcp_initialize_results_are_contained_before_discovery() {
    let mut observations = Vec::new();
    for mode in [
        "empty",
        "missing-version",
        "missing-capabilities",
        "missing-info",
        "wrong-version-type",
        "unsupported",
    ] {
        observations.push(observe(mode).await);
    }
    assert!(
        observations.iter().all(|o| o.probe_status == "failed"
            && o.host_status == "failed"
            && o.ready_servers == 0
            && o.lists == 0
            && o.calls == 0
            && !o.advertised
            && !o.model_received_result),
        "initialization must gate readiness and dispatch: {observations:?}"
    );
}
