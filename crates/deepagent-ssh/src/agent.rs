//! Local Agent providers. Inspection never signs, mutates identities, or connects to a host.
use crate::transport::IdentityHandler;
use crate::{SshError, SshResult};
use russh::client::Handle;
use russh::keys::{self, agent::AgentIdentity, HashAlg};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;

type AgentClient = keys::agent::client::AgentClient<
    Box<dyn keys::agent::client::AgentStream + Send + Unpin + 'static>,
>;
const INSPECTION_TIMEOUT: Duration = Duration::from_secs(2);
const AUTHENTICATION_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_IDENTITIES: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SshAgentProvider {
    Auto,
    OpenSsh,
    Pageant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SshAgentState {
    Available,
    Empty,
    Unavailable,
    Unsupported,
    TimedOut,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SshAgentIdentity {
    pub algorithm: String,
    pub fingerprint: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SshAgentProbe {
    pub provider: SshAgentProvider,
    pub state: SshAgentState,
    pub identities: Vec<SshAgentIdentity>,
    pub reason: Option<String>,
}

struct AgentIssue {
    state: SshAgentState,
    reason: String,
}

impl AgentIssue {
    fn new(state: SshAgentState, reason: impl Into<String>) -> Self {
        Self {
            state,
            reason: reason.into(),
        }
    }
}

#[async_trait::async_trait]
trait AgentConnector: Send + Sync {
    async fn connect(&self, provider: SshAgentProvider) -> Result<AgentClient, AgentIssue>;
    fn automatic_providers(&self) -> &'static [SshAgentProvider];
}

struct SystemAgentConnector;

#[cfg(windows)]
async fn connect_openssh_pipe(path: &str) -> Result<AgentClient, keys::Error> {
    keys::agent::client::AgentClient::connect_named_pipe(path)
        .await
        .map(|agent| agent.dynamic())
}

#[async_trait::async_trait]
impl AgentConnector for SystemAgentConnector {
    async fn connect(&self, provider: SshAgentProvider) -> Result<AgentClient, AgentIssue> {
        match provider {
            #[cfg(unix)]
            SshAgentProvider::OpenSsh => keys::agent::client::AgentClient::connect_env()
                .await
                .map(|agent| agent.dynamic())
                .map_err(|error| {
                    AgentIssue::new(
                        SshAgentState::Unavailable,
                        format!("OpenSSH Agent: {error}"),
                    )
                }),
            #[cfg(windows)]
            SshAgentProvider::OpenSsh => connect_openssh_pipe(r"\\.\pipe\openssh-ssh-agent")
                .await
                .map_err(|error| {
                    AgentIssue::new(
                        SshAgentState::Unavailable,
                        format!("OpenSSH Agent: {error}"),
                    )
                }),
            #[cfg(windows)]
            SshAgentProvider::Pageant => keys::agent::client::AgentClient::connect_pageant()
                .await
                .map(|agent| agent.dynamic())
                .map_err(|error| {
                    AgentIssue::new(SshAgentState::Unavailable, format!("Pageant: {error}"))
                }),
            _ => Err(AgentIssue::new(
                SshAgentState::Unsupported,
                "Agent provider is not supported on this platform",
            )),
        }
    }

    fn automatic_providers(&self) -> &'static [SshAgentProvider] {
        #[cfg(windows)]
        {
            &[SshAgentProvider::OpenSsh, SshAgentProvider::Pageant]
        }
        #[cfg(not(windows))]
        {
            &[SshAgentProvider::OpenSsh]
        }
    }
}

pub(crate) struct ReadyAgent {
    signer: AgentSigner,
    identities: Vec<AgentIdentity>,
    remaining_authentication_time: Duration,
}

// Erase the provider stream before entering russh's borrowed RPITIT Signer boundary.
// The protocol and signature construction remain owned by russh.
struct AgentSigner(AgentClient);

impl russh::Signer for AgentSigner {
    type Error = russh::AgentAuthError;

    fn auth_sign(
        &mut self,
        identity: &AgentIdentity,
        hash_alg: Option<HashAlg>,
        to_sign: Vec<u8>,
    ) -> impl std::future::Future<Output = Result<Vec<u8>, Self::Error>> + Send {
        let identity = identity.clone();
        Box::pin(async move {
            self.0
                .sign_request(&identity, hash_alg, to_sign)
                .await
                .map_err(Into::into)
        })
    }
}

struct Resolution {
    probes: Vec<SshAgentProbe>,
    ready: Option<ReadyAgent>,
}

async fn resolve_agent(
    provider: SshAgentProvider,
    connector: Arc<dyn AgentConnector>,
) -> Resolution {
    let providers = if provider == SshAgentProvider::Auto {
        connector.automatic_providers().to_vec()
    } else {
        vec![provider]
    };
    let mut resolution = Resolution {
        probes: Vec::new(),
        ready: None,
    };
    for provider in providers {
        let connector = connector.clone();
        let inspected = tokio::time::timeout(INSPECTION_TIMEOUT, async move {
            let mut client = connector.connect(provider).await?;
            let identities = client.request_identities().await
                .map_err(|error| AgentIssue::new(SshAgentState::Error, format!("Agent identity query failed: {error}")))?;
            if identities.len() > MAX_IDENTITIES {
                return Err(AgentIssue::new(SshAgentState::Error, "Agent has more than 64 identities; select a smaller identity set in your Agent"));
            }
            Ok::<_, AgentIssue>(ReadyAgent { signer: AgentSigner(client), identities, remaining_authentication_time: AUTHENTICATION_TIMEOUT })
        }).await.unwrap_or_else(|_| Err(AgentIssue::new(SshAgentState::TimedOut, "Agent inspection exceeded 2 seconds")));
        let probe = match inspected {
            Ok(ready) if !ready.identities.is_empty() => {
                let identities = ready
                    .identities
                    .iter()
                    .map(|identity| {
                        let key = identity.public_key();
                        SshAgentIdentity {
                            algorithm: key.algorithm().as_str().to_owned(),
                            fingerprint: key.fingerprint(HashAlg::Sha256).to_string(),
                        }
                    })
                    .collect();
                resolution.ready = Some(ready);
                SshAgentProbe {
                    provider,
                    state: SshAgentState::Available,
                    identities,
                    reason: None,
                }
            }
            Ok(_) => SshAgentProbe {
                provider,
                state: SshAgentState::Empty,
                identities: vec![],
                reason: None,
            },
            Err(issue) => SshAgentProbe {
                provider,
                state: issue.state,
                identities: vec![],
                reason: Some(issue.reason),
            },
        };
        let fallback = matches!(
            probe.state,
            SshAgentState::Empty | SshAgentState::Unavailable | SshAgentState::Unsupported
        );
        resolution.probes.push(probe);
        if !fallback {
            break;
        }
    }
    resolution
}

/// Bounded, local, read-only public-identity discovery. Available is not an authentication result.
pub async fn probe_ssh_agent(provider: SshAgentProvider) -> Vec<SshAgentProbe> {
    resolve_agent(provider, Arc::new(SystemAgentConnector))
        .await
        .probes
}

pub(crate) async fn authentication_agent(
    username: &str,
    provider: SshAgentProvider,
    optional: bool,
) -> SshResult<Option<ReadyAgent>> {
    let resolution = resolve_agent(provider, Arc::new(SystemAgentConnector)).await;
    if let Some(ready) = resolution.ready {
        return Ok(Some(ready));
    }
    if optional
        && resolution.probes.iter().all(|probe| {
            matches!(
                probe.state,
                SshAgentState::Empty | SshAgentState::Unavailable | SshAgentState::Unsupported
            )
        })
    {
        return Ok(None);
    }
    let reason = resolution
        .probes
        .iter()
        .map(|probe| {
            format!(
                "{:?}: {:?}{}",
                probe.provider,
                probe.state,
                probe
                    .reason
                    .as_ref()
                    .map(|reason| format!(" ({reason})"))
                    .unwrap_or_default()
            )
        })
        .collect::<Vec<_>>()
        .join("; ");
    Err(SshError::AuthFailed(
        username.into(),
        format!("SSH Agent unavailable: {reason}"),
    ))
}

impl ReadyAgent {
    pub(crate) fn select(
        &mut self,
        username: &str,
        fingerprint: Option<&str>,
        limit: usize,
    ) -> SshResult<()> {
        let mut seen = std::collections::HashSet::new();
        self.identities.retain(|identity| {
            let value = identity
                .public_key()
                .fingerprint(HashAlg::Sha256)
                .to_string();
            fingerprint.is_none_or(|selected| selected == value) && seen.insert(value)
        });
        if fingerprint.is_some() && self.identities.is_empty() {
            return Err(SshError::AuthFailed(
                username.into(),
                "selected Agent fingerprint is no longer available; no other identity was offered"
                    .into(),
            ));
        }
        self.identities.truncate(limit);
        Ok(())
    }

    pub(crate) fn has_next(&self) -> bool {
        !self.identities.is_empty()
    }

    pub(crate) fn take_next(&mut self) -> Option<AgentIdentity> {
        if self.identities.is_empty() {
            None
        } else {
            Some(self.identities.remove(0))
        }
    }

    pub(crate) async fn authenticate_identity(
        &mut self,
        handle: &mut Handle<IdentityHandler>,
        username: &str,
        identity: AgentIdentity,
    ) -> SshResult<russh::client::AuthResult> {
        // Human challenges suspend the negotiator between calls. Only active
        // Agent authentication consumes the attempt's shared signing budget.
        let started = tokio::time::Instant::now();
        let result = tokio::time::timeout(self.remaining_authentication_time, async {
            let hash = handle
                .best_supported_rsa_hash()
                .await
                .map_err(|error| {
                    SshError::AuthFailed(
                        username.into(),
                        format!("SSH Agent RSA negotiation: {error}"),
                    )
                })?
                .flatten();
            handle
                .authenticate_publickey_with(
                    username.to_owned(),
                    identity.public_key().into_owned(),
                    hash,
                    &mut self.signer,
                )
                .await
                .map_err(|error| {
                    SshError::AuthFailed(
                        username.into(),
                        format!("SSH Agent signature/authentication failed: {error}"),
                    )
                })
        })
        .await;
        self.remaining_authentication_time = self
            .remaining_authentication_time
            .saturating_sub(started.elapsed());
        result.map_err(|_| {
            SshError::AuthFailed(
                username.into(),
                "SSH Agent authentication exceeded 60 seconds; no fallback was attempted".into(),
            )
        })?
    }
}

#[cfg(test)]
async fn authenticate_ready(
    handle: &mut Handle<IdentityHandler>,
    username: &str,
    mut ready: ReadyAgent,
) -> SshResult<russh::client::AuthResult> {
    ready.select(
        username,
        None,
        crate::public_key_candidates::MAX_PUBLIC_KEY_ATTEMPTS,
    )?;
    while let Some(identity) = ready.take_next() {
        let result = ready
            .authenticate_identity(handle, username, identity)
            .await?;
        if !ready.has_next()
            || !matches!(&result, russh::client::AuthResult::Failure {
            remaining_methods, partial_success: false,
        } if remaining_methods.contains(&russh::MethodKind::PublicKey))
        {
            return Ok(result);
        }
    }
    Err(SshError::AuthFailed(
        username.into(),
        "SSH Agent returned no identities for authentication".into(),
    ))
}

#[cfg(test)]
#[path = "agent_tests.rs"]
mod tests;
