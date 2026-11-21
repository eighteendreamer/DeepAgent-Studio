#[allow(dead_code)]
mod support;

use deepagent_ssh::{SshService, SshStatus};
use serde_json::json;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

struct Relay {
    port: u16,
    discard_replies: Arc<AtomicBool>,
    requests: Arc<AtomicUsize>,
    replies: Arc<AtomicUsize>,
    traffic: Arc<tokio::sync::Notify>,
    task: tokio::task::JoinHandle<()>,
}

impl Relay {
    async fn start(target: std::net::SocketAddr) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let discard_replies = Arc::new(AtomicBool::new(false));
        let requests = Arc::new(AtomicUsize::new(0));
        let replies = Arc::new(AtomicUsize::new(0));
        let traffic = Arc::new(tokio::sync::Notify::new());
        let (discard, sent, received, notify) = (
            discard_replies.clone(),
            requests.clone(),
            replies.clone(),
            traffic.clone(),
        );
        let task = tokio::spawn(async move {
            let (mut client, _) = listener.accept().await.unwrap();
            let mut server = TcpStream::connect(target).await.unwrap();
            let (mut client_read, mut client_write) = client.split();
            let (mut server_read, mut server_write) = server.split();
            let outbound = async {
                let mut bytes = [0; 8192];
                loop {
                    let size = client_read.read(&mut bytes).await.unwrap();
                    if size == 0 {
                        break;
                    }
                    server_write.write_all(&bytes[..size]).await.unwrap();
                    sent.fetch_add(size, Ordering::SeqCst);
                    notify.notify_one();
                }
            };
            let inbound = async {
                let mut bytes = [0; 8192];
                loop {
                    let size = server_read.read(&mut bytes).await.unwrap();
                    if size == 0 {
                        break;
                    }
                    if !discard.load(Ordering::SeqCst) {
                        client_write.write_all(&bytes[..size]).await.unwrap();
                        received.fetch_add(size, Ordering::SeqCst);
                        notify.notify_one();
                    }
                }
            };
            tokio::select! { _ = outbound => {}, _ = inbound => {} }
        });
        Self {
            port,
            discard_replies,
            requests,
            replies,
            traffic,
            task,
        }
    }

    async fn tick_until_traffic(&self, counter: &AtomicUsize) {
        let before = counter.load(Ordering::SeqCst);
        tokio::time::pause();
        tokio::time::advance(Duration::from_millis(1010)).await;
        tokio::time::resume();
        tokio::time::timeout(Duration::from_millis(500), async {
            while counter.load(Ordering::SeqCst) == before {
                self.traffic.notified().await;
            }
        })
        .await
        .expect("native SSH keepalive traffic was not observed");
    }
}

impl Drop for Relay {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn connected(
    inactivity: Option<u32>,
    interval: Option<u32>,
    max: u32,
) -> (
    support::SshServer,
    Relay,
    tempfile::TempDir,
    SshService,
    deepagent_ssh::SshServiceHandle,
) {
    let server = support::SshServer::start("127.0.0.1").await;
    let relay = Relay::start(server.addr).await;
    let root = tempfile::tempdir().unwrap();
    let trust = root.path().join("known_hosts");
    std::fs::write(
        &trust,
        format!(
            "[127.0.0.1]:{} {}\n",
            relay.port,
            server
                .key
                .read()
                .unwrap()
                .public_key()
                .to_openssh()
                .unwrap()
        ),
    )
    .unwrap();
    let ssh = SshService::new(root.path().to_owned());
    let dto = ssh.create_connection(serde_json::from_value(json!({
        "name":"timing loopback", "host":"127.0.0.1", "port":relay.port, "username":"fixture",
        "auth_type":"password", "password":"fixture-only", "known_hosts_path":trust,
        "timing":{"connect_timeout_secs":5,"inactivity_timeout_secs":inactivity,
            "keepalive_interval_secs":interval,"keepalive_max":max}
    })).unwrap()).await.unwrap();
    let handle = ssh.connect(&dto.id).await.unwrap();
    (server, relay, root, ssh, handle)
}

#[tokio::test]
async fn inactivity_closes_the_native_connection_and_exposes_its_reason() {
    let (server, _relay, _root, ssh, handle) = connected(Some(1), None, 3).await;
    tokio::time::pause();
    tokio::time::advance(Duration::from_millis(1010)).await;
    tokio::time::resume();
    tokio::time::timeout(Duration::from_millis(500), async {
        while server.counts.closed_connections.load(Ordering::SeqCst) == 0 {
            server.counts.connection_closed.notified().await;
        }
    })
    .await
    .unwrap();
    let status = ssh.status(&handle.connection_id).await.unwrap();
    assert_eq!(status.status, SshStatus::Error);
    assert!(status.last_error.unwrap().contains("inactivity"));
    assert_eq!(ssh.list_connections().await[0].status, SshStatus::Error);
}

#[tokio::test]
async fn real_channel_activity_resets_inactivity() {
    let (server, _relay, _root, ssh, handle) = connected(Some(1), None, 3).await;
    for _ in 0..3 {
        tokio::time::pause();
        tokio::time::advance(Duration::from_millis(700)).await;
        tokio::time::resume();
        ssh.exec(&handle, "echo active").await.unwrap();
    }
    assert_eq!(server.counts.connections.load(Ordering::SeqCst), 1);
    ssh.disconnect(&handle.connection_id).await.unwrap();
}

#[tokio::test]
async fn native_keepalive_replies_preserve_the_same_connection() {
    let (server, relay, _root, ssh, handle) = connected(None, Some(1), 1).await;
    relay.tick_until_traffic(&relay.replies).await;
    // Observe a second native response on real I/O time: advancing the virtual
    // clock before the client consumes the first reply would race its timer reset.
    let before = relay.replies.load(Ordering::SeqCst);
    tokio::time::timeout(Duration::from_millis(2500), async {
        while relay.replies.load(Ordering::SeqCst) == before {
            relay.traffic.notified().await;
        }
    })
    .await
    .expect("a replying server must survive keepalive_max=1");
    ssh.exec(&handle, "echo alive").await.unwrap();
    assert_eq!(server.counts.connections.load(Ordering::SeqCst), 1);
    ssh.disconnect(&handle.connection_id).await.unwrap();
}

#[tokio::test]
async fn configurable_missed_keepalive_limit_closes_a_silent_connection() {
    for max in [1, 2] {
        let (server, relay, _root, ssh, handle) = connected(None, Some(1), max).await;
        relay.discard_replies.store(true, Ordering::SeqCst);
        for _ in 0..max {
            relay.tick_until_traffic(&relay.requests).await;
            assert_eq!(server.counts.closed_connections.load(Ordering::SeqCst), 0);
        }
        tokio::time::pause();
        tokio::time::advance(Duration::from_millis(1010)).await;
        tokio::time::resume();
        tokio::time::timeout(Duration::from_millis(500), async {
            while server.counts.closed_connections.load(Ordering::SeqCst) == 0 {
                server.counts.connection_closed.notified().await;
            }
        })
        .await
        .unwrap();
        let status = ssh.status(&handle.connection_id).await.unwrap();
        assert_eq!(status.status, SshStatus::Error);
        assert!(status.last_error.unwrap().contains("keepalive"));
    }
}

#[tokio::test]
async fn disabling_inactivity_and_keepalive_leaves_idle_connections_open() {
    let (server, _relay, _root, ssh, handle) = connected(None, None, 3).await;
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(1000)).await;
    tokio::time::resume();
    ssh.exec(&handle, "echo no timers").await.unwrap();
    assert_eq!(server.counts.connections.load(Ordering::SeqCst), 1);
    ssh.disconnect(&handle.connection_id).await.unwrap();
}
