//! Opt-in terminal audit. Runs a real Windows ConPTY shell against temporary
//! files and the production Direct backend, including SQLite cursor storage.
#![cfg(windows)]

use std::sync::Arc;
use std::time::Duration;

use deepagent_app_core::settings::TerminalShell;
use deepagent_app_core::{
    DirectTerminalSessionBackend, LocalPtyHandle, ProjectService, TerminalService,
};
use deepagent_builtins::SystemExecutor;
use deepagent_persistence::Database;
use deepagent_terminal::{TerminalInputHolder, TerminalOpenRequest, TerminalSessionBackend};

const TAIL: &str = "AUDIT_TERMINAL_PAYLOAD_TAIL";

#[derive(Debug)]
struct ReadObservation {
    first_bytes: usize,
    first_cursor: u64,
    persisted_cursor: u64,
    truncated: bool,
    next_bytes: usize,
    suffix_bytes: usize,
    tail_received: bool,
    tail_recoverable: bool,
}

async fn observe_direct_output(rows: usize) -> ReadObservation {
    let temp = tempfile::tempdir().unwrap();
    let mut payload = String::new();
    for row in 0..rows {
        payload.push_str(&format!("{row:06}:{}\r\n", "x".repeat(96)));
    }
    payload.push_str(TAIL);
    payload.push_str("\r\n");
    std::fs::write(temp.path().join("payload.txt"), payload).unwrap();
    let db = Arc::new(Database::open(&temp.path().join("audit.sqlite")).unwrap());
    let projects = Arc::new(ProjectService::new(db.clone()));
    let service = Arc::new(TerminalService::with_executor(
        projects,
        temp.path().to_string_lossy(),
        Arc::new(SystemExecutor),
    ));
    let backend = DirectTerminalSessionBackend::with_sqlite_lease_store(
        service.clone(),
        TerminalShell::CommandPrompt,
        db,
    )
    .unwrap();
    let (session, lease) = backend
        .open(TerminalOpenRequest {
            run_id: "audit-terminal-read".into(),
            cwd: temp.path().to_string_lossy().into_owned(),
            cols: 120,
            rows: 30,
            initial_holder: TerminalInputHolder::User,
        })
        .await
        .unwrap();
    // Cleanup precedes the defect assertions, including on a bounded wait
    // failure. This is a user-terminal command in an isolated test directory.
    let observed = tokio::time::timeout(Duration::from_secs(10), async {
        backend
            .write(
                &session,
                &lease,
                b"type payload.txt\r\necho done > ready.txt\r\n",
            )
            .await?;
        let ready = tokio::time::timeout(Duration::from_secs(3), async {
            while !temp.path().join("ready.txt").exists() {
                // A real terminal viewer drains the bounded transport queue.
                // Prime retained history through that production legacy read
                // before attaching the cursor-based observer under audit.
                service
                    .pty_read(&LocalPtyHandle {
                        pty_id: session.session_id.clone(),
                        cols: session.cols,
                        rows: session.rows,
                    })
                    .await
                    .unwrap();
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await;
        if ready.is_err() {
            let startup = backend.read(&session, 0).await?;
            return Err(deepagent_terminal::TerminalError::Backend(format!(
                "fixture shell did not execute command; startup={:?}",
                String::from_utf8_lossy(&startup.data[..startup.data.len().min(512)])
            )));
        }
        // The marker proves the shell finished the output command. Allow the
        // ConPTY reader thread to enqueue its final bytes before observing.
        tokio::time::sleep(Duration::from_millis(100)).await;
        let first = backend.read(&session, 0).await?;
        let persisted_cursor = backend.last_cursor(&session.session_id)?;
        let next = backend.read(&session, first.cursor).await?;
        // Independent observation: retained bytes after the actual returned
        // prefix must still exist; the test never rewrites production cursors.
        let suffix = backend.read(&session, first.data.len() as u64).await?;
        Ok::<_, deepagent_terminal::TerminalError>(ReadObservation {
            first_bytes: first.data.len(),
            first_cursor: first.cursor,
            persisted_cursor,
            truncated: first.truncated,
            next_bytes: next.data.len(),
            suffix_bytes: suffix.data.len(),
            tail_received: first
                .data
                .windows(TAIL.len())
                .chain(next.data.windows(TAIL.len()))
                .any(|window| window == TAIL.as_bytes()),
            tail_recoverable: suffix
                .data
                .windows(TAIL.len())
                .any(|window| window == TAIL.as_bytes()),
        })
    })
    .await;
    tokio::time::timeout(Duration::from_secs(5), backend.close(&session, &lease))
        .await
        .expect("terminal cleanup timed out")
        .expect("terminal cleanup failed");
    observed
        .expect("terminal command/read timed out")
        .expect("terminal command/read failed")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn audit_direct_terminal_cursor_preserves_unreturned_output() {
    let small = observe_direct_output(32).await;
    let large = observe_direct_output(1024).await;
    eprintln!("terminal cursor audit: small={small:?}; large={large:?}");
    assert!(small.tail_received, "small-output control must be received");
    assert!(!small.truncated);
    assert_eq!(small.first_cursor, small.first_bytes as u64);
    assert_eq!(small.next_bytes, 0);
    assert_eq!(small.suffix_bytes, 0);
    assert_eq!(large.first_bytes, 64 * 1024, "exercise the read cap");
    assert!(!large.truncated, "fixture must fit retained history");
    assert!(large.tail_recoverable, "suffix must still be in memory");
    assert!(large.suffix_bytes > 0);
    assert_eq!(large.persisted_cursor, large.first_cursor);
    assert_eq!(large.first_cursor, large.first_bytes as u64);
    assert!(
        large.tail_received,
        "the second page must contain the retained tail"
    );
    assert_eq!(large.next_bytes, large.suffix_bytes);
}
