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
    pub accept_gate: RwLock<Option<Arc<tokio::sync::Notify>>>,
    pub auth_gate: RwLock<Option<Arc<tokio::sync::Notify>>>,
    pub connections: AtomicUsize,
    pub passwords: AtomicUsize,
    pub public_keys: AtomicUsize,
    pub public_key_offers: AtomicUsize,
    pub commands: AtomicUsize,
    pub closed_channels: AtomicUsize,
    pub pty_size: RwLock<Option<(u32, u32)>>,
    pub files: Arc<RwLock<HashMap<String, Vec<u8>>>>,
}

struct Handler {
    counts: Arc<Counts>,
    channels: HashMap<ChannelId, Channel<Msg>>,
    tasks: JoinSet<()>,
}

impl server::Handler for Handler {
    type Error = russh::Error;

    async fn auth_password(&mut self, _: &str, _: &str) -> Result<Auth, Self::Error> {
        let gate = self.counts.auth_gate.read().unwrap().clone();
        self.counts.passwords.fetch_add(1, Ordering::SeqCst);
        if let Some(gate) = gate {
            gate.notified().await;
        }
        Ok(Auth::Accept)
    }

    async fn auth_publickey(&mut self, _: &str, _: &PublicKey) -> Result<Auth, Self::Error> {
        self.counts.public_keys.fetch_add(1, Ordering::SeqCst);
        Ok(Auth::Accept)
    }

    async fn auth_publickey_offered(
        &mut self,
        _: &str,
        _: &PublicKey,
    ) -> Result<Auth, Self::Error> {
        self.counts.public_key_offers.fetch_add(1, Ordering::SeqCst);
        Ok(Auth::Accept)
    }

    async fn channel_open_session(
        &mut self,
        channel: Channel<Msg>,
        _: &mut Session,
    ) -> Result<bool, Self::Error> {
        self.channels.insert(channel.id(), channel);
        Ok(true)
    }

    async fn channel_close(
        &mut self,
        channel: ChannelId,
        _: &mut Session,
    ) -> Result<(), Self::Error> {
        self.channels.remove(&channel);
        self.counts.closed_channels.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    async fn pty_request(
        &mut self,
        _: ChannelId,
        _: &str,
        cols: u32,
        rows: u32,
        _: u32,
        _: u32,
        _: &[(russh::Pty, u32)],
        _: &mut Session,
    ) -> Result<(), Self::Error> {
        *self.counts.pty_size.write().unwrap() = Some((cols, rows));
        Ok(())
    }

    async fn shell_request(
        &mut self,
        channel: ChannelId,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        session.data(channel, CryptoVec::from_slice(b"fixture$ "))?;
        Ok(())
    }

    async fn window_change_request(
        &mut self,
        _: ChannelId,
        cols: u32,
        rows: u32,
        _: u32,
        _: u32,
        _: &mut Session,
    ) -> Result<(), Self::Error> {
        *self.counts.pty_size.write().unwrap() = Some((cols, rows));
        Ok(())
    }

    async fn data(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        session.data(channel, CryptoVec::from_slice(data))?;
        Ok(())
    }

    async fn subsystem_request(
        &mut self,
        channel: ChannelId,
        name: &str,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        if name != "sftp" {
            session.channel_failure(channel)?;
            return Ok(());
        }
        session.channel_success(channel)?;
        let channel = self
            .channels
            .remove(&channel)
            .expect("fixture SFTP channel");
        let files = self.counts.files.clone();
        self.tasks.spawn(async move {
            russh_sftp::server::run(channel.into_stream(), sftp::MemoryFiles::new(files)).await;
        });
        Ok(())
    }

    async fn exec_request(
        &mut self,
        channel: ChannelId,
        command: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.counts.commands.fetch_add(1, Ordering::SeqCst);
        if command == b"fixture-rejected" {
            session.channel_failure(channel)?;
            return Ok(()); // Client must close this rejected channel itself.
        }
        session.channel_success(channel)?;
        let output = if command.starts_with(b"wc -c < ") {
            format!(
                "{}\n",
                self.counts
                    .files
                    .read()
                    .unwrap()
                    .values()
                    .next()
                    .unwrap()
                    .len()
            )
        } else if command.starts_with(b"if command -v sha256sum") {
            use sha2::Digest;
            format!(
                "{:x}\n",
                sha2::Sha256::digest(self.counts.files.read().unwrap().values().next().unwrap())
            )
        } else {
            "ok\n".to_owned()
        };
        session.data(channel, CryptoVec::from_slice(output.as_bytes()))?;
        if command == b"fixture-nonzero" {
            session.extended_data(channel, 1, CryptoVec::from_slice(b"fixture error"))?;
        }
        if command != b"fixture-missing-status" {
            session
                .exit_status_request(channel, if command == b"fixture-nonzero" { 7 } else { 0 })?;
        }
        if command == b"fixture-late-output" {
            session.data(channel, CryptoVec::from_slice("尾部\n".as_bytes()))?;
            session.extended_data(channel, 1, CryptoVec::from_slice(b"late stderr"))?;
        }
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
                        let gate = server_counts.accept_gate.read().unwrap().clone();
                        if let Some(gate) = gate {
                            gate.notified().await;
                        }
                        let config = Arc::new(server::Config {
                            keys: vec![server_key.read().expect("fixture key").clone()],
                            inactivity_timeout: None,
                            ..Default::default()
                        });
                        let handler = Handler {
                            counts: server_counts.clone(),
                            channels: HashMap::new(),
                            tasks: JoinSet::new(),
                        };
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
mod sftp;

use std::collections::HashMap;
