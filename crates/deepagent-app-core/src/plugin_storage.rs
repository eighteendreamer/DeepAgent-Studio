//! Managed plugin storage shared by the desktop and CLI hosts.

use std::collections::BTreeMap;
use std::fs::{self, Metadata};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use deepagent_core::error::{CoreError, Result};

use crate::atomic_file::remove_dir_all_with_retry;
use crate::data_root::UserPaths;
use crate::install_ledger::LedgerStore;
use crate::migrate::identical_files;
use crate::plugin_service::{copy_dir, create_plugin_staging_dir, replace_plugin_dir};

/// Installer resources are only a seed. Both host types always load plugin
/// files from the user root, including a headless host with no bundled source.
/// Reconcile from the package on startup without rewriting identical files or
/// retaining components removed by a newer package.
pub fn prepare_builtin_plugins(paths: &UserPaths, source: Option<&Path>) -> Result<PathBuf> {
    let destination = &paths.builtin_plugins;
    let state = &paths.plugin_state_root;
    check_directory(state)?;
    check_directory(destination)?;
    let Some(source) = source else {
        return Ok(destination.clone());
    };
    if !check_directory(source)? {
        return Ok(destination.clone());
    }
    if fs::canonicalize(state).is_ok_and(|state| {
        fs::canonicalize(source)
            .is_ok_and(|source| source.starts_with(&state) || state.starts_with(&source))
    }) {
        return Err(CoreError::invalid(
            "bundled and managed plugin storage overlap",
        ));
    }

    LedgerStore::open(paths).with_locked(|_| {
        let expected = tree(source)?;
        if check_directory(destination)? && same_tree(source, destination, &expected)? {
            return Ok(destination.clone());
        }
        check_directory(&state.join(".staging"))?;
        let stage = create_plugin_staging_dir(state, "builtin")?;
        let result = copy_dir(source, &stage)
            .and_then(|()| replace_plugin_dir(&stage, destination, state));
        if result.is_err() {
            if let Err(error) = remove_dir_all_with_retry(&stage) {
                tracing::warn!(path = %stage.display(), %error, "could not clean plugin staging directory");
            }
        }
        result?;
        tracing::info!(path = %destination.display(), "prepared managed built-in plugins");
        Ok(destination.clone())
    })
}

fn same_tree(
    source: &Path,
    destination: &Path,
    expected: &BTreeMap<PathBuf, bool>,
) -> Result<bool> {
    if &tree(destination)? != expected {
        return Ok(false);
    }
    for (relative, is_dir) in expected {
        if !is_dir && !identical_files(&source.join(relative), &destination.join(relative))? {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Symlinks/junctions are never followed by installation or relocation. Reject
/// a malformed tree before a staging swap or any move changes it.
pub(crate) fn check_directory(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() && !is_link(&meta) => Ok(true),
        Ok(_) => Err(invalid_entry(path)),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
        Err(error) => Err(io_error(path, error)),
    }
}

pub(crate) fn tree(root: &Path) -> Result<BTreeMap<PathBuf, bool>> {
    fn visit(root: &Path, dir: &Path, output: &mut BTreeMap<PathBuf, bool>) -> Result<()> {
        for entry in fs::read_dir(dir).map_err(|error| io_error(dir, error))? {
            let path = entry.map_err(|error| io_error(dir, error))?.path();
            let meta = fs::symlink_metadata(&path).map_err(|error| io_error(&path, error))?;
            if is_link(&meta) || !(meta.is_dir() || meta.is_file()) {
                return Err(invalid_entry(&path));
            }
            output.insert(
                path.strip_prefix(root).expect("child of tree").into(),
                meta.is_dir(),
            );
            if meta.is_dir() {
                visit(root, &path, output)?;
            }
        }
        Ok(())
    }
    let mut entries = BTreeMap::new();
    if check_directory(root)? {
        visit(root, root, &mut entries)?;
    }
    Ok(entries)
}

fn is_link(meta: &Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;
        if meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return true;
        }
    }
    meta.is_symlink()
}

fn invalid_entry(path: &Path) -> CoreError {
    CoreError::invalid(format!(
        "unexpected plugin storage entry (linked or wrong file type): {}",
        path.display()
    ))
}

fn io_error(path: &Path, error: std::io::Error) -> CoreError {
    CoreError::Persistence(format!("read plugin storage {}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin_loader::PluginRoots;
    use crate::plugin_service::PluginService;

    #[test]
    fn managed_plugin_storage_supplies_detail_paths_and_refreshes_packaged_files() {
        let tmp = tempfile::tempdir().unwrap();
        let bundle = tmp.path().join("package/plugins");
        let packaged = bundle.join("demo");
        fs::create_dir_all(packaged.join(".codex-plugin")).unwrap();
        fs::create_dir_all(packaged.join("commands")).unwrap();
        let manifest = packaged.join(".codex-plugin/plugin.json");
        fs::write(
            &manifest,
            r#"{"name":"demo","version":"1.0.0","commands":"./commands"}"#,
        )
        .unwrap();
        fs::write(packaged.join("commands/old.md"), "old contents").unwrap();
        let paths = UserPaths::under(tmp.path().join("home/.deepagent"));
        paths.ensure_dirs().unwrap();
        let builtin = prepare_builtin_plugins(&paths, Some(&bundle)).unwrap();
        assert_eq!(builtin, paths.plugin_state_root.join("builtin"));
        let service = PluginService::new(
            PluginRoots {
                session: vec![],
                builtin: builtin.clone(),
                workspace: None,
                personal: paths.plugins.clone(),
            },
            &paths.root,
        );
        let detail = service.read("demo@builtin").unwrap().unwrap();
        let detail_path = |value: &str| PathBuf::from(value);
        assert_eq!(
            detail_path(detail.path.as_deref().unwrap()),
            builtin.join("demo")
        );
        assert!(detail_path(detail.manifest_path.as_deref().unwrap()).starts_with(&builtin));
        assert!(!detail.entrypoints.is_empty());
        assert!(detail
            .entrypoints
            .iter()
            .all(|path| detail_path(path).starts_with(&builtin)));
        assert_eq!(
            detail_path(&detail.data_dir),
            paths.plugin_data.join("demo-builtin")
        );
        let copied_manifest = builtin.join("demo/.codex-plugin/plugin.json");
        let original = fs::metadata(&copied_manifest).unwrap().modified().unwrap();
        prepare_builtin_plugins(&paths, Some(&bundle)).unwrap();
        assert_eq!(
            fs::metadata(&copied_manifest).unwrap().modified().unwrap(),
            original
        );

        fs::remove_file(packaged.join("commands/old.md")).unwrap();
        fs::write(packaged.join("commands/new.md"), "new contents").unwrap();
        prepare_builtin_plugins(&paths, Some(&bundle)).unwrap();
        assert!(!builtin.join("demo/commands/old.md").exists());
        assert_eq!(
            fs::read_to_string(builtin.join("demo/commands/new.md")).unwrap(),
            "new contents"
        );
        assert!(manifest.is_file());
        assert_eq!(
            prepare_builtin_plugins(&paths, Some(&tmp.path().join("missing"))).unwrap(),
            builtin
        );
        assert!(copied_manifest.is_file());
    }
}
