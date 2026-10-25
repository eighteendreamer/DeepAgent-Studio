//! Accelerated real-clock lease expiry through the production Direct PTY adapter.
#![cfg(windows)]

use deepagent_app_core::settings::TerminalShell;
use deepagent_app_core::{
    DirectTerminalSessionBackend, LocalPtyHandle, ProjectService, SqliteTerminalLeaseStore,
    TerminalService,
};
use deepagent_builtins::SystemExecutor;
use deepagent_persistence::Database;
use deepagent_terminal::{
    TerminalInputHolder, TerminalOpenRequest, TerminalSessionBackend, TerminalSignal,
};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const TTL_MS: i64 = 2_000;

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

#[derive(Debug)]
struct Observation {
    renewed: bool,
    initial_command: bool,
    middle_command: bool,
    final_command: bool,
    reads: usize,
    durable_valid: bool,
    operations: Vec<(&'static str, Result<(), String>)>,
    available_after_close: bool,
    legacy_probe_executed: bool,
}

async fn observe(renew: bool) -> Observation {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("terminal.sqlite");
    let db = Arc::new(Database::open(&path).expect("file db"));
    let store = Arc::new(SqliteTerminalLeaseStore::new(db.clone(), TTL_MS).expect("short TTL"));
    let service = Arc::new(TerminalService::with_executor(
        Arc::new(ProjectService::new(db)),
        dir.path().to_string_lossy(),
        Arc::new(SystemExecutor),
    ));
    let backend = DirectTerminalSessionBackend::with_lease_persistence(
        service.clone(),
        TerminalShell::CommandPrompt,
        Some(store.clone()),
    );
    let (session, lease) = backend
        .open(TerminalOpenRequest {
            run_id: "terminal-lease-liveness-audit".to_owned(),
            cwd: dir.path().to_string_lossy().into_owned(),
            cols: 100,
            rows: 30,
            initial_holder: TerminalInputHolder::User,
        })
        .await
        .expect("real ConPTY open");
    let handle = LocalPtyHandle {
        pty_id: session.session_id.clone(),
        cols: session.cols,
        rows: session.rows,
    };

    // Every bounded observation result, including a fixture failure, reaches
    // the independent host cleanup below before any final test assertion.
    let result = tokio::time::timeout(Duration::from_secs(10), async {
        let began = Instant::now();
        backend
            .write(&session, &lease, b"echo initial > initial.txt\r\n")
            .await
            .map_err(|e| e.to_string())?;
        let mut middle_sent = false;
        let mut reads = 0;
        let mut cursor = 0;
        while began.elapsed() < Duration::from_millis(TTL_MS as u64 + 250) {
            if renew {
                store.renew(&lease, now()).map_err(|e| e.to_string())?;
            }
            if !middle_sent && began.elapsed() > Duration::from_millis(500) {
                backend
                    .write(&session, &lease, b"echo middle > middle.txt\r\n")
                    .await
                    .map_err(|e| e.to_string())?;
                middle_sent = true;
            }
            cursor = backend
                .read(&session, cursor)
                .await
                .map_err(|e| e.to_string())?
                .cursor;
            reads += 1;
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let reopened = SqliteTerminalLeaseStore::new(
            Arc::new(Database::open(&path).map_err(|e| e.to_string())?),
            TTL_MS,
        )
        .map_err(|e| e.to_string())?;
        let durable_valid = reopened
            .validate(&lease, now())
            .map_err(|e| e.to_string())?;
        let mut operations = Vec::new();
        operations.push((
            "write",
            backend
                .write(&session, &lease, b"echo final > final.txt\r\n")
                .await
                .map_err(|e| e.to_string()),
        ));
        // Establish command completion before injecting an interrupt in the
        // healthy control; Ctrl-C must not race its marker command.
        if renew {
            tokio::time::timeout(Duration::from_secs(1), async {
                while !dir.path().join("final.txt").exists() {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .map_err(|_| "final control command did not execute".to_owned())?;
        }
        operations.push((
            "resize",
            backend
                .resize(&session, &lease, 110, 32)
                .await
                .map_err(|e| e.to_string()),
        ));
        // Interrupt affects only this idle synthetic shell, not any user process.
        operations.push((
            "interrupt",
            backend
                .signal(&session, &lease, TerminalSignal::Interrupt)
                .await
                .map_err(|e| e.to_string()),
        ));
        let mut current = lease.clone();
        let taken = backend
            .takeover(&session, TerminalInputHolder::Runtime)
            .await;
        let take_result = match taken {
            Ok(next) => {
                current = next;
                Ok(())
            }
            Err(error) => Err(error.to_string()),
        };
        operations.push(("takeover", take_result));
        let released = backend
            .release(&session, &current, TerminalInputHolder::User)
            .await;
        let release_result = match released {
            Ok(next) => {
                current = next;
                Ok(())
            }
            Err(error) => Err(error.to_string()),
        };
        operations.push(("release", release_result));
        operations.push((
            "close",
            backend
                .close(&session, &current)
                .await
                .map_err(|e| e.to_string()),
        ));
        let available_after_close = backend
            .recovery_status(&session.session_id)
            .await
            .map_err(|e| e.to_string())?
            .available;
        let mut legacy_probe_executed = false;
        if available_after_close {
            // This proves the shell is really alive, not just a stale map row.
            // It also bounds the finding: the legacy host escape hatch still works.
            service
                .pty_write(&handle, b"echo alive > alive.txt\r\n")
                .await
                .map_err(|e| e.to_string())?;
            tokio::time::timeout(Duration::from_secs(1), async {
                while !dir.path().join("alive.txt").exists() {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .map_err(|_| "legacy liveness probe did not execute".to_owned())?;
            legacy_probe_executed = true;
        }
        Ok::<_, String>(Observation {
            renewed: renew,
            initial_command: dir.path().join("initial.txt").exists(),
            middle_command: dir.path().join("middle.txt").exists(),
            final_command: dir.path().join("final.txt").exists(),
            reads,
            durable_valid,
            operations,
            available_after_close,
            legacy_probe_executed,
        })
    })
    .await;
    if service.pty_exists(&session.session_id).await {
        tokio::time::timeout(Duration::from_secs(5), service.pty_close(&handle))
            .await
            .expect("independent cleanup deadline")
            .expect("independent host PTY cleanup");
    }
    assert!(!service.pty_exists(&session.session_id).await);
    result
        .expect("observation deadline")
        .expect("valid real-shell fixture")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn explicit_store_renewal_keeps_shared_terminal_controllable() {
    let observation = observe(true).await;
    eprintln!("renewal control: {observation:?}");
    assert!(observation.renewed && observation.durable_valid);
    assert!(observation.initial_command && observation.middle_command && observation.final_command);
    assert!(observation.reads > 0);
    assert!(observation
        .operations
        .iter()
        .all(|(_, result)| result.is_ok()));
    assert!(!observation.available_after_close && !observation.legacy_probe_executed);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "audit reproducer: lease expiry must not strand a live PTY without shared-protocol recovery"]
async fn audit_active_terminal_lease_expiry_preserves_control_or_cleanup() {
    let observation = observe(false).await;
    eprintln!("expiry observation: {observation:?}");
    assert!(!observation.renewed);
    assert!(observation.initial_command && observation.middle_command && observation.reads > 0);
    assert!(
        !observation.available_after_close,
        "lease expiry stranded a live PTY: shared control/recovery/close did not reclaim it"
    );
}
