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
const SESSION_UI_PREFS_COLLECTION: &str = "session_ui_prefs";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PinnedSessionRecord {
    session_id: String,
    pinned_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SessionUiPrefsRecord {
    session_id: String,
    env_panel_auto_open: bool,
    updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionUiPrefs {
    pub env_panel_auto_open: bool,
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

    /// Durable UI preferences for one session. Missing records fall back to
    /// defaults so older sessions remain valid without migration.
    pub fn ui_prefs(&self, session_id: &str) -> Result<SessionUiPrefs> {
        let store = DocumentStore::new(&self.db);
        let Some(doc) = store.get(SESSION_UI_PREFS_COLLECTION, session_id)? else {
            return Ok(SessionUiPrefs {
                env_panel_auto_open: true,
            });
        };

        match serde_json::from_str::<SessionUiPrefsRecord>(&doc.body) {
            Ok(record) => Ok(SessionUiPrefs {
                env_panel_auto_open: record.env_panel_auto_open,
            }),
            Err(err) => {
                tracing::warn!(id = %doc.id, error = %err, "invalid session ui prefs record");
                Ok(SessionUiPrefs {
                    env_panel_auto_open: true,
                })
            }
        }
    }

    /// Persist whether the environment panel may auto-open for this session.
    pub fn set_env_panel_auto_open(
        &self,
        session_id: &str,
        enabled: bool,
    ) -> Result<SessionUiPrefs> {
        let store = DocumentStore::new(&self.db);
        let now = SystemClock.now();
        let record = SessionUiPrefsRecord {
            session_id: session_id.to_string(),
            env_panel_auto_open: enabled,
            updated_at: now.as_millis(),
        };
        store.put(
            SESSION_UI_PREFS_COLLECTION,
            session_id,
            &serde_json::to_string(&record)?,
            None,
            now,
        )?;
        Ok(SessionUiPrefs {
            env_panel_auto_open: enabled,
        })
    }

    /// Remove a session from the pinned index.
    pub fn clear_session(&self, session_id: &str) -> Result<bool> {
        DocumentStore::new(&self.db).delete(PINNED_SESSIONS_COLLECTION, session_id)
    }

    /// Remove all app-level state associated with a session.
    pub fn purge_session_state(&self, session_id: &str) -> Result<bool> {
        let store = DocumentStore::new(&self.db);
        let mut removed = store.delete(PINNED_SESSIONS_COLLECTION, session_id)?;
        removed |= store.delete(SESSION_UI_PREFS_COLLECTION, session_id)?;
        Ok(removed)
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

    #[test]
    fn session_ui_prefs_default_and_persist() {
        let svc = service();
        assert_eq!(
            svc.ui_prefs("s1").unwrap(),
            SessionUiPrefs {
                env_panel_auto_open: true,
            }
        );

        let updated = svc.set_env_panel_auto_open("s1", false).unwrap();
        assert_eq!(
            updated,
            SessionUiPrefs {
                env_panel_auto_open: false,
            }
        );
        assert_eq!(svc.ui_prefs("s1").unwrap(), updated);
    }

    #[test]
    fn clear_session_keeps_ui_prefs() {
        let svc = service();
        svc.set_pinned("s1", true).unwrap();
        svc.set_env_panel_auto_open("s1", false).unwrap();

        assert!(svc.clear_session("s1").unwrap());
        assert!(!svc.ui_prefs("s1").unwrap().env_panel_auto_open);
        assert!(!svc.pinned_ids().unwrap().contains("s1"));
    }

    #[test]
    fn purge_session_state_removes_ui_prefs() {
        let svc = service();
        svc.set_env_panel_auto_open("s1", false).unwrap();
        assert!(!svc.ui_prefs("s1").unwrap().env_panel_auto_open);

        assert!(svc.purge_session_state("s1").unwrap());
        assert!(svc.ui_prefs("s1").unwrap().env_panel_auto_open);
    }
}
