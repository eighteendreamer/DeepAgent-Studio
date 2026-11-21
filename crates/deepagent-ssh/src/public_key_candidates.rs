//! Attempt-scoped public identities; the existing negotiator owns server progression.
use crate::agent::ReadyAgent;
use crate::private_identity::{read_private_identity, PrivateIdentity};
use crate::{SshAuthType, SshConnectionConfig, SshDefaultIdentityState, SshError, SshResult};
use russh::keys::{agent::AgentIdentity, HashAlg};
use std::collections::{HashSet, VecDeque};

pub(crate) const MAX_PUBLIC_KEY_ATTEMPTS: usize = 5;

pub(crate) enum PublicKeyCandidate {
    Agent(AgentIdentity),
    Private(PrivateIdentity),
    Unlock(String),
    Exhausted,
}

#[derive(Default)]
pub(crate) struct PublicKeyCandidates {
    initialized: bool,
    pub(crate) agent: Option<ReadyAgent>,
    files: VecDeque<String>,
    pending_file: Option<SshConnectionConfig>,
    fingerprints: HashSet<String>,
    attempts: usize,
    explanations: Vec<String>,
}

impl PublicKeyCandidates {
    #[cfg(test)]
    pub(crate) fn from_sources(
        config: &SshConnectionConfig,
        agent: Option<ReadyAgent>,
        files: Vec<String>,
    ) -> SshResult<Self> {
        let mut sources = Self {
            initialized: true,
            agent,
            files: files.into_iter().take(3).collect(),
            ..Default::default()
        };
        sources.select_agent(config)?;
        Ok(sources)
    }

    fn select_agent(&mut self, config: &SshConnectionConfig) -> SshResult<()> {
        if let Some(agent) = &mut self.agent {
            agent.select(
                &config.username,
                config.agent_fingerprint.as_deref(),
                MAX_PUBLIC_KEY_ATTEMPTS.saturating_sub(self.files.len()),
            )?;
        }
        Ok(())
    }

    pub(crate) async fn initialize(&mut self, config: &SshConnectionConfig) -> SshResult<()> {
        if self.initialized {
            return Ok(());
        }
        let automatic = config.auth_type == SshAuthType::AutoPublicKey;
        if automatic {
            match crate::discover_default_ssh_identities().await {
                Ok(files) => {
                    for file in files {
                        match file.state {
                            SshDefaultIdentityState::Available => self.files.push_back(file.path),
                            SshDefaultIdentityState::Unavailable => self.explanations.push(
                                format!("{}: {}", file.name, file.reason.unwrap_or_default()),
                            ),
                            SshDefaultIdentityState::Missing => {}
                        }
                    }
                }
                Err(error) => self
                    .explanations
                    .push(format!("default identities: {error}")),
            }
        }
        let provider = match config.auth_type {
            SshAuthType::OpenSshAgent => crate::SshAgentProvider::OpenSsh,
            SshAuthType::Pageant => crate::SshAgentProvider::Pageant,
            _ => crate::SshAgentProvider::Auto,
        };
        self.agent =
            crate::agent::authentication_agent(&config.username, provider, automatic).await?;
        // Reserve room for all three default-file names within the same global budget.
        self.select_agent(config)?;
        if self.agent.is_none() {
            self.explanations
                .push("no usable local Agent identities".into());
        }
        self.initialized = true;
        Ok(())
    }

    pub(crate) fn has_next(&self) -> bool {
        self.attempts < MAX_PUBLIC_KEY_ATTEMPTS
            && (self.agent.as_ref().is_some_and(ReadyAgent::has_next) || !self.files.is_empty())
    }

    pub(crate) async fn next(
        &mut self,
        config: &SshConnectionConfig,
    ) -> SshResult<PublicKeyCandidate> {
        while self.has_next() {
            if let Some(identity) = self.agent.as_mut().and_then(ReadyAgent::take_next) {
                let fingerprint = identity
                    .public_key()
                    .fingerprint(HashAlg::Sha256)
                    .to_string();
                if self.offer(fingerprint) {
                    return Ok(PublicKeyCandidate::Agent(identity));
                }
            } else if let Some(path) = self.files.pop_front() {
                let mut identity = config.clone();
                identity.auth_type = SshAuthType::KeyFile;
                identity.key_path = Some(path);
                identity.key_content = None;
                identity.password = None;
                identity.certificate_path = None;
                identity.agent_fingerprint = None;
                match read_private_identity(&identity, None).await {
                    Ok(Some(key)) => {
                        if self.offer(
                            key.key
                                .public_key()
                                .fingerprint(HashAlg::Sha256)
                                .to_string(),
                        ) {
                            return Ok(PublicKeyCandidate::Private(key));
                        }
                    }
                    Ok(None) => {
                        let label = identity.key_path.clone().unwrap_or_default();
                        self.pending_file = Some(identity);
                        return Ok(PublicKeyCandidate::Unlock(label));
                    }
                    Err(error) => self
                        .explanations
                        .push(format!("default key unavailable: {error}")),
                }
            }
        }
        Ok(PublicKeyCandidate::Exhausted)
    }

    pub(crate) async fn unlock(&mut self, passphrase: &str) -> SshResult<PublicKeyCandidate> {
        let identity = self.pending_file.take().ok_or_else(|| {
            SshError::InvalidConfig("automatic private-key unlock has no active identity".into())
        })?;
        let key = read_private_identity(&identity, Some(passphrase))
            .await?
            .ok_or_else(|| {
                SshError::AuthFailed(identity.username, "private key remains locked".into())
            })?;
        if self.offer(
            key.key
                .public_key()
                .fingerprint(HashAlg::Sha256)
                .to_string(),
        ) {
            Ok(PublicKeyCandidate::Private(key))
        } else {
            Ok(PublicKeyCandidate::Exhausted)
        }
    }

    fn offer(&mut self, fingerprint: String) -> bool {
        if self.attempts >= MAX_PUBLIC_KEY_ATTEMPTS
            || !self.fingerprints.insert(fingerprint.clone())
        {
            return false;
        }
        self.attempts += 1;
        self.explanations.push(format!("offered {fingerprint}"));
        true
    }

    pub(crate) fn failure_context(&self) -> String {
        if !self.initialized {
            return String::new();
        }
        format!(
            "; public-key identities offered={}/{MAX_PUBLIC_KEY_ATTEMPTS}; {}",
            self.attempts,
            self.explanations.join("; ")
        )
    }
}
