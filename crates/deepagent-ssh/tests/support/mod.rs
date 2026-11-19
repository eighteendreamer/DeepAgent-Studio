use std::net::SocketAddr;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, RwLock};

use deepagent_ssh::{CreateSshConnectionRequest, SshConnectionDto, SshService};
use russh::keys::{ssh_key::Fingerprint, Algorithm, Certificate, PrivateKey};
use russh::server::{self, Auth, Msg, Session};
use russh::{Channel, ChannelId, MethodKind, MethodSet};
use tokio::net::TcpListener;
use tokio::task::{JoinHandle, JoinSet};

#[derive(Default)]
pub struct Counts {
    pub forward_target: RwLock<Option<SocketAddr>>,
    pub forward_requests: RwLock<Vec<(String, u32)>>,
    pub reject_forward: AtomicBool,
    pub completed_forwards: AtomicUsize,
    pub authentication_factors: RwLock<Vec<MethodKind>>,
    pub factor_passwords: RwLock<HashMap<usize, String>>,
    pub factor_keys: RwLock<HashMap<usize, Fingerprint>>,
    pub received_passwords: RwLock<Vec<String>>,
    pub received_keys: RwLock<Vec<Fingerprint>>,
    pub advance_rejected_password: AtomicBool,
    pub reject_password: AtomicBool,
    pub require_keyboard_interactive: AtomicBool,
    pub require_password_after_interactive: AtomicBool,
    pub unavailable_keyboard_interactive: AtomicBool,
    pub keyboard_interactive_starts: AtomicUsize,
    pub keyboard_interactive_response_rounds: AtomicUsize,
    pub accept_gate: RwLock<Option<Arc<tokio::sync::Notify>>>,
    pub auth_gate: RwLock<Option<Arc<tokio::sync::Notify>>>,
    pub auth_rejection_time: RwLock<Option<std::time::Duration>>,
    pub connections: AtomicUsize,
    pub closed_connections: AtomicUsize,
    pub connection_closed: tokio::sync::Notify,
    pub passwords: AtomicUsize,
    pub public_keys: AtomicUsize,
    pub public_key_offers: AtomicUsize,
    pub trusted_user_ca: RwLock<Option<Fingerprint>>,
    pub user_certificates: AtomicUsize,
    pub commands: AtomicUsize,
    pub closed_channels: AtomicUsize,
    pub reject_shell: AtomicBool,
    pub pty_size: RwLock<Option<(u32, u32)>>,
    pub files: Arc<RwLock<HashMap<String, Vec<u8>>>>,
    pub stage_created: tokio::sync::Notify,
    pub stage_open_gate: RwLock<Option<Arc<tokio::sync::Notify>>>,
    pub directories: Arc<RwLock<std::collections::HashSet<String>>>,
    pub directory_entries: RwLock<HashMap<String, Vec<russh_sftp::protocol::File>>>,
    pub directory_reads: AtomicUsize,
    pub directory_closes: AtomicUsize,
    pub directory_read_started: tokio::sync::Notify,
    pub directory_read_gate: RwLock<Option<Arc<tokio::sync::Notify>>>,
    pub file_attributes: RwLock<HashMap<String, russh_sftp::protocol::FileAttributes>>,
    pub canonical_paths: RwLock<HashMap<String, String>>,
    pub repeat_directory_page: AtomicBool,
    pub reject_directory_close: AtomicBool,
    pub reject_publish_paths: RwLock<std::collections::HashSet<String>>,
    pub directory_renames: RwLock<Vec<(String, String)>>,
    pub directory_rename_started: tokio::sync::Notify,
    pub directory_rename_gate: RwLock<Option<Arc<tokio::sync::Notify>>>,
    pub directory_create_started: tokio::sync::Notify,
    pub directory_create_gate: RwLock<Option<Arc<tokio::sync::Notify>>>,
    pub reject_directory_publish: AtomicBool,
    pub reject_directory_restore: AtomicBool,
    pub reject_directory_cleanup: AtomicBool,
}

struct Handler {
    counts: Arc<Counts>,
    channels: HashMap<ChannelId, Channel<Msg>>,
    forward_channels: std::collections::HashSet<ChannelId>,
    tasks: JoinSet<()>,
    interactive_verified: bool,
    factor_index: usize,
    interactive_round: usize,
}

impl Handler {
    fn remaining_factor(&self, accepted: bool) -> Option<Auth> {
        let factors = self.counts.authentication_factors.read().unwrap();
        if factors.is_empty() {
            return None;
        }
        Some(match factors.get(self.factor_index) {
            Some(method) => Auth::Reject {
                proceed_with_methods: Some(MethodSet::from([*method].as_slice())),
                partial_success: accepted,
            },
            None => Auth::Accept,
        })
    }

    fn accept_factor(&mut self, method: MethodKind) -> Option<Auth> {
        let expected = self
            .counts
            .authentication_factors
            .read()
            .unwrap()
            .get(self.factor_index)
            .copied();
        if expected == Some(method) {
            self.factor_index += 1;
        }
        self.remaining_factor(expected == Some(method))
    }
}

impl server::Handler for Handler {
    type Error = russh::Error;

    async fn auth_none(&mut self, _: &str) -> Result<Auth, Self::Error> {
        if let Some(result) = self.remaining_factor(false) {
            return Ok(result);
        }
        let mut methods = if self
            .counts
            .require_password_after_interactive
            .load(Ordering::SeqCst)
        {
            vec![MethodKind::KeyboardInteractive]
        } else {
            vec![MethodKind::Password, MethodKind::PublicKey]
        };
        if self
            .counts
            .unavailable_keyboard_interactive
            .load(Ordering::SeqCst)
        {
            methods.push(MethodKind::KeyboardInteractive);
        }
        Ok(Auth::Reject {
            proceed_with_methods: Some(MethodSet::from(methods.as_slice())),
            partial_success: false,
        })
    }

    async fn auth_password(&mut self, _: &str, password: &str) -> Result<Auth, Self::Error> {
        let gate = self.counts.auth_gate.read().unwrap().clone();
        self.counts.passwords.fetch_add(1, Ordering::SeqCst);
        self.counts
            .received_passwords
            .write()
            .unwrap()
            .push(password.into());
        if let Some(gate) = gate {
            gate.notified().await;
        }
        if self
            .counts
            .factor_passwords
            .read()
            .unwrap()
            .get(&self.factor_index)
            .is_some_and(|expected| expected != password)
        {
            if self
                .counts
                .advance_rejected_password
                .swap(false, Ordering::SeqCst)
            {
                self.factor_index += 1;
            }
            return Ok(self.remaining_factor(false).unwrap_or_else(Auth::reject));
        }
        if let Some(result) = self.accept_factor(MethodKind::Password) {
            return Ok(result);
        }
        if self
            .counts
            .require_password_after_interactive
            .load(Ordering::SeqCst)
        {
            return Ok(if self.interactive_verified {
                Auth::Accept
            } else {
                Auth::Reject {
                    proceed_with_methods: Some(MethodSet::from(
                        [MethodKind::KeyboardInteractive].as_slice(),
                    )),
                    partial_success: false,
                }
            });
        }
        if self
            .counts
            .require_keyboard_interactive
            .load(Ordering::SeqCst)
        {
            return Ok(Auth::Reject {
                proceed_with_methods: Some(MethodSet::from(
                    [MethodKind::KeyboardInteractive].as_slice(),
                )),
                partial_success: true,
            });
        }
        Ok(if self.counts.reject_password.load(Ordering::SeqCst) {
            Auth::reject()
        } else {
            Auth::Accept
        })
    }

    async fn auth_keyboard_interactive<'a>(
        &'a mut self,
        _: &str,
        _: &str,
        response: Option<server::Response<'a>>,
    ) -> Result<Auth, Self::Error> {
        let expected = self
            .counts
            .authentication_factors
            .read()
            .unwrap()
            .get(self.factor_index)
            .copied();
        if expected.is_some_and(|method| method != MethodKind::KeyboardInteractive) {
            return Ok(self.remaining_factor(false).unwrap());
        }
        let Some(response) = response else {
            self.counts
                .keyboard_interactive_starts
                .fetch_add(1, Ordering::SeqCst);
            if self
                .counts
                .unavailable_keyboard_interactive
                .load(Ordering::SeqCst)
            {
                return Ok(Auth::Reject {
                    proceed_with_methods: Some(MethodSet::from(
                        [MethodKind::KeyboardInteractive, MethodKind::Password].as_slice(),
                    )),
                    partial_success: false,
                });
            }
            return Ok(Auth::Partial {
                name: "Fixture verification".into(),
                instructions: "Complete both verification steps".into(),
                prompts: vec![
                    ("Verification code".into(), false),
                    ("Device label".into(), true),
                ]
                .into(),
            });
        };
        let answers = response.map(|value| value.to_vec()).collect::<Vec<_>>();
        self.counts
            .keyboard_interactive_response_rounds
            .fetch_add(1, Ordering::SeqCst);
        let round = self.interactive_round;
        self.interactive_round += 1;
        match round {
            0 if answers == [b"482901".to_vec(), b"terminal".to_vec()] => Ok(Auth::Partial {
                name: "Fixture verification".into(),
                instructions: "Enter the second factor".into(),
                prompts: vec![("One-time password".into(), false)].into(),
            }),
            1 if answers == [b"731604".to_vec()] => {
                self.interactive_verified = true;
                self.interactive_round = 0;
                if let Some(result) = self.accept_factor(MethodKind::KeyboardInteractive) {
                    return Ok(result);
                }
                Ok(
                    if self
                        .counts
                        .require_password_after_interactive
                        .load(Ordering::SeqCst)
                    {
                        Auth::Reject {
                            proceed_with_methods: Some(MethodSet::from(
                                [MethodKind::Password].as_slice(),
                            )),
                            partial_success: true,
                        }
                    } else {
                        Auth::Accept
                    },
                )
            }
            _ => Ok(self.remaining_factor(false).unwrap_or_else(Auth::reject)),
        }
    }

    async fn auth_publickey(
        &mut self,
        _: &str,
        key: server::VerifiedPublicKey<'_>,
    ) -> Result<Auth, Self::Error> {
        self.counts.public_keys.fetch_add(1, Ordering::SeqCst);
        let fingerprint = key.public_key.fingerprint(Default::default());
        self.counts
            .received_keys
            .write()
            .unwrap()
            .push(fingerprint.clone());
        if self
            .counts
            .factor_keys
            .read()
            .unwrap()
            .get(&self.factor_index)
            .is_some_and(|expected| expected != &fingerprint)
        {
            return Ok(self.remaining_factor(false).unwrap_or_else(Auth::reject));
        }
        if let Some(result) = self.accept_factor(MethodKind::PublicKey) {
            return Ok(result);
        }
        Ok(if self.counts.trusted_user_ca.read().unwrap().is_some() {
            Auth::reject()
        } else if self
            .counts
            .require_keyboard_interactive
            .load(Ordering::SeqCst)
        {
            Auth::Reject {
                proceed_with_methods: Some(MethodSet::from(
                    [MethodKind::KeyboardInteractive].as_slice(),
                )),
                partial_success: true,
            }
        } else {
            Auth::Accept
        })
    }

    async fn auth_openssh_certificate(
        &mut self,
        user: &str,
        certificate: &Certificate,
    ) -> Result<Auth, Self::Error> {
        self.counts.user_certificates.fetch_add(1, Ordering::SeqCst);
        let ca = self.counts.trusted_user_ca.read().unwrap();
        Ok(
            if certificate.validate(ca.iter()).is_ok()
                && certificate.valid_principals().iter().any(|p| p == user)
            {
                Auth::Accept
            } else {
                Auth::reject()
            },
        )
    }

    async fn auth_publickey_offered(
        &mut self,
        _: &str,
        _: server::PublicKeyOffer<'_>,
    ) -> Result<Auth, Self::Error> {
        self.counts.public_key_offers.fetch_add(1, Ordering::SeqCst);
        Ok(Auth::Accept)
    }

    async fn channel_open_session(
        &mut self,
        channel: Channel<Msg>,
        reply: server::ChannelOpenHandle,
        _: &mut Session,
    ) -> Result<(), Self::Error> {
        self.channels.insert(channel.id(), channel);
        reply.accept().await;
        Ok(())
    }

    async fn channel_open_direct_tcpip(
        &mut self,
        channel: Channel<Msg>,
        host: &str,
        port: u32,
        _: &str,
        _: u32,
        reply: server::ChannelOpenHandle,
        _: &mut Session,
    ) -> Result<(), Self::Error> {
        self.counts
            .forward_requests
            .write()
            .unwrap()
            .push((host.into(), port));
        let target = *self.counts.forward_target.read().unwrap();
        let Some(target) = target.filter(|_| !self.counts.reject_forward.load(Ordering::SeqCst))
        else {
            reply
                .reject(russh::ChannelOpenFailure::AdministrativelyProhibited)
                .await;
            return Ok(());
        };
        reply.accept().await;
        let counts = self.counts.clone();
        self.forward_channels.insert(channel.id());
        self.tasks.spawn(async move {
            let mut stream = channel.into_stream();
            if let Ok(mut upstream) = tokio::net::TcpStream::connect(target).await {
                let _ = tokio::io::copy_bidirectional(&mut stream, &mut upstream).await;
            }
            counts.completed_forwards.fetch_add(1, Ordering::SeqCst);
        });
        Ok(())
    }

    async fn channel_close(
        &mut self,
        channel: ChannelId,
        _: &mut Session,
    ) -> Result<(), Self::Error> {
        self.channels.remove(&channel);
        self.forward_channels.remove(&channel);
        self.counts.closed_channels.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    async fn pty_request(
        &mut self,
        channel: ChannelId,
        _: &str,
        cols: u32,
        rows: u32,
        _: u32,
        _: u32,
        _: &[(russh::Pty, u32)],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        *self.counts.pty_size.write().unwrap() = Some((cols, rows));
        session.channel_success(channel)?;
        Ok(())
    }

    async fn shell_request(
        &mut self,
        channel: ChannelId,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        if self.counts.reject_shell.load(Ordering::SeqCst) {
            session.channel_failure(channel)?;
            return Ok(());
        }
        session.channel_success(channel)?;
        session.data(channel, b"fixture$ ".as_slice())?;
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
        if self.forward_channels.contains(&channel) {
            return Ok(());
        }
        if data == b"fixture-exit" {
            session.data(channel, "尾部".as_bytes())?;
            session.exit_status_request(channel, 0)?;
            session.eof(channel)?;
            session.close(channel)?;
            return Ok(());
        }
        session.data(channel, data.to_vec())?;
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
        let counts = self.counts.clone();
        self.tasks.spawn(async move {
            russh_sftp::server::run(channel.into_stream(), sftp::MemoryFiles::new(counts)).await;
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
        session.data(channel, output.into_bytes())?;
        if command == b"fixture-nonzero" {
            session.extended_data(channel, 1, b"fixture error".as_slice())?;
        }
        if command != b"fixture-missing-status" {
            session
                .exit_status_request(channel, if command == b"fixture-nonzero" { 7 } else { 0 })?;
        }
        if command == b"fixture-late-output" {
            session.data(channel, "尾部\n".as_bytes())?;
            session.extended_data(channel, 1, b"late stderr".as_slice())?;
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
                        let mut config = server::Config {
                            keys: vec![server_key.read().expect("fixture key").clone()],
                            inactivity_timeout: None,
                            // Method discovery is not a rejected credential. Keep the real
                            // rejection delay for subsequent password/key/MFA failures.
                            auth_rejection_time_initial: Some(std::time::Duration::ZERO),
                            ..Default::default()
                        };
                        if let Some(delay) = *server_counts.auth_rejection_time.read().unwrap() {
                            config.auth_rejection_time = delay;
                        }
                        let config = Arc::new(config);
                        let handler = Handler {
                            counts: server_counts.clone(),
                            channels: HashMap::new(),
                            forward_channels: Default::default(),
                            tasks: JoinSet::new(),
                            interactive_verified: false,
                            factor_index: 0,
                            interactive_round: 0,
                        };
                        let server_counts_for_close = server_counts.clone();
                        sessions.spawn(async move {
                            if let Ok(session) = server::run_stream(config, stream, handler).await {
                                let _ = session.await;
                            }
                            server_counts_for_close.closed_connections.fetch_add(1, Ordering::SeqCst);
                            server_counts_for_close.connection_closed.notify_one();
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
    PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).expect("generate temporary key")
}
mod sftp;

use std::collections::HashMap;
