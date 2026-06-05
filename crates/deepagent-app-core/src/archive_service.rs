//! Archived conversation index for the app shell.
//!
//! Archiving is an app-level visibility state, not an event-log mutation. The
//! session event stream stays append-only and replayable; this service stores a
//! small document-store index of session ids hidden from the live sidebar.

use std::collections::HashSet;
use std::sync::Arc;

use deepagent_core::clock::{Clock, SystemClock};
use deepagent_core::error::Result;
use deepagent_persistence::document_store::DocumentStore;
use deepagent_persistence::event_store::EventStore;
use deepagent_persistence::Database;
use serde::{Deserialize, Serialize};

use crate::dto::{ArchiveProjectResultDto, ArchivedConversationDto};
use crate::project_service::folder_name;

const ARCHIVE_COLLECTION: &str = "archived_conversations";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ArchivedConversationRecord {
    session_id: String,
    title: Option<String>,
    project: Option<String>,
    project_path: Option<String>,
    archived_at: i64,
    updated_at: i64,
}

impl ArchivedConversationRecord {
    fn into_dto(self) -> ArchivedConversationDto {
        ArchivedConversationDto {
            session_id: self.session_id,
            title: self.title,
            project: self.project,
            project_path: self.project_path,
            archived_at: self.archived_at,
            updated_at: self.updated_at,
        }
    }
}

/// Stores and queries app-level archived-session state.
pub struct ArchiveService {
    db: Arc<Database>,
}

impl ArchiveService {
    /// Build over the shared application database.
    pub fn new(db: Arc<Database>) -> Self {
        Self { db }
    }

    /// Session ids currently hidden from the live sidebar.
    pub fn archived_ids(&self) -> Result<HashSet<String>> {
        Ok(self.list()?.into_iter().map(|a| a.session_id).collect())
    }

    /// Whether `session_id` is archived.
    pub fn is_archived(&self, session_id: &str) -> Result<bool> {
        Ok(DocumentStore::new(&self.db)
            .get(ARCHIVE_COLLECTION, session_id)?
            .is_some())
    }

    /// Archive every non-archived conversation under `project_path`.
    pub fn archive_project(&self, project_path: &str) -> Result<ArchiveProjectResultDto> {
        let now = SystemClock.now().as_millis();
        let event_store = EventStore::new(&self.db);
        let doc_store = DocumentStore::new(&self.db);
        let existing = self.archived_ids()?;
        let project_name = folder_name(project_path);
        let mut archived_count = 0u32;

        for session in event_store.list_sessions()? {
            if session.project.as_deref() != Some(project_path) {
                continue;
            }
            let session_id = session.id.to_string();
            if existing.contains(&session_id) {
                continue;
            }

            let record = ArchivedConversationRecord {
                session_id: session_id.clone(),
                title: session.title.clone(),
                project: Some(project_name.clone()),
                project_path: Some(project_path.to_string()),
                archived_at: now,
                updated_at: session.updated_at.as_millis(),
            };
            let body = serde_json::to_string(&record)?;
            doc_store.put(
                ARCHIVE_COLLECTION,
                &session_id,
                &body,
                None,
                SystemClock.now(),
            )?;
            archived_count += 1;
        }

        Ok(ArchiveProjectResultDto {
            project_path: project_path.to_string(),
            project_name,
            archived_count,
        })
    }

    /// List archived conversations, newest archived first.
    pub fn list(&self) -> Result<Vec<ArchivedConversationDto>> {
        let mut out = Vec::new();
        for doc in DocumentStore::new(&self.db).list(ARCHIVE_COLLECTION)? {
            match serde_json::from_str::<ArchivedConversationRecord>(&doc.body) {
                Ok(record) => out.push(record.into_dto()),
                Err(err) => tracing::warn!(id = %doc.id, error = %err, "invalid archive record"),
            }
        }
        out.sort_by_key(|a| std::cmp::Reverse(a.archived_at));
        Ok(out)
    }

    /// Remove one conversation from the archive index.
    pub fn unarchive_session(&self, session_id: &str) -> Result<bool> {
        DocumentStore::new(&self.db).delete(ARCHIVE_COLLECTION, session_id)
    }

    /// Delete one archived entry from the archive index.
    pub fn delete_archived_session(&self, session_id: &str) -> Result<bool> {
        self.unarchive_session(session_id)
    }

    /// Clear all archived entries. Returns how many archive records were removed.
    pub fn delete_all(&self) -> Result<u32> {
        let archived = self.list()?;
        let store = DocumentStore::new(&self.db);
        let mut removed = 0u32;
        for item in archived {
            if store.delete(ARCHIVE_COLLECTION, &item.session_id)? {
                removed += 1;
            }
        }
        Ok(removed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use deepagent_core::clock::FixedClock;
    use deepagent_session::Session;

    fn service() -> (ArchiveService, Arc<Database>) {
        let db = Arc::new(Database::open_in_memory().unwrap());
        (ArchiveService::new(db.clone()), db)
    }

    #[test]
    fn archive_project_records_matching_sessions_only() {
        let (svc, db) = service();
        let clock = FixedClock::new(1_000);
        Session::create_in_project(&db, &clock, Some("a"), Default::default(), Some("/work/p"))
            .unwrap();
        Session::create_in_project(&db, &clock, Some("b"), Default::default(), Some("/work/p"))
            .unwrap();
        Session::create_in_project(&db, &clock, Some("c"), Default::default(), Some("/work/q"))
            .unwrap();

        let result = svc.archive_project("/work/p").unwrap();
        assert_eq!(result.archived_count, 2);
        let archived = svc.list().unwrap();
        assert_eq!(archived.len(), 2);
        assert!(archived.iter().all(|a| a.project.as_deref() == Some("p")));
    }

    #[test]
    fn archive_project_is_idempotent() {
        let (svc, db) = service();
        let clock = FixedClock::new(1_000);
        Session::create_in_project(&db, &clock, Some("a"), Default::default(), Some("/work/p"))
            .unwrap();

        assert_eq!(svc.archive_project("/work/p").unwrap().archived_count, 1);
        assert_eq!(svc.archive_project("/work/p").unwrap().archived_count, 0);
        assert_eq!(svc.list().unwrap().len(), 1);
    }

    #[test]
    fn unarchive_removes_index_entry() {
        let (svc, db) = service();
        let clock = FixedClock::new(1_000);
        let session =
            Session::create_in_project(&db, &clock, Some("a"), Default::default(), Some("/work/p"))
                .unwrap();
        svc.archive_project("/work/p").unwrap();

        assert!(svc.is_archived(&session.id().to_string()).unwrap());
        assert!(svc.unarchive_session(&session.id().to_string()).unwrap());
        assert!(!svc.is_archived(&session.id().to_string()).unwrap());
    }
}
