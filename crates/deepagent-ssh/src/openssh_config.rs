//! Read-only, bounded import of the explicitly supported OpenSSH declarations.
use crate::{SshError, SshResult};
use serde::{Deserialize, Serialize};
use std::{io::Read, path::Path};

mod parser;
mod resolver;

const MAX_CONFIG_BYTES: usize = 1024 * 1024;

fn invalid(message: impl Into<String>) -> SshError {
    SshError::InvalidConfig(format!("SSH config import: {}", message.into()))
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SshConfigOverrides {
    pub host: Option<String>,
    pub port: Option<u16>,
    pub username: Option<String>,
    pub identity_files: Option<Vec<String>>,
    pub proxy_jump: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SshConfigImportedEndpoint {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub identity_files: Vec<String>,
    pub default_identities: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SshConfigImportedConnection {
    pub alias: String,
    pub target: SshConfigImportedEndpoint,
    pub jump: Option<SshConfigImportedEndpoint>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SshConfigImportPreview {
    pub path: String,
    pub aliases: Vec<String>,
    pub connection: Option<SshConfigImportedConnection>,
}

pub fn resolve_ssh_config(
    source: &str,
    home: &Path,
    local_user: &str,
    alias: Option<&str>,
    overrides: &SshConfigOverrides,
) -> SshResult<SshConfigImportPreview> {
    if !home.is_absolute() {
        return Err(invalid("requires an absolute home directory"));
    }
    let parsed = parser::ParsedConfig::parse(source)?;
    let connection = alias
        .map(|alias| resolver::resolve(&parsed, home, local_user, alias, overrides))
        .transpose()?;
    Ok(SshConfigImportPreview {
        path: String::new(),
        aliases: parsed.aliases,
        connection,
    })
}

pub fn preview_ssh_config_in(
    path: &Path,
    home: &Path,
    local_user: &str,
    alias: Option<&str>,
    overrides: &SshConfigOverrides,
) -> SshResult<SshConfigImportPreview> {
    if !path.is_absolute() {
        return Err(invalid("requires an absolute config path"));
    }
    let file =
        std::fs::File::open(path).map_err(|error| invalid(format!("open config: {error}")))?;
    let metadata = file
        .metadata()
        .map_err(|error| invalid(format!("config metadata: {error}")))?;
    if !metadata.is_file() || metadata.len() > MAX_CONFIG_BYTES as u64 {
        return Err(invalid("config must be a regular file of at most 1 MiB"));
    }
    let mut bytes = Vec::new();
    file.take((MAX_CONFIG_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| invalid(format!("read config: {error}")))?;
    if bytes.len() > MAX_CONFIG_BYTES {
        return Err(invalid("config exceeds 1 MiB"));
    }
    let source = std::str::from_utf8(&bytes).map_err(|_| invalid("config must be UTF-8"))?;
    let mut preview = resolve_ssh_config(source, home, local_user, alias, overrides)?;
    preview.path = path
        .to_str()
        .ok_or_else(|| invalid("config path is not Unicode"))?
        .to_owned();
    Ok(preview)
}

/// Parse only the selected file. Includes, commands and private identities are never opened.
pub async fn preview_ssh_config(
    path: Option<String>,
    alias: Option<String>,
    overrides: SshConfigOverrides,
) -> SshResult<SshConfigImportPreview> {
    tokio::task::spawn_blocking(move || {
        let home = dirs::home_dir().ok_or_else(|| invalid("home directory is unavailable"))?;
        let path = path
            .filter(|path| !path.trim().is_empty())
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| home.join(".ssh/config"));
        let local_user =
            std::env::var(if cfg!(windows) { "USERNAME" } else { "USER" }).unwrap_or_default();
        preview_ssh_config_in(&path, &home, &local_user, alias.as_deref(), &overrides)
    })
    .await
    .map_err(|error| SshError::Internal(format!("SSH config import worker: {error}")))?
}
