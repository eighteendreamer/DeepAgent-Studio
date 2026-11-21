mod support;

use deepagent_ssh::{CreateSshConnectionRequest, SshError, SshHostKeyStore, SshService};
use std::{future::Future, sync::atomic::Ordering, sync::Arc, time::Duration};
use support::SshServer;
use tokio::sync::Notify;

async fn bounded<T>(work: impl Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(8), work)
        .await
        .expect("SSH shutdown lifecycle must settle promptly")
}

async fn service_for(server: &SshServer, data: &tempfile::TempDir) -> (Arc<SshService>, String) {
    let trust = data.path().join("known_hosts");
    server.write_trust(&trust);
    let service = Arc::new(
        SshService::new(data.path().into()).with_host_key_store(Arc::new(SshHostKeyStore::new(
            Arc::new(deepagent_persistence::Database::open_in_memory().unwrap()),
        ))),
    );
    let connection = server.create(&service, Some(&trust)).await;
    (service, connection.id)
}

#[tokio::test]
async fn shutdown_closes_published_transport_and_rejects_new_network_work() {
    bounded(async {
        let server = SshServer::start("127.0.0.1").await;
        let data = tempfile::tempdir().unwrap();
        let (service, id) = service_for(&server, &data).await;
        let handle = service.connect(&id).await.unwrap();
        assert_eq!(
            service
                .exec(&handle, "before shutdown")
                .await
                .unwrap()
                .stdout,
            "ok\n"
        );

        assert_eq!(service.shutdown().await.unwrap(), 1);
        assert_eq!(service.shutdown().await.unwrap(), 0);
        assert!(matches!(
            service.connect(&id).await,
            Err(SshError::ServiceStopped)
        ));
        assert!(matches!(
            service.begin_connection(&id).await,
            Err(SshError::ServiceStopped)
        ));
        assert!(matches!(
            service.exec(&handle, "after shutdown").await,
            Err(SshError::ServiceStopped)
        ));
        let create: CreateSshConnectionRequest = serde_json::from_value(serde_json::json!({
            "name": "after shutdown", "host": "127.0.0.1", "port": 22,
            "username": "fixture", "auth_type": "password"
        }))
        .unwrap();
        assert!(matches!(
            service.create_connection(create).await,
            Err(SshError::ServiceStopped)
        ));
        assert_eq!(service.refresh_due_statuses().await.unwrap(), 0);
        assert_eq!(
            service.status(&id).await.unwrap().status,
            deepagent_ssh::SshStatus::Disconnected
        );
        assert_eq!(server.auth_count(), 1, "shutdown must not reconnect");
        assert_eq!(server.counts.connections.load(Ordering::SeqCst), 1);
        assert_eq!(server.counts.closed_connections.load(Ordering::SeqCst), 1);
    })
    .await;
}

#[tokio::test]
async fn shutdown_closes_transport_and_drains_an_inflight_channel() {
    bounded(async {
        let server = SshServer::start("127.0.0.1").await;
        let data = tempfile::tempdir().unwrap();
        let (service, id) = service_for(&server, &data).await;
        let handle = service.connect(&id).await.unwrap();
        let exec_gate = Arc::new(Notify::new());
        *server.counts.exec_gate.write().unwrap() = Some(Arc::clone(&exec_gate));
        let task = {
            let service = Arc::clone(&service);
            let handle = handle.clone();
            tokio::spawn(async move { service.exec(&handle, "held channel").await })
        };
        while server.counts.commands.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }

        let shutdown = service.shutdown().await;
        exec_gate.notify_one();
        *server.counts.exec_gate.write().unwrap() = None;
        let operation = tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .expect("shutdown must release the in-flight channel")
            .unwrap();

        assert_eq!(shutdown.unwrap(), 1);
        assert!(
            operation.is_err(),
            "closing the transport cancels channel work"
        );
        while server.counts.closed_connections.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
        assert_eq!(server.counts.closed_connections.load(Ordering::SeqCst), 1);
    })
    .await;
}

#[tokio::test]
async fn shutdown_cancels_a_handshake_waiting_for_its_server() {
    bounded(async {
        let server = SshServer::start("127.0.0.1").await;
        let data = tempfile::tempdir().unwrap();
        let (service, id) = service_for(&server, &data).await;
        let accept_gate = Arc::new(Notify::new());
        *server.counts.accept_gate.write().unwrap() = Some(Arc::clone(&accept_gate));

        let connecting = {
            let service = Arc::clone(&service);
            let id = id.clone();
            tokio::spawn(async move { service.connect(&id).await })
        };
        while server.counts.connections.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }

        assert_eq!(service.shutdown().await.unwrap(), 1);
        assert!(tokio::time::timeout(Duration::from_secs(1), connecting)
            .await
            .expect("shutdown waits for cancelled admission owners")
            .unwrap()
            .is_err());
        assert!(matches!(
            service.connect(&id).await,
            Err(SshError::ServiceStopped)
        ));

        *server.counts.accept_gate.write().unwrap() = None;
        accept_gate.notify_waiters();
        while server.counts.closed_connections.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
        assert_eq!(server.counts.connections.load(Ordering::SeqCst), 1);
    })
    .await;
}
