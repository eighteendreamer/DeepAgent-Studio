//! One native SSH transport for exec, PTY and SFTP channels.
//!
//! Host identity is checked against the pre-handshake snapshot; its source is
//! revalidated before any credentials are offered, not just after authentication.

use crate::host_trust::HostTrust;
use crate::{SshAuthType, SshConnectionConfig, SshError, SshHostKeyStore, SshResult};
use deepagent_core::clock::{Clock, SystemClock};
use russh::client::{self, Handle, Msg};
use russh::keys::{self, HashAlg, PrivateKeyWithHashAlg, PublicKey};
use russh::{Channel, ChannelMsg, Disconnect};
use std::sync::{Arc, Mutex};

pub(crate) struct IdentityHandler {
    // None is a probe that unconditionally rejects every key before authentication.
    trust: Option<HostTrust>,
    rejected: Arc<Mutex<Option<PublicKey>>>,
    accepted: Arc<Mutex<Option<PublicKey>>>,
}

impl IdentityHandler {
    pub(crate) fn verified(trust: HostTrust) -> Self {
        Self {
            trust: Some(trust),
            rejected: Arc::new(Mutex::new(None)),
            accepted: Arc::new(Mutex::new(None)),
        }
    }
}

impl client::Handler for IdentityHandler {
    type Error = russh::Error;

    async fn check_server_key(&mut self, presented: &PublicKey) -> Result<bool, Self::Error> {
        let accepted = self
            .trust
            .as_ref()
            .is_some_and(|trust| trust.accepts(presented));
        let observation = if accepted {
            &self.accepted
        } else {
            &self.rejected
        };
        *observation
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(presented.clone());
        Ok(accepted)
    }
}

/// A verified, authenticated native connection. Clones share its channel owner.
/// No authentication configuration or secrets are retained in this handle.
#[derive(Clone)]
pub struct SshClient {
    handle: Arc<Handle<IdentityHandler>>,
}

pub struct SshCommandOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_status: u32,
}

impl SshClient {
    /// Strictly reject the key to end a pre-auth probe. Never authenticates or keeps a client.
    pub(crate) async fn probe_host_key(
        config: &SshConnectionConfig,
        trust: &HostTrust,
    ) -> SshResult<PublicKey> {
        if !trust.is_unknown() {
            return Err(HostTrust::error(
                config,
                "only an unknown endpoint may be probed for confirmation",
            ));
        }
        Self::observe_host_key(config).await
    }

    /// Management probe: also rejects unchanged/known keys, never offers credentials.
    pub(crate) async fn observe_host_key(config: &SshConnectionConfig) -> SshResult<PublicKey> {
        let observed = Arc::new(Mutex::new(None));
        let result = client::connect(
            Arc::new(client::Config::default()),
            (config.host.as_str(), config.port),
            IdentityHandler {
                trust: None,
                rejected: observed.clone(),
                accepted: Arc::new(Mutex::new(None)),
            },
        )
        .await;
        let failure = match result {
            Ok(handle) => {
                let _ = handle.disconnect(Disconnect::ByApplication, "", "en").await;
                return Err(HostTrust::error(
                    config,
                    "host-key probe unexpectedly accepted a key",
                ));
            }
            Err(error) => error,
        };
        let key = observed
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take();
        match key {
            Some(key) => Ok(key),
            None => Err(protocol_error("SSH host-key probe", failure)),
        }
    }
    pub(crate) async fn connect(
        config: &SshConnectionConfig,
        trust: &HostTrust,
        store: Option<Arc<SshHostKeyStore>>,
    ) -> SshResult<Self> {
        let handler = IdentityHandler::verified(trust.clone());
        let rejected = handler.rejected.clone();
        let accepted = handler.accepted.clone();
        let mut handle = client::connect(
            Arc::new(client::Config::default()),
            (config.host.as_str(), config.port),
            handler,
        )
        .await
        .map_err(|error| {
            let rejected = rejected.lock().unwrap_or_else(|error| error.into_inner());
            match rejected.as_ref() {
                Some(key) => HostTrust::error(
                    config,
                    &format!(
                        "server key changed or does not match the verified entry; presented {} {}",
                        key.algorithm().as_str(),
                        key.fingerprint(HashAlg::Sha256)
                    ),
                ),
                None => protocol_error("SSH handshake", error),
            }
        })?;

        let key = accepted
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take()
            .ok_or_else(|| {
                HostTrust::error(config, "handshake did not retain a verified server key")
            })?;
        let authentication = async {
            if &trust.reload(config, store.clone()).await? != trust {
                return Err(HostTrust::error(
                    config,
                    "trust source changed before authentication",
                ));
            }
            authenticate(&mut handle, config).await
        }
        .await;
        if let Err(error) = authentication {
            let _ = handle.disconnect(Disconnect::ByApplication, "", "en").await;
            return Err(error);
        }
        if let Some(store) = store {
            let identity = crate::SshHostIdentity::direct(&config.host, config.port)?;
            let now = SystemClock.now();
            let result = tokio::task::spawn_blocking(move || {
                store.record_authentication(&identity, &key, now)
            })
            .await
            .map_err(|error| SshError::Internal(format!("SSH host activity task: {error}")))
            .and_then(|result| result);
            if let Err(error) = result {
                tracing::warn!(target:"deepagent_ssh", %error, "SSH authenticated but last-used metadata was not saved; trust enforcement is unaffected");
            }
        }
        Ok(Self {
            handle: Arc::new(handle),
        })
    }

    pub fn is_closed(&self) -> bool {
        self.handle.is_closed()
    }

    pub async fn get_channel(&self) -> SshResult<Channel<Msg>> {
        self.handle
            .channel_open_session()
            .await
            .map_err(|error| protocol_error("open SSH channel", error))
    }

    pub async fn execute(&self, command: &str) -> SshResult<SshCommandOutput> {
        let mut channel = self.get_channel().await?;
        if let Err(error) = channel.exec(true, command).await {
            let _ = channel.close().await;
            return Err(protocol_error("SSH exec", error));
        }
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let mut exit_status = None;
        // ExitStatus/EOF are not the end of output: drain until the channel closes.
        while let Some(message) = channel.wait().await {
            match message {
                ChannelMsg::Data { data } => stdout.extend_from_slice(&data),
                ChannelMsg::ExtendedData { data, ext: 1 } => stderr.extend_from_slice(&data),
                ChannelMsg::ExitStatus {
                    exit_status: status,
                } => exit_status = Some(status),
                ChannelMsg::Failure => {
                    channel.close().await.map_err(|error| {
                        protocol_error("close rejected SSH exec channel", error)
                    })?;
                    return Err(SshError::Internal("SSH exec was rejected".into()));
                }
                _ => {}
            }
        }
        Ok(SshCommandOutput {
            stdout: String::from_utf8_lossy(&stdout).into_owned(),
            stderr: String::from_utf8_lossy(&stderr).into_owned(),
            exit_status: exit_status.ok_or_else(|| {
                SshError::Internal("SSH command closed without an exit status".into())
            })?,
        })
    }

    pub async fn disconnect(&self) -> SshResult<()> {
        if self.is_closed() {
            return Ok(());
        }
        self.handle
            .disconnect(Disconnect::ByApplication, "", "en")
            .await
            .map_err(|error| protocol_error("SSH disconnect", error))
    }
}

fn protocol_error(context: &str, error: russh::Error) -> SshError {
    SshError::ConnectionLost(format!("{context}: {error}"))
}

async fn authenticate(
    handle: &mut Handle<IdentityHandler>,
    config: &SshConnectionConfig,
) -> SshResult<()> {
    let result = match config.auth_type {
        SshAuthType::Password => {
            handle
                .authenticate_password(
                    &config.username,
                    config
                        .password
                        .as_deref()
                        .ok_or_else(|| SshError::InvalidConfig("password is required".into()))?,
                )
                .await
        }
        SshAuthType::KeyFile => {
            let path = config
                .key_path
                .clone()
                .ok_or_else(|| SshError::InvalidConfig("key path is required".into()))?;
            let passphrase = config.password.clone();
            let key = tokio::task::spawn_blocking(move || {
                keys::load_secret_key(path, passphrase.as_deref())
            })
            .await
            .map_err(|error| SshError::Internal(format!("SSH identity reader: {error}")))?
            .map_err(|error| {
                SshError::AuthFailed(
                    config.username.clone(),
                    format!("read SSH private key: {error}"),
                )
            })?;
            let hash = handle
                .best_supported_rsa_hash()
                .await
                .map_err(|error| protocol_error("SSH RSA negotiation", error))?
                .flatten();
            handle
                .authenticate_publickey(
                    &config.username,
                    PrivateKeyWithHashAlg::new(Arc::new(key), hash),
                )
                .await
        }
        SshAuthType::Agent | SshAuthType::OpenSshAgent | SshAuthType::Pageant => {
            let provider = match config.auth_type {
                SshAuthType::Pageant => crate::SshAgentProvider::Pageant,
                SshAuthType::OpenSshAgent => crate::SshAgentProvider::OpenSsh,
                _ => crate::SshAgentProvider::Auto,
            };
            return crate::agent::authenticate(handle, &config.username, provider).await;
        }
    }
    .map_err(|error| protocol_error("SSH authentication", error))?;
    if !result.success() {
        return Err(SshError::AuthFailed(
            config.username.clone(),
            "server rejected the configured authentication method".into(),
        ));
    }
    Ok(())
}
