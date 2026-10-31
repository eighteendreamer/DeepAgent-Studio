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
async fn h236_cancellation_interrupts_response_diagnostics_waiting_for_child_lock() {
    assert_cancellable_response_diagnostics(true).await;
}

#[tokio::test]
async fn h236_cancellation_interrupts_response_diagnostics_waiting_for_stderr_lock() {
    assert_cancellable_response_diagnostics(false).await;
}

async fn assert_cancellable_response_diagnostics(hold_child_lock: bool) {
    let (_dir, transport) = spawn_server(
        r#"
require('node:readline').createInterface({input:process.stdin}).on('line', line => {
  if (JSON.parse(line).method === 'disconnect') {
    process.stderr.write('fixture diagnostic\n', () => process.exit(23));
  }
});
"#,
    );
    let context = ToolExecutionContext::default();
    let cancel = context.cancellation_token();
    let request = JsonRpcRequest::new(1, "hold", None);
    let mut call = transport.send_with_context(&request, context);
    std::future::poll_fn(|cx| {
        assert!(call.as_mut().poll(cx).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
    // This fence can acquire stdin only after the request's complete flush.
    tokio::select! {
        result = &mut call => panic!("held request completed early: {result:?}"),
        fence = transport.notify("fence", None) => fence.unwrap(),
    }
    let child = if hold_child_lock {
        Some(transport.child.lock().await)
    } else {
        None
    };
    let stderr = if hold_child_lock {
        None
    } else {
        Some(transport.stderr_task.lock().await)
    };
    transport.notify("disconnect", None).await.unwrap();
    tokio::time::timeout(DEADLINE, async {
        while !transport.reader_task.is_finished() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("real peer EOF must reach the response dispatcher");
    if !hold_child_lock {
        let mut child = transport.child.lock().await;
        tokio::time::timeout(DEADLINE, child.wait())
            .await
            .unwrap()
            .unwrap();
    }
    std::future::poll_fn(|cx| {
        assert!(call.as_mut().poll(cx).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
    cancel.cancel();
    let result = tokio::time::timeout(Duration::from_millis(250), &mut call).await;
    // A regression must still release the fixture and its locks before failing.
    drop((child, stderr));
    drop(call);
    transport.close().await.unwrap();
    eprintln!("response diagnostics cancellation: child_lock={hold_child_lock}, result={result:?}");
    assert_eq!(
        result
            .expect("request cancellation must not wait for diagnostic locks")
            .unwrap_err()
            .to_string(),
        MCP_CANCELLED_UNKNOWN_OUTCOME
    );
    assert_eq!(pending_count(&transport), 0);
    assert!(transport.dispatcher.check_open().is_err());
}

#[cfg(windows)]
#[tokio::test]
async fn h236_cancellation_interrupts_write_error_diagnostics() {
    assert_cancellable_write_diagnostics(8 * 1024 * 1024, "write to MCP server failed").await;
}

#[cfg(windows)]
#[tokio::test]
async fn h236_cancellation_interrupts_flush_error_diagnostics() {
    assert_cancellable_write_diagnostics(0, "flush to MCP server failed").await;
}

#[cfg(windows)]
async fn assert_cancellable_write_diagnostics(payload_bytes: usize, expected_stage: &str) {
    let (_dir, transport) = spawn_server("process.exit(23);");
    // Hold off the EOF reader to isolate the real broken-pipe write/flush
    // diagnostic. Otherwise EOF can win first and reject dispatch entirely.
    transport.reader_task.abort();
    let mut child = transport.child.lock().await;
    tokio::time::timeout(DEADLINE, child.wait())
        .await
        .unwrap()
        .unwrap();
    let context = ToolExecutionContext::default();
    let cancel = context.cancellation_token();
    let request = JsonRpcRequest::new(
        1,
        "closed_pipe",
        Some(serde_json::json!({"payload": "x".repeat(payload_bytes)})),
    );
    let mut call = transport.send_with_context(&request, context);
    let stage = tokio::select! {
        result = &mut call => panic!("diagnostic must wait for its held lock: {result:?}"),
        stage = tokio::time::timeout(DEADLINE, async {
            loop {
                if let Some(reason) = transport.dispatcher.state.lock().unwrap().terminal_error.clone() {
                    break reason;
                }
                tokio::task::yield_now().await;
            }
        }) => stage.expect("closed peer stdin must produce a real IO error"),
    };
    cancel.cancel();
    let result = tokio::time::timeout(Duration::from_millis(250), &mut call).await;
    drop(child);
    drop(call);
    transport.close().await.unwrap();
    eprintln!("write diagnostics cancellation: stage={stage}, result={result:?}");
    assert!(stage.starts_with(expected_stage), "{stage}");
    assert_eq!(
        result
            .expect("IO diagnostic cancellation must not wait for the child lock")
            .unwrap_err()
            .to_string(),
        MCP_CANCELLED_UNKNOWN_OUTCOME
    );
    assert_eq!(pending_count(&transport), 0);
    assert!(transport.dispatcher.check_open().is_err());
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

#[tokio::test]
async fn cancelling_queued_write_never_notifies_an_unissued_request_or_cancels_shared_waiter() {
    let (_dir, transport) = spawn_server(HOLD_SERVER);
    let healthy = tokio::spawn({
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

    let stdin = transport.stdin.lock().await;
    let context = ToolExecutionContext::default();
    let cancel = context.cancellation_token();
    let request = JsonRpcRequest::new(3, "hold", None);
    let mut queued = transport.send_with_context(&request, context);
    std::future::poll_fn(|cx| {
        assert!(queued.as_mut().poll(cx).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
    assert_eq!(pending_count(&transport), 2);
    cancel.cancel();
    // Drive cancellation while the writer is still locked, then release it.
    // This also exposes an invalid cancellation notification if the cancelled
    // caller queues one for a request which has never acquired stdin.
    let early_result = std::future::poll_fn(|cx| {
        std::task::Poll::Ready(match queued.as_mut().poll(cx) {
            std::task::Poll::Ready(result) => Some(result),
            std::task::Poll::Pending => None,
        })
    })
    .await;
    let completed_while_locked = early_result.is_some();
    drop(stdin);
    let error = match early_result {
        Some(result) => result,
        None => tokio::time::timeout(DEADLINE, queued).await.unwrap(),
    }
    .unwrap_err();
    let pending_after_cancel = pending_count(&transport);
    let barrier = send(&transport, 4, "ping").await.unwrap().result.unwrap();
    let healthy_still_waiting = !healthy.is_finished();
    transport.notify("release", None).await.unwrap();
    let healthy_result = healthy.await.unwrap().unwrap();
    transport.close().await.unwrap();

    eprintln!("queued cancellation: error={error}, pending={pending_after_cancel}, peer={barrier}");
    assert_eq!(error.to_string(), MCP_CANCELLED_BEFORE_DISPATCH);
    assert!(
        completed_while_locked,
        "cancellation must not wait for the stdin owner"
    );
    assert!(
        barrier["cancelled"].is_null(),
        "unsent request must not have a cancellation notification"
    );
    assert_eq!(
        barrier["held"], 1,
        "cancelled queued request must never reach the peer"
    );
    assert_eq!(pending_after_cancel, 1);
    assert!(healthy_still_waiting);
    assert_eq!(healthy_result.id, 1);
    assert_eq!(healthy_result.result.unwrap()["late"], true);
}

#[tokio::test]
async fn precancelled_request_has_no_wire_effect_and_keeps_connection_usable() {
    let (_dir, transport) = spawn_server(HOLD_SERVER);
    let context = ToolExecutionContext::default();
    context.cancellation_token().cancel();
    context.cancellation_token().cancel();
    let error = transport
        .send_with_context(&JsonRpcRequest::new(1, "hold", None), context)
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), MCP_CANCELLED_BEFORE_DISPATCH);
    assert_eq!(pending_count(&transport), 0);
    let barrier = send(&transport, 1, "ping").await.unwrap();
    transport.close().await.unwrap();
    assert_eq!(barrier.id, 1);
    let peer = barrier.result.unwrap();
    assert_eq!(peer["held"], 0);
    assert!(peer["cancelled"].is_null());
    assert_eq!(
        peer["wire_id"], 1,
        "precancellation must not even allocate a wire id"
    );
}

#[tokio::test]
async fn token_cancellation_during_partial_write_retires_stream_and_wakes_shared_waiters() {
    let (dir, transport) = spawn_server(
        r#"
const fs = require('node:fs');
const path = require('node:path');
let buffer = '', held = 0;
process.stdin.on('data', chunk => {
  buffer += chunk.toString('utf8');
  if (buffer.length > 1024) {
    process.stdin.pause();
    fs.writeFileSync(path.join(__dirname, 'partial-write'), String(buffer.length));
    return;
  }
  let end;
  while ((end = buffer.indexOf('\n')) >= 0) {
    const req = JSON.parse(buffer.slice(0, end));
    buffer = buffer.slice(end + 1);
    if (req.method === 'hold') held++;
    else process.stdout.write(JSON.stringify({jsonrpc:'2.0',id:req.id,result:{held}})+'\n');
  }
});
// Pausing stdin removes its active read handle; retain a bounded peer lifetime.
setTimeout(() => process.exit(2), 15000);
"#,
    );
    let healthy = tokio::spawn({
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

    let context = ToolExecutionContext::default();
    let cancel = context.cancellation_token();
    let writing = tokio::spawn({
        let transport = transport.clone();
        async move {
            transport
                .send_with_context(
                    &JsonRpcRequest::new(
                        3,
                        "large",
                        Some(serde_json::json!({"payload":"x".repeat(8 * 1024 * 1024)})),
                    ),
                    context,
                )
                .await
        }
    });
    let marker = dir.path().join("partial-write");
    wait_for_marker(&marker).await;
    let read_bytes: usize = std::fs::read_to_string(marker).unwrap().parse().unwrap();
    assert!(read_bytes > 0 && read_bytes < 8 * 1024 * 1024);
    let queued_request = JsonRpcRequest::new(4, "ping", None);
    let mut queued = transport.send(&queued_request);
    std::future::poll_fn(|cx| {
        assert!(queued.as_mut().poll(cx).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
    assert_eq!(pending_count(&transport), 3);
    cancel.cancel();
    cancel.cancel();
    let error = tokio::time::timeout(DEADLINE, writing)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    let queued_error = tokio::time::timeout(DEADLINE, queued)
        .await
        .unwrap()
        .unwrap_err();
    let healthy_error = healthy.await.unwrap().unwrap_err();
    let extra_write = transport.notify("release", None).await;
    transport.close().await.unwrap();

    eprintln!("partial write: cancelled={error}, queued={queued_error}, healthy={healthy_error}, notification={extra_write:?}");
    assert_eq!(error.to_string(), MCP_CANCELLED_UNKNOWN_OUTCOME);
    for error in [queued_error, healthy_error, extra_write.unwrap_err()] {
        assert!(
            error.to_string().contains("during message write"),
            "{error}"
        );
    }
    assert_eq!(pending_count(&transport), 0);
    assert!(transport.dispatcher.check_open().is_err());
}

#[cfg(windows)]
const FLUSH_SERVER: &str = r#"
const fs = require('node:fs');
const path = require('node:path');
const mark = (name, id) => fs.writeFileSync(path.join(__dirname, name), String(id));
let pending, timer, sibling;
require('node:readline').createInterface({input:process.stdin}).on('line', line => {
  const req = JSON.parse(line);
  if (req.method === 'hold') {
    pending = req;
    mark('started', req.id);
    timer = setInterval(() => {
      if (!fs.existsSync(path.join(__dirname, 'release'))) return;
      clearInterval(timer);
      mark('responded', req.id);
      process.stdout.write(JSON.stringify({jsonrpc:'2.0',id:req.id,result:{late:true}})+'\n');
    }, 5);
  } else if (req.method === 'sibling') {
    sibling = req;
    mark('sibling', req.id);
  } else if (req.method === 'release_sibling') {
    process.stdout.write(JSON.stringify({jsonrpc:'2.0',id:sibling.id,result:{sibling:true}})+'\n');
  } else if (req.method === 'notifications/cancelled' && pending.id === req.params.requestId) {
    clearInterval(timer);
    mark('cancelled', req.params.requestId);
  }
});
setTimeout(() => process.exit(2), 15000).unref();
"#;

#[cfg(windows)]
#[tokio::test]
async fn h236_unpolled_windows_flush_settles_before_correlated_cancellation() {
    assert_windows_flush_boundary(true).await;
}

#[cfg(windows)]
#[tokio::test]
async fn completed_windows_flush_allows_correlated_cancellation_and_reuse() {
    assert_windows_flush_boundary(false).await;
}

#[cfg(windows)]
async fn assert_windows_flush_boundary(defer_flush_poll: bool) {
    let (dir, transport) = spawn_server(FLUSH_SERVER);
    let mut sibling = tokio::spawn({
        let transport = transport.clone();
        async move { send(&transport, 9, "sibling").await }
    });
    wait_for_marker(&dir.path().join("sibling")).await;
    let context = ToolExecutionContext::default();
    let cancel = context.cancellation_token();
    let request = JsonRpcRequest::new(1, "hold", None);
    let mut call = transport.send_with_context(&request, context);
    std::future::poll_fn(|cx| {
        assert!(call.as_mut().poll(cx).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
    wait_for_marker(&dir.path().join("started")).await;
    if !defer_flush_poll {
        // The fence must acquire the same writer, so the caller has observed
        // write+flush completion, not merely the peer's receipt of the bytes.
        tokio::select! {
            result = &mut call => panic!("held request completed early: {result:?}"),
            fence = transport.notify("fence", None) => fence.unwrap(),
        }
    }
    cancel.cancel();
    let error = tokio::time::timeout(DEADLINE, call)
        .await
        .unwrap()
        .unwrap_err();
    let probe = transport.notify("probe", None).await;
    let notified = tokio::time::timeout(
        Duration::from_millis(250),
        wait_for_marker(&dir.path().join("cancelled")),
    )
    .await
    .is_ok();
    std::fs::write(dir.path().join("release"), "release").unwrap();
    if !notified {
        wait_for_marker(&dir.path().join("responded")).await;
    }
    let sibling_held = !sibling.is_finished();
    let release = transport.notify("release_sibling", None).await;
    let sibling_result = tokio::time::timeout(DEADLINE, &mut sibling).await;
    if sibling_result.is_err() {
        sibling.abort();
        let _ = sibling.await;
    }
    transport.close().await.unwrap();

    eprintln!("flush boundary: defer={defer_flush_poll}, notified={notified}, probe={probe:?}");
    assert_eq!(error.to_string(), MCP_CANCELLED_UNKNOWN_OUTCOME);
    assert_eq!(pending_count(&transport), 0);
    assert!(
        sibling_held,
        "the cancelled frame must not retire its sibling"
    );
    assert!(release.is_ok());
    assert_eq!(
        sibling_result.unwrap().unwrap().unwrap().result.unwrap()["sibling"],
        true
    );
    assert!(
        notified,
        "a completed frame must retain its cancellation owner"
    );
    assert!(probe.is_ok());
    assert!(!dir.path().join("responded").exists());
    assert_eq!(
        std::fs::read(dir.path().join("started")).unwrap(),
        std::fs::read(dir.path().join("cancelled")).unwrap()
    );
}

#[cfg(windows)]
#[tokio::test]
async fn dropping_unpolled_windows_flush_still_retires_the_stream() {
    let (dir, transport) = spawn_server(FLUSH_SERVER);
    let context = ToolExecutionContext::default();
    let request = JsonRpcRequest::new(1, "hold", None);
    let mut call = transport.send_with_context(&request, context);
    std::future::poll_fn(|cx| {
        assert!(call.as_mut().poll(cx).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
    wait_for_marker(&dir.path().join("started")).await;
    drop(call);
    let pending = pending_count(&transport);
    let probe = transport.notify("probe", None).await;
    transport.close().await.unwrap();
    assert_eq!(pending, 0);
    assert!(probe
        .unwrap_err()
        .to_string()
        .contains("during message write"));
    assert!(!dir.path().join("cancelled").exists());
}

#[cfg(windows)]
#[test]
fn h236_stalled_flush_cleanup_remains_bounded_and_retires_the_stream() {
    // Two blocking slots are held by the real stdout/stderr readers. Occupy
    // the remaining slot after the peer reads the frame, before polling its
    // flush: Tokio's Windows pipe flush must then wait for a blocking slot.
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(3)
        .build()
        .unwrap();
    runtime.block_on(async {
        let (dir, transport) = spawn_server(FLUSH_SERVER);
        let context = ToolExecutionContext::default();
        let cancel = context.cancellation_token();
        let request = JsonRpcRequest::new(1, "hold", None);
        let mut call = transport.send_with_context(&request, context);
        std::future::poll_fn(|cx| {
            assert!(call.as_mut().poll(cx).is_pending());
            std::task::Poll::Ready(())
        })
        .await;
        wait_for_marker(&dir.path().join("started")).await;
        let (release, blocked) = std::sync::mpsc::channel();
        let (entered, ready) = oneshot::channel();
        let blocker = tokio::task::spawn_blocking(move || {
            let _ = entered.send(());
            let _ = blocked.recv_timeout(DEADLINE);
        });
        let occupied = tokio::time::timeout(DEADLINE, ready).await;
        cancel.cancel();
        let result = tokio::time::timeout(Duration::from_millis(250), &mut call).await;
        // Release before any target assertion or close; even a regression
        // must not leave the blocking pool or fixture process hanging.
        let _ = release.send(());
        blocker.await.unwrap();
        drop(call);
        let probe = transport.notify("probe", None).await;
        transport.close().await.unwrap();
        occupied.unwrap().unwrap();
        assert_eq!(
            result.unwrap().unwrap_err().to_string(),
            MCP_CANCELLED_UNKNOWN_OUTCOME
        );
        assert_eq!(pending_count(&transport), 0);
        assert!(probe
            .unwrap_err()
            .to_string()
            .contains("during message write"));
        assert!(!dir.path().join("cancelled").exists());
    });
}
