//! H-256: real stdio ownership, drained facts and born-closed subscriptions.
use super::*;
use serde_json::json;
use std::time::Duration;
use tokio::time::timeout;

async fn fixture(mode: &str, capacity: usize, terminal: bool) -> (Child, HarnessClient) {
    let script = format!(
        r#"$mode='{mode}'
$null=[Console]::In.ReadLine()
[Console]::Out.WriteLine('{{"jsonrpc":"2.0","id":1,"result":{{"protocolVersion":1,"serverName":"eof-peer","serverVersion":"1","capabilities":{{"threadLifecycle":true,"streaming":true,"interrupt":true,"steer":true,"approval":true,"reconnect":true}}}}}}')
$null=[Console]::In.ReadLine()
[Console]::Out.WriteLine('{{"jsonrpc":"2.0","method":"harness/event","eventSequence":1,"params":{{"type":"turn.started","threadId":"thread-1","turnId":"turn-1"}}}}')
foreach($index in 2..5) {{
 [Console]::Out.WriteLine((@{{jsonrpc='2.0';method='harness/event';eventSequence=$index;params=@{{type='item.updated';threadId='thread-1';turnId='turn-1';item=@{{kind='content_delta';text=[string]$index}}}}}}|ConvertTo-Json -Compress -Depth 10))
}}
if(${terminal}) {{ [Console]::Out.WriteLine('{{"jsonrpc":"2.0","method":"harness/event","eventSequence":6,"params":{{"type":"turn.completed","threadId":"thread-1","turnId":"turn-1","message":"original terminal"}}}}') }}
[Console]::Out.WriteLine('{{"jsonrpc":"2.0","id":2,"result":{{"threadId":"thread-1","turnId":"turn-1","status":"started"}}}}')
$null=[Console]::In.ReadLine()
if($mode -eq 'eof') {{ exit 7 }}
if($mode -eq 'read-error') {{ $output=[Console]::OpenStandardOutput();$output.Write([byte[]]@(255,10),0,2);$output.Flush();$null=[Console]::In.ReadLine();exit 0 }}
exit 0
"#,
        terminal = if terminal { "true" } else { "false" }
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
    (child, client)
}

async fn admit(client: &HarnessClient) {
    client.initialize("h256", "1").await.unwrap();
    let request =
        serde_json::from_value(json!({"threadId":"thread-1","input":"synthetic"})).unwrap();
    client.turn_start(request).await.unwrap();
}

async fn reap(mut child: Child, client: &HarnessClient, mode: &str) {
    client.close().await;
    let status = timeout(Duration::from_secs(5), child.wait())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(status.code(), Some(if mode == "eof" { 7 } else { 0 }));
}

#[tokio::test]
async fn h256_eof_and_real_utf8_read_error_close_raw_queues_with_clones_retained() {
    for mode in ["eof", "read-error"] {
        for terminal in [true, false] {
            let (mut child, client) = fixture(mode, 16, terminal).await;
            let clones = (0..8).map(|_| client.clone()).collect::<Vec<_>>();
            let mut raw = client.subscribe();
            admit(&client).await;
            assert!(matches!(
                client.thread_list().await,
                Err(SdkError::ResponseChannelClosed { .. })
            ));
            assert!(client.pending.lock().unwrap().is_empty());
            if mode == "read-error" {
                assert!(
                    child.try_wait().unwrap().is_none(),
                    "invalid UTF-8 retired a live pipe, not an exited process"
                );
            }
            let mut late = clones[0].subscribe();
            let born_closed =
                matches!(late.try_recv(), Err(broadcast::error::TryRecvError::Closed));
            let expected = if terminal { 6 } else { 5 };
            for index in 1..=expected {
                let notification = timeout(Duration::from_secs(3), raw.recv())
                    .await
                    .unwrap()
                    .unwrap();
                assert_eq!(notification.event_sequence, Some(index));
                assert!(matches!(
                    notification.sequence_status,
                    crate::SequenceStatus::First | crate::SequenceStatus::Continuous
                ));
                if index == 6 {
                    assert!(
                        matches!(notification.event,HarnessEvent::TurnCompleted{message,..} if message=="original terminal")
                    );
                }
            }
            let ended = matches!(
                timeout(Duration::from_millis(200), raw.recv()).await,
                Ok(Err(broadcast::error::RecvError::Closed))
            );
            reap(child, &client, mode).await;
            assert!(
                born_closed && ended,
                "{mode}, terminal={terminal}: retained handles cannot keep stream open"
            );
            assert_eq!(clones.len(), 8);
        }
    }
}

#[tokio::test]
async fn h256_lag_remains_explicit_then_retained_terminal_drains_before_closed() {
    let (child, client) = fixture("eof", 2, true).await;
    let mut raw = client.subscribe();
    admit(&client).await;
    assert!(client.thread_list().await.is_err());
    assert!(matches!(
        raw.try_recv(),
        Err(broadcast::error::TryRecvError::Lagged(4))
    ));
    assert_eq!(raw.recv().await.unwrap().event_sequence, Some(5));
    assert!(matches!(
        raw.recv().await.unwrap().event,
        HarnessEvent::TurnCompleted { .. }
    ));
    let closed = matches!(raw.try_recv(), Err(broadcast::error::TryRecvError::Closed));
    reap(child, &client, "eof").await;
    assert!(closed, "lag must not conceal stream closure");
}

#[tokio::test]
async fn h256_closing_one_connection_cannot_close_another_or_erase_its_queued_facts() {
    let (child_a, a) = fixture("close", 16, false).await;
    let (child_b, b) = fixture("close", 16, false).await;
    let mut a_events = a.subscribe();
    let mut b_events = b.subscribe();
    admit(&a).await;
    admit(&b).await;
    a.close().await;
    for _ in 0..5 {
        a_events.recv().await.unwrap();
        b_events.recv().await.unwrap();
    }
    assert!(matches!(
        timeout(Duration::from_secs(3), a_events.recv()).await,
        Ok(Err(broadcast::error::RecvError::Closed))
    ));
    assert!(timeout(Duration::from_millis(30), b_events.recv())
        .await
        .is_err());
    assert!(!*b.lifecycle.borrow());
    reap(child_a, &a, "close").await;
    reap(child_b, &b, "close").await;
}

#[tokio::test]
async fn h256_live_process_native_stdout_close_preserves_batch_approval_terminal_for_raw_and_owned()
{
    // A dedicated native producer avoids Node/PowerShell host-owned pipe copies.
    // Only its own redirected stdout HANDLE is closed; stdin keeps it alive.
    let source = r###"use std::io::{self,Write};
use std::ffi::c_void;
#[link(name="kernel32")]
extern "system" {
 fn GetStdHandle(id:u32)->*mut c_void;
 fn CloseHandle(handle:*mut c_void)->i32;
}
fn line(){let mut s=String::new();io::stdin().read_line(&mut s).unwrap();}
fn main(){
 line();
 println!(r#"{{"jsonrpc":"2.0","id":1,"result":{{"protocolVersion":1,"serverName":"live-eof-peer","serverVersion":"1","capabilities":{{"threadLifecycle":true,"streaming":true,"interrupt":true,"steer":true,"approval":true,"reconnect":true}}}}}}"#);
 line();
 let frames=[
 r#"{"jsonrpc":"2.0","method":"harness/event","eventSequence":1,"params":{"type":"turn.started","threadId":"thread-1","turnId":"turn-1"}}"#,
 r#"{"jsonrpc":"2.0","method":"harness/event","eventSequence":2,"params":{"type":"approval.requested","threadId":"thread-1","turnId":"turn-1","approvalId":"synthetic-approval","reason":"synthetic reason"}}"#,
 r#"{"jsonrpc":"2.0","method":"harness/event","eventSequence":3,"params":{"type":"turn.completed","threadId":"thread-1","turnId":"turn-1","message":"original terminal"}}"#,
 r#"{"jsonrpc":"2.0","id":2,"result":{"threadId":"thread-1","turnId":"turn-1","status":"started"}}"#];
 eprintln!("h256 synthetic stderr before native stdout EOF");
 println!("{}",frames.join("\n"));io::stdout().flush().unwrap();
 if unsafe{CloseHandle(GetStdHandle(4294967285))}==0{std::process::exit(32)}
 line();
}"###;
    let root = std::env::temp_dir().join(format!(
        "h256-native-producer-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    assert!(!root.exists());
    std::fs::create_dir(&root).unwrap();
    let source_path = root.join("producer.rs");
    let bin = root.join("producer.exe");
    std::fs::write(&source_path, source).unwrap();
    let compiler = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let built = timeout(
        Duration::from_secs(30),
        tokio::process::Command::new(compiler)
            .arg("--edition=2021")
            .arg(&source_path)
            .arg("-o")
            .arg(&bin)
            .kill_on_drop(true)
            .output(),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(
        built.status.success(),
        "native fixture build failed: {}",
        String::from_utf8_lossy(&built.stderr)
    );
    let mut child = tokio::process::Command::new(&bin)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("local synthetic native-stdout producer");
    let client = HarnessClient::from_stdio(
        child.stdin.take().unwrap(),
        child.stdout.take().unwrap(),
        child.stderr.take().unwrap(),
        8,
    );
    let retained = client.clone();
    let mut raw = client.subscribe();
    client.initialize("h256-live-eof", "1").await.unwrap();
    let mut owned = client
        .turn_start_streamed(
            serde_json::from_value(json!({"threadId":"thread-1","input":"synthetic"})).unwrap(),
        )
        .await
        .unwrap();
    for sequence in 1..=3 {
        let raw_event = timeout(Duration::from_secs(3), raw.recv())
            .await
            .unwrap()
            .unwrap();
        let owned_event = timeout(Duration::from_secs(3), owned.recv_notification())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(raw_event, owned_event);
        assert_eq!(raw_event.event_sequence, Some(sequence));
        if sequence == 2 {
            assert!(
                matches!(&raw_event.event,HarnessEvent::ApprovalRequested{approval_id:Some(id),..} if id=="synthetic-approval")
            );
        }
        if sequence == 3 {
            assert!(
                matches!(&raw_event.event,HarnessEvent::TurnCompleted{message,..} if message=="original terminal")
            );
        }
    }
    assert!(owned.recv().await.unwrap().is_none());
    assert!(matches!(
        timeout(Duration::from_secs(3), raw.recv()).await,
        Ok(Err(broadcast::error::RecvError::Closed))
    ));
    assert!(
        child.try_wait().unwrap().is_none(),
        "producer remains alive with stdin open"
    );
    assert!(matches!(
        retained.subscribe().try_recv(),
        Err(broadcast::error::TryRecvError::Closed)
    ));
    assert!(client.pending.lock().unwrap().is_empty());
    client.close().await;
    assert!(timeout(Duration::from_secs(3), child.wait())
        .await
        .unwrap()
        .unwrap()
        .success());
}

#[tokio::test]
async fn h256_parked_receivers_and_post_close_subscribers_settle_without_dropping_clients() {
    let (child, client) = fixture("close", 16, false).await;
    let mut control = client.subscribe();
    admit(&client).await;
    for _ in 0..5 {
        control.recv().await.unwrap();
    }
    // Cancelling recv must not close the live channel or synthesize an event.
    assert!(timeout(Duration::from_millis(30), control.recv())
        .await
        .is_err());
    let mut waits = Vec::new();
    let clones = (0..8).map(|_| client.clone()).collect::<Vec<_>>();
    for clone in &clones {
        let mut events = clone.subscribe();
        let (ready, started) = oneshot::channel();
        let task = tokio::spawn(async move {
            let _ = ready.send(());
            events.recv().await
        });
        started.await.unwrap();
        waits.push(task);
    }
    timeout(Duration::from_secs(3), client.close())
        .await
        .unwrap();
    client.close().await;
    let born_closed = matches!(
        clones[0].subscribe().try_recv(),
        Err(broadcast::error::TryRecvError::Closed)
    );
    let mut settled = true;
    for task in &mut waits {
        settled &= matches!(
            timeout(Duration::from_millis(200), task).await,
            Ok(Ok(Err(broadcast::error::RecvError::Closed)))
        );
    }
    for task in waits {
        task.abort();
    }
    reap(child, &client, "close").await;
    assert!(
        settled && born_closed,
        "explicit close must retire the sole producer, not only the RPC writer"
    );
}

async fn real_process(root: &Path, bin: &Path) -> HarnessProcess {
    let mut command = tokio::process::Command::new(bin);
    command
        .args(["server", "--transport", "stdio"])
        .current_dir(root)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    for (name, _) in std::env::vars_os() {
        let key = name.to_string_lossy().to_ascii_uppercase();
        if [
            "KEY",
            "TOKEN",
            "SECRET",
            "PASSWORD",
            "CREDENTIAL",
            "PROXY",
            "DEEPAGENT",
            "DEEPSEEK",
            "OPENAI",
            "ANTHROPIC",
        ]
        .iter()
        .any(|part| key.contains(part))
        {
            command.env_remove(name);
        }
    }
    for name in [
        "HOME",
        "USERPROFILE",
        "APPDATA",
        "LOCALAPPDATA",
        "XDG_CONFIG_HOME",
        "DEEPAGENT_DATA_ROOT",
    ] {
        let path = root.join(name.to_ascii_lowercase());
        std::fs::create_dir_all(&path).unwrap();
        command.env(name, path);
    }
    let mut child = command.spawn().unwrap();
    let client = HarnessClient::from_stdio(
        child.stdin.take().unwrap(),
        child.stdout.take().unwrap(),
        child.stderr.take().unwrap(),
        16,
    );
    HarnessProcess { child, client }
}

#[tokio::test]
#[ignore = "requires the current CLI at target/debug/deepagent.exe; mandatory H-256 acceptance"]
async fn h256_real_cli_os_kill_closes_retained_clients_without_inventing_run_terminals_and_reopens()
{
    let original = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/debug/deepagent.exe");
    assert!(original.is_file(), "build the current CLI first");
    let root = std::env::temp_dir().join(format!(
        "h256-real-cli-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let bin = root.join("deepagent.exe");
    std::fs::copy(&original, &bin).unwrap();
    assert_eq!(
        std::fs::read(&original).unwrap(),
        std::fs::read(&bin).unwrap()
    );
    let mut process = real_process(&root, &bin).await;
    let client = process.client();
    let retained = (0..4).map(|_| client.clone()).collect::<Vec<_>>();
    let mut raw = client.subscribe();
    client.initialize("h256-real", "1").await.unwrap();
    let mut admitted = client
        .thread_start_streamed(
            serde_json::from_value(json!({
                "permissionProfile":"read_only","sandboxBackend":"direct"
            }))
            .unwrap(),
        )
        .await
        .unwrap();
    let thread = admitted.thread_id().to_owned();
    assert!(matches!(
        admitted.recv().await.unwrap(),
        Some(HarnessEvent::ThreadStarted { .. })
    ));
    let read = || serde_json::from_value(json!({"threadId":thread})).unwrap();
    let before = client.thread_read(read()).await.unwrap();
    assert!(before.result()["runEvents"].as_array().unwrap().is_empty());
    process.child.kill().await.unwrap();
    let status = timeout(Duration::from_secs(5), process.child.wait())
        .await
        .unwrap()
        .unwrap();
    assert!(!status.success(), "actual OS kill, not graceful EOF");
    let queued = timeout(Duration::from_secs(5), raw.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(queued.event, HarnessEvent::ThreadStarted { thread_id, .. } if thread_id == thread)
    );
    assert!(matches!(
        timeout(Duration::from_secs(5), raw.recv()).await,
        Ok(Err(broadcast::error::RecvError::Closed))
    ));
    assert!(matches!(
        retained[0].subscribe().try_recv(),
        Err(broadcast::error::TryRecvError::Closed)
    ));
    assert!(client.pending.lock().unwrap().is_empty());
    assert!(client.thread_read(read()).await.is_err());
    let reopened = real_process(&root, &bin).await;
    let current = reopened.client();
    let mut reconnected_events = current.subscribe();
    current.initialize("h256-reopened", "1").await.unwrap();
    let after = current.thread_read(read()).await.unwrap();
    assert!(
        after.result()["runEvents"].as_array().unwrap().is_empty(),
        "disconnect must not invent a run terminal or redispatch"
    );
    assert_eq!(after.result()["events"], before.result()["events"]);
    assert_eq!(
        current.thread_list().await.unwrap().result()["threads"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert!(
        timeout(Duration::from_millis(30), reconnected_events.recv())
            .await
            .is_err(),
        "read/reopen emits no invented turn"
    );
    assert!(timeout(Duration::from_secs(5), reopened.wait())
        .await
        .unwrap()
        .unwrap()
        .success());
    assert!(matches!(
        timeout(Duration::from_secs(3), reconnected_events.recv()).await,
        Ok(Err(broadcast::error::RecvError::Closed))
    ));
    eprintln!("H-256 actual CLI killed; queued thread fact drained, retained subscriptions closed, SQLite reopen unchanged and no run dispatched");
}
