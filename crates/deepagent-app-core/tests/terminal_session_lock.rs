#![cfg(windows)]
use deepagent_app_core::settings::TerminalShell;
use deepagent_app_core::{
    DirectTerminalSessionBackend, LocalPtyHandle, ProjectService, TerminalService,
};
use deepagent_builtins::SystemExecutor;
use deepagent_persistence::Database;
use deepagent_terminal::{
    TerminalInputHolder, TerminalOpenRequest, TerminalSessionBackend, TerminalSignal,
};
use std::{sync::Arc, time::Duration};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_lock_fences_text_binary_signals_and_legacy_control_and_retains_cleanup() {
    let dir = tempfile::tempdir().unwrap();
    let db = Arc::new(Database::open_in_memory().unwrap());
    let service = Arc::new(TerminalService::with_executor(
        Arc::new(ProjectService::new(db.clone())),
        dir.path().to_string_lossy(),
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
            target: Default::default(),
            run_id: "native-terminal-lock".into(),
            cwd: dir.path().to_string_lossy().into_owned(),
            cols: 80,
            rows: 24,
            initial_holder: TerminalInputHolder::User,
        })
        .await
        .unwrap();
    let legacy = LocalPtyHandle {
        pty_id: session.session_id.clone(),
        cols: 80,
        rows: 24,
    };
    backend
        .lock(&session, &lease, "fixture-only", true)
        .await
        .unwrap();
    let write = backend
        .write(&session, &lease, b"echo bypass > blocked.txt\r\n")
        .await;
    let binary = backend.write(&session, &lease, &[0, 255, 24]).await;
    let signal = backend
        .signal(&session, &lease, TerminalSignal::Interrupt)
        .await;
    let resize = backend.resize(&session, &lease, 100, 30).await;
    let takeover = backend
        .takeover(&session, TerminalInputHolder::Runtime)
        .await;
    let release = backend
        .release(&session, &lease, TerminalInputHolder::Runtime)
        .await;
    let legacy_write = service
        .pty_write(&legacy, b"echo bypass > blocked.txt\r\n")
        .await;
    let renew = backend.renew(&session, &lease).await;
    let read = backend.read(&session, 0).await;
    let wrong = backend.unlock(&session, &lease, "wrong").await;
    backend
        .unlock(&session, &lease, "fixture-only")
        .await
        .unwrap();
    backend
        .write(&session, &lease, b"echo unlocked > ready.txt\r\n")
        .await
        .unwrap();
    let ready = tokio::time::timeout(Duration::from_secs(5), async {
        while !dir.path().join("ready.txt").exists() {
            let _ = backend.read(&session, 0).await;
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    backend
        .lock(&session, &lease, "fixture-only", false)
        .await
        .unwrap();
    backend.close(&session, &lease).await.unwrap();
    assert!([write, binary, signal, resize].iter().all(Result::is_err));
    assert!(takeover.is_err() && release.is_err() && legacy_write.is_err() && wrong.is_err());
    assert!(renew.is_ok() && read.is_ok() && ready.is_ok());
    assert!(!dir.path().join("blocked.txt").exists());
    assert!(!service.pty_exists(&session.session_id).await);
}
