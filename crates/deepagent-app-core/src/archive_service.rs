//! Archived conversation index for the app shell.
//!
//! Archiving is a durable visibility/admission state, not an event-log mutation. The
//! session event stream stays append-only and replayable; this service stores a
//! small document-store index of session ids hidden from the live sidebar.

use std::collections::HashSet;
use std::sync::Arc;

use deepagent_core::clock::{Clock, SystemClock};
use deepagent_core::error::{CoreError, Result};
use deepagent_core::id::SessionId;
use deepagent_persistence::document_store::DocumentStore;
use deepagent_persistence::event_store::EventStore;
use deepagent_persistence::Database;
use serde::{Deserialize, Serialize};

use crate::dto::{ArchiveProjectResultDto, ArchivedConversationDto};
use crate::project_service::folder_name;

const ARCHIVE_COLLECTION: &str = "archived_conversations";

#[cfg(test)]
#[path = "archive_lifecycle_tests.rs"]
mod lifecycle_tests;

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
        // A corrupt display body must not reopen a hidden execution identity.
        Ok(DocumentStore::new(&self.db)
            .list(ARCHIVE_COLLECTION)?
            .into_iter()
            .map(|document| document.id)
            .collect())
    }

    /// Whether `session_id` is archived.
    pub fn is_archived(&self, session_id: &str) -> Result<bool> {
        let session_id = session_id.parse::<SessionId>()?.to_string();
        Ok(DocumentStore::new(&self.db)
            .get(ARCHIVE_COLLECTION, &session_id)?
            .is_some())
    }

    /// Read the one durable state, including for stale loaded host views.
    pub fn require_active(&self, session_id: &str) -> Result<()> {
        if self.is_archived(session_id)? {
            return Err(CoreError::SessionArchived);
        }
        Ok(())
    }

    /// Share the existing admission/history gate: archive publication cannot
    /// pass between this check and the lifetime of the admitted owner.
    pub fn active_guard(
        &self,
        session_id: SessionId,
    ) -> Result<deepagent_persistence::event_store::SessionActivityGuard> {
        let guard = EventStore::new(&self.db).acquire_activity(session_id)?;
        self.require_active(&session_id.to_string())?;
        Ok(guard)
    }

    /// Archive every non-archived conversation under `project_path`.
    pub fn archive_project(&self, project_path: &str) -> Result<ArchiveProjectResultDto> {
        let event_store = EventStore::new(&self.db);
        let existing = self.archived_ids()?;
        let project_name = folder_name(project_path);
        let mut archived_count = 0u32;
        let mut sessions = event_store
            .list_sessions()?
            .into_iter()
            .filter(|session| {
                session.project.as_deref() == Some(project_path)
                    && !existing.contains(&session.id.to_string())
            })
            .collect::<Vec<_>>();
        sessions.sort_by_key(|session| session.id);
        // Preflight every selected owner before publishing any marker. Batch
        // storage failure retains the existing partial-write error contract.
        let _controls = sessions
            .iter()
            .map(|session| event_store.begin_control(session.id))
            .collect::<Result<Vec<_>>>()?;
        for session in sessions {
            archived_count += u32::from(self.archive_in_control(session)?);
        }

        Ok(ArchiveProjectResultDto {
            project_path: project_path.to_string(),
            project_name,
            archived_count,
        })
    }

    /// Archive one conversation by session id. Returns whether it was newly archived.
    pub fn archive_session(&self, session_id: &str) -> Result<bool> {
        let id = session_id.parse::<SessionId>()?;
        let store = EventStore::new(&self.db);
        let _control = store.begin_control(id)?;
        let session = store
            .get_session(id)?
            .ok_or_else(|| CoreError::not_found(format!("session {session_id}")))?;
        self.archive_in_control(session)
    }

    fn archive_in_control(
        &self,
        session: deepagent_persistence::event_store::SessionRecord,
    ) -> Result<bool> {
        let session_id = session.id.to_string();
        if self.is_archived(&session_id)? {
            return Ok(false);
        }

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
            &session_id,
            &serde_json::to_string(&record)?,
            None,
            SystemClock.now(),
        )?;
        tracing::info!(%session_id, "idle session archived; new admission requires explicit restore");
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
        let id = session_id.parse::<SessionId>()?;
        let session_id = id.to_string();
        let store = EventStore::new(&self.db);
        let _control = store.begin_control(id)?;
        let Some(_) = self.record(&session_id)? else {
            return Ok(false);
        };
        if !store.restore_trashed_session(id)? {
            return Err(CoreError::not_found(format!(
                "session files for {session_id} are no longer available"
            )));
        }
        let changed = DocumentStore::new(&self.db).delete(ARCHIVE_COLLECTION, &session_id)?;
        tracing::info!(%session_id, changed, "session explicitly unarchived");
        Ok(changed)
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
                let store = EventStore::new(&self.db);
                if store.purge_deleted_session_metadata(session_id)?
                    || store.get_session(session_id)?.is_none()
                {
                    docs.delete(ARCHIVE_COLLECTION, &item.session_id)?;
                }
            }
        }
        let results = deepagent_persistence::artifact_store::ArtifactStore::new(&self.db)
            .gc_tool_results_before(cutoff)?;
        if results > 0 {
            tracing::info!(results, "collected expired unreferenced tool results");
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
