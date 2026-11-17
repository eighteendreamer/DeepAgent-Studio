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
    control_available_before_takeover: bool,
    recovered_command: bool,
    old_fenced_after_takeover: bool,
    operations: Vec<(&'static str, Result<(), String>)>,
    available_after_close: bool,
    legacy_probe_executed: bool,
}

async fn observe(renew: bool, protocol: bool) -> Observation {
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
            target: Default::default(),
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
                if protocol {
                    backend
                        .renew(&session, &lease)
                        .await
                        .map_err(|e| e.to_string())?;
                } else {
                    store.renew(&lease, now()).map_err(|e| e.to_string())?;
                }
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
        let before = backend
            .recovery_status(&session.session_id)
            .await
            .map_err(|e| e.to_string())?;
        if !before.available {
            return Err("idle synthetic shell unexpectedly exited".into());
        }
        if !renew
            && (backend.renew(&session, &lease).await.is_ok()
                || backend
                    .release(&session, &lease, TerminalInputHolder::Runtime)
                    .await
                    .is_ok()
                || backend.close(&session, &lease).await.is_ok())
        {
            return Err(
                "expired input token was accepted without an explicit generation takeover".into(),
            );
        }
        let mut recovered_command = false;
        let mut old_fenced_after_takeover = false;
        let mut current = lease.clone();
        let taken = backend
            .takeover(&session, TerminalInputHolder::Runtime)
            .await;
        let take_result = match taken {
            Ok(next) => {
                if next.epoch <= lease.epoch || next.lease_id == lease.lease_id {
                    return Err("takeover did not rotate the generation".into());
                }
                old_fenced_after_takeover = backend
                    .write(&session, &lease, b"echo stale > stale.txt\r\n")
                    .await
                    .is_err()
                    && backend.renew(&session, &lease).await.is_err()
                    && backend.resize(&session, &lease, 80, 24).await.is_err()
                    && backend
                        .signal(&session, &lease, TerminalSignal::Kill)
                        .await
                        .is_err()
                    && backend
                        .release(&session, &lease, TerminalInputHolder::User)
                        .await
                        .is_err()
                    && backend.close(&session, &lease).await.is_err();
                backend
                    .write(&session, &next, b"echo recovered > recovered.txt\r\n")
                    .await
                    .map_err(|e| e.to_string())?;
                tokio::time::timeout(Duration::from_secs(2), async {
                    while !dir.path().join("recovered.txt").exists() {
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                })
                .await
                .map_err(|_| "recovered generation could not execute".to_owned())?;
                recovered_command = true;
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
            control_available_before_takeover: before.control_available,
            recovered_command,
            old_fenced_after_takeover,
            operations,
            available_after_close,
            legacy_probe_executed,
        })
    })
    .await;
    if service.pty_exists(&session.session_id).await {
        let cleanup = backend
            .takeover(&session, TerminalInputHolder::User)
            .await
            .expect("owner cleanup takeover");
        tokio::time::timeout(Duration::from_secs(5), backend.close(&session, &cleanup))
            .await
            .expect("cleanup deadline")
            .expect("owner PTY cleanup");
    }
    assert!(!service.pty_exists(&session.session_id).await);
    result
        .expect("observation deadline")
        .expect("valid real-shell fixture")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn explicit_store_renewal_keeps_shared_terminal_controllable() {
    let observation = observe(true, false).await;
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
async fn audit_active_terminal_lease_expiry_preserves_control_or_cleanup() {
    let observation = observe(false, false).await;
    eprintln!("expiry observation: {observation:?}");
    assert!(!observation.renewed && !observation.durable_valid);
    assert!(!observation.control_available_before_takeover);
    assert!(observation.recovered_command && observation.old_fenced_after_takeover);
    assert!(observation.operations[..3]
        .iter()
        .all(|(_, result)| result.is_err()));
    assert!(observation.operations[3..]
        .iter()
        .all(|(_, result)| result.is_ok()));
    assert!(observation.initial_command && observation.middle_command && observation.reads > 0);
    assert!(
        !observation.available_after_close,
        "lease expiry stranded a live PTY: shared control/recovery/close did not reclaim it"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn h293_shared_owner_blocks_legacy_control_and_wrong_run_takeover() {
    let dir = tempfile::tempdir().unwrap();
    let db = Arc::new(Database::open(dir.path().join("owner.sqlite")).unwrap());
    let store = Arc::new(SqliteTerminalLeaseStore::new(db.clone(), 60_000).unwrap());
    let service = Arc::new(TerminalService::with_executor(
        Arc::new(ProjectService::new(db)),
        dir.path().to_string_lossy(),
        Arc::new(SystemExecutor),
    ));
    let backend = DirectTerminalSessionBackend::with_lease_persistence(
        service.clone(),
        TerminalShell::CommandPrompt,
        Some(store),
    );
    let (session, _) = backend
        .open(TerminalOpenRequest {
            target: Default::default(),
            run_id: "h293-owner".into(),
            cwd: dir.path().to_string_lossy().into_owned(),
            cols: 80,
            rows: 24,
            initial_holder: TerminalInputHolder::User,
        })
        .await
        .unwrap();
    let mut wrong_scope = session.clone();
    wrong_scope.run_id = "unrelated-run".into();
    let wrong = backend
        .takeover(&wrong_scope, TerminalInputHolder::Runtime)
        .await;
    let current = backend
        .takeover(&session, TerminalInputHolder::User)
        .await
        .unwrap();
    let handle = LocalPtyHandle {
        pty_id: session.session_id.clone(),
        cols: 80,
        rows: 24,
    };
    let legacy_write = service.pty_write(&handle, b"\r\n").await;
    let legacy_resize = service.pty_resize(&handle, 90, 26).await;
    let legacy_close = service.pty_close(&handle).await;
    if service.pty_exists(&session.session_id).await {
        backend.close(&session, &current).await.unwrap();
    }
    assert!(!service.pty_exists(&session.session_id).await);
    assert!(
        wrong.is_err(),
        "a session id is not authority for a different run"
    );
    assert!(
        legacy_write.is_err(),
        "legacy input bypassed the shared owner"
    );
    assert!(
        legacy_resize.is_err(),
        "legacy resize bypassed the shared owner"
    );
    assert!(
        legacy_close.is_err(),
        "legacy close bypassed the shared owner"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn h293_protocol_renewal_keeps_shared_terminal_controllable() {
    let observation = observe(true, true).await;
    eprintln!("protocol renewal: {observation:?}");
    assert!(observation.durable_valid && observation.control_available_before_takeover);
    assert!(observation.initial_command && observation.middle_command && observation.final_command);
    assert!(observation.recovered_command && observation.old_fenced_after_takeover);
    assert!(observation
        .operations
        .iter()
        .all(|(_, result)| result.is_ok()));
    assert!(!observation.available_after_close && !observation.legacy_probe_executed);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn h293_legacy_pty_keeps_normal_write_resize_and_close() {
    let dir = tempfile::tempdir().unwrap();
    let db = Arc::new(Database::open_in_memory().unwrap());
    let service = TerminalService::with_executor(
        Arc::new(ProjectService::new(db)),
        dir.path().to_string_lossy(),
        Arc::new(SystemExecutor),
    );
    let handle = service
        .pty_spawn(TerminalShell::CommandPrompt, 80, 24)
        .await
        .unwrap();
    let write = service
        .pty_write(&handle, b"echo legacy > legacy.txt\r\n")
        .await;
    let resize = service.pty_resize(&handle, 100, 30).await;
    let executed = tokio::time::timeout(Duration::from_secs(3), async {
        while !dir.path().join("legacy.txt").exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    service.pty_close(&handle).await.unwrap();
    assert!(!service.pty_exists(&handle.pty_id).await);
    write.unwrap();
    resize.unwrap();
    executed.unwrap();
}
