//! Real stdio producer, production SDK reader, no model or external network.

use super::*;
use std::time::Duration;

struct Observation {
    terminal_delivered: bool,
    existing_wait_settled: bool,
    late_wait_settled: bool,
}

async fn observe(emit_terminal: bool) -> Observation {
    let script = format!(
        r#"$null = [Console]::In.ReadLine()
[Console]::Out.WriteLine('{{"jsonrpc":"2.0","id":1,"result":{{"protocolVersion":1,"serverName":"audit","serverVersion":"1"}}}}')
$null = [Console]::In.ReadLine()
[Console]::Out.WriteLine('{{"jsonrpc":"2.0","method":"harness/event","eventSequence":1,"params":{{"type":"turn.started","threadId":"audit-thread","turnId":"audit-turn"}}}}')
[Console]::Out.WriteLine('{{"jsonrpc":"2.0","id":2,"result":{{"threadId":"audit-thread","turnId":"audit-turn","status":"started"}}}}')
if (${emit_terminal}) {{
  [Console]::Out.WriteLine('{{"jsonrpc":"2.0","method":"harness/event","eventSequence":2,"params":{{"type":"turn.completed","threadId":"audit-thread","turnId":"audit-turn","message":"synthetic completion"}}}}')
}}
$null = [Console]::In.ReadLine()
exit 7"#,
        emit_terminal = if emit_terminal { "true" } else { "false" },
    );
    let mut child = tokio::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("spawn finite local protocol producer");
    let (events, _) = broadcast::channel(8);
    let client = HarnessClient {
        stdin: Arc::new(tokio::sync::Mutex::new(child.stdin.take())),
        next_id: Arc::new(AtomicI64::new(1)),
        pending: Arc::new(Mutex::new(HashMap::new())),
        events,
        initialized: Arc::new(Mutex::new(false)),
    };
    spawn_reader(
        child.stdout.take().unwrap(),
        child.stderr.take().unwrap(),
        client.pending.clone(),
        client.events.clone(),
    );
    let mut existing = client.subscribe();
    tokio::time::timeout(Duration::from_secs(5), client.initialize("audit", "1"))
        .await
        .unwrap()
        .unwrap();
    let started = tokio::time::timeout(
        Duration::from_secs(5),
        client.turn_start(TurnStartRequest {
            thread_id: "audit-thread".into(),
            input: "synthetic audit input".into(),
            provider: None,
            model: None,
            reasoning_effort: None,
            permission_profile: None,
            sandbox_backend: None,
        }),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(started.result()["turnId"], "audit-turn");
    let first = tokio::time::timeout(Duration::from_secs(3), existing.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(first, HarnessEvent::TurnStarted { .. }));
    let terminal_delivered = if emit_terminal {
        let terminal = tokio::time::timeout(Duration::from_secs(3), existing.recv())
            .await
            .unwrap()
            .unwrap();
        matches!(terminal, HarnessEvent::TurnCompleted { .. })
    } else {
        false
    };

    // The unanswered request is an EOF barrier: the production reader must
    // clear its pending table before this future can return the closed error.
    let closed_request = tokio::time::timeout(Duration::from_secs(5), client.thread_list())
        .await
        .unwrap();
    assert!(matches!(
        closed_request,
        Err(SdkError::ResponseChannelClosed { .. })
    ));
    assert!(client.pending.lock().unwrap().is_empty());
    let exit = tokio::time::timeout(Duration::from_secs(3), child.wait())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(exit.code(), Some(7));

    let mut late = client.subscribe();
    let (existing_wait, late_wait) = tokio::join!(
        tokio::time::timeout(Duration::from_millis(200), existing.recv()),
        tokio::time::timeout(Duration::from_millis(200), late.recv()),
    );
    let observation = Observation {
        terminal_delivered,
        existing_wait_settled: existing_wait.is_ok(),
        late_wait_settled: late_wait.is_ok(),
    };
    eprintln!(
        "SDK EOF audit: exit=7 pending=0 terminal={} existing_settled={} late_settled={}",
        observation.terminal_delivered,
        observation.existing_wait_settled,
        observation.late_wait_settled,
    );

    // Control: all producer-side reader handles really ended. Only the
    // retained public client was preventing broadcast closure after EOF.
    drop(client);
    for receiver in [&mut existing, &mut late] {
        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(3), receiver.recv()).await,
            Ok(Err(broadcast::error::RecvError::Closed))
        ));
    }
    observation
}

#[tokio::test]
#[ignore = "audit H-256: SDK event consumers must observe producer EOF while the client is retained"]
async fn audit_eof_settles_existing_and_late_event_subscribers() {
    let observed = observe(false).await;
    assert!(
        observed.existing_wait_settled && observed.late_wait_settled,
        "confirmed producer EOF must close/fail existing and new event waits without requiring the caller to drop every client clone"
    );
}

#[tokio::test]
async fn completed_event_before_eof_remains_deliverable() {
    let observed = observe(true).await;
    assert!(observed.terminal_delivered);
}
