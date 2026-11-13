//! H-204: actual stdio requests, local waiter ownership and no implicit replay/cancel.
use super::*;
use std::time::Duration;
use tokio::time::timeout;

async fn fixture(
    read_requests: bool,
) -> (
    Child,
    HarnessClient,
    broadcast::Receiver<HarnessNotification>,
) {
    let script = if read_requests {
        r#"$held = [System.Collections.Generic.List[object]]::new()
$starts = 0
while ($null -ne ($line = [Console]::In.ReadLine())) {
  $r = $line | ConvertFrom-Json
  if ($r.method -eq 'thread/list' -or $r.method -eq 'thread/start' -or ($r.method -eq 'initialize' -and $r.params.clientName -eq 'hold')) {
    if ($r.method -eq 'thread/start') { $starts++ }
    $held.Add($r.id)
    [Console]::Out.WriteLine((@{jsonrpc='2.0';method='harness/event';params=@{type='thread.started';threadId=('received-'+$r.id);protocolVersion=1}} | ConvertTo-Json -Compress))
    continue
  }
  if ($r.method -eq 'config/read') { exit 7 }
  if ($r.method -eq 'tool/list') {
    foreach ($id in $held) {
      [Console]::Out.WriteLine((@{jsonrpc='2.0';id=$id;result=@{late=$true}} | ConvertTo-Json -Compress))
    }
    $held.Clear()
  }
  [Console]::Out.WriteLine((@{jsonrpc='2.0';id=$r.id;result=@{protocolVersion=1;serverName='lifecycle-fixture';serverVersion='1';capabilities=@{threadLifecycle=$true;streaming=$true;interrupt=$true;steer=$true;approval=$true;reconnect=$true};starts=$starts}} | ConvertTo-Json -Compress -Depth 10))
}"#
    } else {
        r#"[Console]::Out.WriteLine('{"jsonrpc":"2.0","method":"harness/event","params":{"type":"thread.started","threadId":"ready","protocolVersion":1}}'); $null=[Console]::In.Read(); [Console]::Out.WriteLine('{"jsonrpc":"2.0","method":"harness/event","params":{"type":"thread.started","threadId":"prefix-received","protocolVersion":1}}'); Start-Sleep -Seconds 30"#
    };
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
        256,
    );
    *client.initialized.lock().unwrap() = true;
    let receiver = client.subscribe();
    (child, client, receiver)
}

async fn received(receiver: &mut broadcast::Receiver<HarnessNotification>) {
    let event = timeout(Duration::from_secs(5), receiver.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(event.event, HarnessEvent::ThreadStarted { .. }));
}

async fn finish(mut child: Child, client: &HarnessClient) -> std::process::ExitStatus {
    client.close().await;
    match timeout(Duration::from_secs(3), child.wait()).await {
        Ok(status) => status.unwrap(),
        Err(_) => {
            child.kill().await.unwrap();
            child.wait().await.unwrap()
        }
    }
}

#[tokio::test]
async fn h204_dropped_sent_calls_release_only_their_own_pending_before_eof() {
    let (child, client, mut events) = fixture(true).await;
    for _ in 0..32 {
        let request_client = client.clone();
        let request = tokio::spawn(async move { request_client.thread_list().await });
        received(&mut events).await;
        assert_eq!(client.pending.lock().unwrap().len(), 1);
        request.abort();
        assert!(request.await.unwrap_err().is_cancelled());
        assert!(
            client.pending.lock().unwrap().is_empty(),
            "cancelled waiter retained"
        );
        // Replies to abandoned IDs must not complete this later request.
        assert_eq!(client.tool_list().await.unwrap().result()["starts"], 0);
    }
    finish(child, &client).await;
}

#[tokio::test]
async fn h204_dropped_waiter_for_writer_does_not_send_or_poison_other_requests() {
    let (child, client, _) = fixture(true).await;
    let locked = client.stdin.lock().await;
    let caller = client.clone();
    let request = tokio::spawn(async move { caller.thread_list().await });
    timeout(Duration::from_secs(3), async {
        while client.pending.lock().unwrap().is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    request.abort();
    assert!(request.await.unwrap_err().is_cancelled());
    assert!(client.pending.lock().unwrap().is_empty());
    drop(locked);
    assert_eq!(client.tool_list().await.unwrap().result()["starts"], 0);
    finish(child, &client).await;
}

#[tokio::test]
async fn h204_dropped_partial_frame_closes_writer_instead_of_corrupting_next_rpc() {
    let (mut child, client, mut events) = fixture(false).await;
    received(&mut events).await;
    let caller = client.clone();
    let request = tokio::spawn(async move {
        caller
            .turn_start(TurnStartRequest {
                thread_id: "synthetic".into(),
                input: "X".repeat(8 * 1024 * 1024),
                expected_config_snapshot_id: None,
                provider: None,
                model: None,
                reasoning_effort: None,
                permission_profile: None,
                sandbox_backend: None,
            })
            .await
    });
    received(&mut events).await; // Peer read a real byte, but cannot consume the whole frame.
    timeout(Duration::from_secs(5), async {
        loop {
            if !client.pending.lock().unwrap().is_empty() && client.stdin.try_lock().is_err() {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    request.abort();
    assert!(request.await.unwrap_err().is_cancelled());
    assert!(client.pending.lock().unwrap().is_empty());
    assert!(
        client.stdin.lock().await.is_none(),
        "interrupted frame left reusable pipe"
    );
    assert!(client.lifecycle.borrow().is_closed());
    child.kill().await.unwrap();
    child.wait().await.unwrap();
}

#[tokio::test]
async fn h204_write_deadline_and_explicit_close_retire_partial_frames_and_release_all_waiters() {
    for close in [false, true] {
        let (mut child, client, mut events) = fixture(false).await;
        received(&mut events).await;
        let caller = bounded(&client, 5000, 2000);
        let request = tokio::spawn(async move {
            caller
                .turn_start(TurnStartRequest {
                    thread_id: "synthetic".into(),
                    input: "X".repeat(8 * 1024 * 1024),
                    expected_config_snapshot_id: None,
                    provider: None,
                    model: None,
                    reasoning_effort: None,
                    permission_profile: None,
                    sandbox_backend: None,
                })
                .await
        });
        received(&mut events).await; // Real pipe has consumed a prefix, not a complete frame.
        if close {
            timeout(Duration::from_secs(3), client.close())
                .await
                .unwrap();
        }
        let result = timeout(Duration::from_secs(5), request)
            .await
            .unwrap()
            .unwrap();
        if close {
            assert!(result.is_err());
        } else {
            deadline(result.unwrap_err(), "turn/start", true);
        }
        assert!(client.pending.lock().unwrap().is_empty());
        assert!(client.stdin.lock().await.is_none());
        assert!(client.lifecycle.borrow().is_closed());
        assert!(
            matches!(
                timeout(Duration::from_secs(1), events.recv()).await,
                Ok(Err(broadcast::error::RecvError::Closed))
            ),
            "retiring a partial frame must also retire the event producer"
        );
        assert!(matches!(
            client.subscribe().try_recv(),
            Err(broadcast::error::TryRecvError::Closed)
        ));
        assert!(timeout(Duration::from_secs(1), client.tool_list())
            .await
            .unwrap()
            .is_err());
        child.kill().await.unwrap();
        child.wait().await.unwrap();
    }
}

#[tokio::test]
async fn h204_unpolled_future_and_invalid_policy_create_no_pending_or_frame() {
    let (child, client, _) = fixture(true).await;
    let id = client.next_id.load(Ordering::Relaxed);
    drop(client.thread_list());
    assert_eq!(client.next_id.load(Ordering::Relaxed), id);
    for invalid in [Duration::ZERO, Duration::MAX] {
        assert!(matches!(
            client.clone().with_request_timeouts(RequestTimeouts {
                request: invalid,
                ..RequestTimeouts::default()
            }),
            Err(SdkError::InvalidRequestTimeout)
        ));
    }
    assert!(client.pending.lock().unwrap().is_empty());
    assert_eq!(client.next_id.load(Ordering::Relaxed), id);
    assert!(client.tool_list().await.is_ok());
    finish(child, &client).await;
}

fn bounded(client: &HarnessClient, initialize: u64, request: u64) -> HarnessClient {
    client
        .clone()
        .with_request_timeouts(RequestTimeouts {
            initialize: Duration::from_millis(initialize),
            request: Duration::from_millis(request),
        })
        .unwrap()
}

fn deadline(error: SdkError, expected_method: &str, unknown: bool) -> i64 {
    assert!(!error.to_string().contains("PRIVATE_PARAM"));
    match error {
        SdkError::RequestTimeout {
            id,
            method,
            outcome_unknown,
        } => {
            assert_eq!(method, expected_method);
            assert_eq!(outcome_unknown, unknown);
            id
        }
        other => panic!("expected local timeout, got {other:?}"),
    }
}

#[tokio::test]
async fn h204_request_deadline_releases_sent_waiters_and_late_replies_do_not_replay() {
    let (child, client, mut events) = fixture(true).await;
    assert_eq!(client.tool_list().await.unwrap().result()["starts"], 0); // Ready and normal control.
    let short = bounded(&client, 2000, 75);
    for _ in 0..8 {
        let error = timeout(Duration::from_secs(3), short.thread_list())
            .await
            .unwrap()
            .unwrap_err();
        deadline(error, "thread/list", true);
        received(&mut events).await;
        assert!(client.pending.lock().unwrap().is_empty());
        assert!(
            !client.lifecycle.borrow().is_closed(),
            "reply timeout must not close a valid writer"
        );
        assert_eq!(client.tool_list().await.unwrap().result()["starts"], 0);
    }
    finish(child, &client).await;
}

#[tokio::test]
async fn h204_deadline_while_queued_sends_nothing_and_preserves_the_active_writer() {
    let (child, client, _) = fixture(true).await;
    let short = bounded(&client, 2000, 50);
    let locked = client.stdin.lock().await;
    let error = timeout(Duration::from_secs(3), short.thread_list())
        .await
        .unwrap()
        .unwrap_err();
    deadline(error, "thread/list", false);
    assert!(client.pending.lock().unwrap().is_empty());
    assert!(!client.lifecycle.borrow().is_closed());
    drop(locked);
    assert!(client.tool_list().await.is_ok());
    finish(child, &client).await;
}

#[tokio::test]
async fn h204_initialize_has_its_own_deadline_and_late_reply_cannot_authorize_calls() {
    let (child, client, mut events) = fixture(true).await;
    assert!(client.tool_list().await.is_ok());
    *client.initialized.lock().unwrap() = false;
    let short = bounded(&client, 75, 5000);
    let error = timeout(
        Duration::from_secs(3),
        short.initialize("hold", "PRIVATE_PARAM"),
    )
    .await
    .unwrap()
    .unwrap_err();
    deadline(error, "initialize", true);
    received(&mut events).await;
    assert!(!*client.initialized.lock().unwrap());
    assert!(client.pending.lock().unwrap().is_empty());
    assert!(matches!(
        client.thread_list().await,
        Err(SdkError::NotInitialized)
    ));
    assert!(client.initialize("normal", "1").await.is_ok());
    assert!(client.tool_list().await.is_ok()); // Also releases the stale initialize reply.
    assert!(client.pending.lock().unwrap().is_empty());
    finish(child, &client).await;
}

#[tokio::test]
async fn h204_local_timeout_of_admitted_mutation_is_unknown_and_readback_is_explicit() {
    let (child, client, mut events) = fixture(true).await;
    assert!(client.tool_list().await.is_ok());
    let short = bounded(&client, 2000, 100);
    let request = ThreadStartRequest {
        cwd: Some("PRIVATE_PARAM".into()),
        provider: None,
        model: None,
        reasoning_effort: None,
        permission_profile: None,
        sandbox_backend: None,
    };
    let error = short.thread_start(request).await.unwrap_err();
    deadline(error, "thread/start", true);
    received(&mut events).await;
    assert!(client.pending.lock().unwrap().is_empty());
    // The fixture accepted exactly once. SDK timeout/drop sent no remote interrupt/retry.
    let readback = client
        .thread_read(ThreadReadRequest {
            thread_id: "synthetic".into(),
            page_size: None,
            max_bytes: None,
            cursor: None,
            after_sequence: None,
            session_after_sequence: None,
            run_after_sequence: None,
            run_after_sequences: None,
        })
        .await
        .unwrap();
    assert_eq!(readback.result()["starts"], 1);
    assert_eq!(client.tool_list().await.unwrap().result()["starts"], 1);
    finish(child, &client).await;
}

#[tokio::test]
async fn h204_drop_and_eof_settle_only_owned_ids_and_do_not_leak() {
    let (mut child, client, mut events) = fixture(true).await;
    let caller = client.clone();
    let first = tokio::spawn(async move { caller.thread_list().await });
    received(&mut events).await;
    let caller = client.clone();
    let second = tokio::spawn(async move { caller.thread_list().await });
    received(&mut events).await;
    first.abort();
    assert!(first.await.unwrap_err().is_cancelled());
    assert_eq!(client.pending.lock().unwrap().len(), 1);
    assert!(client.tool_list().await.is_ok());
    assert_eq!(second.await.unwrap().unwrap().result()["late"], true);
    let caller = client.clone();
    let held = tokio::spawn(async move { caller.thread_list().await });
    received(&mut events).await;
    assert!(client.config_read().await.is_err()); // Peer exits with a withheld request alive.
    assert!(timeout(Duration::from_secs(3), held)
        .await
        .unwrap()
        .unwrap()
        .is_err());
    assert!(client.pending.lock().unwrap().is_empty());
    assert_eq!(
        timeout(Duration::from_secs(3), child.wait())
            .await
            .unwrap()
            .unwrap()
            .code(),
        Some(7)
    );
}

#[test]
fn h204_deadline_policy_is_finite_and_invalid_configuration_is_rejected() {
    assert_eq!(
        RequestTimeouts::default().initialize,
        Duration::from_secs(30)
    );
    assert_eq!(RequestTimeouts::default().request, Duration::from_secs(60));
    for duration in [Duration::ZERO, Duration::MAX] {
        assert!(matches!(
            RequestTimeouts {
                initialize: duration,
                ..RequestTimeouts::default()
            }
            .validate(),
            Err(SdkError::InvalidRequestTimeout)
        ));
        assert!(matches!(
            RequestTimeouts {
                request: duration,
                ..RequestTimeouts::default()
            }
            .validate(),
            Err(SdkError::InvalidRequestTimeout)
        ));
    }
}

async fn real_cli_relay(bin: &Path, root: &Path) -> (Child, HarnessClient) {
    // Test-only sequential line relay. It drops exactly the first thread/start
    // reply, not the CLI's admission, notifications, runtime or persistence.
    let script = r#"$si = [System.Diagnostics.ProcessStartInfo]::new()
$si.FileName = $env:DEEPAGENT_H204_FIXTURE_CLI
$si.Arguments = 'server --transport stdio'
$si.UseShellExecute = $false; $si.CreateNoWindow = $true
$si.RedirectStandardInput = $true; $si.RedirectStandardOutput = $true; $si.RedirectStandardError = $true
$p = [System.Diagnostics.Process]::Start($si)
$errors = $p.StandardError.ReadToEndAsync()
$held = $null
try {
 while ($null -ne ($line = [Console]::In.ReadLine())) {
  $r = $line | ConvertFrom-Json
  if ($null -ne $held) { [Console]::Out.WriteLine($held); $held = $null }
  $p.StandardInput.WriteLine($line); $p.StandardInput.Flush()
  while ($null -ne ($out = $p.StandardOutput.ReadLine())) {
   $v = $out | ConvertFrom-Json
   if ($null -ne $v.id -and $v.id -eq $r.id) {
    if ($r.method -eq 'thread/start') { $held = $out } else { [Console]::Out.WriteLine($out) }
    break
   }
   [Console]::Out.WriteLine($out)
  }
 }
} finally {
 $p.StandardInput.Close()
 if (!$p.WaitForExit(5000)) { $p.Kill(); $p.WaitForExit(); exit 9 }
 if ($p.ExitCode -ne 0) { exit $p.ExitCode }
}"#;
    let mut command = tokio::process::Command::new("powershell.exe");
    command
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
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
    let mut child = command
        .env("DEEPAGENT_H204_FIXTURE_CLI", bin)
        .spawn()
        .unwrap();
    let client = HarnessClient::from_stdio(
        child.stdin.take().unwrap(),
        child.stdout.take().unwrap(),
        child.stderr.take().unwrap(),
        64,
    );
    (child, client)
}

#[tokio::test]
#[ignore = "requires the current built CLI at target/debug/deepagent.exe; mandatory H-204 acceptance"]
async fn h204_real_cli_withheld_admission_reply_reconciles_once_and_survives_database_reopen() {
    let binary = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/debug/deepagent.exe");
    assert!(
        binary.is_file(),
        "build the current CLI before running this acceptance"
    );
    let root = std::env::temp_dir().join(format!(
        "h204-real-cli-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let isolated_bin = root.join("deepagent.exe");
    std::fs::copy(&binary, &isolated_bin).unwrap();
    assert_eq!(
        std::fs::read(binary).unwrap(),
        std::fs::read(&isolated_bin).unwrap()
    );
    let (child, client) = real_cli_relay(&isolated_bin, &root).await;
    client.initialize("h204-real-cli", "1").await.unwrap();
    assert!(client.thread_list().await.unwrap().result()["threads"]
        .as_array()
        .unwrap()
        .is_empty());
    let mut events = client.subscribe();
    let short = bounded(&client, 5000, 500);
    let id = deadline(
        short
            .thread_start(ThreadStartRequest {
                cwd: None,
                provider: None,
                model: None,
                reasoning_effort: None,
                permission_profile: None,
                sandbox_backend: None,
            })
            .await
            .unwrap_err(),
        "thread/start",
        true,
    );
    assert!(client.pending.lock().unwrap().is_empty());
    assert_eq!(
        client.next_id.load(Ordering::Relaxed),
        id + 1,
        "no implicit retry"
    );
    // Explicitly query accepted state, not replay the mutation. This sequential
    // relay resumes reading CLI stdout on the next request, including a real
    // notification that may have been queued after the withheld response.
    let listed = client.thread_list().await.unwrap();
    let threads = listed.result()["threads"].as_array().unwrap();
    assert_eq!(
        threads.len(),
        1,
        "one actual persisted admission, not a synthetic counter"
    );
    let thread_id = timeout(Duration::from_secs(5), async {
        loop {
            if let HarnessEvent::ThreadStarted { thread_id, .. } =
                events.recv().await.unwrap().event
            {
                break thread_id;
            }
        }
    })
    .await
    .unwrap();
    assert!(client.pending.lock().unwrap().is_empty());
    assert_eq!(threads[0]["threadId"], thread_id);
    let read = || ThreadReadRequest {
        thread_id: thread_id.clone(),
        page_size: None,
        max_bytes: None,
        cursor: None,
        after_sequence: None,
        session_after_sequence: None,
        run_after_sequence: None,
        run_after_sequences: None,
    };
    let before = client.thread_read(read()).await.unwrap(); // Also receives the discarded late response first.
    assert_eq!(before.result()["threadId"], thread_id);
    assert!(client.pending.lock().unwrap().is_empty());
    assert!(
        finish(child, &client).await.success(),
        "relay must confirm clean CLI exit before reopen"
    );
    let (child, reopened) = real_cli_relay(&isolated_bin, &root).await;
    reopened.initialize("h204-reopened", "1").await.unwrap();
    let after = reopened.thread_read(read()).await.unwrap();
    assert_eq!(after.result(), before.result());
    assert_eq!(
        reopened.thread_list().await.unwrap().result()["threads"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert!(
        finish(child, &reopened).await.success(),
        "reopened CLI must also exit cleanly"
    );
    eprintln!("H-204 real CLI accepted one thread/start; local timeout unknown, late reply ignored, explicit read/reopen agree");
}
