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
    Password,
}

/// Only explicit TOFU may enroll unknown hosts; normal production has no Insecure mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SshHostKeyPolicy {
    #[default]
    Strict,
    AcceptNew,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SshConnectionConfig {
    pub id: String,
    pub name: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub auth_type: SshAuthType,
    pub key_path: Option<String>,
    /// Explicit verified OpenSSH known_hosts source. Legacy None resolves to app data,
    /// never the process user's default SSH directory.
    #[serde(default)]
    pub known_hosts_path: Option<String>,
    #[serde(default)]
    pub host_key_policy: SshHostKeyPolicy,
    #[serde(default)]
    pub password: Option<String>,
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
    pub key_path: Option<String>,
    #[serde(default)]
    pub known_hosts_path: Option<String>,
    #[serde(default)]
    pub host_key_policy: SshHostKeyPolicy,
    pub status: SshStatus,
    pub last_error: Option<String>,
    pub latency_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateSshConnectionRequest {
    pub name: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub auth_type: SshAuthType,
    pub key_path: Option<String>,
    #[serde(default)]
    pub known_hosts_path: Option<String>,
    #[serde(default)]
    pub host_key_policy: SshHostKeyPolicy,
    pub password: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateSshConnectionRequest {
    pub id: String,
    pub name: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub auth_type: SshAuthType,
    pub key_path: Option<String>,
    /// None preserves the existing source; Some("") resets to the app default.
    #[serde(default)]
    pub known_hosts_path: Option<String>,
    /// Legacy callers preserve the saved policy instead of silently changing it.
    #[serde(default)]
    pub host_key_policy: Option<SshHostKeyPolicy>,
    pub password: Option<String>,
}

impl SshConnectionConfig {
    pub(crate) fn discard_agent_credentials(&mut self) {
        if matches!(
            self.auth_type,
            SshAuthType::Agent | SshAuthType::OpenSshAgent | SshAuthType::Pageant
        ) {
            self.key_path = None;
            self.password = None;
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
            known_hosts_path: None,
            host_key_policy: SshHostKeyPolicy::Strict,
            password: None,
            extra_options: HashMap::new(),
            control_path: None,
            cached_status: SshStatus::Disconnected,
            cached_last_error: None,
            cached_latency_ms: None,
            cached_checked_at_ms: None,
        }
    }
}
