//! Server-directed authentication on one verified SSH connection.
//! Only server success completes authentication; partial success advances a bounded stage.

use crate::authentication_challenge::{SshAuthenticationKind, SshAuthenticationPrompt};
use crate::host_trust::HostTrust;
use crate::transport::{protocol_error, IdentityHandler, SshAuthenticationRound};
use crate::{SshAuthType, SshConnectionConfig, SshError, SshHostKeyStore, SshResult};
use russh::client::{self, Handle};
use russh::keys::PrivateKeyWithHashAlg;
use russh::{MethodKind, MethodSet};
use std::sync::Arc;

const MAX_AUTHENTICATION_STAGES: usize = 16;

#[derive(Default)]
pub(crate) struct AuthenticationNegotiation {
    attempted: Vec<AuthenticationCandidate>,
    satisfied: Vec<usize>,
    current_candidate: Option<AuthenticationCandidate>,
    completed_stages: usize,
    pending_kind: Option<SshAuthenticationKind>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum AuthenticationCandidate {
    Credential(usize),
    KeyboardInteractive,
}

enum AuthenticationReply {
    Result(client::AuthResult),
    Prompt(SshAuthenticationRound),
}

impl AuthenticationNegotiation {
    pub(crate) async fn begin(
        &mut self,
        handle: &mut Handle<IdentityHandler>,
        config: &SshConnectionConfig,
        trust: &HostTrust,
        store: Option<Arc<SshHostKeyStore>>,
    ) -> SshResult<Option<SshAuthenticationRound>> {
        verify_authentication_trust(config, trust, store.clone()).await?;
        let result = handle
            .authenticate_none(config.username.clone())
            .await
            .map_err(|error| protocol_error("SSH authentication method discovery", error))?;
        self.advance(
            handle,
            config,
            trust,
            store,
            AuthenticationReply::Result(result),
        )
        .await
    }

    pub(crate) async fn respond(
        &mut self,
        handle: &mut Handle<IdentityHandler>,
        config: &SshConnectionConfig,
        trust: &HostTrust,
        store: Option<Arc<SshHostKeyStore>>,
        responses: Vec<String>,
    ) -> SshResult<Option<SshAuthenticationRound>> {
        verify_authentication_trust(config, trust, store.clone()).await?;
        let reply = match self.pending_kind.take() {
            Some(SshAuthenticationKind::PrivateKey) => {
                let passphrase = responses.first().ok_or_else(|| {
                    SshError::InvalidConfig("private-key unlock expects one passphrase".into())
                })?;
                let Some(AuthenticationCandidate::Credential(index)) = self.current_candidate
                else {
                    return Err(SshError::InvalidConfig(
                        "private-key unlock has no active identity".into(),
                    ));
                };
                let identity = config.authentication_config(index)?;
                AuthenticationReply::Result(
                    authenticate(handle, &identity, Some(passphrase), trust, store.clone())
                        .await?
                        .ok_or_else(|| {
                            SshError::AuthFailed(
                                config.username.clone(),
                                "private key remains locked".into(),
                            )
                        })?,
                )
            }
            Some(SshAuthenticationKind::KeyboardInteractive) => {
                let response = handle
                    .authenticate_keyboard_interactive_respond(responses)
                    .await
                    .map_err(|error| protocol_error("SSH keyboard-interactive response", error))?;
                // A rejected answer is terminal, unlike an unavailable method before any prompt.
                if let client::KeyboardInteractiveAuthResponse::Failure {
                    ref remaining_methods,
                    partial_success: false,
                } = response
                {
                    return Err(no_method(
                        config,
                        remaining_methods,
                        "SSH keyboard-interactive answer was rejected",
                    ));
                }
                interactive_reply(response)
            }
            None => {
                return Err(SshError::AuthFailed(
                    config.username.clone(),
                    "authentication prompt is no longer active".into(),
                ))
            }
        };
        self.advance(handle, config, trust, store, reply).await
    }

    async fn advance(
        &mut self,
        handle: &mut Handle<IdentityHandler>,
        config: &SshConnectionConfig,
        trust: &HostTrust,
        store: Option<Arc<SshHostKeyStore>>,
        mut reply: AuthenticationReply,
    ) -> SshResult<Option<SshAuthenticationRound>> {
        loop {
            let (remaining_methods, partial_success) = match reply {
                AuthenticationReply::Prompt(round) => {
                    self.pending_kind = Some(round.kind);
                    return Ok(Some(round));
                }
                AuthenticationReply::Result(client::AuthResult::Success) => return Ok(None),
                AuthenticationReply::Result(client::AuthResult::Failure {
                    remaining_methods,
                    partial_success,
                }) => (remaining_methods, partial_success),
            };
            if partial_success {
                if config.auth_type == SshAuthType::Chain {
                    if let Some(AuthenticationCandidate::Credential(index)) = self.current_candidate
                    {
                        self.satisfied.push(index);
                    }
                }
                self.completed_stages += 1;
                if self.completed_stages >= MAX_AUTHENTICATION_STAGES {
                    return Err(SshError::AuthFailed(
                        config.username.clone(),
                        "SSH authentication stage limit (16) exceeded".into(),
                    ));
                }
                self.attempted.clear();
            }
            let candidate = self
                .pick_candidate(config, &remaining_methods)
                .ok_or_else(|| {
                    no_method(
                        config,
                        &remaining_methods,
                        "no untried configured SSH authentication method is offered",
                    )
                })?;
            verify_authentication_trust(config, trust, store.clone()).await?;
            self.attempted.push(candidate);
            self.current_candidate = Some(candidate);
            reply = if candidate == AuthenticationCandidate::KeyboardInteractive {
                interactive_reply(
                    handle
                        .authenticate_keyboard_interactive_start(
                            config.username.clone(),
                            None::<String>,
                        )
                        .await
                        .map_err(|error| {
                            protocol_error("start SSH keyboard-interactive authentication", error)
                        })?,
                )
            } else {
                let AuthenticationCandidate::Credential(index) = candidate else {
                    unreachable!()
                };
                let identity = config.authentication_config(index)?;
                match authenticate(handle, &identity, None, trust, store.clone()).await? {
                    Some(result) => AuthenticationReply::Result(result),
                    None => AuthenticationReply::Prompt(SshAuthenticationRound {
                        kind: SshAuthenticationKind::PrivateKey,
                        name: String::new(),
                        instructions: String::new(),
                        prompts: vec![SshAuthenticationPrompt {
                            prompt: "Private key passphrase".into(),
                            echo: false,
                        }],
                    }),
                }
            };
        }
    }

    fn pick_candidate(
        &self,
        config: &SshConnectionConfig,
        remaining: &MethodSet,
    ) -> Option<AuthenticationCandidate> {
        let chain = config.auth_type == SshAuthType::Chain;
        // Explicit chains follow the advertised methods; legacy single-identity
        // priority stays compatible with the established keyboard-interactive flow.
        let methods = if chain {
            remaining.iter().copied().collect::<Vec<_>>()
        } else {
            let configured = configured_method(config.auth_type)?;
            if configured == MethodKind::Password {
                vec![MethodKind::KeyboardInteractive, configured]
            } else {
                vec![configured, MethodKind::KeyboardInteractive]
            }
        };
        for method in methods
            .into_iter()
            .filter(|method| remaining.contains(method))
        {
            if method == MethodKind::KeyboardInteractive {
                let candidate = AuthenticationCandidate::KeyboardInteractive;
                if !self.attempted.contains(&candidate) {
                    return Some(candidate);
                }
            } else {
                let count = if chain {
                    config.authentication_factors.len()
                } else {
                    1
                };
                for index in 0..count {
                    let candidate = AuthenticationCandidate::Credential(index);
                    let auth_type = if chain {
                        config.authentication_factors[index].auth_type
                    } else {
                        config.auth_type
                    };
                    if configured_method(auth_type) == Some(method)
                        && !self.attempted.contains(&candidate)
                        && (!chain || !self.satisfied.contains(&index))
                    {
                        return Some(candidate);
                    }
                }
            }
        }
        None
    }
}

fn no_method(config: &SshConnectionConfig, methods: &MethodSet, reason: &str) -> SshError {
    SshError::AuthFailed(
        config.username.clone(),
        format!("{reason} (remaining_methods={methods:?})"),
    )
}

fn interactive_reply(response: client::KeyboardInteractiveAuthResponse) -> AuthenticationReply {
    match response {
        client::KeyboardInteractiveAuthResponse::Success => {
            AuthenticationReply::Result(client::AuthResult::Success)
        }
        client::KeyboardInteractiveAuthResponse::Failure {
            remaining_methods,
            partial_success,
        } => AuthenticationReply::Result(client::AuthResult::Failure {
            remaining_methods,
            partial_success,
        }),
        client::KeyboardInteractiveAuthResponse::InfoRequest {
            name,
            instructions,
            prompts,
        } => AuthenticationReply::Prompt(SshAuthenticationRound {
            kind: SshAuthenticationKind::KeyboardInteractive,
            name,
            instructions,
            prompts: prompts
                .into_iter()
                .map(|prompt| SshAuthenticationPrompt {
                    prompt: prompt.prompt,
                    echo: prompt.echo,
                })
                .collect(),
        }),
    }
}

async fn verify_authentication_trust(
    config: &SshConnectionConfig,
    trust: &HostTrust,
    store: Option<Arc<SshHostKeyStore>>,
) -> SshResult<()> {
    if &trust.reload(config, store).await? != trust {
        return Err(HostTrust::error(
            config,
            "trust source changed before authentication",
        ));
    }
    Ok(())
}

fn configured_method(auth_type: SshAuthType) -> Option<MethodKind> {
    match auth_type {
        SshAuthType::Password => Some(MethodKind::Password),
        SshAuthType::KeyFile
        | SshAuthType::KeyContent
        | SshAuthType::Agent
        | SshAuthType::OpenSshAgent
        | SshAuthType::Pageant => Some(MethodKind::PublicKey),
        SshAuthType::Chain => None,
    }
}

async fn authenticate(
    handle: &mut Handle<IdentityHandler>,
    config: &SshConnectionConfig,
    temporary_passphrase: Option<&str>,
    trust: &HostTrust,
    store: Option<Arc<SshHostKeyStore>>,
) -> SshResult<Option<russh::client::AuthResult>> {
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
        SshAuthType::KeyFile | SshAuthType::KeyContent => {
            let Some(identity) =
                crate::private_identity::read_private_identity(config, temporary_passphrase)
                    .await?
            else {
                return Ok(None);
            };
            let crate::private_identity::PrivateIdentity { key, certificate } = identity;
            // File IO and decryption can outlive the preceding trust check.
            verify_authentication_trust(config, trust, store).await?;
            if let Some(certificate) = certificate {
                handle
                    .authenticate_openssh_cert(&config.username, Arc::new(key), certificate)
                    .await
            } else {
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
        }
        SshAuthType::Agent | SshAuthType::OpenSshAgent | SshAuthType::Pageant => {
            let provider = match config.auth_type {
                SshAuthType::Pageant => crate::SshAgentProvider::Pageant,
                SshAuthType::OpenSshAgent => crate::SshAgentProvider::OpenSsh,
                _ => crate::SshAgentProvider::Auto,
            };
            return crate::agent::authenticate(handle, &config.username, provider)
                .await
                .map(Some);
        }
        SshAuthType::Chain => {
            return Err(SshError::InvalidConfig(
                "SSH authentication requires a concrete credential".into(),
            ))
        }
    }
    .map_err(|error| protocol_error("SSH authentication", error))?;
    Ok(Some(result))
}
