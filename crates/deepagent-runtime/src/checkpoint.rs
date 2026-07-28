//! Incremental file checkpoints for rewind and crash recovery.

use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};

use deepagent_core::error::{CoreError, Result};
use deepagent_persistence::checkpoint_store::{CheckpointRecord, CheckpointStore};
use deepagent_persistence::Database;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CheckpointEntry {
    path: String,
    existed: bool,
    was_dir: bool,
    backup: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CheckpointManifest {
    storage_dir: String,
    entries: Vec<CheckpointEntry>,
    #[serde(default)]
    external_evidence: Vec<MutationEvidence>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MutationKind {
    Created,
    Modified,
    Deleted,
    Unchanged,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MutationEvidence {
    pub path: PathBuf,
    pub kind: MutationKind,
}

/// Captures each mutation target once, immediately before its first write.
pub struct CheckpointManager {
    db: Arc<Database>,
    id: String,
    run_id: String,
    session_sequence: i64,
    workspace_root: PathBuf,
    storage_dir: PathBuf,
    captured: Mutex<HashSet<PathBuf>>,
    entries: Mutex<Vec<CheckpointEntry>>,
    external_evidence: Mutex<Vec<MutationEvidence>>,
}

impl std::fmt::Debug for CheckpointManager {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CheckpointManager")
            .field("id", &self.id)
            .field("run_id", &self.run_id)
            .field("workspace_root", &self.workspace_root)
            .finish()
    }
}

impl CheckpointManager {
    pub fn new(
        db: Arc<Database>,
        run_id: impl Into<String>,
        session_sequence: i64,
        workspace_root: impl AsRef<Path>,
        checkpoint_root: impl AsRef<Path>,
    ) -> Result<Self> {
        let id = format!("chk_{}", deepagent_core::id::EventId::new());
        let workspace_root = normalize_path(workspace_root.as_ref());
        let storage_dir = checkpoint_root.as_ref().join(&id);
        std::fs::create_dir_all(storage_dir.join("files")).map_err(|error| {
            CoreError::other(format!("failed to create checkpoint directory: {error}"))
        })?;
        Ok(Self {
            db,
            id,
            run_id: run_id.into(),
            session_sequence,
            workspace_root,
            storage_dir,
            captured: Mutex::new(HashSet::new()),
            entries: Mutex::new(Vec::new()),
            external_evidence: Mutex::new(Vec::new()),
        })
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    /// The session event sequence this checkpoint is anchored to (the last
    /// event before the owning run started). Rewind/fork restore every
    /// checkpoint with a sequence GREATER than the rewind target.
    pub fn session_sequence(&self) -> i64 {
        self.session_sequence
    }

    pub fn capture_before(&self, path: impl AsRef<Path>) -> Result<bool> {
        let path = self.confined(path.as_ref())?;
        {
            let mut captured = self.captured.lock().unwrap_or_else(|p| p.into_inner());
            if !captured.insert(path.clone()) {
                return Ok(false);
            }
        }

        let existed = path.exists();
        let was_dir = path.is_dir();
        let backup = if existed {
            let index = self.entries.lock().unwrap_or_else(|p| p.into_inner()).len();
            let destination = self.storage_dir.join("files").join(index.to_string());
            if was_dir {
                copy_directory(&path, &destination)?;
            } else {
                if let Some(parent) = destination.parent() {
                    std::fs::create_dir_all(parent).map_err(fs_error)?;
                }
                std::fs::copy(&path, &destination).map_err(fs_error)?;
            }
            Some(destination.to_string_lossy().to_string())
        } else {
            None
        };
        self.entries
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(CheckpointEntry {
                path: path.to_string_lossy().to_string(),
                existed,
                was_dir,
                backup,
            });
        // Crash consistency: persist the manifest incrementally after every
        // new capture instead of only at run end. If the process dies
        // mid-run, the backups already on disk stay reachable through the
        // checkpoints table, so startup recovery / rewind can still restore
        // files touched before the crash.
        if let Err(error) = self.commit() {
            tracing::warn!(error = %error, "failed to persist incremental checkpoint manifest");
        }
        Ok(true)
    }

    pub(crate) fn normalize_target(&self, path: impl AsRef<Path>) -> Result<PathBuf> {
        self.confined(path.as_ref())
    }

    pub fn commit(&self) -> Result<String> {
        let manifest = CheckpointManifest {
            storage_dir: self.storage_dir.to_string_lossy().to_string(),
            entries: self
                .entries
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .clone(),
            external_evidence: self
                .external_evidence
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .clone(),
        };
        let record = CheckpointRecord {
            id: self.id.clone(),
            run_id: self.run_id.clone(),
            session_sequence: self.session_sequence,
            workspace_root: self.workspace_root.to_string_lossy().to_string(),
            manifest: serde_json::to_string(&manifest)
                .map_err(|error| CoreError::Persistence(error.to_string()))?,
            created_at: now_millis(),
        };
        CheckpointStore::new(&self.db).put(&record)?;
        Ok(self.id.clone())
    }

    /// Compare captured pre-mutation state with the current workspace. This is
    /// factual completion evidence: it reflects what remains on disk, not what
    /// a tool or model claimed to have changed.
    pub fn mutation_evidence(&self) -> Result<Vec<MutationEvidence>> {
        let entries = self
            .entries
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        let mut evidence = entries
            .iter()
            .map(|entry| {
                let path = PathBuf::from(&entry.path);
                let kind = if !entry.existed {
                    if path.exists() {
                        MutationKind::Created
                    } else {
                        MutationKind::Unchanged
                    }
                } else if !path.exists() {
                    MutationKind::Deleted
                } else {
                    let backup = entry.backup.as_ref().ok_or_else(|| {
                        CoreError::Persistence(format!(
                            "checkpoint entry has no backup: {}",
                            path.display()
                        ))
                    })?;
                    if filesystem_entries_equal(Path::new(backup), &path)? {
                        MutationKind::Unchanged
                    } else {
                        MutationKind::Modified
                    }
                };
                Ok(MutationEvidence { path, kind })
            })
            .collect::<Result<Vec<_>>>()?;
        for item in self
            .external_evidence
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .iter()
        {
            if !evidence.contains(item) {
                evidence.push(item.clone());
            }
        }
        Ok(evidence)
    }

    /// Merge factual effects verified by a nested execution root, such as a
    /// retained Git worktree, into this run's completion evidence.
    pub fn record_external_evidence(&self, evidence: impl IntoIterator<Item = MutationEvidence>) {
        let mut external = self
            .external_evidence
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        for item in evidence {
            if !external.contains(&item) {
                external.push(item);
            }
        }
    }

    pub fn restore(db: &Database, checkpoint_id: &str) -> Result<Vec<PathBuf>> {
        let record = CheckpointStore::new(db)
            .get(checkpoint_id)?
            .ok_or_else(|| CoreError::invalid(format!("checkpoint not found: {checkpoint_id}")))?;
        let root = normalize_path(Path::new(&record.workspace_root));
        let manifest: CheckpointManifest = serde_json::from_str(&record.manifest)
            .map_err(|error| CoreError::Persistence(error.to_string()))?;
        let storage_dir = normalize_path(Path::new(&manifest.storage_dir));
        let mut restored = Vec::new();
        // Reverse order handles nested paths without a child restore being
        // overwritten by an earlier parent snapshot.
        for entry in manifest.entries.iter().rev() {
            let target = normalize_path(Path::new(&entry.path));
            if !target.starts_with(&root) {
                return Err(CoreError::invalid(format!(
                    "checkpoint path escapes workspace: {}",
                    target.display()
                )));
            }
            remove_existing(&target)?;
            if entry.existed {
                let backup =
                    entry.backup.as_ref().map(PathBuf::from).ok_or_else(|| {
                        CoreError::Persistence("missing checkpoint backup".into())
                    })?;
                let backup = normalize_path(&backup);
                if !backup.starts_with(&storage_dir) {
                    return Err(CoreError::invalid("checkpoint backup escapes storage"));
                }
                if entry.was_dir {
                    copy_directory(&backup, &target)?;
                } else {
                    if let Some(parent) = target.parent() {
                        std::fs::create_dir_all(parent).map_err(fs_error)?;
                    }
                    std::fs::copy(&backup, &target).map_err(fs_error)?;
                }
            }
            restored.push(target);
        }
        Ok(restored)
    }

    fn confined(&self, path: &Path) -> Result<PathBuf> {
        let path = if path.is_absolute() {
            normalize_path(path)
        } else {
            normalize_path(&self.workspace_root.join(path))
        };
        if !path.starts_with(&self.workspace_root) {
            return Err(CoreError::invalid(format!(
                "checkpoint target escapes workspace: {}",
                path.display()
            )));
        }
        Ok(path)
    }
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

fn copy_directory(source: &Path, destination: &Path) -> Result<()> {
    std::fs::create_dir_all(destination).map_err(fs_error)?;
    for entry in std::fs::read_dir(source).map_err(fs_error)? {
        let entry = entry.map_err(fs_error)?;
        let target = destination.join(entry.file_name());
        if entry.file_type().map_err(fs_error)?.is_dir() {
            copy_directory(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), target).map_err(fs_error)?;
        }
    }
    Ok(())
}

fn filesystem_entries_equal(left: &Path, right: &Path) -> Result<bool> {
    if left.is_dir() != right.is_dir() || left.is_file() != right.is_file() {
        return Ok(false);
    }
    if left.is_file() {
        return Ok(
            std::fs::read(left).map_err(fs_error)? == std::fs::read(right).map_err(fs_error)?
        );
    }
    if !left.is_dir() {
        return Ok(false);
    }

    let mut left_entries = directory_entries(left)?;
    let mut right_entries = directory_entries(right)?;
    left_entries.sort_by(|a, b| a.0.cmp(&b.0));
    right_entries.sort_by(|a, b| a.0.cmp(&b.0));
    if left_entries.len() != right_entries.len() {
        return Ok(false);
    }
    for ((left_name, left_path), (right_name, right_path)) in
        left_entries.iter().zip(&right_entries)
    {
        if left_name != right_name || !filesystem_entries_equal(left_path, right_path)? {
            return Ok(false);
        }
    }
    Ok(true)
}

fn directory_entries(path: &Path) -> Result<Vec<(std::ffi::OsString, PathBuf)>> {
    std::fs::read_dir(path)
        .map_err(fs_error)?
        .map(|entry| {
            let entry = entry.map_err(fs_error)?;
            Ok((entry.file_name(), entry.path()))
        })
        .collect()
}

fn remove_existing(path: &Path) -> Result<()> {
    if path.is_dir() {
        std::fs::remove_dir_all(path).map_err(fs_error)?;
    } else if path.exists() {
        std::fs::remove_file(path).map_err(fs_error)?;
    }
    Ok(())
}

fn fs_error(error: std::io::Error) -> CoreError {
    CoreError::other(format!("checkpoint filesystem error: {error}"))
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    use deepagent_persistence::run_store::RunStore;

    #[test]
    fn restore_reverts_modified_deleted_and_created_files() {
        let db = Arc::new(Database::open_in_memory().unwrap());
        db.with_conn(|conn| {
            conn.execute(
                "INSERT INTO sessions (id, created_at, updated_at) VALUES ('s1', 1, 1)",
                [],
            )
            .map_err(|error| CoreError::Persistence(error.to_string()))?;
            Ok(())
        })
        .unwrap();
        RunStore::new(&db).create("r1", "s1", None, 1).unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let storage = tempfile::tempdir().unwrap();
        let modified = workspace.path().join("modified.txt");
        let deleted = workspace.path().join("deleted.txt");
        let created = workspace.path().join("created.txt");
        std::fs::write(&modified, "before").unwrap();
        std::fs::write(&deleted, "keep").unwrap();

        let checkpoint =
            CheckpointManager::new(db.clone(), "r1", 0, workspace.path(), storage.path()).unwrap();
        checkpoint.capture_before(&modified).unwrap();
        checkpoint.capture_before(&deleted).unwrap();
        checkpoint.capture_before(&created).unwrap();
        std::fs::write(&modified, "after").unwrap();
        std::fs::remove_file(&deleted).unwrap();
        std::fs::write(&created, "new").unwrap();
        let id = checkpoint.commit().unwrap();

        let evidence = checkpoint.mutation_evidence().unwrap();
        assert_eq!(
            evidence.iter().map(|item| item.kind).collect::<Vec<_>>(),
            vec![
                MutationKind::Modified,
                MutationKind::Deleted,
                MutationKind::Created
            ]
        );

        let restored = CheckpointManager::restore(&db, &id).unwrap();
        assert_eq!(std::fs::read_to_string(modified).unwrap(), "before");
        assert_eq!(std::fs::read_to_string(deleted).unwrap(), "keep");
        assert!(!created.exists());
        assert_eq!(restored.len(), 3);
    }

    #[test]
    fn incremental_commit_survives_crash_without_final_commit() {
        let db = Arc::new(Database::open_in_memory().unwrap());
        db.with_conn(|conn| {
            conn.execute(
                "INSERT INTO sessions (id, created_at, updated_at) VALUES ('s1', 1, 1)",
                [],
            )
            .map_err(|error| CoreError::Persistence(error.to_string()))?;
            Ok(())
        })
        .unwrap();
        RunStore::new(&db).create("r1", "s1", None, 1).unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let storage = tempfile::tempdir().unwrap();
        let file = workspace.path().join("work.txt");
        std::fs::write(&file, "before").unwrap();

        let checkpoint =
            CheckpointManager::new(db.clone(), "r1", 7, workspace.path(), storage.path()).unwrap();
        checkpoint.capture_before(&file).unwrap();
        std::fs::write(&file, "after").unwrap();
        let id = checkpoint.id().to_string();
        // Simulate a crash: drop the manager WITHOUT calling commit().
        drop(checkpoint);

        // The incremental commit already persisted the manifest, so restore
        // works from the table alone after a restart.
        let record = CheckpointStore::new(&db).get(&id).unwrap();
        assert!(record.is_some(), "manifest must be durable before run end");
        assert_eq!(record.unwrap().session_sequence, 7);
        CheckpointManager::restore(&db, &id).unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "before");
    }

    #[test]
    fn capture_rejects_workspace_escape() {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let workspace = tempfile::tempdir().unwrap();
        let storage = tempfile::tempdir().unwrap();
        let checkpoint =
            CheckpointManager::new(db, "r", 0, workspace.path(), storage.path()).unwrap();
        assert!(checkpoint.capture_before("../outside.txt").is_err());
    }

    #[test]
    fn evidence_marks_same_content_as_unchanged() {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let workspace = tempfile::tempdir().unwrap();
        let storage = tempfile::tempdir().unwrap();
        let path = workspace.path().join("same.txt");
        std::fs::write(&path, "same").unwrap();
        let checkpoint =
            CheckpointManager::new(db, "r", 0, workspace.path(), storage.path()).unwrap();
        checkpoint.capture_before(&path).unwrap();
        std::fs::write(&path, "same").unwrap();

        assert_eq!(
            checkpoint.mutation_evidence().unwrap()[0].kind,
            MutationKind::Unchanged
        );
    }

    #[test]
    fn external_worktree_evidence_is_deduplicated_and_persisted() {
        let db = Arc::new(Database::open_in_memory().unwrap());
        db.with_conn(|conn| {
            conn.execute(
                "INSERT INTO sessions (id, created_at, updated_at) VALUES ('s1', 1, 1)",
                [],
            )
            .map_err(|error| CoreError::Persistence(error.to_string()))?;
            Ok(())
        })
        .unwrap();
        RunStore::new(&db).create("parent", "s1", None, 1).unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let storage = tempfile::tempdir().unwrap();
        let checkpoint =
            CheckpointManager::new(db.clone(), "parent", 0, workspace.path(), storage.path())
                .unwrap();
        let external = MutationEvidence {
            path: PathBuf::from("C:/retained-worktree/created.txt"),
            kind: MutationKind::Created,
        };
        checkpoint.record_external_evidence([external.clone(), external.clone()]);

        assert_eq!(checkpoint.mutation_evidence().unwrap(), vec![external]);
        let id = checkpoint.commit().unwrap();
        let record = CheckpointStore::new(&db).get(&id).unwrap().unwrap();
        let manifest: serde_json::Value = serde_json::from_str(&record.manifest).unwrap();
        assert_eq!(manifest["external_evidence"].as_array().unwrap().len(), 1);
        assert_eq!(manifest["external_evidence"][0]["kind"], "created");
    }
}
