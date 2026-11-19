//! Explicit credentials, opaque edits and secret-free projections for the common negotiator.
use crate::{SshAuthType, SshConnectionConfig, SshError, SshResult};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// Local configuration bound; navop bounds attempts rather than this stored factor count.
pub const MAX_AUTHENTICATION_FACTORS: usize = 6;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SshAuthenticationFactor {
    pub id: String,
    pub auth_type: SshAuthType,
    pub key_path: Option<String>,
    pub key_content: Option<String>,
    pub certificate_path: Option<String>,
    pub password: Option<String>,
    pub passphrase: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct SshAuthenticationFactorRequest {
    pub id: Option<String>,
    pub auth_type: SshAuthType,
    pub key_path: Option<String>,
    pub key_content: Option<String>,
    pub certificate_path: Option<String>,
    /// Omitted secrets preserve only a compatible factor from this connection.
    pub password: Option<String>,
    /// Some("") clears a saved passphrase and enables an attempt-local unlock.
    pub passphrase: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SshAuthenticationFactorDto {
    pub id: String,
    pub auth_type: SshAuthType,
    pub key_path: Option<String>,
    pub certificate_path: Option<String>,
    pub has_key_content: bool,
    pub has_password: bool,
    pub has_passphrase: bool,
}

impl std::fmt::Debug for SshAuthenticationFactor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SshAuthenticationFactor")
            .field("id", &self.id)
            .field("auth_type", &self.auth_type)
            .finish_non_exhaustive()
    }
}

impl std::fmt::Debug for SshAuthenticationFactorRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SshAuthenticationFactorRequest")
            .field("id", &self.id)
            .field("auth_type", &self.auth_type)
            .finish_non_exhaustive()
    }
}

impl SshAuthenticationFactor {
    pub(crate) fn dto(&self) -> SshAuthenticationFactorDto {
        SshAuthenticationFactorDto {
            id: self.id.clone(),
            auth_type: self.auth_type,
            key_path: self.key_path.clone(),
            certificate_path: self.certificate_path.clone(),
            has_key_content: self.key_content.is_some(),
            has_password: self.password.is_some(),
            has_passphrase: self.passphrase.is_some(),
        }
    }

    pub(crate) fn contains_secret(&self) -> bool {
        self.password.is_some() || self.passphrase.is_some() || self.key_content.is_some()
    }

    fn normalize(&mut self) {
        let secret = if self.auth_type == SshAuthType::Password {
            self.passphrase = None;
            &mut self.password
        } else {
            self.password = None;
            &mut self.passphrase
        };
        crate::config::normalize_credential(
            self.auth_type,
            &mut self.key_path,
            &mut self.key_content,
            &mut self.certificate_path,
            secret,
        );
    }
}

pub(crate) fn validate_factors(factors: &[SshAuthenticationFactor]) -> SshResult<()> {
    if factors.is_empty() || factors.len() > MAX_AUTHENTICATION_FACTORS {
        return Err(SshError::InvalidConfig(
            "SSH authentication chain requires 1 to 6 explicit factors".into(),
        ));
    }
    let mut ids = HashSet::new();
    for factor in factors {
        if factor.id.is_empty() || !ids.insert(&factor.id) {
            return Err(SshError::InvalidConfig(
                "SSH authentication factor IDs must be nonempty and unique".into(),
            ));
        }
        // navop explicit chains use passwords and concrete private identities;
        // dynamic Agent identity enumeration remains owned by the existing Agent strategy.
        if !matches!(
            factor.auth_type,
            SshAuthType::Password | SshAuthType::KeyFile | SshAuthType::KeyContent
        ) {
            return Err(SshError::InvalidConfig(
                "SSH authentication chain factors must be passwords or explicit private keys"
                    .into(),
            ));
        }
        crate::config::validate_credential(
            factor.auth_type,
            factor.key_path.as_deref(),
            factor.key_content.as_deref(),
            factor.password.as_deref(),
        )?;
    }
    Ok(())
}

pub(crate) fn resolve_factors(
    requests: Option<Vec<SshAuthenticationFactorRequest>>,
    existing: &[SshAuthenticationFactor],
) -> SshResult<Vec<SshAuthenticationFactor>> {
    let Some(requests) = requests else {
        return Ok(existing.to_vec());
    };
    if requests.len() > MAX_AUTHENTICATION_FACTORS {
        return Err(SshError::InvalidConfig(
            "SSH authentication chain exceeds 6 factors".into(),
        ));
    }
    let mut resolved = Vec::with_capacity(requests.len());
    for request in requests {
        let previous =
            match request.id.as_ref() {
                Some(id) => Some(existing.iter().find(|factor| &factor.id == id).ok_or_else(
                    || {
                        SshError::InvalidConfig(
                            "SSH authentication factor does not belong to this connection".into(),
                        )
                    },
                )?),
                None => None,
            };
        let compatible = previous.filter(|old| old.auth_type == request.auth_type);
        let key_path = request
            .key_path
            .or_else(|| compatible.and_then(|old| old.key_path.clone()));
        let key_content = request
            .key_content
            .or_else(|| compatible.and_then(|old| old.key_content.clone()));
        let same_key =
            compatible.filter(|old| old.key_path == key_path && old.key_content == key_content);
        let mut factor = SshAuthenticationFactor {
            id: previous
                .map(|old| old.id.clone())
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
            auth_type: request.auth_type,
            key_path,
            key_content,
            certificate_path: request
                .certificate_path
                .or_else(|| same_key.and_then(|old| old.certificate_path.clone())),
            password: request
                .password
                .or_else(|| compatible.and_then(|old| old.password.clone())),
            passphrase: request
                .passphrase
                .or_else(|| same_key.and_then(|old| old.passphrase.clone())),
        };
        factor.normalize();
        resolved.push(factor);
    }
    validate_factors(&resolved)?;
    Ok(resolved)
}

impl SshConnectionConfig {
    /// Reuse the existing password/key/certificate reader with one concrete factor.
    /// Trust, host and username remain those of the original verified connection.
    pub(crate) fn authentication_config(&self, index: usize) -> SshResult<Self> {
        let mut config = self.clone();
        if self.auth_type == SshAuthType::Chain {
            let factor = self.authentication_factors.get(index).ok_or_else(|| {
                SshError::InvalidConfig("SSH authentication factor is no longer available".into())
            })?;
            config.auth_type = factor.auth_type;
            config.key_path = factor.key_path.clone();
            config.key_content = factor.key_content.clone();
            config.certificate_path = factor.certificate_path.clone();
            config.password = if factor.auth_type == SshAuthType::Password {
                factor.password.clone()
            } else {
                factor.passphrase.clone()
            };
            config.authentication_factors.clear();
        }
        Ok(config)
    }
}
