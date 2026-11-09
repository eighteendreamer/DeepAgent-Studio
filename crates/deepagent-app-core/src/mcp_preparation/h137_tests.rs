//! Catalog notifications through real host/plugin ownership. Synthetic Node,
//! files and SQLite only; no provider, credentials or external service.
use super::*;
use deepagent_persistence::Database;

struct Fixture(tempfile::TempDir);
impl Fixture {
    fn new() -> Self {
        let fixture = Self(tempfile::tempdir().unwrap());
        std::fs::write(fixture.0.path().join("server.cjs"),r#"
const fs=require('node:fs'),path=require('node:path');
const [root,label]=process.argv.slice(2); const file=s=>path.join(root,label+s);
let changed=false,lists=0; fs.appendFileSync(file('.starts'),process.pid+'\n');
require('node:readline').createInterface({input:process.stdin}).on('line',line=>{
 const req=JSON.parse(line); if(req.id===undefined)return;
 const reply=result=>process.stdout.write(JSON.stringify({jsonrpc:'2.0',id:req.id,result})+'\n');
 if(req.method==='initialize')reply({protocolVersion:req.params.protocolVersion,
  capabilities:{tools:{listChanged:true}},serverInfo:{name:label,version:'1'}});
 else if(req.method==='tools/list'){
  lists++; const tools=[{name:'control',description:label,inputSchema:{type:'object',properties:{change:{type:'boolean'}},additionalProperties:false}},
   {name:changed?'updated':'echo',description:label,inputSchema:{type:'object',additionalProperties:false}}];
  if(lists===2&&label==='error'){
   process.stdout.write(JSON.stringify({jsonrpc:'2.0',id:req.id,error:{code:-32603,message:'synthetic refresh rejection'}})+'\n');return;
  }
  if(lists>1&&label!=='error'){
   fs.writeFileSync(file('.refresh-entered'),'entered');
   const timer=setInterval(()=>{if(!fs.existsSync(file('.release')))return;
    clearInterval(timer);reply({tools});fs.writeFileSync(file('.released'),'released');},5);
  }else reply({tools});
 }else if(req.method==='tools/call'){
  if(req.params.arguments.change){changed=true;
   process.stdout.write(JSON.stringify({jsonrpc:'2.0',method:'notifications/tools/list_changed'})+'\n');}
  reply({content:[{type:'text',text:label}],isError:false});
 }else reply({});
});
setTimeout(()=>process.exit(2),label==='timeout'?45000:30000).unref();
"#).unwrap();
        fixture
    }

    fn config(&self, label: &str) -> McpServerConfig {
        McpServerConfig {
            response_limit_bytes: None,
            transport: Some(TransportType::Stdio),
            command: Some("node".into()),
            args: vec![
                self.0.path().join("server.cjs").display().to_string(),
                self.0.path().display().to_string(),
                label.into(),
            ],
            env: Default::default(),
            cwd: Some(self.0.path().into()),
            url: None,
            headers: Default::default(),
        }
    }
    fn put(&self, service: &McpService, label: &str) {
        service
            .upsert(McpServerDto::from_config(
                "catalog",
                &self.config(label),
                true,
            ))
            .unwrap();
    }
    fn marker(&self, label: &str, suffix: &str) -> std::path::PathBuf {
        self.0.path().join(format!("{label}{suffix}"))
    }
    fn release(&self, label: &str) {
        std::fs::write(self.marker(label, ".release"), "release").unwrap();
    }
}

async fn until(mut condition: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while !condition() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("finite host catalog observation");
}
fn service() -> Arc<McpService> {
    Arc::new(McpService::new(Arc::new(
        Database::open_in_memory().unwrap(),
    )))
}
async fn invoke(
    registry: &McpRegistry,
    name: &str,
    raw: &str,
    arguments: serde_json::Value,
) -> String {
    registry
        .invoke(&deepagent_mcp::namespaced_name(name, raw), arguments)
        .await
        .unwrap()
        .text()
}

#[tokio::test]
async fn h137_disable_and_replacement_reject_late_refreshes_but_keep_borrowed_routes() {
    for replace in [false, true] {
        let fixture = Fixture::new();
        let service = service();
        fixture.put(&service, "old");
        service.start_preparation();
        until(|| {
            service
                .ready_registry()
                .find_tool("mcp__catalog__echo")
                .is_some()
        })
        .await;
        let old = service.ready_registry();
        assert_eq!(
            invoke(
                &old,
                "catalog",
                "control",
                serde_json::json!({"change":true})
            )
            .await,
            "old"
        );
        until(|| fixture.marker("old", ".refresh-entered").exists()).await;
        if replace {
            fixture.put(&service, "new");
            service.start_preparation();
            until(|| {
                service
                    .ready_registry()
                    .find_tool("mcp__catalog__echo")
                    .is_some_and(|tool| tool.description == "new")
            })
            .await;
        } else {
            service.set_enabled("catalog", false).unwrap();
            service.start_preparation();
            assert_eq!(service.ready_registry().server_count(), 0);
        }
        fixture.release("old");
        until(|| fixture.marker("old", ".released").exists()).await;
        // Reply ordering on the same pipe proves the old reader passed the
        // released discovery response before this successful borrowed call.
        assert_eq!(
            invoke(&old, "catalog", "control", serde_json::json!({})).await,
            "old"
        );
        assert!(old.find_tool("mcp__catalog__echo").is_some());
        let current = service.ready_registry();
        assert!(
            current.find_tool("mcp__catalog__updated").is_none(),
            "retired response cannot publish into the replacement/disabled owner"
        );
        if replace {
            assert_eq!(
                invoke(&current, "catalog", "echo", serde_json::json!({})).await,
                "new"
            );
            assert_eq!(
                std::fs::read_to_string(fixture.marker("new", ".starts"))
                    .unwrap()
                    .lines()
                    .count(),
                1
            );
        } else {
            assert!(current.all_tools().is_empty());
        }
        old.close_all().await.unwrap();
        current.close_all().await.unwrap();
    }
}

#[tokio::test]
async fn h137_refresh_failure_reaches_status_and_lifecycle_then_recovers_on_notification() {
    let fixture = Fixture::new();
    let service = service();
    fixture.put(&service, "error");
    service.start_preparation();
    until(|| {
        service
            .ready_registry()
            .find_tool("mcp__catalog__echo")
            .is_some()
    })
    .await;
    let old = service.ready_registry();
    let mut tools = deepagent_tools::ToolRegistry::new();
    let before =
        crate::mcp_runtime::attach_mcp_tools(&mut tools, Some(&service), None, None, None).unwrap();
    invoke(
        &old,
        "catalog",
        "control",
        serde_json::json!({"change":true}),
    )
    .await;
    until(|| service.cached_status("catalog").status == "failed").await;
    let status = service.cached_status("catalog");
    assert!(status
        .error
        .unwrap()
        .contains("synthetic refresh rejection"));
    assert!(status.tools.is_empty());
    let mut tools = deepagent_tools::ToolRegistry::new();
    let failed =
        crate::mcp_runtime::attach_mcp_tools(&mut tools, Some(&service), None, None, None).unwrap();
    assert_eq!(failed.lifecycle.len(), 1);
    assert_eq!(
        failed.lifecycle[0].degradation_code.as_deref(),
        Some("mcp_catalog_refresh_failed")
    );
    assert_eq!(failed.lifecycle[0].tool_count, 0);
    assert_ne!(before.generation, failed.generation);
    invoke(
        &old,
        "catalog",
        "control",
        serde_json::json!({"change":true}),
    )
    .await;
    until(|| {
        service
            .ready_registry()
            .find_tool("mcp__catalog__updated")
            .is_some()
    })
    .await;
    assert_eq!(service.cached_status("catalog").status, "connected");
    assert_eq!(
        std::fs::read_to_string(fixture.marker("error", ".starts"))
            .unwrap()
            .lines()
            .count(),
        1,
        "catalog error must not restart a healthy peer"
    );
    old.close_all().await.unwrap();
}

#[tokio::test]
async fn h137_real_production_refresh_deadline_withdraws_tools_and_discards_late_response() {
    let fixture = Fixture::new();
    let service = service();
    fixture.put(&service, "timeout");
    service.start_preparation();
    until(|| {
        service
            .ready_registry()
            .find_tool("mcp__catalog__echo")
            .is_some()
    })
    .await;
    let old = service.ready_registry();
    invoke(
        &old,
        "catalog",
        "control",
        serde_json::json!({"change":true}),
    )
    .await;
    until(|| fixture.marker("timeout", ".refresh-entered").exists()).await;
    tokio::time::timeout(Duration::from_secs(35), async {
        while service.cached_status("catalog").status != "failed" {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the actual production 30-second refresh deadline must settle");
    let status = service.cached_status("catalog");
    assert!(status
        .error
        .unwrap()
        .contains("catalog refresh timed out after 30 seconds"));
    assert!(service.ready_registry().all_tools().is_empty());
    fixture.release("timeout");
    until(|| fixture.marker("timeout", ".released").exists()).await;
    assert_eq!(
        invoke(&old, "catalog", "control", serde_json::json!({})).await,
        "timeout"
    );
    assert!(
        service.ready_registry().all_tools().is_empty(),
        "a late timed-out response cannot republish tools"
    );
    invoke(
        &old,
        "catalog",
        "control",
        serde_json::json!({"change":true}),
    )
    .await;
    until(|| {
        service
            .ready_registry()
            .find_tool("mcp__catalog__updated")
            .is_some()
    })
    .await;
    assert_eq!(service.cached_status("catalog").status, "connected");
    assert_eq!(
        std::fs::read_to_string(fixture.marker("timeout", ".starts"))
            .unwrap()
            .lines()
            .count(),
        1
    );
    old.close_all().await.unwrap();
}

#[tokio::test]
async fn h137_plugin_catalog_reaches_the_next_runtime_manifest_without_repreparation() {
    use crate::plugin_loader::{load_plugins, PluginRoots};
    let fixture = Fixture::new();
    let root = fixture.0.path();
    fixture.release("plugin");
    let plugin = root.join("builtin/h137-plugin");
    std::fs::create_dir_all(plugin.join(".codex-plugin")).unwrap();
    std::fs::write(
        plugin.join(".codex-plugin/plugin.json"),
        r#"{"name":"h137-plugin","version":"0.1.0","mcpServers":".mcp.json"}"#,
    )
    .unwrap();
    let config = fixture.config("plugin");
    std::fs::write(plugin.join(".mcp.json"),serde_json::json!({"mcpServers":{"catalog":{"command":config.command,"args":config.args,"cwd":config.cwd}}}).to_string()).unwrap();
    let roots = PluginRoots {
        session: Vec::new(),
        builtin: root.join("builtin"),
        workspace: None,
        personal: root.join("personal"),
    };
    assert_eq!(load_plugins(&roots).len(), 1);
    let plugins = Arc::new(crate::PluginService::new(roots, root.join("plugin-data")));
    let service = service();
    plugins.start_preparation(service.clone(), Arc::new(|| {}));
    until(|| plugins.ready_snapshot().1.all_tools().len() == 2).await;
    let (before_projection, before) = plugins.ready_snapshot();
    let name = before.server_names()[0].clone();
    let mut tools = deepagent_tools::ToolRegistry::new();
    let before_runtime = crate::mcp_runtime::attach_mcp_tools(
        &mut tools,
        Some(&service),
        Some(&before_projection),
        Some(before.clone()),
        None,
    )
    .unwrap();
    invoke(
        &before,
        &name,
        "control",
        serde_json::json!({"change":true}),
    )
    .await;
    let updated = deepagent_mcp::namespaced_name(&name, "updated");
    until(|| plugins.ready_snapshot().1.find_tool(&updated).is_some()).await;
    let (projection, current) = plugins.ready_snapshot();
    assert!(
        before.find_tool(&updated).is_none(),
        "borrowed plugin view is immutable"
    );
    let mut tools = deepagent_tools::ToolRegistry::new();
    let after = crate::mcp_runtime::attach_mcp_tools(
        &mut tools,
        Some(&service),
        Some(&projection),
        Some(current.clone()),
        None,
    )
    .unwrap();
    assert_ne!(before_runtime.generation, after.generation);
    let result = tools
        .invoke(
            &updated,
            serde_json::json!({}),
            &deepagent_tools::PermissionSet::developer(),
            true,
        )
        .await
        .unwrap();
    assert!(result.ok);
    assert_eq!(result.value["text"], "plugin");
    assert!(tools
        .get(&deepagent_mcp::namespaced_name(&name, "echo"))
        .is_none());
    assert_eq!(
        std::fs::read_to_string(fixture.marker("plugin", ".starts"))
            .unwrap()
            .lines()
            .count(),
        1
    );
    before.close_all().await.unwrap();
    current.close_all().await.unwrap();
}
