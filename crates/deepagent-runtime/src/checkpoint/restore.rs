//! Prepared, compensating restore for an ordered set of existing checkpoints.
//!
//! All replacement trees are built before workspace publication. Overlapping
//! targets share one staged root, preserving reverse-entry/checkpoint ordering.
//! Previous roots remain reachable until the caller's history commit succeeds.
//! This is an in-process compensation boundary, not a crash-atomic transaction
//! or a lock against unrelated workspace writers.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use deepagent_core::{CoreError, Result};
use deepagent_persistence::checkpoint_store::CheckpointRecord;
use deepagent_persistence::fs_commit::rename_no_replace;

use super::{normalize_path, CheckpointManifest};

struct RestoreEntry {
    target: PathBuf,
    reported: PathBuf,
    backup: Option<PathBuf>,
}

struct StagedRoot {
    target: PathBuf,
    area: PathBuf,
    previous_moved: bool,
    published: bool,
}

#[derive(Default)]
pub(super) struct RestoreTransaction {
    stages: Vec<StagedRoot>,
    created_parents: Vec<PathBuf>,
    // Once business data is moved, Drop must never delete its only old copy.
    dirty: bool,
}

impl RestoreTransaction {
    pub(super) fn run<T>(
        records: &[CheckpointRecord],
        finalize: impl FnOnce(Vec<PathBuf>) -> Result<T>,
    ) -> Result<T> {
        let entries = parse_entries(records)?;
        if entries.is_empty() {
            return finalize(Vec::new());
        }
        let paths = entries.iter().map(|entry| entry.reported.clone()).collect();
        let mut transaction = Self::prepare(&entries)?;
        transaction.dirty = true;
        let result = transaction.publish().and_then(|()| finalize(paths));
        match result {
            Ok(value) => {
                transaction.dirty = false;
                Ok(value)
            }
            Err(error) => Err(transaction.compensate(error)),
        }
    }

    fn compensate(&mut self, error: CoreError) -> CoreError {
        if let Err(rollback) = self.rollback() {
            let recovery = self
                .stages
                .iter()
                .map(|stage| &stage.area)
                .collect::<Vec<_>>();
            return CoreError::other(format!("checkpoint restore failed: {error}; compensation incomplete: {rollback}; recovery directories retained: {recovery:?}"));
        }
        self.dirty = false;
        tracing::warn!(%error, workspace_restored = true, "checkpoint restore rejected; compensated workspace publication");
        error
    }

    fn prepare(entries: &[RestoreEntry]) -> Result<Self> {
        let mut roots: Vec<PathBuf> = entries.iter().map(|entry| entry.target.clone()).collect();
        roots.sort_by_key(|path| path.components().count());
        let mut transaction = Self::default();
        for target in roots {
            if transaction
                .stages
                .iter()
                .any(|stage| target.starts_with(&stage.target))
            {
                continue;
            }
            let mut parent = target
                .parent()
                .ok_or_else(|| CoreError::invalid("restore target has no parent"))?;
            while !parent.try_exists().map_err(fs_error)? {
                parent = parent
                    .parent()
                    .ok_or_else(|| CoreError::invalid("restore parent unavailable"))?;
            }
            // Sibling/ancestor location stays on the target filesystem. No
            // cross-volume copy/delete fallback is used for publication.
            let area = tempfile::Builder::new()
                .prefix(".deepagent-restore-")
                .tempdir_in(parent)
                .map_err(fs_error)?
                .keep();
            transaction.stages.push(StagedRoot {
                target: target.clone(),
                area: area.clone(),
                previous_moved: false,
                published: false,
            });
        }
        for stage in &transaction.stages {
            // Every minimal root is itself restored by at least one entry.
            // Its last complete snapshot supersedes all earlier descendants;
            // only later descendant operations can affect its final contents.
            let start = entries
                .iter()
                .rposition(|entry| entry.target == stage.target)
                .expect("root is a restore target");
            for entry in entries[start..]
                .iter()
                .filter(|entry| entry.target.starts_with(&stage.target))
            {
                let suffix = entry
                    .target
                    .strip_prefix(&stage.target)
                    .expect("matching root");
                let desired = if suffix.as_os_str().is_empty() {
                    stage.area.join("desired")
                } else {
                    stage.area.join("desired").join(suffix)
                };
                remove_staged(&desired)?;
                if let Some(backup) = &entry.backup {
                    if let Some(parent) = desired.parent() {
                        fs::create_dir_all(parent).map_err(fs_error)?;
                    }
                    copy_entry(backup, &desired)?;
                }
            }
        }
        Ok(transaction)
    }

    fn publish(&mut self) -> Result<()> {
        for stage in &mut self.stages {
            create_parents(&stage.target, &mut self.created_parents)?;
            if exists(&stage.target)? {
                rename_no_replace(&stage.target, &stage.area.join("previous")).map_err(fs_error)?;
                stage.previous_moved = true;
            }
            let desired = stage.area.join("desired");
            if exists(&desired)? {
                rename_no_replace(&desired, &stage.target).map_err(fs_error)?;
                stage.published = true;
            }
        }
        Ok(())
    }

    fn rollback(&mut self) -> Result<()> {
        let mut failures = Vec::new();
        for stage in self.stages.iter_mut().rev() {
            let result = (|| -> Result<()> {
                if stage.published {
                    // Retain the replacement as well if restoring the previous
                    // target fails. Never recursively delete a live target.
                    rename_no_replace(&stage.target, &stage.area.join("desired"))
                        .map_err(fs_error)?;
                    stage.published = false;
                }
                if stage.previous_moved {
                    rename_no_replace(&stage.area.join("previous"), &stage.target)
                        .map_err(fs_error)?;
                    stage.previous_moved = false;
                }
                Ok(())
            })();
            if let Err(error) = result {
                failures.push(format!("{}: {error}", stage.target.display()));
            }
        }
        for parent in self.created_parents.iter().rev() {
            if let Err(error) = fs::remove_dir(parent) {
                failures.push(format!(
                    "remove newly created parent {}: {error}",
                    parent.display()
                ));
            }
        }
        if failures.is_empty() {
            Ok(())
        } else {
            Err(CoreError::other(failures.join("; ")))
        }
    }
}

impl Drop for RestoreTransaction {
    fn drop(&mut self) {
        for stage in &self.stages {
            if self.dirty {
                tracing::error!(target = %stage.target.display(), recovery_path = %stage.area.display(), "checkpoint recovery directory retained; restore did not finish");
            } else if let Err(error) = fs::remove_dir_all(&stage.area) {
                // Final publication/history commit already succeeded, or the
                // pre-call state was restored. Cleanup is not a failed restore.
                tracing::warn!(%error, recovery_path = %stage.area.display(), "checkpoint staging cleanup failed");
            }
        }
    }
}

fn parse_entries(records: &[CheckpointRecord]) -> Result<Vec<RestoreEntry>> {
    let mut entries = Vec::new();
    let mut storage_roots = Vec::new();
    for record in records {
        let root = fs::canonicalize(&record.workspace_root).map_err(fs_error)?;
        let lexical_root = absolute(Path::new(&record.workspace_root))?;
        let manifest: CheckpointManifest = serde_json::from_str(&record.manifest)
            .map_err(|error| CoreError::Persistence(error.to_string()))?;
        let storage = fs::canonicalize(&manifest.storage_dir).map_err(fs_error)?;
        storage_roots.push(storage.clone());
        for entry in manifest.entries.iter().rev() {
            let lexical_target = absolute(Path::new(&entry.path))?;
            let suffix = lexical_target
                .strip_prefix(&lexical_root)
                .map_err(|_| CoreError::invalid("checkpoint path escapes workspace"))?;
            let target = resolve_target(&root.join(suffix))?;
            if target == root || !target.starts_with(&root) {
                return Err(CoreError::invalid(
                    "checkpoint path escapes workspace or replaces its root",
                ));
            }
            let backup = if entry.existed {
                let source = entry
                    .backup
                    .as_deref()
                    .ok_or_else(|| CoreError::Persistence("missing checkpoint backup".into()))?;
                let source = fs::canonicalize(source).map_err(fs_error)?;
                if source == storage || !source.starts_with(&storage) {
                    return Err(CoreError::invalid("checkpoint backup escapes storage"));
                }
                let metadata = fs::symlink_metadata(&source).map_err(fs_error)?;
                if metadata.is_dir() != entry.was_dir || !(metadata.is_dir() || metadata.is_file())
                {
                    return Err(CoreError::invalid(
                        "checkpoint backup type differs from manifest",
                    ));
                }
                Some(source)
            } else {
                if entry.backup.is_some() {
                    return Err(CoreError::invalid(
                        "absent checkpoint entry unexpectedly has a backup",
                    ));
                }
                None
            };
            entries.push(RestoreEntry {
                target,
                reported: lexical_target,
                backup,
            });
        }
    }
    for entry in &entries {
        if storage_roots
            .iter()
            .any(|storage| storage.starts_with(&entry.target) || entry.target.starts_with(storage))
        {
            return Err(CoreError::invalid(
                "checkpoint target overlaps checkpoint storage",
            ));
        }
    }
    Ok(entries)
}

fn absolute(path: &Path) -> Result<PathBuf> {
    Ok(normalize_path(&if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir().map_err(fs_error)?.join(path)
    }))
}

fn resolve_target(path: &Path) -> Result<PathBuf> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() {
                return Err(CoreError::invalid("checkpoint target is a symbolic link"));
            }
            fs::canonicalize(path).map_err(fs_error)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let parent = path
                .parent()
                .ok_or_else(|| CoreError::invalid("checkpoint target has no parent"))?;
            Ok(resolve_target(parent)?.join(
                path.file_name()
                    .ok_or_else(|| CoreError::invalid("checkpoint target has no name"))?,
            ))
        }
        Err(error) => Err(fs_error(error)),
    }
}

fn copy_entry(source: &Path, destination: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(source).map_err(fs_error)?;
    if metadata.file_type().is_symlink() {
        return Err(CoreError::invalid(format!(
            "checkpoint restore does not follow symbolic links: {}",
            source.display()
        )));
    }
    if metadata.is_dir() {
        fs::create_dir(destination).map_err(fs_error)?;
        for entry in fs::read_dir(source).map_err(fs_error)? {
            let entry = entry.map_err(fs_error)?;
            copy_entry(&entry.path(), &destination.join(entry.file_name()))?;
        }
        fs::set_permissions(destination, metadata.permissions()).map_err(fs_error)?;
    } else if metadata.is_file() {
        fs::copy(source, destination).map_err(fs_error)?;
    } else {
        return Err(CoreError::invalid(
            "checkpoint restore requires regular files or directories",
        ));
    }
    Ok(())
}

fn exists(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(fs_error(error)),
    }
}

fn remove_staged(path: &Path) -> Result<()> {
    if exists(path)? {
        if fs::symlink_metadata(path).map_err(fs_error)?.is_dir() {
            fs::remove_dir_all(path).map_err(fs_error)?;
        } else {
            fs::remove_file(path).map_err(fs_error)?;
        }
    }
    Ok(())
}

fn create_parents(target: &Path, created: &mut Vec<PathBuf>) -> Result<()> {
    let parent = target
        .parent()
        .ok_or_else(|| CoreError::invalid("restore target has no parent"))?;
    if !exists(parent)? {
        create_parents(parent, created)?;
        fs::create_dir(parent).map_err(fs_error)?;
        created.push(parent.to_owned());
    }
    Ok(())
}

fn fs_error(error: io::Error) -> CoreError {
    CoreError::other(format!("checkpoint restore filesystem error: {error}"))
}

#[cfg(test)]
mod tests {
    use super::super::CheckpointEntry;
    use super::*;

    struct Fixture {
        workspace: tempfile::TempDir,
        storage: tempfile::TempDir,
    }

    impl Fixture {
        fn new() -> Self {
            Self {
                workspace: tempfile::tempdir().unwrap(),
                storage: tempfile::tempdir().unwrap(),
            }
        }

        fn path(&self, name: &str) -> PathBuf {
            self.workspace.path().join(name)
        }

        fn snapshot(&self, names: &[&str]) -> CheckpointRecord {
            let id = deepagent_core::id::EventId::new().to_string();
            let storage_dir = self.storage.path().join(&id);
            fs::create_dir(&storage_dir).unwrap();
            let entries = names
                .iter()
                .enumerate()
                .map(|(index, name)| {
                    let path = self.path(name);
                    let existed = exists(&path).unwrap();
                    let was_dir = path.is_dir();
                    let backup = if existed {
                        let backup = storage_dir.join(index.to_string());
                        copy_entry(&path, &backup).unwrap();
                        Some(backup.to_string_lossy().to_string())
                    } else {
                        None
                    };
                    CheckpointEntry {
                        path: path.to_string_lossy().to_string(),
                        existed,
                        was_dir,
                        backup,
                    }
                })
                .collect();
            CheckpointRecord {
                id,
                run_id: "fixture".into(),
                session_sequence: 1,
                workspace_root: self.workspace.path().to_string_lossy().to_string(),
                manifest: serde_json::to_string(&CheckpointManifest {
                    storage_dir: storage_dir.to_string_lossy().to_string(),
                    entries,
                    external_evidence: vec![],
                })
                .unwrap(),
                created_at: 1,
            }
        }

        fn assert_no_stages(&self) {
            assert!(fs::read_dir(self.workspace.path())
                .unwrap()
                .all(|entry| !entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".deepagent-restore-")));
        }
    }

    #[test]
    fn missing_backup_in_older_checkpoint_preserves_all_current_files() {
        let fixture = Fixture::new();
        fs::write(fixture.path("first"), "first-before").unwrap();
        let older = fixture.snapshot(&["first"]);
        fs::write(fixture.path("second"), "second-before").unwrap();
        let newer = fixture.snapshot(&["second"]);
        fs::write(fixture.path("first"), "first-current").unwrap();
        fs::write(fixture.path("second"), "second-current").unwrap();
        let manifest: CheckpointManifest = serde_json::from_str(&older.manifest).unwrap();
        fs::remove_file(manifest.entries[0].backup.as_ref().unwrap()).unwrap();
        assert!(RestoreTransaction::run(&[newer, older], |_| -> Result<()> {
            panic!("cannot commit incomplete plan")
        })
        .is_err());
        assert_eq!(
            fs::read_to_string(fixture.path("first")).unwrap(),
            "first-current"
        );
        assert_eq!(
            fs::read_to_string(fixture.path("second")).unwrap(),
            "second-current"
        );
        fixture.assert_no_stages();
    }

    #[test]
    fn malformed_late_entry_is_rejected_before_any_publication() {
        for invalid in [
            "missing-field",
            "outside-backup",
            "outside-target",
            "root-target",
            "wrong-type",
        ] {
            let fixture = Fixture::new();
            fs::write(fixture.path("first"), "first-before").unwrap();
            fs::write(fixture.path("second"), "second-before").unwrap();
            let mut record = fixture.snapshot(&["first", "second"]);
            let mut manifest: CheckpointManifest = serde_json::from_str(&record.manifest).unwrap();
            match invalid {
                "missing-field" => manifest.entries[0].backup = None,
                "outside-backup" => {
                    manifest.entries[0].backup =
                        Some(fixture.path("first").to_string_lossy().to_string())
                }
                "outside-target" => {
                    manifest.entries[0].path = fixture
                        .storage
                        .path()
                        .join("escape")
                        .to_string_lossy()
                        .to_string()
                }
                "root-target" => {
                    manifest.entries[0].path =
                        fixture.workspace.path().to_string_lossy().to_string()
                }
                "wrong-type" => manifest.entries[0].was_dir = true,
                _ => unreachable!(),
            }
            record.manifest = serde_json::to_string(&manifest).unwrap();
            fs::write(fixture.path("first"), "first-current").unwrap();
            fs::write(fixture.path("second"), "second-current").unwrap();
            assert!(
                RestoreTransaction::run(&[record], |_| Ok(())).is_err(),
                "{invalid}"
            );
            assert_eq!(
                fs::read_to_string(fixture.path("first")).unwrap(),
                "first-current"
            );
            assert_eq!(
                fs::read_to_string(fixture.path("second")).unwrap(),
                "second-current"
            );
            fixture.assert_no_stages();
        }
    }

    #[test]
    fn failed_history_commit_compensates_modified_deleted_and_created_targets() {
        let fixture = Fixture::new();
        fs::write(fixture.path("modified"), "before").unwrap();
        fs::write(fixture.path("deleted"), "deleted-before").unwrap();
        let record = fixture.snapshot(&["modified", "deleted", "created"]);
        fs::write(fixture.path("modified"), "current").unwrap();
        fs::remove_file(fixture.path("deleted")).unwrap();
        fs::write(fixture.path("created"), "created-current").unwrap();
        let result = RestoreTransaction::run(&[record], |_| -> Result<()> {
            assert_eq!(
                fs::read_to_string(fixture.path("modified")).unwrap(),
                "before"
            );
            assert_eq!(
                fs::read_to_string(fixture.path("deleted")).unwrap(),
                "deleted-before"
            );
            assert!(!fixture.path("created").exists());
            Err(CoreError::Persistence("injected pre-commit failure".into()))
        });
        assert!(
            matches!(result, Err(CoreError::Persistence(message)) if message == "injected pre-commit failure")
        );
        assert_eq!(
            fs::read_to_string(fixture.path("modified")).unwrap(),
            "current"
        );
        assert!(!fixture.path("deleted").exists());
        assert_eq!(
            fs::read_to_string(fixture.path("created")).unwrap(),
            "created-current"
        );
        fixture.assert_no_stages();
    }

    #[test]
    fn nested_snapshots_and_multiple_checkpoints_keep_existing_restore_order() {
        let fixture = Fixture::new();
        fs::create_dir(fixture.path("tree")).unwrap();
        fs::write(fixture.path("tree/child"), "oldest").unwrap();
        let older = fixture.snapshot(&["tree"]);
        fs::write(fixture.path("tree/child"), "middle").unwrap();
        let newer = fixture.snapshot(&["tree/child", "tree"]);
        fs::write(fixture.path("tree/child"), "current").unwrap();
        RestoreTransaction::run(&[newer, older], |paths| {
            assert_eq!(paths.len(), 3);
            Ok(())
        })
        .unwrap();
        assert_eq!(
            fs::read_to_string(fixture.path("tree/child")).unwrap(),
            "oldest"
        );
        fixture.assert_no_stages();
    }

    #[test]
    fn failed_rollback_retains_current_copy_and_reports_its_location() {
        let fixture = Fixture::new();
        fs::write(fixture.path("file"), "before").unwrap();
        let record = fixture.snapshot(&["file"]);
        fs::write(fixture.path("file"), "current").unwrap();
        let result = RestoreTransaction::run(&[record], |_| -> Result<()> {
            // Deterministically obstruct the no-replace rollback destination
            // inside the fixture's own stage, without deleting either version.
            let area = fs::read_dir(fixture.workspace.path())
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .find(|path| {
                    path.file_name()
                        .unwrap()
                        .to_string_lossy()
                        .starts_with(".deepagent-restore-")
                })
                .unwrap();
            fs::write(area.join("desired"), "rollback obstruction").unwrap();
            Err(CoreError::Persistence("injected finalization error".into()))
        });
        let error = result.unwrap_err().to_string();
        assert!(error.contains("compensation incomplete"));
        assert!(error.contains("recovery directories retained"));
        assert_eq!(fs::read_to_string(fixture.path("file")).unwrap(), "before");
        let area = fs::read_dir(fixture.workspace.path())
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|path| path.is_dir())
            .unwrap();
        assert_eq!(
            fs::read_to_string(area.join("previous")).unwrap(),
            "current"
        );
    }

    #[test]
    fn panic_never_drops_the_only_pre_restore_copy() {
        let fixture = Fixture::new();
        fs::write(fixture.path("file"), "before").unwrap();
        let record = fixture.snapshot(&["file"]);
        fs::write(fixture.path("file"), "current").unwrap();
        assert!(std::panic::catch_unwind(|| RestoreTransaction::run(
            &[record],
            |_| -> Result<()> { panic!("injected commit panic") }
        ))
        .is_err());
        let area = fs::read_dir(fixture.workspace.path())
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|path| path.is_dir())
            .unwrap();
        assert_eq!(
            fs::read_to_string(area.join("previous")).unwrap(),
            "current"
        );
        assert_eq!(fs::read_to_string(fixture.path("file")).unwrap(), "before");
    }

    #[cfg(windows)]
    #[test]
    fn native_publication_failure_compensates_earlier_roots() {
        use std::os::windows::fs::OpenOptionsExt;
        let fixture = Fixture::new();
        fs::write(fixture.path("locked"), "locked-before").unwrap();
        fs::write(fixture.path("healthy"), "healthy-before").unwrap();
        let record = fixture.snapshot(&["locked", "healthy"]);
        fs::write(fixture.path("locked"), "locked-current").unwrap();
        fs::write(fixture.path("healthy"), "healthy-current").unwrap();
        // share_mode(0) is an exclusive handle; the real OS refuses rename.
        let handle = fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(fixture.path("locked"))
            .unwrap();
        assert!(RestoreTransaction::run(&[record], |_| -> Result<()> {
            panic!("publication must fail")
        })
        .is_err());
        drop(handle);
        assert_eq!(
            fs::read_to_string(fixture.path("locked")).unwrap(),
            "locked-current"
        );
        assert_eq!(
            fs::read_to_string(fixture.path("healthy")).unwrap(),
            "healthy-current"
        );
        fixture.assert_no_stages();
    }

    #[cfg(windows)]
    #[test]
    fn failed_directory_copy_never_publishes_partial_replacement() {
        use std::os::windows::fs::OpenOptionsExt;
        let fixture = Fixture::new();
        fs::create_dir(fixture.path("tree")).unwrap();
        fs::write(fixture.path("tree/child"), "before").unwrap();
        fs::write(fixture.path("healthy"), "healthy-before").unwrap();
        let record = fixture.snapshot(&["tree", "healthy"]);
        let manifest: CheckpointManifest = serde_json::from_str(&record.manifest).unwrap();
        let backup = Path::new(manifest.entries[0].backup.as_ref().unwrap()).join("child");
        fs::write(fixture.path("tree/child"), "current").unwrap();
        fs::write(fixture.path("healthy"), "healthy-current").unwrap();
        let handle = fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(backup)
            .unwrap();
        assert!(RestoreTransaction::run(&[record], |_| -> Result<()> {
            panic!("incomplete replacement cannot commit")
        })
        .is_err());
        drop(handle);
        assert_eq!(
            fs::read_to_string(fixture.path("tree/child")).unwrap(),
            "current"
        );
        assert_eq!(
            fs::read_to_string(fixture.path("healthy")).unwrap(),
            "healthy-current"
        );
        fixture.assert_no_stages();
    }
}
