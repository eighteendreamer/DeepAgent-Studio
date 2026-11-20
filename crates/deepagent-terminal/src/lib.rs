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

mod output;
mod session_lock;
pub use output::{TerminalOutputNotice, TerminalOutputSignal, TerminalOutputSubscription};

#[derive(Debug, Error)]
pub enum TerminalError {
    #[error("terminal session {0} was not found")]
    SessionNotFound(String),
    #[error("terminal session {0} is already registered")]
    SessionExists(String),
    #[error("terminal input lease is stale or belongs to another scope")]
    InvalidLease,
    #[error("terminal session is locked")]
    Locked,
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

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TerminalTarget {
    #[default]
    Local,
    Ssh {
        #[serde(rename = "connectionId")]
        connection_id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalOpenRequest {
    #[serde(default)]
    pub target: TerminalTarget,
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
    /// Fences SSH operations to the transport that created this session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connection_generation: Option<u64>,
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
    /// Cursor immediately after the bytes returned in `data`. It is not the
    /// producer's current output position when the response is capped.
    pub cursor: u64,
    pub data: Vec<u8>,
    /// The requested cursor was older than the retained history.
    pub truncated: bool,
    /// True only after the producer ended and this response drained the final bytes.
    #[serde(default)]
    pub eof: bool,
}

/// State returned when a client asks whether a persisted cursor can still be
/// attached to a live terminal session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalRecoveryStatus {
    pub cursor: u64,
    /// A real child process is still alive, not merely present in a host map.
    pub available: bool,
    /// The current host generation can accept input without explicit takeover.
    #[serde(default)]
    pub control_available: bool,
}

/// Process-local input ownership with epoch fencing. A stale lease can never
/// write after a takeover, even if its holder still has the old value.
pub struct TerminalLeaseRegistry {
    leases: Mutex<HashMap<String, TerminalInputLease>>,
    durable: Option<Arc<dyn TerminalLeasePersistence>>,
    locks: session_lock::SessionLocks,
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

    /// Extend the current, unexpired generation; never revive an old token.
    fn renew(&self, _lease: &TerminalInputLease, _now: i64) -> TerminalResult<()> {
        Err(TerminalError::UnsupportedCapability {
            backend: "lease_store".into(),
            capability: "renew".into(),
        })
    }

    /// Atomically replace the exact expired generation, not an unrelated
    /// successor. Stores without this guarantee must explicitly reject it.
    fn recover(
        &self,
        _current: &TerminalInputLease,
        _next: &TerminalInputLease,
        _now: i64,
    ) -> TerminalResult<u64> {
        Err(TerminalError::UnsupportedCapability {
            backend: "lease_store".into(),
            capability: "expired_lease_recovery".into(),
        })
    }

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
            locks: Default::default(),
        }
    }
}

impl TerminalLeaseRegistry {
    pub fn with_persistence(persistence: Arc<dyn TerminalLeasePersistence>) -> Self {
        Self {
            leases: Mutex::new(HashMap::new()),
            durable: Some(persistence),
            locks: Default::default(),
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

    fn validate_identity(
        leases: &HashMap<String, TerminalInputLease>,
        lease: &TerminalInputLease,
    ) -> TerminalResult<()> {
        if leases.get(&lease.session_id) != Some(lease) {
            return Err(TerminalError::InvalidLease);
        }
        Ok(())
    }

    pub fn validate(&self, lease: &TerminalInputLease) -> TerminalResult<()> {
        let leases = self.leases.lock().unwrap_or_else(|p| p.into_inner());
        Self::validate_identity(&leases, lease)?;
        if let Some(durable) = &self.durable {
            if !durable.validate(lease, unix_now())? {
                return Err(TerminalError::InvalidLease);
            }
        }
        Ok(())
    }

    /// Backend adapters call this inside the same control gate as the actual write.
    /// Renewal and read ownership intentionally remain valid while locked.
    pub fn validate_input(&self, lease: &TerminalInputLease) -> TerminalResult<()> {
        self.validate(lease)?;
        self.locks.ensure_unlocked(&lease.session_id)
    }

    pub fn lock(
        &self,
        lease: &TerminalInputLease,
        password: &str,
        hide_output: bool,
    ) -> TerminalResult<()> {
        let leases = self.leases.lock().unwrap_or_else(|p| p.into_inner());
        Self::validate_identity(&leases, lease)?;
        if let Some(store) = &self.durable {
            if !store.validate(lease, unix_now())? {
                return Err(TerminalError::InvalidLease);
            }
        }
        self.locks.lock(&lease.session_id, password, hide_output)
    }

    pub fn unlock(&self, lease: &TerminalInputLease, password: &str) -> TerminalResult<()> {
        let leases = self.leases.lock().unwrap_or_else(|p| p.into_inner());
        Self::validate_identity(&leases, lease)?;
        if let Some(store) = &self.durable {
            if !store.validate(lease, unix_now())? {
                return Err(TerminalError::InvalidLease);
            }
        }
        self.locks.unlock(&lease.session_id, password)
    }

    pub fn lock_status(&self, session_id: &str) -> TerminalResult<Option<bool>> {
        let leases = self.leases.lock().unwrap_or_else(|p| p.into_inner());
        if !leases.contains_key(session_id) {
            return Err(TerminalError::SessionNotFound(session_id.into()));
        }
        Ok(self.locks.status(session_id))
    }

    pub fn renew(&self, lease: &TerminalInputLease) -> TerminalResult<()> {
        let leases = self.leases.lock().unwrap_or_else(|p| p.into_inner());
        Self::validate_identity(&leases, lease)?;
        if let Some(durable) = &self.durable {
            durable.renew(lease, unix_now())?;
        }
        Ok(())
    }

    pub fn takeover(
        &self,
        session_id: &str,
        holder: TerminalInputHolder,
    ) -> TerminalResult<TerminalInputLease> {
        let mut leases = self.leases.lock().unwrap_or_else(|p| p.into_inner());
        self.locks.ensure_unlocked(session_id)?;
        let current = leases
            .get(session_id)
            .cloned()
            .ok_or_else(|| TerminalError::SessionNotFound(session_id.to_string()))?;
        let mut next = new_lease(session_id, &current.run_id, holder, current.epoch + 1);
        if let Some(durable) = &self.durable {
            let now = unix_now();
            next.epoch = if durable.validate(&current, now)? {
                durable.transfer(&current, &next, now, "takeover")?
            } else {
                durable.recover(&current, &next, now)?
            };
        }
        leases.insert(session_id.to_string(), next.clone());
        Ok(next)
    }

    pub fn release(
        &self,
        lease: &TerminalInputLease,
        next_holder: TerminalInputHolder,
    ) -> TerminalResult<TerminalInputLease> {
        let mut leases = self.leases.lock().unwrap_or_else(|p| p.into_inner());
        Self::validate_identity(&leases, lease)?;
        self.locks.ensure_unlocked(&lease.session_id)?;
        let mut next = new_lease(
            &lease.session_id,
            &lease.run_id,
            next_holder,
            lease.epoch + 1,
        );
        if let Some(durable) = &self.durable {
            next.epoch = durable.transfer(lease, &next, unix_now(), "release")?;
        }
        leases.insert(lease.session_id.clone(), next.clone());
        Ok(next)
    }

    pub fn remove(&self, lease: &TerminalInputLease) -> TerminalResult<()> {
        let mut leases = self.leases.lock().unwrap_or_else(|p| p.into_inner());
        Self::validate_identity(&leases, lease)?;
        if let Some(durable) = &self.durable {
            durable.revoke(lease, unix_now(), "terminal_closed")?;
        }
        leases.remove(&lease.session_id);
        self.locks.remove(&lease.session_id);
        Ok(())
    }

    pub fn control_available(&self, session_id: &str) -> TerminalResult<bool> {
        let leases = self.leases.lock().unwrap_or_else(|p| p.into_inner());
        match leases.get(session_id) {
            Some(lease) => self
                .durable
                .as_ref()
                .map_or(Ok(true), |store| store.validate(lease, unix_now())),
            None => Ok(false),
        }
    }

    /// Check host/run ownership without accepting an expired input token.
    /// Used before explicit takeover, which itself rotates the generation.
    pub fn validate_scope(&self, session_id: &str, run_id: &str) -> TerminalResult<()> {
        if self.scope_is_registered(session_id, run_id)? {
            Ok(())
        } else {
            Err(TerminalError::InvalidLease)
        }
    }

    /// A removed session is absent; a registered session owned by another run
    /// remains an error rather than looking like a closed session.
    pub fn scope_is_registered(&self, session_id: &str, run_id: &str) -> TerminalResult<bool> {
        let leases = self.leases.lock().unwrap_or_else(|p| p.into_inner());
        match leases.get(session_id) {
            Some(lease) if lease.run_id == run_id => Ok(true),
            Some(_) => Err(TerminalError::InvalidLease),
            None => Ok(false),
        }
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

    async fn lock(
        &self,
        _session: &TerminalSession,
        _lease: &TerminalInputLease,
        _password: &str,
        _hide_output: bool,
    ) -> TerminalResult<()> {
        Err(TerminalError::UnsupportedCapability {
            backend: self.backend_kind().into(),
            capability: "session_lock".into(),
        })
    }

    async fn unlock(
        &self,
        _session: &TerminalSession,
        _lease: &TerminalInputLease,
        _password: &str,
    ) -> TerminalResult<()> {
        Err(TerminalError::UnsupportedCapability {
            backend: self.backend_kind().into(),
            capability: "session_lock".into(),
        })
    }

    async fn lock_status(&self, _session: &TerminalSession) -> TerminalResult<Option<bool>> {
        Err(TerminalError::UnsupportedCapability {
            backend: self.backend_kind().into(),
            capability: "session_lock".into(),
        })
    }

    /// Subscribe before the first cursor read. Notices contain no output and
    /// cannot consume history or grant input ownership.
    async fn subscribe_output(
        &self,
        _session: &TerminalSession,
    ) -> TerminalResult<TerminalOutputSubscription> {
        Err(TerminalError::UnsupportedCapability {
            backend: self.backend_kind().into(),
            capability: "output_notification".into(),
        })
    }

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

    /// Renew the same unexpired generation. Expired owners must explicitly
    /// take over, obtain a new token and discard the old one.
    async fn renew(
        &self,
        _session: &TerminalSession,
        _lease: &TerminalInputLease,
    ) -> TerminalResult<()> {
        Err(TerminalError::UnsupportedCapability {
            backend: self.backend_kind().into(),
            capability: "renew".into(),
        })
    }

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
    fn legacy_terminal_open_defaults_to_local_and_ssh_target_is_explicit() {
        let mut wire = serde_json::json!({"runId":"fixture", "cwd":"", "cols":80, "rows":24, "initialHolder":"user"});
        let legacy: TerminalOpenRequest = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(legacy.target, TerminalTarget::Local);
        wire["target"] = serde_json::json!({"kind":"ssh", "connectionId":"fixture-connection"});
        let remote: TerminalOpenRequest = serde_json::from_value(wire).unwrap();
        assert_eq!(
            remote.target,
            TerminalTarget::Ssh {
                connection_id: "fixture-connection".into()
            }
        );
        let old: TerminalReadChunk =
            serde_json::from_value(serde_json::json!({"cursor":0,"data":[],"truncated":false}))
                .unwrap();
        assert!(!old.eof);
    }

    #[test]
    fn terminal_session_generation_is_optional_for_legacy_and_local_sessions() {
        let legacy: TerminalSession = serde_json::from_value(serde_json::json!({
            "sessionId": "direct:fixture",
            "runId": "run",
            "backend": "direct",
            "cols": 80,
            "rows": 24
        }))
        .unwrap();
        assert_eq!(legacy.connection_generation, None);
        assert!(serde_json::to_value(&legacy)
            .unwrap()
            .get("connectionGeneration")
            .is_none());

        let ssh = TerminalSession {
            session_id: "ssh:fixture:pty".into(),
            run_id: "run".into(),
            backend: "ssh".into(),
            cols: 80,
            rows: 24,
            connection_generation: Some(42),
        };
        assert_eq!(
            serde_json::to_value(ssh).unwrap()["connectionGeneration"],
            42
        );
    }

    #[test]
    fn h293_concurrent_old_release_and_remove_cannot_replace_a_successor() {
        let registry = Arc::new(TerminalLeaseRegistry::default());
        let old = registry
            .register("pty", "run", TerminalInputHolder::User)
            .unwrap();
        let gate = Arc::new(std::sync::Barrier::new(8));
        let workers: Vec<_> = (0..8)
            .map(|index| {
                let registry = registry.clone();
                let gate = gate.clone();
                let old = old.clone();
                std::thread::spawn(move || {
                    gate.wait();
                    if index % 2 == 0 {
                        registry.remove(&old).is_ok()
                    } else {
                        registry.release(&old, TerminalInputHolder::Runtime).is_ok()
                    }
                })
            })
            .collect();
        assert_eq!(
            workers
                .into_iter()
                .map(|worker| usize::from(worker.join().unwrap()))
                .sum::<usize>(),
            1
        );
        assert!(registry.validate(&old).is_err());
        assert!(registry.renew(&old).is_err());
    }

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
    fn scope_lookup_distinguishes_removed_sessions_from_another_run() {
        let registry = TerminalLeaseRegistry::default();
        assert!(!registry.scope_is_registered("terminal-1", "run-1").unwrap());
        let lease = registry
            .register("terminal-1", "run-1", TerminalInputHolder::User)
            .unwrap();
        assert!(registry.scope_is_registered("terminal-1", "run-1").unwrap());
        assert!(matches!(
            registry.scope_is_registered("terminal-1", "another-run"),
            Err(TerminalError::InvalidLease)
        ));
        assert!(registry
            .validate_scope("terminal-1", "another-run")
            .is_err());

        registry.remove(&lease).unwrap();
        assert!(!registry.scope_is_registered("terminal-1", "run-1").unwrap());
        assert!(registry.validate_scope("terminal-1", "run-1").is_err());
        assert!(registry.validate(&lease).is_err());
        assert!(registry.renew(&lease).is_err());
        assert!(registry
            .takeover("terminal-1", TerminalInputHolder::Runtime)
            .is_err());

        registry
            .register("terminal-1", "successor-run", TerminalInputHolder::User)
            .unwrap();
        assert!(matches!(
            registry.scope_is_registered("terminal-1", "run-1"),
            Err(TerminalError::InvalidLease)
        ));
        assert!(registry
            .scope_is_registered("terminal-1", "successor-run")
            .unwrap());
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
            target: Default::default(),
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

    #[test]
    fn recovery_status_wire_contract_is_camel_case() {
        let status = TerminalRecoveryStatus {
            cursor: 4096,
            available: false,
            control_available: false,
        };
        let json = serde_json::to_value(status).unwrap();
        assert_eq!(json["cursor"], 4096);
        assert_eq!(json["available"], false);
        assert_eq!(json["controlAvailable"], false);
        assert!(json.get("available_status").is_none());
    }
}
