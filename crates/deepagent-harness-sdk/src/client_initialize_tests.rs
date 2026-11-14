//! H-206: handshake result is a gate, not just a JSON-RPC success bit.
//! H-100: response member presence is validated before that result gate.
use super::*;
use serde_json::{json, Value};
use std::time::Duration;

fn invalid_envelopes() -> Vec<Value> {
    vec![
        json!({"result":valid(),"error":null}),
        json!({}),
        json!({"error":null}),
        json!({"result":null,"error":null}),
        json!({"result":valid(),"error":{"code":-32004,"message":"PRIVATE_PAYLOAD"}}),
        json!({"result":null,"error":{"code":-32004,"message":"PRIVATE_PAYLOAD"}}),
        json!({"error":"PRIVATE_PAYLOAD"}),
        json!({"error":[]}),
    ]
}

// Both modes hold stdout open until client EOF. A business test waits for two
// concurrent requests before emitting the bad frame; late successes must not
// revive either the connection or the handshake.
fn envelope_peer(mut body: Value, business: bool) -> (Child, HarnessClient) {
    let id = if business { 2 } else { 1 };
    body["jsonrpc"] = json!("2.0");
    body["id"] = json!(id);
    let first = json!({"jsonrpc":"2.0","id":1,"result":valid()});
    let late = json!({"jsonrpc":"2.0","id":id,"result":valid()});
    let other = json!({"jsonrpc":"2.0","id":3,"result":{"source":"other"}});
    let reads = if business { 3 } else { 1 };
    let script = format!(
        r#"$count=0
while ($null -ne ($line=[Console]::In.ReadLine())) {{
 $count++
 if ({business} -and $count -eq 1) {{ [Console]::Out.WriteLine('{first}') }}
 if ($count -eq {reads}) {{
  [Console]::Out.WriteLine('{body}')
  [Console]::Out.WriteLine('{late}')
  if ({business}) {{ [Console]::Out.WriteLine('{other}') }}
 }}
 if ($count -gt {reads}) {{ exit 9 }}
}}
exit 0"#,
        business = if business { "$true" } else { "$false" },
        first = first.to_string().replace('\'', "''"),
        body = body.to_string().replace('\'', "''"),
        late = late.to_string().replace('\'', "''"),
        other = other.to_string().replace('\'', "''"),
    );
    spawn_peer(&script)
}

#[tokio::test]
async fn h100_ambiguous_handshake_envelopes_never_authorize_or_wait_for_eof() {
    for (index, body) in invalid_envelopes().into_iter().enumerate() {
        let (mut child, client) = envelope_peer(body, false);
        let result =
            tokio::time::timeout(Duration::from_secs(5), client.initialize("h100", "1")).await;
        let alive = child.try_wait().unwrap().is_none();
        let initialized = *client.initialized.lock().unwrap();
        let pending = client.pending.lock().unwrap().len();
        let decode = match &result {
            Ok(Err(SdkError::Decode(error))) => Some(error.to_string()),
            _ => None,
        };
        if let Some(cause) = &decode {
            let clone = client.clone();
            let late = clone.thread_list().await.unwrap_err();
            assert!(matches!(late, SdkError::Decode(_)));
            assert!(!late.to_string().contains("PRIVATE_PAYLOAD"));
            assert!(!cause.contains("PRIVATE_PAYLOAD"));
            assert_eq!(client.next_id.load(Ordering::Relaxed), 2);
        }
        finish(child, &client).await;
        assert!(
            decode.is_some() && alive && !initialized && pending == 0,
            "invalid envelope {index}: result={result:?}, alive={alive}, initialized={initialized}, pending={pending}"
        );
    }
}

#[tokio::test]
async fn h100_business_envelopes_settle_pending_and_retire_clones_without_replay() {
    for (index, body) in invalid_envelopes().into_iter().enumerate() {
        let (mut child, client) = envelope_peer(body, true);
        client.initialize("h100", "1").await.unwrap();
        let result = tokio::time::timeout(Duration::from_secs(5), async {
            tokio::join!(client.thread_list(), client.tool_list())
        })
        .await;
        let alive = child.try_wait().unwrap().is_none();
        let pending = client.pending.lock().unwrap().len();
        let rejected = matches!(
            &result,
            Ok((Err(SdkError::Decode(_)), Err(SdkError::Decode(_))))
        );
        if rejected {
            let (left, right) = result.as_ref().unwrap();
            assert_eq!(
                left.as_ref().unwrap_err().to_string(),
                right.as_ref().unwrap_err().to_string()
            );
            let clone = client.clone();
            let late = clone.thread_list().await.unwrap_err();
            assert!(matches!(late, SdkError::Decode(_)));
            assert!(!late.to_string().contains("PRIVATE_PAYLOAD"));
            assert_eq!(client.next_id.load(Ordering::Relaxed), 4);
        }
        finish(child, &client).await;
        assert!(
            rejected && alive && pending == 0,
            "invalid envelope {index}: result={result:?}, alive={alive}, pending={pending}"
        );
    }
}

#[tokio::test]
async fn h100_null_success_and_rpc_errors_keep_the_connection_usable() {
    for body in [
        json!({"result":null}),
        json!({"result":false}),
        json!({"result":0}),
        json!({"error":{"code":-32004,"message":"synthetic rejection"}}),
    ] {
        let (child, client) = envelope_peer(body.clone(), true);
        client.initialize("h100", "1").await.unwrap();
        let (response, other) = tokio::time::timeout(Duration::from_secs(5), async {
            tokio::join!(client.thread_list(), client.tool_list())
        })
        .await
        .unwrap();
        assert_eq!(other.unwrap().result()["source"], "other");
        assert!(!client.lifecycle.borrow().is_closed());
        assert!(client.pending.lock().unwrap().is_empty());
        finish(child, &client).await;
        if let Some(expected) = body.get("result") {
            let response = response.unwrap();
            assert!(
                response.is_success(),
                "null remains a present success member"
            );
            assert_eq!(response.result.as_ref(), Some(expected));
        } else {
            assert!(matches!(response, Err(SdkError::Rpc { code: -32004, .. })));
        }
    }
}

fn valid() -> Value {
    json!({"protocolVersion":1,"serverName":"synthetic-peer","serverVersion":"1",
        "capabilities":{"threadLifecycle":true,"streaming":true,"interrupt":true,
            "steer":true,"approval":true,"reconnect":true}})
}

async fn fixture(result: Value, rpc_error: bool) -> (Child, HarnessClient) {
    // One immediate reply, then a valid late duplicate and an ordered barrier.
    // Further initializes return the existing server's one-shot error contract.
    let first = if rpc_error {
        json!({"jsonrpc":"2.0","id":1,"error":{"code":-32602,"message":"synthetic rejection"}})
    } else {
        json!({"jsonrpc":"2.0","id":1,"result":result})
    };
    let late = json!({"jsonrpc":"2.0","id":1,"result":valid()});
    let script = format!(
        r#"$r = [Console]::In.ReadLine() | ConvertFrom-Json
if ($r.method -ne 'initialize' -or $r.params.protocolVersion -ne 1) {{ exit 31 }}
[Console]::Out.WriteLine('{}')
[Console]::Out.WriteLine('{}')
[Console]::Out.WriteLine('{{"jsonrpc":"2.0","method":"harness/event","params":{{"type":"thread.started","threadId":"handshake-barrier","protocolVersion":1}}}}')
while ($null -ne ($line = [Console]::In.ReadLine())) {{
  $r = $line | ConvertFrom-Json
  if ($r.method -eq 'initialize') {{
    [Console]::Out.WriteLine((@{{jsonrpc='2.0';id=$r.id;error=@{{code=-32002;message='initialize may only be called once'}}}} | ConvertTo-Json -Compress))
  }} else {{ [Console]::Out.WriteLine((@{{jsonrpc='2.0';id=$r.id;result=@{{accepted=$true}}}} | ConvertTo-Json -Compress)) }}
}}"#,
        first.to_string().replace('\'', "''"),
        late.to_string().replace('\'', "''")
    );
    spawn_peer(&script)
}

fn spawn_peer(script: &str) -> (Child, HarnessClient) {
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
    (child, client)
}

async fn barrier(events: &mut broadcast::Receiver<HarnessNotification>) {
    let event = tokio::time::timeout(Duration::from_secs(5), events.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(event.event, HarnessEvent::ThreadStarted { thread_id, .. } if thread_id == "handshake-barrier")
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

async fn assert_not_ready(client: &HarnessClient) {
    assert!(!*client.initialized.lock().unwrap());
    assert!(client.pending.lock().unwrap().is_empty());
    // Every business method shares this exact call gate; not only thread/start.
    for (method, params) in [
        ("thread/start", json!({})),
        ("thread/resume", json!({"threadId":"t"})),
        ("thread/list", json!({})),
        ("thread/read", json!({"threadId":"t"})),
        ("thread/fork", json!({"threadId":"t"})),
        ("thread/archive", json!({"threadId":"t"})),
        ("turn/start", json!({"threadId":"t","input":"synthetic"})),
        ("turn/interrupt", json!({"threadId":"t","turnId":"r"})),
        (
            "turn/steer",
            json!({"threadId":"t","turnId":"r","input":"synthetic"}),
        ),
        (
            "approval/respond",
            json!({"approvalId":"a","approved":true}),
        ),
        ("event/ack", json!({"eventSequence":1})),
        ("tool/list", json!({})),
        ("config/read", json!({})),
        ("sandbox/status", json!({})),
    ] {
        let request = serde_json::from_value(json!({"method":method,"params":params})).unwrap();
        assert!(
            matches!(client.call(request).await, Err(SdkError::NotInitialized)),
            "{method}"
        );
    }
    assert_eq!(
        client.next_id.load(Ordering::Relaxed),
        2,
        "no business frame was sent"
    );
}

#[tokio::test]
async fn h206_incompatible_or_malformed_results_never_authorize_even_after_late_success() {
    let mut cases = vec![Value::Null, json!([]), json!("PRIVATE_PAYLOAD"), json!({})];
    for version in [
        json!(0),
        json!(2),
        json!(999),
        json!(u64::MAX),
        json!(-1),
        json!(1.5),
        json!("1"),
        Value::Null,
    ] {
        let mut value = valid();
        value["protocolVersion"] = version;
        cases.push(value);
    }
    for field in [
        "protocolVersion",
        "serverName",
        "serverVersion",
        "capabilities",
    ] {
        let mut value = valid();
        value.as_object_mut().unwrap().remove(field);
        cases.push(value);
    }
    for field in ["serverName", "serverVersion"] {
        for bad in [json!(""), json!(" \t\n"), json!(17), Value::Null] {
            let mut value = valid();
            value[field] = bad;
            cases.push(value);
        }
    }
    for field in [
        "threadLifecycle",
        "streaming",
        "interrupt",
        "steer",
        "approval",
        "reconnect",
    ] {
        let mut value = valid();
        value["capabilities"].as_object_mut().unwrap().remove(field);
        cases.push(value);
        let mut value = valid();
        value["capabilities"][field] = json!("PRIVATE_PAYLOAD");
        cases.push(value);
    }
    let mut value = valid();
    value["capabilities"] = json!([]);
    cases.push(value);
    for (index, result) in cases.into_iter().enumerate() {
        let (child, client) = fixture(result, false).await;
        let mut events = client.subscribe();
        let response = client.initialize("h206", "1").await;
        barrier(&mut events).await;
        // Reap even on a red assertion, rather than orphaning the synthetic peer.
        let rejected = response.is_err();
        if rejected {
            assert!(!response
                .unwrap_err()
                .to_string()
                .contains("PRIVATE_PAYLOAD"));
            assert_not_ready(&client).await;
        }
        finish(child, &client).await;
        assert!(rejected, "malformed case {index} passed handshake");
    }
}

#[tokio::test]
async fn h206_rpc_error_and_duplicate_success_do_not_authorize() {
    let (child, client) = fixture(valid(), true).await;
    let mut events = client.subscribe();
    let result = client.initialize("h206", "1").await;
    barrier(&mut events).await;
    assert!(matches!(result, Err(SdkError::Rpc { code: -32602, .. })));
    assert_not_ready(&client).await;
    finish(child, &client).await;
}

#[tokio::test]
async fn h206_valid_identity_boolean_capabilities_and_extensions_keep_raw_response() {
    let mut value = valid();
    for flag in value["capabilities"].as_object_mut().unwrap().values_mut() {
        *flag = json!(false);
    }
    value["capabilities"]["futureCapability"] = json!(true);
    value["extension"] = json!({"retained":true});
    let (child, client) = fixture(value.clone(), false).await;
    let response = client.initialize("h206", "1").await.unwrap();
    assert_eq!(*response.result(), value);
    assert!(*client.initialized.lock().unwrap());
    assert!(client.thread_list().await.is_ok());
    // A repeated initialize still returns the server error; it does not erase
    // a previously validated connection or silently replay the handshake.
    assert!(matches!(
        client.initialize("h206", "1").await,
        Err(SdkError::Rpc { code: -32002, .. })
    ));
    assert!(client.tool_list().await.is_ok());
    assert!(client.pending.lock().unwrap().is_empty());
    finish(child, &client).await;
}

#[tokio::test]
async fn h206_timeout_or_dropped_initialize_cannot_be_authorized_by_late_valid_result() {
    use tokio::io::AsyncWriteExt;
    let reply = json!({"jsonrpc":"2.0","id":1,"result":valid()});
    let barrier_line = json!({"jsonrpc":"2.0","method":"harness/event","params":{
        "type":"thread.started","threadId":"handshake-barrier","protocolVersion":1}});
    let script = format!(
        r#"$null = [Console]::In.ReadLine()
[Console]::Out.WriteLine('{barrier_line}')
$null = [Console]::In.ReadLine()
[Console]::Out.WriteLine('{reply}')
[Console]::Out.WriteLine('{barrier_line}')
$null = [Console]::In.ReadLine()"#
    );
    for timeout in [true, false] {
        let (child, client) = spawn_peer(&script);
        let mut events = client.subscribe();
        let caller = if timeout {
            client
                .clone()
                .with_request_timeouts(RequestTimeouts {
                    initialize: Duration::from_millis(200),
                    request: Duration::from_secs(5),
                })
                .unwrap()
        } else {
            client.clone()
        };
        let task = tokio::spawn(async move { caller.initialize("h206", "1").await });
        barrier(&mut events).await;
        if timeout {
            assert!(
                matches!(task.await.unwrap(), Err(SdkError::RequestTimeout { ref method, .. }) if method == "initialize")
            );
        } else {
            task.abort();
            assert!(task.await.unwrap_err().is_cancelled());
        }
        assert_not_ready(&client).await;
        // Synthetic control, not a second request/cancel implementation.
        client
            .stdin
            .lock()
            .await
            .as_mut()
            .unwrap()
            .write_all(b"release\n")
            .await
            .unwrap();
        barrier(&mut events).await;
        assert_not_ready(&client).await;
        finish(child, &client).await;
    }
}
