//! One native SSH transport for exec, PTY and SFTP channels.
//!
//! Host identity is checked against the pre-handshake snapshot; its source is
//! revalidated before any credentials are offered, not just after authentication.

use crate::authentication_challenge::{SshAuthenticationKind, SshAuthenticationPrompt};
use crate::host_trust::HostTrust;
use crate::transport_authentication::AuthenticationNegotiation;
use crate::{SshConnectionConfig, SshError, SshHostKeyStore, SshResult};
use russh::client::{self, Handle, Msg};
use russh::keys::{HashAlg, PublicKey, PublicKeyOrCertificate};
use russh::{Channel, ChannelMsg, Disconnect};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_util::sync::CancellationToken;

trait SshStream: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> SshStream for T {}

struct VerifiedJump {
    client: SshClient,
    trust: HostTrust,
}

enum SshRouteConnection {
    Stream {
        stream: Box<dyn SshStream>,
        jump: Option<Arc<VerifiedJump>>,
    },
    AuthenticationRequired {
        pending: PendingClientAuthentication,
        round: SshAuthenticationRound,
    },
}

impl Drop for VerifiedJump {
    fn drop(&mut self) {
        // A dropped/expired downstream handshake must also release its dedicated hop.
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            let client = self.client.clone();
            runtime.spawn(async move {
                let _ = client.disconnect().await;
            });
        }
    }
}

pub(crate) struct IdentityHandler {
    // None is a probe that unconditionally rejects every key before authentication.
    trust: Option<HostTrust>,
    rejected: Arc<Mutex<Option<PublicKey>>>,
    accepted: Arc<Mutex<Option<PublicKey>>>,
    closed_by_timer: Arc<Mutex<Option<&'static str>>>,
}

impl IdentityHandler {
    pub(crate) fn verified(trust: HostTrust) -> Self {
        Self {
            trust: Some(trust),
            rejected: Arc::new(Mutex::new(None)),
            accepted: Arc::new(Mutex::new(None)),
            closed_by_timer: Arc::new(Mutex::new(None)),
        }
    }
}

impl client::Handler for IdentityHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        presented: &PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        let presented_key = presented.public_key();
        let accepted = self.trust.as_ref().is_some_and(|trust| {
            presented.certificate().is_none() && trust.accepts(&presented_key)
        });
        let observation = if accepted {
            &self.accepted
        } else {
            &self.rejected
        };
        *observation
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(presented_key);
        Ok(accepted)
    }

    async fn disconnected(
        &mut self,
        reason: client::DisconnectReason<Self::Error>,
    ) -> Result<(), Self::Error> {
        match reason {
            client::DisconnectReason::ReceivedDisconnect(_) => Ok(()),
            client::DisconnectReason::Error(error) => {
                let message = match error {
                    russh::Error::InactivityTimeout => Some("SSH inactivity timeout"),
                    russh::Error::KeepaliveTimeout => Some("SSH keepalive response limit exceeded"),
                    _ => None,
                };
                *self
                    .closed_by_timer
                    .lock()
                    .unwrap_or_else(|error| error.into_inner()) = message;
                Err(error)
            }
        }
    }
}

/// A verified, authenticated native connection. Clones share its channel owner.
/// No authentication configuration or secrets are retained in this handle.
#[derive(Clone)]
pub struct SshClient {
    handle: Arc<Handle<IdentityHandler>>,
    jump: Option<Arc<VerifiedJump>>,
    closed_by_timer: Arc<Mutex<Option<&'static str>>>,
}

pub(crate) struct SshAuthenticationRound {
    pub kind: SshAuthenticationKind,
    pub name: String,
    pub instructions: String,
    pub prompts: Vec<SshAuthenticationPrompt>,
}

pub(crate) enum SshClientConnection {
    ConfirmationRequired {
        pending: PendingHostVerification,
    },
    Connected {
        client: SshClient,
        trust: HostTrust,
        server_key: PublicKey,
    },
    AuthenticationRequired {
        pending: PendingClientAuthentication,
        round: SshAuthenticationRound,
    },
}

/// Holds only the authenticated route while the target key awaits a decision.
/// A fresh target channel is pinned to the reviewed key; the jump is not logged in again.
pub(crate) struct PendingHostVerification {
    pub trust: HostTrust,
    pub key: PublicKey,
    jump: Option<Arc<VerifiedJump>>,
}

impl PendingHostVerification {
    pub(crate) async fn resume(
        self,
        config: &SshConnectionConfig,
        trust: HostTrust,
        store: Option<Arc<SshHostKeyStore>>,
    ) -> SshResult<SshClientConnection> {
        if let Some(jump) = &self.jump {
            verify_jump(jump, config, store.clone()).await?;
        }
        if trust.reload(config, store.clone()).await? != trust {
            return Err(HostTrust::error(
                config,
                "target trust changed before confirmation resumed",
            ));
        }
        let stream = match &self.jump {
            Some(jump) => SshClient::forward_stream(config, jump).await?,
            None => Box::new(crate::proxy::open_tcp(config).await?) as Box<dyn SshStream>,
        };
        SshClient::authenticate_stream(config, &trust, store, stream, self.jump).await
    }
}

pub(crate) struct PendingClientAuthentication {
    pub role: crate::SshConnectionRole,
    target_trust: Option<HostTrust>,
    handle: Option<Handle<IdentityHandler>>,
    pub trust: HostTrust,
    pub server_key: PublicKey,
    negotiation: AuthenticationNegotiation,
    jump: Option<Arc<VerifiedJump>>,
    closed_by_timer: Arc<Mutex<Option<&'static str>>>,
}

pub(crate) enum AuthenticationStep {
    ConfirmationRequired(PendingHostVerification),
    Connected {
        client: SshClient,
        trust: HostTrust,
        server_key: PublicKey,
    },
    Continue(SshAuthenticationRound),
}

impl PendingClientAuthentication {
    pub(crate) async fn respond(
        &mut self,
        cancelled: &CancellationToken,
        config: &SshConnectionConfig,
        store: Option<Arc<SshHostKeyStore>>,
        responses: Vec<String>,
    ) -> SshResult<AuthenticationStep> {
        self.verify_jump_trust(config, store.clone()).await?;
        let active = self.role.config(config)?;
        let handle = self.handle.as_mut().ok_or_else(|| {
            SshError::AuthFailed(
                config.username.clone(),
                "authentication session is closed".into(),
            )
        })?;
        let advance =
            self.negotiation
                .respond(handle, &active, &self.trust, store.clone(), responses);
        let round = tokio::select! {
            biased;
            _ = cancelled.cancelled() => {
                return Err(SshError::AuthFailed(
                    config.username.clone(),
                    "SSH authentication challenge was cancelled or expired".into(),
                ));
            }
            result = advance => result?,
        };
        if let Some(round) = round {
            return Ok(AuthenticationStep::Continue(round));
        }
        let handle = self.handle.take().ok_or_else(|| {
            SshError::AuthFailed(
                config.username.clone(),
                "authentication session is closed".into(),
            )
        })?;
        if self.role == crate::SshConnectionRole::Jump {
            let target_trust = self.target_trust.take().ok_or_else(|| {
                SshError::InvalidConfig("missing target trust for jump continuation".into())
            })?;
            let jump = Arc::new(VerifiedJump {
                client: SshClient {
                    handle: Arc::new(handle),
                    jump: None,
                    closed_by_timer: self.closed_by_timer.clone(),
                },
                trust: self.trust.clone(),
            });
            let continuation = async {
                verify_jump(&jump, config, store.clone()).await?;
                if target_trust.reload(config, store.clone()).await? != target_trust {
                    return Err(HostTrust::error(
                        config,
                        "target trust changed during jump authentication",
                    ));
                }
                let stream = SshClient::forward_stream(config, &jump).await?;
                SshClient::authenticate_stream(
                    config,
                    &target_trust,
                    store,
                    stream,
                    Some(jump.clone()),
                )
                .await
            };
            let progress = tokio::select! {
                biased;
                _ = cancelled.cancelled() => { return Err(SshError::AuthFailed(active.username, "SSH authentication challenge was cancelled or expired".into())); }
                result = continuation => result?,
            };
            return match progress {
                SshClientConnection::ConfirmationRequired { pending } => {
                    Ok(AuthenticationStep::ConfirmationRequired(pending))
                }
                SshClientConnection::Connected {
                    client,
                    trust,
                    server_key,
                } => Ok(AuthenticationStep::Connected {
                    client,
                    trust,
                    server_key,
                }),
                SshClientConnection::AuthenticationRequired { pending, round } => {
                    *self = pending;
                    Ok(AuthenticationStep::Continue(round))
                }
            };
        }
        Ok(AuthenticationStep::Connected {
            client: SshClient {
                handle: Arc::new(handle),
                jump: self.jump.take(),
                closed_by_timer: self.closed_by_timer.clone(),
            },
            trust: self.trust.clone(),
            server_key: self.server_key.clone(),
        })
    }

    pub(crate) async fn verify_jump_trust(
        &self,
        config: &SshConnectionConfig,
        store: Option<Arc<SshHostKeyStore>>,
    ) -> SshResult<()> {
        if let Some(jump) = &self.jump {
            verify_jump(jump, config, store).await?;
        }
        Ok(())
    }

    pub(crate) async fn disconnect(&mut self) {
        if let Some(handle) = self.handle.take() {
            let _ = handle.disconnect(Disconnect::ByApplication, "", "en").await;
        }
        if let Some(jump) = self.jump.take() {
            let _ = jump.client.disconnect().await;
        }
    }
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
        store: Option<Arc<SshHostKeyStore>>,
    ) -> SshResult<PublicKey> {
        if !trust.is_unknown() {
            return Err(HostTrust::error(
                config,
                "only an unknown endpoint may be probed for confirmation",
            ));
        }
        Self::observe_host_key(config, store).await
    }

    /// Management probe: also rejects unchanged/known keys, never offers credentials.
    pub(crate) async fn observe_host_key(
        config: &SshConnectionConfig,
        store: Option<Arc<SshHostKeyStore>>,
    ) -> SshResult<PublicKey> {
        let (stream, _jump) = match Box::pin(Self::open_route(config, store, None)).await? {
            SshRouteConnection::Stream { stream, jump } => (stream, jump),
            SshRouteConnection::AuthenticationRequired { mut pending, .. } => {
                pending.disconnect().await;
                return Err(jump_error(
                    "authentication",
                    SshError::AuthFailed(
                        config.username.clone(),
                        "host-key probe requires an authenticated jump".into(),
                    ),
                ));
            }
        };
        Self::observe_stream(config, stream).await
    }

    async fn observe_stream(
        config: &SshConnectionConfig,
        stream: Box<dyn SshStream>,
    ) -> SshResult<PublicKey> {
        let observed = Arc::new(Mutex::new(None));
        let result = client::connect_stream(
            Arc::new(config.timing.protocol_config()),
            stream,
            IdentityHandler {
                trust: None,
                rejected: observed.clone(),
                accepted: Arc::new(Mutex::new(None)),
                closed_by_timer: Arc::new(Mutex::new(None)),
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
        confirmed_jump: Option<HostTrust>,
    ) -> SshResult<SshClientConnection> {
        match Self::open_route(config, store.clone(), confirmed_jump).await? {
            SshRouteConnection::Stream { stream, jump } => {
                Self::authenticate_stream(config, trust, store, stream, jump).await
            }
            SshRouteConnection::AuthenticationRequired { mut pending, round } => {
                pending.target_trust = Some(trust.clone());
                Ok(SshClientConnection::AuthenticationRequired { pending, round })
            }
        }
    }

    async fn open_route(
        config: &SshConnectionConfig,
        store: Option<Arc<SshHostKeyStore>>,
        confirmed_jump: Option<HostTrust>,
    ) -> SshResult<SshRouteConnection> {
        let Some(jump) = &config.jump else {
            return Ok(SshRouteConnection::Stream {
                stream: Box::new(crate::proxy::open_tcp(config).await?),
                jump: None,
            });
        };
        let hop = jump.config(config);
        let trust = match confirmed_jump {
            Some(trust) => {
                if trust.reload(&hop, store.clone()).await? != trust {
                    return Err(jump_error(
                        "host verification",
                        HostTrust::error(&hop, "confirmed jump trust changed"),
                    ));
                }
                trust
            }
            None => crate::connection_flow::connection_trust(&hop, store.clone())
                .await
                .map_err(|error| jump_error("host verification", error))?,
        };
        let client = match Box::pin(Self::connect(&hop, &trust, store.clone(), None))
            .await
            .map_err(|error| jump_error("authentication", error))?
        {
            SshClientConnection::ConfirmationRequired { .. } => {
                return Err(jump_error(
                    "host verification",
                    HostTrust::error(&hop, "jump requires explicit host verification"),
                ));
            }
            SshClientConnection::Connected { client, .. } => client,
            SshClientConnection::AuthenticationRequired { mut pending, round } => {
                pending.role = crate::SshConnectionRole::Jump;
                return Ok(SshRouteConnection::AuthenticationRequired { pending, round });
            }
        };
        let jump = Arc::new(VerifiedJump { client, trust });
        if jump.trust.reload(&hop, store).await? != jump.trust {
            return Err(jump_error(
                "host verification",
                HostTrust::error(&hop, "jump trust changed during authentication"),
            ));
        }
        let stream = Self::forward_stream(config, &jump).await?;
        Ok(SshRouteConnection::Stream {
            stream,
            jump: Some(jump),
        })
    }

    async fn forward_stream(
        config: &SshConnectionConfig,
        jump: &VerifiedJump,
    ) -> SshResult<Box<dyn SshStream>> {
        let channel = jump
            .client
            .handle
            .channel_open_direct_tcpip(&config.host, u32::from(config.port), "127.0.0.1", 0)
            .await
            .map_err(|error| jump_error("forwarding", protocol_error("direct-tcpip", error)))?;
        Ok(Box::new(channel.into_stream()))
    }

    async fn authenticate_stream(
        config: &SshConnectionConfig,
        trust: &HostTrust,
        store: Option<Arc<SshHostKeyStore>>,
        stream: Box<dyn SshStream>,
        jump: Option<Arc<VerifiedJump>>,
    ) -> SshResult<SshClientConnection> {
        if trust.needs_confirmation() {
            let key = Self::observe_stream(config, stream).await?;
            if trust.reload(config, store.clone()).await? != *trust {
                return Err(HostTrust::error(
                    config,
                    "target trust changed during host-key probe",
                ));
            }
            if let Some(jump) = &jump {
                verify_jump(jump, config, store.clone()).await?;
            }
            let pending = PendingHostVerification {
                trust: trust.clone(),
                key: key.clone(),
                jump,
            };
            if config.host_key_policy == crate::SshHostKeyPolicy::AcceptNew {
                let writer = store.clone().ok_or_else(|| {
                    HostTrust::error(config, "AcceptNew requires the application trust store")
                })?;
                let trusted = trust.clone().persist_unknown(config, writer, key).await?;
                return Box::pin(pending.resume(config, trusted, store)).await;
            }
            return Ok(SshClientConnection::ConfirmationRequired { pending });
        }
        let handler = IdentityHandler::verified(trust.clone());
        let rejected = handler.rejected.clone();
        let accepted = handler.accepted.clone();
        let closed_by_timer = handler.closed_by_timer.clone();
        let mut handle =
            client::connect_stream(Arc::new(config.timing.protocol_config()), stream, handler)
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
        let mut negotiation = AuthenticationNegotiation::default();
        if let Some(jump) = &jump {
            verify_jump(jump, config, store.clone()).await?;
        }
        let authentication = negotiation.begin(&mut handle, config, trust, store).await;
        match authentication {
            Ok(Some(round)) => Ok(SshClientConnection::AuthenticationRequired {
                pending: PendingClientAuthentication {
                    role: crate::SshConnectionRole::Target,
                    target_trust: None,
                    handle: Some(handle),
                    trust: trust.clone(),
                    server_key: key,
                    negotiation,
                    jump,
                    closed_by_timer,
                },
                round,
            }),
            Ok(None) => Ok(SshClientConnection::Connected {
                client: Self {
                    handle: Arc::new(handle),
                    jump,
                    closed_by_timer,
                },
                trust: trust.clone(),
                server_key: key,
            }),
            Err(error) => {
                let _ = handle.disconnect(Disconnect::ByApplication, "", "en").await;
                Err(error)
            }
        }
    }

    pub fn is_closed(&self) -> bool {
        self.handle.is_closed()
            || self
                .jump
                .as_ref()
                .is_some_and(|jump| jump.client.is_closed())
    }

    pub(crate) fn timing_error(&self) -> Option<String> {
        self.closed_by_timer
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .map(str::to_owned)
            .or_else(|| {
                self.jump
                    .as_ref()
                    .and_then(|jump| jump.client.timing_error())
            })
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
        let result = if self.handle.is_closed() {
            Ok(())
        } else {
            self.handle
                .disconnect(Disconnect::ByApplication, "", "en")
                .await
                .map_err(|error| protocol_error("SSH disconnect", error))
        };
        if let Some(jump) = &self.jump {
            Box::pin(jump.client.disconnect()).await?;
        }
        result
    }

    pub(crate) async fn verify_jump_trust(
        &self,
        config: &SshConnectionConfig,
        store: Option<Arc<SshHostKeyStore>>,
    ) -> SshResult<()> {
        if let Some(jump) = &self.jump {
            verify_jump(jump, config, store).await?;
        }
        Ok(())
    }
}

fn jump_error(stage: &'static str, source: SshError) -> SshError {
    SshError::Jump {
        stage,
        source: Box::new(source),
    }
}

async fn verify_jump(
    jump: &VerifiedJump,
    config: &SshConnectionConfig,
    store: Option<Arc<SshHostKeyStore>>,
) -> SshResult<()> {
    let hop = crate::SshConnectionRole::Jump.config(config)?;
    let current = jump
        .trust
        .reload(&hop, store)
        .await
        .map_err(|error| jump_error("host verification", error))?;
    if current != jump.trust {
        return Err(jump_error(
            "host verification",
            HostTrust::error(&hop, "jump trust changed; reconnect after verification"),
        ));
    }
    Ok(())
}

pub(crate) fn protocol_error(context: &str, error: russh::Error) -> SshError {
    SshError::ConnectionLost(format!("{context}: {error}"))
}
