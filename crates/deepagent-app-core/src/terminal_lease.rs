//! SQLite-backed terminal input leases.
//!
//! `execution_leases` remains the single durable lease table. Terminal
//! sessions use a resource id scoped by both run and terminal session, while
//! the public lease keeps those fields separate for protocol consumers.

use std::sync::Arc;

use deepagent_core::error::Result;
use deepagent_persistence::run_control::{NewExecutionLease, RunControlStore};
use deepagent_persistence::Database;
use deepagent_terminal::{
    TerminalError, TerminalInputHolder, TerminalInputLease, TerminalLeasePersistence,
    TerminalResult,
};
use rusqlite::OptionalExtension;
use sha2::{Digest, Sha256};
use uuid::Uuid;

const TERMINAL_RESOURCE_KIND: &str = "terminal_session";

#[derive(Clone)]
pub struct SqliteTerminalLeaseStore {
    db: Arc<Database>,
    ttl_ms: i64,
}

impl SqliteTerminalLeaseStore {
    pub fn new(db: Arc<Database>, ttl_ms: i64) -> Result<Self> {
        if ttl_ms <= 0 {
            return Err(deepagent_core::error::CoreError::invalid(
                "terminal lease ttl must be positive",
            ));
        }
        Ok(Self { db, ttl_ms })
    }

    pub fn acquire(
        &self,
        session_id: &str,
        run_id: &str,
        holder: TerminalInputHolder,
        now: i64,
    ) -> Result<TerminalInputLease> {
        let lease_id = Uuid::new_v4().to_string();
        self.acquire_with_id(session_id, run_id, &lease_id, holder, now)
    }

    pub fn last_cursor(&self, session_id: &str) -> Result<u64> {
        self.db.with_conn(|connection| {
            let value: Option<i64> = connection
                .query_row(
                    "SELECT cursor FROM terminal_session_cursors WHERE session_id=?1",
                    [session_id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|error| {
                    deepagent_core::error::CoreError::Persistence(error.to_string())
                })?;
            Ok(value.unwrap_or(0).max(0) as u64)
        })
    }

    fn acquire_with_id(
        &self,
        session_id: &str,
        run_id: &str,
        lease_id: &str,
        holder: TerminalInputHolder,
        now: i64,
    ) -> Result<TerminalInputLease> {
        let owner = holder_name(holder);
        let resource_id = resource_id(run_id, session_id);
        let token_hash = lease_token_hash(lease_id, run_id, session_id);
        let record = RunControlStore::new(&self.db).acquire_lease(&NewExecutionLease {
            lease_id,
            resource_kind: TERMINAL_RESOURCE_KIND,
            resource_id: &resource_id,
            owner,
            fencing_token_hash: &token_hash,
            expires_at: now.saturating_add(self.ttl_ms),
            now,
        })?;
        Ok(to_terminal_lease(
            record.lease_id,
            session_id,
            run_id,
            holder,
            record.epoch,
        ))
    }

    pub fn transfer(
        &self,
        current: &TerminalInputLease,
        next_holder: TerminalInputHolder,
        now: i64,
        reason: &str,
    ) -> Result<TerminalInputLease> {
        let next_id = Uuid::new_v4().to_string();
        self.transfer_with_id(current, &next_id, next_holder, now, reason)
    }

    fn transfer_with_id(
        &self,
        current: &TerminalInputLease,
        next_id: &str,
        next_holder: TerminalInputHolder,
        now: i64,
        reason: &str,
    ) -> Result<TerminalInputLease> {
        let next_owner = holder_name(next_holder);
        let current_owner = holder_name(current.holder);
        let resource_id = resource_id(&current.run_id, &current.session_id);
        let token_hash = lease_token_hash(next_id, &current.run_id, &current.session_id);
        let record = RunControlStore::new(&self.db).transfer_lease(
            &current.lease_id,
            current_owner,
            current.epoch,
            &NewExecutionLease {
                lease_id: next_id,
                resource_kind: TERMINAL_RESOURCE_KIND,
                resource_id: &resource_id,
                owner: next_owner,
                fencing_token_hash: &token_hash,
                expires_at: now.saturating_add(self.ttl_ms),
                now,
            },
            reason,
        )?;
        Ok(to_terminal_lease(
            record.lease_id,
            &current.session_id,
            &current.run_id,
            next_holder,
            record.epoch,
        ))
    }

    pub fn validate(&self, lease: &TerminalInputLease, now: i64) -> Result<bool> {
        RunControlStore::new(&self.db).validate_fence(
            TERMINAL_RESOURCE_KIND,
            &resource_id(&lease.run_id, &lease.session_id),
            holder_name(lease.holder),
            lease.epoch,
            now,
        )
    }

    pub fn renew(&self, lease: &TerminalInputLease, now: i64) -> Result<()> {
        RunControlStore::new(&self.db).renew_lease(
            &lease.lease_id,
            holder_name(lease.holder),
            lease.epoch,
            now.saturating_add(self.ttl_ms),
            now,
        )?;
        Ok(())
    }

    pub fn revoke(&self, lease: &TerminalInputLease, now: i64, reason: &str) -> Result<()> {
        RunControlStore::new(&self.db).revoke_lease(
            &lease.lease_id,
            holder_name(lease.holder),
            lease.epoch,
            now,
            reason,
        )?;
        Ok(())
    }
}

impl TerminalLeasePersistence for SqliteTerminalLeaseStore {
    fn acquire(&self, lease: &TerminalInputLease, now: i64) -> TerminalResult<u64> {
        self.acquire_with_id(
            &lease.session_id,
            &lease.run_id,
            &lease.lease_id,
            lease.holder,
            now,
        )
        .map(|record| record.epoch)
        .map_err(|error| TerminalError::Backend(error.to_string()))
    }

    fn transfer(
        &self,
        current: &TerminalInputLease,
        next: &TerminalInputLease,
        now: i64,
        reason: &str,
    ) -> TerminalResult<u64> {
        self.transfer_with_id(current, &next.lease_id, next.holder, now, reason)
            .map(|record| record.epoch)
            .map_err(|error| TerminalError::Backend(error.to_string()))
    }

    fn validate(&self, lease: &TerminalInputLease, now: i64) -> TerminalResult<bool> {
        self.validate(lease, now)
            .map_err(|error| TerminalError::Backend(error.to_string()))
    }

    fn revoke(&self, lease: &TerminalInputLease, now: i64, reason: &str) -> TerminalResult<()> {
        self.revoke(lease, now, reason)
            .map_err(|error| TerminalError::Backend(error.to_string()))
    }

    fn record_cursor(&self, session_id: &str, cursor: u64) -> TerminalResult<()> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .min(i64::MAX as u128) as i64;
        let cursor = i64::try_from(cursor)
            .map_err(|_| TerminalError::Backend("terminal cursor exceeds SQLite range".into()))?;
        self.db
            .with_conn(|connection| {
                connection
                    .execute(
                        "INSERT INTO terminal_session_cursors(session_id,cursor,updated_at) VALUES (?1,?2,?3) ON CONFLICT(session_id) DO UPDATE SET cursor=MAX(cursor,excluded.cursor), updated_at=excluded.updated_at",
                        rusqlite::params![session_id, cursor, now],
                    )
                    .map_err(|error| deepagent_core::error::CoreError::Persistence(error.to_string()))?;
                Ok(())
            })
            .map_err(|error| TerminalError::Backend(error.to_string()))
    }
}

fn holder_name(holder: TerminalInputHolder) -> &'static str {
    match holder {
        TerminalInputHolder::Runtime => "runtime",
        TerminalInputHolder::User => "user",
        TerminalInputHolder::RemoteViewer => "remote_viewer",
    }
}

fn resource_id(run_id: &str, session_id: &str) -> String {
    format!("{run_id}\u{1f}{session_id}")
}

fn lease_token_hash(lease_id: &str, run_id: &str, session_id: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(lease_id.as_bytes());
    hasher.update([0]);
    hasher.update(run_id.as_bytes());
    hasher.update([0]);
    hasher.update(session_id.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn to_terminal_lease(
    lease_id: String,
    session_id: &str,
    run_id: &str,
    holder: TerminalInputHolder,
    epoch: u64,
) -> TerminalInputLease {
    TerminalInputLease {
        lease_id,
        session_id: session_id.to_string(),
        run_id: run_id.to_string(),
        holder,
        epoch,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use deepagent_terminal::TerminalLeaseRegistry;

    #[test]
    fn transfer_persists_fence_and_survives_store_recreation() {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let store = SqliteTerminalLeaseStore::new(db.clone(), 1_000).unwrap();
        let user = store
            .acquire("pty-1", "run-1", TerminalInputHolder::User, 10)
            .unwrap();
        assert!(store.validate(&user, 11).unwrap());

        let runtime = store
            .transfer(&user, TerminalInputHolder::Runtime, 20, "user_released")
            .unwrap();
        assert_eq!(runtime.epoch, user.epoch + 1);
        assert!(!store.validate(&user, 21).unwrap());

        let reopened = SqliteTerminalLeaseStore::new(db, 1_000).unwrap();
        assert!(reopened.validate(&runtime, 22).unwrap());
    }

    #[test]
    fn lease_is_scoped_to_run_and_terminal_session() {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let store = SqliteTerminalLeaseStore::new(db, 1_000).unwrap();
        let lease = store
            .acquire("pty-1", "run-1", TerminalInputHolder::Runtime, 10)
            .unwrap();
        let mut wrong_run = lease.clone();
        wrong_run.run_id = "run-2".into();
        let mut wrong_session = lease.clone();
        wrong_session.session_id = "pty-2".into();

        assert!(!store.validate(&wrong_run, 11).unwrap());
        assert!(!store.validate(&wrong_session, 11).unwrap());
    }

    #[test]
    fn shared_registry_can_use_sqlite_fencing() {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let durable = Arc::new(SqliteTerminalLeaseStore::new(db, 1_000).unwrap());
        let registry = TerminalLeaseRegistry::with_persistence(durable);
        let first = registry
            .register("pty-1", "run-1", TerminalInputHolder::Runtime)
            .unwrap();
        let next = registry
            .takeover("pty-1", TerminalInputHolder::User)
            .unwrap();

        assert!(next.epoch > first.epoch);
        assert!(registry.validate(&next).is_ok());
        assert!(registry.validate(&first).is_err());
    }

    #[test]
    fn persisted_cursor_is_monotonic_and_survives_store_recreation() {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let store = SqliteTerminalLeaseStore::new(db.clone(), 1_000).unwrap();
        store.record_cursor("pty-1", 128).unwrap();
        store.record_cursor("pty-1", 64).unwrap();
        assert_eq!(store.last_cursor("pty-1").unwrap(), 128);

        let reopened = SqliteTerminalLeaseStore::new(db, 1_000).unwrap();
        assert_eq!(reopened.last_cursor("pty-1").unwrap(), 128);
        assert_eq!(reopened.last_cursor("pty-missing").unwrap(), 0);
    }
}
