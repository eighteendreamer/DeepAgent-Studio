//! Explicit Windows process-ownership probe; no external MCP or model is used.
#![cfg(windows)]

use std::os::windows::process::CommandExt;
use std::process::Command;
use std::time::Duration;

use deepagent_mcp::protocol::JsonRpcRequest;
use deepagent_mcp::{McpServerConfig, McpTransport, StdioTransport};

fn node_command() -> Command {
    let mut command = Command::new("node");
    command.creation_flags(0x08000000);
    command
}

fn process_alive(pid: u32) -> bool {
    let output = node_command()
        .args([
            "-e",
            "try { process.kill(Number(process.argv[1]), 0); } catch (e) { process.exit(e.code === 'ESRCH' ? 1 : 2); }",
        ])
        .arg(pid.to_string())
        .output()
        .unwrap();
    assert!(matches!(output.status.code(), Some(0 | 1)));
    output.status.success()
}

// Own only this fixture's freshly reported PID. The worker also has an 8s
// watchdog, so an unexpected panic before reading its PID is bounded.
struct OwnedFixtureProcess(u32);

impl Drop for OwnedFixtureProcess {
    fn drop(&mut self) {
        let _ = node_command()
            .args([
                "-e",
                "try { process.kill(Number(process.argv[1]), 'SIGKILL'); } catch (e) { if (e.code !== 'ESRCH') process.exitCode = 1; }",
            ])
            .arg(self.0.to_string())
            .status();
    }
}

async fn wait_for_exit(pid: u32) {
    tokio::time::timeout(Duration::from_secs(3), async {
        while process_alive(pid) {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("fixture process must exit after its owned cleanup");
}

#[tokio::test]
#[ignore = "audit: shutting down a local MCP must also stop its ordinary descendant"]
async fn audit_stdio_close_and_drop_terminate_owned_descendants() {
    let mut observations = Vec::new();
    for (launcher, explicit_close) in [
        ("node", true),
        ("node", false),
        ("dotnet", true),
        ("dotnet", false),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("mcp-process-owner.cjs");
        let ready = dir.path().join("worker-ready.json");
        let heartbeat = dir.path().join("worker-heartbeat");
        std::fs::write(
            &script,
            r#"
const fs = require('node:fs');
const [mode, ready, heartbeat] = process.argv.slice(2);
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
    process.stdout.write(JSON.stringify({ jsonrpc: '2.0', id: request.id, result: { server_pid: process.pid } }) + '\n');
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
param([string]$WorkerScript, [string]$ReadyPath, [string]$HeartbeatPath)
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
  [Console]::Out.WriteLine((@{ jsonrpc = '2.0'; id = $request.id; result = @{ server_pid = $PID } } | ConvertTo-Json -Compress))
}
"#,
        )
        .unwrap();
        let mut args = if launcher == "dotnet" {
            vec![
                "-NoProfile".into(),
                "-NonInteractive".into(),
                "-File".into(),
                dotnet_script.to_string_lossy().into_owned(),
                script.to_string_lossy().into_owned(),
            ]
        } else {
            vec![script.to_string_lossy().into_owned(), "server".into()]
        };
        args.extend([
            ready.to_string_lossy().into_owned(),
            heartbeat.to_string_lossy().into_owned(),
        ]);
        let config = McpServerConfig {
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
            env: Default::default(),
            cwd: None,
            url: None,
            headers: Default::default(),
        };
        let transport = StdioTransport::spawn(&config).unwrap();
        let pids: serde_json::Value = tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if let Some(value) = std::fs::read(&ready)
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
        let cleanup = OwnedFixtureProcess(worker);
        assert!(process_alive(parent) && process_alive(worker));
        let response = tokio::time::timeout(
            Duration::from_secs(2),
            transport.send(&JsonRpcRequest::new(1, "ping", None)),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(response.result.unwrap()["server_pid"], parent);
        if explicit_close {
            transport.close().await.unwrap();
        }
        drop(transport);
        wait_for_exit(parent).await;
        let before = std::fs::metadata(&heartbeat).unwrap().len();
        tokio::time::sleep(Duration::from_millis(250)).await;
        let after = std::fs::metadata(&heartbeat).unwrap().len();
        observations.push(serde_json::json!({
            "launcher": launcher,
            "explicit_close": explicit_close,
            "parent_exited": !process_alive(parent),
            "descendant_alive": process_alive(worker),
            "heartbeats_after_parent_exit": after.saturating_sub(before),
        }));
        // Clean both cases before asserting the invariant known to fail.
        drop(cleanup);
        wait_for_exit(worker).await;
    }
    eprintln!("{}", serde_json::to_string_pretty(&observations).unwrap());
    assert!(
        observations.iter().all(|o| o["descendant_alive"] == false),
        "closing or dropping the MCP owner must not leave its descendant running"
    );
}
