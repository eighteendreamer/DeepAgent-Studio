//! SSH connection configuration DTOs.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SshAuthType {
    #[default]
    Agent,
    #[serde(rename = "openssh_agent")]
    OpenSshAgent,
    Pageant,
    KeyFile,
    KeyContent,
    Password,
    Chain,
}

/// Only explicit TOFU may enroll unknown hosts; normal production has no Insecure mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SshHostKeyPolicy {
    #[default]
    Strict,
    AcceptNew,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct SshConnectionConfig {
    pub id: String,
    pub name: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub auth_type: SshAuthType,
    pub key_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_content: Option<String>,
    /// Optional OpenSSH user certificate attached to the configured private key.
    #[serde(default)]
    pub certificate_path: Option<String>,
    /// Explicit verified OpenSSH known_hosts source. Legacy None resolves to app data,
    /// never the process user's default SSH directory.
    #[serde(default)]
    pub known_hosts_path: Option<String>,
    #[serde(default)]
    pub host_key_policy: SshHostKeyPolicy,
    #[serde(default)]
    pub password: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub authentication_factors: Vec<crate::SshAuthenticationFactor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy: Option<crate::SshProxyConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jump: Option<crate::SshJumpConfig>,
    #[serde(default)]
    pub extra_options: HashMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub control_path: Option<String>,
    #[serde(default)]
    pub cached_status: SshStatus,
    #[serde(default)]
    pub cached_last_error: Option<String>,
    #[serde(default)]
    pub cached_latency_ms: Option<u64>,
    #[serde(default)]
    pub cached_checked_at_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SshStatus {
    #[default]
    Disconnected,
    Connecting,
    Connected,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SshConnectionDto {
    pub id: String,
    pub name: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub auth_type: SshAuthType,
    #[serde(default)]
    pub authentication_factors: Vec<crate::SshAuthenticationFactorDto>,
    #[serde(default)]
    pub proxy: Option<crate::SshProxyDto>,
    #[serde(default)]
    pub jump: Option<crate::SshJumpDto>,
    pub key_path: Option<String>,
    #[serde(default)]
    pub has_key_content: bool,
    #[serde(default)]
    pub certificate_path: Option<String>,
    #[serde(default)]
    pub known_hosts_path: Option<String>,
    #[serde(default)]
    pub host_key_policy: SshHostKeyPolicy,
    pub status: SshStatus,
    pub last_error: Option<String>,
    pub latency_ms: Option<u64>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct CreateSshConnectionRequest {
    pub name: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub auth_type: SshAuthType,
    #[serde(default)]
    pub authentication_factors: Option<Vec<crate::SshAuthenticationFactorRequest>>,
    #[serde(default)]
    pub proxy: Option<crate::SshProxyRequest>,
    #[serde(default)]
    pub jump: Option<crate::SshJumpRequest>,
    pub key_path: Option<String>,
    #[serde(default)]
    pub key_content: Option<String>,
    #[serde(default)]
    pub certificate_path: Option<String>,
    #[serde(default)]
    pub known_hosts_path: Option<String>,
    #[serde(default)]
    pub host_key_policy: SshHostKeyPolicy,
    pub password: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct UpdateSshConnectionRequest {
    pub id: String,
    pub name: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub auth_type: SshAuthType,
    /// None preserves an existing chain; an explicit list replaces it by opaque factor IDs.
    #[serde(default)]
    pub authentication_factors: Option<Vec<crate::SshAuthenticationFactorRequest>>,
    /// Omitted preserves routing; mode=direct explicitly removes a saved proxy.
    #[serde(default)]
    pub proxy: Option<crate::SshProxyRequest>,
    #[serde(default)]
    pub jump: Option<crate::SshJumpRequest>,
    pub key_path: Option<String>,
    /// Omitted content preserves the saved identity only when its source is unchanged.
    #[serde(default)]
    pub key_content: Option<String>,
    /// None preserves the attachment; Some("") explicitly removes it.
    #[serde(default)]
    pub certificate_path: Option<String>,
    /// None preserves the existing source; Some("") resets to the app default.
    #[serde(default)]
    pub known_hosts_path: Option<String>,
    /// Legacy callers preserve the saved policy instead of silently changing it.
    #[serde(default)]
    pub host_key_policy: Option<SshHostKeyPolicy>,
    pub password: Option<String>,
}

impl SshConnectionConfig {
    pub(crate) fn route_label(&self) -> &'static str {
        if self.jump.is_some() {
            return "jump";
        }
        self.proxy
            .as_ref()
            .map_or("direct", |proxy| proxy.mode.label())
    }

    pub(crate) fn normalize_credentials(&mut self) {
        normalize_credential(
            self.auth_type,
            &mut self.key_path,
            &mut self.key_content,
            &mut self.certificate_path,
            &mut self.password,
        );
        if self.auth_type != SshAuthType::Chain {
            self.authentication_factors.clear();
        }
    }

    pub(crate) fn validate_credentials(&self) -> crate::SshResult<()> {
        if self.auth_type == SshAuthType::Chain {
            crate::authentication_factors::validate_factors(&self.authentication_factors)
        } else {
            validate_credential(
                self.auth_type,
                self.key_path.as_deref(),
                self.key_content.as_deref(),
                self.password.as_deref(),
            )
        }
    }

    pub fn new(
        name: impl Into<String>,
        host: impl Into<String>,
        port: u16,
        username: impl Into<String>,
        auth_type: SshAuthType,
    ) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            name: name.into(),
            host: host.into(),
            port,
            username: username.into(),
            auth_type,
            key_path: None,
            key_content: None,
            certificate_path: None,
            known_hosts_path: None,
            host_key_policy: SshHostKeyPolicy::Strict,
            password: None,
            authentication_factors: Vec::new(),
            proxy: None,
            jump: None,
            extra_options: HashMap::new(),
            control_path: None,
            cached_status: SshStatus::Disconnected,
            cached_last_error: None,
            cached_latency_ms: None,
            cached_checked_at_ms: None,
        }
    }
}

pub(crate) fn normalize_credential(
    auth_type: SshAuthType,
    key_path: &mut Option<String>,
    key_content: &mut Option<String>,
    certificate_path: &mut Option<String>,
    password: &mut Option<String>,
) {
    if !matches!(auth_type, SshAuthType::KeyFile | SshAuthType::KeyContent)
        || certificate_path
            .as_ref()
            .is_some_and(|path| path.trim().is_empty())
    {
        *certificate_path = None;
    }
    if auth_type != SshAuthType::KeyContent {
        *key_content = None;
    }
    if auth_type != SshAuthType::KeyFile {
        *key_path = None;
    }
    if password.as_deref() == Some("")
        || !matches!(
            auth_type,
            SshAuthType::Password | SshAuthType::KeyFile | SshAuthType::KeyContent
        )
    {
        *password = None;
    }
}

pub(crate) fn validate_credential(
    auth_type: SshAuthType,
    key_path: Option<&str>,
    key_content: Option<&str>,
    password: Option<&str>,
) -> crate::SshResult<()> {
    let missing = match auth_type {
        SshAuthType::Password if password.unwrap_or_default().is_empty() => {
            Some("password is required")
        }
        SshAuthType::KeyFile if key_path.unwrap_or_default().trim().is_empty() => {
            Some("key path is required")
        }
        SshAuthType::KeyContent if key_content.unwrap_or_default().trim().is_empty() => {
            Some("private-key content is required")
        }
        _ => None,
    };
    match missing {
        Some(message) => Err(crate::SshError::InvalidConfig(message.into())),
        None => Ok(()),
    }
}

impl std::fmt::Debug for SshConnectionConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SshConnectionConfig")
            .field("id", &self.id)
            .field("auth_type", &self.auth_type)
            .finish_non_exhaustive()
    }
}

impl std::fmt::Debug for CreateSshConnectionRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CreateSshConnectionRequest")
            .field("auth_type", &self.auth_type)
            .finish_non_exhaustive()
    }
}

impl std::fmt::Debug for UpdateSshConnectionRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UpdateSshConnectionRequest")
            .field("id", &self.id)
            .field("auth_type", &self.auth_type)
            .finish_non_exhaustive()
    }
}
