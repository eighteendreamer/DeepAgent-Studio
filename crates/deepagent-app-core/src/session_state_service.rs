//! App-level sidebar state for sessions.
//!
//! These flags are UI visibility/ordering metadata. They intentionally live
//! outside the append-only session event log.

use std::collections::HashSet;
use std::sync::Arc;

use deepagent_core::clock::{Clock, SystemClock};
use deepagent_core::error::Result;
use deepagent_persistence::document_store::DocumentStore;
use deepagent_persistence::Database;
use serde::{Deserialize, Serialize};

const PINNED_SESSIONS_COLLECTION: &str = "pinned_sessions";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PinnedSessionRecord {
    session_id: String,
    pinned_at: i64,
}

/// Stores and queries app-level session sidebar state.
pub struct SessionStateService {
    db: Arc<Database>,
}

impl SessionStateService {
    /// Build over the shared application database.
    pub fn new(db: Arc<Database>) -> Self {
        Self { db }
    }

    /// Session ids pinned in the live sidebar.
    pub fn pinned_ids(&self) -> Result<HashSet<String>> {
        let mut out = HashSet::new();
        for doc in DocumentStore::new(&self.db).list(PINNED_SESSIONS_COLLECTION)? {
            match serde_json::from_str::<PinnedSessionRecord>(&doc.body) {
                Ok(record) => {
                    out.insert(record.session_id);
                }
                Err(err) => {
                    tracing::warn!(id = %doc.id, error = %err, "invalid pinned session record")
                }
            }
        }
        Ok(out)
    }

    /// Set whether one session is pinned.
    pub fn set_pinned(&self, session_id: &str, pinned: bool) -> Result<bool> {
        let store = DocumentStore::new(&self.db);
        if pinned {
            let record = PinnedSessionRecord {
                session_id: session_id.to_string(),
                pinned_at: SystemClock.now().as_millis(),
            };
            store.put(
                PINNED_SESSIONS_COLLECTION,
                session_id,
                &serde_json::to_string(&record)?,
                None,
                SystemClock.now(),
            )?;
            Ok(true)
        } else {
            store.delete(PINNED_SESSIONS_COLLECTION, session_id)?;
            Ok(false)
        }
    }

    /// Remove a session from the pinned index.
    pub fn clear_session(&self, session_id: &str) -> Result<bool> {
        DocumentStore::new(&self.db).delete(PINNED_SESSIONS_COLLECTION, session_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn service() -> SessionStateService {
        SessionStateService::new(Arc::new(Database::open_in_memory().unwrap()))
    }

    #[test]
    fn set_and_clear_pinned_session() {
        let svc = service();
        assert!(svc.pinned_ids().unwrap().is_empty());

        assert!(svc.set_pinned("s1", true).unwrap());
        assert!(svc.pinned_ids().unwrap().contains("s1"));

        assert!(!svc.set_pinned("s1", false).unwrap());
        assert!(!svc.pinned_ids().unwrap().contains("s1"));
    }
}
