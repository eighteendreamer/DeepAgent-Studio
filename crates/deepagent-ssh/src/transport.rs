//! One native SSH transport for exec, PTY and SFTP channels.
//!
//! Host identity is checked against the pre-handshake snapshot; its source is
//! revalidated before any credentials are offered, not just after authentication.

use crate::host_trust::HostTrust;
use crate::{SshAuthType, SshConnectionConfig, SshError, SshHostKeyStore, SshResult};
use russh::client::{self, Handle, Msg};
use russh::keys::{self, HashAlg, PrivateKeyWithHashAlg, PublicKey};
use russh::{Channel, ChannelMsg, Disconnect};
use std::sync::{Arc, Mutex};

struct IdentityHandler {
    trust: HostTrust,
    rejected: Arc<Mutex<Option<PublicKey>>>,
}

impl client::Handler for IdentityHandler {
    type Error = russh::Error;

    async fn check_server_key(&mut self, presented: &PublicKey) -> Result<bool, Self::Error> {
        let accepted = self.trust.accepts(presented);
        if !accepted {
            *self
                .rejected
                .lock()
                .unwrap_or_else(|error| error.into_inner()) = Some(presented.clone());
        }
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
    pub(crate) async fn connect(
        config: &SshConnectionConfig,
        trust: &HostTrust,
        store: Option<Arc<SshHostKeyStore>>,
    ) -> SshResult<Self> {
        let rejected = Arc::new(Mutex::new(None));
        let handler = IdentityHandler {
            trust: trust.clone(),
            rejected: rejected.clone(),
        };
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

        let authentication = async {
            if &HostTrust::load_with_store(config, store).await? != trust {
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
        SshAuthType::Agent => return authenticate_agent(handle, &config.username).await,
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

#[cfg(unix)]
async fn authenticate_agent(handle: &mut Handle<IdentityHandler>, username: &str) -> SshResult<()> {
    let failure = |error: String| SshError::AuthFailed(username.to_owned(), error);
    let mut agent = keys::agent::client::AgentClient::connect_env()
        .await
        .map_err(|error| failure(format!("connect SSH Agent: {error}")))?;
    let identities = agent
        .request_identities()
        .await
        .map_err(|error| failure(format!("list SSH Agent identities: {error}")))?;
    if identities.is_empty() {
        return Err(failure("SSH Agent has no identities".into()));
    }
    let hash = handle
        .best_supported_rsa_hash()
        .await
        .map_err(|error| protocol_error("SSH Agent RSA negotiation", error))?
        .flatten();
    let mut reason = "server rejected SSH Agent identities".to_owned();
    for identity in identities {
        match handle
            .authenticate_publickey_with(username, identity, hash, &mut agent)
            .await
        {
            Ok(result) if result.success() => return Ok(()),
            Ok(_) => {}
            Err(error) => reason = format!("SSH Agent authentication: {error}"),
        }
    }
    Err(failure(reason))
}

#[cfg(not(unix))]
async fn authenticate_agent(_: &mut Handle<IdentityHandler>, _: &str) -> SshResult<()> {
    Err(SshError::InvalidConfig(
        "SSH Agent is not enabled on this platform; use password or key file authentication".into(),
    ))
}
