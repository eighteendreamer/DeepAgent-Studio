use super::*;
use crate::protocol::JsonRpcError;

const DEADLINE: Duration = Duration::from_secs(5);

fn spawn_server(script: &str) -> (tempfile::TempDir, Arc<StdioTransport>) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("server.cjs");
    std::fs::write(&path, script).unwrap();
    let config = McpServerConfig {
        transport: None,
        command: Some("node".into()),
        args: vec![path.to_string_lossy().into_owned()],
        env: Default::default(),
        cwd: None,
        url: None,
        headers: Default::default(),
    };
    (dir, Arc::new(StdioTransport::spawn(&config).unwrap()))
}

fn pending_count(transport: &StdioTransport) -> usize {
    transport.dispatcher.state.lock().unwrap().pending.len()
}

async fn send(transport: &StdioTransport, id: u64, method: &str) -> Result<JsonRpcResponse> {
    tokio::time::timeout(
        DEADLINE,
        transport.send(&JsonRpcRequest::new(id, method, None)),
    )
    .await
    .expect("local fixture response deadline")
}

// A ping response is an explicit peer-side barrier: it lists all earlier
// held requests, so tests do not infer dispatch from sleeps or local locks.
const HOLD_SERVER: &str = r#"
const readline = require('node:readline');
let held = [];
let cancelled;
function reply(req, result) {
  process.stdout.write(JSON.stringify({jsonrpc:'2.0', id:req.id, result})+'\n');
}
readline.createInterface({input:process.stdin}).on('line', line => {
  const req = JSON.parse(line);
  if (req.method === 'hold') held.push(req);
  else if (req.method === 'notifications/cancelled') cancelled = req.params.requestId;
  else if (req.method === 'release') {
    for (const old of held) reply(old, {late:true});
    held = [];
  } else {
    process.stdout.write(JSON.stringify({jsonrpc:'2.0', method:'notifications/progress',
      params:{progress:1}})+'\n');
    process.stdout.write(JSON.stringify({jsonrpc:'2.0', id:999999, result:{unsolicited:true}})+'\n');
    reply(req, {held:held.length, held_wire_id:held[0]?.id, cancelled, wire_id:req.id});
  }
});
"#;

#[tokio::test]
async fn slow_request_does_not_block_ping_or_interleaved_notifications() {
    let (_dir, transport) = spawn_server(HOLD_SERVER);
    let hold = tokio::spawn({
        let transport = transport.clone();
        async move { send(&transport, 100, "hold").await }
    });
    tokio::time::timeout(DEADLINE, async {
        loop {
            if send(&transport, 101, "ping").await.unwrap().result.unwrap()["held"] == 1 {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(
        !hold.is_finished(),
        "ping must finish while the slow request is pending"
    );
    assert_eq!(
        pending_count(&transport),
        1,
        "unsolicited ids must not be buffered"
    );
    transport.notify("release", None).await.unwrap();
    let response = tokio::time::timeout(DEADLINE, hold)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(response.id, 100);
    assert_eq!(response.result.unwrap()["late"], true);
    assert_eq!(pending_count(&transport), 0);
    transport.close().await.unwrap();
}

#[tokio::test]
async fn cancelled_waiter_is_removed_and_late_response_cannot_satisfy_reused_local_id() {
    let (_dir, transport) = spawn_server(HOLD_SERVER);
    let context = ToolExecutionContext::default();
    let cancel = context.cancel_flag();
    let hold = tokio::spawn({
        let transport = transport.clone();
        async move {
            transport
                .send_with_context(&JsonRpcRequest::new(1, "hold", None), context)
                .await
        }
    });
    tokio::time::timeout(DEADLINE, async {
        while send(&transport, 2, "ping").await.unwrap().result.unwrap()["held"] != 1 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    cancel.store(true, Ordering::Release);
    let error = tokio::time::timeout(DEADLINE, hold)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert_eq!(error.to_string(), MCP_CANCELLED_UNKNOWN_OUTCOME);
    assert_eq!(pending_count(&transport), 0);
    let barrier = send(&transport, 2, "ping").await.unwrap().result.unwrap();
    assert_eq!(barrier["cancelled"], barrier["held_wire_id"]);
    transport.notify("release", None).await.unwrap();
    let response = send(&transport, 1, "ping").await.unwrap();
    assert_eq!(response.id, 1);
    assert_eq!(response.result.as_ref().unwrap()["held"], 0);
    assert!(response.result.unwrap().get("late").is_none());
    assert_eq!(pending_count(&transport), 0);
    transport.close().await.unwrap();
}

#[tokio::test]
async fn dropped_waiter_does_not_cancel_reader_or_other_pending_request() {
    let (_dir, transport) = spawn_server(HOLD_SERVER);
    let hold = tokio::spawn({
        let transport = transport.clone();
        async move { send(&transport, 10, "hold").await }
    });
    tokio::time::timeout(DEADLINE, async {
        while send(&transport, 11, "ping").await.unwrap().result.unwrap()["held"] != 1 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    hold.abort();
    assert!(hold.await.unwrap_err().is_cancelled());
    assert_eq!(pending_count(&transport), 0);
    transport.notify("release", None).await.unwrap();
    let response = send(&transport, 10, "ping").await.unwrap();
    assert_eq!(response.result.unwrap()["held"], 0);
    assert!(!transport.reader_task.is_finished());
    transport.close().await.unwrap();
}

#[tokio::test]
async fn eof_fails_every_pending_request_and_future_dispatch() {
    let (_dir, transport) = spawn_server(
        r#"
const readline = require('node:readline');
let count = 0;
readline.createInterface({input:process.stdin}).on('line', () => {
  if (++count === 2) process.exit(0);
});
"#,
    );
    let (first, second) = tokio::join!(send(&transport, 1, "hold"), send(&transport, 2, "hold"));
    for result in [first, second, send(&transport, 3, "ping").await] {
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("closed the connection"));
    }
    assert_eq!(pending_count(&transport), 0);
    transport.close().await.unwrap();
}

#[tokio::test]
async fn concurrent_close_wakes_waiters_and_prevents_more_writes() {
    let (_dir, transport) = spawn_server(HOLD_SERVER);
    let hold = tokio::spawn({
        let transport = transport.clone();
        async move { send(&transport, 1, "hold").await }
    });
    tokio::time::timeout(DEADLINE, async {
        while send(&transport, 2, "ping").await.unwrap().result.unwrap()["held"] != 1 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let (a, b) = tokio::time::timeout(DEADLINE, async {
        tokio::join!(transport.close(), transport.close())
    })
    .await
    .unwrap();
    a.unwrap();
    b.unwrap();
    assert!(hold.await.unwrap().is_err());
    assert_eq!(pending_count(&transport), 0);
    assert!(send(&transport, 3, "ping").await.is_err());
    assert!(transport.notify("release", None).await.is_err());
}

#[tokio::test]
async fn error_response_is_delivered_only_to_its_correlated_waiter() {
    let (_dir, transport) = spawn_server(
        r#"
const readline = require('node:readline');
let pending = [];
readline.createInterface({input:process.stdin}).on('line', line => {
  pending.push(JSON.parse(line));
  if (pending.length === 2) {
    for (const req of pending.reverse()) {
      const value = req.method === 'fail' ? {error:{code:-32001,message:'fixture'}} : {result:{ok:true}};
      process.stdout.write(JSON.stringify({jsonrpc:'2.0',id:req.id,...value})+'\n');
    }
  }
});
"#,
    );
    let (ok, error) = tokio::join!(send(&transport, 42, "ping"), send(&transport, 43, "fail"));
    assert_eq!(ok.unwrap().result.unwrap()["ok"], true);
    let response = error.unwrap();
    assert_eq!(response.id, 43);
    assert_eq!(
        response.error,
        Some(JsonRpcError {
            code: -32001,
            message: "fixture".into(),
            data: None
        })
    );
    transport.close().await.unwrap();
}

#[test]
fn pending_limit_duplicate_rejection_and_drop_cleanup_are_bounded() {
    let dispatcher = ResponseDispatcher::default();
    let mut guards = Vec::new();
    let mut receivers = Vec::new();
    for id in 0..MAX_PENDING_REQUESTS as u64 {
        let (sender, receiver) = oneshot::channel();
        guards.push(dispatcher.register(id, sender).unwrap());
        receivers.push(receiver);
    }
    let (sender, _) = oneshot::channel();
    assert!(dispatcher.register(0, sender).is_err());
    let (sender, _) = oneshot::channel();
    assert!(dispatcher
        .register(MAX_PENDING_REQUESTS as u64, sender)
        .is_err());
    guards.clear();
    assert!(dispatcher.state.lock().unwrap().pending.is_empty());
    let (sender, _) = oneshot::channel();
    let guard = dispatcher.register(0, sender).unwrap();
    dispatcher.finish("closed");
    drop(guard);
    assert!(dispatcher.check_open().is_err());
}

async fn wait_for_marker(path: &std::path::Path) {
    tokio::time::timeout(DEADLINE, async {
        while !path.exists() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("fixture must reach the observed IO boundary");
}

#[tokio::test]
async fn caller_timeout_during_partial_response_keeps_reader_frame_intact() {
    let (dir, transport) = spawn_server(
        r#"
const fs = require('node:fs');
const path = require('node:path');
const readline = require('node:readline');
let first, second;
readline.createInterface({input:process.stdin}).on('line', line => {
  const req = JSON.parse(line);
  if (req.method === 'partial') {
    first = req;
    process.stdout.write('{"jsonrpc":"2.0","id":'+req.id+',"result":');
    fs.writeFileSync(path.join(__dirname,'partial'), 'ready');
  } else if (req.method === 'ping') {
    second = req;
    fs.writeFileSync(path.join(__dirname,'second'), 'ready');
  }
  else if (req.method === 'finish') {
    process.stdout.write('{"late":true}}\n');
    process.stdout.write(JSON.stringify({jsonrpc:'2.0',id:second.id,result:{complete:true}})+'\n');
  }
});
"#,
    );
    let first_request = JsonRpcRequest::new(1, "partial", None);
    let mut first = Box::pin(transport.send(&first_request));
    let partial_marker = dir.path().join("partial");
    tokio::select! {
        result = &mut first => panic!("partial response must still be pending: {result:?}"),
        _ = wait_for_marker(&partial_marker) => {},
    }
    assert!(tokio::time::timeout(Duration::from_millis(30), &mut first)
        .await
        .is_err());
    drop(first);
    assert_eq!(pending_count(&transport), 0);
    // Reuse the public id; the wire id must differ from the timed out request.
    let second = tokio::spawn({
        let transport = transport.clone();
        async move { send(&transport, 1, "ping").await }
    });
    wait_for_marker(&dir.path().join("second")).await;
    transport.notify("finish", None).await.unwrap();
    let result = tokio::time::timeout(DEADLINE, second)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(result.id, 1);
    assert_eq!(result.result.unwrap()["complete"], true);
    assert_eq!(pending_count(&transport), 0);
    transport.close().await.unwrap();
}

#[tokio::test]
async fn aborting_blocked_write_retires_connection_instead_of_appending_a_new_json_line() {
    let (_dir, transport) = spawn_server("process.stdin.pause(); setInterval(() => {}, 1000);");
    let writing = tokio::spawn({
        let transport = transport.clone();
        async move {
            transport
                .send(&JsonRpcRequest::new(
                    1,
                    "large",
                    Some(serde_json::json!({
                        "payload": "x".repeat(8 * 1024 * 1024)
                    })),
                ))
                .await
        }
    });
    tokio::time::timeout(DEADLINE, async {
        loop {
            if transport.stdin.try_lock().is_err() {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    writing.abort();
    assert!(writing.await.unwrap_err().is_cancelled());
    assert_eq!(pending_count(&transport), 0);
    assert!(transport
        .dispatcher
        .check_open()
        .unwrap_err()
        .to_string()
        .contains("during message write"));
    assert!(send(&transport, 2, "ping").await.is_err());
    transport.close().await.unwrap();
}
