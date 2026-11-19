//! Short-lived, connection-scoped host-key enrollment; not a runtime approval store.
//! Target confirmation may retain its verified jump route, never credentials or answers.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use russh::keys::{HashAlg, PublicKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::time::Instant;
use uuid::Uuid;

use crate::host_trust::HostTrust;
use crate::timing::ChallengeDeadline;
use crate::{SshAuthenticationChallenge, SshConnectionConfig, SshError, SshResult};

const CHALLENGE_TTL: Duration = Duration::from_secs(120);
const MAX_ATTEMPTS: usize = 128;

#[derive(Debug, Clone, Serialize)]
pub struct SshHostKeyChallenge {
    pub connection_id: String,
    pub attempt_id: String,
    pub challenge_id: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub role: &'static str,
    pub route: &'static str,
    pub proxy: Option<crate::SshProxyEndpoint>,
    pub jump: Option<crate::SshJumpEndpoint>,
    pub kind: &'static str,
    pub algorithm: String,
    pub fingerprint: String,
    pub expires_at_ms: i64,
    pub can_save: bool,
}

#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum SshConnectionAttempt {
    Connected,
    ConfirmationRequired {
        challenge: Box<SshHostKeyChallenge>,
    },
    AuthenticationRequired {
        challenge: Box<SshAuthenticationChallenge>,
    },
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SshHostKeyDecision {
    Once,
    Save,
}

pub(crate) struct PendingHostKey {
    pub challenge: SshHostKeyChallenge,
    pub trust: HostTrust,
    pub key: PublicKey,
    pub config_proof: [u8; 32],
    pub deadline: ChallengeDeadline,
    pub continuation: Option<crate::transport::PendingHostVerification>,
}

pub(crate) enum HostKeyPurpose {
    Enrollment { can_save: bool },
    Rotation,
}

struct Attempt {
    id: String,
    deadline: ChallengeDeadline,
    pending: Option<PendingHostKey>,
    expiry: tokio_util::sync::CancellationToken,
}

impl Drop for Attempt {
    fn drop(&mut self) {
        self.expiry.cancel();
    }
}

#[derive(Default)]
pub(crate) struct HostKeyChallenges(Arc<Mutex<HashMap<String, Attempt>>>);

impl HostKeyChallenges {
    pub fn reserve(&self, id: &str) -> SshResult<String> {
        let mut attempts = self.0.lock().map_err(|_| stale())?;
        attempts.retain(|_, attempt| attempt.deadline.instant > Instant::now());
        if !attempts.contains_key(id) && attempts.len() >= MAX_ATTEMPTS {
            return Err(SshError::InvalidConfig(
                "too many pending SSH connection attempts".into(),
            ));
        }
        let attempt_id = Uuid::new_v4().to_string();
        let expiry = tokio_util::sync::CancellationToken::new();
        attempts.insert(
            id.into(),
            Attempt {
                id: attempt_id.clone(),
                deadline: ChallengeDeadline::new(CHALLENGE_TTL),
                pending: None,
                expiry: expiry.clone(),
            },
        );
        let deadline = attempts[id].deadline;
        let registry = Arc::downgrade(&self.0);
        let connection_id = id.to_owned();
        let current_attempt = attempt_id.clone();
        tokio::spawn(async move {
            tokio::select! {
                _ = expiry.cancelled() => return,
                _ = tokio::time::sleep_until(deadline.instant) => {},
            }
            if let Some(registry) = registry.upgrade() {
                if let Ok(mut attempts) = registry.lock() {
                    if attempts
                        .get(&connection_id)
                        .is_some_and(|attempt| attempt.id == current_attempt)
                    {
                        attempts.remove(&connection_id);
                    }
                }
            }
        });
        Ok(attempt_id)
    }

    pub(crate) fn deadline(&self, id: &str, attempt_id: &str) -> SshResult<ChallengeDeadline> {
        self.0
            .lock()
            .map_err(|_| stale())?
            .get(id)
            .filter(|attempt| attempt.id == attempt_id && attempt.deadline.instant > Instant::now())
            .map(|attempt| attempt.deadline)
            .ok_or_else(stale)
    }

    pub(crate) fn present_target(
        &self,
        cfg: &SshConnectionConfig,
        attempt_id: &str,
        continuation: crate::transport::PendingHostVerification,
        can_save: bool,
    ) -> SshResult<SshHostKeyChallenge> {
        let challenge = self.present(
            cfg,
            attempt_id,
            continuation.trust.clone(),
            continuation.key.clone(),
            HostKeyPurpose::Enrollment { can_save },
        )?;
        let mut attempts = self.0.lock().map_err(|_| stale())?;
        let pending = attempts
            .get_mut(&cfg.id)
            .and_then(|attempt| attempt.pending.as_mut())
            .filter(|pending| pending.challenge.challenge_id == challenge.challenge_id)
            .ok_or_else(stale)?;
        pending.continuation = Some(continuation);
        Ok(challenge)
    }

    pub fn present(
        &self,
        cfg: &SshConnectionConfig,
        attempt_id: &str,
        trust: HostTrust,
        key: PublicKey,
        purpose: HostKeyPurpose,
    ) -> SshResult<SshHostKeyChallenge> {
        self.present_for_role(
            cfg,
            attempt_id,
            trust,
            key,
            purpose,
            crate::SshConnectionRole::Target,
        )
    }

    pub(crate) fn present_for_role(
        &self,
        cfg: &SshConnectionConfig,
        attempt_id: &str,
        trust: HostTrust,
        key: PublicKey,
        purpose: HostKeyPurpose,
        role: crate::SshConnectionRole,
    ) -> SshResult<SshHostKeyChallenge> {
        let active = role.config(cfg)?;
        let mut attempts = self.0.lock().map_err(|_| stale())?;
        let attempt = attempts
            .get_mut(&cfg.id)
            .filter(|attempt| attempt.id == attempt_id && attempt.deadline.instant > Instant::now())
            .ok_or_else(stale)?;
        let (kind, can_save) = match purpose {
            HostKeyPurpose::Enrollment { can_save } => ("unknown_host", can_save),
            HostKeyPurpose::Rotation => ("key_rotation", true),
        };
        let challenge = SshHostKeyChallenge {
            connection_id: cfg.id.clone(),
            attempt_id: attempt_id.into(),
            challenge_id: Uuid::new_v4().to_string(),
            host: active.host,
            port: active.port,
            username: active.username,
            role: match role {
                crate::SshConnectionRole::Target => "target",
                crate::SshConnectionRole::Jump => "jump",
            },
            route: cfg.route_label(),
            proxy: cfg.proxy.as_ref().map(crate::SshProxyConfig::endpoint),
            jump: cfg.jump.as_ref().map(crate::SshJumpConfig::endpoint),
            kind,
            algorithm: key.algorithm().as_str().into(),
            fingerprint: key.fingerprint(HashAlg::Sha256).to_string(),
            expires_at_ms: attempt.deadline.expires_at_ms,
            can_save,
        };
        attempt.pending = Some(PendingHostKey {
            challenge: challenge.clone(),
            trust,
            key,
            config_proof: config_proof(cfg)?,
            deadline: attempt.deadline,
            continuation: None,
        });
        Ok(challenge)
    }

    pub fn take(
        &self,
        id: &str,
        attempt_id: &str,
        challenge_id: &str,
    ) -> SshResult<PendingHostKey> {
        let mut attempts = self.0.lock().map_err(|_| stale())?;
        let attempt = attempts
            .get_mut(id)
            .filter(|attempt| attempt.id == attempt_id)
            .ok_or_else(stale)?;
        if attempt.deadline.instant <= Instant::now() {
            attempts.remove(id);
            return Err(stale());
        }
        if attempt.pending.as_ref().map_or(true, |pending| {
            pending.challenge.challenge_id != challenge_id
        }) {
            return Err(stale());
        }
        attempt.pending.take().ok_or_else(stale)
    }

    pub fn ensure_current(&self, id: &str, attempt_id: &str) -> SshResult<()> {
        let attempts = self.0.lock().map_err(|_| stale())?;
        attempts
            .get(id)
            .filter(|attempt| attempt.id == attempt_id && attempt.deadline.instant > Instant::now())
            .map(|_| ())
            .ok_or_else(stale)
    }

    pub fn discard(&self, id: &str, attempt_id: &str) {
        if let Ok(mut attempts) = self.0.lock() {
            if attempts
                .get(id)
                .is_some_and(|attempt| attempt.id == attempt_id)
            {
                attempts.remove(id);
            }
        }
    }

    pub fn invalidate(&self, id: &str) {
        if let Ok(mut attempts) = self.0.lock() {
            attempts.remove(id);
        }
    }

    pub fn clear(&self) {
        if let Ok(mut attempts) = self.0.lock() {
            attempts.clear();
        }
    }
}

pub(crate) fn config_proof(cfg: &SshConnectionConfig) -> SshResult<[u8; 32]> {
    // Credential-derived digest is process-local, never serialized or logged.
    let options: BTreeMap<_, _> = cfg.extra_options.iter().collect();
    let bytes = serde_json::to_vec(&(
        &cfg.host,
        cfg.port,
        &cfg.username,
        cfg.auth_type,
        &cfg.key_path,
        &cfg.key_content,
        &cfg.certificate_path,
        &cfg.known_hosts_path,
        cfg.host_key_policy,
        &cfg.password,
        &cfg.authentication_factors,
        &cfg.proxy,
        &cfg.jump,
        cfg.timing,
        options,
    ))
    .map_err(|_| stale())?;
    Ok(Sha256::digest(bytes).into())
}

fn stale() -> SshError {
    SshError::InvalidConfig("SSH host-key challenge is expired, cancelled, superseded or already answered; start a new connection attempt".into())
}
