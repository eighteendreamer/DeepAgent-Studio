//! H-211 contract controls: a response must echo the ID this client issued.
use super::*;
use std::time::Duration;

const SCRIPT: &str = r#"
$mode = '$MODE'
$held = $null
$listCalls = 0
function Send($value) { [Console]::Out.WriteLine(($value | ConvertTo-Json -Compress -Depth 10)) }
while (($line = [Console]::In.ReadLine()) -ne $null) {
  $request = $line | ConvertFrom-Json
  if ($request.method -eq 'initialize') {
    Send @{jsonrpc='2.0';id=$request.id;result=@{protocolVersion=1;serverName='id-contract';serverVersion='1';capabilities=@{threadLifecycle=$true;streaming=$true;interrupt=$true;steer=$true;approval=$true;reconnect=$true}}}
  } elseif ($request.method -eq 'thread/list') {
    $listCalls++
    $held = $request
    # Same text, different JSON type: this is NOT a reply to the numeric ID.
    Send @{jsonrpc='2.0';id=[string]$request.id;result=@{source='wrong-type'}}
    Send @{jsonrpc='2.0';id=999;result=@{source='unknown-id'}}
    # The SDK reader sees this only after processing both preceding frames.
    Send @{jsonrpc='2.0';method='harness/event';params=@{type='thread.started';threadId='id-barrier';protocolVersion=1}}
  } elseif ($request.method -eq 'tool/list' -and $null -ne $held) {
    # Respond in reverse request order; neither unknown IDs nor a duplicate
    # response may be associated with the other outstanding request.
    Send @{jsonrpc='2.0';id=$request.id;result=@{source='tool/list';listCalls=$listCalls}}
    if ($mode -eq 'error') {
      Send @{jsonrpc='2.0';id=$held.id;error=@{code=-32004;message='synthetic matched error'}}
    } else {
      Send @{jsonrpc='2.0';id=$held.id;result=@{source='thread/list';wireId=$held.id}}
    }
    Send @{jsonrpc='2.0';id=$held.id;result=@{source='duplicate'}}
  } else { throw 'unexpected synthetic request' }
}
"#;

#[tokio::test]
async fn h211_actual_numeric_ids_match_without_coercing_foreign_string_ids() {
    for mode in ["result", "error"] {
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
        let ready = tokio::time::timeout(Duration::from_secs(5), client.initialize("h211", "1"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(ready.id, serde_json::json!(1));
        let mut events = client.subscribe();
        let pending_client = client.clone();
        let list = tokio::spawn(async move { pending_client.thread_list().await });
        let barrier = tokio::time::timeout(Duration::from_secs(5), events.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(
            matches!(barrier.event, HarnessEvent::ThreadStarted { thread_id, .. } if thread_id == "id-barrier")
        );
        assert!(
            !list.is_finished(),
            "string/unknown IDs cannot complete ID 2"
        );
        assert_eq!(client.pending.lock().unwrap().len(), 1);

        let tools = tokio::time::timeout(Duration::from_secs(5), client.tool_list())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(tools.id, serde_json::json!(3));
        assert_eq!(tools.result()["source"], "tool/list");
        assert_eq!(tools.result()["listCalls"], 1, "no automatic redispatch");
        let list = tokio::time::timeout(Duration::from_secs(5), list)
            .await
            .unwrap()
            .unwrap();
        if mode == "error" {
            assert!(matches!(list, Err(SdkError::Rpc { code: -32004, .. })));
        } else {
            let list = list.unwrap();
            assert_eq!(list.id, serde_json::json!(2));
            assert_eq!(list.result()["source"], "thread/list");
            assert_eq!(list.result()["wireId"], serde_json::json!(2));
        }
        assert!(client.pending.lock().unwrap().is_empty());
        client.close().await;
        assert!(tokio::time::timeout(Duration::from_secs(5), child.wait())
            .await
            .unwrap()
            .unwrap()
            .success());
        eprintln!("H-211 {mode}: issued numeric IDs 1/2/3, wrong string ignored, exact reverse replies settled, pending=0");
    }
}
