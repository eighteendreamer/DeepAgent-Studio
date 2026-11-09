//! Production host preparation -> real stdio lifecycle -> formal tool boundary.
use super::*;
use deepagent_persistence::Database;
use deepagent_tools::{ToolExecutionContext, ToolRegistry};

async fn wait_for(mut predicate: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while !predicate() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("synthetic host state did not settle");
}

#[tokio::test]
async fn h48_real_host_retires_exhausted_capabilities_and_explicit_toggle_recovers() {
    let root = tempfile::tempdir().unwrap();
    let script = root.path().join("server.cjs");
    std::fs::write(&script, r#"
const fs = require('node:fs');
const path = require('node:path');
const [root, label] = process.argv.slice(2);
const file = suffix => path.join(root, label + suffix);
fs.appendFileSync(file('.starts'), process.pid + '\n');
if (fs.existsSync(file('.fail'))) process.exit(19);
require('node:readline').createInterface({ input: process.stdin }).on('line', line => {
  const msg = JSON.parse(line);
  if (msg.id === undefined) return;
  const reply = result => process.stdout.write(JSON.stringify({jsonrpc:'2.0',id:msg.id,result}) + '\n');
  if (fs.existsSync(file('.fail'))) process.exit(19);
  if (msg.method === 'initialize') reply({protocolVersion:msg.params.protocolVersion,
    capabilities:{tools:{}},serverInfo:{name:label,version:'1'},instructions:label});
  else if (msg.method === 'tools/list') reply({tools:[{name:'echo',description:label,inputSchema:{type:'object'}}]});
  else if (msg.method === 'tools/call') reply({content:[{type:'text',text:label}],isError:false});
  else reply({});
});
"#).unwrap();
    let config = |label: &str| McpServerConfig {
        response_limit_bytes: None,
        transport: Some(TransportType::Stdio),
        command: Some("node".into()),
        args: vec![
            script.display().to_string(),
            root.path().display().to_string(),
            label.into(),
        ],
        cwd: Some(root.path().to_path_buf()),
        env: BTreeMap::new(),
        url: None,
        headers: BTreeMap::new(),
    };
    let service = Arc::new(McpService::new(Arc::new(
        Database::open_in_memory().unwrap(),
    )));
    for label in ["a", "b"] {
        service
            .upsert(McpServerDto::from_config(label, &config(label), true))
            .unwrap();
    }
    service.start_preparation();
    wait_for(|| service.ready_registry().all_tools().len() == 2).await;
    let old = service.ready_registry();
    let old_adapters = deepagent_mcp::adapters_for(old.clone());
    let mut previous_tools = ToolRegistry::new();
    crate::mcp_runtime::attach_mcp_tools(&mut previous_tools, Some(&service), None, None, None)
        .unwrap();
    std::fs::write(root.path().join("a.fail"), "synthetic failure").unwrap();
    assert!(matches!(
        old.invoke("mcp__a__echo", serde_json::json!({})).await,
        Err(CoreError::RemoteOutcomeUnknown { .. })
    ));
    let result = tokio::time::timeout(
        Duration::from_secs(60),
        old.invoke("mcp__a__echo", serde_json::json!({})),
    )
    .await
    .unwrap();
    assert!(matches!(
        result,
        Err(CoreError::McpUnavailable {
            recovery_attempts: 3,
            ..
        })
    ));
    let starts = |label: &str| {
        std::fs::read_to_string(root.path().join(format!("{label}.starts")))
            .unwrap()
            .lines()
            .count()
    };
    assert_eq!(starts("a"), 4);
    assert_eq!(starts("b"), 1);
    let status = service.cached_status("a");
    assert_eq!(status.status, "failed");
    assert!(status.tools.is_empty() && status.error.is_some());
    let (connected, failures) = service.connected_registry().await.unwrap();
    assert_eq!(connected.all_tools().len(), 1);
    assert_eq!(failures.len(), 1);
    assert_eq!(failures[0].0, "a");
    let mut registry = ToolRegistry::new();
    let projection =
        crate::mcp_runtime::attach_mcp_tools(&mut registry, Some(&service), None, None, None)
            .unwrap();
    assert!(registry.get("mcp__a__echo").is_none());
    assert!(registry.get("mcp__b__echo").is_some());
    assert_eq!(
        projection.instructions_entries,
        vec![("b".into(), "b".into())]
    );
    let lifecycle = projection
        .lifecycle
        .iter()
        .find(|entry| entry.server_id == "a")
        .unwrap();
    assert_eq!(lifecycle.status, "unavailable");
    assert_eq!(
        lifecycle.degradation_code.as_deref(),
        Some("mcp_recovery_exhausted")
    );
    assert_eq!(lifecycle.tool_count, 0);
    assert_eq!(lifecycle.reason.as_deref(), Some(
        "MCP local connection unavailable after 3 recovery attempts; explicit lifecycle retry required"
    ));
    let adapter = old_adapters
        .iter()
        .find(|tool| tool.descriptor().name == "mcp__a__echo")
        .unwrap();
    let output = adapter
        .invoke_with_context(serde_json::json!({}), ToolExecutionContext::default())
        .await
        .unwrap();
    assert!(!output.ok);
    assert_eq!(output.value["outcome"], "not_dispatched");
    let output = previous_tools
        .invoke(
            "mcp__a__echo",
            serde_json::json!({}),
            &deepagent_tools::PermissionSet::developer(),
            true,
        )
        .await
        .unwrap();
    assert!(!output.ok);
    assert_eq!(output.value["error_type"], "mcp_unavailable");
    assert!(registry
        .invoke(
            "mcp__b__echo",
            serde_json::json!({}),
            &deepagent_tools::PermissionSet::empty(),
            false
        )
        .await
        .is_err());
    assert!(
        registry
            .invoke(
                "mcp__b__echo",
                serde_json::json!({}),
                &deepagent_tools::PermissionSet::developer(),
                true
            )
            .await
            .unwrap()
            .ok
    );
    assert_eq!(starts("a"), 4);
    assert_eq!(
        old.invoke("mcp__b__echo", serde_json::json!({}))
            .await
            .unwrap()
            .text(),
        "b"
    );
    service.start_preparation();
    assert_eq!(
        starts("a"),
        4,
        "a routine host view must not reopen the budget"
    );

    // The existing UI enable command calls these same production methods.
    service.set_enabled("a", false).unwrap();
    service.start_preparation();
    wait_for(|| {
        !service
            .ready_registry()
            .server_names()
            .contains(&"a".to_owned())
    })
    .await;
    std::fs::remove_file(root.path().join("a.fail")).unwrap();
    service.set_enabled("a", true).unwrap();
    service.start_preparation();
    wait_for(|| service.ready_registry().find_tool("mcp__a__echo").is_some()).await;
    let fresh = service.ready_registry();
    assert_eq!(
        fresh
            .invoke("mcp__a__echo", serde_json::json!({}))
            .await
            .unwrap()
            .text(),
        "a"
    );
    assert_eq!(starts("a"), 5);
    assert_eq!(service.cached_status("a").status, "connected");
    assert!(matches!(
        old.invoke("mcp__a__echo", serde_json::json!({})).await,
        Err(CoreError::McpUnavailable { .. })
    ));

    // Plugin refresh's existing explicit retry flag replaces a depleted slot;
    // a normal waiter must instead observe the same retired identity.
    let cfg = config("a");
    fresh.close_all().await.unwrap();
    let no_retry = service
        .prepare_server_with_cancel("a", cfg.clone(), None, false)
        .await
        .unwrap();
    assert!(no_retry.find_tool("mcp__a__echo").is_none());
    let retried = service
        .prepare_server_with_cancel("a", cfg, None, true)
        .await
        .unwrap();
    assert_eq!(
        retried
            .invoke("mcp__a__echo", serde_json::json!({}))
            .await
            .unwrap()
            .text(),
        "a"
    );
    assert_eq!(starts("a"), 6);
    retried.close_all().await.unwrap();
    old.close_all().await.unwrap();
}
