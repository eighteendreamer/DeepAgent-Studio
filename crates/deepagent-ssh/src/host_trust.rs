//! Strict host identity boundary for every SSH handshake and live-client reuse.
//!
//! Trust enrollment is deliberately external: an administrator's fingerprint must
//! be independently verified before the user supplies a known_hosts entry.

use crate::host_key_store::StoreSnapshot;
use crate::{SshConnectionConfig, SshError, SshHostIdentity, SshHostKeyStore, SshResult};
use russh::keys::{HashAlg, PublicKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::Path;
use std::sync::Arc;

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
    digest: Option<[u8; 32]>,
    keys: Vec<(usize, PublicKey)>,
    application: Option<StoreSnapshot>,
    revoked: Vec<PublicKey>,
    confirmed_key: Option<PublicKey>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TrustRead {
    Strict,
    Enrollment,
    Rotation,
}

impl HostTrust {
    pub(crate) async fn load(config: &SshConnectionConfig) -> SshResult<Self> {
        Self::load_with_store(config, None).await
    }

    pub(crate) async fn load_with_store(
        config: &SshConnectionConfig,
        store: Option<Arc<SshHostKeyStore>>,
    ) -> SshResult<Self> {
        let config = config.clone();
        tokio::task::spawn_blocking(move || {
            Self::read_with_store(&config, store.as_deref(), TrustRead::Strict)
        })
        .await
        .map_err(|error| SshError::Internal(format!("SSH trust reader failed: {error}")))?
    }

    #[cfg(test)]
    fn read(config: &SshConnectionConfig) -> SshResult<Self> {
        Self::read_with_store(config, None, TrustRead::Strict)
    }

    fn read_with_store(
        config: &SshConnectionConfig,
        store: Option<&SshHostKeyStore>,
        mode: TrustRead,
    ) -> SshResult<Self> {
        let identity = SshHostIdentity::direct(&config.host, config.port)?;
        let application = store.map(|store| store.snapshot(&identity)).transpose()?;
        let application_keys = application
            .as_ref()
            .map(StoreSnapshot::keys)
            .transpose()?
            .unwrap_or_default();
        let path = config.known_hosts_path.as_deref().unwrap_or_default();
        if !Path::new(path).is_absolute() {
            return Err(Self::error(config, "known_hosts requires an absolute path"));
        }
        let bytes = match std::fs::read(path) {
            Ok(bytes) => Some(bytes),
            Err(error)
                if error.kind() == std::io::ErrorKind::NotFound
                    && (mode != TrustRead::Strict || !application_keys.is_empty()) =>
            {
                None
            }
            Err(error) => {
                return Err(Self::error(
                    config,
                    &format!("known_hosts is unavailable ({:?})", error.kind()),
                ))
            }
        };
        let text = std::str::from_utf8(bytes.as_deref().unwrap_or_default())
            .map_err(|_| Self::error(config, "known_hosts must be UTF-8"))?;
        // russh 0.55 does not implement OpenSSH marker semantics. In particular,
        // ignoring @revoked next to an ordinary entry could accept a revoked key.
        if text.lines().any(|line| line.trim_start().starts_with('@')) {
            return Err(Self::error(config, "known_hosts markers (@revoked/@cert-authority) are unsupported; use a dedicated verified plain/hashed-host file"));
        }
        // russh's path matcher skips incomplete lines and does not implement
        // wildcard/negated hosts. They must not silently become "unknown" for
        // TOFU or the explicit unknown-host confirmation flow. Validate with
        // its existing ssh-key parser, keeping host+port/hash matching below.
        for parsed in russh::keys::ssh_key::known_hosts::KnownHosts::new(text) {
            let entry = parsed
                .map_err(|_| Self::error(config, "known_hosts contains a malformed entry"))?;
            if let russh::keys::ssh_key::known_hosts::HostPatterns::Patterns(patterns) =
                entry.host_patterns()
            {
                if patterns
                    .iter()
                    .any(|pattern| pattern.contains(['*', '?', '!']))
                {
                    return Err(Self::error(config, "known_hosts wildcard/negated hosts are unsupported; use a dedicated verified plain/hashed-host file"));
                }
            }
        }
        // Reuse russh's host+port and hashed-host rules; do not invent a second parser.
        let keys = russh::keys::known_hosts::known_host_keys_path(&config.host, config.port, path)
            .map_err(|_| Self::error(config, "known_hosts contains an invalid matching key"))?;
        // russh 0.55's reported counter skips '#' comment lines. Adapt source
        // positions for display only; keep its key/host/port parser authoritative.
        let source_lines = text
            .lines()
            .enumerate()
            .filter(|(_, line)| !line.starts_with('#'))
            .map(|(index, _)| index + 1)
            .collect::<Vec<_>>();
        let mut keys = keys
            .into_iter()
            .map(|(line_number, key)| {
                let line_number = source_lines
                    .get(line_number.saturating_sub(1))
                    .copied()
                    .ok_or_else(|| Self::error(config, "trust source changed during inspection"))?;
                Ok((line_number, key))
            })
            .collect::<SshResult<Vec<_>>>()?;
        let mut revoked = Vec::new();
        for (key, is_revoked) in application_keys {
            if is_revoked {
                revoked.push(key);
            } else {
                keys.push((0, key)); // Application records have no file line number.
            }
        }
        if keys.is_empty()
            && (mode == TrustRead::Strict || (mode == TrustRead::Enrollment && !revoked.is_empty()))
        {
            return Err(Self::error(
                config,
                if revoked.is_empty() {
                    "unknown host: no verified host+port entry"
                } else {
                    "application trust has revoked this host key"
                },
            ));
        }
        Ok(Self {
            path: path.to_owned(),
            digest: bytes.map(|bytes| Sha256::digest(bytes).into()),
            keys,
            application,
            revoked,
            confirmed_key: None,
        })
    }

    pub(crate) async fn capture(
        config: &SshConnectionConfig,
        store: Option<Arc<SshHostKeyStore>>,
    ) -> SshResult<Self> {
        let config = config.clone();
        tokio::task::spawn_blocking(move || {
            Self::read_with_store(&config, store.as_deref(), TrustRead::Enrollment)
        })
        .await
        .map_err(|error| SshError::Internal(format!("SSH trust capture task: {error}")))?
    }

    pub(crate) async fn capture_rotation(
        config: &SshConnectionConfig,
        store: Arc<SshHostKeyStore>,
    ) -> SshResult<Self> {
        let config = config.clone();
        tokio::task::spawn_blocking(move || {
            Self::read_with_store(&config, Some(&store), TrustRead::Rotation)
        })
        .await
        .map_err(|error| SshError::Internal(format!("SSH rotation trust reader: {error}")))?
    }

    pub(crate) fn rotation_previous(
        &self,
        config: &SshConnectionConfig,
        replacement: &PublicKey,
    ) -> SshResult<Vec<(usize, PublicKey)>> {
        if self.is_unknown() || self.revoked.contains(replacement) || self.accepts(replacement) {
            return Err(Self::error(
                config,
                "rotation requires an existing identity and a changed, never-revoked replacement",
            ));
        }
        let mut previous = Vec::new();
        for (line, key) in &self.keys {
            if key != replacement
                && !self.revoked.contains(key)
                && !previous.iter().any(|(_, prior)| prior == key)
            {
                if previous.len() >= crate::host_key_store::MAX_KEYS - 1 {
                    return Err(Self::error(
                        config,
                        "rotation exceeds the application key capacity; no trust was changed",
                    ));
                }
                previous.push((*line, key.clone()));
            }
        }
        Ok(previous)
    }

    pub(crate) fn store_snapshot(&self) -> SshResult<StoreSnapshot> {
        self.application.clone().ok_or_else(|| {
            SshError::InvalidConfig("SSH rotation requires application trust".into())
        })
    }

    pub(crate) async fn reload(
        &self,
        config: &SshConnectionConfig,
        store: Option<Arc<SshHostKeyStore>>,
    ) -> SshResult<Self> {
        let mut current = if self.confirmed_key.is_some() {
            Self::capture(config, store).await?
        } else {
            Self::load_with_store(config, store).await?
        };
        current.confirmed_key = self.confirmed_key.clone();
        Ok(current)
    }

    pub(crate) fn is_unknown(&self) -> bool {
        self.keys.is_empty() && self.revoked.is_empty()
    }

    pub(crate) async fn persist_unknown(
        self,
        config: &SshConnectionConfig,
        store: Arc<SshHostKeyStore>,
        key: PublicKey,
    ) -> SshResult<Self> {
        if !self.is_unknown() || Self::capture(config, Some(store.clone())).await? != self {
            return Err(Self::error(config, "trust changed during AcceptNew probe"));
        }
        let expected = self.application.clone().ok_or_else(|| {
            Self::error(config, "AcceptNew requires an application trust snapshot")
        })?;
        let public = key.clone();
        let writer = store.clone();
        tokio::task::spawn_blocking(move || writer.trust_unknown(expected, &public))
            .await
            .map_err(|error| {
                SshError::Internal(format!("SSH AcceptNew persistence task: {error}"))
            })??;
        let persisted = Self::load_with_store(config, Some(store)).await?;
        if !persisted.same_file(&self) || !persisted.accepts(&key) {
            return Err(Self::error(
                config,
                "trust changed while saving an unknown key",
            ));
        }
        Ok(persisted.confirm_key(key))
    }

    pub(crate) fn confirm_key(mut self, key: PublicKey) -> Self {
        self.confirmed_key = Some(key);
        self
    }

    pub(crate) fn same_file(&self, other: &Self) -> bool {
        self.path == other.path && self.digest == other.digest
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

    pub(crate) fn public_keys(&self) -> Vec<PublicKey> {
        self.keys.iter().map(|(_, key)| key.clone()).collect()
    }

    pub(crate) fn accepts(&self, presented: &PublicKey) -> bool {
        // Preserve the existing russh 0.55 fail-closed semantics for conflicting
        // same-algorithm entries, but compare the captured keys, never reopen a path.
        if let Some(confirmed) = &self.confirmed_key {
            if presented != confirmed {
                return false;
            }
            if self.is_unknown() {
                return true;
            }
        }
        !self.revoked.iter().any(|key| key == presented)
            && self.keys.iter().any(|(_, key)| key == presented)
            && self.keys.iter().all(|(_, key)| {
                key.algorithm() != presented.algorithm()
                    || key == presented
                    || self.revoked.contains(key)
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SshAuthType;
    use russh::keys::{ssh_key::rand_core::OsRng, Algorithm, PrivateKey};

    #[test]
    fn rotation_preview_bounds_active_file_keys_before_presenting_an_impossible_plan() {
        let key = || {
            PrivateKey::random(&mut OsRng, Algorithm::Ed25519)
                .unwrap()
                .public_key()
                .clone()
        };
        let trust = HostTrust {
            path: "fixture".into(),
            digest: None,
            application: None,
            confirmed_key: None,
            keys: (0..crate::host_key_store::MAX_KEYS)
                .map(|line| (line + 1, key()))
                .collect(),
            revoked: Vec::new(),
        };
        let cfg = SshConnectionConfig::new(
            "fixture",
            "fixture.invalid",
            22,
            "fixture",
            SshAuthType::Agent,
        );
        assert!(
            trust.rotation_previous(&cfg, &key()).is_err(),
            "32 active old keys plus a replacement cannot fit; do not present an unbounded list"
        );
    }

    #[test]
    fn rotation_tombstone_disables_old_file_key_without_disabling_its_replacement() {
        let old = PrivateKey::random(&mut OsRng, Algorithm::Ed25519)
            .unwrap()
            .public_key()
            .clone();
        let new = PrivateKey::random(&mut OsRng, Algorithm::Ed25519)
            .unwrap()
            .public_key()
            .clone();
        let mut trust = HostTrust {
            path: "fixture".into(),
            digest: None,
            application: None,
            confirmed_key: None,
            keys: vec![(1, old.clone()), (0, new.clone())],
            revoked: vec![old.clone()],
        };
        assert!(
            !trust.accepts(&old),
            "the retired file entry must remain revoked"
        );
        assert!(
            trust.accepts(&new),
            "explicit rotation must not require editing the source file"
        );
        trust.revoked.clear();
        assert!(
            !trust.accepts(&new),
            "unrevoked conflicting file keys still fail closed"
        );
    }

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
