//! Resource ownership for one exact session generation, separate from input rights.
use crate::service::SshServiceImpl;
use crate::session::SshSession;
use crate::{SshError, SshResult, SshStatus};
use std::ops::Deref;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::time::Instant;

#[derive(Default)]
struct ResourceState {
    users: usize,
    idle_since: Option<Instant>,
    retired: bool,
}

#[derive(Default)]
pub(crate) struct ConnectionResources(Arc<Mutex<ResourceState>>);

impl ConnectionResources {
    pub(crate) fn acquire(&self) -> Option<ResourceLease> {
        let mut state = self.0.lock().unwrap_or_else(|error| error.into_inner());
        if state.retired {
            return None;
        }
        state.users = state.users.checked_add(1)?;
        state.idle_since = None;
        Some(ResourceLease(Arc::clone(&self.0)))
    }

    pub(crate) fn published(&self) {
        let mut state = self.0.lock().unwrap_or_else(|error| error.into_inner());
        if !state.retired && state.users == 0 {
            state.idle_since = Some(Instant::now());
        }
    }

    pub(crate) fn retire(&self) {
        let mut state = self.0.lock().unwrap_or_else(|error| error.into_inner());
        state.retired = true;
        state.idle_since = None;
    }

    fn retire_if_idle(&self, timeout: Duration) -> bool {
        let mut state = self.0.lock().unwrap_or_else(|error| error.into_inner());
        if state.retired
            || state.users != 0
            || !state
                .idle_since
                .is_some_and(|since| since.elapsed() >= timeout)
        {
            return false;
        }
        state.retired = true;
        state.idle_since = None;
        true
    }
}

// This guard owns no session/PTY. Keeping it in PtyState cannot form a cycle.
pub(crate) struct ResourceLease(Arc<Mutex<ResourceState>>);

impl Drop for ResourceLease {
    fn drop(&mut self) {
        let mut state = self.0.lock().unwrap_or_else(|error| error.into_inner());
        state.users -= 1;
        if state.users == 0 && !state.retired {
            state.idle_since = Some(Instant::now());
        }
    }
}

pub(crate) struct SessionLease {
    session: Arc<SshSession>,
    _resource: ResourceLease,
}

impl SessionLease {
    pub(crate) fn new(session: Arc<SshSession>) -> SshResult<Self> {
        let resource = session
            .resources
            .acquire()
            .ok_or_else(|| SshError::ConnectionLost(session.key().into()))?;
        Ok(Self {
            session,
            _resource: resource,
        })
    }
}

impl Deref for SessionLease {
    type Target = SshSession;

    fn deref(&self) -> &Self::Target {
        &self.session
    }
}

impl SshServiceImpl {
    pub async fn reap_idle_connections(&self) -> SshResult<usize> {
        self.ensure_loaded().await;
        let candidates = self
            .sessions
            .read()
            .await
            .values()
            .cloned()
            .collect::<Vec<_>>();
        let mut retired = Vec::new();
        let mut failure = None;
        for session in candidates {
            // Never wait for a network owner, challenge resume, or rotation probe.
            let Some(permit) = self.connection_operations.try_acquire(session.key()) else {
                continue;
            };
            let _lifecycle = self.lifecycle.write().await;
            if permit.ensure_current().is_err() {
                continue;
            }
            let mut sessions = self.sessions.write().await;
            if !sessions
                .get(session.key())
                .is_some_and(|current| Arc::ptr_eq(current, &session))
                || session.client().await.is_none()
                || !session
                    .resources
                    .retire_if_idle(self.connection_idle_timeout)
            {
                continue;
            }
            sessions.remove(session.key());
            drop(sessions);
            self.connection_operations.retire(session.key());
            tracing::debug!(target: "deepagent_ssh", connection_id = session.key(), idle_timeout_ms = self.connection_idle_timeout.as_millis() as u64, "retired idle SSH transport");
            if let Err(error) = self
                .update_cached_status(session.key(), SshStatus::Disconnected, None, None)
                .await
            {
                tracing::warn!(target: "deepagent_ssh", connection_id = session.key(), %error, "failed to persist idle SSH retirement");
                failure.get_or_insert(error);
            }
            retired.push(session);
        }
        let count = retired.len();
        // Retired resources cannot affect replacement accounting or cached status.
        // All transport cleanup happens after releasing the global writer.
        for session in retired {
            let result =
                tokio::time::timeout(Duration::from_secs(3), Self::close_session(&session))
                    .await
                    .unwrap_or_else(|_| Err(SshError::Timeout(Duration::from_secs(3))));
            if let Err(error) = result {
                tracing::warn!(target: "deepagent_ssh", connection_id = session.key(), %error, "failed to close idle SSH transport");
                failure.get_or_insert(error);
            }
        }
        failure.map_or(Ok(count), Err)
    }
}
