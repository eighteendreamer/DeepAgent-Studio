//! H-147: real stdio identity -> registry -> wire -> raw call, plus kernel replay.
use super::*;
use deepagent_app_core::{McpServerDto, McpService};
use deepagent_builtins::ToolSearchMode;
use deepagent_core::message::Message;
use deepagent_hooks::PermissionRules;
use deepagent_mcp::{
    adapters_for, namespaced_name, McpClient, McpRegistry, McpServerConfig, StdioTransport,
};
use deepagent_models::{ModelClient, ModelConfig, ResponseRequest, ToolSchema, WireMode};
use deepagent_tools::{PermissionSet, ToolRegistry};
use serde_json::{json, Value};
use std::path::Path;
use std::time::Duration;

const SERVER: &str = "plugin__h147__synthetic.server";
const RAW: &str = "read.file";
const FIXTURE: &str = r#"
const fs = require('node:fs');
const tools = JSON.parse(process.env.H147_TOOLS);
const report = process.env.H147_REPORT;
let calls = [];
fs.writeFileSync(report, JSON.stringify(calls));
require('node:readline').createInterface({input:process.stdin}).on('line', line => {
  const r = JSON.parse(line);
  const reply = result => process.stdout.write(JSON.stringify({jsonrpc:'2.0',id:r.id,result})+'\n');
  if (r.method === 'initialize') reply({protocolVersion:r.params.protocolVersion,
    capabilities:{tools:{}},serverInfo:{name:'synthetic-name-test',version:'1'}});
  else if (r.method === 'tools/list') reply({tools:tools.map(name => ({name,
    description:'Inspect synthetic evidence',inputSchema:{type:'object',properties:{}}}))});
  else if (r.method === 'tools/call') {
    calls.push(r.params.name); fs.writeFileSync(report,JSON.stringify(calls));
    reply({content:[{type:'text',text:'H147_SYNTHETIC_RESULT'}],isError:false});
  } else if (r.id !== undefined) reply({});
});
process.stdin.on('end',()=>process.exit(0));
setTimeout(()=>process.exit(2),20000).unref();
"#;

fn config(root: &Path, tools: &[String]) -> McpServerConfig {
    let script = root.join("synthetic-mcp.cjs");
    std::fs::write(&script, FIXTURE).unwrap();
    serde_json::from_value(json!({"command":"node","args":[script],"env":{
        "H147_TOOLS":serde_json::to_string(tools).unwrap(),
        "H147_REPORT":root.join("raw-calls.json")
    }}))
    .unwrap()
}

fn calls(root: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(root.join("raw-calls.json")).unwrap()).unwrap()
}

fn valid(name: &str) -> bool {
    (1..=128).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

struct WireCapture(Mutex<Vec<Value>>);

#[async_trait]
impl HttpTransport for WireCapture {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        let sink: &mut dyn deepagent_models::transport::EventSink =
            &mut crate::sequence_fixture::sink(sink);
        let body: Value = serde_json::from_str(&request.body)?;
        let chat = body.get("messages").is_some();
        self.0.lock().unwrap().push(body);
        if chat {
            sink.on_event(
                r#"{"choices":[{"index":0,"delta":{"content":"ok"},"finish_reason":"stop"}]}"#,
            )?;
            sink.on_event("[DONE]")?;
            Ok(())
        } else {
            emit_responses_events(sink, &response_text("ok"))
        }
    }
}

#[tokio::test]
async fn h147_real_stdio_snapshot_reaches_both_wires_and_routes_only_raw_names() {
    let root = tempfile::tempdir().unwrap();
    let raw = vec![
        "normal".into(),
        "Normal".into(),
        "".into(),
        RAW.into(),
        "read_file".into(),
        "读取".into(),
        "x".repeat(200),
    ];
    let config = config(root.path(), &raw);
    let transport = Arc::new(StdioTransport::spawn(&config).unwrap());
    let client = Arc::new(McpClient::new(transport));
    client.initialize("h147").await.unwrap();
    let mut mcp = McpRegistry::new();
    mcp.register(SERVER, client).await.unwrap();
    let mcp = Arc::new(mcp);
    let mut tools = ToolRegistry::new();
    for adapter in adapters_for(mcp.clone()) {
        tools.register(adapter).unwrap();
    }
    let snapshot: Vec<_> = tools
        .visible_to(&PermissionSet::developer())
        .into_iter()
        .map(|d| ToolSchema::function(d.name, d.description, d.parameters))
        .collect();
    let capture = Arc::new(WireCapture(Mutex::new(Vec::new())));
    for mode in [WireMode::Responses, WireMode::ChatCompletions] {
        ModelClient::new(
            capture.clone(),
            ModelConfig::deepseek("placeholder").with_wire_mode(mode),
        )
        .stream_response(
            ResponseRequest::new("deepseek-flash", vec![Message::user("fixture")])
                .with_tools(snapshot.clone()),
        )
        .await
        .unwrap();
    }
    assert!(tools.visible_to(&PermissionSet::empty()).is_empty());
    for tool in mcp.all_tools() {
        assert!(tools
            .invoke(
                &tool.namespaced_name,
                json!({}),
                &PermissionSet::empty(),
                true
            )
            .await
            .is_err());
    }
    assert!(
        mcp.invoke(&format!("mcp__{SERVER}__{RAW}"), json!({}))
            .await
            .is_err(),
        "old ambiguous names must not be guessed or rerouted"
    );
    let denied_calls = calls(root.path());
    for tool in mcp.all_tools() {
        tools
            .invoke(
                &tool.namespaced_name,
                json!({}),
                &PermissionSet::developer(),
                false,
            )
            .await
            .unwrap();
    }
    mcp.close_all().await.unwrap();
    assert_eq!(
        denied_calls,
        json!([]),
        "name projection must not bypass permissions"
    );
    let requests = capture.0.lock().unwrap();
    for (index, request) in requests.iter().enumerate() {
        let names: Vec<_> = request["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| {
                if index == 0 {
                    t["name"].as_str().unwrap()
                } else {
                    t["function"]["name"].as_str().unwrap()
                }
            })
            .collect();
        assert_eq!(names.len(), raw.len());
        assert!(names.iter().all(|name| valid(name)), "{names:?}");
        assert_eq!(
            names
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len(),
            raw.len()
        );
    }
    let mut actual: Vec<String> = serde_json::from_value(calls(root.path())).unwrap();
    let mut expected = raw;
    actual.sort();
    expected.sort();
    assert_eq!(
        actual, expected,
        "tools/call must never receive a model alias"
    );
}

struct KernelModel {
    alias: String,
    discover: bool,
    requests: Mutex<Vec<Value>>,
}

#[async_trait]
impl HttpTransport for KernelModel {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        let sink: &mut dyn deepagent_models::transport::EventSink =
            &mut crate::sequence_fixture::sink(sink);
        let body: Value = serde_json::from_str(&request.body)?;
        let index = {
            let mut requests = self.requests.lock().unwrap();
            requests.push(body);
            requests.len()
        };
        let events = match (self.discover, index) {
            (true, 1) => response_function_call(
                "h147-search",
                "tool_search",
                json!({"query":format!("select:{}", self.alias)}),
            ),
            (true, 2) => response_function_call("h147-call", &self.alias, json!({})),
            _ => response_text("Synthetic identity test complete."),
        };
        emit_responses_events(sink, &events)
    }
}

async fn prepared(mcp: &Arc<McpService>) {
    mcp.start_preparation();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let statuses = mcp.connection_status().await.unwrap();
            if statuses.iter().any(|s| s.status == "failed") {
                panic!("{statuses:?}");
            }
            if mcp.ready_registry().server_count() == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn h147_alias_is_shared_by_discovery_approval_history_and_reopened_kernel() {
    let root = tempfile::tempdir().unwrap();
    let database = root.path().join("synthetic.sqlite");
    let config = config(root.path(), &[RAW.into(), "other".into()]);
    let alias = namespaced_name(SERVER, RAW);
    let secrets = Arc::new(MemorySecretStore::new());
    let session;
    let first_tools;
    {
        let db = Arc::new(Database::open(&database).unwrap());
        let settings = Arc::new(SettingsService::new(
            db.clone(),
            Arc::new(MockTransport::with_get_json(
                r#"{"data":[{"id":"deepseek-flash","object":"model","owned_by":"deepseek"}]}"#,
            )),
            secrets.clone(),
        ));
        settings.initialize("offline-placeholder").await.unwrap();
        settings
            .set_tool_search_mode(ToolSearchMode::Enabled)
            .unwrap();
        settings
            .set_active_permission_preset(PermissionPreset::Default)
            .unwrap();
        settings
            .set_permission_rules(PermissionRules::new([], [alias.clone()], []))
            .unwrap();
        let mcp = Arc::new(McpService::new(db.clone()));
        mcp.upsert(serde_json::from_value::<McpServerDto>(json!({"name":SERVER,"enabled":true,"transport":"stdio","command":config.command,"args":config.args,"env":config.env})).unwrap()).unwrap();
        prepared(&mcp).await;
        let model = Arc::new(KernelModel {
            alias: alias.clone(),
            discover: true,
            requests: Mutex::new(Vec::new()),
        });
        let chat = ChatService::new(db, settings, model.clone(), root.path()).with_mcp(mcp.clone());
        session = chat.create_session(None).unwrap();
        let approvals = Arc::new(Mutex::new(Vec::new()));
        let approved = approvals.clone();
        let approval_chat = chat.clone();
        let result = tokio::time::timeout(
            Duration::from_secs(8),
            chat.run_in_session(
                "Inspect synthetic evidence",
                Some(&session),
                None,
                None,
                Vec::new(),
                None,
                false,
                Some("h147-first"),
                |_| {},
                move |request| {
                    approved.lock().unwrap().push(request.tool.clone());
                    assert!(approval_chat
                        .resolve_approval(&request.call_id, true, "synthetic-user")
                        .unwrap());
                },
            ),
        )
        .await;
        mcp.ready_registry().close_all().await.unwrap();
        result.unwrap().unwrap();
        assert_eq!(*approvals.lock().unwrap(), vec![alias.clone()]);
        assert_eq!(calls(root.path()), json!([RAW]));
        assert_eq!(chat.discovered_tool_names(&session), vec![alias.clone()]);
        let requests = model.requests.lock().unwrap();
        assert_eq!(requests.len(), 3);
        assert!(!requests[0]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["name"] == alias));
        first_tools = requests[1]["tools"].clone();
        assert!(first_tools
            .as_array()
            .unwrap()
            .iter()
            .all(|t| valid(t["name"].as_str().unwrap())));
        let history = requests[2]["input"].as_array().unwrap();
        assert_eq!(
            history
                .iter()
                .find(|i| i["call_id"] == "h147-call" && i["type"] == "function_call")
                .unwrap()["name"],
            alias
        );
    }
    let db = Arc::new(Database::open(&database).unwrap());
    let settings = Arc::new(SettingsService::new(
        db.clone(),
        Arc::new(MockTransport::new(Vec::new())),
        secrets,
    ));
    let mcp = Arc::new(McpService::new(db.clone()));
    prepared(&mcp).await;
    let model = Arc::new(KernelModel {
        alias: alias.clone(),
        discover: false,
        requests: Mutex::new(Vec::new()),
    });
    let chat = ChatService::new(db, settings, model.clone(), root.path()).with_mcp(mcp.clone());
    let result = chat
        .run_in_session(
            "Continue synthetic history",
            Some(&session),
            None,
            None,
            Vec::new(),
            None,
            false,
            Some("h147-reopen"),
            |_| {},
            |_| panic!("reopen must not redispatch an old call"),
        )
        .await;
    // Validate the catalog advertised by this run, not live discovery after
    // teardown. H-48 retires closed connections at the next request boundary.
    let advertised = mcp.ready_registry().discovery_snapshot();
    mcp.ready_registry().close_all().await.unwrap();
    result.unwrap();
    assert_eq!(calls(root.path()), json!([]));
    assert_eq!(
        advertised
            .all_tools()
            .iter()
            .find(|t| t.local_name == RAW)
            .unwrap()
            .namespaced_name,
        alias
    );
    let requests = model.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0]["tools"], first_tools);
    assert_eq!(
        requests[0]["input"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|i| i["call_id"] == "h147-call"
                && i["type"] == "function_call"
                && i["name"] == alias)
            .count(),
        1
    );
    for run in ["h147-first", "h147-reopen"] {
        let events = chat.run_events(run, None).unwrap();
        assert_eq!(
            events
                .iter()
                .filter(|e| e.event_type == "run_terminal" && e.data["kind"] == "succeeded")
                .count(),
            1
        );
    }
}

#[tokio::test]
async fn h147_duplicate_raw_names_fail_management_without_publishing_partial_capabilities() {
    let root = tempfile::tempdir().unwrap();
    let db = Arc::new(Database::open_in_memory().unwrap());
    let mcp = Arc::new(McpService::new(db));
    for (name, raw) in [
        ("healthy", vec!["inspect".into()]),
        ("ambiguous", vec![RAW.into(), RAW.into()]),
    ] {
        let directory = root.path().join(name);
        std::fs::create_dir(&directory).unwrap();
        let config = config(&directory, &raw);
        mcp.upsert(
            serde_json::from_value::<McpServerDto>(json!({"name":name,"enabled":true,
            "transport":"stdio","command":config.command,"args":config.args,"env":config.env}))
            .unwrap(),
        )
        .unwrap();
    }
    mcp.start_preparation();
    let result = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let statuses = mcp.connection_status().await.unwrap();
            if statuses.len() == 2 && statuses.iter().all(|s| s.status != "preparing") {
                break statuses;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    let advertised = mcp.ready_registry().discovery_snapshot();
    mcp.ready_registry().close_all().await.unwrap();
    let statuses = result.unwrap();
    let failed = statuses.iter().find(|s| s.name == "ambiguous").unwrap();
    assert_eq!(failed.status, "failed");
    assert!(failed
        .error
        .as_ref()
        .unwrap()
        .contains("duplicate raw tool identity"));
    assert!(failed.tools.is_empty());
    assert_eq!(mcp.ready_registry().server_names(), vec!["healthy"]);
    assert_eq!(advertised.all_tools().len(), 1);
    assert!(mcp.ready_registry().all_tools().is_empty());
    assert_eq!(calls(&root.path().join("ambiguous")), json!([]));
    assert_eq!(calls(&root.path().join("healthy")), json!([]));
}
