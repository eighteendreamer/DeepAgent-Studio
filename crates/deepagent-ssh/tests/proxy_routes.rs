mod support;

use base64::Engine;
use deepagent_persistence::Database;
use deepagent_ssh::{
    SshConfigStore, SshConnectionAttempt, SshConnectionConfig, SshHostKeyDecision, SshHostKeyStore,
    SshResult, SshService,
};
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::{JoinHandle, JoinSet};

#[derive(Default)]
struct SecretStore(Mutex<Vec<SshConnectionConfig>>);

#[async_trait::async_trait]
impl SshConfigStore for SecretStore {
    fn supports_secret_storage(&self) -> bool {
        true
    }
    async fn load(&self) -> SshResult<Vec<SshConnectionConfig>> {
        Ok(self.0.lock().unwrap().clone())
    }
    async fn save(&self, configs: &[SshConnectionConfig]) -> SshResult<()> {
        *self.0.lock().unwrap() = configs.to_vec();
        Ok(())
    }
}

#[derive(Clone, Copy)]
enum Reply {
    Tunnel,
    Denied,
    Malformed,
    Stall,
    OpaqueHeader,
    HttpResponse(&'static [u8]),
    HeaderOverflow,
    InformationalOverflow,
}

struct Proxy {
    addr: SocketAddr,
    requests: Arc<AtomicUsize>,
    targets: Arc<Mutex<Vec<(String, u16)>>>,
    authenticated: Arc<AtomicUsize>,
    closed: Arc<AtomicUsize>,
    task: JoinHandle<()>,
}

async fn header(stream: &mut TcpStream) -> Vec<u8> {
    let mut bytes = Vec::new();
    while !bytes.ends_with(b"\r\n\r\n") {
        bytes.push(stream.read_u8().await.unwrap());
        assert!(bytes.len() <= 16 * 1024);
    }
    bytes
}

impl Proxy {
    async fn start(target: SocketAddr, mode: &'static str, auth: bool, reply: Reply) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let requests = Arc::new(AtomicUsize::new(0));
        let targets = Arc::new(Mutex::new(Vec::new()));
        let authenticated = Arc::new(AtomicUsize::new(0));
        let closed = Arc::new(AtomicUsize::new(0));
        let ended = closed.clone();
        let (seen, endpoints, accepted) =
            (requests.clone(), targets.clone(), authenticated.clone());
        let task = tokio::spawn(async move {
            let mut sessions = JoinSet::new();
            loop {
                tokio::select! {
                    socket = listener.accept() => {
                        let (mut incoming, _) = socket.unwrap();
                        seen.fetch_add(1, Ordering::SeqCst);
                        let (endpoints, accepted, ended) = (endpoints.clone(), accepted.clone(), ended.clone());
                        sessions.spawn(async move {
                            if matches!(reply, Reply::Stall) {
                                let mut sink = [0; 1024];
                                while incoming.read(&mut sink).await.unwrap_or(0) != 0 {}
                                ended.fetch_add(1, Ordering::SeqCst);
                                return;
                            }
                            if mode == "http_connect" {
                                let bytes = header(&mut incoming).await;
                                let text = std::str::from_utf8(&bytes).unwrap();
                                let authority = text.lines().next().unwrap().split_whitespace().nth(1).unwrap();
                                let (host, port) = authority.rsplit_once(':').unwrap();
                                endpoints.lock().unwrap().push((host.trim_matches(['[', ']']).into(), port.parse().unwrap()));
                                if auth {
                                    let expected = base64::engine::general_purpose::STANDARD.encode("proxy-fixture-user:proxy-fixture-password");
                                    assert!(text.contains(&format!("Proxy-Authorization: Basic {expected}\r\n")));
                                    accepted.fetch_add(1, Ordering::SeqCst);
                                } else { assert!(!text.contains("Proxy-Authorization")); }
                                let raw = match reply {
                                    Reply::HttpResponse(bytes) => Some(bytes.to_vec()),
                                    Reply::HeaderOverflow => Some([b"HTTP/1.1 200 OK\r\nX: ".as_slice(), &vec![b'a'; 16 * 1024]].concat()),
                                    Reply::InformationalOverflow => Some(b"HTTP/1.1 100 Continue\r\n\r\n".repeat(5)),
                                    _ => None,
                                };
                                if let Some(raw) = raw { let _ = incoming.write_all(&raw).await; return; }
                                if matches!(reply, Reply::Denied) {
                                    incoming.write_all(b"HTTP/1.1 407 Proxy Authentication Required\r\nX-Echo: proxy-fixture-password\r\n\r\n").await.unwrap();
                                    return;
                                }
                                if matches!(reply, Reply::Malformed) {
                                    incoming.write_all(b"HTTP/1.1 1200 not-a-status\r\n\r\n").await.unwrap();
                                    return;
                                }
                                let mut upstream = TcpStream::connect(target).await.unwrap();
                                let mut banner = Vec::new();
                                while !banner.ends_with(b"\n") { banner.push(upstream.read_u8().await.unwrap()); }
                                // One write deliberately coalesces the CONNECT response with the SSH banner.
                                let mut response = b"HTTP/1.1 100 Continue\r\n\r\nHTTP/1.1 201 Tunnel Ready\r\nContent-Length: 999\r\n\r\n".to_vec();
                                if matches!(reply, Reply::OpaqueHeader) { response = b"HTTP/1.1 200 OK\r\nX-Trace: \xff\r\n\r\n".to_vec(); }
                                response.extend(banner);
                                incoming.write_all(&response).await.unwrap();
                                let _ = tokio::io::copy_bidirectional(&mut incoming, &mut upstream).await;
                            } else {
                                assert_eq!(incoming.read_u8().await.unwrap(), 5);
                                let n = incoming.read_u8().await.unwrap();
                                let mut methods = vec![0; n.into()];
                                incoming.read_exact(&mut methods).await.unwrap();
                                assert_eq!(methods, [if auth { 2 } else { 0 }]);
                                if matches!(reply, Reply::Malformed) {
                                    incoming.write_all(&[5, if auth { 0 } else { 2 }]).await.unwrap();
                                    return;
                                }
                                incoming.write_all(&[5, if auth { 2 } else { 0 }]).await.unwrap();
                                if auth {
                                    assert_eq!(incoming.read_u8().await.unwrap(), 1);
                                    let n = incoming.read_u8().await.unwrap();
                                    let mut username = vec![0; n.into()];
                                    incoming.read_exact(&mut username).await.unwrap();
                                    let n = incoming.read_u8().await.unwrap();
                                    let mut password = vec![0; n.into()];
                                    incoming.read_exact(&mut password).await.unwrap();
                                    assert_eq!(username, b"proxy-fixture-user");
                                    assert_eq!(password, b"proxy-fixture-password");
                                    accepted.fetch_add(1, Ordering::SeqCst);
                                    incoming.write_all(&[1, u8::from(matches!(reply, Reply::Denied))]).await.unwrap();
                                    if matches!(reply, Reply::Denied) { return; }
                                }
                                let mut prefix = [0; 4];
                                incoming.read_exact(&mut prefix).await.unwrap();
                                assert_eq!(&prefix[..3], &[5, 1, 0]);
                                let host = match prefix[3] {
                                    1 => { let mut ip = [0; 4]; incoming.read_exact(&mut ip).await.unwrap(); std::net::Ipv4Addr::from(ip).to_string() },
                                    4 => { let mut ip = [0; 16]; incoming.read_exact(&mut ip).await.unwrap(); std::net::Ipv6Addr::from(ip).to_string() },
                                    3 => { let n = incoming.read_u8().await.unwrap(); let mut name = vec![0; n.into()]; incoming.read_exact(&mut name).await.unwrap(); String::from_utf8(name).unwrap() },
                                    other => panic!("unexpected address type {other}"),
                                };
                                let port = incoming.read_u16().await.unwrap();
                                endpoints.lock().unwrap().push((host, port));
                                incoming.write_all(&[5, 0, 0, 1, 127, 0, 0, 1, 0, 22]).await.unwrap();
                                let mut upstream = TcpStream::connect(target).await.unwrap();
                                let _ = tokio::io::copy_bidirectional(&mut incoming, &mut upstream).await;
                            }
                        });
                    },
                    _ = sessions.join_next(), if !sessions.is_empty() => {},
                }
            }
        });
        Self {
            addr,
            requests,
            targets,
            authenticated,
            closed,
            task,
        }
    }

    fn config(&self, mode: &str, auth: bool) -> Value {
        let mut value = json!({"mode":mode, "host":"127.0.0.1", "port":self.addr.port()});
        if auth {
            value["username"] = json!("proxy-fixture-user");
            value["password"] = json!("proxy-fixture-password");
        }
        value
    }
}

impl Drop for Proxy {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[tokio::test]
async fn authenticated_proxy_routes_only_to_the_jump_and_preserves_independent_ssh_credentials() {
    for mode in ["http_connect", "socks5"] {
        let target = support::SshServer::start("127.0.0.1").await;
        let jump = support::SshServer::start("127.0.0.1").await;
        *jump.counts.forward_target.write().unwrap() = Some(target.addr);
        let proxy = Proxy::start(jump.addr, mode, true, Reply::Tunnel).await;
        let root = tempfile::tempdir().unwrap();
        target.write_trust(&root.path().join("known_hosts"));
        jump.write_trust(&root.path().join("jump_known_hosts"));
        let ssh =
            SshService::with_config_store(root.path().into(), Arc::new(SecretStore::default()));
        let mut wire = request(&target, root.path(), proxy.config(mode, true));
        wire["jump"] = json!({"mode":"ssh", "host":"127.0.0.1", "port":jump.addr.port(), "username":"jump-fixture", "auth_type":"password", "password":"jump-only-secret", "known_hosts_path":root.path().join("jump_known_hosts")});
        let dto = ssh
            .create_connection(serde_json::from_value(wire).unwrap())
            .await
            .unwrap();
        let handle = ssh.connect(&dto.id).await.unwrap();
        assert_eq!(ssh.exec(&handle, "echo ok").await.unwrap().stdout, "ok\n");
        assert_eq!(
            proxy.targets.lock().unwrap().as_slice(),
            &[("127.0.0.1".into(), jump.addr.port())]
        );
        assert_eq!(proxy.authenticated.load(Ordering::SeqCst), 1);
        assert_eq!(
            jump.counts.received_passwords.read().unwrap().as_slice(),
            &["jump-only-secret"]
        );
        assert_eq!(
            target.counts.received_passwords.read().unwrap().as_slice(),
            &["ssh-fixture-only"]
        );
        assert_eq!(
            jump.counts.forward_requests.read().unwrap().as_slice(),
            &[("127.0.0.1".into(), u32::from(target.addr.port()))]
        );
        ssh.disconnect(&dto.id).await.unwrap();
    }
}

fn request(server: &support::SshServer, root: &std::path::Path, proxy: Value) -> Value {
    json!({"name":"proxy fixture", "host":"127.0.0.1", "port":server.addr.port(), "username":"fixture",
        "auth_type":"password", "password":"ssh-fixture-only", "known_hosts_path":root.join("known_hosts"), "proxy":proxy})
}

#[tokio::test]
async fn both_proxy_protocols_tunnel_the_existing_ssh_transport_with_optional_authentication() {
    for mode in ["http_connect", "socks5"] {
        for auth in [false, true] {
            let server = support::SshServer::start("127.0.0.1").await;
            let proxy = Proxy::start(server.addr, mode, auth, Reply::Tunnel).await;
            let root = tempfile::tempdir().unwrap();
            server.write_trust(&root.path().join("known_hosts"));
            let ssh =
                SshService::with_config_store(root.path().into(), Arc::new(SecretStore::default()));
            let saved = ssh
                .create_connection(
                    serde_json::from_value(request(&server, root.path(), proxy.config(mode, auth)))
                        .unwrap(),
                )
                .await
                .unwrap();
            let handle = ssh.connect(&saved.id).await.unwrap();
            assert_eq!(ssh.exec(&handle, "fixture").await.unwrap().stdout, "ok\n");
            assert_eq!(
                proxy.requests.load(Ordering::SeqCst),
                1,
                "configured proxy must not be bypassed"
            );
            assert_eq!(
                proxy.authenticated.load(Ordering::SeqCst),
                usize::from(auth)
            );
            assert_eq!(server.counts.connections.load(Ordering::SeqCst), 1);
            let dto = serde_json::to_value(saved).unwrap();
            assert_eq!(dto["proxy"]["has_password"], auth);
            assert!(dto["proxy"].get("password").is_none());
        }
    }
}

#[tokio::test]
async fn proxy_authentication_and_protocol_failures_never_fall_back_or_reach_ssh_authentication() {
    for mode in ["http_connect", "socks5"] {
        for (reply, expected) in [
            (Reply::Denied, "authentication"),
            (Reply::Malformed, "protocol"),
        ] {
            let server = support::SshServer::start("127.0.0.1").await;
            let proxy = Proxy::start(server.addr, mode, true, reply).await;
            let root = tempfile::tempdir().unwrap();
            server.write_trust(&root.path().join("known_hosts"));
            let ssh =
                SshService::with_config_store(root.path().into(), Arc::new(SecretStore::default()));
            let saved = ssh
                .create_connection(
                    serde_json::from_value(request(&server, root.path(), proxy.config(mode, true)))
                        .unwrap(),
                )
                .await
                .unwrap();
            let error = ssh
                .connect(&saved.id)
                .await
                .expect_err("proxy rejection must stop the connection")
                .to_string();
            assert!(
                error.contains("proxy") && error.contains(expected),
                "{error}"
            );
            assert!(!error.contains("proxy-fixture-password"));
            assert_eq!(server.auth_count(), 0);
            assert_eq!(server.counts.connections.load(Ordering::SeqCst), 0);
        }
    }
}

#[tokio::test]
async fn proxy_preserves_remote_dns_and_ipv6_target_authorities() {
    for mode in ["http_connect", "socks5"] {
        for host in ["remote-only.invalid", "2001:db8::42"] {
            let server = support::SshServer::start("127.0.0.1").await;
            let proxy = Proxy::start(server.addr, mode, false, Reply::Tunnel).await;
            let root = tempfile::tempdir().unwrap();
            let path = root.path().join("known_hosts");
            server.write_trust(&path);
            let text = std::fs::read_to_string(&path)
                .unwrap()
                .replace("127.0.0.1", host);
            std::fs::write(&path, text).unwrap();
            let ssh =
                SshService::with_config_store(root.path().into(), Arc::new(SecretStore::default()));
            let mut wire = request(&server, root.path(), proxy.config(mode, false));
            wire["host"] = json!(host);
            let saved = ssh
                .create_connection(serde_json::from_value(wire).unwrap())
                .await
                .unwrap();
            let handle = ssh.connect(&saved.id).await.unwrap();
            assert_eq!(ssh.exec(&handle, "fixture").await.unwrap().stdout, "ok\n");
            assert_eq!(
                *proxy.targets.lock().unwrap(),
                [(host.to_owned(), server.addr.port())]
            );
        }
    }
}

#[tokio::test]
async fn proxy_secrets_require_encryption_and_only_same_endpoint_edits_preserve_them() {
    let server = support::SshServer::start("127.0.0.1").await;
    let proxy = Proxy::start(server.addr, "http_connect", true, Reply::Tunnel).await;
    let root = tempfile::tempdir().unwrap();
    let wire = request(&server, root.path(), proxy.config("http_connect", true));
    let insecure = SshService::new(root.path().join("insecure"));
    assert!(insecure
        .create_connection(serde_json::from_value(wire.clone()).unwrap())
        .await
        .is_err());
    let storage = Arc::new(SecretStore::default());
    let ssh = SshService::with_config_store(root.path().into(), storage.clone());
    let saved = ssh
        .create_connection(serde_json::from_value(wire.clone()).unwrap())
        .await
        .unwrap();
    let mut update = wire;
    update["id"] = json!(saved.id);
    update["proxy"].as_object_mut().unwrap().remove("password");
    ssh.update_connection(serde_json::from_value(update.clone()).unwrap())
        .await
        .unwrap();
    let configs = serde_json::to_value(storage.0.lock().unwrap().clone()).unwrap();
    assert_eq!(configs[0]["proxy"]["password"], "proxy-fixture-password");
    update["proxy"]["port"] = json!(proxy.addr.port().saturating_sub(1));
    let saved = ssh
        .update_connection(serde_json::from_value(update).unwrap())
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(saved).unwrap()["proxy"]["has_password"],
        false
    );
    assert!(!format!("{:?}", storage.0.lock().unwrap()).contains("proxy-fixture-password"));
}

#[tokio::test]
async fn proxy_host_key_probe_enrollment_and_audit_are_isolated_from_direct_trust() {
    let server = support::SshServer::start("127.0.0.1").await;
    let proxy = Proxy::start(server.addr, "socks5", false, Reply::Tunnel).await;
    let root = tempfile::tempdir().unwrap();
    let trust = Arc::new(SshHostKeyStore::new(Arc::new(
        Database::open_in_memory().unwrap(),
    )));
    let ssh = SshService::with_config_store(root.path().into(), Arc::new(SecretStore::default()))
        .with_host_key_store(trust);
    let mut wire = request(&server, root.path(), proxy.config("socks5", false));
    wire["host_key_policy"] = json!("accept_new");
    let proxied = ssh
        .create_connection(serde_json::from_value(wire.clone()).unwrap())
        .await
        .unwrap();
    ssh.connect(&proxied.id).await.unwrap();
    assert_eq!(
        proxy.requests.load(Ordering::SeqCst),
        2,
        "probe and pinned authentication must both use the proxy"
    );
    assert_eq!(server.auth_count(), 1);
    assert_eq!(ssh.list_host_keys(&proxied.id).await.unwrap().len(), 1);
    let audit = serde_json::to_value(ssh.host_trust_audit(&proxied.id).await.unwrap()).unwrap();
    assert_eq!(audit["route"], "socks5");
    assert_eq!(audit["proxy"]["port"], proxy.addr.port());
    wire["proxy"] = json!({"mode":"direct"});
    wire["host_key_policy"] = json!("strict");
    let direct = ssh
        .create_connection(serde_json::from_value(wire).unwrap())
        .await
        .unwrap();
    assert!(
        ssh.list_host_keys(&direct.id).await.unwrap().is_empty(),
        "direct trust must not inherit the proxy route"
    );
}

#[tokio::test]
async fn http_connect_accepts_opaque_header_values_without_losing_the_coalesced_ssh_banner() {
    let server = support::SshServer::start("127.0.0.1").await;
    let proxy = Proxy::start(server.addr, "http_connect", false, Reply::OpaqueHeader).await;
    let root = tempfile::tempdir().unwrap();
    server.write_trust(&root.path().join("known_hosts"));
    let ssh = SshService::with_config_store(root.path().into(), Arc::new(SecretStore::default()));
    let saved = ssh
        .create_connection(
            serde_json::from_value(request(
                &server,
                root.path(),
                proxy.config("http_connect", false),
            ))
            .unwrap(),
        )
        .await
        .unwrap();
    let handle = ssh.connect(&saved.id).await.unwrap();
    assert_eq!(ssh.exec(&handle, "fixture").await.unwrap().stdout, "ok\n");
    assert_eq!(proxy.requests.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn malformed_or_excessive_http_replies_stop_before_the_ssh_handshake() {
    for reply in [
        Reply::HeaderOverflow,
        Reply::InformationalOverflow,
        Reply::HttpResponse(b"HTTP/1.1 200 OK\r\nBad Header: value\r\n\r\n"),
        Reply::HttpResponse(b"HTTP/1.1 200 OK\r\nX: \x01\r\n\r\n"),
        Reply::HttpResponse(b"HTTP/1.1 200 OK\r\nMissing-Colon\r\n\r\n"),
    ] {
        let server = support::SshServer::start("127.0.0.1").await;
        let proxy = Proxy::start(server.addr, "http_connect", false, reply).await;
        let root = tempfile::tempdir().unwrap();
        server.write_trust(&root.path().join("known_hosts"));
        let ssh =
            SshService::with_config_store(root.path().into(), Arc::new(SecretStore::default()));
        let saved = ssh
            .create_connection(
                serde_json::from_value(request(
                    &server,
                    root.path(),
                    proxy.config("http_connect", false),
                ))
                .unwrap(),
            )
            .await
            .unwrap();
        let error = ssh.connect(&saved.id).await.unwrap_err().to_string();
        assert!(error.contains("proxy protocol"), "{error}");
        assert_eq!(server.counts.connections.load(Ordering::SeqCst), 0);
        assert_eq!(server.auth_count(), 0);
    }
}

#[tokio::test]
async fn stalled_proxy_times_out_and_closes_the_socket_without_direct_fallback() {
    for mode in ["http_connect", "socks5"] {
        let server = support::SshServer::start("127.0.0.1").await;
        let proxy = Proxy::start(server.addr, mode, false, Reply::Stall).await;
        let root = tempfile::tempdir().unwrap();
        server.write_trust(&root.path().join("known_hosts"));
        let ssh = Arc::new(SshService::with_config_store(
            root.path().into(),
            Arc::new(SecretStore::default()),
        ));
        let saved = ssh
            .create_connection(
                serde_json::from_value(request(&server, root.path(), proxy.config(mode, false)))
                    .unwrap(),
            )
            .await
            .unwrap();
        let connecting = tokio::spawn({
            let ssh = ssh.clone();
            async move { ssh.connect(&saved.id).await }
        });
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while proxy.requests.load(Ordering::SeqCst) == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        tokio::time::pause();
        tokio::time::advance(std::time::Duration::from_secs(11)).await;
        tokio::time::resume();
        let error = connecting.await.unwrap().unwrap_err().to_string();
        assert!(
            error.contains("proxy connect") && error.contains("10 seconds"),
            "{error}"
        );
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while proxy.closed.load(Ordering::SeqCst) == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(server.auth_count(), 0);
        assert_eq!(server.counts.connections.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn known_hosts_import_preview_cannot_be_committed_after_the_proxy_route_changes() {
    let server = support::SshServer::start("127.0.0.1").await;
    let proxy = Proxy::start(server.addr, "socks5", false, Reply::Tunnel).await;
    let root = tempfile::tempdir().unwrap();
    server.write_trust(&root.path().join("known_hosts"));
    let trust = Arc::new(SshHostKeyStore::new(Arc::new(
        Database::open_in_memory().unwrap(),
    )));
    let ssh = SshService::with_config_store(root.path().into(), Arc::new(SecretStore::default()))
        .with_host_key_store(trust);
    let wire = request(&server, root.path(), proxy.config("socks5", false));
    let saved = ssh
        .create_connection(serde_json::from_value(wire.clone()).unwrap())
        .await
        .unwrap();
    let preview = ssh.preview_known_hosts_import(&saved.id).await.unwrap();
    assert_eq!(preview.importable, 1);
    let mut update = wire;
    update["id"] = json!(saved.id);
    update["proxy"] = json!({"mode":"direct"});
    ssh.update_connection(serde_json::from_value(update).unwrap())
        .await
        .unwrap();
    assert!(
        ssh.import_known_hosts(&saved.id, preview).await.is_err(),
        "a reviewed proxied route must not authorize a direct import"
    );
    assert!(ssh.list_host_keys(&saved.id).await.unwrap().is_empty());
}

#[tokio::test]
async fn single_host_import_preview_cannot_authorize_another_proxy_route() {
    let server = support::SshServer::start("127.0.0.1").await;
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("known_hosts");
    server.write_trust(&source);
    let trust = Arc::new(SshHostKeyStore::new(Arc::new(
        Database::open_in_memory().unwrap(),
    )));
    let ssh = SshService::with_config_store(root.path().into(), Arc::new(SecretStore::default()))
        .with_host_key_store(trust);
    let mut wire = request(&server, root.path(), json!({"mode":"direct"}));
    let saved = ssh
        .create_connection(serde_json::from_value(wire.clone()).unwrap())
        .await
        .unwrap();
    let preview = ssh
        .inspect_host_trust(&saved.host, saved.port, source.to_str())
        .await
        .unwrap();
    wire["id"] = json!(saved.id);
    wire["proxy"] = json!({"mode":"socks5","host":"proxy.invalid","port":1080});
    ssh.update_connection(serde_json::from_value(wire).unwrap())
        .await
        .unwrap();
    assert!(
        ssh.import_host_trust(&saved.id, preview).await.is_err(),
        "a direct inspection must not authorize enrollment for a proxy route"
    );
    assert!(ssh.list_host_keys(&saved.id).await.unwrap().is_empty());
}

#[tokio::test]
async fn proxy_unknown_host_confirmation_uses_the_existing_once_and_save_lifecycle() {
    for mode in ["http_connect", "socks5"] {
        for save in [false, true] {
            let server = support::SshServer::start("127.0.0.1").await;
            let proxy = Proxy::start(server.addr, mode, false, Reply::Tunnel).await;
            let root = tempfile::tempdir().unwrap();
            let trust = Arc::new(SshHostKeyStore::new(Arc::new(
                Database::open_in_memory().unwrap(),
            )));
            let ssh =
                SshService::with_config_store(root.path().into(), Arc::new(SecretStore::default()))
                    .with_host_key_store(trust);
            let saved = ssh
                .create_connection(
                    serde_json::from_value(request(
                        &server,
                        root.path(),
                        proxy.config(mode, false),
                    ))
                    .unwrap(),
                )
                .await
                .unwrap();
            let SshConnectionAttempt::ConfirmationRequired { challenge } =
                ssh.begin_connection(&saved.id).await.unwrap()
            else {
                panic!("unknown proxy target needs confirmation")
            };
            assert_eq!(challenge.route, mode);
            assert_eq!(challenge.proxy.as_ref().unwrap().port, proxy.addr.port());
            assert_eq!(server.auth_count(), 0);
            let decision = if save {
                SshHostKeyDecision::Save
            } else {
                SshHostKeyDecision::Once
            };
            let result = ssh
                .respond_host_key(
                    &saved.id,
                    &challenge.attempt_id,
                    &challenge.challenge_id,
                    decision,
                )
                .await
                .unwrap();
            assert!(matches!(result, SshConnectionAttempt::Connected));
            assert_eq!(server.auth_count(), 1);
            assert_eq!(proxy.requests.load(Ordering::SeqCst), 2);
            assert_eq!(
                ssh.list_host_keys(&saved.id).await.unwrap().len(),
                usize::from(save)
            );
            ssh.disconnect(&saved.id).await.unwrap();
            assert_eq!(ssh.connect(&saved.id).await.is_ok(), save);
        }
    }
}

#[tokio::test]
async fn routed_trust_distinguishes_protocol_host_and_port_but_not_proxy_login_credentials() {
    let server = support::SshServer::start("127.0.0.1").await;
    let root = tempfile::tempdir().unwrap();
    server.write_trust(&root.path().join("known_hosts"));
    let trust = Arc::new(SshHostKeyStore::new(Arc::new(
        Database::open_in_memory().unwrap(),
    )));
    let ssh = SshService::with_config_store(root.path().into(), Arc::new(SecretStore::default()))
        .with_host_key_store(trust);
    let proxy = json!({"mode":"socks5","host":"proxy.invalid","port":1080});
    let saved = ssh
        .create_connection(
            serde_json::from_value(request(&server, root.path(), proxy.clone())).unwrap(),
        )
        .await
        .unwrap();
    let preview = ssh.inspect_connection_host_trust(&saved.id).await.unwrap();
    assert_eq!(
        serde_json::to_value(&preview).unwrap()["proxy"]["host"],
        "proxy.invalid"
    );
    ssh.import_host_trust(&saved.id, preview).await.unwrap();
    for (route, expected) in [
        (proxy.clone(), 1),
        (
            json!({"mode":"socks5","host":"proxy.invalid","port":1080,"username":"another-user","password":"another-fixture-secret"}),
            1,
        ),
        (
            json!({"mode":"http_connect","host":"proxy.invalid","port":1080}),
            0,
        ),
        (
            json!({"mode":"socks5","host":"other.invalid","port":1080}),
            0,
        ),
        (
            json!({"mode":"socks5","host":"proxy.invalid","port":1081}),
            0,
        ),
        (json!({"mode":"direct"}), 0),
    ] {
        let alias = ssh
            .create_connection(
                serde_json::from_value(request(&server, root.path(), route)).unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(ssh.list_host_keys(&alias.id).await.unwrap().len(), expected);
    }
    assert_eq!(server.auth_count(), 0);
}

#[tokio::test]
async fn editing_a_proxy_route_invalidates_its_pending_host_key_challenge() {
    let server = support::SshServer::start("127.0.0.1").await;
    let proxy = Proxy::start(server.addr, "socks5", false, Reply::Tunnel).await;
    let root = tempfile::tempdir().unwrap();
    let trust = Arc::new(SshHostKeyStore::new(Arc::new(
        Database::open_in_memory().unwrap(),
    )));
    let ssh = SshService::with_config_store(root.path().into(), Arc::new(SecretStore::default()))
        .with_host_key_store(trust);
    let mut wire = request(&server, root.path(), proxy.config("socks5", false));
    let saved = ssh
        .create_connection(serde_json::from_value(wire.clone()).unwrap())
        .await
        .unwrap();
    let SshConnectionAttempt::ConfirmationRequired { challenge } =
        ssh.begin_connection(&saved.id).await.unwrap()
    else {
        panic!("unknown target needs confirmation")
    };
    wire["id"] = json!(saved.id);
    wire["proxy"] = json!({"mode":"direct"});
    ssh.update_connection(serde_json::from_value(wire).unwrap())
        .await
        .unwrap();
    assert!(ssh
        .respond_host_key(
            &saved.id,
            &challenge.attempt_id,
            &challenge.challenge_id,
            SshHostKeyDecision::Save
        )
        .await
        .is_err());
    assert!(ssh.list_host_keys(&saved.id).await.unwrap().is_empty());
    assert_eq!(server.auth_count(), 0);
}
