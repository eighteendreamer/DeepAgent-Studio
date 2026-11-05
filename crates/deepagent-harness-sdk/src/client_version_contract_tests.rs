//! H-212 real-pipe version rejection before pending/initialization mutation.
use super::*;
use std::time::Duration;

const SCRIPT: &str = r#"
$mode = '$MODE'
$held = $null
$calls = 0
function Send($value) { [Console]::Out.WriteLine(($value | ConvertTo-Json -Compress -Depth 10)) }
function Noise($id) {
  foreach ($version in @('1.0','2','2.0 ','','2.O',2,$null,$false,@(),@{})) {
    Send @{jsonrpc=$version;id=$id;result=@{protocolVersion=1;serverName='wrong-version';serverVersion='1'}}
    Send @{jsonrpc=$version;id=$id;error=@{code=-32004;message='wrong-version error'}}
  }
  Send @{id=$id;result=@{}}
  Send @{id=$id;error=@{code=-32004;message='missing version'}}
}
function Barrier { Send @{jsonrpc='2.0';method='harness/event';params=@{type='thread.started';threadId='version-barrier';protocolVersion=1}} }
while (($line = [Console]::In.ReadLine()) -ne $null) {
  $request = $line | ConvertFrom-Json
  if ($request.method -eq 'initialize') {
    if ($mode -like 'initialize-*') { Noise $request.id; Barrier }
    if ($mode -ne 'initialize-eof') {
      Send @{jsonrpc='2.0';id=$request.id;result=@{protocolVersion=1;serverName='valid-peer';serverVersion='1'}}
    }
  } elseif ($request.method -eq 'thread/list') {
    $calls++
    $held = $request
    Noise $request.id
    Barrier
  } elseif ($request.method -eq 'tool/list' -and $null -ne $held) {
    Send @{jsonrpc='2.0';id=$request.id;result=@{calls=$calls}}
    if ($mode -eq 'error') {
      Send @{jsonrpc='2.0';id=$held.id;error=@{code=-32004;message='valid matched error'}}
    } else {
      Send @{jsonrpc='2.0';id=$held.id;result=@{source='valid matched result'}}
    }
    Send @{jsonrpc='1.0';id=$held.id;result=@{source='late invalid duplicate'}}
  } else { throw 'unexpected synthetic method' }
}
"#;

async fn fixture(mode: &str) -> (Child, HarnessClient) {
    let mut child = tokio::process::Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            &SCRIPT.replace("$MODE", mode),
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let client = HarnessClient::from_stdio(
        child.stdin.take().unwrap(),
        child.stdout.take().unwrap(),
        child.stderr.take().unwrap(),
        8,
    );
    (child, client)
}

async fn barrier(events: &mut broadcast::Receiver<HarnessEvent>) {
    let event = tokio::time::timeout(Duration::from_secs(5), events.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(event, HarnessEvent::ThreadStarted { thread_id, .. } if thread_id == "version-barrier")
    );
}

async fn finish(mut child: Child, client: &HarnessClient) {
    client.close().await;
    assert!(tokio::time::timeout(Duration::from_secs(5), child.wait())
        .await
        .unwrap()
        .unwrap()
        .success());
}

#[tokio::test]
async fn h212_wrong_version_results_and_errors_preserve_pending_until_exact_valid_reply() {
    for mode in ["result", "error"] {
        let (child, client) = fixture(mode).await;
        client.initialize("h212", "1").await.unwrap();
        let mut events = client.subscribe();
        let caller = client.clone();
        let list = tokio::spawn(async move { caller.thread_list().await });
        barrier(&mut events).await;
        // The reader has consumed every invalid frame; no scheduling sleep is needed.
        assert_eq!(client.pending.lock().unwrap().len(), 1);
        assert!(!list.is_finished());
        let tools = client.tool_list().await.unwrap();
        assert_eq!(tools.result()["calls"], 1, "no redispatch");
        let result = tokio::time::timeout(Duration::from_secs(5), list)
            .await
            .unwrap()
            .unwrap();
        if mode == "error" {
            assert!(
                matches!(result, Err(SdkError::Rpc { code: -32004, message }) if message == "valid matched error")
            );
        } else {
            let response = result.unwrap();
            assert_eq!(response.jsonrpc, "2.0");
            assert_eq!(response.id, serde_json::json!(2));
            assert_eq!(response.result()["source"], "valid matched result");
        }
        assert!(client.pending.lock().unwrap().is_empty());
        finish(child, &client).await;
    }
}

#[tokio::test]
async fn h212_wrong_version_initialize_never_authorizes_calls_and_eof_releases_pending() {
    let (child, client) = fixture("initialize-eof").await;
    let mut events = client.subscribe();
    let caller = client.clone();
    let initialize = tokio::spawn(async move { caller.initialize("h212", "1").await });
    barrier(&mut events).await;
    assert!(!*client.initialized.lock().unwrap());
    assert_eq!(client.pending.lock().unwrap().len(), 1);
    assert!(!initialize.is_finished());
    assert!(matches!(
        client.thread_list().await,
        Err(SdkError::NotInitialized)
    ));
    finish(child, &client).await;
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(5), initialize)
            .await
            .unwrap()
            .unwrap(),
        Err(SdkError::ResponseChannelClosed { id: 1 })
    ));
    assert!(client.pending.lock().unwrap().is_empty());
    assert!(!*client.initialized.lock().unwrap());
}

#[tokio::test]
async fn h212_initialize_can_recover_only_on_the_later_valid_version_response() {
    let (child, client) = fixture("initialize-recover").await;
    let response = tokio::time::timeout(Duration::from_secs(5), client.initialize("h212", "1"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(response.jsonrpc, "2.0");
    assert_eq!(response.result()["serverName"], "valid-peer");
    assert!(*client.initialized.lock().unwrap());
    assert!(client.pending.lock().unwrap().is_empty());
    finish(child, &client).await;
}
