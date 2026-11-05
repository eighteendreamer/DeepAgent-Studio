use super::*;
use deepagent_app_core::{McpServerDto, McpService};

const SERVER: &str = r#"
const fs = require('node:fs');
const readline = require('node:readline');
function reply(id,result) { process.stdout.write(JSON.stringify({jsonrpc:'2.0',id,result})+'\n'); }
readline.createInterface({input:process.stdin}).on('line',line => {
  const req = JSON.parse(line);
  if(req.method === 'initialize') reply(req.id,{
    protocolVersion:req.params.protocolVersion,capabilities:{tools:{}},serverInfo:{name:'synthetic-dedup',version:'1'}
  });
  else if(req.method === 'tools/list') reply(req.id,{tools:[{
    name:'review',description:'synthetic policy',inputSchema:{type:'object'}
  }]});
  else if(req.method === 'tools/call') {
    fs.appendFileSync(process.env.SYNTHETIC_REQUESTS,JSON.stringify(req.params.arguments)+'\n');
    reply(req.id,{content:[{type:'text',text:JSON.stringify({ok:req.params.arguments.policy==='allow',reason:'synthetic MCP denial'})}],isError:false});
  } else if(req.id !== undefined) reply(req.id,{});
});
process.stdin.on('end',()=>process.exit(0));
setTimeout(()=>process.exit(2),15000).unref();
"#;

#[tokio::test]
async fn h239_real_stdio_mcp_static_policies_both_execute_and_deny_survives_sqlite_reopen() {
    let root = tempfile::tempdir().unwrap();
    let script = root.path().join("synthetic-policy.cjs");
    let requests = root.path().join("requests.jsonl");
    std::fs::write(&script, SERVER).unwrap();
    std::fs::write(root.path().join("fixture.txt"), CANARY).unwrap();
    let db = Arc::new(Database::open(root.path().join("state.sqlite")).unwrap());
    let settings = settings(db.clone(), Case::Identical, "prompt").await;
    let action = |policy| json!({"type":"mcp_tool","command":"mcp__dedup_policy__review","arguments":{"policy":policy},"timeout":5});
    settings.set_hooks_json(&json!({"hooks":{"PreToolUse":[{"matcher":"read_file","hooks":[action("allow"),action("deny"),action("deny")]}]}}).to_string()).unwrap();
    let mcp = Arc::new(McpService::new(db.clone()));
    let dto: McpServerDto = serde_json::from_value(json!({
        "name":"dedup_policy","transport":"stdio","enabled":true,
        "command":"node","args":[script],"env":{"SYNTHETIC_REQUESTS":requests}
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
        panic!("synthetic local stdio peer must become ready");
    }
    let model = Arc::new(DedupModel::default());
    let chat = ChatService::new(db.clone(), settings, model.clone(), root.path()).with_mcp(mcp);
    let events = Arc::new(Mutex::new(Vec::new()));
    let captured = events.clone();
    let result = tokio::time::timeout(
        Duration::from_secs(8),
        chat.run_in_session(
            "Read synthetic fixture.txt and reply briefly.",
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
    result.expect("formal MCP policy run must settle").unwrap();
    let calls = std::fs::read_to_string(&requests)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(calls.len(), 2);
    assert_eq!(
        calls
            .iter()
            .filter(|args| args["policy"] == "allow")
            .count(),
        1
    );
    assert_eq!(
        calls.iter().filter(|args| args["policy"] == "deny").count(),
        1
    );
    assert!(calls
        .iter()
        .all(|args| args["hook_input"]["tool_name"] == "read_file"));
    let parent = model.parent.lock().unwrap();
    assert_eq!(parent.len(), 2);
    assert!(!parent[1].to_string().contains(CANARY));
    let outputs = parent[1]["input"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|item| item["type"] == "function_call_output" && item["call_id"] == CALL)
        .collect::<Vec<_>>();
    assert_eq!(outputs.len(), 1);
    assert!(outputs[0]["output"]
        .as_str()
        .unwrap()
        .contains("hook_denied"));
    drop(parent);
    assert!(model.hooks.lock().unwrap().is_empty());
    let completed = events
        .lock()
        .unwrap()
        .iter()
        .filter_map(|event| match event {
            RuntimeEvent::HookCompleted { event, outcome, .. } if event == "PreToolUse" => {
                Some(outcome.clone())
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(completed.len(), 2);
    assert_eq!(
        completed
            .iter()
            .filter(|outcome| outcome.as_str() == "blocked")
            .count(),
        1
    );
    let run = RunStore::new(&db).get(RUN).unwrap().unwrap();
    assert_eq!(run.terminal_kind.as_deref(), Some("succeeded"));
    drop(chat);
    drop(ready);
    drop(db);
    let reopened = Database::open(root.path().join("state.sqlite")).unwrap();
    let history = EventStore::new(&reopened)
        .load_session(SessionId::from_str(&run.session_id).unwrap())
        .unwrap();
    assert_eq!(history.iter().filter(|event|matches!(&event.payload,EventPayload::ToolCallRequested {call} if call.id==CALL)).count(),1);
    assert_eq!(history.iter().filter(|event|matches!(&event.payload,EventPayload::ToolCallCompleted {call_id,ok:false,..} if call_id==CALL)).count(),1);
    assert_eq!(RunStore::new(&reopened).get(RUN).unwrap(), Some(run));
    assert_eq!(
        RunStore::new(&reopened)
            .events_after(RUN, None)
            .unwrap()
            .iter()
            .filter(|event| event.event_type == "run_terminal")
            .count(),
        1
    );
    assert_eq!(
        std::fs::read_to_string(&requests).unwrap().lines().count(),
        2
    );
    assert_eq!(
        std::fs::read_to_string(root.path().join("fixture.txt")).unwrap(),
        CANARY
    );
}
