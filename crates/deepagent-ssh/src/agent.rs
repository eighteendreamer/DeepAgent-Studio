//! Local Agent providers. Inspection never signs, mutates identities, or connects to a host.
use crate::transport::IdentityHandler;
use crate::{SshError, SshResult};
use russh::client::Handle;
use russh::keys::{self, HashAlg, PublicKey};
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

struct ReadyAgent {
    client: AgentClient,
    identities: Vec<PublicKey>,
}

// Erase the provider stream before entering russh's borrowed RPITIT Signer boundary.
// The protocol and signature construction remain owned by russh.
struct AgentSigner(AgentClient);

impl russh::Signer for AgentSigner {
    type Error = russh::AgentAuthError;

    fn auth_publickey_sign(
        &mut self,
        key: &PublicKey,
        hash_alg: Option<HashAlg>,
        to_sign: russh::CryptoVec,
    ) -> impl std::future::Future<Output = Result<russh::CryptoVec, Self::Error>> + Send {
        let key = key.clone();
        Box::pin(async move {
            self.0
                .sign_request(&key, hash_alg, to_sign)
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
            Ok::<_, AgentIssue>(ReadyAgent { client, identities })
        }).await.unwrap_or_else(|_| Err(AgentIssue::new(SshAgentState::TimedOut, "Agent inspection exceeded 2 seconds")));
        let probe = match inspected {
            Ok(ready) if !ready.identities.is_empty() => {
                let identities = ready
                    .identities
                    .iter()
                    .map(|key| SshAgentIdentity {
                        algorithm: key.algorithm().as_str().to_owned(),
                        fingerprint: key.fingerprint(HashAlg::Sha256).to_string(),
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

pub(crate) async fn authenticate(
    handle: &mut Handle<IdentityHandler>,
    username: &str,
    provider: SshAgentProvider,
) -> SshResult<()> {
    let resolution = resolve_agent(provider, Arc::new(SystemAgentConnector)).await;
    let ready = resolution.ready.ok_or_else(|| {
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
        SshError::AuthFailed(
            username.to_owned(),
            format!("SSH Agent unavailable: {reason}"),
        )
    })?;
    authenticate_ready(handle, username, ready).await
}

async fn authenticate_ready(
    handle: &mut Handle<IdentityHandler>,
    username: &str,
    ready: ReadyAgent,
) -> SshResult<()> {
    let deadline = tokio::time::Instant::now() + AUTHENTICATION_TIMEOUT;
    let timeout_error = || {
        SshError::AuthFailed(
            username.to_owned(),
            "SSH Agent authentication exceeded 60 seconds; no fallback was attempted".into(),
        )
    };
    let hash = tokio::time::timeout_at(deadline, handle.best_supported_rsa_hash())
        .await
        .map_err(|_| timeout_error())?
        .map_err(|error| {
            SshError::AuthFailed(
                username.to_owned(),
                format!("SSH Agent RSA negotiation: {error}"),
            )
        })?
        .flatten();
    let mut signer = AgentSigner(ready.client);
    for identity in ready.identities {
        let result = tokio::time::timeout_at(
            deadline,
            handle.authenticate_publickey_with(username.to_owned(), identity, hash, &mut signer),
        )
        .await
        .map_err(|_| timeout_error())?
        .map_err(|error| {
            SshError::AuthFailed(
                username.to_owned(),
                format!("SSH Agent signature/authentication failed: {error}"),
            )
        })?;
        if result.success() {
            return Ok(());
        }
    }
    Err(SshError::AuthFailed(
        username.to_owned(),
        "server rejected SSH Agent identities; no other provider or credentials were tried".into(),
    ))
}

#[cfg(test)]
#[path = "agent_tests.rs"]
mod tests;
