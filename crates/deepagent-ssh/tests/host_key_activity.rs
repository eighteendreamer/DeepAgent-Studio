mod support;

use deepagent_persistence::Database;
use deepagent_ssh::{SshConnectionAttempt, SshHostIdentity, SshHostKeyStore, SshService};
use std::sync::{atomic::Ordering, Arc};

#[tokio::test]
async fn successful_authentication_records_usage_without_changing_trust_or_writing_on_pty_reads() {
    let server = support::SshServer::start("127.0.0.1").await;
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("app.sqlite");
    let file = temp.path().join("known_hosts");
    server.write_trust(&file);
    let db = Arc::new(Database::open(&path).unwrap());
    let store = Arc::new(SshHostKeyStore::new(db.clone()));
    let service = SshService::new(temp.path().into()).with_host_key_store(store.clone());
    let c = server.create(&service, Some(&file)).await;
    let identity = SshHostIdentity::direct(&c.host, c.port).unwrap();
    store
        .trust_verified(
            &identity,
            server.key.read().unwrap().public_key(),
            "fixture",
        )
        .unwrap();
    let original_audit = service.host_trust_audit(&c.id).await.unwrap();
    assert!(original_audit.last_used_at_ms.is_none());
    server.counts.reject_password.store(true, Ordering::SeqCst);
    assert!(service.connect(&c.id).await.is_err());
    assert!(service
        .host_trust_audit(&c.id)
        .await
        .unwrap()
        .last_used_at_ms
        .is_none());
    server.counts.reject_password.store(false, Ordering::SeqCst);
    let handle = service.connect(&c.id).await.unwrap();
    let authenticated = service.host_trust_audit(&c.id).await.unwrap();
    assert!(authenticated.last_used_at_ms.is_some());
    assert_eq!(authenticated.records, original_audit.records);
    assert_eq!(authenticated.revision, original_audit.revision);
    assert_eq!(
        store.list(&identity).unwrap()[0].last_used_at_ms,
        authenticated.last_used_at_ms
    );
    db.with_conn(|conn| conn.execute_batch("CREATE TABLE fixture_activity_writes (count INTEGER); INSERT INTO fixture_activity_writes VALUES (0); CREATE TRIGGER fixture_activity_update AFTER UPDATE ON documents WHEN NEW.collection='ssh_host_activity_v1' BEGIN UPDATE fixture_activity_writes SET count=count+1; END;")
        .map_err(|error|deepagent_core::error::CoreError::Persistence(error.to_string()))).unwrap();
    let pty = service.pty_spawn(&handle, 80, 24).await.unwrap();
    for _ in 0..16 {
        service.pty_read(&pty).await.unwrap();
        service.connect(&c.id).await.unwrap();
        service.list_host_keys(&c.id).await.unwrap();
    }
    assert_eq!(
        service.exec(&handle, "fixture").await.unwrap().stdout,
        "ok\n"
    );
    let writes: i64 = db
        .with_conn(|conn| {
            conn.query_row("SELECT count FROM fixture_activity_writes", [], |row| {
                row.get(0)
            })
            .map_err(|error| deepagent_core::error::CoreError::Persistence(error.to_string()))
        })
        .unwrap();
    assert_eq!(
        writes, 0,
        "PTY reads / live client reuse / list never update usage metadata"
    );
    assert_eq!(
        server.auth_count(),
        2,
        "one rejected password and one successful authentication; no reauthentication in the loop"
    );
    let reopened = SshHostKeyStore::new(Arc::new(Database::open(&path).unwrap()));
    assert_eq!(
        reopened.list(&identity).unwrap()[0].last_used_at_ms,
        authenticated.last_used_at_ms
    );
    service.disconnect(&c.id).await.unwrap();
}

#[tokio::test]
async fn file_only_authentication_is_visible_but_probes_and_cancelled_enrollment_are_not_usage() {
    let server = support::SshServer::start("127.0.0.1").await;
    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("known_hosts");
    let store = Arc::new(SshHostKeyStore::new(Arc::new(
        Database::open_in_memory().unwrap(),
    )));
    let service = SshService::new(temp.path().into()).with_host_key_store(store);
    let c = server.create(&service, Some(&file)).await;
    let SshConnectionAttempt::ConfirmationRequired { challenge } =
        service.begin_connection(&c.id).await.unwrap()
    else {
        panic!("unknown host requires a prompt")
    };
    service
        .cancel_host_key(&c.id, &challenge.attempt_id, &challenge.challenge_id)
        .unwrap();
    assert!(service
        .host_trust_audit(&c.id)
        .await
        .unwrap()
        .last_used_at_ms
        .is_none());
    assert_eq!(server.auth_count(), 0);
    server.write_trust(&file);
    let handle = service.connect(&c.id).await.unwrap();
    let used = service.host_trust_audit(&c.id).await.unwrap();
    assert!(used.last_used_at_ms.is_some());
    assert_eq!(
        used.revision, 0,
        "file-only use does not enroll trust or create a trust audit event"
    );
    assert!(used.records.is_empty());
    assert!(service.list_host_keys(&c.id).await.unwrap().is_empty());
    assert_eq!(
        service.exec(&handle, "fixture").await.unwrap().stdout,
        "ok\n"
    );
    service.disconnect(&c.id).await.unwrap();
}

#[tokio::test]
async fn corrupt_activity_is_visible_but_never_blocks_or_authorizes_authentication() {
    let server = support::SshServer::start("127.0.0.1").await;
    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("known_hosts");
    server.write_trust(&file);
    let db = Arc::new(Database::open_in_memory().unwrap());
    let store = Arc::new(SshHostKeyStore::new(db.clone()));
    let service = SshService::new(temp.path().into()).with_host_key_store(store.clone());
    let c = server.create(&service, Some(&file)).await;
    let identity = SshHostIdentity::direct(&c.host, c.port).unwrap();
    store
        .trust_verified(
            &identity,
            server.key.read().unwrap().public_key(),
            "fixture",
        )
        .unwrap();
    service.connect(&c.id).await.unwrap();
    service.disconnect(&c.id).await.unwrap();
    db.with_conn(|conn| {
        conn.execute(
            "UPDATE documents SET body='not-json' WHERE collection='ssh_host_activity_v1'",
            [],
        )
        .map(|_| ())
        .map_err(|error| deepagent_core::error::CoreError::Persistence(error.to_string()))
    })
    .unwrap();
    let handle = service.connect(&c.id).await.unwrap();
    assert_eq!(
        service.exec(&handle, "fixture").await.unwrap().stdout,
        "ok\n"
    );
    let view = service.host_trust_audit(&c.id).await.unwrap();
    assert!(view.last_used_at_ms.is_none());
    assert!(view.activity_error.is_some());
    assert_eq!(view.records.len(), 1);
    assert!(store.list(&identity).unwrap()[0].last_used_at_ms.is_none());
    service.disconnect(&c.id).await.unwrap();
    *server.key.write().unwrap() = support::random_key();
    assert!(service.connect(&c.id).await.is_err());
    assert_eq!(
        server.auth_count(),
        2,
        "bad usage never accepts a changed server key"
    );
}
