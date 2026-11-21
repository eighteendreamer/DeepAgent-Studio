mod support;

use deepagent_ssh::{
    DirectoryWalkLimits, FileConflictPolicy, FileTransferDirection, FileTransferRequest,
    SshService, SshServiceHandle, SshStatus,
};
use std::{future::Future, sync::atomic::Ordering, sync::Arc, time::Duration};
use support::SshServer;
use tokio::{sync::Notify, task::JoinHandle};
use tokio_util::sync::CancellationToken;

async fn bounded<T>(work: impl Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(5), work)
        .await
        .expect("loopback resource lifecycle must settle promptly")
}

async fn advance(milliseconds: u64) {
    tokio::time::pause();
    tokio::time::advance(Duration::from_millis(milliseconds)).await;
    tokio::time::resume();
    tokio::task::yield_now().await;
}

struct Fixture {
    server: SshServer,
    service: Arc<SshService>,
    handle: SshServiceHandle,
    data: tempfile::TempDir,
}

impl Fixture {
    async fn new() -> Self {
        let server = SshServer::start("127.0.0.1").await;
        let data = tempfile::tempdir().unwrap();
        let trust = data.path().join("known_hosts");
        server.write_trust(&trust);
        let service = Arc::new(
            SshService::new(data.path().into())
                .with_host_key_store(Arc::new(deepagent_ssh::SshHostKeyStore::new(Arc::new(
                    deepagent_persistence::Database::open_in_memory().unwrap(),
                ))))
                .with_connection_idle_timeout(Duration::from_secs(1))
                .unwrap(),
        );
        let connection = server.create(&service, Some(&trust)).await;
        let handle = bounded(service.connect(&connection.id)).await.unwrap();
        Self {
            server,
            service,
            handle,
            data,
        }
    }

    async fn reap(&self) -> usize {
        bounded(self.service.reap_idle_connections()).await.unwrap()
    }

    async fn assert_reaped(&self) {
        assert_eq!(
            self.reap().await,
            1,
            "the last released resource must become idle"
        );
        self.assert_retired().await;
        assert_eq!(self.reap().await, 0, "reaping is idempotent");
    }

    async fn assert_retired(&self) {
        assert_eq!(
            self.service
                .status(&self.handle.connection_id)
                .await
                .unwrap()
                .status,
            SshStatus::Disconnected
        );
        assert_eq!(
            self.service.list_connections().await.len(),
            1,
            "retain saved configuration"
        );
        bounded(async {
            while self.server.counts.closed_connections.load(Ordering::SeqCst) == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await;
    }

    async fn blocked_exec(
        &self,
    ) -> (
        Arc<Notify>,
        JoinHandle<deepagent_ssh::SshResult<deepagent_ssh::SshExecResult>>,
    ) {
        let before = self.server.counts.commands.load(Ordering::SeqCst);
        let gate = Arc::new(Notify::new());
        *self.server.counts.exec_gate.write().unwrap() = Some(gate.clone());
        let service = self.service.clone();
        let handle = self.handle.clone();
        let task = tokio::spawn(async move { service.exec(&handle, "held command").await });
        bounded(async {
            while self.server.counts.commands.load(Ordering::SeqCst) == before {
                tokio::task::yield_now().await;
            }
        })
        .await;
        (gate, task)
    }
}

#[tokio::test]
async fn unused_transport_is_reaped_and_a_later_request_gets_a_new_transport() {
    let fixture = Fixture::new().await;
    advance(900).await;
    assert_eq!(fixture.reap().await, 0);
    advance(200).await;
    fixture.assert_reaped().await;
    let handle = bounded(fixture.service.connect(&fixture.handle.connection_id))
        .await
        .unwrap();
    assert_eq!(fixture.server.auth_count(), 2);
    assert_eq!(
        fixture
            .service
            .exec(&handle, "new explicit work")
            .await
            .unwrap()
            .stdout,
        "ok\n"
    );
    fixture
        .service
        .disconnect(&handle.connection_id)
        .await
        .unwrap();
}

#[tokio::test]
async fn a_live_pty_is_protected_and_idle_starts_only_after_its_close() {
    let fixture = Fixture::new().await;
    let pty = fixture
        .service
        .pty_spawn(&fixture.handle, 80, 24)
        .await
        .unwrap();
    advance(1_100).await;
    assert_eq!(fixture.reap().await, 0, "a live PTY owns its transport");
    fixture
        .service
        .pty_write(&pty, b"still live")
        .await
        .unwrap();
    assert_eq!(fixture.server.auth_count(), 1);
    fixture.service.pty_close(&pty).await.unwrap();
    advance(900).await;
    assert_eq!(
        fixture.reap().await,
        0,
        "idle starts at last release, not connection creation"
    );
    advance(200).await;
    fixture.assert_reaped().await;
}

#[tokio::test]
async fn closing_one_of_two_ptys_does_not_release_the_other_resource() {
    let fixture = Fixture::new().await;
    let first = fixture
        .service
        .pty_spawn(&fixture.handle, 80, 24)
        .await
        .unwrap();
    let second = fixture
        .service
        .pty_spawn(&fixture.handle, 100, 30)
        .await
        .unwrap();
    fixture.service.pty_close(&first).await.unwrap();
    advance(1_100).await;
    assert_eq!(fixture.reap().await, 0);
    fixture
        .service
        .pty_write(&second, b"second remains open")
        .await
        .unwrap();
    fixture.service.pty_close(&second).await.unwrap();
    advance(1_100).await;
    fixture.assert_reaped().await;
}

#[tokio::test]
async fn an_owned_ended_pty_keeps_its_unread_output_until_explicit_close() {
    let fixture = Fixture::new().await;
    let pty = fixture
        .service
        .pty_spawn(&fixture.handle, 80, 24)
        .await
        .unwrap();
    fixture
        .service
        .pty_write(&pty, b"fixture-exit")
        .await
        .unwrap();
    bounded(async {
        while !fixture
            .service
            .pty_read_with_cursor(&pty, 0)
            .await
            .unwrap()
            .eof
        {
            tokio::task::yield_now().await;
        }
    })
    .await;
    advance(1_100).await;
    assert_eq!(
        fixture.reap().await,
        0,
        "terminal ownership also protects unread history"
    );
    let output = fixture.service.pty_read_with_cursor(&pty, 0).await.unwrap();
    assert!(String::from_utf8_lossy(&output.data).contains("尾部"));
    fixture.service.pty_close(&pty).await.unwrap();
    advance(1_100).await;
    fixture.assert_reaped().await;
}

#[tokio::test]
async fn an_inflight_exec_is_not_idle_and_its_completion_starts_a_fresh_deadline() {
    let fixture = Fixture::new().await;
    let (gate, task) = fixture.blocked_exec().await;
    advance(1_100).await;
    let reaped = fixture.reap().await;
    *fixture.server.counts.exec_gate.write().unwrap() = None;
    gate.notify_waiters();
    let output = bounded(task).await.unwrap().unwrap();
    assert_eq!(reaped, 0, "exec must own a resource during server waiting");
    assert_eq!(output.stdout, "ok\n");
    advance(900).await;
    assert_eq!(fixture.reap().await, 0);
    advance(200).await;
    fixture.assert_reaped().await;
}

#[tokio::test]
async fn a_file_transfer_holds_its_resource_while_waiting_for_staging_acknowledgement() {
    let fixture = Fixture::new().await;
    let local = fixture.data.path().join("payload.txt");
    std::fs::write(&local, b"lease payload").unwrap();
    let gate = Arc::new(Notify::new());
    *fixture.server.counts.stage_open_gate.write().unwrap() = Some(gate.clone());
    let service = fixture.service.clone();
    let id = fixture.handle.connection_id.clone();
    let transfer = tokio::spawn(async move {
        service
            .transfer_file(
                &id,
                FileTransferRequest {
                    direction: FileTransferDirection::Upload,
                    local_path: local.to_str().unwrap().into(),
                    remote_path: "/payload.txt".into(),
                    conflict: FileConflictPolicy::Replace,
                },
                CancellationToken::new(),
                Arc::new(|_| {}),
            )
            .await
    });
    bounded(fixture.server.counts.stage_created.notified()).await;
    advance(1_100).await;
    let reaped = fixture.reap().await;
    *fixture.server.counts.stage_open_gate.write().unwrap() = None;
    gate.notify_waiters();
    if reaped != 0 {
        transfer.abort();
        let _ = transfer.await;
        panic!("file transfer lost its returned session owner: {reaped} transport reaped");
    }
    let result = bounded(transfer).await.unwrap();
    assert_eq!(
        reaped, 0,
        "file transfer must retain the returned session owner"
    );
    let receipt = result.unwrap();
    assert_eq!(receipt.bytes, "13");
    advance(1_100).await;
    fixture.assert_reaped().await;
}

#[tokio::test]
async fn cancelled_directory_work_releases_its_resource_without_waiting_for_the_idle_deadline() {
    let fixture = Fixture::new().await;
    let gate = Arc::new(Notify::new());
    *fixture.server.counts.directory_read_gate.write().unwrap() = Some(gate.clone());
    let cancel = CancellationToken::new();
    let task_cancel = cancel.clone();
    let service = fixture.service.clone();
    let id = fixture.handle.connection_id.clone();
    let walk = tokio::spawn(async move {
        service
            .walk_directory(&id, "/", DirectoryWalkLimits::default(), task_cancel)
            .await
    });
    bounded(fixture.server.counts.directory_read_started.notified()).await;
    advance(1_100).await;
    let reaped = fixture.reap().await;
    cancel.cancel();
    *fixture.server.counts.directory_read_gate.write().unwrap() = None;
    gate.notify_waiters();
    assert_eq!(bounded(walk).await.unwrap().unwrap_err().code, "cancelled");
    assert_eq!(reaped, 0);
    advance(1_100).await;
    fixture.assert_reaped().await;
}

#[tokio::test]
async fn dropping_an_old_operation_cannot_release_a_replacement_generations_pty() {
    let fixture = Fixture::new().await;
    let (gate, old) = fixture.blocked_exec().await;
    bounded(fixture.service.disconnect(&fixture.handle.connection_id))
        .await
        .unwrap();
    let next = bounded(fixture.service.connect(&fixture.handle.connection_id))
        .await
        .unwrap();
    let pty = fixture.service.pty_spawn(&next, 80, 24).await.unwrap();
    old.abort();
    let _ = old.await;
    *fixture.server.counts.exec_gate.write().unwrap() = None;
    gate.notify_waiters();
    advance(1_100).await;
    assert_eq!(
        fixture.reap().await,
        0,
        "an old generation cannot decrement the new generation"
    );
    fixture
        .service
        .pty_write(&pty, b"replacement live")
        .await
        .unwrap();
    assert_eq!(fixture.server.auth_count(), 2);
    fixture.service.pty_close(&pty).await.unwrap();
    advance(1_100).await;
    fixture.assert_reaped().await;
}

#[tokio::test]
async fn an_explicit_connect_renews_an_unused_transports_idle_candidate() {
    let fixture = Fixture::new().await;
    advance(900).await;
    fixture
        .service
        .connect(&fixture.handle.connection_id)
        .await
        .unwrap();
    advance(200).await;
    assert_eq!(fixture.reap().await, 0);
    advance(900).await;
    fixture.assert_reaped().await;
    assert_eq!(fixture.server.auth_count(), 1);
}

#[tokio::test]
async fn rejected_pty_and_exec_requests_release_their_connection_resources() {
    let fixture = Fixture::new().await;
    fixture
        .server
        .counts
        .reject_shell
        .store(true, Ordering::SeqCst);
    assert!(fixture
        .service
        .pty_spawn(&fixture.handle, 80, 24)
        .await
        .is_err());
    assert!(fixture
        .service
        .exec(&fixture.handle, "fixture-rejected")
        .await
        .is_err());
    advance(1_100).await;
    fixture.assert_reaped().await;
}

#[tokio::test]
async fn abandoning_an_exec_future_releases_its_resource() {
    let fixture = Fixture::new().await;
    let (gate, task) = fixture.blocked_exec().await;
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    *fixture.server.counts.exec_gate.write().unwrap() = None;
    gate.notify_waiters();
    advance(1_100).await;
    fixture.assert_reaped().await;
}

#[tokio::test]
async fn the_existing_status_maintenance_entry_point_reaps_unused_transports() {
    let fixture = Fixture::new().await;
    advance(1_100).await;
    bounded(fixture.service.refresh_due_statuses())
        .await
        .unwrap();
    fixture.assert_retired().await;
    assert_eq!(fixture.reap().await, 0);
}

#[tokio::test]
async fn idle_retirement_keeps_a_pending_rotation_preview_valid() {
    let fixture = Fixture::new().await;
    *fixture.server.key.write().unwrap() = support::random_key();
    let preview = fixture
        .service
        .begin_host_key_rotation(&fixture.handle.connection_id)
        .await
        .unwrap();
    advance(1_100).await;
    fixture.assert_reaped().await;
    let challenge = preview.challenge;
    let receipt = bounded(fixture.service.confirm_host_key_rotation(
        &challenge.connection_id,
        &challenge.attempt_id,
        &challenge.challenge_id,
    ))
    .await
    .unwrap();
    assert!(receipt.disconnect_failed_ids.is_empty());
    assert_eq!(
        fixture.server.auth_count(),
        1,
        "rotation observations never authenticate"
    );
}

#[tokio::test]
async fn a_live_rotation_probe_is_skipped_without_waiting_for_its_network_owner() {
    let fixture = Fixture::new().await;
    *fixture.server.key.write().unwrap() = support::random_key();
    let preview = fixture
        .service
        .begin_host_key_rotation(&fixture.handle.connection_id)
        .await
        .unwrap();
    let before = fixture.server.counts.connections.load(Ordering::SeqCst);
    let gate = Arc::new(Notify::new());
    *fixture.server.counts.accept_gate.write().unwrap() = Some(gate.clone());
    let service = fixture.service.clone();
    let task = tokio::spawn(async move {
        let challenge = preview.challenge;
        service
            .confirm_host_key_rotation(
                &challenge.connection_id,
                &challenge.attempt_id,
                &challenge.challenge_id,
            )
            .await
    });
    bounded(async {
        while fixture.server.counts.connections.load(Ordering::SeqCst) == before {
            tokio::task::yield_now().await;
        }
    })
    .await;
    advance(1_100).await;
    let reaped = fixture.reap().await;
    *fixture.server.counts.accept_gate.write().unwrap() = None;
    gate.notify_waiters();
    assert!(bounded(task).await.unwrap().is_ok());
    assert_eq!(
        reaped, 0,
        "idle maintenance cannot cancel a management probe"
    );
}

#[test]
fn idle_policy_rejects_zero_or_unbounded_retention_without_persisting_it() {
    assert!(SshService::new(std::path::PathBuf::new())
        .with_connection_idle_timeout(Duration::ZERO)
        .is_err());
    assert!(SshService::new(std::path::PathBuf::new())
        .with_connection_idle_timeout(Duration::from_secs(86_401))
        .is_err());
}
