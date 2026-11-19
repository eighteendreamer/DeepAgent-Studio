//! Both private-key input sources share decoding and certificate validation.

use russh::keys::{self, Certificate, PrivateKey};

use crate::{SshAuthType, SshConnectionConfig, SshError, SshResult};

pub(crate) struct PrivateIdentity {
    pub key: PrivateKey,
    pub certificate: Option<Certificate>,
}

/// None requests a local, attempt-scoped passphrase; no credential is offered.
pub(crate) async fn read_private_identity(
    config: &SshConnectionConfig,
    temporary_passphrase: Option<&str>,
) -> SshResult<Option<PrivateIdentity>> {
    let config = config.clone();
    let passphrase = temporary_passphrase
        .map(str::to_owned)
        .or_else(|| config.password.clone());
    tokio::task::spawn_blocking(move || {
        let content = match config.auth_type {
            SshAuthType::KeyFile => std::fs::read_to_string(
                config
                    .key_path
                    .as_deref()
                    .ok_or_else(|| SshError::InvalidConfig("key path is required".into()))?,
            )
            .map_err(|error| {
                SshError::AuthFailed(
                    config.username.clone(),
                    format!("read SSH private key: {error}"),
                )
            })?,
            SshAuthType::KeyContent => config
                .key_content
                .clone()
                .ok_or_else(|| SshError::InvalidConfig("private-key content is required".into()))?,
            _ => {
                return Err(SshError::InvalidConfig(
                    "authentication does not use a private key".into(),
                ))
            }
        };
        let key = match keys::decode_secret_key(&content, passphrase.as_deref()) {
            Ok(key) => key,
            Err(keys::Error::KeyIsEncrypted) if passphrase.is_none() => return Ok(None),
            Err(_)
                if passphrase.is_none()
                    && content
                        .trim_start()
                        .starts_with("-----BEGIN ENCRYPTED PRIVATE KEY-----") =>
            {
                return Ok(None)
            }
            Err(error) => {
                return Err(SshError::AuthFailed(
                    config.username.clone(),
                    format!("read SSH private key: {error}"),
                ))
            }
        };
        let certificate = config
            .certificate_path
            .as_deref()
            .map(|path| {
                let certificate = keys::load_openssh_certificate(path).map_err(|error| {
                    SshError::AuthFailed(
                        config.username.clone(),
                        format!("read SSH user certificate: {error}"),
                    )
                })?;
                if certificate.cert_type() != keys::ssh_key::certificate::CertType::User {
                    return Err(SshError::InvalidConfig(
                        "SSH authentication requires a user certificate".into(),
                    ));
                }
                if certificate.public_key() != key.public_key().key_data() {
                    return Err(SshError::InvalidConfig(
                        "SSH user certificate does not match the configured private key".into(),
                    ));
                }
                Ok(certificate)
            })
            .transpose()?;
        Ok(Some(PrivateIdentity { key, certificate }))
    })
    .await
    .map_err(|error| SshError::Internal(format!("SSH identity reader: {error}")))?
}
