//! Windows DACL metadata for the existing checkpoint copy/restore transaction.
//!
//! Build every empty entry and acquire its handles first, protect all policies,
//! then copy bytes. This also supports a readable but non-writable source tree:
//! applying a directory's policy must not prevent constructing its children.

use std::path::PathBuf;

use deepagent_persistence::fs_commit::access::WindowsDacl;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct AccessTree {
    version: u32,
    entries: Vec<AccessEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AccessEntry {
    relative: PathBuf,
    directory: bool,
    policy: WindowsDacl,
}

#[cfg(windows)]
mod native {
    use super::*;
    use std::fs::{self, File, Permissions};
    use std::io;
    use std::path::Path;

    use deepagent_core::{CoreError, Result};
    use deepagent_persistence::fs_commit::access::{
        create_snapshot_target, open_policy_source, open_snapshot_source,
    };

    struct Source {
        relative: PathBuf,
        file: File,
        directory: bool,
        permissions: Permissions,
    }

    pub(crate) struct AccessLease {
        pub(crate) staged: PathBuf,
        file: Option<File>,
        policy: WindowsDacl,
    }

    impl AccessLease {
        /// Apply through a held handle, or verify a descendant already closed
        /// for directory publication. The root is only unprotected below its
        /// final parent; descendants are prepared below their original tree.
        pub(crate) fn publish_policy(&self, published: &Path) -> Result<()> {
            if let Some(file) = &self.file {
                self.policy
                    .publish(file, || publication_parent(published))
                    .map_err(fs_error)?;
            }
            let file = open_policy_source(published).map_err(fs_error)?;
            if !self
                .policy
                .verify_publication(&file, || publication_parent(published))
                .map_err(fs_error)?
            {
                return Err(CoreError::other(
                    "checkpoint DACL differs after publication; refusing history commit",
                ));
            }
            Ok(())
        }
    }

    fn publication_parent(published: &Path) -> io::Result<File> {
        open_policy_source(
            published
                .parent()
                .ok_or_else(|| io::Error::other("published entry has no parent"))?,
        )
    }

    /// Windows refuses a directory move with live descendant file objects,
    /// including metadata-only ones. Restore descendant policies while their
    /// *relative* parents already carry original DACLs, verify them, then close
    /// them before the single root rename. Keep the root's protected policy and
    /// handle until it is below the real final parent. Root unprotection can
    /// propagate inherited ACEs, so publish verifies the whole tree afterwards.
    pub(crate) fn prepare_publication(leases: &mut [AccessLease], root: &Path) -> Result<()> {
        leases.sort_by_key(|lease| lease.staged.components().count());
        for lease in leases.iter() {
            if lease.staged != root {
                lease.publish_policy(&lease.staged)?;
            }
        }
        for lease in leases {
            if lease.staged != root {
                lease.file.take();
            }
        }
        Ok(())
    }

    fn entry_path(root: &Path, relative: &Path) -> PathBuf {
        // Joining an empty root-relative name appends a separator on Windows,
        // which changes a regular-file root into an invalid directory path.
        if relative.as_os_str().is_empty() {
            root.to_owned()
        } else {
            root.join(relative)
        }
    }

    fn read_sources(root: &Path) -> Result<Vec<Source>> {
        fn visit(root: &Path, relative: PathBuf, entries: &mut Vec<Source>) -> Result<()> {
            let path = entry_path(root, &relative);
            let file = open_snapshot_source(&path).map_err(|error| {
                CoreError::other(format!("checkpoint snapshot source open failed: {error}"))
            })?;
            let metadata = file.metadata().map_err(fs_error)?;
            let directory = metadata.is_dir();
            entries.push(Source {
                relative: relative.clone(),
                file,
                directory,
                permissions: metadata.permissions(),
            });
            if directory {
                let mut children = fs::read_dir(&path)
                    .map_err(fs_error)?
                    .map(|entry| entry.map(|entry| entry.file_name()))
                    .collect::<io::Result<Vec<_>>>()
                    .map_err(fs_error)?;
                children.sort();
                for name in children {
                    visit(root, relative.join(name), entries)?;
                }
            }
            Ok(())
        }
        let mut entries = Vec::new();
        visit(root, PathBuf::new(), &mut entries)?;
        Ok(entries)
    }

    fn validate_sources(tree: &AccessTree, sources: &[Source]) -> Result<()> {
        if tree.version != 1 || tree.entries.len() != sources.len() {
            return Err(CoreError::invalid(
                "invalid checkpoint DACL tree version or shape",
            ));
        }
        // Comparing against the freshly walked backup rejects absolute paths,
        // traversal, duplicates, omissions, wrong types and a replaced root.
        for (entry, source) in tree.entries.iter().zip(sources) {
            if entry.relative != source.relative || entry.directory != source.directory {
                return Err(CoreError::invalid(
                    "checkpoint DACL tree differs from backup",
                ));
            }
            entry.policy.validate().map_err(fs_error)?;
        }
        Ok(())
    }

    struct CopyPlan {
        sources: Vec<Source>,
        leases: Vec<AccessLease>,
    }

    impl CopyPlan {
        fn prepare(sources: Vec<Source>, destination: &Path, tree: &AccessTree) -> Result<Self> {
            validate_sources(tree, &sources)?;
            let mut leases = Vec::new();
            for entry in &tree.entries {
                let staged = entry_path(destination, &entry.relative);
                let file = create_snapshot_target(&staged, entry.directory).map_err(|error| {
                    CoreError::other(format!(
                        "checkpoint empty entry creation failed (directory={}): {error}",
                        entry.directory
                    ))
                })?;
                leases.push(AccessLease {
                    staged,
                    file: Some(file),
                    policy: entry.policy.clone(),
                });
            }
            // No content exists yet. Protect parents and children before any
            // bytes are copied; held handles retain the creator's write rights.
            for lease in &leases {
                lease
                    .policy
                    .apply(lease.file.as_ref().expect("new staging handle"), true)
                    .map_err(|error| {
                        CoreError::other(format!(
                            "checkpoint staging DACL application failed: {error}"
                        ))
                    })?;
            }
            Ok(Self { sources, leases })
        }

        fn copy(mut self) -> Result<Vec<AccessLease>> {
            for (source, lease) in self.sources.iter_mut().zip(&mut self.leases) {
                let writer = lease.file.as_mut().expect("new staging handle");
                if !source.directory {
                    io::copy(&mut source.file, &mut *writer).map_err(|error| {
                        CoreError::other(format!("checkpoint content copy failed: {error}"))
                    })?;
                }
                writer
                    .set_permissions(source.permissions.clone())
                    .map_err(|error| {
                        CoreError::other(format!(
                            "checkpoint attribute copy failed (directory={}): {error}",
                            source.directory
                        ))
                    })?;
            }
            Ok(self.leases)
        }
    }

    pub(crate) fn capture_copy(source: &Path, destination: &Path) -> Result<AccessTree> {
        let sources = read_sources(source)?;
        let entries = sources
            .iter()
            .map(|source| {
                Ok(AccessEntry {
                    relative: source.relative.clone(),
                    directory: source.directory,
                    policy: WindowsDacl::capture(&source.file).map_err(fs_error)?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let tree = AccessTree {
            version: 1,
            entries,
        };
        CopyPlan::prepare(sources, destination, &tree)?.copy()?;
        Ok(tree)
    }

    pub(crate) fn validate_backup(source: &Path, tree: &AccessTree) -> Result<()> {
        validate_sources(tree, &read_sources(source)?)
    }

    pub(crate) fn prepare_copy(
        source: &Path,
        destination: &Path,
        tree: &AccessTree,
    ) -> Result<Vec<AccessLease>> {
        CopyPlan::prepare(read_sources(source)?, destination, tree)?.copy()
    }

    fn fs_error(error: io::Error) -> CoreError {
        CoreError::other(format!("checkpoint DACL copy error: {error}"))
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use deepagent_persistence::fs_commit::access::open_policy_update;
        use std::io::{self, Write};

        #[derive(Default)]
        struct AclRecovery(Vec<(PathBuf, WindowsDacl)>);

        impl AclRecovery {
            fn track(&mut self, path: PathBuf, policy: WindowsDacl) {
                self.0.push((path, policy));
            }

            fn restore(&mut self) -> io::Result<()> {
                self.0.sort_by_key(|(path, _)| path.components().count());
                let mut first_error = None;
                for (path, policy) in &self.0 {
                    let result = open_policy_update(path).and_then(|file| {
                        policy.apply(&file, false)?;
                        if WindowsDacl::capture(&file)? != *policy {
                            return Err(io::Error::other("restored test DACL differs"));
                        }
                        Ok(())
                    });
                    if first_error.is_none() {
                        first_error = result.err();
                    }
                }
                if let Some(error) = first_error {
                    return Err(error);
                }
                self.0.clear();
                Ok(())
            }
        }

        impl Drop for AclRecovery {
            fn drop(&mut self) {
                let _ = self.restore();
            }
        }

        fn policy(sddl: &str) -> WindowsDacl {
            serde_json::from_value(serde_json::json!({"version":1,"protected":true,"sddl":sddl}))
                .unwrap()
        }

        #[test]
        fn restrictive_tree_is_protected_before_copying_any_bytes() {
            let temporary = tempfile::tempdir().unwrap();
            let cleanup_path = temporary.path().to_owned();
            let source = temporary.path().join("source");
            let destination = temporary.path().join("destination");
            let directory = create_snapshot_target(&source, true).unwrap();
            let mut file = create_snapshot_target(&source.join("child"), false).unwrap();
            file.write_all(b"synthetic checkpoint body").unwrap();
            let original_directory = WindowsDacl::capture(&directory).unwrap();
            let original_file = WindowsDacl::capture(&file).unwrap();
            let original_policies = [original_directory.clone(), original_file.clone()];
            let mut recovery = AclRecovery::default();
            recovery.track(source.clone(), original_directory);
            recovery.track(source.join("child"), original_file);
            // Deny content/add-child writes, not GENERIC_WRITE: the latter
            // also denies READ_CONTROL/SYNCHRONIZE and would forbid reads.
            let restricted = policy("D:P(D;;0x6;;;WD)(A;;GR;;;WD)");
            restricted.apply(&directory, false).unwrap();
            restricted.apply(&file, false).unwrap();
            drop((directory, file));

            let sources = read_sources(&source).unwrap();
            let tree = AccessTree {
                version: 1,
                entries: sources
                    .iter()
                    .map(|source| AccessEntry {
                        relative: source.relative.clone(),
                        directory: source.directory,
                        policy: WindowsDacl::capture(&source.file).unwrap(),
                    })
                    .collect(),
            };
            assert_eq!(tree.entries.len(), original_policies.len());
            for (entry, original) in tree.entries.iter().zip(&original_policies) {
                recovery.track(entry_path(&destination, &entry.relative), original.clone());
            }
            let plan = CopyPlan::prepare(sources, &destination, &tree).unwrap();
            assert_eq!(plan.leases.len(), tree.entries.len());
            assert_eq!(fs::metadata(destination.join("child")).unwrap().len(), 0);
            for (lease, entry) in plan.leases.iter().zip(&tree.entries) {
                assert!(
                    WindowsDacl::capture(lease.file.as_ref().unwrap()).unwrap() == entry.policy,
                    "staging DACL differs before content copy"
                );
            }
            let leases = plan.copy().unwrap();
            assert_eq!(
                fs::read(destination.join("child")).unwrap(),
                b"synthetic checkpoint body"
            );
            for lease in leases {
                lease.publish_policy(&lease.staged).unwrap();
            }
            recovery.restore().unwrap();
            drop(temporary);
            assert!(
                !cleanup_path.exists(),
                "restricted ACL test left its temporary directory behind"
            );
        }

        #[test]
        fn malformed_policy_is_rejected_before_allocating_copy_targets() {
            let temporary = tempfile::tempdir().unwrap();
            let source = temporary.path().join("source");
            let backup = temporary.path().join("backup");
            fs::write(&source, b"synthetic").unwrap();
            let mut tree = capture_copy(&source, &backup).unwrap();
            tree.version = 2;
            let destination = temporary.path().join("desired");
            assert!(prepare_copy(&backup, &destination, &tree).is_err());
            assert!(!destination.exists());
        }

        #[test]
        fn staged_tree_moves_only_after_releasing_descendant_handles() {
            use deepagent_persistence::fs_commit::rename_no_replace;
            for prepare in [false, true] {
                let temporary = tempfile::tempdir().unwrap();
                let source = temporary.path().join("source");
                fs::create_dir(&source).unwrap();
                fs::write(source.join("child"), "synthetic").unwrap();
                fs::create_dir(source.join("nested")).unwrap();
                fs::write(source.join("nested/child"), "synthetic nested").unwrap();
                let backup = temporary.path().join("backup");
                let tree = capture_copy(&source, &backup).unwrap();
                let desired = temporary.path().join("desired");
                let mut leases = prepare_copy(&backup, &desired, &tree).unwrap();
                if prepare {
                    prepare_publication(&mut leases, &desired).unwrap();
                }
                let published = temporary.path().join("published");
                let result = rename_no_replace(&desired, &published);
                if prepare {
                    result.unwrap();
                    for lease in &leases {
                        lease
                            .publish_policy(&entry_path(
                                &published,
                                lease.staged.strip_prefix(&desired).unwrap(),
                            ))
                            .unwrap();
                    }
                } else {
                    assert!(
                        result.is_err(),
                        "native directory move must reject live descendant handles"
                    );
                }
            }
        }
    }
}

#[cfg(windows)]
pub(super) use native::{
    capture_copy, prepare_copy, prepare_publication, validate_backup, AccessLease,
};
