//! Audit the production SSH session reader with exact transport chunks. This
//! does not substitute the read implementation or claim a real remote login.
use super::*;
use crate::config::SshAuthType;

async fn session_with_output(bytes: &[u8]) -> Arc<SshSession> {
    let session = SshSession::new(SshConnectionConfig::new(
        "audit cursor",
        "127.0.0.1",
        22,
        "audit",
        SshAuthType::Agent,
    ));
    let (stdin, _input) = mpsc::channel(1);
    let (commands, _command_rx) = mpsc::unbounded_channel();
    let (output, stdout) = mpsc::channel(64);
    for chunk in bytes.chunks(4096) {
        output.send(chunk.to_vec()).await.unwrap();
    }
    drop(output);
    session
        .insert_pty(
            "fixture-pty".into(),
            PtyState {
                output_signal: Default::default(),
                stdin,
                commands,
                stdout: Mutex::new(stdout),
                join: tokio::spawn(async {}),
                channel: None,
                cols: 120,
                rows: 30,
                output_cursor: AtomicU64::new(0),
                history: Mutex::new(VecDeque::new()),
                history_bytes: Mutex::new(0),
            },
            false,
        )
        .await;
    session
}

#[tokio::test]
async fn audit_ssh_terminal_cursor_preserves_unreturned_output() {
    let small_payload = vec![b's'; 16 * 1024];
    let small = session_with_output(&small_payload).await;
    let first = small.pty_read_with_cursor("fixture-pty", 0).await.unwrap();
    let next = small
        .pty_read_with_cursor("fixture-pty", first.cursor)
        .await
        .unwrap();
    assert_eq!(first.data, small_payload, "small-output control");
    assert_eq!(first.cursor, small_payload.len() as u64);
    assert!(!first.truncated);
    assert!(next.data.is_empty());
    small.close_all_ptys().await;

    let payload: Vec<_> = (0..96 * 1024).map(|n| b'a' + (n % 26) as u8).collect();
    let session = session_with_output(&payload).await;
    let first = session
        .pty_read_with_cursor("fixture-pty", 0)
        .await
        .unwrap();
    let next = session
        .pty_read_with_cursor("fixture-pty", first.cursor)
        .await
        .unwrap();
    let suffix = session
        .pty_read_with_cursor("fixture-pty", first.data.len() as u64)
        .await
        .unwrap();
    session.close_all_ptys().await;
    eprintln!(
        "SSH cursor audit: source={} first={} cursor={} next={} suffix={} truncated={}",
        payload.len(),
        first.data.len(),
        first.cursor,
        next.data.len(),
        suffix.data.len(),
        first.truncated
    );
    assert_eq!(first.data, payload[..64 * 1024]);
    assert!(!first.truncated, "no retention eviction in this fixture");
    assert_eq!(suffix.data, payload[64 * 1024..]);
    let mut received = first.data;
    received.extend(next.data);
    assert_eq!(first.cursor, 64 * 1024);
    assert_eq!(received.len(), payload.len());
    assert_eq!(next.cursor, payload.len() as u64);
}
