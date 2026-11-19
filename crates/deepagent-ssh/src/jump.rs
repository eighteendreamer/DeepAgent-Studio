//! One explicit SSH jump hop; credentials use the existing authentication strategies.
use crate::{
    SshAuthType, SshConnectionConfig, SshError, SshHostIdentity, SshHostKeyPolicy, SshResult,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SshConnectionRole {
    #[default]
    Target,
    Jump,
}

impl SshConnectionRole {
    pub(crate) fn config(self, root: &SshConnectionConfig) -> SshResult<SshConnectionConfig> {
        match self {
            Self::Target => Ok(root.clone()),
            Self::Jump => root
                .jump
                .as_ref()
                .map(|jump| jump.config(root))
                .ok_or_else(|| SshError::InvalidConfig("jump configuration was removed".into())),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SshJumpEndpoint {
    pub host: String,
    pub port: u16,
    pub username: String,
}

impl SshJumpEndpoint {
    pub(crate) fn canonical(&self) -> SshResult<Self> {
        let endpoint = SshHostIdentity::direct(&self.host, self.port)?;
        if self.username.trim().is_empty() || self.username.chars().any(char::is_control) {
            return Err(SshError::InvalidConfig("invalid SSH jump username".into()));
        }
        Ok(Self {
            host: endpoint.host,
            port: endpoint.port,
            username: self.username.clone(),
        })
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SshJumpConfig {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub auth_type: SshAuthType,
    #[serde(default)]
    pub agent_fingerprint: Option<String>,
    pub key_path: Option<String>,
    pub key_content: Option<String>,
    pub certificate_path: Option<String>,
    pub password: Option<String>,
    #[serde(default)]
    pub authentication_factors: Vec<crate::SshAuthenticationFactor>,
    pub known_hosts_path: Option<String>,
    #[serde(default)]
    pub host_key_policy: SshHostKeyPolicy,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SshJumpSettingsRequest {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub auth_type: SshAuthType,
    #[serde(default)]
    pub agent_fingerprint: Option<String>,
    pub key_path: Option<String>,
    pub key_content: Option<String>,
    pub certificate_path: Option<String>,
    pub password: Option<String>,
    pub authentication_factors: Option<Vec<crate::SshAuthenticationFactorRequest>>,
    pub known_hosts_path: Option<String>,
    pub host_key_policy: Option<SshHostKeyPolicy>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum SshJumpRequest {
    Direct,
    Ssh(SshJumpSettingsRequest),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SshJumpDto {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub auth_type: SshAuthType,
    pub agent_fingerprint: Option<String>,
    pub key_path: Option<String>,
    pub certificate_path: Option<String>,
    pub has_key_content: bool,
    pub has_password: bool,
    pub authentication_factors: Vec<crate::SshAuthenticationFactorDto>,
    pub known_hosts_path: Option<String>,
    pub host_key_policy: SshHostKeyPolicy,
}

impl std::fmt::Debug for SshJumpConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SshJumpConfig")
            .field("endpoint", &self.endpoint())
            .finish_non_exhaustive()
    }
}
impl std::fmt::Debug for SshJumpSettingsRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SshJumpSettingsRequest { .. }")
    }
}
impl std::fmt::Debug for SshJumpRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SshJumpRequest { .. }")
    }
}

impl SshJumpConfig {
    pub fn endpoint(&self) -> SshJumpEndpoint {
        SshJumpEndpoint {
            host: self.host.clone(),
            port: self.port,
            username: self.username.clone(),
        }
    }

    pub(crate) fn config(&self, parent: &SshConnectionConfig) -> SshConnectionConfig {
        let mut config = SshConnectionConfig::new(
            &parent.name,
            &self.host,
            self.port,
            &self.username,
            self.auth_type,
        );
        config.id = parent.id.clone();
        config.key_path = self.key_path.clone();
        config.agent_fingerprint = self.agent_fingerprint.clone();
        config.key_content = self.key_content.clone();
        config.certificate_path = self.certificate_path.clone();
        config.password = self.password.clone();
        config.authentication_factors = self.authentication_factors.clone();
        config.known_hosts_path = self.known_hosts_path.clone();
        config.host_key_policy = self.host_key_policy;
        config.proxy = parent.proxy.clone();
        config.timing = parent.timing;
        config
    }

    pub(crate) fn dto(&self) -> SshJumpDto {
        SshJumpDto {
            host: self.host.clone(),
            port: self.port,
            username: self.username.clone(),
            auth_type: self.auth_type,
            agent_fingerprint: self.agent_fingerprint.clone(),
            key_path: self.key_path.clone(),
            certificate_path: self.certificate_path.clone(),
            has_key_content: self.key_content.is_some(),
            has_password: self.password.is_some(),
            authentication_factors: self
                .authentication_factors
                .iter()
                .map(|factor| factor.dto())
                .collect(),
            known_hosts_path: self.known_hosts_path.clone(),
            host_key_policy: self.host_key_policy,
        }
    }

    pub(crate) fn contains_secret(&self) -> bool {
        self.password.is_some()
            || self.key_content.is_some()
            || self
                .authentication_factors
                .iter()
                .any(|factor| factor.contains_secret())
    }

    pub(crate) fn validate(&self) -> SshResult<()> {
        SshHostIdentity::direct(&self.host, self.port)?;
        if self.username.trim().is_empty() || self.username.chars().any(char::is_control) {
            return Err(SshError::InvalidConfig(
                "jump username is required and must contain no control characters".into(),
            ));
        }
        let parent = SshConnectionConfig::new(
            "jump validation",
            &self.host,
            self.port,
            &self.username,
            self.auth_type,
        );
        self.config(&parent).validate_credentials()
    }
}

pub(crate) fn resolve_jump(
    request: Option<SshJumpRequest>,
    previous: Option<&SshJumpConfig>,
    default_known_hosts: &str,
) -> SshResult<Option<SshJumpConfig>> {
    let Some(request) = request else {
        return Ok(previous.cloned());
    };
    let SshJumpRequest::Ssh(settings) = request else {
        return Ok(None);
    };
    let host = SshHostIdentity::direct(&settings.host, settings.port)?.host;
    let same = previous.filter(|old| {
        old.host == host && old.port == settings.port && old.username == settings.username
    });
    let same_credential =
        same.filter(|old| old.auth_type == settings.auth_type && old.key_path == settings.key_path);
    let mut config = SshJumpConfig {
        host,
        port: settings.port,
        username: settings.username,
        auth_type: settings.auth_type,
        agent_fingerprint: settings
            .agent_fingerprint
            .or_else(|| same_credential.and_then(|old| old.agent_fingerprint.clone())),
        key_path: settings.key_path,
        key_content: settings
            .key_content
            .or_else(|| same_credential.and_then(|old| old.key_content.clone())),
        certificate_path: settings
            .certificate_path
            .or_else(|| same_credential.and_then(|old| old.certificate_path.clone())),
        password: settings
            .password
            .or_else(|| same_credential.and_then(|old| old.password.clone())),
        authentication_factors: if settings.auth_type == SshAuthType::Chain {
            crate::authentication_factors::resolve_factors(
                settings.authentication_factors,
                same.map_or(&[], |old| old.authentication_factors.as_slice()),
            )?
        } else {
            Vec::new()
        },
        known_hosts_path: Some(
            settings
                .known_hosts_path
                .or_else(|| same.and_then(|old| old.known_hosts_path.clone()))
                .filter(|path| !path.trim().is_empty())
                .unwrap_or_else(|| default_known_hosts.into()),
        ),
        host_key_policy: settings
            .host_key_policy
            .or_else(|| same.map(|old| old.host_key_policy))
            .unwrap_or_default(),
    };
    crate::config::normalize_agent_fingerprint(config.auth_type, &mut config.agent_fingerprint);
    crate::config::normalize_credential(
        config.auth_type,
        &mut config.key_path,
        &mut config.key_content,
        &mut config.certificate_path,
        &mut config.password,
    );
    config.validate()?;
    Ok(Some(config))
}
