//! App-level sidebar state for sessions.
//!
//! These flags are UI visibility/ordering metadata. They intentionally live
//! outside the append-only session event log.

use std::collections::{BTreeMap, HashSet};
use std::sync::{Arc, Mutex};

use deepagent_core::clock::{Clock, SystemClock};
use deepagent_core::error::{CoreError, Result};
use deepagent_persistence::document_store::DocumentStore;
use deepagent_persistence::Database;
use serde::{Deserialize, Serialize};

const PINNED_SESSIONS_COLLECTION: &str = "pinned_sessions";
const SESSION_UI_PREFS_COLLECTION: &str = "session_ui_prefs";

/// Sidebar presentation state; session event history and project identity are unchanged.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SidebarPreferences {
    pub version: u32,
    pub revision: u64,
    pub sections: Vec<SidebarSection>,
    pub unread: BTreeMap<String, bool>,
    pub session_sections: BTreeMap<String, String>,
    pub default_open_target: String,
}

impl Default for SidebarPreferences {
    fn default() -> Self {
        Self {
            version: 1,
            revision: 0,
            sections: Vec::new(),
            unread: BTreeMap::new(),
            session_sections: BTreeMap::new(),
            default_open_target: "VS Code".into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SidebarSection {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SidebarPreferenceAction {
    SetUnread {
        session_id: String,
        unread: bool,
    },
    SetSection {
        session_id: String,
        section_id: Option<String>,
    },
    CreateSection {
        name: String,
        session_id: Option<String>,
    },
    SetOpenTarget {
        target: String,
    },
}

pub const OPEN_TARGETS: &[&str] = &[
    "VS Code",
    "Visual Studio",
    "Cursor",
    "Zed",
    "Antigravity",
    "Default app",
    "File Explorer",
    "Terminal",
    "WSL",
];

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
    sidebar_lock: Mutex<()>,
}

impl SessionStateService {
    /// Build over the shared application database.
    pub fn new(db: Arc<Database>) -> Self {
        Self {
            db,
            sidebar_lock: Mutex::new(()),
        }
    }

    pub fn sidebar_preferences(&self) -> Result<SidebarPreferences> {
        match DocumentStore::new(&self.db).get("app_ui_prefs", "sidebar")? {
            Some(doc) => Ok(serde_json::from_str(&doc.body)?),
            None => Ok(SidebarPreferences::default()),
        }
    }

    pub fn update_sidebar_preferences(
        &self,
        action: SidebarPreferenceAction,
    ) -> Result<SidebarPreferences> {
        let _guard = self
            .sidebar_lock
            .lock()
            .map_err(|_| CoreError::invalid("sidebar state lock poisoned"))?;
        let mut prefs = self.sidebar_preferences()?;
        match action {
            SidebarPreferenceAction::SetUnread { session_id, unread } => {
                if unread {
                    prefs.unread.insert(session_id, true);
                } else {
                    prefs.unread.remove(&session_id);
                }
            }
            SidebarPreferenceAction::SetSection {
                session_id,
                section_id,
            } => {
                if let Some(id) = section_id {
                    if !prefs.sections.iter().any(|section| section.id == id) {
                        return Err(CoreError::not_found("sidebar section"));
                    }
                    prefs.session_sections.insert(session_id, id);
                } else {
                    prefs.session_sections.remove(&session_id);
                }
            }
            SidebarPreferenceAction::CreateSection { name, session_id } => {
                let name = name.trim();
                if name.is_empty()
                    || name.chars().count() > 64
                    || ["项目", "最近对话"].contains(&name)
                {
                    return Err(CoreError::invalid(
                        "section name must contain 1–64 characters",
                    ));
                }
                if prefs.sections.iter().any(|section| section.name == name) {
                    return Err(CoreError::invalid("section name already exists"));
                }
                let id = deepagent_core::id::EventId::new().to_string();
                prefs.sections.push(SidebarSection {
                    id: id.clone(),
                    name: name.to_string(),
                });
                if let Some(session_id) = session_id {
                    prefs.session_sections.insert(session_id, id);
                }
            }
            SidebarPreferenceAction::SetOpenTarget { target } => {
                if !OPEN_TARGETS.contains(&target.as_str()) {
                    return Err(CoreError::invalid("unsupported open target"));
                }
                prefs.default_open_target = target;
            }
        }
        prefs.revision += 1;
        DocumentStore::new(&self.db).put(
            "app_ui_prefs",
            "sidebar",
            &serde_json::to_string(&prefs)?,
            None,
            SystemClock.now(),
        )?;
        tracing::info!("sidebar preferences updated");
        Ok(prefs)
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
    fn sidebar_preferences_survive_service_restart_and_keep_other_metadata() {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let svc = SessionStateService::new(db.clone());
        svc.set_env_panel_auto_open("s1", false).unwrap();
        svc.update_sidebar_preferences(SidebarPreferenceAction::SetUnread {
            session_id: "s1".into(),
            unread: true,
        })
        .unwrap();
        let prefs = svc
            .update_sidebar_preferences(SidebarPreferenceAction::CreateSection {
                name: "待处理".into(),
                session_id: Some("s1".into()),
            })
            .unwrap();
        let id = prefs.sections[0].id.clone();
        svc.update_sidebar_preferences(SidebarPreferenceAction::SetOpenTarget {
            target: "Cursor".into(),
        })
        .unwrap();
        drop(svc);
        let svc = SessionStateService::new(db);
        let prefs = svc.sidebar_preferences().unwrap();
        assert!(prefs.unread["s1"]);
        assert_eq!(prefs.session_sections["s1"], id);
        assert_eq!(prefs.default_open_target, "Cursor");
        assert!(!svc.ui_prefs("s1").unwrap().env_panel_auto_open);
        svc.update_sidebar_preferences(SidebarPreferenceAction::SetSection {
            session_id: "s1".into(),
            section_id: None,
        })
        .unwrap();
        svc.update_sidebar_preferences(SidebarPreferenceAction::SetUnread {
            session_id: "s1".into(),
            unread: false,
        })
        .unwrap();
        let prefs = svc.sidebar_preferences().unwrap();
        assert!(!prefs.unread.contains_key("s1"));
        assert!(!prefs.session_sections.contains_key("s1"));
        assert_eq!(prefs.sections.len(), 1);
    }

    #[test]
    fn sidebar_preferences_reject_invalid_sections_and_open_targets() {
        let svc = service();
        assert!(svc
            .update_sidebar_preferences(SidebarPreferenceAction::SetSection {
                session_id: "s1".into(),
                section_id: Some("missing".into())
            })
            .is_err());
        assert!(svc
            .update_sidebar_preferences(SidebarPreferenceAction::CreateSection {
                name: " ".into(),
                session_id: None
            })
            .is_err());
        assert!(svc
            .update_sidebar_preferences(SidebarPreferenceAction::SetOpenTarget {
                target: "cmd /c arbitrary".into()
            })
            .is_err());
        assert_eq!(
            svc.sidebar_preferences().unwrap(),
            SidebarPreferences::default()
        );
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
