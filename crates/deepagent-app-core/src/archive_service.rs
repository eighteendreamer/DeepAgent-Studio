//! Archived conversation index for the app shell.
//!
//! Archiving is an app-level visibility state, not an event-log mutation. The
//! session event stream stays append-only and replayable; this service stores a
//! small document-store index of session ids hidden from the live sidebar.

use std::collections::HashSet;
use std::sync::Arc;

use deepagent_core::clock::{Clock, SystemClock};
use deepagent_core::error::{CoreError, Result};
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
    #[serde(default)]
    deleted_at: Option<i64>,
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
            deleted_at: self.deleted_at,
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
                deleted_at: None,
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

    /// Archive one conversation by session id. Returns whether it was newly archived.
    pub fn archive_session(&self, session_id: &str) -> Result<bool> {
        if self.is_archived(session_id)? {
            return Ok(false);
        }

        let session = EventStore::new(&self.db)
            .list_sessions()?
            .into_iter()
            .find(|session| session.id.to_string() == session_id)
            .ok_or_else(|| CoreError::not_found(format!("session {session_id}")))?;

        let record = ArchivedConversationRecord {
            session_id: session_id.to_string(),
            title: session.title,
            project: session.project.as_deref().map(folder_name),
            project_path: session.project,
            archived_at: SystemClock.now().as_millis(),
            deleted_at: None,
            updated_at: session.updated_at.as_millis(),
        };
        DocumentStore::new(&self.db).put(
            ARCHIVE_COLLECTION,
            session_id,
            &serde_json::to_string(&record)?,
            None,
            SystemClock.now(),
        )?;
        Ok(true)
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

    /// Restore recycled files first, then remove the archive marker.
    pub fn unarchive_session(&self, session_id: &str) -> Result<bool> {
        let Some(_) = self.record(session_id)? else {
            return Ok(false);
        };
        let id = session_id
            .parse()
            .map_err(|error| CoreError::invalid(format!("bad session id: {error}")))?;
        if !EventStore::new(&self.db).restore_trashed_session(id)? {
            return Err(CoreError::not_found(format!(
                "session files for {session_id} are no longer available"
            )));
        }
        DocumentStore::new(&self.db).delete(ARCHIVE_COLLECTION, session_id)
    }

    /// Move one archived conversation into the managed recycle area.
    pub fn delete_archived_session(&self, session_id: &str) -> Result<bool> {
        let Some(mut record) = self.record(session_id)? else {
            return Ok(false);
        };
        if record.deleted_at.is_some() {
            return Ok(false);
        }
        let id = session_id
            .parse()
            .map_err(|error| CoreError::invalid(format!("bad session id: {error}")))?;
        let now = SystemClock.now();
        let store = EventStore::new(&self.db);
        record.deleted_at = if store.trash_session(id, now)? {
            Some(now.as_millis())
        } else {
            store
                .trashed_session_deleted_at(id)?
                .map(|value| value.as_millis())
        };
        if record.deleted_at.is_none() {
            return Ok(false);
        }
        DocumentStore::new(&self.db).put(
            ARCHIVE_COLLECTION,
            session_id,
            &serde_json::to_string(&record)?,
            None,
            now,
        )?;
        Ok(true)
    }

    /// Move all archived conversations into the managed recycle area.
    pub fn delete_all(&self) -> Result<u32> {
        let archived = self.list()?;
        let mut removed = 0u32;
        for item in archived {
            if self.delete_archived_session(&item.session_id)? {
                removed += 1;
            }
        }
        Ok(removed)
    }

    /// Permanently remove recycled files and archive markers older than the
    /// configured retention window.
    pub fn gc_deleted_sessions(&self, retention_days: u64) -> Result<u64> {
        let retention_ms = retention_days
            .saturating_mul(24)
            .saturating_mul(60)
            .saturating_mul(60)
            .saturating_mul(1_000);
        let cutoff = SystemClock
            .now()
            .as_millis()
            .saturating_sub(retention_ms.min(i64::MAX as u64) as i64);
        // A crash may have moved the file before the archive document was
        // updated. Reconcile that window before applying the retention cutoff.
        for item in self.list()? {
            if item.deleted_at.is_some() {
                continue;
            }
            let id = item
                .session_id
                .parse()
                .map_err(|error| CoreError::invalid(format!("bad archived session id: {error}")))?;
            if let Some(deleted_at) = EventStore::new(&self.db).trashed_session_deleted_at(id)? {
                let Some(mut record) = self.record(&item.session_id)? else {
                    continue;
                };
                record.deleted_at = Some(deleted_at.as_millis());
                DocumentStore::new(&self.db).put(
                    ARCHIVE_COLLECTION,
                    &item.session_id,
                    &serde_json::to_string(&record)?,
                    None,
                    SystemClock.now(),
                )?;
            }
        }
        let removed = EventStore::new(&self.db)
            .purge_trashed_sessions_before(deepagent_core::clock::Timestamp::from_millis(cutoff))?;
        let docs = DocumentStore::new(&self.db);
        for item in self.list()? {
            if item
                .deleted_at
                .is_some_and(|deleted_at| deleted_at <= cutoff)
            {
                let session_id = item.session_id.parse().map_err(|error| {
                    CoreError::invalid(format!("bad archived session id: {error}"))
                })?;
                EventStore::new(&self.db).purge_deleted_session_metadata(session_id)?;
                docs.delete(ARCHIVE_COLLECTION, &item.session_id)?;
            }
        }
        Ok(removed)
    }

    fn record(&self, session_id: &str) -> Result<Option<ArchivedConversationRecord>> {
        DocumentStore::new(&self.db)
            .get(ARCHIVE_COLLECTION, session_id)?
            .map(|document| serde_json::from_str(&document.body).map_err(CoreError::from))
            .transpose()
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
    fn archive_single_session_is_idempotent() {
        let (svc, db) = service();
        let clock = FixedClock::new(1_000);
        let session =
            Session::create_in_project(&db, &clock, Some("a"), Default::default(), Some("/work/p"))
                .unwrap();
        let session_id = session.id().to_string();

        assert!(svc.archive_session(&session_id).unwrap());
        assert!(!svc.archive_session(&session_id).unwrap());
        let archived = svc.list().unwrap();
        assert_eq!(archived.len(), 1);
        assert_eq!(archived[0].session_id, session_id);
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

    #[test]
    fn delete_recycles_files_and_unarchive_restores_them() {
        let (svc, db) = service();
        let clock = FixedClock::new(1_000);
        let mut session =
            Session::create_in_project(&db, &clock, Some("a"), Default::default(), Some("/work/p"))
                .unwrap();
        session
            .append(deepagent_core::event::EventPayload::MessageAppended {
                message: deepagent_core::message::Message::user("restore after delete"),
            })
            .unwrap();
        let session_id = session.id().to_string();
        let event_count = EventStore::new(&db).event_count(session.id()).unwrap();
        svc.archive_session(&session_id).unwrap();

        assert!(svc.delete_archived_session(&session_id).unwrap());
        assert!(svc.list().unwrap()[0].deleted_at.is_some());
        assert!(EventStore::new(&db)
            .load_session(session.id())
            .unwrap()
            .is_empty());

        assert!(svc.unarchive_session(&session_id).unwrap());
        assert!(!svc.is_archived(&session_id).unwrap());
        assert_eq!(
            EventStore::new(&db).event_count(session.id()).unwrap(),
            event_count
        );
    }

    #[test]
    fn unarchive_restores_file_moved_before_archive_marker_update() {
        let (svc, db) = service();
        let clock = FixedClock::new(1_000);
        let session = Session::create(&db, &clock, Some("interrupted delete")).unwrap();
        let session_id = session.id().to_string();
        svc.archive_session(&session_id).unwrap();
        EventStore::new(&db)
            .trash_session(
                session.id(),
                deepagent_core::clock::Timestamp::from_millis(10),
            )
            .unwrap();
        assert!(svc.list().unwrap()[0].deleted_at.is_none());

        assert!(svc.unarchive_session(&session_id).unwrap());
        assert!(EventStore::new(&db)
            .restore_trashed_session(session.id())
            .unwrap());
        assert!(!svc.is_archived(&session_id).unwrap());
    }

    #[test]
    fn gc_reconciles_file_moved_before_archive_marker_update() {
        let (svc, db) = service();
        let clock = FixedClock::new(1_000);
        let session = Session::create(&db, &clock, Some("interrupted gc")).unwrap();
        let session_id = session.id().to_string();
        svc.archive_session(&session_id).unwrap();
        EventStore::new(&db)
            .trash_session(
                session.id(),
                deepagent_core::clock::Timestamp::from_millis(10),
            )
            .unwrap();

        assert_eq!(svc.gc_deleted_sessions(0).unwrap(), 1);
        assert!(svc.list().unwrap().is_empty());
        assert!(EventStore::new(&db)
            .get_session(session.id())
            .unwrap()
            .is_none());
    }

    #[test]
    fn gc_permanently_removes_expired_files_metadata_and_search_projection() {
        let (svc, db) = service();
        let clock = FixedClock::new(1_000);
        let mut session = Session::create_in_project(
            &db,
            &clock,
            Some("expired"),
            Default::default(),
            Some("/work/p"),
        )
        .unwrap();
        session
            .append(deepagent_core::event::EventPayload::MessageAppended {
                message: deepagent_core::message::Message::user("expired searchable marker"),
            })
            .unwrap();
        let session_id = session.id();
        svc.archive_session(&session_id.to_string()).unwrap();
        svc.delete_archived_session(&session_id.to_string())
            .unwrap();

        assert_eq!(svc.gc_deleted_sessions(0).unwrap(), 1);
        assert!(svc.list().unwrap().is_empty());
        assert!(EventStore::new(&db)
            .get_session(session_id)
            .unwrap()
            .is_none());
        assert!(EventStore::new(&db)
            .search(
                "expired searchable marker",
                deepagent_persistence::event_store::SessionSearchScope::All,
                10
            )
            .unwrap()
            .is_empty());
    }
}
