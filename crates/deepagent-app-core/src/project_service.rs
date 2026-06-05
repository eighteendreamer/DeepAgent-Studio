//! Multi-project registry (Phase C — projects → sessions in the sidebar).
//!
//! The product model: a **project is a folder**. The sidebar lists projects;
//! each project owns the sessions created while it was active; the agent's file
//! operations are rooted at the active project's folder.
//!
//! This service persists the set of opened projects + which one is active in
//! the document store, and enriches the list with live session counts from the
//! event store. Sessions themselves carry their project path (the stable key);
//! the folder *name* is derived for display.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use deepagent_core::clock::{Clock, SystemClock};
use deepagent_core::error::{CoreError, Result};
use deepagent_persistence::document_store::DocumentStore;
use deepagent_persistence::event_store::EventStore;
use deepagent_persistence::Database;
use serde::{Deserialize, Serialize};

use crate::dto::ProjectDto;
use crate::ArchiveService;

/// Document-store location for the project registry.
const PROJECTS_COLLECTION: &str = "projects";
const PROJECTS_ID: &str = "registry";

/// Persisted project registry: the opened folder paths + the active one.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct ProjectRegistry {
    /// Absolute folder paths the user has opened (insertion order preserved).
    #[serde(default)]
    paths: Vec<String>,
    /// The active project path (agent ops + new sessions attach here).
    #[serde(default)]
    active: Option<String>,
    /// Project paths pinned to the top of the sidebar.
    #[serde(default)]
    pinned: HashSet<String>,
    /// User-facing project names keyed by project path.
    #[serde(default)]
    names: HashMap<String, String>,
}

/// Derive the display name (last path component) from a folder path.
pub fn folder_name(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| path.to_string())
}

/// Manages the multi-project registry over the shared database.
pub struct ProjectService {
    db: Arc<Database>,
}

impl ProjectService {
    /// Build over the shared database.
    pub fn new(db: Arc<Database>) -> Self {
        Self { db }
    }

    /// Ensure `path` is registered and active (used to seed the launch project).
    /// Idempotent: re-opening an existing project just marks it active.
    pub fn ensure_default(&self, path: &str) -> Result<()> {
        let mut reg = self.load()?;
        let norm = path.trim().to_string();
        if norm.is_empty() {
            return Ok(());
        }
        if !reg.paths.iter().any(|p| p == &norm) {
            reg.paths.push(norm.clone());
        }
        if reg.active.is_none() {
            reg.active = Some(norm);
        }
        self.save(&reg)
    }

    /// Add (open) a project folder. Becomes active. Returns its DTO.
    pub fn add_project(&self, path: &str) -> Result<ProjectDto> {
        let norm = path.trim().to_string();
        if norm.is_empty() {
            return Err(CoreError::invalid("project path must not be empty"));
        }
        let mut reg = self.load()?;
        if !reg.paths.iter().any(|p| p == &norm) {
            reg.paths.push(norm.clone());
        }
        reg.active = Some(norm.clone());
        self.save(&reg)?;
        let reg = self.load()?;
        self.enrich(&norm, &reg)
    }

    /// Remove (close) a project from the registry. Its sessions are left intact
    /// in the database (only the registry entry is removed). Returns whether it
    /// existed. If the active project is removed, active falls back to the first
    /// remaining project (or none).
    pub fn remove_project(&self, path: &str) -> Result<bool> {
        let mut reg = self.load()?;
        let before = reg.paths.len();
        reg.paths.retain(|p| p != path);
        reg.pinned.remove(path);
        reg.names.remove(path);
        let existed = reg.paths.len() != before;
        if reg.active.as_deref() == Some(path) {
            reg.active = reg.paths.first().cloned();
        }
        if existed {
            self.save(&reg)?;
        }
        Ok(existed)
    }

    /// Set the active project (must already be registered).
    pub fn set_active(&self, path: &str) -> Result<()> {
        let mut reg = self.load()?;
        if !reg.paths.iter().any(|p| p == path) {
            return Err(CoreError::not_found(format!("project '{path}' not opened")));
        }
        reg.active = Some(path.to_string());
        self.save(&reg)
    }

    /// The active project path, if any.
    pub fn active(&self) -> Result<Option<String>> {
        Ok(self.load()?.active)
    }

    /// Registered project paths currently visible in the sidebar.
    pub fn registered_paths(&self) -> Result<HashSet<String>> {
        Ok(self.load()?.paths.into_iter().collect())
    }

    /// Set whether a project is pinned to the top of the sidebar.
    pub fn set_pinned(&self, path: &str, pinned: bool) -> Result<ProjectDto> {
        let mut reg = self.load()?;
        if !reg.paths.iter().any(|p| p == path) {
            return Err(CoreError::not_found(format!("project '{path}' not opened")));
        }
        if pinned {
            reg.pinned.insert(path.to_string());
        } else {
            reg.pinned.remove(path);
        }
        self.save(&reg)?;
        self.enrich(path, &reg)
    }

    /// Set a display alias for a project without renaming the folder on disk.
    pub fn rename_project(&self, path: &str, name: &str) -> Result<ProjectDto> {
        let clean = name.trim();
        if clean.is_empty() {
            return Err(CoreError::invalid("project name must not be empty"));
        }
        let mut reg = self.load()?;
        if !reg.paths.iter().any(|p| p == path) {
            return Err(CoreError::not_found(format!("project '{path}' not opened")));
        }
        reg.names.insert(path.to_string(), clean.to_string());
        self.save(&reg)?;
        self.enrich(path, &reg)
    }

    /// Display name for a path, honoring a user-provided alias when present.
    pub fn display_name(&self, path: &str) -> Result<String> {
        let reg = self.load()?;
        Ok(display_name_from_registry(path, &reg))
    }

    /// All registered projects as DTOs (enriched with session counts), ordered
    /// by most-recent session activity then registry order.
    pub fn list(&self) -> Result<Vec<ProjectDto>> {
        let reg = self.load()?;
        let mut out = Vec::with_capacity(reg.paths.len());
        for path in &reg.paths {
            out.push(self.enrich(path, &reg)?);
        }
        // Pinned projects first, then most-recent activity; zero-session projects
        // keep registry order at the end because sort_by_key is stable.
        out.sort_by_key(|p| (!p.pinned, std::cmp::Reverse(p.updated_at)));
        Ok(out)
    }

    /// Build a [`ProjectDto`] for `path` with live session count + last update.
    fn enrich(&self, path: &str, reg: &ProjectRegistry) -> Result<ProjectDto> {
        let store = EventStore::new(&self.db);
        let archived = ArchiveService::new(self.db.clone()).archived_ids()?;
        let mut count = 0u32;
        let mut updated_at = 0i64;
        // Count sessions belonging to this project.
        for s in store.list_sessions()? {
            if s.project.as_deref() == Some(path) {
                if archived.contains(&s.id.to_string()) {
                    continue;
                }
                count += 1;
                updated_at = updated_at.max(s.updated_at.as_millis());
            }
        }
        Ok(ProjectDto {
            name: display_name_from_registry(path, reg),
            path: path.to_string(),
            pinned: reg.pinned.contains(path),
            session_count: count,
            updated_at,
        })
    }

    fn load(&self) -> Result<ProjectRegistry> {
        let store = DocumentStore::new(&self.db);
        match store.get(PROJECTS_COLLECTION, PROJECTS_ID)? {
            Some(doc) => Ok(serde_json::from_str(&doc.body).unwrap_or_default()),
            None => Ok(ProjectRegistry::default()),
        }
    }

    fn save(&self, reg: &ProjectRegistry) -> Result<()> {
        let store = DocumentStore::new(&self.db);
        let body = serde_json::to_string(reg)?;
        store.put(
            PROJECTS_COLLECTION,
            PROJECTS_ID,
            &body,
            None,
            SystemClock.now(),
        )
    }
}

fn display_name_from_registry(path: &str, reg: &ProjectRegistry) -> String {
    reg.names
        .get(path)
        .filter(|name| !name.trim().is_empty())
        .cloned()
        .unwrap_or_else(|| folder_name(path))
}

#[cfg(test)]
mod tests {
    use super::*;
    use deepagent_core::clock::FixedClock;
    use deepagent_session::Session;

    fn service() -> (ProjectService, Arc<Database>) {
        let db = Arc::new(Database::open_in_memory().unwrap());
        (ProjectService::new(db.clone()), db)
    }

    #[test]
    fn folder_name_extraction() {
        assert_eq!(folder_name("/work/小红草"), "小红草");
        assert_eq!(folder_name("C:/code/Looker-v2"), "Looker-v2");
    }

    #[test]
    fn add_set_active_and_list() {
        let (svc, _db) = service();
        svc.add_project("/work/alpha").unwrap();
        let beta = svc.add_project("/work/beta").unwrap();
        assert_eq!(beta.name, "beta");
        // Last added is active.
        assert_eq!(svc.active().unwrap().as_deref(), Some("/work/beta"));
        svc.set_active("/work/alpha").unwrap();
        assert_eq!(svc.active().unwrap().as_deref(), Some("/work/alpha"));
        assert_eq!(svc.list().unwrap().len(), 2);
    }

    #[test]
    fn ensure_default_is_idempotent() {
        let (svc, _db) = service();
        svc.ensure_default("/work/x").unwrap();
        svc.ensure_default("/work/x").unwrap();
        assert_eq!(svc.list().unwrap().len(), 1);
        assert_eq!(svc.active().unwrap().as_deref(), Some("/work/x"));
    }

    #[test]
    fn list_counts_sessions_in_project() {
        let (svc, db) = service();
        svc.add_project("/work/p").unwrap();
        let clock = FixedClock::new(1_000);
        // Two sessions in /work/p, one elsewhere.
        Session::create_in_project(&db, &clock, Some("a"), Default::default(), Some("/work/p"))
            .unwrap();
        Session::create_in_project(&db, &clock, Some("b"), Default::default(), Some("/work/p"))
            .unwrap();
        Session::create_in_project(
            &db,
            &clock,
            Some("c"),
            Default::default(),
            Some("/work/other"),
        )
        .unwrap();
        let p = svc
            .list()
            .unwrap()
            .into_iter()
            .find(|p| p.path == "/work/p")
            .unwrap();
        assert_eq!(p.session_count, 2);
    }

    #[test]
    fn remove_reassigns_active() {
        let (svc, _db) = service();
        svc.add_project("/work/a").unwrap();
        svc.add_project("/work/b").unwrap();
        svc.set_active("/work/b").unwrap();
        assert!(svc.remove_project("/work/b").unwrap());
        // Active falls back to the remaining project.
        assert_eq!(svc.active().unwrap().as_deref(), Some("/work/a"));
        assert!(!svc.remove_project("/work/b").unwrap());
    }

    #[test]
    fn pin_and_rename_project() {
        let (svc, _db) = service();
        svc.add_project("/work/a").unwrap();
        svc.add_project("/work/b").unwrap();

        let renamed = svc.rename_project("/work/b", "Backend").unwrap();
        assert_eq!(renamed.name, "Backend");
        assert_eq!(svc.display_name("/work/b").unwrap(), "Backend");

        let pinned = svc.set_pinned("/work/b", true).unwrap();
        assert!(pinned.pinned);
        let listed = svc.list().unwrap();
        assert_eq!(listed.first().unwrap().path, "/work/b");

        let unpinned = svc.set_pinned("/work/b", false).unwrap();
        assert!(!unpinned.pinned);
    }

    #[test]
    fn set_active_unknown_errors() {
        let (svc, _db) = service();
        assert!(svc.set_active("/nope").is_err());
    }
}
