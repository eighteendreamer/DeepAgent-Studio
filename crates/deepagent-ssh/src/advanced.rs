//! Explicit, per-connection extensions. Saving options never opens a listener or mutates Agent.
use crate::{SshAgentProvider, SshError, SshResult};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SshAdvancedOptions {
    pub forward_agent: bool,
    pub agent_provider: SshAgentProvider,
    pub allow_legacy_algorithms: bool,
    pub x11: Option<crate::x11::SshX11Options>,
}

impl SshAdvancedOptions {
    pub(crate) fn validate(&self) -> SshResult<()> {
        if let Some(x11) = &self.x11 {
            x11.validate()?;
        }
        Ok(())
    }

    pub(crate) fn authentication_hash(
        &self,
        algorithm: russh::keys::Algorithm,
        advertised: Option<Option<russh::keys::HashAlg>>,
    ) -> SshResult<Option<russh::keys::HashAlg>> {
        use russh::keys::{Algorithm, HashAlg};
        match algorithm {
            Algorithm::Dsa if !self.allow_legacy_algorithms => Err(SshError::InvalidConfig(
                "DSA authentication requires explicit legacy compatibility".into(),
            )),
            Algorithm::Rsa { .. } => match advertised {
                Some(Some(hash)) => Ok(Some(hash)),
                Some(None) if !self.allow_legacy_algorithms => Err(SshError::InvalidConfig(
                    "server only advertises RSA/SHA1; explicit legacy compatibility is required"
                        .into(),
                )),
                None if !self.allow_legacy_algorithms => Ok(Some(HashAlg::Sha512)),
                _ => Ok(None),
            },
            _ => Ok(None),
        }
    }

    pub(crate) fn protocol_config(
        &self,
        timing: crate::SshConnectionTiming,
    ) -> russh::client::Config {
        let mut config = timing.protocol_config();
        // russh includes RSA/SHA-1 in its own defaults. Keep weak algorithms behind our opt-in.
        let mut keys = config.preferred.key.into_owned();
        keys.retain(|key| {
            !matches!(
                key,
                russh::keys::Algorithm::Dsa | russh::keys::Algorithm::Rsa { hash: None }
            )
        });
        if self.allow_legacy_algorithms {
            let mut kex = config.preferred.kex.into_owned();
            let position = kex
                .iter()
                .position(|name| name == &russh::kex::EXTENSION_SUPPORT_AS_CLIENT)
                .unwrap_or(kex.len());
            kex.splice(
                position..position,
                [
                    russh::kex::DH_G14_SHA1,
                    russh::kex::DH_G1_SHA1,
                    russh::kex::DH_GEX_SHA1,
                ],
            );
            config.preferred.kex = Cow::Owned(kex);
            keys.push(russh::keys::Algorithm::Rsa { hash: None });
            keys.push(russh::keys::Algorithm::Dsa);
            let mut mac = config.preferred.mac.into_owned();
            mac.push(russh::mac::HMAC_SHA1);
            config.preferred.mac = Cow::Owned(mac);
        }
        config.preferred.key = Cow::Owned(keys);
        config
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SshNegotiatedAlgorithms {
    pub kex: String,
    pub host_key: String,
    pub cipher: String,
    pub client_mac: String,
    pub server_mac: String,
}

impl From<&russh::Names> for SshNegotiatedAlgorithms {
    fn from(names: &russh::Names) -> Self {
        Self {
            kex: names.kex.as_ref().to_owned(),
            host_key: names.key.to_string(),
            cipher: names.cipher.as_ref().to_owned(),
            client_mac: names.client_mac.as_ref().to_owned(),
            server_mac: names.server_mac.as_ref().to_owned(),
        }
    }
}

pub(crate) fn extension_error(stage: &'static str, reason: impl Into<String>) -> SshError {
    SshError::Forwarding {
        stage,
        reason: reason.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn weak_authentication_requires_opt_in_and_missing_extensions_never_trigger_sha1_fallback() {
        use russh::keys::{Algorithm, HashAlg};
        let modern = SshAdvancedOptions::default();
        assert!(modern.authentication_hash(Algorithm::Dsa, None).is_err());
        assert!(modern
            .authentication_hash(Algorithm::Rsa { hash: None }, Some(None))
            .is_err());
        assert_eq!(
            modern
                .authentication_hash(Algorithm::Rsa { hash: None }, None)
                .unwrap(),
            Some(HashAlg::Sha512)
        );
        assert_eq!(
            modern
                .authentication_hash(Algorithm::Rsa { hash: None }, Some(Some(HashAlg::Sha256)))
                .unwrap(),
            Some(HashAlg::Sha256)
        );
        let legacy = SshAdvancedOptions {
            allow_legacy_algorithms: true,
            ..Default::default()
        };
        assert_eq!(
            legacy
                .authentication_hash(Algorithm::Rsa { hash: None }, Some(None))
                .unwrap(),
            None
        );
        assert!(legacy.authentication_hash(Algorithm::Dsa, None).is_ok());
    }

    #[test]
    fn weak_algorithms_require_explicit_opt_in_and_modern_algorithms_stay_first() {
        let defaults = SshAdvancedOptions::default().protocol_config(Default::default());
        assert!(!defaults
            .preferred
            .key
            .contains(&russh::keys::Algorithm::Rsa { hash: None }));
        assert!(!defaults.preferred.kex.contains(&russh::kex::DH_G1_SHA1));
        assert!(!defaults.preferred.mac.contains(&russh::mac::HMAC_SHA1));
        let enabled = SshAdvancedOptions {
            allow_legacy_algorithms: true,
            ..Default::default()
        }
        .protocol_config(Default::default());
        assert_eq!(enabled.preferred.kex[0], defaults.preferred.kex[0]);
        assert!(enabled.preferred.kex.contains(&russh::kex::DH_G1_SHA1));
        assert!(enabled.preferred.mac.contains(&russh::mac::HMAC_SHA1));
    }
}
