//! A presentation-size limit must not turn a valid MCP hook denial into permission.

use super::responses_fixture::{call, completed, text};
use super::*;
use deepagent_app_core::{McpServerDto, McpService};
use deepagent_hooks::{parse_structured_hook_output, HookOutcome};
use deepagent_persistence::run_store::RunStore;
use deepagent_runtime::RuntimeEvent;
use serde_json::{json, Value};
use std::time::Duration;

const MODEL: &str = "deepseek-flash";
const RUN: &str = "audit-mcp-hook-size";
const CALL: &str = "size-parent-write";
const WRITTEN: &str = "SYNTHETIC_FILE_AFTER_MCP_HOOK";
const SERVER: &str = r#"
const fs = require('node:fs');
const readline = require('node:readline');
const decision = fs.readFileSync(process.env.AUDIT_DECISION, 'utf8');
function reply(id, result) {
  process.stdout.write(JSON.stringify({jsonrpc:'2.0', id, result}) + '\n');
}
readline.createInterface({input:process.stdin}).on('line', line => {
  const req = JSON.parse(line);
  if (req.method === 'initialize') reply(req.id, {
    protocolVersion:req.params.protocolVersion, capabilities:{tools:{}},
    serverInfo:{name:'mcp-decision-size-audit', version:'1'}
  });
  else if (req.method === 'tools/list') reply(req.id, {tools:[{
    name:'review', description:'Return the supplied complete hook decision',
    inputSchema:{type:'object'}
  }]});
  else if (req.method === 'tools/call') {
    fs.writeFileSync(process.env.AUDIT_REQUEST, JSON.stringify(req));
    reply(req.id, {content:[{type:'text',text:decision}],isError:false});
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
            requests.push(body);
            requests.len() - 1
        };
        let item = match index {
            0 => call(
                CALL,
                "write_file",
                json!({"path":"target.txt","content":WRITTEN}),
            ),
            1 => text("size-parent-final", "Offline decision-size audit complete."),
            _ => panic!("unexpected parent model request {index}"),
        };
        emit_responses_events(
            sink,
            &completed(
                &format!("size-parent-response-{index}"),
                MODEL,
                item,
                json!({"input_tokens":12,"output_tokens":6,"total_tokens":18}),
            ),
        )
    }
}

fn decision(ok: bool, chars: usize, letter: char) -> String {
    let base = json!({"ok":ok,"reason":""}).to_string();
    let reason = letter.to_string().repeat(chars - base.chars().count());
    let decision = json!({"ok":ok,"reason":reason}).to_string();
    assert_eq!(decision.chars().count(), chars);
    assert_eq!(serde_json::from_str::<Value>(&decision).unwrap()["ok"], ok);
    assert!(matches!(
        (ok, parse_structured_hook_output(&decision)),
        (true, Some(HookOutcome::Continue)) | (false, Some(HookOutcome::Deny { .. }))
    ));
    decision
}

#[derive(Debug)]
struct Observation {
    chars: usize,
    bytes: usize,
    hook_outcome: String,
    written: bool,
    tool_ok: bool,
}

async fn observe(ok: bool, chars: usize, letter: char) -> Observation {
    let decision = decision(ok, chars, letter);
    let root = tempfile::tempdir().unwrap();
    let script = root.path().join("decision-size-mcp.cjs");
    let decision_path = root.path().join("decision.json");
    let request_path = root.path().join("mcp-request.json");
    std::fs::write(&script, SERVER).unwrap();
    std::fs::write(&decision_path, &decision).unwrap();
    let db = Arc::new(Database::open(root.path().join("state.sqlite")).unwrap());
    let settings = Arc::new(SettingsService::new(
        db.clone(),
        Arc::new(MockTransport::with_get_json(
            r#"{"data":[{"id":"deepseek-flash","object":"model","owned_by":"deepseek"}]}"#,
        )),
        Arc::new(MemorySecretStore::new()),
    ));
    crate::registry_fixture::configure(&settings, "offline-audit-placeholder")
        .await
        .unwrap();
    settings
        .set_active_permission_preset(PermissionPreset::Default)
        .unwrap();
    settings
        .set_hooks_json(
            &json!({"hooks":{"PreToolUse":[{"matcher":"write_file","hooks":[{
                "type":"mcp_tool","command":"mcp__decision_size_audit__review","timeout":5
            }]}]}})
            .to_string(),
        )
        .unwrap();
    let mcp = Arc::new(McpService::new(db.clone()));
    let dto: McpServerDto = serde_json::from_value(json!({
        "name":"decision_size_audit","transport":"stdio","enabled":true,
        "command":"node","args":[script],"env":{
            "AUDIT_DECISION":decision_path,"AUDIT_REQUEST":request_path
        }
    }))
    .unwrap();
    mcp.upsert(dto).unwrap();
    mcp.start_preparation();
    let prepared = tokio::time::timeout(Duration::from_secs(4), async {
        while mcp.ready_registry().server_count() != 1 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await;
    let ready = mcp.ready_registry();
    if prepared.is_err() {
        ready.close_all().await.unwrap();
        panic!("the real MCP server must finish discovery before the parent run");
    }
    let model = Arc::new(ParentModel {
        requests: Mutex::new(Vec::new()),
    });
    let chat = ChatService::new(db.clone(), settings, model.clone(), root.path()).with_mcp(mcp);
    let events = Arc::new(Mutex::new(Vec::new()));
    let captured = events.clone();
    let result = tokio::time::timeout(
        Duration::from_secs(8),
        chat.run_in_session(
            "Create target.txt with the supplied synthetic text, then reply briefly.",
            None,
            None,
            None,
            Vec::new(),
            None,
            false,
            Some(RUN),
            move |event| captured.lock().unwrap().push(event),
            |_| {},
        ),
    )
    .await;
    ready.close_all().await.unwrap();
    result.expect("formal workflow must settle").unwrap();
    let request: Value =
        serde_json::from_str(&std::fs::read_to_string(request_path).unwrap()).unwrap();
    assert_eq!(request["method"], "tools/call");
    assert_eq!(request["params"]["name"], "review");
    assert_eq!(
        request["params"]["arguments"]["hook_event_name"],
        "PreToolUse"
    );
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
            .filter(|e| e.event_type == "run_terminal")
            .count(),
        1
    );
    let requests = model.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    let outputs: Vec<_> = requests[1]["input"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|item| item["type"] == "function_call_output" && item["call_id"] == CALL)
        .collect();
    assert_eq!(outputs.len(), 1);
    let output: Value = serde_json::from_str(outputs[0]["output"].as_str().unwrap()).unwrap();
    let tool_ok = output["status"] == "ok";
    if !tool_ok {
        assert!(output.to_string().contains("hook_denied"));
    }
    let events = events.lock().unwrap();
    let outcomes: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            RuntimeEvent::HookCompleted { event, outcome, .. } if event == "PreToolUse" => {
                Some(outcome.clone())
            }
            _ => None,
        })
        .collect();
    assert_eq!(outcomes.len(), 1);
    let target = root.path().join("target.txt");
    if target.exists() {
        assert_eq!(std::fs::read_to_string(&target).unwrap(), WRITTEN);
    }
    let observed = Observation {
        chars,
        bytes: decision.len(),
        hook_outcome: outcomes[0].clone(),
        written: target.exists(),
        tool_ok,
    };
    eprintln!("MCP hook decision ok={ok}: {observed:?}");
    assert_eq!(observed.chars, chars);
    assert_eq!(observed.bytes, decision.len());
    observed
}

#[tokio::test]
async fn mcp_hook_short_and_exact_limit_denials_block_the_file_write() {
    for (chars, letter) in [(100, 'd'), (8192, 'd'), (8192, '拒')] {
        let observed = observe(false, chars, letter).await;
        assert_eq!(observed.hook_outcome, "blocked");
        assert!(!observed.written);
        assert!(!observed.tool_ok);
    }
}

#[tokio::test]
async fn mcp_hook_short_and_long_allow_decisions_allow_the_file_write() {
    for chars in [100, 8193] {
        let observed = observe(true, chars, 'a').await;
        assert_eq!(observed.hook_outcome, "continued");
        assert!(observed.written);
        assert!(observed.tool_ok);
    }
}

#[tokio::test]
async fn audit_mcp_hook_long_denials_still_block_the_file_write() {
    let mut observations = Vec::new();
    for (chars, letter) in [(8193, 'd'), (10000, 'd'), (8193, '拒')] {
        observations.push(observe(false, chars, letter).await);
    }
    assert!(
        observations
            .iter()
            .all(|o| o.hook_outcome == "blocked" && !o.written && !o.tool_ok),
        "complete valid denials must stay blocking: {observations:?}"
    );
}
