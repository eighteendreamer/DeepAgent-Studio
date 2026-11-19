mod support;

use deepagent_ssh::{
    SshConnectionAttempt, SshHostKeyDecision, SshService, SshStatus, UpdateSshConnectionRequest,
};
use std::{future::Future, sync::Arc, time::Duration};
use support::SshServer;
use tokio::{sync::Notify, task::JoinHandle};

async fn bounded<T>(work: impl Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(12), work)
        .await
        .expect("loopback lifecycle must settle without a production timeout")
}

async fn auth_started(server: &SshServer) {
    bounded(async {
        while server.auth_count() == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await;
}

fn release(server: &SshServer, gate: &Notify) {
    *server.counts.auth_gate.write().unwrap() = None;
    gate.notify_waiters();
}

fn start(
    service: &Arc<SshService>,
    id: &str,
) -> JoinHandle<deepagent_ssh::SshResult<deepagent_ssh::SshServiceHandle>> {
    let service = Arc::clone(service);
    let id = id.to_owned();
    tokio::spawn(async move { service.connect(&id).await })
}

#[tokio::test]
async fn concurrent_checkouts_share_one_authenticated_transport() {
    bounded(async {
        let server = SshServer::start("127.0.0.1").await;
        let data = tempfile::tempdir().unwrap();
        let trust = data.path().join("known_hosts");
        server.write_trust(&trust);
        let service = Arc::new(SshService::new(data.path().to_path_buf()));
        let config = server.create(&service, Some(&trust)).await;
        let gate = Arc::new(Notify::new());
        *server.counts.auth_gate.write().unwrap() = Some(Arc::clone(&gate));
        let mut requests = Vec::new();
        for _ in 0..8 {
            requests.push(start(&service, &config.id));
        }
        auth_started(&server).await;
        tokio::time::sleep(Duration::from_millis(100)).await;
        release(&server, &gate);
        for request in requests {
            request.await.unwrap().unwrap();
        }
        assert_eq!(
            server.auth_count(),
            1,
            "one saved connection must have only one handshake owner"
        );
        service.disconnect(&config.id).await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn disconnect_retires_the_inflight_generation_before_late_success() {
    bounded(async {
        let server = SshServer::start("127.0.0.1").await;
        let data = tempfile::tempdir().unwrap();
        let trust = data.path().join("known_hosts");
        server.write_trust(&trust);
        let service = Arc::new(SshService::new(data.path().to_path_buf()));
        let config = server.create(&service, Some(&trust)).await;
        let gate = Arc::new(Notify::new());
        *server.counts.auth_gate.write().unwrap() = Some(Arc::clone(&gate));
        let old = start(&service, &config.id);
        auth_started(&server).await;
        service.disconnect(&config.id).await.unwrap();
        release(&server, &gate);
        let late = old.await.unwrap();
        assert!(
            late.is_err(),
            "disconnect must fence a handshake that has not published yet"
        );
        assert_eq!(
            service.status(&config.id).await.unwrap().status,
            SshStatus::Disconnected
        );
    })
    .await;
}

#[tokio::test]
async fn a_retired_handshake_cannot_replace_a_new_live_generation() {
    bounded(async {
        let server = SshServer::start("127.0.0.1").await;
        let data = tempfile::tempdir().unwrap();
        let trust = data.path().join("known_hosts");
        server.write_trust(&trust);
        let service = Arc::new(SshService::new(data.path().to_path_buf()));
        let config = server.create(&service, Some(&trust)).await;
        let gate = Arc::new(Notify::new());
        *server.counts.auth_gate.write().unwrap() = Some(Arc::clone(&gate));
        let old = start(&service, &config.id);
        auth_started(&server).await;
        service.disconnect(&config.id).await.unwrap();
        *server.counts.auth_gate.write().unwrap() = None;
        let fresh = service.connect(&config.id).await.unwrap();
        gate.notify_waiters();
        assert!(
            old.await.unwrap().is_err(),
            "retired success must not overwrite a newer connection"
        );
        assert_eq!(
            service.exec(&fresh, "new-generation").await.unwrap().stdout,
            "ok\n"
        );
        assert_eq!(server.auth_count(), 2);
        service.disconnect(&config.id).await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn cancelling_a_waiter_does_not_cancel_the_handshake_owner() {
    bounded(async {
        let server = SshServer::start("127.0.0.1").await;
        let data = tempfile::tempdir().unwrap();
        let trust = data.path().join("known_hosts");
        server.write_trust(&trust);
        let service = Arc::new(SshService::new(data.path().to_path_buf()));
        let config = server.create(&service, Some(&trust)).await;
        let gate = Arc::new(Notify::new());
        *server.counts.auth_gate.write().unwrap() = Some(Arc::clone(&gate));
        let owner = start(&service, &config.id);
        auth_started(&server).await;
        let waiter = start(&service, &config.id);
        tokio::time::sleep(Duration::from_millis(100)).await;
        waiter.abort();
        assert!(waiter.await.unwrap_err().is_cancelled());
        release(&server, &gate);
        let handle = owner.await.unwrap().unwrap();
        assert_eq!(
            service
                .exec(&handle, "owner-still-live")
                .await
                .unwrap()
                .stdout,
            "ok\n"
        );
        assert_eq!(
            server.auth_count(),
            1,
            "an aborted waiter must never authenticate separately"
        );
        service.disconnect(&config.id).await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn cancelling_the_owner_does_not_strand_the_next_checkout() {
    bounded(async {
        let server = SshServer::start("127.0.0.1").await;
        let data = tempfile::tempdir().unwrap();
        let trust = data.path().join("known_hosts");
        server.write_trust(&trust);
        let service = Arc::new(SshService::new(data.path().to_path_buf()));
        let config = server.create(&service, Some(&trust)).await;
        let gate = Arc::new(Notify::new());
        *server.counts.auth_gate.write().unwrap() = Some(Arc::clone(&gate));
        let owner = start(&service, &config.id);
        auth_started(&server).await;
        owner.abort();
        assert!(owner.await.unwrap_err().is_cancelled());
        release(&server, &gate);
        let handle = service.connect(&config.id).await.unwrap();
        assert_eq!(
            service
                .exec(&handle, "retry-after-abandon")
                .await
                .unwrap()
                .stdout,
            "ok\n"
        );
        assert_eq!(server.auth_count(), 2);
        service.disconnect(&config.id).await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn explicit_begin_and_automatic_checkout_share_the_network_phase() {
    bounded(async {
        let server = SshServer::start("127.0.0.1").await;
        let data = tempfile::tempdir().unwrap();
        let trust = data.path().join("known_hosts");
        server.write_trust(&trust);
        let service = Arc::new(SshService::new(data.path().to_path_buf()));
        let config = server.create(&service, Some(&trust)).await;
        let gate = Arc::new(Notify::new());
        *server.counts.auth_gate.write().unwrap() = Some(Arc::clone(&gate));
        let owner = start(&service, &config.id);
        auth_started(&server).await;
        let other_service = Arc::clone(&service);
        let other_id = config.id.clone();
        let explicit = tokio::spawn(async move { other_service.begin_connection(&other_id).await });
        tokio::time::sleep(Duration::from_millis(100)).await;
        release(&server, &gate);
        owner.await.unwrap().unwrap();
        assert!(matches!(
            explicit.await.unwrap().unwrap(),
            SshConnectionAttempt::Connected
        ));
        assert_eq!(
            server.auth_count(),
            1,
            "UI and file operations must not own independent handshakes"
        );
        service.disconnect(&config.id).await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn a_blocked_connection_does_not_block_an_unrelated_target() {
    bounded(async {
        let blocked = SshServer::start("127.0.0.1").await;
        let other = SshServer::start("127.0.0.1").await;
        let data = tempfile::tempdir().unwrap();
        let trust = data.path().join("known_hosts");
        blocked.write_trust(&trust);
        let other_trust = data.path().join("other_known_hosts");
        other.write_trust(&other_trust);
        let service = Arc::new(SshService::new(data.path().to_path_buf()));
        let first = blocked.create(&service, Some(&trust)).await;
        let second = other.create(&service, Some(&other_trust)).await;
        let gate = Arc::new(Notify::new());
        *blocked.counts.auth_gate.write().unwrap() = Some(Arc::clone(&gate));
        let owner = start(&service, &first.id);
        auth_started(&blocked).await;
        let handle = service.connect(&second.id).await.unwrap();
        assert_eq!(
            service
                .exec(&handle, "unrelated-target")
                .await
                .unwrap()
                .stdout,
            "ok\n"
        );
        release(&blocked, &gate);
        owner.await.unwrap().unwrap();
        service.disconnect(&first.id).await.unwrap();
        service.disconnect(&second.id).await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn editing_configuration_retires_a_blocked_handshake_without_waiting_for_it() {
    bounded(async {
        let server = SshServer::start("127.0.0.1").await;
        let data = tempfile::tempdir().unwrap();
        let trust = data.path().join("known_hosts");
        server.write_trust(&trust);
        let service = Arc::new(SshService::new(data.path().to_path_buf()));
        let config = server.create(&service, Some(&trust)).await;
        let gate = Arc::new(Notify::new());
        *server.counts.auth_gate.write().unwrap() = Some(Arc::clone(&gate));
        let owner = start(&service, &config.id);
        auth_started(&server).await;
        let request: UpdateSshConnectionRequest = serde_json::from_value(serde_json::json!({
            "id":config.id,"name":"new credential revision","host":server.addr.ip().to_string(),
            "port":server.addr.port(),"username":"new-user","auth_type":"password",
            "password":"new-fixture-secret","known_hosts_path":trust,
        }))
        .unwrap();
        let updated =
            tokio::time::timeout(Duration::from_secs(2), service.update_connection(request)).await;
        release(&server, &gate);
        assert!(
            updated.is_ok(),
            "configuration updates must not await a blocked network handshake"
        );
        assert_eq!(updated.unwrap().unwrap().username, "new-user");
        assert!(
            owner.await.unwrap().is_err(),
            "the old credential revision must never publish"
        );
        let current = service.connect(&config.id).await.unwrap();
        assert_eq!(
            service
                .exec(&current, "updated-config")
                .await
                .unwrap()
                .stdout,
            "ok\n"
        );
        service.disconnect(&config.id).await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn concurrent_cold_checkouts_load_configuration_once_before_admission() {
    bounded(async {
        let server = SshServer::start("127.0.0.1").await;
        let data = tempfile::tempdir().unwrap();
        let trust = data.path().join("known_hosts");
        server.write_trust(&trust);
        let writer = SshService::new(data.path().to_path_buf());
        let config = server.create(&writer, Some(&trust)).await;
        drop(writer);
        let service = Arc::new(SshService::new(data.path().to_path_buf()));
        let mut requests = Vec::new();
        for _ in 0..8 {
            requests.push(start(&service, &config.id));
        }
        for request in requests {
            request
                .await
                .unwrap()
                .expect("initial loads cannot retire another initial checkout");
        }
        assert_eq!(
            server.auth_count(),
            1,
            "cold admission must also share exactly one transport"
        );
        service.disconnect(&config.id).await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn reloading_configuration_cannot_write_the_retired_configuration_back_to_disk() {
    bounded(async {
        let server = SshServer::start("127.0.0.1").await;
        let data = tempfile::tempdir().unwrap();
        let trust = data.path().join("known_hosts");
        server.write_trust(&trust);
        let service = SshService::new(data.path().to_path_buf());
        let config = server.create(&service, Some(&trust)).await;
        service.connect(&config.id).await.unwrap();
        let editor = SshService::new(data.path().to_path_buf());
        let request: UpdateSshConnectionRequest = serde_json::from_value(serde_json::json!({
            "id":config.id,"name":"reloaded revision","host":server.addr.ip().to_string(),
            "port":server.addr.port(),"username":"newly-loaded-user","auth_type":"password",
            "password":"newly-loaded-fixture-secret","known_hosts_path":trust,
        }))
        .unwrap();
        editor.update_connection(request).await.unwrap();
        service.load().await.unwrap();
        assert_eq!(
            service.list_connections().await[0].username,
            "newly-loaded-user"
        );
        let reopened = SshService::new(data.path().to_path_buf());
        assert_eq!(
            reopened.list_connections().await[0].username,
            "newly-loaded-user",
            "retiring a previous runtime must not persist its obsolete configuration"
        );
    })
    .await;
}

#[tokio::test]
async fn metadata_reload_preserves_an_inflight_connection_and_its_verified_transport() {
    bounded(async {
        let server = SshServer::start("127.0.0.1").await;
        let data = tempfile::tempdir().unwrap();
        let trust = data.path().join("known_hosts");
        server.write_trust(&trust);
        let service = Arc::new(SshService::new(data.path().to_path_buf()));
        let config = server.create(&service, Some(&trust)).await;
        let gate = Arc::new(Notify::new());
        *server.counts.auth_gate.write().unwrap() = Some(Arc::clone(&gate));
        let owner = start(&service, &config.id);
        auth_started(&server).await;
        let path = data.path().join("deepagent-ssh/connections.json");
        let mut stored: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        stored[&config.id]["name"] = serde_json::json!("renamed during handshake");
        stored[&config.id]["cached_checked_at_ms"] = serde_json::Value::Null;
        std::fs::write(&path, serde_json::to_vec(&stored).unwrap()).unwrap();
        service.load().await.unwrap();
        release(&server, &gate);
        let handle = owner
            .await
            .unwrap()
            .expect("metadata cannot retire an unchanged identity");
        service.connect(&config.id).await.unwrap();
        assert_eq!(
            server.auth_count(),
            1,
            "reload preserves the verified transport"
        );
        assert_eq!(
            service
                .exec(&handle, "after-metadata-reload")
                .await
                .unwrap()
                .stdout,
            "ok\n"
        );
        service.disconnect(&config.id).await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn identity_reload_cancels_the_old_handshake_without_waiting_for_a_server_reply() {
    bounded(async {
        let fingerprint = support::random_key()
            .public_key()
            .fingerprint(russh::keys::HashAlg::Sha256)
            .to_string();
        for (field, value) in [
            ("username", "new-user"),
            ("agent_fingerprint", fingerprint.as_str()),
        ] {
            let server = SshServer::start("127.0.0.1").await;
            let data = tempfile::tempdir().unwrap();
            let trust = data.path().join("known_hosts");
            server.write_trust(&trust);
            let service = Arc::new(SshService::new(data.path().to_path_buf()));
            let config = server.create(&service, Some(&trust)).await;
            let gate = Arc::new(Notify::new());
            *server.counts.auth_gate.write().unwrap() = Some(Arc::clone(&gate));
            let owner = start(&service, &config.id);
            auth_started(&server).await;
            let path = data.path().join("deepagent-ssh/connections.json");
            let mut stored: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            stored[&config.id][field] = serde_json::json!(value);
            std::fs::write(&path, serde_json::to_vec(&stored).unwrap()).unwrap();
            service.load().await.unwrap();
            let retired = tokio::time::timeout(Duration::from_secs(2), owner).await;
            release(&server, &gate);
            assert!(retired
                .expect("reload must cancel the old handshake")
                .unwrap()
                .is_err());
            let current = service.connect(&config.id).await.unwrap();
            assert_eq!(server.auth_count(), 2);
            assert_eq!(
                service
                    .exec(&current, "after-identity-reload")
                    .await
                    .unwrap()
                    .stdout,
                "ok\n"
            );
            service.disconnect(&config.id).await.unwrap();
        }
    })
    .await;
}

#[tokio::test]
async fn an_old_cached_trust_failure_cannot_overwrite_an_edited_configuration_status() {
    use std::task::Poll;

    bounded(async {
        let server = SshServer::start("127.0.0.1").await;
        let data = tempfile::tempdir().unwrap();
        let trust = data.path().join("known_hosts");
        server.write_trust(&trust);
        let service = SshService::new(data.path().to_path_buf());
        let config = server.create(&service, Some(&trust)).await;
        service.connect(&config.id).await.unwrap();
        std::fs::write(&trust, "# revoked\n").unwrap();
        let mut revalidating = Box::pin(service.connect(&config.id));
        std::future::poll_fn(|cx| match revalidating.as_mut().poll(cx) {
            Poll::Pending => Poll::Ready(()),
            Poll::Ready(_) => panic!("cached trust validation must yield for its real file read"),
        })
        .await;
        let request: UpdateSshConnectionRequest = serde_json::from_value(serde_json::json!({
            "id": config.id, "name": "edited", "host": config.host, "port": config.port,
            "username": "new-user", "auth_type": "password", "password": "new-fixture-secret",
            "known_hosts_path": trust,
        }))
        .unwrap();
        let mut updating = Box::pin(service.update_connection(request));
        let result = match tokio::time::timeout(Duration::from_millis(100), &mut updating).await {
            Ok(updated) => {
                updated.unwrap();
                revalidating.await
            }
            Err(_) => {
                let (previous, updated) = tokio::join!(revalidating, updating);
                updated.unwrap();
                previous
            }
        };
        assert!(result.is_err());
        assert_eq!(server.auth_count(), 1);
        assert_eq!(service.list_connections().await[0].username, "new-user");
        assert_eq!(
            service.status(&config.id).await.unwrap().status,
            SshStatus::Disconnected,
            "the retired trust failure cannot publish into the edited configuration"
        );
    })
    .await;
}

#[tokio::test]
async fn disconnect_wakes_all_waiters_without_a_server_authentication_reply() {
    bounded(async {
        let server = SshServer::start("127.0.0.1").await;
        let data = tempfile::tempdir().unwrap();
        let trust = data.path().join("known_hosts");
        server.write_trust(&trust);
        let service = Arc::new(SshService::new(data.path().to_path_buf()));
        let config = server.create(&service, Some(&trust)).await;
        let gate = Arc::new(Notify::new());
        *server.counts.auth_gate.write().unwrap() = Some(Arc::clone(&gate));
        let mut requests = vec![start(&service, &config.id)];
        auth_started(&server).await;
        for _ in 0..6 {
            requests.push(start(&service, &config.id));
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
        service.disconnect(&config.id).await.unwrap();
        for request in requests {
            let result = tokio::time::timeout(Duration::from_secs(1), request).await;
            assert!(
                result.is_ok(),
                "cancellation must not wait for the blocked server"
            );
            assert!(result.unwrap().unwrap().is_err());
        }
        assert_eq!(server.auth_count(), 1);
        release(&server, &gate);
        assert_eq!(
            service.status(&config.id).await.unwrap().status,
            SshStatus::Disconnected
        );
    })
    .await;
}

#[tokio::test]
async fn disconnect_also_fences_the_host_confirmation_resume() {
    bounded(async {
        let server = SshServer::start("127.0.0.1").await;
        let data = tempfile::tempdir().unwrap();
        let service = Arc::new(SshService::new(data.path().to_path_buf()));
        let config = server.create(&service, None).await;
        let SshConnectionAttempt::ConfirmationRequired { challenge } =
            service.begin_connection(&config.id).await.unwrap()
        else {
            panic!("unknown target must await explicit verification")
        };
        let gate = Arc::new(Notify::new());
        *server.counts.auth_gate.write().unwrap() = Some(Arc::clone(&gate));
        let other_service = Arc::clone(&service);
        let other_id = config.id.clone();
        let resume = tokio::spawn(async move {
            other_service
                .respond_host_key(
                    &other_id,
                    &challenge.attempt_id,
                    &challenge.challenge_id,
                    SshHostKeyDecision::Once,
                )
                .await
        });
        auth_started(&server).await;
        service.disconnect(&config.id).await.unwrap();
        let result = tokio::time::timeout(Duration::from_secs(1), resume).await;
        release(&server, &gate);
        assert!(result.unwrap().unwrap().is_err());
        assert_eq!(
            service.status(&config.id).await.unwrap().status,
            SshStatus::Disconnected
        );
        assert!(
            matches!(
                service.begin_connection(&config.id).await.unwrap(),
                SshConnectionAttempt::ConfirmationRequired { .. }
            ),
            "retired once-trust cannot be reused"
        );
    })
    .await;
}

#[tokio::test]
async fn disconnect_before_a_host_confirmation_commit_cannot_save_stale_trust() {
    use deepagent_persistence::Database;
    use deepagent_ssh::{SshHostIdentity, SshHostKeyStore};
    use std::task::Poll;

    bounded(async {
        let server = SshServer::start("127.0.0.1").await;
        let data = tempfile::tempdir().unwrap();
        let store = Arc::new(SshHostKeyStore::new(Arc::new(
            Database::open_in_memory().unwrap(),
        )));
        let service =
            SshService::new(data.path().to_path_buf()).with_host_key_store(Arc::clone(&store));
        let config = server.create(&service, None).await;
        let SshConnectionAttempt::ConfirmationRequired { challenge } =
            service.begin_connection(&config.id).await.unwrap()
        else {
            panic!("unknown host requires confirmation")
        };
        let mut response = Box::pin(service.respond_host_key(
            &config.id,
            &challenge.attempt_id,
            &challenge.challenge_id,
            SshHostKeyDecision::Save,
        ));
        // Poll once to start the real asynchronous trust read, then request
        // disconnect before polling any subsequent commit or network phase.
        std::future::poll_fn(|cx| match response.as_mut().poll(cx) {
            Poll::Pending => Poll::Ready(()),
            Poll::Ready(_) => panic!("the local trust read must yield before commit"),
        })
        .await;
        let mut disconnect = Box::pin(service.disconnect(&config.id));
        let completed = std::future::poll_fn(|cx| {
            Poll::Ready(match disconnect.as_mut().poll(cx) {
                Poll::Ready(result) => Some(result),
                Poll::Pending => None,
            })
        })
        .await;
        let result = match completed {
            Some(result) => {
                result.unwrap();
                response.await
            }
            None => {
                let (response, disconnect) = tokio::join!(response, disconnect);
                disconnect.unwrap();
                response
            }
        };
        assert!(result.is_err());
        assert_eq!(server.auth_count(), 0);
        let identity = SshHostIdentity::direct(&config.host, config.port).unwrap();
        assert!(
            store.list(&identity).unwrap().is_empty(),
            "retired confirmation cannot save trust"
        );
        assert!(store.audit_history(&identity).unwrap().is_empty());
    })
    .await;
}

#[tokio::test]
async fn a_blocked_mfa_resume_does_not_block_another_connection_or_disconnect() {
    bounded(async {
        use std::sync::atomic::Ordering;
        let server = SshServer::start("127.0.0.1").await;
        server
            .counts
            .require_password_after_interactive
            .store(true, Ordering::SeqCst);
        let other = SshServer::start("127.0.0.1").await;
        let data = tempfile::tempdir().unwrap();
        let trust = data.path().join("known_hosts");
        server.write_trust(&trust);
        let other_trust = data.path().join("other_known_hosts");
        other.write_trust(&other_trust);
        let service = Arc::new(SshService::new(data.path().to_path_buf()));
        let config = server.create(&service, Some(&trust)).await;
        let other_config = other.create(&service, Some(&other_trust)).await;
        let SshConnectionAttempt::AuthenticationRequired { challenge: first } =
            service.begin_connection(&config.id).await.unwrap()
        else {
            panic!("MFA required")
        };
        let SshConnectionAttempt::AuthenticationRequired { challenge: second } = service
            .respond_keyboard_interactive(
                &config.id,
                &first.attempt_id,
                &first.challenge_id,
                vec!["482901".into(), "terminal".into()],
            )
            .await
            .unwrap()
        else {
            panic!("second factor required")
        };
        let gate = Arc::new(Notify::new());
        *server.counts.auth_gate.write().unwrap() = Some(Arc::clone(&gate));
        let before = server.auth_count();
        let other_service = Arc::clone(&service);
        let other_id = config.id.clone();
        let resume = tokio::spawn(async move {
            other_service
                .respond_keyboard_interactive(
                    &other_id,
                    &second.attempt_id,
                    &second.challenge_id,
                    vec!["731604".into()],
                )
                .await
        });
        bounded(async {
            while server.auth_count() == before {
                tokio::task::yield_now().await;
            }
        })
        .await;
        let fresh =
            tokio::time::timeout(Duration::from_secs(2), service.connect(&other_config.id)).await;
        assert!(
            fresh.is_ok(),
            "one MFA response cannot hold a global network lock"
        );
        fresh.unwrap().unwrap();
        service.disconnect(&config.id).await.unwrap();
        let result = tokio::time::timeout(Duration::from_secs(1), resume).await;
        release(&server, &gate);
        assert!(result.unwrap().unwrap().is_err());
        assert_eq!(
            service.status(&config.id).await.unwrap().status,
            SshStatus::Disconnected
        );
        service.disconnect(&other_config.id).await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn a_diagnostic_probe_does_not_block_disconnect_on_network_io() {
    bounded(async {
        let server = SshServer::start("127.0.0.1").await;
        let data = tempfile::tempdir().unwrap();
        let trust = data.path().join("known_hosts");
        server.write_trust(&trust);
        let service = Arc::new(SshService::new(data.path().to_path_buf()));
        let config = server.create(&service, Some(&trust)).await;
        let gate = Arc::new(Notify::new());
        *server.counts.auth_gate.write().unwrap() = Some(Arc::clone(&gate));
        let input = config.clone();
        let other_service = Arc::clone(&service);
        let task = tokio::spawn(async move { other_service.test_connection(&input).await });
        auth_started(&server).await;
        let disconnected =
            tokio::time::timeout(Duration::from_secs(2), service.disconnect(&config.id)).await;
        release(&server, &gate);
        assert!(
            disconnected.is_ok(),
            "diagnostics must not keep a global lock while awaiting the network"
        );
        disconnected.unwrap().unwrap();
        let result = task.await.unwrap();
        assert!(
            result.is_err() || !result.unwrap().ok,
            "a retired diagnostic cannot report success"
        );
        assert_eq!(
            service.status(&config.id).await.unwrap().status,
            SshStatus::Disconnected
        );
    })
    .await;
}

#[tokio::test]
async fn a_background_status_probe_does_not_block_disconnect_or_publish_late_status() {
    bounded(async {
        let server = SshServer::start("127.0.0.1").await;
        let data = tempfile::tempdir().unwrap();
        let trust = data.path().join("known_hosts");
        server.write_trust(&trust);
        let service = Arc::new(SshService::new(data.path().to_path_buf()));
        let config = server.create(&service, Some(&trust)).await;
        let gate = Arc::new(Notify::new());
        *server.counts.auth_gate.write().unwrap() = Some(Arc::clone(&gate));
        let other_service = Arc::clone(&service);
        let task = tokio::spawn(async move { other_service.refresh_due_statuses().await });
        auth_started(&server).await;
        let disconnected =
            tokio::time::timeout(Duration::from_secs(2), service.disconnect(&config.id)).await;
        release(&server, &gate);
        assert!(
            disconnected.is_ok(),
            "status refresh must not block all configuration/disconnect operations"
        );
        disconnected.unwrap().unwrap();
        task.await.unwrap().unwrap();
        assert_eq!(
            service.status(&config.id).await.unwrap().status,
            SshStatus::Disconnected
        );
    })
    .await;
}

#[tokio::test]
async fn retired_and_abandoned_handshakes_close_their_real_server_connections() {
    use std::sync::atomic::Ordering;
    bounded(async {
        for abandon_owner in [false, true] {
            let server = SshServer::start("127.0.0.1").await;
            let data = tempfile::tempdir().unwrap();
            let trust = data.path().join("known_hosts");
            server.write_trust(&trust);
            let service = Arc::new(SshService::new(data.path().to_path_buf()));
            let config = server.create(&service, Some(&trust)).await;
            let gate = Arc::new(Notify::new());
            *server.counts.auth_gate.write().unwrap() = Some(Arc::clone(&gate));
            let owner = start(&service, &config.id);
            auth_started(&server).await;
            if abandon_owner {
                owner.abort();
                assert!(owner.await.unwrap_err().is_cancelled());
            } else {
                service.disconnect(&config.id).await.unwrap();
                assert!(owner.await.unwrap().is_err());
            }
            release(&server, &gate);
            tokio::time::timeout(Duration::from_secs(2), async {
                while server.counts.closed_connections.load(Ordering::SeqCst)
                    != server.counts.connections.load(Ordering::SeqCst)
                {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .expect("retired/abandoned network owners must close the real server socket");
        }
    })
    .await;
}
