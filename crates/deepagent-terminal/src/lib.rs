//! Shared terminal-session boundary.
//!
//! This crate intentionally owns no PTY implementation. Direct and SSH
//! backends implement the same contract without depending on each other or on
//! a UI transport.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Error)]
pub enum TerminalError {
    #[error("terminal session {0} was not found")]
    SessionNotFound(String),
    #[error("terminal session {0} is already registered")]
    SessionExists(String),
    #[error("terminal input lease is stale or belongs to another scope")]
    InvalidLease,
    #[error("terminal capability is unsupported by backend {backend}: {capability}")]
    UnsupportedCapability { backend: String, capability: String },
    #[error("terminal backend failed: {0}")]
    Backend(String),
}

pub type TerminalResult<T> = Result<T, TerminalError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalInputHolder {
    Runtime,
    User,
    RemoteViewer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalSignal {
    Interrupt,
    Terminate,
    Kill,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalOpenRequest {
    pub run_id: String,
    pub cwd: String,
    pub cols: u16,
    pub rows: u16,
    pub initial_holder: TerminalInputHolder,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalSession {
    pub session_id: String,
    pub run_id: String,
    pub backend: String,
    pub cols: u16,
    pub rows: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalInputLease {
    pub lease_id: String,
    pub session_id: String,
    pub run_id: String,
    pub holder: TerminalInputHolder,
    pub epoch: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalReadChunk {
    pub cursor: u64,
    pub data: Vec<u8>,
    pub truncated: bool,
}

/// State returned when a client asks whether a persisted cursor can still be
/// attached to a live terminal session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalRecoveryStatus {
    pub cursor: u64,
    pub available: bool,
}

/// Process-local input ownership with epoch fencing. A stale lease can never
/// write after a takeover, even if its holder still has the old value.
pub struct TerminalLeaseRegistry {
    leases: Mutex<HashMap<String, TerminalInputLease>>,
    durable: Option<Arc<dyn TerminalLeasePersistence>>,
}

pub trait TerminalLeasePersistence: Send + Sync {
    fn acquire(&self, lease: &TerminalInputLease, now: i64) -> TerminalResult<u64>;
    fn transfer(
        &self,
        current: &TerminalInputLease,
        next: &TerminalInputLease,
        now: i64,
        reason: &str,
    ) -> TerminalResult<u64>;
    fn validate(&self, lease: &TerminalInputLease, now: i64) -> TerminalResult<bool>;
    fn revoke(&self, lease: &TerminalInputLease, now: i64, reason: &str) -> TerminalResult<()>;

    /// Persist the last output cursor observed for a session. Backends that
    /// cannot persist cursors may keep the compatibility no-op.
    fn record_cursor(&self, _session_id: &str, _cursor: u64) -> TerminalResult<()> {
        Ok(())
    }

    fn last_cursor(&self, _session_id: &str) -> TerminalResult<u64> {
        Ok(0)
    }
}

impl std::fmt::Debug for TerminalLeaseRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TerminalLeaseRegistry")
            .field("leases", &self.leases)
            .field("durable", &self.durable.as_ref().map(|_| "configured"))
            .finish()
    }
}

impl Default for TerminalLeaseRegistry {
    fn default() -> Self {
        Self {
            leases: Mutex::new(HashMap::new()),
            durable: None,
        }
    }
}

impl TerminalLeaseRegistry {
    pub fn with_persistence(persistence: Arc<dyn TerminalLeasePersistence>) -> Self {
        Self {
            leases: Mutex::new(HashMap::new()),
            durable: Some(persistence),
        }
    }

    pub fn register(
        &self,
        session_id: &str,
        run_id: &str,
        holder: TerminalInputHolder,
    ) -> TerminalResult<TerminalInputLease> {
        let mut leases = self.leases.lock().unwrap_or_else(|p| p.into_inner());
        if leases.contains_key(session_id) {
            return Err(TerminalError::SessionExists(session_id.to_string()));
        }
        let mut lease = new_lease(session_id, run_id, holder, 0);
        if let Some(durable) = &self.durable {
            lease.epoch = durable.acquire(&lease, unix_now())?;
        } else {
            lease.epoch = 1;
        }
        leases.insert(session_id.to_string(), lease.clone());
        Ok(lease)
    }

    pub fn validate(&self, lease: &TerminalInputLease) -> TerminalResult<()> {
        let leases = self.leases.lock().unwrap_or_else(|p| p.into_inner());
        if leases.get(&lease.session_id) != Some(lease) {
            return Err(TerminalError::InvalidLease);
        }
        if let Some(durable) = &self.durable {
            if !durable.validate(lease, unix_now())? {
                return Err(TerminalError::InvalidLease);
            }
        }
        Ok(())
    }

    pub fn takeover(
        &self,
        session_id: &str,
        holder: TerminalInputHolder,
    ) -> TerminalResult<TerminalInputLease> {
        let mut leases = self.leases.lock().unwrap_or_else(|p| p.into_inner());
        let current = leases
            .get(session_id)
            .cloned()
            .ok_or_else(|| TerminalError::SessionNotFound(session_id.to_string()))?;
        let mut next = new_lease(session_id, &current.run_id, holder, current.epoch + 1);
        if let Some(durable) = &self.durable {
            next.epoch = durable.transfer(&current, &next, unix_now(), "takeover")?;
        }
        leases.insert(session_id.to_string(), next.clone());
        Ok(next)
    }

    pub fn release(
        &self,
        lease: &TerminalInputLease,
        next_holder: TerminalInputHolder,
    ) -> TerminalResult<TerminalInputLease> {
        self.validate(lease)?;
        self.takeover(&lease.session_id, next_holder)
    }

    pub fn remove(&self, lease: &TerminalInputLease) -> TerminalResult<()> {
        self.validate(lease)?;
        if let Some(durable) = &self.durable {
            durable.revoke(lease, unix_now(), "terminal_closed")?;
        }
        self.leases
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&lease.session_id);
        Ok(())
    }

    pub fn record_cursor(&self, session_id: &str, cursor: u64) -> TerminalResult<()> {
        if let Some(durable) = &self.durable {
            durable.record_cursor(session_id, cursor)?;
        }
        Ok(())
    }

    pub fn last_cursor(&self, session_id: &str) -> TerminalResult<u64> {
        self.durable
            .as_ref()
            .map_or(Ok(0), |durable| durable.last_cursor(session_id))
    }
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
}

fn new_lease(
    session_id: &str,
    run_id: &str,
    holder: TerminalInputHolder,
    epoch: u64,
) -> TerminalInputLease {
    TerminalInputLease {
        lease_id: Uuid::new_v4().to_string(),
        session_id: session_id.to_string(),
        run_id: run_id.to_string(),
        holder,
        epoch,
    }
}

#[async_trait]
pub trait TerminalSessionBackend: Send + Sync {
    fn backend_kind(&self) -> &'static str;

    async fn open(
        &self,
        request: TerminalOpenRequest,
    ) -> TerminalResult<(TerminalSession, TerminalInputLease)>;

    async fn write(
        &self,
        session: &TerminalSession,
        lease: &TerminalInputLease,
        data: &[u8],
    ) -> TerminalResult<()>;

    async fn read(
        &self,
        session: &TerminalSession,
        after_cursor: u64,
    ) -> TerminalResult<TerminalReadChunk>;

    async fn resize(
        &self,
        session: &TerminalSession,
        lease: &TerminalInputLease,
        cols: u16,
        rows: u16,
    ) -> TerminalResult<()>;

    async fn signal(
        &self,
        session: &TerminalSession,
        lease: &TerminalInputLease,
        signal: TerminalSignal,
    ) -> TerminalResult<()>;

    async fn takeover(
        &self,
        session: &TerminalSession,
        holder: TerminalInputHolder,
    ) -> TerminalResult<TerminalInputLease>;

    async fn release(
        &self,
        session: &TerminalSession,
        lease: &TerminalInputLease,
        next_holder: TerminalInputHolder,
    ) -> TerminalResult<TerminalInputLease>;

    async fn close(
        &self,
        session: &TerminalSession,
        lease: &TerminalInputLease,
    ) -> TerminalResult<()>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn takeover_fences_the_previous_lease() {
        let registry = TerminalLeaseRegistry::default();
        let runtime = registry
            .register("terminal-1", "run-1", TerminalInputHolder::Runtime)
            .unwrap();
        let user = registry
            .takeover("terminal-1", TerminalInputHolder::User)
            .unwrap();

        assert_eq!(user.epoch, 2);
        assert_eq!(
            registry.validate(&runtime).unwrap_err().to_string(),
            "terminal input lease is stale or belongs to another scope"
        );
        registry.validate(&user).unwrap();
    }

    #[test]
    fn release_rotates_identity_and_holder() {
        let registry = TerminalLeaseRegistry::default();
        let user = registry
            .register("terminal-1", "run-1", TerminalInputHolder::User)
            .unwrap();
        let runtime = registry
            .release(&user, TerminalInputHolder::Runtime)
            .unwrap();

        assert_ne!(runtime.lease_id, user.lease_id);
        assert_eq!(runtime.epoch, user.epoch + 1);
        assert_eq!(runtime.holder, TerminalInputHolder::Runtime);
    }

    #[test]
    fn wire_contract_is_camel_case() {
        let request = TerminalOpenRequest {
            run_id: "run-1".into(),
            cwd: "C:/workspace".into(),
            cols: 80,
            rows: 24,
            initial_holder: TerminalInputHolder::Runtime,
        };
        let json = serde_json::to_value(request).unwrap();
        assert_eq!(json["runId"], "run-1");
        assert_eq!(json["initialHolder"], "runtime");
        assert!(json.get("run_id").is_none());
    }
}
