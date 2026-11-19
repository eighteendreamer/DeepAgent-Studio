use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use serde::Serialize;
use tokio::sync::watch;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::session::SshSession;
use crate::timing::ChallengeDeadline;
use crate::transport::{PendingClientAuthentication, SshAuthenticationRound};
use crate::{SshConnectionConfig, SshError, SshResult};

const CHALLENGE_TTL: Duration = Duration::from_secs(120);
const MAX_ATTEMPTS: usize = 128;

#[derive(Debug, Clone, Serialize)]
pub struct SshAuthenticationPrompt {
    pub prompt: String,
    pub echo: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SshAuthenticationKind {
    KeyboardInteractive,
    PrivateKey,
}

#[derive(Debug, Clone, Serialize)]
pub struct SshAuthenticationChallenge {
    pub role: crate::SshConnectionRole,
    pub kind: SshAuthenticationKind,
    pub connection_id: String,
    pub attempt_id: String,
    pub challenge_id: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub name: String,
    pub instructions: String,
    pub prompts: Vec<SshAuthenticationPrompt>,
    pub expires_at_ms: i64,
}

pub(crate) struct PendingAuthenticationState {
    pub challenge: SshAuthenticationChallenge,
    pub config_proof: [u8; 32],
    pub deadline: ChallengeDeadline,
    pub elapsed: Duration,
    pub client: Option<PendingClientAuthentication>,
}

pub(crate) struct PendingAuthentication {
    pub(crate) attempt_id: String,
    pub(crate) active_challenge_id: RwLock<String>,
    session: Arc<SshSession>,
    pub state: tokio::sync::Mutex<PendingAuthenticationState>,
    pub(crate) cancelled: CancellationToken,
    expiry: CancellationToken,
    waiting_timeout: Option<Duration>,
    waiting_until: watch::Sender<Option<Instant>>,
}

impl PendingAuthentication {
    pub(crate) fn expiration(&self, deadline: ChallengeDeadline) -> Option<SshError> {
        if deadline.instant <= Instant::now() {
            Some(stale())
        } else if self
            .waiting_until
            .borrow()
            .is_some_and(|until| until <= Instant::now())
        {
            Some(SshError::InvalidConfig(
                "SSH inactivity timeout while waiting for authentication".into(),
            ))
        } else {
            None
        }
    }

    pub(crate) fn resume_network(&self) {
        self.waiting_until.send_replace(None);
    }

    pub(crate) fn await_user(&self, deadline: ChallengeDeadline) -> ChallengeDeadline {
        let waiting_until = self
            .waiting_timeout
            .map(|duration| Instant::now() + duration);
        self.waiting_until.send_replace(waiting_until);
        waiting_until.map_or(deadline, |until| deadline.capped_at(until))
    }

    async fn fail_and_close(&self, error: &SshError) {
        self.session
            .fail_attempt_if_current(&self.attempt_id, error.to_string())
            .await;
        self.cancel_and_close().await;
    }

    async fn cancel_and_close(&self) {
        self.cancelled.cancel();
        self.expiry.cancel();
        let mut state = self.state.lock().await;
        if let Some(mut client) = state.client.take() {
            client.disconnect().await;
        }
    }
}

#[derive(Default)]
pub(crate) struct AuthenticationChallengeRegistry(
    Mutex<HashMap<String, Arc<PendingAuthentication>>>,
);

impl AuthenticationChallengeRegistry {
    pub(crate) async fn present(
        self: &Arc<Self>,
        config: &SshConnectionConfig,
        attempt_id: &str,
        config_proof: [u8; 32],
        elapsed: Duration,
        session: Arc<SshSession>,
        client: PendingClientAuthentication,
        round: SshAuthenticationRound,
        deadline: Option<ChallengeDeadline>,
    ) -> SshResult<SshAuthenticationChallenge> {
        let deadline = deadline.unwrap_or_else(|| ChallengeDeadline::new(CHALLENGE_TTL));
        if deadline.instant <= Instant::now() {
            let mut client = client;
            client.disconnect().await;
            return Err(stale());
        }
        // russh waits inside its MFA packet handler, suspending the protocol
        // loop's native inactivity timer. The existing challenge expiry task
        // owns this user-wait boundary; native timers own active network work.
        let waiting_timeout = config
            .timing
            .inactivity_timeout_secs
            .map(|seconds| Duration::from_secs(u64::from(seconds)));
        let waiting_until = waiting_timeout.map(|duration| Instant::now() + duration);
        let (waiting_until, mut waiting_updates) = watch::channel(waiting_until);
        let challenge_deadline = waiting_until
            .borrow()
            .map_or(deadline, |until| deadline.capped_at(until));
        let challenge = challenge(config, attempt_id, challenge_deadline, round, client.role)?;
        let pending = Arc::new(PendingAuthentication {
            attempt_id: attempt_id.to_owned(),
            active_challenge_id: RwLock::new(challenge.challenge_id.clone()),
            session,
            state: tokio::sync::Mutex::new(PendingAuthenticationState {
                challenge: challenge.clone(),
                config_proof,
                deadline,
                elapsed,
                client: Some(client),
            }),
            cancelled: CancellationToken::new(),
            expiry: CancellationToken::new(),
            waiting_timeout,
            waiting_until,
        });

        let (previous, at_capacity) = {
            let mut entries = self.0.lock().map_err(|_| stale())?;
            if !entries.contains_key(&config.id) && entries.len() >= MAX_ATTEMPTS {
                (None, true)
            } else {
                (
                    entries.insert(config.id.clone(), Arc::clone(&pending)),
                    false,
                )
            }
        };
        if at_capacity {
            pending.cancel_and_close().await;
            return Err(SshError::InvalidConfig(
                "too many pending SSH authentication challenges".into(),
            ));
        }
        if let Some(previous) = previous {
            previous.cancel_and_close().await;
        }

        let registry = Arc::downgrade(self);
        let pending_for_expiry = Arc::clone(&pending);
        let connection_id = config.id.clone();
        tokio::spawn(async move {
            loop {
                let until = (*waiting_updates.borrow_and_update())
                    .map_or(deadline.instant, |until| until.min(deadline.instant));
                tokio::select! {
                    biased;
                    _ = pending_for_expiry.expiry.cancelled() => break,
                    _ = waiting_updates.changed() => continue,
                    _ = tokio::time::sleep_until(until) => {
                        let Some(error) = pending_for_expiry.expiration(deadline) else {
                            continue;
                        };
                        if let Some(registry) = registry.upgrade() {
                            if registry.remove_if_current(&connection_id, &pending_for_expiry) {
                                pending_for_expiry.fail_and_close(&error).await;
                            }
                        } else {
                            pending_for_expiry.cancel_and_close().await;
                        }
                        break;
                    }
                }
            }
        });

        Ok(challenge)
    }

    pub(crate) async fn get(
        &self,
        connection_id: &str,
        attempt_id: &str,
        challenge_id: &str,
    ) -> SshResult<Arc<PendingAuthentication>> {
        let pending = self
            .0
            .lock()
            .map_err(|_| stale())?
            .get(connection_id)
            .cloned()
            .ok_or_else(stale)?;
        if pending.attempt_id != attempt_id
            || *pending.active_challenge_id.read().map_err(|_| stale())? != challenge_id
        {
            return Err(stale());
        }
        let state = pending.state.lock().await;
        if let Some(error) = pending.expiration(state.deadline) {
            drop(state);
            if self.remove_if_current(connection_id, &pending) {
                pending.fail_and_close(&error).await;
            }
            return Err(error);
        }
        drop(state);
        Ok(pending)
    }

    pub(crate) async fn cancel(
        &self,
        connection_id: &str,
        attempt_id: &str,
        challenge_id: &str,
    ) -> SshResult<()> {
        let pending = self
            .0
            .lock()
            .map_err(|_| stale())?
            .get(connection_id)
            .cloned()
            .ok_or_else(stale)?;
        if pending.attempt_id != attempt_id
            || *pending.active_challenge_id.read().map_err(|_| stale())? != challenge_id
        {
            return Err(stale());
        }
        if !self.remove_if_current(connection_id, &pending) {
            return Err(stale());
        }
        pending.cancel_and_close().await;
        Ok(())
    }

    pub(crate) fn is_current(
        &self,
        connection_id: &str,
        pending: &Arc<PendingAuthentication>,
    ) -> bool {
        self.0
            .lock()
            .ok()
            .and_then(|entries| entries.get(connection_id).cloned())
            .is_some_and(|current| Arc::ptr_eq(&current, pending))
    }

    pub(crate) fn remove_if_current(
        &self,
        connection_id: &str,
        pending: &Arc<PendingAuthentication>,
    ) -> bool {
        let Ok(mut entries) = self.0.lock() else {
            return false;
        };
        if entries
            .get(connection_id)
            .is_some_and(|current| Arc::ptr_eq(current, pending))
        {
            entries.remove(connection_id);
            pending.expiry.cancel();
            true
        } else {
            false
        }
    }

    pub(crate) async fn close_if_current(
        &self,
        connection_id: &str,
        pending: &Arc<PendingAuthentication>,
    ) -> bool {
        if !self.remove_if_current(connection_id, pending) {
            return false;
        }
        pending.cancel_and_close().await;
        true
    }

    pub(crate) async fn invalidate(&self, connection_id: &str) {
        let pending = self
            .0
            .lock()
            .ok()
            .and_then(|mut entries| entries.remove(connection_id));
        if let Some(pending) = pending {
            pending.cancel_and_close().await;
        }
    }

    pub(crate) async fn clear(&self) {
        let pending = self
            .0
            .lock()
            .map(|mut entries| {
                entries
                    .drain()
                    .map(|(_, pending)| pending)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        for pending in pending {
            pending.cancel_and_close().await;
        }
    }
}

pub(crate) fn next_challenge(
    config: &SshConnectionConfig,
    attempt_id: &str,
    deadline: ChallengeDeadline,
    round: SshAuthenticationRound,
    role: crate::SshConnectionRole,
) -> SshResult<SshAuthenticationChallenge> {
    challenge(config, attempt_id, deadline, round, role)
}

fn challenge(
    config: &SshConnectionConfig,
    attempt_id: &str,
    deadline: ChallengeDeadline,
    round: SshAuthenticationRound,
    role: crate::SshConnectionRole,
) -> SshResult<SshAuthenticationChallenge> {
    let active = role.config(config)?;
    Ok(SshAuthenticationChallenge {
        role,
        kind: round.kind,
        connection_id: config.id.clone(),
        attempt_id: attempt_id.to_owned(),
        challenge_id: Uuid::new_v4().to_string(),
        host: active.host,
        port: active.port,
        username: active.username,
        name: round.name,
        instructions: round.instructions,
        prompts: round.prompts,
        expires_at_ms: deadline.expires_at_ms,
    })
}

fn stale() -> SshError {
    SshError::InvalidConfig(
        "SSH authentication challenge is expired, cancelled, superseded or already answered; start a new connection attempt".into(),
    )
}

pub(crate) fn stale_error() -> SshError {
    stale()
}
