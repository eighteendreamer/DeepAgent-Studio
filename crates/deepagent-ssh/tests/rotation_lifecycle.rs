mod support;

use deepagent_persistence::Database;
use deepagent_ssh::{
    SshConnectionDto, SshHostIdentity, SshHostKeyRotationPreview, SshHostKeyRotationResult,
    SshHostKeyStore, SshResult, SshService, SshStatus, UpdateSshConnectionRequest,
};
use std::{future::Future, path::PathBuf, sync::atomic::Ordering, sync::Arc, time::Duration};
use support::SshServer;
use tokio::{sync::Notify, task::JoinHandle};

async fn bounded<T>(work: impl Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(12), work)
        .await
        .expect("loopback rotation must settle without a production timeout")
}

struct Rotation {
    server: SshServer,
    service: Arc<SshService>,
    store: Arc<SshHostKeyStore>,
    database: Arc<Database>,
    connection: SshConnectionDto,
    preview: SshHostKeyRotationPreview,
    source: PathBuf,
    original: Vec<u8>,
    data: tempfile::TempDir,
}

impl Rotation {
    async fn new() -> Self {
        let server = SshServer::start("127.0.0.1").await;
        let data = tempfile::tempdir().unwrap();
        let source = data.path().join("known_hosts");
        server.write_trust(&source);
        let original = std::fs::read(&source).unwrap();
        let database = Arc::new(Database::open_in_memory().unwrap());
        let store = Arc::new(SshHostKeyStore::new(Arc::clone(&database)));
        let service =
            Arc::new(SshService::new(data.path().into()).with_host_key_store(Arc::clone(&store)));
        let connection = server.create(&service, Some(&source)).await;
        *server.key.write().unwrap() = support::random_key();
        let preview = service
            .begin_host_key_rotation(&connection.id)
            .await
            .unwrap();
        Self {
            server,
            service,
            store,
            database,
            connection,
            preview,
            source,
            original,
            data,
        }
    }

    async fn block_confirmation(
        &self,
    ) -> (Arc<Notify>, JoinHandle<SshResult<SshHostKeyRotationResult>>) {
        let before = self.server.counts.connections.load(Ordering::SeqCst);
        let gate = Arc::new(Notify::new());
        *self.server.counts.accept_gate.write().unwrap() = Some(Arc::clone(&gate));
        let service = Arc::clone(&self.service);
        let challenge = self.preview.challenge.clone();
        let task = tokio::spawn(async move {
            service
                .confirm_host_key_rotation(
                    &challenge.connection_id,
                    &challenge.attempt_id,
                    &challenge.challenge_id,
                )
                .await
        });
        bounded(async {
            while self.server.counts.connections.load(Ordering::SeqCst) == before {
                tokio::task::yield_now().await;
            }
        })
        .await;
        (gate, task)
    }

    fn release(&self, gate: &Notify) {
        *self.server.counts.accept_gate.write().unwrap() = None;
        gate.notify_waiters();
    }

    async fn hold_local_trust_read(
        &self,
    ) -> (
        std::sync::mpsc::Sender<()>,
        JoinHandle<deepagent_core::error::Result<()>>,
    ) {
        let (ready, started) = tokio::sync::oneshot::channel();
        let (release, released) = std::sync::mpsc::channel();
        let database = Arc::clone(&self.database);
        let holder = tokio::task::spawn_blocking(move || {
            database.with_conn(|_| {
                let _ = ready.send(());
                let _ = released.recv();
                Ok(())
            })
        });
        started.await.unwrap();
        (release, holder)
    }

    async fn all_probe_sockets_closed(&self) {
        bounded(async {
            while self.server.counts.closed_connections.load(Ordering::SeqCst)
                != self.server.counts.connections.load(Ordering::SeqCst)
            {
                tokio::task::yield_now().await;
            }
        })
        .await;
    }

    fn assert_uncommitted(&self) {
        let identity =
            SshHostIdentity::direct(&self.connection.host, self.connection.port).unwrap();
        assert!(
            self.store.list(&identity).unwrap().is_empty(),
            "cancelled rotation cannot change trust"
        );
        assert!(self.store.audit_history(&identity).unwrap().is_empty());
        assert_eq!(std::fs::read(&self.source).unwrap(), self.original);
        assert_eq!(
            self.server.auth_count(),
            0,
            "rotation never authenticates to the target"
        );
    }

    fn edit_request(&self) -> UpdateSshConnectionRequest {
        serde_json::from_value(serde_json::json!({
            "id": self.connection.id, "name": "edited during rotation", "host": self.connection.host,
            "port": self.connection.port, "username": "new-user", "auth_type": "password",
            "password": "new-fixture-secret", "known_hosts_path": self.source,
        })).unwrap()
    }
}

#[tokio::test]
async fn rotation_confirmation_network_io_does_not_block_an_unrelated_connection() {
    bounded(async {
        let fixture = Rotation::new().await;
        let other = SshServer::start("127.0.0.1").await;
        let trust = fixture.data.path().join("other_known_hosts");
        other.write_trust(&trust);
        let other_connection = other.create(&fixture.service, Some(&trust)).await;
        let (gate, task) = fixture.block_confirmation().await;
        let connected = tokio::time::timeout(
            Duration::from_secs(2),
            fixture.service.connect(&other_connection.id),
        )
        .await;
        fixture.release(&gate);
        let receipt = task.await.unwrap().unwrap();
        let handle = connected
            .expect("a rotation probe cannot hold a global network lock")
            .unwrap();
        assert_eq!(
            fixture
                .service
                .exec(&handle, "independent")
                .await
                .unwrap()
                .stdout,
            "ok\n"
        );
        fixture
            .service
            .disconnect(&other_connection.id)
            .await
            .unwrap();
        assert!(receipt.disconnect_failed_ids.is_empty());
        assert_eq!(fixture.server.auth_count(), 0);
        assert_eq!(std::fs::read(&fixture.source).unwrap(), fixture.original);
    })
    .await;
}

#[tokio::test]
async fn disconnect_cancels_a_rotation_confirmation_before_the_server_replies() {
    bounded(async {
        let fixture = Rotation::new().await;
        let (gate, mut task) = fixture.block_confirmation().await;
        let disconnected = tokio::time::timeout(
            Duration::from_secs(2),
            fixture.service.disconnect(&fixture.connection.id),
        )
        .await;
        let stopped = if disconnected.is_ok() {
            Some(tokio::time::timeout(Duration::from_secs(2), &mut task).await)
        } else {
            None
        };
        let stopped_before_reply = matches!(&stopped, Some(Ok(_)));
        fixture.release(&gate);
        let result = match stopped {
            Some(Ok(result)) => result.unwrap(),
            _ => task.await.unwrap(),
        };
        disconnected
            .expect("disconnect must not wait for the rotation network reply")
            .unwrap();
        assert!(
            stopped_before_reply,
            "the cancelled confirmation must stop before the server reply"
        );
        assert!(result.is_err(), "retired rotation cannot commit");
        assert_eq!(
            fixture
                .service
                .status(&fixture.connection.id)
                .await
                .unwrap()
                .status,
            SshStatus::Disconnected
        );
        fixture.assert_uncommitted();
    })
    .await;
}

#[tokio::test]
async fn editing_identity_cancels_the_rotation_probe_and_prevents_late_trust_commit() {
    bounded(async {
        let fixture = Rotation::new().await;
        let (gate, task) = fixture.block_confirmation().await;
        let edited = tokio::time::timeout(
            Duration::from_secs(2),
            fixture.service.update_connection(fixture.edit_request()),
        )
        .await;
        fixture.release(&gate);
        let result = task.await.unwrap();
        assert_eq!(
            edited
                .expect("identity editing cannot await the old probe")
                .unwrap()
                .username,
            "new-user"
        );
        assert!(result.is_err());
        assert_eq!(
            fixture.service.list_connections().await[0].username,
            "new-user"
        );
        fixture.assert_uncommitted();
    })
    .await;
}

#[tokio::test]
async fn reload_retires_the_rotation_probe_without_rewriting_the_new_disk_configuration() {
    bounded(async {
        let fixture = Rotation::new().await;
        let (gate, task) = fixture.block_confirmation().await;
        let editor = SshService::new(fixture.data.path().into());
        editor
            .update_connection(fixture.edit_request())
            .await
            .unwrap();
        let loaded = tokio::time::timeout(Duration::from_secs(2), fixture.service.load()).await;
        fixture.release(&gate);
        let result = task.await.unwrap();
        loaded
            .expect("reload must not wait for the retired rotation probe")
            .unwrap();
        assert!(result.is_err());
        assert_eq!(
            fixture.service.list_connections().await[0].username,
            "new-user"
        );
        let reopened = SshService::new(fixture.data.path().into());
        assert_eq!(reopened.list_connections().await[0].username, "new-user");
        fixture.assert_uncommitted();
    })
    .await;
}

#[tokio::test]
async fn trust_changes_during_the_confirmation_probe_refuse_the_entire_commit() {
    bounded(async {
        let fixture = Rotation::new().await;
        let (gate, task) = fixture.block_confirmation().await;
        let updated = b"# separately revoked while the server is stalled\n";
        std::fs::write(&fixture.source, updated).unwrap();
        fixture.release(&gate);
        assert!(task.await.unwrap().is_err());
        let identity =
            SshHostIdentity::direct(&fixture.connection.host, fixture.connection.port).unwrap();
        assert!(fixture.store.list(&identity).unwrap().is_empty());
        assert!(fixture.store.audit_history(&identity).unwrap().is_empty());
        assert_eq!(std::fs::read(&fixture.source).unwrap(), updated);
        assert_eq!(fixture.server.auth_count(), 0);
    })
    .await;
}

#[tokio::test]
async fn abandoning_a_confirmation_releases_admission_and_closes_the_real_probe_socket() {
    bounded(async {
        let fixture = Rotation::new().await;
        let (gate, task) = fixture.block_confirmation().await;
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        fixture.release(&gate);
        fixture.all_probe_sockets_closed().await;
        fixture.assert_uncommitted();
        let next = fixture
            .service
            .begin_host_key_rotation(&fixture.connection.id)
            .await
            .unwrap();
        assert_ne!(
            next.challenge.attempt_id,
            fixture.preview.challenge.attempt_id
        );
        assert!(fixture
            .service
            .confirm_host_key_rotation(
                &fixture.connection.id,
                &fixture.preview.challenge.attempt_id,
                &fixture.preview.challenge.challenge_id,
            )
            .await
            .is_err());
    })
    .await;
}

#[tokio::test]
async fn cancellation_during_local_trust_validation_cannot_reach_the_rotation_cas() {
    use std::task::Poll;

    bounded(async {
        let fixture = Rotation::new().await;
        let (gate, confirmation) = fixture.block_confirmation().await;
        let (release_database, holder) = fixture.hold_local_trust_read().await;
        fixture.release(&gate);
        fixture.all_probe_sockets_closed().await;
        let mut disconnect = Box::pin(fixture.service.disconnect(&fixture.connection.id));
        let completed = std::future::poll_fn(|cx| {
            Poll::Ready(match disconnect.as_mut().poll(cx) {
                Poll::Ready(result) => Some(result),
                Poll::Pending => None,
            })
        })
        .await;
        drop(release_database);
        holder.await.unwrap().unwrap();
        if let Some(completed) = completed {
            completed.unwrap();
        } else {
            disconnect.await.unwrap();
        }
        assert!(confirmation.await.unwrap().is_err());
        fixture.assert_uncommitted();
    })
    .await;
}

#[tokio::test]
async fn challenge_expiry_during_local_trust_validation_cannot_commit_after_a_successful_probe() {
    let fixture = bounded(Rotation::new()).await;
    let (gate, confirmation) = fixture.block_confirmation().await;
    let (release_database, holder) = fixture.hold_local_trust_read().await;
    fixture.release(&gate);
    fixture.all_probe_sockets_closed().await;
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(121)).await;
    tokio::time::resume();
    drop(release_database);
    holder.await.unwrap().unwrap();
    assert!(bounded(confirmation).await.unwrap().is_err());
    fixture.assert_uncommitted();
}
