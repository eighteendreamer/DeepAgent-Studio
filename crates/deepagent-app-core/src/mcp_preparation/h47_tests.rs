//! Real gated MCP processes; synthetic files/SQLite only, no provider or network.
use super::*;

struct GatedServers {
    root: tempfile::TempDir,
}

impl GatedServers {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(
            root.path().join("server.cjs"),
            r#"
const fs = require('node:fs');
const path = require('node:path');
const [root, label] = process.argv.slice(2);
const file = suffix => path.join(root, label + suffix);
fs.appendFileSync(file('.starts'), process.pid + '\n');
require('node:readline').createInterface({ input: process.stdin }).on('line', line => {
  const msg = JSON.parse(line);
  if (msg.id === undefined) return;
  const reply = result => process.stdout.write(JSON.stringify({ jsonrpc: '2.0', id: msg.id, result }) + '\n');
  if (msg.method === 'initialize') {
    fs.writeFileSync(file('.entered'), String(process.pid));
    const timer = setInterval(() => {
      if (!fs.existsSync(file('.gate'))) return;
      clearInterval(timer);
      reply({ protocolVersion: msg.params.protocolVersion, capabilities: { tools: {} },
        serverInfo: { name: label, version: '1' }, instructions: label });
    }, 10);
  } else if (msg.method === 'tools/list') {
    reply({ tools: [{ name: 'echo', description: label, inputSchema: { type: 'object' } }] });
  } else if (msg.method === 'tools/call') {
    reply({ content: [{ type: 'text', text: label }], isError: false });
  } else { reply({}); }
});
"#,
        )
        .unwrap();
        Self { root }
    }

    fn config(&self, label: &str) -> McpServerConfig {
        McpServerConfig {
            response_limit_bytes: None,
            transport: Some(TransportType::Stdio),
            command: Some("node".into()),
            args: vec![
                self.root.path().join("server.cjs").display().to_string(),
                self.root.path().display().to_string(),
                label.into(),
            ],
            env: BTreeMap::new(),
            cwd: Some(self.root.path().to_path_buf()),
            url: None,
            headers: BTreeMap::new(),
        }
    }

    fn put(&self, service: &McpService, name: &str, label: &str) {
        service
            .upsert(McpServerDto::from_config(name, &self.config(label), true))
            .unwrap();
    }

    fn open(&self, label: &str) {
        std::fs::write(self.root.path().join(format!("{label}.gate")), "open").unwrap();
    }

    fn starts(&self, label: &str) -> usize {
        std::fs::read_to_string(self.root.path().join(format!("{label}.starts")))
            .map(|text| text.lines().count())
            .unwrap_or(0)
    }

    async fn entered(&self, label: &str) {
        wait_for(|| self.root.path().join(format!("{label}.entered")).exists()).await;
    }

    #[cfg(windows)]
    fn witness(&self, label: &str) -> ProcessWitness {
        use std::os::windows::io::FromRawHandle;
        use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE};
        let pid = std::fs::read_to_string(self.root.path().join(format!("{label}.entered")))
            .unwrap()
            .parse()
            .unwrap();
        // Observe exactly the fixture process; never kill by a recyclable PID.
        let handle = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
        assert!(!handle.is_null(), "{}", std::io::Error::last_os_error());
        ProcessWitness(unsafe { std::os::windows::io::OwnedHandle::from_raw_handle(handle.cast()) })
    }
}

#[cfg(windows)]
struct ProcessWitness(std::os::windows::io::OwnedHandle);

#[cfg(windows)]
impl ProcessWitness {
    async fn exited(&self) {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Foundation::WAIT_OBJECT_0;
        use windows_sys::Win32::System::Threading::WaitForSingleObject;
        wait_for(|| unsafe {
            WaitForSingleObject(self.0.as_raw_handle().cast(), 0) == WAIT_OBJECT_0
        })
        .await;
    }
}

async fn wait_for(mut predicate: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while !predicate() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("fixture gate/state did not reach the required condition");
}

fn service() -> Arc<McpService> {
    Arc::new(McpService::new(Arc::new(
        Database::open_in_memory().unwrap(),
    )))
}

async fn ready(service: &McpService, name: &str, label: &str) -> Arc<McpRegistry> {
    wait_for(|| {
        service
            .ready_registry()
            .find_tool(&deepagent_mcp::registry::namespaced_name(name, "echo"))
            .is_some_and(|tool| tool.description == label)
    })
    .await;
    service.ready_registry()
}

async fn assert_echo(registry: &McpRegistry, name: &str, label: &str) {
    let result = registry
        .invoke(
            &deepagent_mcp::registry::namespaced_name(name, "echo"),
            serde_json::json!({}),
        )
        .await
        .unwrap();
    assert!(!result.is_error);
    assert_eq!(
        serde_json::to_value(result).unwrap()["content"][0]["text"],
        label
    );
}

async fn assert_attached_echo(
    service: &McpService,
    projection: Option<&crate::plugin_runtime::PluginRuntimeProjection>,
    plugins: Option<Arc<McpRegistry>>,
    name: &str,
    label: &str,
) {
    let mut tools = deepagent_tools::ToolRegistry::new();
    let snapshot =
        crate::mcp_runtime::attach_mcp_tools(&mut tools, Some(service), projection, plugins, None)
            .unwrap();
    assert!(snapshot.generation.is_some());
    assert!(snapshot
        .instructions_entries
        .contains(&(name.into(), label.into())));
    let output = tools
        .invoke(
            &deepagent_mcp::registry::namespaced_name(name, "echo"),
            serde_json::json!({}),
            &deepagent_tools::PermissionSet::developer(),
            true,
        )
        .await
        .unwrap();
    assert!(output.ok);
    assert_eq!(output.value["text"], label);
}

#[tokio::test]
async fn h47_adding_or_changing_b_keeps_the_same_pending_a_and_real_routes() {
    for change_existing in [false, true] {
        let fixture = GatedServers::new();
        let service = service();
        fixture.put(&service, "a", "a1");
        if change_existing {
            fixture.open("b1");
            fixture.put(&service, "b", "b1");
        }
        service.start_preparation();
        fixture.entered("a1").await;
        if change_existing {
            ready(&service, "b", "b1").await;
        }
        let before = service.get("a").unwrap().unwrap();
        fixture.open("b2");
        fixture.put(&service, "b", "b2");
        service.start_preparation();
        let b = ready(&service, "b", "b2").await;
        assert_echo(&b, "b", "b2").await;
        assert_eq!(service.get("a").unwrap().unwrap(), before);
        assert_eq!(service.cached_status("a").status, "preparing");
        assert_eq!(fixture.starts("a1"), 1);
        fixture.open("a1");
        let all = ready(&service, "a", "a1").await;
        assert_echo(&all, "a", "a1").await;
        assert_echo(&all, "b", "b2").await;
        assert_attached_echo(&service, None, None, "a", "a1").await;
        assert_attached_echo(&service, None, None, "b", "b2").await;
        assert_eq!(fixture.starts("a1"), 1);
        let generation = service.preparation.generation.load(Ordering::Acquire);
        service.start_preparation();
        assert_eq!(
            service.preparation.generation.load(Ordering::Acquire),
            generation
        );
        all.close_all().await.unwrap();
    }
}

#[tokio::test]
async fn h47_cancelled_or_dropped_plugin_waiter_does_not_own_shared_startup() {
    for drop_waiter in [false, true] {
        let fixture = GatedServers::new();
        let service = service();
        service
            .reconcile_owner("plugin:test", BTreeSet::from(["a".into()]))
            .await
            .unwrap();
        let cancel = deepagent_runtime::CancellationTree::new();
        let pending = tokio::spawn({
            let service = service.clone();
            let config = fixture.config("a1");
            let cancel = cancel.clone();
            async move {
                service
                    .prepare_server_with_cancel("a", config, Some(cancel), false)
                    .await
            }
        });
        fixture.entered("a1").await;
        fixture.put(&service, "a", "a1");
        service.start_preparation();
        if drop_waiter {
            pending.abort();
            assert!(matches!(pending.await, Err(error) if error.is_cancelled()));
        } else {
            cancel.cancel();
            assert!(pending.await.unwrap().is_err());
        }
        service
            .reconcile_owner("plugin:test", BTreeSet::new())
            .await
            .unwrap();
        fixture.open("a1");
        let all = ready(&service, "a", "a1").await;
        assert_eq!(
            fixture.starts("a1"),
            1,
            "the host must reuse the original process"
        );
        assert_echo(&all, "a", "a1").await;
        all.close_all().await.unwrap();
    }
}

#[tokio::test]
async fn h47_disabling_a_retires_only_its_last_owner_not_a_shared_plugin() {
    for shared in [false, true] {
        let fixture = GatedServers::new();
        let service = service();
        fixture.put(&service, "a", "a1");
        fixture.open("b1");
        fixture.put(&service, "b", "b1");
        if shared {
            service
                .reconcile_owner("plugin:test", BTreeSet::from(["a".into()]))
                .await
                .unwrap();
        }
        service.start_preparation();
        fixture.entered("a1").await;
        #[cfg(windows)]
        let process = fixture.witness("a1");
        ready(&service, "b", "b1").await;
        assert!(service.set_enabled("a", false).unwrap());
        service.start_preparation();
        assert!(service.ready_registry().find_tool("mcp__a__echo").is_none());
        if !shared {
            #[cfg(windows)]
            process.exited().await;
            fixture.open("a1");
        } else {
            fixture.open("a1");
            let plugin = service
                .prepare_server_with_cancel("a", fixture.config("a1"), None, false)
                .await
                .unwrap();
            assert_eq!(fixture.starts("a1"), 1);
            assert_echo(&plugin, "a", "a1").await;
            service
                .reconcile_owner("plugin:test", BTreeSet::new())
                .await
                .unwrap();
            // Retiring a cache owner must not close an already-borrowed client.
            assert_echo(&plugin, "a", "a1").await;
            plugin.close_all().await.unwrap();
        }
        assert!(service.ready_registry().find_tool("mcp__a__echo").is_none());
        let b = ready(&service, "b", "b1").await;
        assert_echo(&b, "b", "b1").await;
        assert_eq!(fixture.starts("b1"), 1);
        b.close_all().await.unwrap();
    }
}

#[tokio::test]
async fn h47_replacing_a_stops_old_pending_identity_without_stale_publication() {
    let fixture = GatedServers::new();
    let service = service();
    fixture.put(&service, "a", "a1");
    fixture.put(&service, "b", "b1");
    fixture.open("b1");
    service.start_preparation();
    fixture.entered("a1").await;
    #[cfg(windows)]
    let process = fixture.witness("a1");
    ready(&service, "b", "b1").await;
    fixture.put(&service, "a", "a2");
    service.start_preparation();
    fixture.entered("a2").await;
    #[cfg(windows)]
    process.exited().await;
    fixture.open("a1");
    fixture.open("a2");
    let all = ready(&service, "a", "a2").await;
    assert_echo(&all, "a", "a2").await;
    assert_echo(&all, "b", "b1").await;
    assert_eq!(fixture.starts("a1"), 1);
    assert_eq!(fixture.starts("a2"), 1);
    assert_eq!(fixture.starts("b1"), 1);
    all.close_all().await.unwrap();
}

#[tokio::test]
async fn h47_real_failure_remains_cached_until_explicit_retry() {
    let fixture = GatedServers::new();
    let service = service();
    let mut missing = fixture.config("never-started");
    missing.command = Some("deepagent-missing-mcp-test-binary".into());
    missing.args.clear();
    service
        .upsert(McpServerDto::from_config("a", &missing, true))
        .unwrap();
    service.start_preparation();
    wait_for(|| service.cached_status("a").status == "failed").await;
    let before = service.cached_status("a").error.unwrap();
    assert!(!before.contains("cancelled"));
    fixture.open("b1");
    fixture.put(&service, "b", "b1");
    service.start_preparation();
    let b = ready(&service, "b", "b1").await;
    assert_eq!(
        service.cached_status("a").error.as_deref(),
        Some(before.as_str())
    );
    let attempts = service.preparation.startup_attempts.load(Ordering::Acquire);
    assert!(service
        .prepare_server_with_cancel("a", missing.clone(), None, false)
        .await
        .is_err());
    assert_eq!(
        service.preparation.startup_attempts.load(Ordering::Acquire),
        attempts
    );
    assert!(service
        .prepare_server_with_cancel("a", missing, None, true)
        .await
        .is_err());
    assert_eq!(
        service.preparation.startup_attempts.load(Ordering::Acquire),
        attempts + 1
    );
    b.close_all().await.unwrap();
}

#[tokio::test]
async fn h47_real_plugin_claims_pending_identity_before_waiting_and_retires_it_on_disable() {
    use crate::plugin_loader::{load_plugins, PluginRoots};
    use crate::PluginService;

    let fixture = GatedServers::new();
    let root = fixture.root.path();
    let plugin = root.join("builtin/h47-plugin");
    std::fs::create_dir_all(plugin.join(".codex-plugin")).unwrap();
    std::fs::write(
        plugin.join(".codex-plugin/plugin.json"),
        r#"{"name":"h47-plugin","version":"0.1.0","mcpServers":".mcp.json"}"#,
    )
    .unwrap();
    let cfg = fixture.config("plugin1");
    std::fs::write(
        plugin.join(".mcp.json"),
        serde_json::json!({"mcpServers":{"shared":{
            "command": cfg.command, "args": cfg.args, "cwd": cfg.cwd
        }}})
        .to_string(),
    )
    .unwrap();
    let roots = PluginRoots {
        session: Vec::new(),
        builtin: root.join("builtin"),
        workspace: None,
        personal: root.join("personal"),
    };
    let loaded = load_plugins(&roots);
    assert_eq!(loaded.len(), 1);
    let id = loaded[0].id.clone();
    let plugins = Arc::new(PluginService::new(roots, root.join("plugin-data")));
    let service = service();
    plugins.start_preparation(service.clone(), Arc::new(|| {}));
    fixture.entered("plugin1").await;
    let names: Vec<_> = service
        .preparation
        .slots
        .lock()
        .unwrap()
        .keys()
        .cloned()
        .collect();
    assert_eq!(names.len(), 1);
    let name = &names[0];
    assert!(service.preparation.owners.lock().unwrap()[&format!("plugin:{id}")].contains(name));
    service
        .reconcile_owner("host", BTreeSet::from([name.clone()]))
        .await
        .unwrap();
    service
        .reconcile_owner("host", BTreeSet::new())
        .await
        .unwrap();
    fixture.open("plugin1");
    let tool = deepagent_mcp::registry::namespaced_name(name, "echo");
    wait_for(|| plugins.ready_snapshot().1.find_tool(&tool).is_some()).await;
    let borrowed = plugins.ready_snapshot().1;
    assert_eq!(fixture.starts("plugin1"), 1);
    assert_echo(&borrowed, name, "plugin1").await;
    let (projection, plugin_registry) = plugins.ready_snapshot();
    assert_attached_echo(
        &service,
        Some(&projection),
        Some(plugin_registry),
        name,
        "plugin1",
    )
    .await;
    plugins.set_enabled(&id, false).unwrap();
    wait_for(|| !service.preparation.slots.lock().unwrap().contains_key(name)).await;
    assert!(plugins.ready_snapshot().1.find_tool(&tool).is_none());
    let (projection, current) = plugins.ready_snapshot();
    let mut tools = deepagent_tools::ToolRegistry::new();
    let fresh = crate::mcp_runtime::attach_mcp_tools(
        &mut tools,
        Some(&service),
        Some(&projection),
        Some(current),
        None,
    )
    .unwrap();
    assert!(fresh.lifecycle.iter().all(|entry| entry.server_id != *name));
    assert_echo(&borrowed, name, "plugin1").await;
    borrowed.close_all().await.unwrap();
}

#[tokio::test]
async fn h47_retired_queued_startup_never_spawns_after_budget_is_released() {
    let fixture = GatedServers::new();
    let service = service();
    let first = service
        .preparation
        .local
        .clone()
        .acquire_owned()
        .await
        .unwrap();
    let second = service
        .preparation
        .local
        .clone()
        .acquire_owned()
        .await
        .unwrap();
    let cancel = deepagent_runtime::CancellationTree::new();
    let pending = tokio::spawn({
        let service = service.clone();
        let config = fixture.config("queued");
        let cancel = cancel.clone();
        async move {
            service
                .prepare_server_with_cancel("a", config, Some(cancel), false)
                .await
        }
    });
    wait_for(|| service.preparation.slots.lock().unwrap().contains_key("a")).await;
    cancel.cancel();
    assert!(pending.await.unwrap().is_err());
    service.retire_server("a").await.unwrap();
    drop((first, second));
    fixture.put(&service, "b", "b1");
    fixture.open("b1");
    service.start_preparation();
    let b = ready(&service, "b", "b1").await;
    assert_eq!(fixture.starts("queued"), 0);
    assert_eq!(
        service.preparation.startup_attempts.load(Ordering::Acquire),
        1
    );
    b.close_all().await.unwrap();
}

#[tokio::test]
async fn h47_unrelated_view_keeps_the_real_startup_timeout_and_caches_its_failure() {
    let fixture = GatedServers::new();
    let service = service();
    fixture.put(&service, "a", "a1");
    service.start_preparation();
    fixture.entered("a1").await;
    #[cfg(windows)]
    let process = fixture.witness("a1");
    fixture.put(&service, "b", "b1");
    fixture.open("b1");
    service.start_preparation();
    let b = ready(&service, "b", "b1").await;
    assert_eq!(service.cached_status("a").status, "preparing");
    tokio::time::timeout(MCP_STARTUP_TIMEOUT + Duration::from_secs(10), async {
        while service.cached_status("a").status == "preparing" {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the real production startup deadline must settle the unchanged identity");
    let status = service.cached_status("a");
    assert_eq!(status.status, "failed");
    assert!(status
        .error
        .unwrap()
        .contains("MCP startup timed out after 30 seconds"));
    #[cfg(windows)]
    process.exited().await;
    assert_eq!(fixture.starts("a1"), 1);
    assert!(service
        .prepare_server_with_cancel("a", fixture.config("a1"), None, false)
        .await
        .is_err());
    assert_eq!(
        fixture.starts("a1"),
        1,
        "a deadline is a cached real failure, not a waiter cancellation"
    );
    assert_echo(&b, "b", "b1").await;
    b.close_all().await.unwrap();
}
