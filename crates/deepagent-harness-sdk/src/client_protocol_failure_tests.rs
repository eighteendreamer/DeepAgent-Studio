//! H-101: real pipes stay live after the bad frame; local failure must not wait for EOF.
use super::*;
use std::time::Duration;
use tokio::time::timeout;

async fn fixture(frame: &str, requests: usize) -> (Child, HarnessClient) {
    let script = format!(
        r#"
$count=0
while (($line=[Console]::In.ReadLine()) -ne $null) {{
 $count++
 if ($count -eq {requests}) {{
  [Console]::Out.WriteLine('{{"jsonrpc":"2.0","method":"harness/event","params":{{"type":"thread.started","threadId":"before-bad-frame","protocolVersion":1}}}}')
  [Console]::Out.WriteLine('{frame}')
 }}
 if ($count -gt {requests}) {{ exit 9 }}
}}
exit 0
"#,
        frame = frame.replace('\'', "''")
    );
    let mut child = tokio::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
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

fn decode(error: SdkError) -> String {
    assert!(
        matches!(error, SdkError::Decode(_)),
        "expected protocol decode failure, got {error:?}"
    );
    let text = error.to_string();
    assert!(
        !text.contains("PRIVATE_CANARY"),
        "peer payload must never enter diagnostics"
    );
    text
}

async fn finish(mut child: Child, client: &HarnessClient) {
    assert!(
        child.try_wait().unwrap().is_none(),
        "the failure must precede real child EOF"
    );
    client.close().await;
    assert!(
        timeout(Duration::from_secs(3), child.wait())
            .await
            .unwrap()
            .unwrap()
            .success(),
        "no late request/retry may be written"
    );
}

#[tokio::test]
async fn h101_bad_frames_settle_all_pending_and_retire_clones_without_replay() {
    for frame in [
        "not-json-PRIVATE_CANARY",
        r#"{"jsonrpc":"2.0","id":3,"error":"PRIVATE_CANARY"}"#,
        r#"{"jsonrpc":"2.0","params":{"private":"PRIVATE_CANARY"}}"#,
        // Exact notification envelope cases formerly mixed into H-213's
        // rejection-only fixture. Their business payload is still well formed.
        r#"{"jsonrpc":"2.0","params":{"type":"turn.completed","threadId":"A","turnId":"turn-A","message":"first terminal","reason":"synthetic"}}"#,
        r#"{"jsonrpc":"2.0","method":17,"params":{"type":"turn.completed","threadId":"A","turnId":"turn-A","message":"first terminal","reason":"synthetic"}}"#,
        r#"{"jsonrpc":"2.0","method":null,"params":{"type":"turn.completed","threadId":"A","turnId":"turn-A","message":"first terminal","reason":"synthetic"}}"#,
        r#"{"jsonrpc":"2.0","method":"harness/event","params":{"type":"turn.completed","message":"PRIVATE_CANARY","protocolVersion":2}}"#,
        r#"{"jsonrpc":"2.0","method":"harness/event","params":{"type":"turn.completed","message":"PRIVATE_CANARY","protocolVersion":null}}"#,
        r#"{"jsonrpc":"2.0","method":"harness/event","params":{"type":"turn.completed","message":"PRIVATE_CANARY","protocolVersion":"1"}}"#,
        "[]",
        "",
    ] {
        let (child, client) = fixture(frame, 3).await;
        *client.initialized.lock().unwrap() = true;
        let mut events = client.subscribe();
        let mut jobs = Vec::new();
        for _ in 0..3 {
            let cloned = client.clone();
            jobs.push(tokio::spawn(async move { cloned.thread_list().await }));
        }
        let mut errors = Vec::new();
        for job in jobs {
            errors.push(decode(
                timeout(Duration::from_secs(3), job)
                    .await
                    .expect("bad frame must settle without EOF or RPC timeout")
                    .unwrap()
                    .unwrap_err(),
            ));
        }
        assert!(errors.iter().all(|error| error == &errors[0]));
        assert!(client.pending.lock().unwrap().is_empty());
        let late = decode(client.tool_list().await.unwrap_err());
        assert_eq!(late, errors[0]);
        client.close().await;
        assert_eq!(
            decode(client.initialize("late", "1").await.unwrap_err()),
            errors[0],
            "local close must not overwrite first protocol cause"
        );
        assert!(matches!(
            events.recv().await.unwrap().event,
            HarnessEvent::ThreadStarted { .. }
        ));
        assert!(matches!(
            events.recv().await,
            Err(broadcast::error::RecvError::Closed)
        ));
        assert!(matches!(
            client.subscribe().recv().await,
            Err(broadcast::error::RecvError::Closed)
        ));
        // close above already sent stdin EOF; verify no late RPC reached the fixture.
        let mut child = child;
        assert!(timeout(Duration::from_secs(3), child.wait())
            .await
            .unwrap()
            .unwrap()
            .success());
    }
}

#[tokio::test]
async fn h101_bad_handshake_never_authorizes_and_is_distinct_from_clean_eof() {
    let (child, client) = fixture("not-json-PRIVATE_CANARY", 1).await;
    let failure = timeout(Duration::from_secs(3), client.initialize("h101", "1"))
        .await
        .expect("bad handshake must fail promptly")
        .unwrap_err();
    let cause = decode(failure);
    assert!(!*client.initialized.lock().unwrap());
    assert_eq!(decode(client.thread_list().await.unwrap_err()), cause);
    assert!(client.pending.lock().unwrap().is_empty());
    finish(child, &client).await;
}

#[tokio::test]
async fn h101_owned_stream_drains_existing_events_then_reports_same_protocol_cause() {
    let (child, client) = fixture("not-json-PRIVATE_CANARY", 1).await;
    *client.initialized.lock().unwrap() = true;
    let mut owned = RequestEvents::turn(
        RpcResponse::success(1.into(), serde_json::json!({"threadId":"t","turnId":"r"})),
        client.subscribe(),
        client.lifecycle.subscribe(),
        "t",
    )
    .unwrap();
    let cause = decode(
        timeout(Duration::from_secs(3), client.thread_list())
            .await
            .unwrap()
            .unwrap_err(),
    );
    // The pre-failure notification belongs to another scope, not this turn.
    assert_eq!(
        decode(
            timeout(Duration::from_secs(3), owned.recv())
                .await
                .unwrap()
                .unwrap_err()
        ),
        cause
    );
    assert!(owned.recv().await.unwrap().is_none());
    finish(child, &client).await;
}

#[tokio::test]
async fn h101_valid_unknown_notification_is_ignored_and_normal_reply_still_completes() {
    let script = r#"
$r=[Console]::In.ReadLine() | ConvertFrom-Json
[Console]::Out.WriteLine('{"jsonrpc":"2.0","method":"future/event","params":{"private":"PRIVATE_CANARY"}}')
[Console]::Out.WriteLine((@{jsonrpc='2.0';id=$r.id;result=@{protocolVersion=1;serverName='h101';serverVersion='1';capabilities=@{threadLifecycle=$true;streaming=$true;interrupt=$true;steer=$true;approval=$true;reconnect=$true}}}|ConvertTo-Json -Compress -Depth 10))
$null=[Console]::In.ReadLine()
"#;
    let mut child = tokio::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
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
    let mut events = client.subscribe();
    timeout(Duration::from_secs(3), client.initialize("h101", "1"))
        .await
        .unwrap()
        .unwrap();
    assert!(!client.lifecycle.borrow().is_closed());
    assert!(client.pending.lock().unwrap().is_empty());
    assert!(matches!(
        events.try_recv(),
        Err(broadcast::error::TryRecvError::Empty)
    ));
    finish(child, &client).await;
}

#[tokio::test]
async fn h101_delivered_admission_and_terminal_are_not_rewritten_by_a_later_bad_frame() {
    let script = r#"
$r=[Console]::In.ReadLine()|ConvertFrom-Json
[Console]::Out.WriteLine((@{jsonrpc='2.0';id=$r.id;result=@{protocolVersion=1;serverName='h101';serverVersion='1';capabilities=@{threadLifecycle=$true;streaming=$true;interrupt=$true;steer=$true;approval=$true;reconnect=$true}}}|ConvertTo-Json -Compress -Depth 10))
$r=[Console]::In.ReadLine()|ConvertFrom-Json
[Console]::Out.WriteLine((@{jsonrpc='2.0';id=$r.id;result=@{threadId='t';turnId='r';status='started'}}|ConvertTo-Json -Compress))
[Console]::Out.WriteLine('{"jsonrpc":"2.0","method":"harness/event","params":{"type":"turn.completed","threadId":"t","turnId":"r","message":"normal completion"}}')
$null=[Console]::In.ReadLine()
[Console]::Out.WriteLine('not-json-PRIVATE_CANARY')
while ($null -ne [Console]::In.ReadLine()) {exit 9}
"#;
    let mut child = tokio::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
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
    client.initialize("h101", "1").await.unwrap();
    let mut stream = client
        .turn_start_streamed(
            serde_json::from_value(serde_json::json!({"threadId":"t","input":"synthetic"}))
                .unwrap(),
        )
        .await
        .unwrap();
    decode(
        timeout(Duration::from_secs(3), client.tool_list())
            .await
            .unwrap()
            .unwrap_err(),
    );
    assert_eq!(stream.response().result()["status"], "started");
    assert!(
        matches!(stream.recv().await.unwrap(),Some(HarnessEvent::TurnCompleted{message,..}) if message=="normal completion")
    );
    assert!(stream.recv().await.unwrap().is_none());
    assert!(client.pending.lock().unwrap().is_empty());
    finish(child, &client).await;
}
