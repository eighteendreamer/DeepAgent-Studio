//! Windows real-pipe tests of the production SDK reader and owned requests.
use super::*;
use std::time::Duration;

const SCRIPT: &str = r#"
$mode = '$MODE'
$starts = 0
$approvals = 0
$threads = 0
function Send($value) { [Console]::Out.WriteLine(($value | ConvertTo-Json -Compress -Depth 10)) }
function Batch($values) { [Console]::Out.WriteLine((($values | ForEach-Object { $_ | ConvertTo-Json -Compress -Depth 10 }) -join "`n")) }
function Notify($value) { return @{jsonrpc='2.0';method='harness/event';params=$value} }
while (($line = [Console]::In.ReadLine()) -ne $null) {
  $request = $line | ConvertFrom-Json
  $reply = @{jsonrpc='2.0';id=$request.id;result=@{}}
  if ($request.method -eq 'initialize') {
    $reply.result = @{protocolVersion=1;serverName='owned-fixture';serverVersion='1';capabilities=@{threadLifecycle=$true;streaming=$true;interrupt=$true;steer=$true;approval=$true;reconnect=$true}}
    Send $reply
  } elseif ($request.method -eq 'thread/start') {
    $threads++
    $thread = "thread-$threads"
    $reply.result = @{threadId=$thread;status='ready'}
    Batch @((Notify @{type='thread.started';threadId=$thread;protocolVersion=1}),$reply)
  } elseif ($request.method -eq 'turn/start') {
    $starts++
    $thread = $request.params.threadId
    $turn = "turn-$thread"
    $reply.result = @{threadId=$thread;turnId=$turn;status='started'}
    $started = Notify @{type='turn.started';threadId=$thread;turnId=$turn}
    $delta = Notify @{type='item.updated';threadId=$thread;turnId=$turn;item=@{kind='content_delta';text='synthetic'}}
    $terminal = Notify @{type='turn.completed';threadId=$thread;turnId=$turn;message='first terminal';reason='synthetic'}
    if ($mode -eq 'failed') { $terminal.params.type='turn.failed' }
    if ($mode -eq 'interrupted') { $terminal.params.type='turn.interrupted' }
    if ($mode -eq 'notification-boundary') {
      $noise = @()
      foreach ($method in @('other/event','','harness/Event','harness/event ','prefix/harness/event')) {
        foreach ($payload in @($terminal.params,@{type='turn.failed';threadId=$thread;turnId=$turn;reason='not harness'},@{type='turn.interrupted';threadId=$thread;turnId=$turn},@{type='approval.requested';threadId=$thread;turnId=$turn;approvalId='fake';reason='not harness'})) {
          $noise += @{jsonrpc='2.0';method=$method;params=$payload}
        }
      }
      foreach ($version in @('1.0','2.1','',2,$null)) {
        $noise += @{jsonrpc=$version;method='harness/event';params=$terminal.params}
      }
      $noise += @{method='harness/event';params=$terminal.params}
      $noise += @{jsonrpc='2.0';params=$terminal.params}
      $noise += @{jsonrpc='2.0';method=17;params=$terminal.params}
      $noise += @{jsonrpc='2.0';method=$null;params=$terminal.params}
      foreach ($payload in @($null,'invalid',@(),@{type='future.event'},@{type='turn.started';turnId=17})) {
        $noise += @{jsonrpc='2.0';method='harness/event';params=$payload}
      }
      Batch ($noise + @($started,$delta,(Notify @{type='approval.requested';threadId=$thread;turnId=$turn;approvalId='fixture-approval';reason='synthetic approval'}),$reply))
    } elseif ($mode -eq 'notification-flood') {
      $noise = @(1..300 | ForEach-Object { @{jsonrpc='2.0';method='other/event';params=$terminal.params} })
      Batch ($noise + @($started,$delta,$terminal,$reply))
    } elseif ($mode -eq 'rpc-error') {
      Send @{jsonrpc='2.0';id=$request.id;error=@{code=-32005;message='synthetic busy'}}
    } elseif ($mode -eq 'hold-start') {
      Send $started
    } elseif ($mode -eq 'hold') {
      Batch @($reply,$started)
    } elseif ($mode -eq 'approval') {
      Batch @($started,(Notify @{type='approval.requested';threadId=$thread;turnId=$turn;approvalId='fixture-approval';reason='synthetic approval'}),$reply)
    } elseif ($mode -eq 'overflow') {
      $burst = @(1..300 | ForEach-Object { $delta }) + @($terminal,$reply)
      Batch $burst
    } elseif ($mode -eq 'after') {
      Batch @($reply,$started,$delta,$terminal,$terminal)
    } else {
      Batch @($started,$delta,$terminal,$terminal,$reply)
    }
  } elseif ($request.method -eq 'approval/respond') {
    $approvals++
    $reply.result = @{approvalId=$request.params.approvalId;approved=$request.params.approved;status='resolved'}
    Batch @($reply,(Notify @{type='turn.completed';threadId='A';turnId='turn-A';message='approved fixture terminal'}))
  } elseif ($request.method -eq 'tool/list') {
    $reply.result = @{starts=$starts;approvals=$approvals;threadId=$request.params.threadId;runId=$request.params.runId;expectedSnapshotId=$request.params.expectedSnapshotId}
    Send $reply
  } elseif ($request.method -eq 'config/read') {
    exit 7
  } else { throw "unexpected synthetic method" }
}
"#;

async fn fixture(mode: &str, capacity: usize) -> (Child, HarnessClient) {
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
        capacity,
    );
    tokio::time::timeout(
        Duration::from_secs(5),
        client.initialize("h214-fixture", "1"),
    )
    .await
    .unwrap()
    .unwrap();
    (child, client)
}

fn request(thread: &str) -> TurnStartRequest {
    TurnStartRequest {
        expected_config_snapshot_id: None,
        thread_id: thread.into(),
        input: "synthetic only".into(),
        provider: None,
        model: None,
        reasoning_effort: None,
        permission_profile: None,
        sandbox_backend: None,
    }
}

async fn receive(stream: &mut RequestEvents) -> Result<Option<HarnessEvent>, SdkError> {
    tokio::time::timeout(Duration::from_secs(3), stream.recv())
        .await
        .expect("bounded fixture receive")
}

async fn finish(mut child: Child, client: &HarnessClient) {
    client.close().await;
    tokio::time::timeout(Duration::from_secs(3), child.wait())
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn h214_real_pipe_retains_before_after_fast_failure_and_interruption_until_consumed() {
    for mode in ["before", "after", "failed", "interrupted"] {
        let (child, client) = fixture(mode, 16).await;
        let mut stream = client.turn_start_streamed(request("A")).await.unwrap();
        assert_eq!(stream.thread_id(), "A");
        assert_eq!(stream.turn_id(), Some("turn-A"));
        assert_eq!(stream.response().result()["status"], "started");
        assert_eq!(client.tool_list().await.unwrap().result()["starts"], 1);
        assert!(matches!(
            receive(&mut stream).await.unwrap(),
            Some(HarnessEvent::TurnStarted { .. })
        ));
        assert!(matches!(
            receive(&mut stream).await.unwrap(),
            Some(HarnessEvent::ItemUpdated { .. })
        ));
        let event = receive(&mut stream).await.unwrap().unwrap();
        match mode {
            "failed" => assert!(matches!(event, HarnessEvent::TurnFailed { .. })),
            "interrupted" => assert!(matches!(event, HarnessEvent::TurnInterrupted { .. })),
            _ => assert!(matches!(event, HarnessEvent::TurnCompleted { .. })),
        }
        assert!(receive(&mut stream).await.unwrap().is_none());
        assert_eq!(client.event_receiver_count(), 0);
        finish(child, &client).await;
    }
}

#[tokio::test]
async fn h214_real_pipe_concurrent_requests_keep_their_own_events() {
    let (child, client) = fixture("before", 32).await;
    let (a, b) = tokio::join!(
        client.turn_start_streamed(request("A")),
        client.turn_start_streamed(request("B"))
    );
    for mut stream in [a.unwrap(), b.unwrap()] {
        let thread = stream.thread_id().to_owned();
        let mut events = Vec::new();
        while let Some(event) = receive(&mut stream).await.unwrap() {
            events.push(serde_json::to_value(event).unwrap());
        }
        assert_eq!(events.len(), 3);
        assert!(events.iter().all(|event| event["threadId"] == thread));
    }
    assert_eq!(client.event_receiver_count(), 0);
    assert_eq!(client.tool_list().await.unwrap().result()["starts"], 2);
    finish(child, &client).await;
}

#[tokio::test]
async fn h214_real_pipe_exposes_the_owned_approval_before_its_terminal() {
    let (child, client) = fixture("approval", 8).await;
    let mut stream = client.turn_start_streamed(request("A")).await.unwrap();
    assert!(matches!(
        receive(&mut stream).await.unwrap(),
        Some(HarnessEvent::TurnStarted { .. })
    ));
    assert!(
        matches!(receive(&mut stream).await.unwrap(),Some(HarnessEvent::ApprovalRequested { approval_id:Some(id), .. }) if id=="fixture-approval")
    );
    client
        .approval_respond(ApprovalRespondRequest {
            approval_id: "fixture-approval".into(),
            approved: true,
            scope: None,
        })
        .await
        .unwrap();
    assert!(matches!(
        receive(&mut stream).await.unwrap(),
        Some(HarnessEvent::TurnCompleted { .. })
    ));
    assert!(receive(&mut stream).await.unwrap().is_none());
    assert_eq!(client.tool_list().await.unwrap().result()["approvals"], 1);
    finish(child, &client).await;
}

#[tokio::test]
async fn h214_real_pipe_rpc_failure_and_overflow_release_without_redispatch() {
    for mode in ["rpc-error", "overflow"] {
        let (child, client) = fixture(mode, 8).await;
        if mode == "rpc-error" {
            assert!(matches!(
                client.turn_start_streamed(request("A")).await,
                Err(SdkError::Rpc { code: -32005, .. })
            ));
        } else {
            let mut stream = client.turn_start_streamed(request("A")).await.unwrap();
            assert!(matches!(
                receive(&mut stream).await,
                Err(SdkError::EventLagged { .. })
            ));
            assert!(receive(&mut stream).await.unwrap().is_none());
        }
        assert_eq!(client.event_receiver_count(), 0);
        assert_eq!(client.tool_list().await.unwrap().result()["starts"], 1);
        finish(child, &client).await;
    }
}

#[tokio::test]
async fn h219_real_pipe_scoped_tool_list_preserves_requested_run_identity() {
    let (child, client) = fixture("after", 16).await;
    let response = client
        .tool_list_scoped(ToolListRequest {
            thread_id: Some("thread-A".into()),
            run_id: Some("run-A".into()),
            expected_snapshot_id: Some("snapshot-A".into()),
        })
        .await
        .unwrap();
    assert_eq!(response.result()["threadId"], "thread-A");
    assert_eq!(response.result()["runId"], "run-A");
    assert_eq!(response.result()["expectedSnapshotId"], "snapshot-A");
    finish(child, &client).await;
}

#[tokio::test]
async fn h214_real_pipe_eof_and_local_close_settle_owned_waits_without_claiming_completion() {
    for close_locally in [false, true] {
        let (mut child, client) = fixture("hold", 8).await;
        let mut stream = client.turn_start_streamed(request("A")).await.unwrap();
        assert!(matches!(
            receive(&mut stream).await.unwrap(),
            Some(HarnessEvent::TurnStarted { .. })
        ));
        if close_locally {
            client.close().await;
        } else {
            assert!(matches!(
                client.config_read().await,
                Err(SdkError::ResponseChannelClosed { .. })
            ));
        }
        assert!(matches!(
            receive(&mut stream).await,
            Err(SdkError::EventStreamClosed)
        ));
        assert_eq!(client.event_receiver_count(), 0);
        tokio::time::timeout(Duration::from_secs(3), child.wait())
            .await
            .unwrap()
            .unwrap();
    }
}

#[tokio::test]
async fn h214_real_pipe_dropped_start_future_releases_its_subscription() {
    let (child, client) = fixture("hold-start", 8).await;
    let mut observer = client.subscribe();
    let caller = client.clone();
    let request = tokio::spawn(async move { caller.turn_start_streamed(request("A")).await });
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(3), observer.recv())
            .await
            .unwrap()
            .unwrap()
            .event,
        HarnessEvent::TurnStarted { .. }
    ));
    assert_eq!(client.event_receiver_count(), 2);
    request.abort();
    assert!(matches!(request.await, Err(error) if error.is_cancelled()));
    assert_eq!(client.event_receiver_count(), 1);
    assert_eq!(client.tool_list().await.unwrap().result()["starts"], 1);
    drop(observer);
    // H-204 owns the same RPC registration: abandonment releases it before EOF.
    assert!(client.pending.lock().unwrap().is_empty());
    finish(child, &client).await;
    assert!(client.pending.lock().unwrap().is_empty());
}

#[tokio::test]
async fn h214_raw_subscribe_remains_explicitly_from_now_not_a_replay_api() {
    let (child, client) = fixture("before", 16).await;
    client.turn_start(request("A")).await.unwrap();
    client.tool_list().await.unwrap(); // Reader consumed the entire earlier stdout batch.
    let mut late = client.subscribe();
    assert!(matches!(
        late.try_recv(),
        Err(broadcast::error::TryRecvError::Empty)
    ));
    drop(late);
    finish(child, &client).await;
}

#[tokio::test]
async fn h213_real_pipe_routes_only_valid_harness_notifications_to_raw_and_owned_consumers() {
    // A second independent connection proves filtering is not a one-shot startup gate.
    for _ in 0..2 {
        let (child, client) = fixture("notification-boundary", 64).await;
        let mut raw = client.subscribe();
        let mut stream = client.turn_start_streamed(request("A")).await.unwrap();
        // The response is an ordered reader barrier, not a sleep-based absence assertion.
        assert_eq!(client.tool_list().await.unwrap().result()["approvals"], 0);
        assert!(client.pending.lock().unwrap().is_empty());
        for index in 0..3 {
            let event = receive(&mut stream).await.unwrap().unwrap();
            assert_eq!(raw.try_recv().unwrap().event, event);
            match index {
                0 => assert!(matches!(event, HarnessEvent::TurnStarted { .. })),
                1 => assert!(matches!(event, HarnessEvent::ItemUpdated { .. })),
                _ => assert!(matches!(
                    event,
                    HarnessEvent::ApprovalRequested { approval_id: Some(id), .. }
                        if id == "fixture-approval"
                )),
            }
        }
        assert!(matches!(
            raw.try_recv(),
            Err(broadcast::error::TryRecvError::Empty)
        ));
        client
            .approval_respond(ApprovalRespondRequest {
                approval_id: "fixture-approval".into(),
                approved: true,
                scope: None,
            })
            .await
            .unwrap();
        assert_eq!(client.tool_list().await.unwrap().result()["approvals"], 1);
        let terminal = receive(&mut stream).await.unwrap().unwrap();
        assert!(
            matches!(&terminal, HarnessEvent::TurnCompleted { message, .. } if message == "approved fixture terminal")
        );
        assert_eq!(raw.try_recv().unwrap().event, terminal);
        assert!(receive(&mut stream).await.unwrap().is_none());
        assert!(matches!(
            raw.try_recv(),
            Err(broadcast::error::TryRecvError::Empty)
        ));
        drop(raw);
        finish(child, &client).await;
    }
}

#[tokio::test]
async fn h213_real_pipe_unknown_notifications_do_not_consume_broadcast_capacity() {
    let (child, client) = fixture("notification-flood", 4).await;
    let mut raw = client.subscribe();
    let mut stream = client.turn_start_streamed(request("A")).await.unwrap();
    let receipt = client.tool_list().await.unwrap();
    assert_eq!(receipt.result()["starts"], 1);
    assert_eq!(receipt.result()["approvals"], 0);
    for index in 0..3 {
        let event = receive(&mut stream).await.unwrap().unwrap();
        assert_eq!(raw.try_recv().unwrap().event, event);
        assert!(matches!(
            (index, event),
            (0, HarnessEvent::TurnStarted { .. })
                | (1, HarnessEvent::ItemUpdated { .. })
                | (2, HarnessEvent::TurnCompleted { .. })
        ));
    }
    assert!(receive(&mut stream).await.unwrap().is_none());
    assert!(matches!(
        raw.try_recv(),
        Err(broadcast::error::TryRecvError::Empty)
    ));
    drop(raw);
    finish(child, &client).await;
}
