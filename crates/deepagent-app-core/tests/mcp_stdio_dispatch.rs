//! H-09: the production McpService/registry/adapter path receives reversed replies.
use std::sync::Arc;
use std::time::Duration;

use deepagent_app_core::{McpServerDto, McpService};
use deepagent_persistence::Database;
use deepagent_tools::ToolExecutionContext;
use serde_json::json;

#[tokio::test]
async fn mcp_stdio_dispatch_routes_reversed_tool_results_through_shared_service() {
    let root = tempfile::tempdir().unwrap();
    let script = root.path().join("dispatch.cjs");
    let observed = root.path().join("slow-received");
    std::fs::write(&script, r#"
const fs = require('node:fs');
let ready = false, slow;
const reply = (req,result) => process.stdout.write(JSON.stringify({jsonrpc:'2.0',id:req.id,result})+'\n');
const result = label => ({content:[{type:'text',text:label}],isError:false});
require('node:readline').createInterface({input:process.stdin}).on('line', line => {
  const req = JSON.parse(line);
  if (req.method === 'initialize') reply(req, {
    protocolVersion:req.params.protocolVersion,capabilities:{tools:{}},
    serverInfo:{name:'dispatch',version:'1'}
  });
  else if (req.method === 'notifications/initialized') ready = true;
  else if (req.method === 'tools/list') {
    if (!ready) throw new Error('incomplete handshake');
    reply(req,{tools:[{name:'echo',description:'Local fixture',inputSchema:{type:'object'}}]});
  } else if (req.method === 'tools/call') {
    if (!ready) throw new Error('incomplete handshake');
    if (req.params.arguments.label === 'slow') {
      slow = req;
      fs.writeFileSync(process.env.OBSERVED,'received');
    } else {
      process.stdout.write(JSON.stringify({jsonrpc:'2.0',method:'notifications/progress',params:{progress:1}})+'\n');
      reply(req,result('fast'));
      reply(slow,result('slow'));
    }
  } else if (req.id !== undefined) reply(req,{});
});
"#).unwrap();
    let db = Arc::new(Database::open(root.path().join("state.sqlite")).unwrap());
    let service = Arc::new(McpService::new(db));
    let dto: McpServerDto = serde_json::from_value(json!({
        "name":"dispatch","transport":"stdio","enabled":true,"command":"node",
        "args":[script],"env":{"OBSERVED":observed}
    }))
    .unwrap();
    assert_eq!(
        service.test_server(dto.clone()).await.unwrap().status,
        "connected"
    );
    service.upsert(dto).unwrap();
    service.start_preparation();
    tokio::time::timeout(Duration::from_secs(5), async {
        while service.ready_registry().server_count() != 1 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("production MCP preparation deadline");
    let registry = service.ready_registry();
    let adapters = deepagent_mcp::adapters_for(registry.clone());
    let adapter = adapters
        .iter()
        .find(|tool| tool.descriptor().name == "mcp__dispatch__echo")
        .expect("prepared tool must be published")
        .clone();
    let slow = tokio::spawn({
        let adapter = adapter.clone();
        async move {
            adapter
                .invoke_with_context(json!({"label":"slow"}), ToolExecutionContext::default())
                .await
        }
    });
    tokio::time::timeout(Duration::from_secs(5), async {
        while !observed.exists() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("server must observe the pending slow call");
    let fast = tokio::time::timeout(
        Duration::from_secs(5),
        adapter.invoke(json!({"label":"fast"})),
    )
    .await;
    let slow = tokio::time::timeout(Duration::from_secs(5), slow).await;
    registry.close_all().await.unwrap();
    let fast = fast.expect("fast result deadline").unwrap();
    let slow = slow.expect("slow result deadline").unwrap().unwrap();
    assert!(fast.ok && slow.ok);
    assert_eq!(fast.value["text"], "fast");
    assert_eq!(slow.value["text"], "slow");
}
