//! H-207: actual wire results cross the same gate for raw and owned requests.
use super::*;
use serde_json::{json, Value};
use std::time::Duration;

fn contracts() -> Vec<Value> {
    serde_json::from_str(include_str!(
        "../../deepagent-harness-protocol/tests/fixtures/control_results_v1.json"
    ))
    .unwrap()
}

fn invalid_results(case: &Value) -> Vec<Value> {
    let valid = &case["result"];
    let mut values = vec![
        Value::Null,
        json!([]),
        json!("PRIVATE_PAYLOAD"),
        json!(17),
        json!({}),
    ];
    for field in valid.as_object().unwrap().keys() {
        let mut missing = valid.clone();
        missing.as_object_mut().unwrap().remove(field);
        values.push(missing);
        for wrong in [Value::Null, json!(""), json!(" \t"), json!({}), json!([])] {
            if wrong.is_array() && valid[field].is_array() {
                continue;
            }
            let mut bad = valid.clone();
            bad[field] = wrong;
            values.push(bad);
        }
    }
    let method = case["method"].as_str().unwrap();
    let echo = match method {
        "thread/resume" | "thread/archive" | "thread/unarchive" | "turn/start"
        | "turn/interrupt" | "turn/steer" => Some("threadId"),
        "thread/fork" => Some("sourceThreadId"),
        "approval/respond" => Some("approvalId"),
        _ => None,
    };
    if let Some(field) = echo {
        let mut bad = valid.clone();
        bad[field] = json!("PRIVATE_BAD_ID");
        values.push(bad);
    }
    if method == "turn/interrupt" || method == "turn/steer" {
        let mut bad = valid.clone();
        bad[if method == "turn/steer" {
            "replacesTurnId"
        } else {
            "turnId"
        }] = json!("PRIVATE_BAD_ID");
        values.push(bad);
    }
    if method == "turn/steer" {
        let mut bad = valid.clone();
        bad["turnId"] = case["params"]["turnId"].clone();
        values.push(bad);
    }
    if valid.get("status").is_some() {
        let mut bad = valid.clone();
        bad["status"] = json!("PRIVATE_BAD_STATUS");
        values.push(bad);
    }
    if method == "approval/respond" {
        let mut bad = valid.clone();
        bad["approved"] = json!(true);
        values.push(bad);
    }
    if method == "thread/archive" || method == "thread/unarchive" {
        let mut bad = valid.clone();
        bad["archived"] = json!(method != "thread/archive");
        values.push(bad);
    }
    if method == "event/ack" {
        for ack in [json!(6), json!(8), json!(-1), json!(1.5)] {
            let mut bad = valid.clone();
            bad["acknowledgedEventSequence"] = ack;
            values.push(bad);
        }
    }
    values
}

async fn fixture(valid: &Value, replies: Vec<Value>) -> (Child, HarnessClient) {
    let frames = replies
        .into_iter()
        .map(|reply| {
            let envelope = if reply.get("error").is_some() {
                json!({"jsonrpc":"2.0","id":0,"error":reply["error"]})
            } else {
                json!({"jsonrpc":"2.0","id":0,"result":reply})
            };
            format!("'{}'", envelope.to_string().replace('\'', "''"))
        })
        .collect::<Vec<_>>()
        .join(",\n");
    let duplicate = json!({"jsonrpc":"2.0","id":0,"result":valid})
        .to_string()
        .replace('\'', "''");
    let script = format!(
        r#"$frames=@({frames})
$index=0
$null=[Console]::In.ReadLine()
[Console]::Out.WriteLine('{{"jsonrpc":"2.0","id":1,"result":{{"protocolVersion":1,"serverName":"receipt-peer","serverVersion":"1","capabilities":{{"threadLifecycle":true,"streaming":true,"interrupt":true,"steer":true,"approval":true,"reconnect":true}}}}}}')
while ($null -ne ($line=[Console]::In.ReadLine())) {{
 $r=$line|ConvertFrom-Json
 if($r.method -eq 'tool/list') {{ [Console]::Out.WriteLine((@{{jsonrpc='2.0';id=$r.id;result=@{{calls=$index}}}}|ConvertTo-Json -Compress));continue }}
 [Console]::Out.WriteLine($frames[$index].Replace('"id":0',('"id":'+$r.id)))
 $index++
 [Console]::Out.WriteLine('{duplicate}'.Replace('"id":0',('"id":'+$r.id)))
 [Console]::Out.WriteLine('{{"jsonrpc":"2.0","method":"harness/event","params":{{"type":"thread.started","threadId":"receipt-barrier","protocolVersion":1}}}}')
}}"#
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
    client.initialize("h207", "1").await.unwrap();
    (child, client)
}

async fn barrier(events: &mut broadcast::Receiver<HarnessNotification>) {
    let event = tokio::time::timeout(Duration::from_secs(5), events.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(event.event,HarnessEvent::ThreadStarted{thread_id,..} if thread_id=="receipt-barrier")
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
async fn h207_invalid_mutation_receipts_fail_before_return_and_late_duplicates_do_not_retry() {
    for case in contracts() {
        let invalid = invalid_results(&case);
        let (child, client) = fixture(&case["result"], invalid.clone()).await;
        let mut events = client.subscribe();
        let mut accepted = None;
        for (index, _) in invalid.iter().enumerate() {
            let request =
                serde_json::from_value(json!({"method":case["method"],"params":case["params"]}))
                    .unwrap();
            let result = client.call(request).await;
            barrier(&mut events).await;
            assert!(client.pending.lock().unwrap().is_empty());
            if let Err(error) = result {
                assert!(
                    matches!(&error,SdkError::InvalidMethodResult(details) if details.method==case["method"].as_str().unwrap())
                );
                assert!(*client.initialized.lock().unwrap(), "handshake was valid");
                let message = error.to_string();
                assert!(!message.contains("PRIVATE_"));
            } else {
                accepted = Some(index);
                break;
            }
        }
        let calls = client.tool_list().await.unwrap().result()["calls"]
            .as_u64()
            .unwrap();
        finish(child, &client).await;
        assert!(
            accepted.is_none(),
            "{} invalid case {accepted:?} returned success",
            case["method"]
        );
        assert_eq!(calls, invalid.len() as u64, "no redispatch");
    }
}

#[tokio::test]
async fn h207_valid_receipts_preserve_original_extensions_and_rpc_errors() {
    for case in contracts() {
        let mut valid = case["result"].clone();
        valid["future"] = json!({"opaque":"retained"});
        let (child, client) = fixture(
            &valid,
            vec![
                valid.clone(),
                json!({"error":{"code":-32602,"message":"synthetic rejection"}}),
            ],
        )
        .await;
        let mut events = client.subscribe();
        let request = || {
            serde_json::from_value(json!({"method":case["method"],"params":case["params"]}))
                .unwrap()
        };
        assert_eq!(client.call(request()).await.unwrap().result(), &valid);
        barrier(&mut events).await;
        assert!(matches!(
            client.call(request()).await,
            Err(SdkError::Rpc { code: -32602, .. })
        ));
        barrier(&mut events).await;
        assert!(client.pending.lock().unwrap().is_empty());
        assert_eq!(client.tool_list().await.unwrap().result()["calls"], 2);
        finish(child, &client).await;
    }
}

#[tokio::test]
async fn h207_raw_and_owned_thread_turn_use_the_same_gate_and_release_receivers() {
    for method in ["thread/start", "turn/start"] {
        let case = contracts()
            .into_iter()
            .find(|c| c["method"] == method)
            .unwrap();
        let (child, client) = fixture(&case["result"], vec![json!({}), json!({})]).await;
        let mut events = client.subscribe();
        let thread = || ThreadStartRequest {
            cwd: None,
            provider: None,
            model: None,
            reasoning_effort: None,
            permission_profile: None,
            sandbox_backend: None,
        };
        let turn = || serde_json::from_value(case["params"].clone()).unwrap();
        let raw = if method == "thread/start" {
            client.thread_start(thread()).await
        } else {
            client.turn_start(turn()).await
        };
        barrier(&mut events).await;
        let owned = if method == "thread/start" {
            client.thread_start_streamed(thread()).await
        } else {
            client.turn_start_streamed(turn()).await
        };
        barrier(&mut events).await;
        let owned_error = owned.err();
        assert_eq!(client.event_receiver_count(), 1);
        finish(child, &client).await;
        assert!(
            raw.is_err() && owned_error.is_some(),
            "raw and owned must both reject"
        );
    }
}
