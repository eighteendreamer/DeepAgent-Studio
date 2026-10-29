use std::net::SocketAddr;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, RwLock};

use deepagent_ssh::{CreateSshConnectionRequest, SshConnectionDto, SshService};
use russh::keys::{ssh_key::rand_core::OsRng, Algorithm, PrivateKey, PublicKey};
use russh::server::{self, Auth, Msg, Session};
use russh::{Channel, ChannelId, CryptoVec};
use tokio::net::TcpListener;
use tokio::task::{JoinHandle, JoinSet};

#[derive(Default)]
pub struct Counts {
    pub auth_gate: RwLock<Option<Arc<tokio::sync::Notify>>>,
    pub connections: AtomicUsize,
    pub passwords: AtomicUsize,
    pub public_keys: AtomicUsize,
    pub public_key_offers: AtomicUsize,
    pub commands: AtomicUsize,
}

struct Handler(Arc<Counts>);

impl server::Handler for Handler {
    type Error = russh::Error;

    async fn auth_password(&mut self, _: &str, _: &str) -> Result<Auth, Self::Error> {
        let gate = self.0.auth_gate.read().unwrap().clone();
        self.0.passwords.fetch_add(1, Ordering::SeqCst);
        if let Some(gate) = gate {
            gate.notified().await;
        }
        Ok(Auth::Accept)
    }

    async fn auth_publickey(&mut self, _: &str, _: &PublicKey) -> Result<Auth, Self::Error> {
        self.0.public_keys.fetch_add(1, Ordering::SeqCst);
        Ok(Auth::Accept)
    }

    async fn auth_publickey_offered(
        &mut self,
        _: &str,
        _: &PublicKey,
    ) -> Result<Auth, Self::Error> {
        self.0.public_key_offers.fetch_add(1, Ordering::SeqCst);
        Ok(Auth::Accept)
    }

    async fn channel_open_session(
        &mut self,
        _: Channel<Msg>,
        _: &mut Session,
    ) -> Result<bool, Self::Error> {
        Ok(true)
    }

    async fn exec_request(
        &mut self,
        channel: ChannelId,
        _: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.0.commands.fetch_add(1, Ordering::SeqCst);
        session.channel_success(channel)?;
        session.data(channel, CryptoVec::from_slice(b"ok\n"))?;
        session.exit_status_request(channel, 0)?;
        session.eof(channel)?;
        session.close(channel)?;
        Ok(())
    }
}

/// All keys, credentials and files belong to this loopback fixture, never to a user.
pub struct SshServer {
    pub addr: SocketAddr,
    pub key: Arc<RwLock<PrivateKey>>,
    pub counts: Arc<Counts>,
    task: JoinHandle<()>,
}

impl SshServer {
    pub async fn start(host: &str) -> Self {
        let listener = TcpListener::bind((host, 0)).await.expect("bind loopback");
        let addr = listener.local_addr().expect("local address");
        let key = Arc::new(RwLock::new(random_key()));
        let counts = Arc::new(Counts::default());
        let server_key = key.clone();
        let server_counts = counts.clone();
        let task = tokio::spawn(async move {
            let mut sessions = JoinSet::new();
            loop {
                tokio::select! {
                    accepted = listener.accept() => {
                        let (stream, _) = accepted.expect("accept fixture client");
                        server_counts.connections.fetch_add(1, Ordering::SeqCst);
                        let config = Arc::new(server::Config {
                            keys: vec![server_key.read().expect("fixture key").clone()],
                            inactivity_timeout: None,
                            ..Default::default()
                        });
                        let handler = Handler(server_counts.clone());
                        sessions.spawn(async move {
                            if let Ok(session) = server::run_stream(config, stream, handler).await {
                                let _ = session.await;
                            }
                        });
                    }
                    _ = sessions.join_next(), if !sessions.is_empty() => {}
                }
            }
        });
        Self {
            addr,
            key,
            counts,
            task,
        }
    }

    pub fn write_trust(&self, path: &Path) {
        let public = self
            .key
            .read()
            .expect("fixture key")
            .public_key()
            .to_openssh()
            .expect("encode public key");
        std::fs::create_dir_all(path.parent().expect("known_hosts parent")).expect("create parent");
        std::fs::write(
            path,
            format!("[{}]:{} {public}\n", self.addr.ip(), self.addr.port()),
        )
        .expect("write fixture trust");
    }

    pub async fn create(&self, service: &SshService, trust: Option<&Path>) -> SshConnectionDto {
        let request: CreateSshConnectionRequest = serde_json::from_value(serde_json::json!({
            "name": "loopback-only", "host": self.addr.ip().to_string(), "port": self.addr.port(),
            "username": "fixture", "auth_type": "password", "key_path": null,
            "password": "temporary-fixture-only", "known_hosts_path": trust,
        }))
        .expect("create DTO");
        service
            .create_connection(request)
            .await
            .expect("create fixture connection")
    }

    pub fn auth_count(&self) -> usize {
        self.counts.passwords.load(Ordering::SeqCst)
            + self.counts.public_keys.load(Ordering::SeqCst)
    }
}

impl Drop for SshServer {
    fn drop(&mut self) {
        // Dropping the JoinSet aborts its children, including failed handshakes.
        self.task.abort();
    }
}

pub fn random_key() -> PrivateKey {
    PrivateKey::random(&mut OsRng, Algorithm::Ed25519).expect("generate temporary key")
}
