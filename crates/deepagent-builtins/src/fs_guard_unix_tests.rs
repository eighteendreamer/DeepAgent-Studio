//! Unix compatibility for the shared H-78 policy resolver. These tests do not
//! claim operation-time rename protection for the portable string I/O backend.

use super::{FsAccess, WorkspaceRoot};
use crate::file_tools::{GlobTool, GrepTool, ReadFileTool, WriteFileTool};
use deepagent_tools::Tool;
use std::fs;
use std::os::unix::fs::symlink;

#[tokio::test]
async fn unix_static_symlink_targets_and_discovery_obey_access_modes() {
    let workspace = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let real = workspace.path().join("real");
    fs::create_dir(&real).unwrap();
    fs::write(real.join("inside.txt"), "inside fixture").unwrap();
    fs::write(outside.path().join("outside.txt"), "outside fixture").unwrap();
    symlink("real", workspace.path().join("inward")).unwrap();
    symlink(outside.path(), workspace.path().join("outward")).unwrap();
    symlink(workspace.path(), real.join("cycle")).unwrap();
    symlink(&real, outside.path().join("back")).unwrap();
    symlink(outside.path().join("back"), workspace.path().join("hidden")).unwrap();
    symlink(&real, workspace.path().join("secret-store")).unwrap();
    symlink("secret-store", workspace.path().join("alias")).unwrap();

    for access in [
        FsAccess::ReadOnly,
        FsAccess::Workspace,
        FsAccess::ReadAnywhere,
        FsAccess::Full,
    ] {
        let root = WorkspaceRoot::new(workspace.path()).with_access(access);
        let reader = ReadFileTool::new(root.clone());
        let inward = reader
            .invoke(serde_json::json!({"path":"inward/inside.txt"}))
            .await
            .unwrap();
        assert!(inward.ok, "{access:?}: {inward:?}");
        let reads_outside = matches!(access, FsAccess::ReadAnywhere | FsAccess::Full);
        let outward = reader
            .invoke(serde_json::json!({"path":"outward/outside.txt"}))
            .await
            .unwrap();
        assert_eq!(outward.ok, reads_outside, "{access:?}: {outward:?}");
        let hidden = reader
            .invoke(serde_json::json!({"path":"hidden/inside.txt"}))
            .await
            .unwrap();
        assert_eq!(hidden.ok, reads_outside, "{access:?}: {hidden:?}");
        let sensitive = reader
            .invoke(serde_json::json!({"path":"alias/inside.txt"}))
            .await
            .unwrap();
        assert!(
            !sensitive.ok,
            "intermediate sensitive alias: {access:?}: {sensitive:?}"
        );
        let glob = GlobTool::new(root.clone())
            .invoke(serde_json::json!({"pattern":"**/*.txt"}))
            .await
            .unwrap();
        assert!(glob.ok, "{access:?}: {glob:?}");
        assert_eq!(
            glob.value.to_string().contains("outward/outside.txt"),
            reads_outside
        );
        assert!(
            glob.value["matches"].as_array().unwrap().len() < 20,
            "cycle was not bounded: {glob:?}"
        );
        let grep = GrepTool::new(root)
            .invoke(serde_json::json!({"query":"outside fixture"}))
            .await
            .unwrap();
        assert!(grep.ok, "{access:?}: {grep:?}");
        assert_eq!(
            !grep.value["hits"].as_array().unwrap().is_empty(),
            reads_outside
        );
    }
}

#[tokio::test]
async fn unix_new_parent_and_final_file_symlink_preserve_entry_semantics() {
    let workspace = tempfile::tempdir().unwrap();
    let alias_home = tempfile::tempdir().unwrap();
    let logical = alias_home.path().join("workspace");
    symlink(workspace.path(), &logical).unwrap();
    let root = WorkspaceRoot::new(&logical);
    let target = workspace.path().join("target.txt");
    let alias = workspace.path().join("alias.txt");
    fs::write(&target, "original fixture").unwrap();
    symlink("target.txt", &alias).unwrap();
    let cache = std::sync::Arc::new(std::sync::Mutex::new(
        crate::file_cache::FileStateCache::new(),
    ));
    let read = ReadFileTool::with_cache(root.clone(), cache.clone())
        .invoke(serde_json::json!({"path":"alias.txt"}))
        .await
        .unwrap();
    assert!(read.ok, "{read:?}");
    let writer = WriteFileTool::with_cache(root, cache);
    let replaced = writer
        .invoke(serde_json::json!({"path":"alias.txt", "content":"replacement fixture"}))
        .await
        .unwrap();
    assert!(replaced.ok, "{replaced:?}");
    assert_eq!(fs::read_to_string(&target).unwrap(), "original fixture");
    assert_eq!(fs::read_to_string(&alias).unwrap(), "replacement fixture");
    assert!(!fs::symlink_metadata(&alias)
        .unwrap()
        .file_type()
        .is_symlink());
    let created = writer
        .invoke(serde_json::json!({"path":"new/深层🦀/created.txt", "content":"new fixture"}))
        .await
        .unwrap();
    assert!(created.ok, "{created:?}");
    assert_eq!(
        fs::read_to_string(workspace.path().join("new/深层🦀/created.txt")).unwrap(),
        "new fixture"
    );
}

#[test]
fn unix_root_snapshot_rejects_same_path_replacement_before_first_read() {
    let home = tempfile::tempdir().unwrap();
    let logical = home.path().join("workspace");
    let parked = home.path().join("parked");
    fs::create_dir(&logical).unwrap();
    fs::write(logical.join("visible.txt"), "original fixture").unwrap();
    let root = WorkspaceRoot::new(&logical);
    let cloned = root.clone();
    fs::rename(&logical, &parked).unwrap();
    fs::create_dir(&logical).unwrap();
    fs::write(logical.join("visible.txt"), "replacement fixture").unwrap();
    assert!(cloned.open_read(&logical.join("visible.txt")).is_err());
    assert!(WorkspaceRoot::new(&logical)
        .open_read(&logical.join("visible.txt"))
        .is_ok());
    assert_eq!(
        fs::read_to_string(parked.join("visible.txt")).unwrap(),
        "original fixture"
    );
}
