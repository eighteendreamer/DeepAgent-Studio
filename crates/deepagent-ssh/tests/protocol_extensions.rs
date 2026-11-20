mod support;
use deepagent_ssh::{SshAdvancedOptions, SshService, SshX11Options};
use std::{borrow::Cow, sync::atomic::Ordering, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

async fn configure(service: &SshService, id: &str, options: SshAdvancedOptions) {
    service.update_connection(serde_json::from_value(serde_json::json!({
        "id":id, "name":"extensions", "host":"127.0.0.1", "port": service.list_connections().await[0].port,
        "username":"fixture", "auth_type":"password", "password":"temporary-fixture-only", "advanced":options,
    })).unwrap()).await.unwrap();
}

#[tokio::test]
async fn weak_peer_fails_by_default_and_reports_real_algorithms_only_after_opt_in() {
    tokio::time::timeout(Duration::from_secs(20), async {
        let server = support::SshServer::start("127.0.0.1").await;
        *server.counts.preferred.write().unwrap() = Some(russh::Preferred {
            kex: Cow::Owned(vec![russh::kex::DH_G14_SHA1]),
            cipher: Cow::Owned(vec![russh::cipher::AES_128_CTR]),
            mac: Cow::Owned(vec![russh::mac::HMAC_SHA1]),
            ..Default::default()
        });
        let data = tempfile::tempdir().unwrap();
        let trust = data.path().join("known_hosts");
        server.write_trust(&trust);
        let service = SshService::new(data.path().into());
        let connection = server.create(&service, Some(&trust)).await;
        assert!(service.connect(&connection.id).await.is_err());
        assert!(service.connection_details(&connection.id).await.is_err());
        configure(
            &service,
            &connection.id,
            SshAdvancedOptions {
                allow_legacy_algorithms: true,
                ..Default::default()
            },
        )
        .await;
        service.connect(&connection.id).await.unwrap();
        let algorithms = service
            .connection_details(&connection.id)
            .await
            .unwrap()
            .algorithms
            .unwrap();
        assert_eq!(algorithms.kex, "diffie-hellman-group14-sha1");
        assert_eq!(algorithms.cipher, "aes128-ctr");
        assert_eq!(algorithms.client_mac, "hmac-sha1");
        assert_eq!(algorithms.server_mac, "hmac-sha1");
        service.disconnect(&connection.id).await.unwrap();
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn default_shell_rejects_unsolicited_agent_and_x11_channels() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let server = support::SshServer::start("127.0.0.1").await;
        let data = tempfile::tempdir().unwrap();
        let trust = data.path().join("known_hosts");
        server.write_trust(&trust);
        let service = SshService::new(data.path().into());
        let connection = server.create(&service, Some(&trust)).await;
        let base = service.connect(&connection.id).await.unwrap();
        let pty = service.pty_spawn(&base, 80, 24).await.unwrap();
        let handle = server.counts.server_handles.read().unwrap()[0].clone();
        assert!(handle.channel_open_agent().await.is_err());
        assert!(handle.channel_open_x11("127.0.0.1", 123).await.is_err());
        assert_eq!(server.counts.agent_requests.load(Ordering::SeqCst), 0);
        assert!(server.counts.x11_requests.read().unwrap().is_empty());
        service.pty_close(&pty).await.unwrap();
        service.disconnect(&connection.id).await.unwrap();
    })
    .await
    .unwrap();
}

fn setup(cookie: &str) -> Vec<u8> {
    let mut packet = vec![b'l', 0, 11, 0, 0, 0, 18, 0, 16, 0, 0, 0];
    packet.extend_from_slice(b"MIT-MAGIC-COOKIE-1\0\0");
    for index in (0..cookie.len()).step_by(2) {
        packet.push(u8::from_str_radix(&cookie[index..index + 2], 16).unwrap());
    }
    packet
}

#[tokio::test]
async fn x11_uses_fake_cookie_handles_rejection_and_retires_channels_with_last_pty() {
    tokio::time::timeout(Duration::from_secs(15), async {
        let server = support::SshServer::start("127.0.0.1").await;
        let xserver = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let display = format!("127.0.0.1:{}", xserver.local_addr().unwrap().port() - 6000);
        let data = tempfile::tempdir().unwrap();
        let trust = data.path().join("known_hosts");
        server.write_trust(&trust);
        let authority = data.path().join("Xauthority");
        let mut bytes = vec![0, 0];
        for field in [
            &[127, 0, 0, 1][..],
            display.split(':').next_back().unwrap().as_bytes(),
            b"MIT-MAGIC-COOKIE-1",
            &[9; 16],
        ] {
            bytes.extend_from_slice(&(field.len() as u16).to_be_bytes());
            bytes.extend_from_slice(field);
        }
        std::fs::write(&authority, bytes).unwrap();
        let options = SshX11Options {
            display,
            xauthority_path: Some(authority.to_str().unwrap().into()),
        };
        let service = SshService::new(data.path().into());
        let connection = server.create(&service, Some(&trust)).await;
        configure(
            &service,
            &connection.id,
            SshAdvancedOptions {
                x11: Some(options),
                ..Default::default()
            },
        )
        .await;
        let base = service.connect(&connection.id).await.unwrap();
        server
            .counts
            .reject_x11_forward
            .store(true, Ordering::SeqCst);
        let error = service
            .pty_spawn(&base, 80, 24)
            .await
            .err()
            .unwrap()
            .to_string();
        assert!(error.contains("X11 forwarding"), "{error}");
        // prepare() probes connectivity without sending authentication bytes.
        let (mut probe, _) = xserver.accept().await.unwrap();
        let mut empty = Vec::new();
        probe.read_to_end(&mut empty).await.unwrap();
        assert!(empty.is_empty());
        server
            .counts
            .reject_x11_forward
            .store(false, Ordering::SeqCst);
        let pty = service.pty_spawn(&base, 80, 24).await.unwrap();
        let cookie = server
            .counts
            .x11_requests
            .read()
            .unwrap()
            .last()
            .unwrap()
            .1
            .clone();
        assert_ne!(cookie, "09".repeat(16));
        assert_eq!(cookie.len(), 32);
        let handle = server.counts.server_handles.read().unwrap()[0].clone();
        let mut invalid = handle
            .channel_open_x11("127.0.0.1", 123)
            .await
            .unwrap()
            .into_stream();
        invalid.write_all(&setup(&"08".repeat(16))).await.unwrap();
        let mut invalid_bytes = Vec::new();
        let _ = invalid.read_to_end(&mut invalid_bytes).await;
        assert!(invalid_bytes.is_empty());
        assert!(
            tokio::time::timeout(Duration::from_millis(30), xserver.accept())
                .await
                .is_err()
        );
        let mut valid = handle
            .channel_open_x11("127.0.0.1", 123)
            .await
            .unwrap()
            .into_stream();
        valid.write_all(&setup(&cookie)).await.unwrap();
        let (mut local, _) = xserver.accept().await.unwrap();
        let mut received = vec![0; 48];
        local.read_exact(&mut received).await.unwrap();
        assert_eq!(received, setup(&"09".repeat(16)));
        local.write_all(b"X11-ok").await.unwrap();
        let mut response = [0; 6];
        valid.read_exact(&mut response).await.unwrap();
        assert_eq!(&response, b"X11-ok");
        service.pty_close(&pty).await.unwrap();
        let mut tail = Vec::new();
        let _ = valid.read_to_end(&mut tail).await;
        assert!(handle.channel_open_x11("127.0.0.1", 123).await.is_err());
        service.disconnect(&connection.id).await.unwrap();
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn unavailable_xauthority_does_not_publish_a_forwarded_shell() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let server = support::SshServer::start("127.0.0.1").await;
        let xserver = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let data = tempfile::tempdir().unwrap();
        let trust = data.path().join("known_hosts");
        server.write_trust(&trust);
        let service = SshService::new(data.path().into());
        let connection = server.create(&service, Some(&trust)).await;
        configure(
            &service,
            &connection.id,
            SshAdvancedOptions {
                x11: Some(SshX11Options {
                    display: format!("127.0.0.1:{}", xserver.local_addr().unwrap().port() - 6000),
                    xauthority_path: Some(data.path().join("missing").to_str().unwrap().into()),
                }),
                ..Default::default()
            },
        )
        .await;
        let base = service.connect(&connection.id).await.unwrap();
        assert!(service.pty_spawn(&base, 80, 24).await.is_err());
        assert!(server.counts.x11_requests.read().unwrap().is_empty());
        service.disconnect(&connection.id).await.unwrap();
    })
    .await
    .unwrap();
}
