//! Short-lived, connection-scoped host-key enrollment; not a runtime approval store.
//! No live TCP handle, authentication secret or user response is retained here.

use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;
use std::time::Duration;

use deepagent_core::clock::{Clock, SystemClock};
use russh::keys::{HashAlg, PublicKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::time::Instant;
use uuid::Uuid;

use crate::host_trust::HostTrust;
use crate::{SshConnectionConfig, SshError, SshResult};

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
    ConfirmationRequired { challenge: Box<SshHostKeyChallenge> },
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
}

struct Attempt {
    id: String,
    deadline: Instant,
    pending: Option<PendingHostKey>,
}

#[derive(Default)]
pub(crate) struct HostKeyChallenges(Mutex<HashMap<String, Attempt>>);

impl HostKeyChallenges {
    pub fn reserve(&self, id: &str) -> SshResult<String> {
        let mut attempts = self.0.lock().map_err(|_| stale())?;
        attempts.retain(|_, attempt| attempt.deadline > Instant::now());
        if !attempts.contains_key(id) && attempts.len() >= MAX_ATTEMPTS {
            return Err(SshError::InvalidConfig(
                "too many pending SSH connection attempts".into(),
            ));
        }
        let attempt_id = Uuid::new_v4().to_string();
        attempts.insert(
            id.into(),
            Attempt {
                id: attempt_id.clone(),
                deadline: Instant::now() + CHALLENGE_TTL,
                pending: None,
            },
        );
        Ok(attempt_id)
    }

    pub fn present(
        &self,
        cfg: &SshConnectionConfig,
        attempt_id: &str,
        trust: HostTrust,
        key: PublicKey,
        can_save: bool,
    ) -> SshResult<SshHostKeyChallenge> {
        let mut attempts = self.0.lock().map_err(|_| stale())?;
        let attempt = attempts
            .get_mut(&cfg.id)
            .filter(|attempt| attempt.id == attempt_id && attempt.deadline > Instant::now())
            .ok_or_else(stale)?;
        let challenge = SshHostKeyChallenge {
            connection_id: cfg.id.clone(),
            attempt_id: attempt_id.into(),
            challenge_id: Uuid::new_v4().to_string(),
            host: cfg.host.clone(),
            port: cfg.port,
            username: cfg.username.clone(),
            role: "target",
            route: "direct",
            kind: "unknown_host",
            algorithm: key.algorithm().as_str().into(),
            fingerprint: key.fingerprint(HashAlg::Sha256).to_string(),
            expires_at_ms: SystemClock.now().as_millis()
                + attempt
                    .deadline
                    .saturating_duration_since(Instant::now())
                    .as_millis() as i64,
            can_save,
        };
        attempt.pending = Some(PendingHostKey {
            challenge: challenge.clone(),
            trust,
            key,
            config_proof: config_proof(cfg)?,
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
            .get(id)
            .filter(|attempt| attempt.id == attempt_id)
            .ok_or_else(stale)?;
        if attempt.deadline <= Instant::now() {
            attempts.remove(id);
            return Err(stale());
        }
        if attempt.pending.as_ref().map_or(true, |pending| {
            pending.challenge.challenge_id != challenge_id
        }) {
            return Err(stale());
        }
        attempts
            .remove(id)
            .and_then(|attempt| attempt.pending)
            .ok_or_else(stale)
    }

    pub fn ensure_current(&self, id: &str, attempt_id: &str) -> SshResult<()> {
        let attempts = self.0.lock().map_err(|_| stale())?;
        attempts
            .get(id)
            .filter(|attempt| attempt.id == attempt_id && attempt.deadline > Instant::now())
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
        &cfg.known_hosts_path,
        &cfg.password,
        options,
    ))
    .map_err(|_| stale())?;
    Ok(Sha256::digest(bytes).into())
}

fn stale() -> SshError {
    SshError::InvalidConfig("SSH host-key challenge is expired, cancelled, superseded or already answered; start a new connection attempt".into())
}
