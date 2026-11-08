//! H-48: closing during a real replacement handshake reaps that exact process.
#![cfg(windows)]

use std::sync::Arc;
use std::time::Duration;

use deepagent_mcp::{
    ConfigReconnectFactory, McpClient, McpServerConfig, ReconnectingTransport, StdioTransport,
};

#[tokio::test]
async fn h48_close_during_real_recovery_reaps_the_unpublished_child() {
    let root = tempfile::tempdir().unwrap();
    let script = root.path().join("server.cjs");
    let starts = root.path().join("starts");
    let entered = root.path().join("handshake");
    std::fs::write(&script, r#"
const fs = require('node:fs');
const [starts, entered] = process.argv.slice(2);
const first = !fs.existsSync(starts);
fs.appendFileSync(starts, process.pid + '\n');
setInterval(() => {}, 1000); // EOF alone cannot make the proof pass.
require('node:readline').createInterface({input:process.stdin}).on('line', line => {
  const msg = JSON.parse(line);
  if (msg.id === undefined) return;
  if (!first) { fs.writeFileSync(entered, String(process.pid)); return; }
  if (msg.method === 'ping') process.exit(19);
  process.stdout.write(JSON.stringify({jsonrpc:'2.0',id:msg.id,result:{
    protocolVersion:msg.params.protocolVersion,capabilities:{tools:{}},serverInfo:{name:'synthetic',version:'1'}
  }}) + '\n');
});
"#).unwrap();
    let config = McpServerConfig {
        response_limit_bytes: None,
        transport: None,
        command: Some("node".into()),
        args: vec![
            script.display().to_string(),
            starts.display().to_string(),
            entered.display().to_string(),
        ],
        env: Default::default(),
        cwd: Some(root.path().to_path_buf()),
        url: None,
        headers: Default::default(),
    };
    let transport = Arc::new(
        ReconnectingTransport::new(
            Arc::new(StdioTransport::spawn(&config).unwrap()),
            Arc::new(ConfigReconnectFactory::new(config, "h48-real-close")),
        )
        .with_backoff(vec![Duration::ZERO; 3]),
    );
    let client = Arc::new(McpClient::new(transport));
    client.initialize("h48-real-close").await.unwrap();
    let pending = tokio::spawn({
        let client = client.clone();
        async move { client.ping().await }
    });
    tokio::time::timeout(Duration::from_secs(5), async {
        while !entered.exists() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use windows_sys::Win32::Foundation::WAIT_OBJECT_0;
    use windows_sys::Win32::System::Threading::{
        OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE,
    };
    let pid = std::fs::read_to_string(&entered).unwrap().parse().unwrap();
    let handle = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
    assert!(!handle.is_null(), "{}", std::io::Error::last_os_error());
    let witness = unsafe { OwnedHandle::from_raw_handle(handle.cast()) };
    tokio::time::timeout(Duration::from_secs(2), client.close())
        .await
        .unwrap()
        .unwrap();
    assert!(tokio::time::timeout(Duration::from_secs(2), pending)
        .await
        .unwrap()
        .unwrap()
        .is_err());
    tokio::time::timeout(Duration::from_secs(5), async {
        while unsafe { WaitForSingleObject(witness.as_raw_handle().cast(), 0) } != WAIT_OBJECT_0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the unpublished replacement must actually exit");
    assert_eq!(std::fs::read_to_string(&starts).unwrap().lines().count(), 2);
    assert!(client.ping().await.is_err());
    assert_eq!(std::fs::read_to_string(&starts).unwrap().lines().count(), 2);
}
