//! Explicit Windows process-ownership probe; no external MCP or model is used.
#![cfg(windows)]

use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use deepagent_mcp::protocol::JsonRpcRequest;
use deepagent_mcp::{
    connect_transport, ConfigReconnectFactory, LivenessConfig, McpClient, McpRegistry,
    McpServerConfig, McpTransport, ReconnectFactory, StdioTransport,
};
use deepagent_tools::ToolExecutionContext;
use windows_sys::Win32::Foundation::{WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows_sys::Win32::System::Threading::{
    OpenProcess, TerminateProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE,
};

// Own only this fixture's freshly reported PID. The worker also has an 8s
// watchdog, so an unexpected panic before reading its PID is bounded.
struct OwnedFixtureProcess {
    pid: u32,
    handle: OwnedHandle,
}

impl OwnedFixtureProcess {
    fn open(pid: u32) -> Self {
        let handle = unsafe { OpenProcess(PROCESS_SYNCHRONIZE | PROCESS_TERMINATE, 0, pid) };
        assert!(
            !handle.is_null(),
            "open fixture {pid}: {}",
            std::io::Error::last_os_error()
        );
        Self {
            pid,
            // SAFETY: OpenProcess returned a fresh, valid owned handle.
            handle: unsafe { OwnedHandle::from_raw_handle(handle.cast()) },
        }
    }

    fn alive(&self) -> bool {
        let state = unsafe { WaitForSingleObject(self.handle.as_raw_handle().cast(), 0) };
        assert!(matches!(state, WAIT_OBJECT_0 | WAIT_TIMEOUT));
        state == WAIT_TIMEOUT
    }

    async fn wait_for_exit(&self) {
        tokio::time::timeout(Duration::from_secs(3), async {
            while self.alive() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("fixture process must exit after its owned cleanup");
    }
}

impl Drop for OwnedFixtureProcess {
    fn drop(&mut self) {
        // Keep a handle, not just a PID: cleanup cannot target a reused PID.
        unsafe { TerminateProcess(self.handle.as_raw_handle().cast(), 1) };
    }
}

struct Fixture {
    _dir: tempfile::TempDir,
    script: PathBuf,
    dotnet_script: PathBuf,
    ready: PathBuf,
    heartbeat: PathBuf,
    requests: PathBuf,
}

struct FixtureOwner {
    root: OwnedFixtureProcess,
    worker: OwnedFixtureProcess,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("mcp-process-owner.cjs");
        let ready = dir.path().join("worker-ready.json");
        let heartbeat = dir.path().join("worker-heartbeat");
        let requests = dir.path().join("requests.jsonl");
        std::fs::write(
            &script,
            r#"
const fs = require('node:fs');
const [mode, ready, heartbeat, requests] = process.argv.slice(2);
if (mode === 'worker') {
  fs.appendFileSync(heartbeat, '.');
  fs.writeFileSync(ready, JSON.stringify({ parent: process.ppid, worker: process.pid }));
  setInterval(() => fs.appendFileSync(heartbeat, '.'), 20);
  setTimeout(() => process.exit(0), 8000);
} else {
  const worker = require('node:child_process').spawn(process.execPath,
    [__filename, 'worker', ready, heartbeat],
    { detached: false, stdio: 'ignore', windowsHide: true });
  worker.unref();
  require('node:readline').createInterface({ input: process.stdin }).on('line', line => {
    const request = JSON.parse(line);
    fs.appendFileSync(requests, JSON.stringify(request) + '\n');
    if (request.id === undefined) return;
    if (request.method === 'initialize' && process.env.H46_HANG_INITIALIZE === '1') return;
    if (request.method === 'tools/call' && request.params.name === 'stall') return;
    let result = { server_pid: process.pid };
    if (request.method === 'initialize') result = { protocolVersion: '2024-11-05',
      capabilities: { tools: {} }, serverInfo: { name: 'ownership-fixture', version: '1' } };
    if (request.method === 'tools/list') result = { tools: ['echo', 'stall'].map(name =>
      ({ name, description: name, inputSchema: { type: 'object' } })) };
    if (request.method === 'tools/call') result = {
      content: [{ type: 'text', text: String(process.pid) }], isError: false };
    process.stdout.write(JSON.stringify({ jsonrpc: '2.0', id: request.id, result }) + '\n',
      () => { if (request.method === 'exit-root') process.exit(0); });
  });
  setTimeout(() => process.exit(0), 12000).unref();
}
"#,
        )
        .unwrap();
        let dotnet_script = dir.path().join("dotnet-mcp-server.ps1");
        std::fs::write(
            &dotnet_script,
            r#"
param([string]$WorkerScript, [string]$ReadyPath, [string]$HeartbeatPath, [string]$RequestsPath)
$ErrorActionPreference = 'Stop'
$startInfo = New-Object System.Diagnostics.ProcessStartInfo
$startInfo.FileName = (Get-Command node.exe).Source
$startInfo.Arguments = '"' + $WorkerScript + '" worker "' + $ReadyPath + '" "' + $HeartbeatPath + '"'
$startInfo.UseShellExecute = $false
$startInfo.CreateNoWindow = $true
$startInfo.RedirectStandardOutput = $true
$startInfo.RedirectStandardError = $true
$worker = [System.Diagnostics.Process]::Start($startInfo)
while ($null -ne ($line = [Console]::ReadLine())) {
  $request = ConvertFrom-Json -InputObject $line
  [IO.File]::AppendAllText($RequestsPath, ($request | ConvertTo-Json -Compress -Depth 20) + "`n")
  if ($null -eq $request.id) { continue }
  if ($request.method -eq 'initialize' -and $env:H46_HANG_INITIALIZE -eq '1') { continue }
  if ($request.method -eq 'tools/call' -and $request.params.name -eq 'stall') { continue }
  $result = @{ server_pid = $PID }
  if ($request.method -eq 'initialize') {
    $result = @{ protocolVersion = '2024-11-05'; capabilities = @{ tools = @{} };
      serverInfo = @{ name = 'ownership-fixture'; version = '1' } }
  }
  if ($request.method -eq 'tools/list') {
    $result = @{ tools = @('echo', 'stall' | ForEach-Object {
      @{ name = $_; description = $_; inputSchema = @{ type = 'object' } }
    }) }
  }
  if ($request.method -eq 'tools/call') {
    $result = @{ content = @(@{ type = 'text'; text = [string]$PID }); isError = $false }
  }
  [Console]::Out.WriteLine((@{ jsonrpc = '2.0'; id = $request.id; result = $result } | ConvertTo-Json -Compress -Depth 20))
  [Console]::Out.Flush()
  if ($request.method -eq 'exit-root') { [Environment]::Exit(0) }
}
"#,
        )
        .unwrap();
        Self {
            _dir: dir,
            script,
            dotnet_script,
            ready,
            heartbeat,
            requests,
        }
    }

    fn config(&self, launcher: &str, hang_initialize: bool) -> McpServerConfig {
        let mut args = if launcher == "dotnet" {
            vec![
                "-NoProfile".into(),
                "-NonInteractive".into(),
                "-File".into(),
                self.dotnet_script.to_string_lossy().into_owned(),
                self.script.to_string_lossy().into_owned(),
            ]
        } else {
            vec![self.script.to_string_lossy().into_owned(), "server".into()]
        };
        args.extend([
            self.ready.to_string_lossy().into_owned(),
            self.heartbeat.to_string_lossy().into_owned(),
            self.requests.to_string_lossy().into_owned(),
        ]);
        McpServerConfig {
            transport: None,
            command: Some(
                if launcher == "dotnet" {
                    "powershell.exe"
                } else {
                    "node"
                }
                .into(),
            ),
            args,
            env: [(
                "H46_HANG_INITIALIZE".into(),
                if hang_initialize { "1" } else { "0" }.into(),
            )]
            .into(),
            cwd: None,
            url: None,
            headers: Default::default(),
        }
    }

    async fn owner(&self) -> FixtureOwner {
        let pids: serde_json::Value = tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if let Some(value) = std::fs::read(&self.ready)
                    .ok()
                    .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
                {
                    break value;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the ordinary descendant must report actual startup");
        let parent = pids["parent"].as_u64().unwrap() as u32;
        let worker = pids["worker"].as_u64().unwrap() as u32;
        FixtureOwner {
            root: OwnedFixtureProcess::open(parent),
            worker: OwnedFixtureProcess::open(worker),
        }
    }

    fn heartbeat_len(&self) -> u64 {
        std::fs::metadata(&self.heartbeat).unwrap().len()
    }

    async fn wait_request(&self, method: &str, tool: Option<&str>) {
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let lines = std::fs::read_to_string(&self.requests).unwrap_or_default();
                if lines
                    .lines()
                    .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
                    .any(|request| {
                        request["method"] == method
                            && match tool {
                                Some(name) => request["params"]["name"] == name,
                                None => true,
                            }
                    })
                {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("server must observe the real wire request");
    }

    async fn assert_heartbeat_advances(&self) {
        let before = self.heartbeat_len();
        tokio::time::timeout(Duration::from_secs(1), async {
            while self.heartbeat_len() <= before {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the live worker must still execute");
    }

    async fn assert_stopped(&self, owner: &FixtureOwner) {
        owner.root.wait_for_exit().await;
        owner.worker.wait_for_exit().await;
        let before = self.heartbeat_len();
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(
            self.heartbeat_len(),
            before,
            "no writes after owner release"
        );
    }

    async fn registry(&self) -> McpRegistry {
        let client = Arc::new(McpClient::new(
            connect_transport(&self.config("dotnet", false)).unwrap(),
        ));
        client.initialize("h46-lifecycle").await.unwrap();
        let mut registry = McpRegistry::new();
        assert_eq!(registry.register("fixture", client).await.unwrap(), 2);
        registry.enable_liveness(LivenessConfig {
            interval: Duration::from_millis(20),
            timeout: Duration::from_secs(1),
            failure_limit: 2,
        });
        registry
    }
}

#[tokio::test]
async fn audit_stdio_close_and_drop_terminate_owned_descendants() {
    let mut observations = Vec::new();
    for (launcher, explicit_close) in [
        ("node", true),
        ("node", false),
        ("dotnet", true),
        ("dotnet", false),
    ] {
        let fixture = Fixture::new();
        let transport = StdioTransport::spawn(&fixture.config(launcher, false)).unwrap();
        let owner = fixture.owner().await;
        assert!(owner.root.alive() && owner.worker.alive());
        let response = tokio::time::timeout(
            Duration::from_secs(2),
            transport.send(&JsonRpcRequest::new(1, "ping", None)),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(response.result.unwrap()["server_pid"], owner.root.pid);
        if explicit_close {
            transport.close().await.unwrap();
        }
        drop(transport);
        owner.root.wait_for_exit().await;
        let before = fixture.heartbeat_len();
        tokio::time::sleep(Duration::from_millis(250)).await;
        let after = fixture.heartbeat_len();
        observations.push(serde_json::json!({
            "launcher": launcher,
            "explicit_close": explicit_close,
            "parent_exited": !owner.root.alive(),
            "descendant_alive": owner.worker.alive(),
            "heartbeats_after_parent_exit": after.saturating_sub(before),
        }));
        // Clean both cases before asserting the invariant known to fail.
        if owner.worker.alive() {
            unsafe { TerminateProcess(owner.worker.handle.as_raw_handle().cast(), 1) };
        }
        owner.worker.wait_for_exit().await;
    }
    eprintln!("{}", serde_json::to_string_pretty(&observations).unwrap());
    assert!(
        observations.iter().all(|o| o["descendant_alive"] == false),
        "closing or dropping the MCP owner must not leave its descendant running"
    );
}

#[tokio::test]
async fn initialize_cancellation_releases_the_whole_service() {
    for launcher in ["node", "dotnet"] {
        let fixture = Fixture::new();
        let factory = ConfigReconnectFactory::new(fixture.config(launcher, true), "h46-init");
        let initialize = tokio::spawn(async move { factory.reconnect().await });
        let owner = fixture.owner().await;
        fixture.wait_request("initialize", None).await;
        fixture.assert_heartbeat_advances().await;
        assert!(
            !initialize.is_finished(),
            "initialize must actually be pending"
        );
        initialize.abort();
        assert!(initialize.await.is_err_and(|error| error.is_cancelled()));
        fixture.assert_stopped(&owner).await;
    }
}

#[tokio::test]
async fn root_exit_does_not_release_descendants_before_owner_close_or_drop() {
    for explicit_close in [true, false] {
        let fixture = Fixture::new();
        let transport = StdioTransport::spawn(&fixture.config("dotnet", false)).unwrap();
        let owner = fixture.owner().await;
        transport
            .send(&JsonRpcRequest::new(1, "exit-root", None))
            .await
            .unwrap();
        owner.root.wait_for_exit().await;
        assert!(
            owner.worker.alive(),
            "ordinary .NET descendant must outlive its root"
        );
        fixture.assert_heartbeat_advances().await;
        if explicit_close {
            transport.close().await.unwrap();
            transport.close().await.unwrap(); // explicit shutdown is idempotent
        }
        drop(transport);
        fixture.assert_stopped(&owner).await;
    }
}

async fn assert_echo(registry: &McpRegistry, expected_pid: u32) {
    let result = registry
        .invoke("mcp__fixture__echo", serde_json::json!({}))
        .await
        .unwrap();
    assert!(!result.is_error);
    assert!(
        matches!(&result.content[0], deepagent_mcp::protocol::ContentBlock::Text { text, .. }
        if text == &expected_pid.to_string())
    );
}

#[tokio::test]
async fn shared_snapshots_keep_old_service_until_last_reference_without_killing_replacement() {
    let old_fixture = Fixture::new();
    let old = old_fixture.registry().await;
    let old_owner = old_fixture.owner().await;
    let snapshot = old.clone();
    let mut published = McpRegistry::new();
    published.extend_ready(&old).unwrap();
    drop(old);
    old_fixture.wait_request("ping", None).await;
    assert_echo(&snapshot, old_owner.root.pid).await;

    let new_fixture = Fixture::new();
    let new = new_fixture.registry().await;
    let new_owner = new_fixture.owner().await;
    published.retain_servers(|_| false);
    published.extend_ready(&new).unwrap();
    drop(new);
    assert_echo(&snapshot, old_owner.root.pid).await;
    assert_echo(&published, new_owner.root.pid).await;
    old_fixture.assert_heartbeat_advances().await;
    drop(snapshot);
    old_fixture.assert_stopped(&old_owner).await;
    assert_echo(&published, new_owner.root.pid).await;
    new_fixture.assert_heartbeat_advances().await;
    drop(published);
    new_fixture.assert_stopped(&new_owner).await;
}

#[tokio::test]
async fn cancelling_one_call_does_not_kill_the_shared_service() {
    let fixture = Fixture::new();
    let registry = fixture.registry().await;
    let owner = fixture.owner().await;
    let other = registry.clone();
    let context = ToolExecutionContext::default();
    let token = context.cancellation_token();
    let call = tokio::spawn(async move {
        other
            .invoke_with_context("mcp__fixture__stall", serde_json::json!({}), context)
            .await
    });
    fixture.wait_request("tools/call", Some("stall")).await;
    assert!(!call.is_finished());
    token.cancel();
    let error = tokio::time::timeout(Duration::from_secs(2), call)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert!(error.to_string().contains("cancel"));
    assert_echo(&registry, owner.root.pid).await;
    assert!(owner.root.alive() && owner.worker.alive());
    fixture.assert_heartbeat_advances().await;
    drop(registry);
    fixture.assert_stopped(&owner).await;
}
