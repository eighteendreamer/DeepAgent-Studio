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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SshAgentProvider {
    #[default]
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

/// Secret fields are never included in Debug, returned DTOs, or persisted by this API.
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SshAgentAddRequest {
    pub provider: SshAgentProvider,
    pub key_path: Option<String>,
    pub key_content: Option<String>,
    pub passphrase: Option<String>,
    pub lifetime_secs: u32,
    pub expected_fingerprint: String,
    pub authorized: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SshAgentAddResult {
    pub provider: SshAgentProvider,
    pub identity: SshAgentIdentity,
    pub already_present: bool,
    /// None means an existing identity was left untouched, including its original constraints.
    pub lifetime_secs: Option<u32>,
}

impl std::fmt::Debug for SshAgentAddRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SshAgentAddRequest")
            .field("provider", &self.provider)
            .field("lifetime_secs", &self.lifetime_secs)
            .finish_non_exhaustive()
    }
}

async fn identity_for_agent(request: &SshAgentAddRequest) -> SshResult<keys::PrivateKey> {
    if request.key_path.is_some() == request.key_content.is_some() {
        return Err(SshError::InvalidConfig(
            "select exactly one private-key source".into(),
        ));
    }
    let mut config = crate::SshConnectionConfig::new(
        "Agent identity",
        "localhost",
        22,
        "local",
        if request.key_path.is_some() {
            crate::SshAuthType::KeyFile
        } else {
            crate::SshAuthType::KeyContent
        },
    );
    config.key_path = request.key_path.clone();
    config.key_content = request.key_content.clone();
    let identity =
        crate::private_identity::read_private_identity(&config, request.passphrase.as_deref())
            .await?
            .ok_or_else(|| SshError::InvalidConfig("private-key passphrase is required".into()))?;
    Ok(identity.key)
}

pub async fn preview_agent_identity(request: SshAgentAddRequest) -> SshResult<SshAgentIdentity> {
    let key = identity_for_agent(&request).await?;
    Ok(SshAgentIdentity {
        algorithm: key.algorithm().to_string(),
        fingerprint: key.public_key().fingerprint(HashAlg::Sha256).to_string(),
    })
}

pub async fn add_ssh_agent_identity(request: SshAgentAddRequest) -> SshResult<SshAgentAddResult> {
    add_identity_with(request, Arc::new(SystemAgentConnector)).await
}

async fn connect_for_extension(
    provider: SshAgentProvider,
    connector: Arc<dyn AgentConnector>,
) -> SshResult<(SshAgentProvider, AgentClient)> {
    let providers = if provider == SshAgentProvider::Auto {
        connector.automatic_providers().to_vec()
    } else {
        vec![provider]
    };
    let mut last = "no supported Agent provider".to_owned();
    for selected in providers {
        match tokio::time::timeout(INSPECTION_TIMEOUT, connector.connect(selected)).await {
            Ok(Ok(client)) => return Ok((selected, client)),
            Ok(Err(issue)) => {
                last = issue.reason;
                if provider != SshAgentProvider::Auto
                    || !matches!(
                        issue.state,
                        SshAgentState::Unavailable | SshAgentState::Unsupported
                    )
                {
                    break;
                }
            }
            Err(_) => {
                last = "Agent connection timed out".into();
                break;
            }
        }
    }
    Err(SshError::InvalidConfig(last))
}

pub(crate) async fn forwarding_stream(
    provider: SshAgentProvider,
) -> SshResult<Box<dyn keys::agent::client::AgentStream + Send + Unpin>> {
    let (_, client) = connect_for_extension(provider, Arc::new(SystemAgentConnector)).await?;
    Ok(client.into_inner())
}

async fn add_identity_with(
    request: SshAgentAddRequest,
    connector: Arc<dyn AgentConnector>,
) -> SshResult<SshAgentAddResult> {
    // Validate authorization before touching Agent or reading a private key.
    if !request.authorized
        || request.expected_fingerprint.is_empty()
        || !(1..=86400).contains(&request.lifetime_secs)
    {
        return Err(SshError::InvalidConfig(
            "explicit fingerprint confirmation and Agent lifetime of 1..86400 seconds are required"
                .into(),
        ));
    }
    let key = identity_for_agent(&request).await?;
    let identity = SshAgentIdentity {
        algorithm: key.algorithm().to_string(),
        fingerprint: key.public_key().fingerprint(HashAlg::Sha256).to_string(),
    };
    if identity.fingerprint != request.expected_fingerprint {
        return Err(SshError::InvalidConfig(
            "private-key identity changed; preview and confirm again".into(),
        ));
    }
    let (provider, mut client) = connect_for_extension(request.provider, connector).await?;
    let mutation = async {
        let identities = client
            .request_identities()
            .await
            .map_err(|_| SshError::InvalidConfig("Agent identity query failed".into()))?;
        if identities.len() > MAX_IDENTITIES {
            return Err(SshError::InvalidConfig(
                "Agent identity limit exceeded".into(),
            ));
        }
        let already_present = identities
            .iter()
            .any(|existing| existing.public_key().as_ref() == key.public_key());
        if !already_present {
            client.add_identity(&key, &[keys::agent::Constraint::KeyLifetime { seconds: request.lifetime_secs }]).await
                .map_err(|_| SshError::InvalidConfig("Agent refused the constrained identity; no unconstrained fallback was attempted".into()))?;
        }
        Ok(SshAgentAddResult {
            provider,
            identity,
            already_present,
            lifetime_secs: (!already_present).then_some(request.lifetime_secs),
        })
    };
    tokio::time::timeout(Duration::from_secs(5), mutation)
        .await
        .map_err(|_| {
            SshError::InvalidConfig(
                "Agent operation timed out; inspect Agent before retrying".into(),
            )
        })?
}

fn allowed_forwarded_request(request: &[u8]) -> bool {
    match request.first() {
        Some(11 | 13) => true,
        Some(27) if request.len() >= 5 => {
            let size = u32::from_be_bytes(request[1..5].try_into().unwrap()) as usize;
            request.get(5..5usize.saturating_add(size))
                == Some(b"session-bind@openssh.com".as_slice())
        }
        _ => false,
    }
}

/// Forward identity queries and signatures; remote peers cannot add/remove/unlock local identities.
pub(crate) async fn bridge_agent<R, L>(remote: &mut R, local: &mut L) -> std::io::Result<()>
where
    R: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
    L: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + ?Sized,
{
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    loop {
        let size = match remote.read_u32().await {
            Ok(value) if (1..=256 * 1024).contains(&value) => value,
            Ok(_) => return Err(std::io::ErrorKind::InvalidData.into()),
            Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(()),
            Err(error) => return Err(error),
        };
        let mut request = vec![0; size as usize];
        remote.read_exact(&mut request).await?;
        if !allowed_forwarded_request(&request) {
            remote.write_all(&[0, 0, 0, 1, 5]).await?;
            continue;
        }
        local.write_u32(size).await?;
        local.write_all(&request).await?;
        local.flush().await?;
        let size = local.read_u32().await?;
        if !(1..=256 * 1024).contains(&size) {
            return Err(std::io::ErrorKind::InvalidData.into());
        }
        let mut response = vec![0; size as usize];
        local.read_exact(&mut response).await?;
        remote.write_u32(size).await?;
        remote.write_all(&response).await?;
        remote.flush().await?;
    }
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
        options: &crate::SshAdvancedOptions,
    ) -> SshResult<russh::client::AuthResult> {
        // Human challenges suspend the negotiator between calls. Only active
        // Agent authentication consumes the attempt's shared signing budget.
        let started = tokio::time::Instant::now();
        let result = tokio::time::timeout(self.remaining_authentication_time, async {
            let hash = handle.best_supported_rsa_hash().await.map_err(|error| {
                SshError::AuthFailed(
                    username.into(),
                    format!("SSH Agent RSA negotiation: {error}"),
                )
            })?;
            let hash = options.authentication_hash(identity.public_key().algorithm(), hash)?;
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
            .authenticate_identity(handle, username, identity, &Default::default())
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
