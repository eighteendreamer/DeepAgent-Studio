//! Strict host identity boundary for every SSH handshake and live-client reuse.
//!
//! Trust enrollment is deliberately external: an administrator's fingerprint must
//! be independently verified before the user supplies a known_hosts entry.

use crate::{SshConnectionConfig, SshError, SshResult};
use russh::keys::{HashAlg, PublicKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::Path;

/// A key already recorded in the selected local trust source, not a remote verdict.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SshKnownHostKey {
    pub line_number: usize,
    pub algorithm: String,
    pub fingerprint: String,
}

/// Read-only view for both connection editors. Contains no authentication material.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SshHostTrustInspection {
    pub host: String,
    pub port: u16,
    pub known_hosts_path: String,
    pub keys: Vec<SshKnownHostKey>,
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct HostTrust {
    pub(crate) path: String,
    digest: [u8; 32],
    keys: Vec<(usize, PublicKey)>,
}

impl HostTrust {
    pub(crate) async fn load(config: &SshConnectionConfig) -> SshResult<Self> {
        let config = config.clone();
        tokio::task::spawn_blocking(move || Self::read(&config))
            .await
            .map_err(|error| SshError::Internal(format!("SSH trust reader failed: {error}")))?
    }

    fn read(config: &SshConnectionConfig) -> SshResult<Self> {
        let path = config.known_hosts_path.as_deref().unwrap_or_default();
        if !Path::new(path).is_absolute() {
            return Err(Self::error(config, "known_hosts requires an absolute path"));
        }
        let bytes = std::fs::read(path).map_err(|error| {
            Self::error(
                config,
                &format!("known_hosts is unavailable ({:?})", error.kind()),
            )
        })?;
        let text = std::str::from_utf8(&bytes)
            .map_err(|_| Self::error(config, "known_hosts must be UTF-8"))?;
        // russh 0.55 does not implement OpenSSH marker semantics. In particular,
        // ignoring @revoked next to an ordinary entry could accept a revoked key.
        if text.lines().any(|line| line.trim_start().starts_with('@')) {
            return Err(Self::error(config, "known_hosts markers (@revoked/@cert-authority) are unsupported; use a dedicated verified plain/hashed-host file"));
        }
        // Reuse russh's host+port and hashed-host rules; do not invent a second parser.
        let keys = russh::keys::known_hosts::known_host_keys_path(&config.host, config.port, path)
            .map_err(|_| Self::error(config, "known_hosts contains an invalid matching key"))?;
        if keys.is_empty() {
            return Err(Self::error(
                config,
                "unknown host: no verified host+port entry",
            ));
        }
        // russh 0.55's reported counter skips '#' comment lines. Adapt source
        // positions for display only; keep its key/host/port parser authoritative.
        let source_lines = text
            .lines()
            .enumerate()
            .filter(|(_, line)| !line.starts_with('#'))
            .map(|(index, _)| index + 1)
            .collect::<Vec<_>>();
        let keys = keys
            .into_iter()
            .map(|(line_number, key)| {
                let line_number = source_lines
                    .get(line_number.saturating_sub(1))
                    .copied()
                    .ok_or_else(|| Self::error(config, "trust source changed during inspection"))?;
                Ok((line_number, key))
            })
            .collect::<SshResult<Vec<_>>>()?;
        Ok(Self {
            path: path.to_owned(),
            digest: Sha256::digest(bytes).into(),
            keys,
        })
    }

    pub(crate) fn inspection(self, config: &SshConnectionConfig) -> SshHostTrustInspection {
        SshHostTrustInspection {
            host: config.host.clone(),
            port: config.port,
            known_hosts_path: self.path,
            // Fingerprinting is UI inspection work, not a tax on every PTY read
            // that revalidates an already connected client's trust source.
            keys: self
                .keys
                .into_iter()
                .map(|(line_number, key)| SshKnownHostKey {
                    line_number,
                    algorithm: key.algorithm().as_str().to_owned(),
                    fingerprint: key.fingerprint(HashAlg::Sha256).to_string(),
                })
                .collect(),
        }
    }

    pub(crate) fn error(config: &SshConnectionConfig, reason: &str) -> SshError {
        SshError::HostKeyVerification {
            host: config.host.clone(),
            port: config.port,
            known_hosts_path: config.known_hosts_path.clone().unwrap_or_default(),
            reason: reason.to_owned(),
        }
    }

    pub(crate) fn accepts(&self, presented: &PublicKey) -> bool {
        // Preserve the existing russh 0.55 fail-closed semantics for conflicting
        // same-algorithm entries, but compare the captured keys, never reopen a path.
        self.keys.iter().any(|(_, key)| key == presented)
            && self
                .keys
                .iter()
                .all(|(_, key)| key.algorithm() != presented.algorithm() || key == presented)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SshAuthType;
    use russh::keys::{ssh_key::rand_core::OsRng, Algorithm, PrivateKey};

    #[test]
    fn h125_known_hosts_hash_port_and_marker_contracts() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("known_hosts");
        let public = PrivateKey::random(&mut OsRng, Algorithm::Ed25519)
            .unwrap()
            .public_key()
            .to_openssh()
            .unwrap();
        let mut cfg =
            SshConnectionConfig::new("fixture", "127.0.0.1", 22, "fixture", SshAuthType::Password);
        cfg.known_hosts_path = Some(path.to_str().unwrap().into());
        // Deterministic fixture HMAC-SHA1 hashes, independently generated with node:crypto.
        for (port, pattern) in [
            (
                22,
                "|1|aDEyNS10ZXN0LW9ubHktc2FsdA==|BaHjvW3QDT36o/gmQ/OjXFd7354=",
            ),
            (
                23456,
                "|1|aDEyNS10ZXN0LW9ubHktc2FsdA==|lzKxCEXKXbYD59XslOlwvKoP+ds=",
            ),
            (22, "127.0.0.1,fixture.invalid"),
        ] {
            cfg.port = port;
            std::fs::write(&path, format!("{pattern} {public}\n")).unwrap();
            assert!(
                HostTrust::read(&cfg).is_ok(),
                "hashed/plain host must match"
            );
            cfg.port = port + 1;
            assert!(HostTrust::read(&cfg).is_err(), "trust is port-scoped");
        }
        cfg.port = 22;
        std::fs::write(
            &path,
            format!("127.0.0.1 {public}\n@revoked 127.0.0.1 {public}\n"),
        )
        .unwrap();
        assert!(
            HostTrust::read(&cfg).is_err(),
            "never ignore revoked markers"
        );
        for invalid in ["", "# comment only\n", "127.0.0.1 ssh-ed25519 not-a-key\n"] {
            std::fs::write(&path, invalid).unwrap();
            assert!(HostTrust::read(&cfg).is_err());
        }
        std::fs::remove_file(&path).unwrap();
        assert!(HostTrust::read(&cfg).is_err());
    }
}
