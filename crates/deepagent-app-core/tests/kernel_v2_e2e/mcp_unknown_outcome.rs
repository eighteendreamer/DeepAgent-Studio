//! H-06 through the production MCP service, tool pipeline, storage and wire.
use super::responses_fixture::{call, completed, text};
use super::*;
use deepagent_app_core::{McpServerDto, McpService};
use deepagent_builtins::ToolSearchMode;
use deepagent_core::event::EventPayload;
use deepagent_persistence::event_store::EventStore;
use deepagent_persistence::run_control::{RunActionState, RunControlStore};
use deepagent_persistence::run_store::RunStore;
use serde_json::{json, Value};
use std::time::Duration;

const RUN: &str = "h06-production-run";
const CALL: &str = "h06-primary-call";
const TOOL: &str = "mcp__h06__append_record";
const SERVER: &str = r#"
const fs = require('node:fs');
const first = !fs.existsSync(process.env.GENERATION);
fs.appendFileSync(process.env.GENERATION, 'start\n');
const reply = (id, result) => process.stdout.write(JSON.stringify({jsonrpc:'2.0',id,result})+'\n');
require('node:readline').createInterface({input:process.stdin}).on('line', line => {
  const req = JSON.parse(line);
  if (req.method === 'initialize') reply(req.id, {
    protocolVersion:req.params.protocolVersion,capabilities:{tools:{}},serverInfo:{name:'h06',version:'1'}
  });
  else if (req.method === 'tools/list') reply(req.id, {tools:[{
    name:'append_record',description:'Append a local test record',inputSchema:{type:'object'}
  }]});
  else if (req.method === 'tools/call') {
    fs.appendFileSync(process.env.EFFECTS, req.params.arguments.label+'\n');
    if (first && process.env.LOSE_REPLY === 'true') process.exit(19);
    reply(req.id,{content:[{type:'text',text:'applied'}],isError:false});
  } else if (req.id !== undefined) reply(req.id, {});
});
process.stdin.on('end', () => process.exit(0));
setTimeout(() => process.exit(3), 30000).unref();
"#;

struct Model {
    requests: Mutex<Vec<Value>>,
}

#[async_trait]
impl HttpTransport for Model {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        let index = {
            let mut requests = self.requests.lock().unwrap();
            let index = requests.len();
            requests.push(serde_json::from_str(&request.body)?);
            index
        };
        let item = match index {
            0 => call(CALL, TOOL, json!({"label":"primary"})),
            1 => text(
                "h06-answer",
                "Tool result received. Verify uncertain remote state before retrying.",
            ),
            _ => return Err(CoreError::other("unexpected extra model request")),
        };
        emit_responses_events(
            sink,
            &completed(
                &format!("h06-response-{index}"),
                "deepseek-v4-flash",
                item,
                json!({"input_tokens":20,"output_tokens":5,"total_tokens":25}),
            ),
        )
    }
}

#[tokio::test]
async fn mcp_unknown_outcome_survives_events_reopen_and_next_model_request() {
    assert_production_outcome(true).await;
}

#[tokio::test]
async fn mcp_success_control_keeps_one_effect_and_a_completed_action() {
    assert_production_outcome(false).await;
}

async fn assert_production_outcome(lose_reply: bool) {
    let root = tempfile::tempdir().unwrap();
    let script = root.path().join("h06.cjs");
    let effects = root.path().join("effects.log");
    let generations = root.path().join("generations.log");
    std::fs::write(&script, SERVER).unwrap();
    let db_path = root.path().join("state.sqlite");
    let db = Arc::new(Database::open(&db_path).unwrap());
    let settings = Arc::new(SettingsService::new(
        db.clone(),
        Arc::new(MockTransport::with_get_json(
            r#"{"data":[{"id":"deepseek-v4-flash","object":"model","owned_by":"deepseek"}]}"#,
        )),
        Arc::new(MemorySecretStore::new()),
    ));
    settings
        .initialize("offline-h06-placeholder")
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
        "name":"h06","transport":"stdio","enabled":true,"command":"node","args":[script],
        "env":{"GENERATION":generations,"EFFECTS":effects,"LOSE_REPLY":lose_reply.to_string()}
    }))
    .unwrap();
    mcp.upsert(dto).unwrap();
    mcp.start_preparation();
    tokio::time::timeout(Duration::from_secs(5), async {
        while mcp.ready_registry().server_count() != 1 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("real Node server must become ready");
    let ready = mcp.ready_registry();
    let model = Arc::new(Model {
        requests: Mutex::new(Vec::new()),
    });
    let chat = ChatService::new(db.clone(), settings, model.clone(), root.path()).with_mcp(mcp);
    let outcome = tokio::time::timeout(
        Duration::from_secs(10),
        chat.run_in_session(
            "Append the local test record once, then acknowledge the result.",
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
    let independent = if outcome.as_ref().is_ok_and(|r| r.is_ok()) {
        Some(
            tokio::time::timeout(
                Duration::from_secs(5),
                ready.invoke_with_context(
                    TOOL,
                    json!({"label":"independent"}),
                    deepagent_tools::ToolExecutionContext::default(),
                ),
            )
            .await,
        )
    } else {
        None
    };
    ready.close_all().await.unwrap();
    let session = outcome.expect("run deadline").unwrap();
    assert!(
        !independent
            .unwrap()
            .expect("recovery deadline")
            .expect("new independent request")
            .is_error
    );
    assert_eq!(
        std::fs::read_to_string(&effects).unwrap(),
        "primary\nindependent\n"
    );
    assert_eq!(
        std::fs::read_to_string(&generations)
            .unwrap()
            .lines()
            .count(),
        if lose_reply { 2 } else { 1 }
    );
    let live = chat.run_events(RUN, None).unwrap();
    let tools: Vec<_> = live
        .iter()
        .filter(|e| e.event_type == "tool_completed" && e.data["call_id"] == CALL)
        .collect();
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].data["ok"], !lose_reply);
    let live_output = tools[0].data["output"].clone();
    drop(chat);
    drop(db);
    let reopened = Database::open(&db_path).unwrap();
    let events = RunStore::new(&reopened).events_after(RUN, None).unwrap();
    let tools: Vec<_> = events
        .iter()
        .filter(|e| e.event_type == "tool_completed" && e.data["call_id"] == CALL)
        .collect();
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].data["ok"], !lose_reply);
    let stored_output = tools[0].data["output"].clone();
    let terminals: Vec<_> = events
        .iter()
        .filter(|e| e.event_type == "run_terminal")
        .collect();
    assert_eq!(terminals.len(), 1);
    assert_eq!(terminals[0].data["kind"], "succeeded");
    let actions = RunControlStore::new(&reopened).list_actions(RUN).unwrap();
    assert_eq!(actions.len(), 1);
    assert_eq!(
        actions[0].state,
        if lose_reply {
            RunActionState::Failed
        } else {
            RunActionState::Completed
        }
    );
    let history = EventStore::new(&reopened)
        .load_session(session.parse().unwrap())
        .unwrap();
    let completions: Vec<_> = history
        .iter()
        .filter_map(|e| match &e.payload {
            EventPayload::ToolCallCompleted {
                call_id,
                ok,
                output,
                ..
            } if call_id == CALL => Some((*ok, output)),
            _ => None,
        })
        .collect();
    assert_eq!(completions.len(), 1);
    assert_eq!(completions[0].0, !lose_reply);
    let requests = model.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    let outputs: Vec<_> = requests[1]["input"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|v| v["type"] == "function_call_output" && v["call_id"] == CALL)
        .collect();
    assert_eq!(outputs.len(), 1);
    let wire: Value = serde_json::from_str(outputs[0]["output"].as_str().unwrap()).unwrap();
    assert_eq!(wire["status"], if lose_reply { "error" } else { "ok" });
    if lose_reply {
        for output in [
            completions[0].1,
            &wire["error"],
            &live_output,
            &stored_output,
        ] {
            assert_eq!(output["outcome"], "unknown");
            assert_eq!(output["retryable"], false);
            assert_eq!(output["error_type"], "mcp_unknown_outcome");
            assert_eq!(output["method"], "tools/call");
            assert!(output["request_id"].is_u64());
            assert!(output["error"]
                .as_str()
                .unwrap()
                .contains("verify the remote state"));
        }
    }
    eprintln!("H-06 AppCore: lose_reply={lose_reply}, effects=primary/independent, requests=2, completions=1, sqlite_reopened=true");
}
