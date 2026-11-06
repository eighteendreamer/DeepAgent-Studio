//! H-205: raw notification metadata survives the actual line reader and bounded bus.
use super::*;
use serde_json::{json, Value};
use std::time::Duration;

async fn fixture(frames: Vec<Value>, capacity: usize) -> (Child, HarnessClient) {
    let frames = frames
        .iter()
        .map(|value| format!("'{}'", value.to_string().replace('\'', "''")))
        .collect::<Vec<_>>()
        .join(",\n");
    let script = format!(
        r#"$frames = @({frames})
$null = [Console]::In.ReadLine()
foreach ($frame in $frames) {{ [Console]::Out.WriteLine($frame) }}
[Console]::Out.WriteLine('{{"jsonrpc":"2.0","id":1,"result":{{"threadId":"A","turnId":"run"}}}}')
while ($null -ne [Console]::In.ReadLine()) {{}}
"#
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
        capacity,
    );
    *client.initialized.lock().unwrap() = true;
    (child, client)
}

fn frame(sequence: Option<u64>, id: &str) -> Value {
    let mut value = json!({"jsonrpc":"2.0","method":"harness/event","params":{
        "type":"thread.started","threadId":id,"protocolVersion":1}});
    if let Some(sequence) = sequence {
        value["eventSequence"] = json!(sequence);
    }
    value
}

#[tokio::test]
async fn h205_single_bus_preserves_outer_sequence_and_reports_gaps_without_reordering() {
    let (mut child, client) = fixture(
        vec![
            frame(Some(7), "a"),
            frame(Some(8), "b"),
            frame(Some(10), "c"),
            frame(Some(10), "d"),
            frame(Some(9), "e"),
            frame(Some(u64::MAX), "f"),
            frame(None, "legacy"),
            frame(Some(0), "restart"),
        ],
        16,
    )
    .await;
    let mut receiver = client.subscribe();
    client.tool_list().await.unwrap();
    let mut received = Vec::new();
    for _ in 0..8 {
        received.push(
            serde_json::to_value(
                tokio::time::timeout(Duration::from_secs(3), receiver.recv())
                    .await
                    .unwrap()
                    .unwrap(),
            )
            .unwrap(),
        );
    }
    client.close().await;
    assert!(tokio::time::timeout(Duration::from_secs(3), child.wait())
        .await
        .unwrap()
        .unwrap()
        .success());
    assert_eq!(
        received
            .iter()
            .map(|value| value["eventSequence"].as_u64())
            .collect::<Vec<_>>(),
        vec![
            Some(7),
            Some(8),
            Some(10),
            Some(10),
            Some(9),
            Some(u64::MAX),
            None,
            Some(0)
        ]
    );
    assert_eq!(received[0]["sequenceStatus"]["kind"], "first");
    assert_eq!(received[1]["sequenceStatus"]["kind"], "continuous");
    assert_eq!(
        received[2]["sequenceStatus"],
        json!({"kind":"gap","previous":8,"missing":1})
    );
    for event in &received[3..5] {
        assert_eq!(
            event["sequenceStatus"],
            json!({"kind":"non_monotonic","previous":10})
        );
    }
    assert_eq!(
        received[5]["sequenceStatus"],
        json!({"kind":"gap","previous":10,"missing":u64::MAX-11})
    );
    assert_eq!(received[6]["sequenceStatus"]["kind"], "unavailable");
    assert_eq!(received[7]["sequenceStatus"]["kind"], "first");
}

#[tokio::test]
async fn h205_rejected_notifications_never_advance_sequence_or_take_broadcast_capacity() {
    let mut unknown = frame(Some(99), "PRIVATE_UNKNOWN");
    unknown["method"] = json!("other/event");
    let mut wrong_version = frame(Some(100), "PRIVATE_VERSION");
    wrong_version["jsonrpc"] = json!("1.0");
    let mut invalid = frame(Some(101), "PRIVATE_PAYLOAD");
    invalid["params"]["threadId"] = json!(17);
    let mut spoofed = frame(Some(8), "accepted");
    spoofed["params"]["eventSequence"] = json!(999);
    spoofed["params"]["sequenceStatus"] = json!({"kind":"gap","previous":0,"missing":999});
    let (mut child, client) = fixture(
        vec![
            frame(Some(7), "normal"),
            unknown,
            wrong_version,
            invalid,
            spoofed,
        ],
        4,
    )
    .await;
    let mut receiver = client.subscribe();
    client.tool_list().await.unwrap();
    let first = serde_json::to_value(receiver.try_recv().unwrap()).unwrap();
    let second = serde_json::to_value(receiver.try_recv().unwrap()).unwrap();
    client.close().await;
    assert!(tokio::time::timeout(Duration::from_secs(3), child.wait())
        .await
        .unwrap()
        .unwrap()
        .success());
    assert_eq!(first["eventSequence"], 7);
    assert_eq!(second["eventSequence"], 8);
    assert_eq!(second["sequenceStatus"]["kind"], "continuous");
    assert!(!serde_json::to_string(&[first, second])
        .unwrap()
        .contains("PRIVATE"));
    assert!(matches!(
        receiver.try_recv(),
        Err(broadcast::error::TryRecvError::Empty)
    ));
}

#[tokio::test]
async fn h205_raw_broadcast_lag_is_explicit_and_does_not_renumber_retained_notifications() {
    let (mut child, client) = fixture(
        (1..=8)
            .map(|sequence| frame(Some(sequence), "synthetic"))
            .collect(),
        2,
    )
    .await;
    let mut receiver = client.subscribe();
    client.tool_list().await.unwrap();
    assert!(matches!(
        receiver.try_recv(),
        Err(broadcast::error::TryRecvError::Lagged(6))
    ));
    assert_eq!(receiver.try_recv().unwrap().event_sequence, Some(7));
    assert_eq!(receiver.try_recv().unwrap().event_sequence, Some(8));
    client.close().await;
    assert!(tokio::time::timeout(Duration::from_secs(3), child.wait())
        .await
        .unwrap()
        .unwrap()
        .success());
}

#[tokio::test]
async fn h205_real_pipe_owned_stream_fails_before_scope_filter_can_hide_a_gap() {
    let (mut child,client)=fixture(vec![
        json!({"jsonrpc":"2.0","method":"harness/event","eventSequence":7,
            "params":{"type":"turn.started","threadId":"A","turnId":"run"}}),
        json!({"jsonrpc":"2.0","method":"harness/event","eventSequence":9,
            "params":{"type":"turn.completed","threadId":"B","turnId":"other","message":"synthetic"}}),
        json!({"jsonrpc":"2.0","method":"harness/event","eventSequence":10,
            "params":{"type":"turn.completed","threadId":"A","turnId":"run","message":"synthetic"}}),
    ],8).await;
    let mut raw = client.subscribe();
    let mut owned = client
        .turn_start_streamed(TurnStartRequest {
            thread_id: "A".into(),
            input: "synthetic".into(),
            expected_config_snapshot_id: None,
            provider: None,
            model: None,
            reasoning_effort: None,
            permission_profile: None,
            sandbox_backend: None,
        })
        .await
        .unwrap();
    assert_eq!(
        owned
            .recv_notification()
            .await
            .unwrap()
            .unwrap()
            .event_sequence,
        Some(7)
    );
    assert!(matches!(
        owned.recv().await,
        Err(SdkError::EventSequenceGap {
            previous: 7,
            received: 9,
            missing: 1
        })
    ));
    assert!(owned.recv_notification().await.unwrap().is_none());
    let records = (0..3).map(|_| raw.try_recv().unwrap()).collect::<Vec<_>>();
    assert_eq!(
        records
            .iter()
            .map(|value| value.event_sequence)
            .collect::<Vec<_>>(),
        vec![Some(7), Some(9), Some(10)]
    );
    assert_eq!(
        records[1].sequence_status,
        crate::SequenceStatus::Gap {
            previous: 7,
            missing: 1
        }
    );
    client.close().await;
    assert!(tokio::time::timeout(Duration::from_secs(3), child.wait())
        .await
        .unwrap()
        .unwrap()
        .success());
}
