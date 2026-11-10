//! Real stdio MCP discovery must survive clean service/database reconstruction.

use super::*;
use deepagent_app_core::{AppService, McpServerDto, McpService};
use deepagent_builtins::ToolSearchMode;
use deepagent_core::{clock::FixedClock, event::EventPayload, message::Message};
use deepagent_persistence::event_store::EventStore;
use deepagent_session::Session;
use std::path::Path;
use std::time::Duration;

const INSPECT: &str = "mcp__discovery_audit__inspect";
const OTHER: &str = "mcp__discovery_audit__other";
const CANARY: &str = "AUDIT_DISCOVERY_CANARY";

#[derive(Clone, Copy)]
enum DiscoveryStep {
    Search(&'static str),
    NativeSearch(&'static str),
    Inspect(&'static str),
    Finish,
}

struct DiscoveryModel {
    steps: Vec<DiscoveryStep>,
    requests: Mutex<Vec<serde_json::Value>>,
}

impl Default for DiscoveryModel {
    fn default() -> Self {
        Self {
            steps: vec![DiscoveryStep::Finish; 2],
            requests: Mutex::new(Vec::new()),
        }
    }
}

impl DiscoveryModel {
    fn discovering(tail: impl IntoIterator<Item = DiscoveryStep>) -> Self {
        Self {
            steps: [
                DiscoveryStep::Search("discovery-search"),
                DiscoveryStep::Inspect("discovery-inspect"),
                DiscoveryStep::Finish,
            ]
            .into_iter()
            .chain(tail)
            .collect(),
            requests: Mutex::new(Vec::new()),
        }
    }
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
        let step = self
            .steps
            .get(attempt - 1)
            .ok_or_else(|| CoreError::other("unexpected discovery audit request"))?;
        let id = format!("discovery-item-{attempt}");
        let item = match step {
            DiscoveryStep::NativeSearch(call_id) => serde_json::json!({
                "type":"tool_search_call", "id":id, "call_id":call_id,
                "execution":"client", "status":"completed",
                "arguments":{"query":format!("select:{INSPECT}"), "max_results":1}
            }),
            DiscoveryStep::Search(call_id) => serde_json::json!({
                "type":"function_call", "id":id, "call_id":call_id,
                "name":"tool_search", "status":"completed",
                "arguments":serde_json::json!({"query":format!("select:{INSPECT}")}).to_string()
            }),
            DiscoveryStep::Inspect(call_id) => serde_json::json!({
                "type":"function_call", "id":id, "call_id":call_id,
                "name":INSPECT, "status":"completed",
                "arguments":serde_json::json!({"value":CANARY}).to_string()
            }),
            DiscoveryStep::Finish => serde_json::json!({
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
                "response":{"created_at":1,"id":format!("discovery-response-{attempt}"),
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

fn assert_replayed_call(request: &serde_json::Value, call_id: &str) {
    let input = request["input"].as_array().unwrap();
    let calls: Vec<_> = input
        .iter()
        .filter(|item| item["type"] == "function_call" && item["call_id"] == call_id)
        .collect();
    let outputs: Vec<_> = input
        .iter()
        .filter(|item| item["type"] == "function_call_output" && item["call_id"] == call_id)
        .collect();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0]["name"], INSPECT);
    assert_eq!(outputs.len(), 1);
    assert!(outputs[0]["output"].to_string().contains(CANARY));
}

fn report(path: &Path) -> serde_json::Value {
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

fn fixture(root: &Path) -> (PathBuf, PathBuf) {
    let script = root.join("discovery-mcp.cjs");
    let report_path = root.join("discovery-report.json");
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
    (script, report_path)
}

fn register_fixture(mcp: &McpService, script: &Path, report_path: &Path) {
    mcp.upsert(
        serde_json::from_value::<McpServerDto>(serde_json::json!({
            "name":"discovery_audit", "transport":"stdio", "enabled":true,
            "command":"node", "args":[script],
            "env":{"AUDIT_DISCOVERY_REPORT":report_path}
        }))
        .unwrap(),
    )
    .unwrap();
}

#[tokio::test]
async fn h219_prepared_mcp_config_generation_is_frozen_and_disabled_servers_leave_next_run() {
    check_prepared_mcp_config_generation(false).await;
}

#[tokio::test]
async fn h219_mcp_environment_change_has_an_opaque_generation_without_logging_values() {
    check_prepared_mcp_config_generation(true).await;
}

async fn check_prepared_mcp_config_generation(environment_only: bool) {
    let root = tempfile::tempdir().unwrap();
    let first_root = root.path().join("first");
    let second_root = root.path().join("second");
    std::fs::create_dir_all(&first_root).unwrap();
    std::fs::create_dir_all(&second_root).unwrap();
    let (first_script, first_report) = fixture(&first_root);
    let (second_script, second_report) = fixture(&second_root);
    let db = Arc::new(Database::open_in_memory().unwrap());
    let settings = settings(&db, &Arc::new(MemorySecretStore::new()));
    settings
        .initialize("offline-h219-placeholder")
        .await
        .unwrap();
    settings
        .set_active_permission_preset(PermissionPreset::FullAccess)
        .unwrap();
    settings
        .set_tool_search_mode(ToolSearchMode::Disabled)
        .unwrap();
    let mcp = Arc::new(McpService::new(db.clone()));
    register_fixture(&mcp, &first_script, &first_report);
    prepare(&mcp).await;
    let old_registry = mcp.ready_registry();
    let model = Arc::new(DiscoveryModel {
        steps: vec![
            DiscoveryStep::Inspect("h219-mcp-old"),
            DiscoveryStep::Finish,
            DiscoveryStep::Inspect("h219-mcp-new"),
            DiscoveryStep::Finish,
        ],
        requests: Mutex::new(Vec::new()),
    });
    let chat =
        ChatService::new(db.clone(), settings, model.clone(), root.path()).with_mcp(mcp.clone());
    let session = chat.create_session(None).unwrap();
    let overrides = || deepagent_app_core::HarnessRunOverrides {
        sandbox_backend: Some("direct".into()),
        ..Default::default()
    };
    let first = chat
        .admit_prepared_run("h219-mcp-first", &session, overrides())
        .unwrap();
    let first_snapshot = first.capability_snapshot().clone();
    // Identical tools, a different real stdio process/configuration. The
    // prepared run must still call its original client, not the new service.
    register_fixture(
        &mcp,
        if environment_only {
            &first_script
        } else {
            &second_script
        },
        &second_report,
    );
    prepare(&mcp).await;
    let second_registry = mcp.ready_registry();
    chat.run_prepared(first, "Inspect the synthetic value.", |_| {}, |_| {})
        .await
        .unwrap();
    let second = chat
        .admit_prepared_run("h219-mcp-second", &session, overrides())
        .unwrap();
    let second_snapshot = second.capability_snapshot().clone();
    chat.run_prepared(second, "Inspect the synthetic value again.", |_| {}, |_| {})
        .await
        .unwrap();
    mcp.set_enabled("discovery_audit", false).unwrap();
    mcp.start_preparation();
    let disabled = chat
        .admit_prepared_run("h219-mcp-disabled", &session, overrides())
        .unwrap();
    let disabled_snapshot = disabled.capability_snapshot().clone();
    chat.request_cancel("h219-mcp-disabled").unwrap();
    chat.run_prepared(disabled, "No dispatch after cancellation.", |_| {}, |_| {})
        .await
        .unwrap();
    mcp.upsert(
        serde_json::from_value::<McpServerDto>(serde_json::json!({
            "name":"discovery_audit", "transport":"stdio", "enabled":true,
            "command":"h219-synthetic-missing-executable", "args":[]
        }))
        .unwrap(),
    )
    .unwrap();
    mcp.start_preparation();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if mcp
                .connection_status()
                .await
                .unwrap()
                .iter()
                .any(|entry| entry.name == "discovery_audit" && entry.status == "failed")
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the synthetic missing executable must publish a failed startup");
    let failed = chat
        .admit_prepared_run("h219-mcp-failed", &session, overrides())
        .unwrap();
    let failed_snapshot = failed.capability_snapshot().clone();
    chat.request_cancel("h219-mcp-failed").unwrap();
    chat.run_prepared(
        failed,
        "No model or tools after cancellation.",
        |_| {},
        |_| {},
    )
    .await
    .unwrap();
    old_registry.close_all().await.unwrap();
    second_registry.close_all().await.unwrap();
    mcp.ready_registry().close_all().await.unwrap();
    assert_eq!(report(&first_report)["calls"].as_array().unwrap().len(), 1);
    assert_eq!(report(&second_report)["calls"].as_array().unwrap().len(), 1);
    assert_eq!(model.requests.lock().unwrap().len(), 4);
    assert_eq!(
        failed_snapshot["context"]["mcpSchemas"],
        serde_json::json!([])
    );
    assert!(!failed_snapshot["tools"]
        .as_array()
        .unwrap()
        .iter()
        .any(|tool| tool["name"] == INSPECT));
    assert!(!failed_snapshot
        .to_string()
        .contains("h219-synthetic-missing-executable"));
    assert_eq!(
        first_snapshot["toolSchemaHash"],
        second_snapshot["toolSchemaHash"]
    );
    let schemas = |snapshot: &serde_json::Value| snapshot["context"]["mcpSchemas"][0].clone();
    assert!(
        schemas(&first_snapshot)["configHash"].is_string(),
        "host MCP metadata must identify the actual admitted configuration"
    );
    if environment_only {
        assert_eq!(
            schemas(&first_snapshot)["configHash"],
            schemas(&second_snapshot)["configHash"],
            "configHash deliberately excludes environment/header values"
        );
    } else {
        assert_ne!(
            schemas(&first_snapshot)["configHash"],
            schemas(&second_snapshot)["configHash"]
        );
    }
    assert_ne!(
        first_snapshot["context"]["mcpGeneration"], second_snapshot["context"]["mcpGeneration"],
        "every replacement of callable clients must have an opaque, non-secret identity"
    );
    assert!(!first_snapshot
        .to_string()
        .contains(first_report.to_str().unwrap()));
    assert_ne!(
        first_snapshot["capabilityHash"],
        second_snapshot["capabilityHash"]
    );
    assert_eq!(
        disabled_snapshot["context"]["mcpSchemas"],
        serde_json::json!([])
    );
    assert!(!disabled_snapshot["tools"]
        .as_array()
        .unwrap()
        .iter()
        .any(|tool| tool["name"] == INSPECT));
    assert_ne!(
        disabled_snapshot["capabilityHash"],
        second_snapshot["capabilityHash"]
    );
    assert_eq!(
        chat.run_events("h219-mcp-first", None)
            .unwrap()
            .iter()
            .find(|event| event.event_type == "capability.snapshot")
            .unwrap()
            .data,
        first_snapshot
    );
}

fn assert_capability_generations(chat: &ChatService, run: &str, requests: &[serde_json::Value]) {
    use sha2::{Digest, Sha256};
    let events = chat.run_events(run, None).unwrap();
    let initial = events
        .iter()
        .find(|e| e.event_type == "capability.snapshot")
        .unwrap();
    let mut schema = initial.data["modelSchema"].clone();
    let mut changes = events
        .iter()
        .filter(|e| e.event_type == "capability_schema_snapshot");
    let mut wire_changes = events
        .iter()
        .filter(|e| e.event_type == "capability_wire_snapshot");
    let mut wire_schema = serde_json::Value::Null;
    let mut next_wire_generation = 0;
    for request in requests {
        let saved_names = |schema: &serde_json::Value| {
            schema["tools"]
                .as_array()
                .unwrap()
                .iter()
                .map(|t| t["name"].clone())
                .collect::<Vec<_>>()
        };
        let actual_hash = format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&request["tools"]).unwrap())
        );
        if wire_schema["schemaHash"] != actual_hash || wire_schema["model"] != request["model"] {
            let event = wire_changes
                .next()
                .expect("each changed adapter schema needs durable wire evidence");
            assert_eq!(event.data["snapshotId"], initial.data["snapshotId"]);
            wire_schema = event.data["wireSchema"].clone();
            assert_eq!(wire_schema["generation"], next_wire_generation);
            assert_eq!(wire_schema["wireMode"], "responses");
            next_wire_generation += 1;
        }
        // Native provider tools need not carry a name. Correlate by the
        // adapter's preserved order, then verify each actual wire hash.
        let names = saved_names(&wire_schema);
        if saved_names(&schema) != names {
            let change = changes
                .next()
                .expect("a new model-visible tool set needs durable evidence");
            assert_eq!(change.data["snapshotId"], initial.data["snapshotId"]);
            assert_eq!(
                change.data["modelSchema"]["generation"].as_u64(),
                Some(schema["generation"].as_u64().unwrap() + 1)
            );
            schema = change.data["modelSchema"].clone();
        }
        assert_eq!(saved_names(&schema), names);
        let actual = request["tools"].as_array().unwrap();
        let saved = wire_schema["tools"].as_array().unwrap();
        assert_eq!(actual.len(), saved.len());
        for (tool, reference) in actual.iter().zip(saved) {
            assert_eq!(tool["type"], reference["kind"]);
            if let Some(name) = tool.get("name") {
                assert_eq!(*name, reference["name"]);
            }
            assert_eq!(
                reference["schemaHash"],
                format!("{:x}", Sha256::digest(serde_json::to_vec(tool).unwrap()))
            );
        }
        assert_eq!(wire_schema["schemaHash"], actual_hash);
        assert_eq!(wire_schema["model"], request["model"]);
        assert_eq!(wire_schema["sourceSchemaHash"], schema["schemaHash"]);
        assert_eq!(saved_names(&wire_schema), names);
    }
    assert!(
        changes.next().is_none(),
        "unchanged model schemas must not create duplicate generations"
    );
    assert!(
        wire_changes.next().is_none(),
        "no duplicate wire generation"
    );
    let inspect = initial.data["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == INSPECT)
        .unwrap();
    assert_eq!(inspect["source"], "discovery_audit");
    assert_eq!(inspect["eligibleForDeferral"], true);
}

#[tokio::test]
async fn deferred_mcp_discovery_survives_turns_and_database_reopen_without_session_leak() {
    check_discovery_reopen(false).await;
}

#[tokio::test]
async fn h152_native_discovery_executes_once_and_cold_replays_its_catalog() {
    check_discovery_reopen(true).await;
}

#[tokio::test]
async fn h152_native_discovery_respects_permission_denial_and_cancellation() {
    struct HeldDiscovery {
        model: Arc<DiscoveryModel>,
        streamed: tokio::sync::Notify,
    }
    #[async_trait]
    impl HttpTransport for HeldDiscovery {
        async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
            self.model.stream(request, sink).await?;
            self.streamed.notify_one();
            std::future::pending().await
        }
    }
    for cancel in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let (script, report_path) = fixture(root.path());
        let db = Arc::new(Database::open_in_memory().unwrap());
        let settings = settings(&db, &Arc::new(MemorySecretStore::new()));
        settings
            .initialize("offline-h152-placeholder")
            .await
            .unwrap();
        settings
            .set_tool_search_mode(ToolSearchMode::Enabled)
            .unwrap();
        if !cancel {
            settings
                .set_permission_rules(deepagent_hooks::PermissionRules::new(
                    [],
                    [],
                    ["tool_search".to_string()],
                ))
                .unwrap();
        }
        let mcp = Arc::new(McpService::new(db.clone()));
        register_fixture(&mcp, &script, &report_path);
        prepare(&mcp).await;
        let model = Arc::new(DiscoveryModel {
            steps: vec![
                DiscoveryStep::NativeSearch("discovery-search"),
                DiscoveryStep::Finish,
            ],
            requests: Mutex::new(Vec::new()),
        });
        let held = Arc::new(HeldDiscovery {
            model: model.clone(),
            streamed: tokio::sync::Notify::new(),
        });
        let transport: Arc<dyn HttpTransport> = if cancel { held.clone() } else { model.clone() };
        let chat =
            ChatService::new(db.clone(), settings, transport, root.path()).with_mcp(mcp.clone());
        let session = chat.create_session(None).unwrap();
        let run = tokio::time::timeout(
            Duration::from_secs(5),
            chat.run_in_session(
                "Discover the synthetic tool.",
                Some(&session),
                None,
                None,
                Vec::new(),
                None,
                false,
                Some("h152-native-gate"),
                |_| {},
                |_| {},
            ),
        );
        let cancellation = async {
            if cancel {
                tokio::time::timeout(Duration::from_secs(3), held.streamed.notified())
                    .await
                    .unwrap();
                assert!(chat.request_cancel("h152-native-gate").unwrap().accepted);
            }
        };
        let (result, ()) = tokio::join!(run, cancellation);
        mcp.ready_registry().close_all().await.unwrap();
        result.unwrap().unwrap();
        assert!(chat.discovered_tool_names(&session).is_empty());
        assert!(discovery_events(&db, &session).is_empty());
        assert_eq!(report(&report_path)["calls"], serde_json::json!([]));
        let requests = model.requests.lock().unwrap();
        assert_eq!(requests.len(), if cancel { 1 } else { 2 });
        if !cancel {
            let native = requests[1]["input"]
                .as_array()
                .unwrap()
                .iter()
                .find(|item| item["type"] == "tool_search_output")
                .unwrap();
            assert_eq!(native["status"], "failed");
            assert_eq!(native["tools"], serde_json::json!([]));
        }
        let events = chat.run_events("h152-native-gate", None).unwrap();
        let terminal: Vec<_> = events
            .iter()
            .filter(|event| event.event_type == "run_terminal")
            .collect();
        assert_eq!(terminal.len(), 1);
        assert_eq!(
            terminal[0].data["kind"],
            if cancel { "cancelled" } else { "succeeded" }
        );
    }
}

fn assert_native_search_pair(request: &serde_json::Value) {
    let input = request["input"].as_array().unwrap();
    let calls: Vec<_> = input
        .iter()
        .filter(|item| item["type"] == "tool_search_call")
        .collect();
    let outputs: Vec<_> = input
        .iter()
        .filter(|item| item["type"] == "tool_search_output")
        .collect();
    assert_eq!(calls.len(), 1);
    assert_eq!(outputs.len(), 1);
    assert_eq!(calls[0]["call_id"], "discovery-search");
    assert_eq!(outputs[0]["call_id"], calls[0]["call_id"]);
    assert_eq!(outputs[0]["execution"], "client");
    assert_eq!(outputs[0]["status"], "completed");
    let tools = outputs[0]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0]["name"], INSPECT);
    assert_eq!(
        tools[0]["parameters"]["required"],
        serde_json::json!(["value"])
    );
    assert!(!input.iter().any(
        |item| item["type"] == "function_call_output" && item["call_id"] == "discovery-search"
    ));
}

fn native_items(db: &Database, session: &str) -> Vec<serde_json::Value> {
    EventStore::new(db)
        .load_session(session.parse().unwrap())
        .unwrap()
        .into_iter()
        .filter_map(|event| match event.payload {
            EventPayload::ResponseItemAppended { item }
                if matches!(
                    item,
                    deepagent_core::response_item::ResponseItem::ToolSearchCall { .. }
                        | deepagent_core::response_item::ResponseItem::ToolSearchOutput { .. }
                ) =>
            {
                Some(serde_json::to_value(item).unwrap())
            }
            _ => None,
        })
        .collect()
}

async fn check_discovery_reopen(native: bool) {
    let root = tempfile::tempdir().unwrap();
    let (script, report_path) = fixture(root.path());
    let database_path = root.path().join("state.sqlite");
    let secrets = Arc::new(MemorySecretStore::new());
    let (session, previous_tools, previous_discovery, previous_native) = {
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
        register_fixture(&mcp, &script, &report_path);
        prepare(&mcp).await;
        let mut model = DiscoveryModel::discovering([DiscoveryStep::Finish]);
        if native {
            model.steps[0] = DiscoveryStep::NativeSearch("discovery-search");
        }
        let model = Arc::new(model);
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
        assert_capability_generations(&chat, "audit-discovery-first", &requests[..3]);
        assert_capability_generations(&chat, "audit-discovery-next", &requests[3..]);
        assert!(advertised(&requests[0]).is_empty());
        assert!(requests[0]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["name"] == "tool_search"));
        for request in &requests[1..] {
            assert_eq!(advertised(request), [INSPECT]);
            assert_eq!(request["tools"], requests[1]["tools"]);
            if native {
                assert_native_search_pair(request);
            }
        }
        assert_replayed_call(&requests[2], "discovery-inspect");
        assert_replayed_call(&requests[3], "discovery-inspect");
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
        let native_history = native_items(&db, &session);
        assert_eq!(native_history.len(), if native { 2 } else { 0 });
        if native {
            for item in &native_history {
                assert!(requests[1]["input"].as_array().unwrap().contains(item));
            }
        }
        (
            session,
            requests[1]["tools"].clone(),
            discovered,
            native_history,
        )
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
    assert_eq!(
        before_resume,
        [INSPECT],
        "discovery queries project durable history even before the first new run"
    );
    assert_eq!(restored_names, [INSPECT]);
    assert!(chat.discovered_tool_names(&fresh).is_empty());
    assert_eq!(discovery_events(&db, &session), previous_discovery);
    assert_eq!(native_items(&db, &session), previous_native);
    assert!(discovery_events(&db, &fresh).is_empty());
    assert_terminal(&chat, "audit-discovery-reopened");
    assert_terminal(&chat, "audit-discovery-fresh");
    let requests = model.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert_capability_generations(&chat, "audit-discovery-reopened", &requests[..1]);
    assert_capability_generations(&chat, "audit-discovery-fresh", &requests[1..]);
    assert_eq!(advertised(&requests[0]), [INSPECT]);
    assert_eq!(requests[0]["tools"], previous_tools);
    assert_replayed_call(&requests[0], "discovery-inspect");
    if native {
        assert_native_search_pair(&requests[0]);
    }
    assert!(advertised(&requests[1]).is_empty());
    assert!(!requests[1]["input"].to_string().contains(CANARY));
    assert!(requests.iter().all(|r| !advertised(r).contains(&OTHER)));
    let server = report(&report_path);
    assert_eq!(server["initialize"], 1);
    assert_eq!(server["list"], 1);
    assert_eq!(server["calls"], serde_json::json!([]));
    eprintln!("discovery host 2: requests={}, server={}, restored={restored_names:?}, fresh=[]; tools snapshot unchanged", requests.len(), server);
}

#[tokio::test]
async fn h219_discovery_schema_durability_failure_stops_before_next_model_and_mcp_call() {
    let root = tempfile::tempdir().unwrap();
    let (script, report_path) = fixture(root.path());
    let db = Arc::new(Database::open(root.path().join("state.sqlite")).unwrap());
    let settings = settings(&db, &Arc::new(MemorySecretStore::new()));
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
    db.with_conn(|connection| connection.execute_batch(
        "CREATE TRIGGER reject_schema BEFORE INSERT ON run_events WHEN NEW.event_type='capability_schema_snapshot' BEGIN SELECT RAISE(ABORT, 'synthetic schema failure'); END;"
    ).map_err(|error| CoreError::Persistence(error.to_string()))).unwrap();
    let mcp = Arc::new(McpService::new(db.clone()));
    register_fixture(&mcp, &script, &report_path);
    prepare(&mcp).await;
    let model = Arc::new(DiscoveryModel::discovering([]));
    let chat = ChatService::new(db, settings, model.clone(), root.path()).with_mcp(mcp.clone());
    let session = chat.create_session(None).unwrap();
    let result = turn(&chat, &session, "h219-schema-failure").await;
    mcp.ready_registry().close_all().await.unwrap();
    assert!(result.is_err());
    assert_eq!(model.requests.lock().unwrap().len(), 1);
    assert_eq!(report(&report_path)["calls"], serde_json::json!([]));
    let events = chat.run_events("h219-schema-failure", None).unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|e| e.event_type == "capability.snapshot")
            .count(),
        1
    );
    assert!(!events
        .iter()
        .any(|e| e.event_type == "capability_schema_snapshot"));
    let terminal: Vec<_> = events
        .iter()
        .filter(|e| e.event_type == "run_terminal")
        .collect();
    assert_eq!(terminal.len(), 1);
    assert_eq!(terminal[0].data["kind"], "failed");
}

#[derive(Clone, Copy)]
enum RewindDiscovery {
    Keep,
    Discard,
    Rediscover,
}

async fn observe_rewind_discovery(case: RewindDiscovery) -> (Vec<String>, Vec<String>) {
    let keep_discovery = matches!(case, RewindDiscovery::Keep);
    let rediscover = matches!(case, RewindDiscovery::Rediscover);
    let root = tempfile::tempdir().unwrap();
    let (script, report_path) = fixture(root.path());
    let database_path = root.path().join("rewind.sqlite");
    let secrets = Arc::new(MemorySecretStore::new());
    let (session_id, warm_tools, warm_discoveries) = {
        let db = Arc::new(Database::open(&database_path).unwrap());
        let clock = FixedClock::new(1_000);
        let mut session = Session::create(&db, &clock, Some("discovery rewind audit")).unwrap();
        for message in [
            Message::user("AUDIT_DISCOVERY_BASE_USER"),
            Message::assistant("AUDIT_DISCOVERY_BASE_ANSWER"),
        ] {
            session
                .append(EventPayload::MessageAppended { message })
                .unwrap();
        }
        let id = session.id();
        let session_id = id.to_string();
        let base_tail = EventStore::new(&db)
            .load_session(id)
            .unwrap()
            .last()
            .unwrap()
            .sequence;
        drop(session);
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
        register_fixture(&mcp, &script, &report_path);
        prepare(&mcp).await;
        let tail = if rediscover {
            vec![
                DiscoveryStep::Search("rediscovery-search"),
                DiscoveryStep::Inspect("rediscovery-inspect"),
                DiscoveryStep::Finish,
            ]
        } else {
            vec![DiscoveryStep::Finish]
        };
        let model = Arc::new(DiscoveryModel::discovering(tail));
        let chat = ChatService::new(db.clone(), settings, model.clone(), root.path())
            .with_mcp(mcp.clone());
        let first = turn(&chat, &session_id, "audit-discovery-before-rewind").await;
        if first.is_err() {
            mcp.ready_registry().close_all().await.unwrap();
            first.unwrap();
        }
        let discovered_before = discovery_events(&db, &session_id);
        let events = EventStore::new(&db).load_session(id).unwrap();
        let keep = if keep_discovery {
            events.last().unwrap().sequence
        } else {
            base_tail
        };
        let mut session = Session::recover(&db, &clock, id).unwrap();
        session
            .append(EventPayload::Note {
                text: "AUDIT_DISCARDED_TAIL".into(),
            })
            .unwrap();
        drop(session);
        let rewind = AppService::from_shared(db.clone()).rewind_session(&session_id, keep);
        let immediately_after = discovery_events(&db, &session_id);
        let warm = if rewind.is_ok() {
            turn(&chat, &session_id, "audit-discovery-after-rewind").await
        } else {
            Err(CoreError::other("discovery rewind failed"))
        };
        mcp.ready_registry().close_all().await.unwrap();
        let rewind = rewind.unwrap();
        warm.unwrap();
        assert!(rewind.events_removed > 0);
        assert_eq!(discovered_before, vec![vec![INSPECT.to_string()]]);
        let expected_events = if keep_discovery {
            discovered_before
        } else {
            Vec::new()
        };
        assert_eq!(immediately_after, expected_events);
        let warm_discoveries = discovery_events(&db, &session_id);
        if rediscover {
            assert!(warm_discoveries.len() <= 1);
            assert!(warm_discoveries
                .iter()
                .flatten()
                .all(|name| name == INSPECT));
        } else {
            assert_eq!(warm_discoveries, expected_events);
        }
        assert_terminal(&chat, "audit-discovery-before-rewind");
        assert_terminal(&chat, "audit-discovery-after-rewind");
        let requests = model.requests.lock().unwrap();
        assert_eq!(requests.len(), if rediscover { 6 } else { 4 });
        let warm = &requests[3];
        assert!(warm["input"]
            .to_string()
            .contains("AUDIT_DISCOVERY_BASE_ANSWER"));
        assert!(!warm["input"].to_string().contains("AUDIT_DISCARDED_TAIL"));
        assert_eq!(warm["input"].to_string().contains(CANARY), keep_discovery);
        let first_tools: Vec<String> = advertised(warm).into_iter().map(str::to_owned).collect();
        // This request precedes the new branch's tool_search. A rediscovery
        // cannot retroactively expose its schema on that earlier request.
        assert_eq!(
            first_tools,
            if keep_discovery {
                vec![INSPECT.to_owned()]
            } else {
                vec![]
            }
        );
        let mut tools = first_tools;
        let server = report(&report_path);
        assert_eq!(
            server["calls"].as_array().unwrap().len(),
            if rediscover { 2 } else { 1 }
        );
        assert_eq!(server["calls"][0]["arguments"]["value"], CANARY);
        if rediscover {
            assert_eq!(warm_discoveries, vec![vec![INSPECT.to_owned()]]);
            // Compare the completed warm branch to the reopened branch only
            // after its successful rediscovery has updated the live schema.
            for request in &requests[4..] {
                let discovered: Vec<_> =
                    advertised(request).into_iter().map(str::to_owned).collect();
                assert_eq!(discovered, [INSPECT]);
            }
            tools = advertised(requests.last().unwrap())
                .into_iter()
                .map(str::to_owned)
                .collect();
            let search_outputs: Vec<_> = requests[4]["input"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|item| {
                    item["type"] == "function_call_output"
                        && item["call_id"] == "rediscovery-search"
                })
                .collect();
            assert_eq!(search_outputs.len(), 1);
            assert!(search_outputs[0]["output"].to_string().contains(INSPECT));
            assert_replayed_call(&requests[5], "rediscovery-inspect");
            assert_eq!(server["calls"][1]["arguments"]["value"], CANARY);
        }
        eprintln!("rewind discovery warm: keep_discovery={keep_discovery}, rediscover={rediscover}, tools={tools:?}, discovery_events={}, in_memory={:?}, server={server}", warm_discoveries.len(), chat.discovered_tool_names(&session_id));
        (session_id, tools, warm_discoveries)
    };

    let db = Arc::new(Database::open(&database_path).unwrap());
    let settings = settings(&db, &secrets);
    let mcp = Arc::new(McpService::new(db.clone()));
    prepare(&mcp).await;
    let model = Arc::new(DiscoveryModel::default());
    let chat =
        ChatService::new(db.clone(), settings, model.clone(), root.path()).with_mcp(mcp.clone());
    let cold = turn(&chat, &session_id, "audit-discovery-after-reopen").await;
    mcp.ready_registry().close_all().await.unwrap();
    cold.unwrap();
    assert_terminal(&chat, "audit-discovery-after-reopen");
    let requests = model.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    let cold_tools: Vec<String> = advertised(&requests[0])
        .into_iter()
        .map(str::to_owned)
        .collect();
    let expected: Vec<String> = warm_discoveries.iter().flatten().cloned().collect();
    assert_eq!(cold_tools, expected);
    assert_eq!(chat.discovered_tool_names(&session_id), expected);
    assert_eq!(
        discovery_events(&db, &session_id).len(),
        warm_discoveries.len()
    );
    assert_eq!(
        requests[0]["input"].to_string().contains(CANARY),
        keep_discovery || rediscover
    );
    if rediscover {
        assert_replayed_call(&requests[0], "rediscovery-inspect");
        assert!(requests[0]["input"]
            .to_string()
            .contains("rediscovery-search"));
    }
    assert_eq!(report(&report_path)["calls"], serde_json::json!([]));
    eprintln!("rewind discovery cold: keep_discovery={keep_discovery}, rediscover={rediscover}, tools={cold_tools:?}, discovery_events={}", warm_discoveries.len());
    (warm_tools, cold_tools)
}

#[tokio::test]
async fn rewind_kept_discovery_matches_reopened_tool_snapshot() {
    let (warm, cold) = observe_rewind_discovery(RewindDiscovery::Keep).await;
    assert_eq!(warm, [INSPECT]);
    assert_eq!(warm, cold);
}

#[tokio::test]
async fn audit_rewind_discarded_discovery_matches_reopened_tool_snapshot() {
    let (warm, cold) = observe_rewind_discovery(RewindDiscovery::Discard).await;
    assert_eq!(
        warm, cold,
        "the same retained discovery history must produce the same tool visibility"
    );
}

#[tokio::test]
async fn audit_rediscovery_after_rewind_survives_reopened_tool_snapshot() {
    let (warm, cold) = observe_rewind_discovery(RewindDiscovery::Rediscover).await;
    assert_eq!(warm, [INSPECT]);
    assert_eq!(
        cold,
        [INSPECT],
        "successful rediscovery in the retained branch must survive reopening"
    );
}
