//! Real Windows namespace compatibility controls for H-78.

use std::fs;
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;

use super::{FsAccess, WorkspaceRoot};
use crate::file_tools::{
    DeletePathTool, GlobTool, GrepTool, ListDirTool, MovePathTool, ReadFileTool, WriteFileTool,
};
use deepagent_tools::Tool;
use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_NOT_SAME_DEVICE};
use windows_sys::Win32::Storage::FileSystem::{
    DefineDosDeviceW, QueryDosDeviceW, DDD_EXACT_MATCH_ON_REMOVE, DDD_NO_BROADCAST_SYSTEM,
    DDD_RAW_TARGET_PATH, DDD_REMOVE_DEFINITION,
};

struct DriveAlias {
    drive: Vec<u16>,
    target: Vec<u16>,
    active: bool,
}

impl DriveAlias {
    fn new(backing: &Path) -> Self {
        let physical = fs::canonicalize(backing).unwrap();
        assert!(matches!(
            physical.components().next(),
            Some(std::path::Component::Prefix(prefix))
                if matches!(prefix.kind(), std::path::Prefix::VerbatimDisk(_))
        ));
        let target: Vec<u16> = r"\??\"
            .encode_utf16()
            .chain(physical.as_os_str().encode_wide().skip(4))
            .chain(Some(0))
            .collect();
        for letter in (b'H'..=b'Z').rev() {
            let drive = vec![u16::from(letter), u16::from(b':'), 0];
            let mut current = vec![0u16; 32768];
            // SAFETY: Both input and output buffers live throughout the call.
            let found = unsafe {
                QueryDosDeviceW(drive.as_ptr(), current.as_mut_ptr(), current.len() as u32)
            };
            if found != 0 {
                continue;
            }
            assert_eq!(
                io::Error::last_os_error().raw_os_error(),
                Some(ERROR_FILE_NOT_FOUND as i32)
            );
            // SAFETY: NUL-terminated UTF-16 names; the chosen letter was unused.
            let created = unsafe {
                DefineDosDeviceW(
                    DDD_RAW_TARGET_PATH | DDD_NO_BROADCAST_SYSTEM,
                    drive.as_ptr(),
                    target.as_ptr(),
                )
            };
            assert_ne!(created, 0, "{}", io::Error::last_os_error());
            return Self {
                drive,
                target,
                active: true,
            };
        }
        panic!("no unused drive letter for the SUBST compatibility fixture");
    }

    fn path(&self) -> std::path::PathBuf {
        format!("{}:\\", char::from_u32(u32::from(self.drive[0])).unwrap()).into()
    }

    fn remove(&mut self) -> io::Result<()> {
        if !self.active {
            return Ok(());
        }
        // SAFETY: Remove only the exact raw target this fixture installed.
        let removed = unsafe {
            DefineDosDeviceW(
                DDD_RAW_TARGET_PATH
                    | DDD_NO_BROADCAST_SYSTEM
                    | DDD_REMOVE_DEFINITION
                    | DDD_EXACT_MATCH_ON_REMOVE,
                self.drive.as_ptr(),
                self.target.as_ptr(),
            )
        };
        if removed == 0 {
            return Err(io::Error::last_os_error());
        }
        self.active = false;
        Ok(())
    }
}

impl Drop for DriveAlias {
    fn drop(&mut self) {
        if let Err(error) = self.remove() {
            eprintln!("H-78 fixture drive cleanup failed: {error}");
        }
    }
}

async fn public_file_tools_roundtrip(path: &Path, access: FsAccess) {
    fs::write(path.join("visible.txt"), "namespace fixture").unwrap();
    let physical = fs::canonicalize(path).unwrap();
    eprintln!("namespace {access:?}: logical={path:?}, canonical={physical:?}");
    let root = WorkspaceRoot::new(path).with_access(access);
    let read = ReadFileTool::new(root.clone())
        .invoke(serde_json::json!({"path":"visible.txt"}))
        .await
        .unwrap();
    assert!(read.ok, "namespace read: {read:?}");
    let write = WriteFileTool::new(root.clone())
        .invoke(
            serde_json::json!({"path":"新建/深层🦀/created.txt", "content":"namespace fixture"}),
        )
        .await
        .unwrap();
    assert!(write.ok, "namespace write: {write:?}");
    let listed = ListDirTool::new(root.clone())
        .invoke(serde_json::json!({"path":"新建/深层🦀"}))
        .await
        .unwrap();
    assert!(listed.ok, "namespace list: {listed:?}");
    assert!(listed.value.to_string().contains("created.txt"));
    let glob = GlobTool::new(root.clone())
        .invoke(serde_json::json!({"pattern":"**/*.txt"}))
        .await
        .unwrap();
    assert!(glob.ok, "namespace glob: {glob:?}");
    assert!(glob.value.to_string().contains("created.txt"));
    let grep = GrepTool::new(root.clone())
        .invoke(serde_json::json!({"query":"namespace fixture"}))
        .await
        .unwrap();
    assert!(grep.ok, "namespace grep: {grep:?}");
    assert!(grep.value.to_string().contains("created.txt"));
    let moved = MovePathTool::new(root.clone())
        .invoke(serde_json::json!({"source":"新建/深层🦀/created.txt", "destination":"moved.txt"}))
        .await
        .unwrap();
    assert!(moved.ok, "namespace move: {moved:?}");
    assert_eq!(
        fs::read_to_string(path.join("moved.txt")).unwrap(),
        "namespace fixture"
    );
    let deleted = DeletePathTool::new(root)
        .invoke(serde_json::json!({"path":"新建", "recursive":true}))
        .await
        .unwrap();
    assert!(deleted.ok, "namespace recursive delete: {deleted:?}");
    assert!(!path.join("新建").exists());
    fs::remove_file(path.join("visible.txt")).unwrap();
    fs::remove_file(path.join("moved.txt")).unwrap();
    assert_eq!(fs::read_dir(path).unwrap().count(), 0);
}

#[tokio::test]
async fn subst_namespace_public_file_tools_preserve_contract() {
    let backing = tempfile::tempdir().unwrap();
    let mut alias = DriveAlias::new(backing.path());
    assert_eq!(
        fs::canonicalize(alias.path()).unwrap(),
        fs::canonicalize(backing.path()).unwrap()
    );
    for access in [FsAccess::Workspace, FsAccess::Full] {
        public_file_tools_roundtrip(&alias.path(), access).await;
    }
    alias.remove().expect("remove the exact fixture mapping");
    assert!(!alias.path().exists());
}

#[tokio::test]
#[ignore = "requires DEEPAGENT_H78_CROSS_VOLUME_ROOT on a different authorized local test volume"]
async fn cross_volume_public_move_preserves_source_and_previous_target() {
    let parent = std::env::var_os("DEEPAGENT_H78_CROSS_VOLUME_ROOT")
        .expect("set DEEPAGENT_H78_CROSS_VOLUME_ROOT to a directory on another local volume");
    assert!(Path::new(&parent).is_absolute());
    let source_root = tempfile::tempdir().unwrap();
    let target_root = tempfile::Builder::new()
        .prefix("deepagent-h78-cross-volume-")
        .tempdir_in(parent)
        .unwrap();
    // A real OS rename proves this fixture spans volumes. A configuration
    // mistake must fail this test rather than silently test a same-volume move.
    let probe = source_root.path().join("volume-probe");
    let target_probe = target_root.path().join("volume-probe");
    fs::write(&probe, "volume fixture").unwrap();
    let error = fs::rename(&probe, &target_probe).unwrap_err();
    assert_eq!(error.raw_os_error(), Some(ERROR_NOT_SAME_DEVICE as i32));
    assert!(!target_probe.exists());
    fs::remove_file(&probe).unwrap();

    for directory in [false, true] {
        for overwrite in [false, true] {
            let source = source_root.path().join("source");
            let destination = target_root.path().join("target");
            let content_path = |path: &Path| {
                if directory {
                    path.join("child.txt")
                } else {
                    path.to_path_buf()
                }
            };
            if directory {
                fs::create_dir(&source).unwrap();
            }
            fs::write(content_path(&source), "source fixture").unwrap();
            if overwrite {
                if directory {
                    fs::create_dir(&destination).unwrap();
                }
                fs::write(content_path(&destination), "previous fixture").unwrap();
            }
            let output = MovePathTool::new(
                WorkspaceRoot::new(source_root.path()).with_access(FsAccess::Full),
            )
            .invoke(serde_json::json!({
                "source":source, "destination":destination, "overwrite":overwrite,
            }))
            .await
            .unwrap();
            eprintln!("cross-volume directory={directory} overwrite={overwrite}: {output:?}");
            assert!(!output.ok, "native cross-volume move must fail");
            assert!(output.value["error"]
                .as_str()
                .unwrap()
                .starts_with("move failed:"));
            assert_eq!(
                fs::read_to_string(content_path(&source)).unwrap(),
                "source fixture"
            );
            if overwrite {
                assert_eq!(output.value["destination_restored"], true);
                assert_eq!(
                    fs::read_to_string(content_path(&destination)).unwrap(),
                    "previous fixture"
                );
            } else {
                assert!(!destination.exists());
            }
            assert!(output.value.get("recovery_path").is_none());
            assert_eq!(fs::read_dir(source_root.path()).unwrap().count(), 1);
            assert_eq!(
                fs::read_dir(target_root.path()).unwrap().count(),
                usize::from(overwrite)
            );
            fs::remove_file(content_path(&source)).unwrap();
            if directory {
                fs::remove_dir(&source).unwrap();
            }
            if overwrite {
                fs::remove_file(content_path(&destination)).unwrap();
                if directory {
                    fs::remove_dir(&destination).unwrap();
                }
            }
        }
    }
}

#[tokio::test]
#[ignore = "local timing probe; records samples without a machine-dependent performance gate"]
async fn namespace_read_authorization_timing_probe() {
    let backing = tempfile::tempdir().unwrap();
    let relative = "level1/level2/level3/level4/level5/level6/level7/level8/visible.txt";
    let target = backing.path().join(relative);
    fs::create_dir_all(target.parent().unwrap()).unwrap();
    fs::write(&target, "namespace timing fixture\n".repeat(2048)).unwrap();
    // Same public tool and complete bytes in both modes. This comparison
    // isolates current confinement cost; it is not a benchmark of old HEAD.
    for access in [FsAccess::Full, FsAccess::Workspace] {
        let reader = ReadFileTool::new(WorkspaceRoot::new(backing.path()).with_access(access));
        let mut samples = Vec::new();
        for index in 0..45 {
            let start = std::time::Instant::now();
            let read = reader
                .invoke(serde_json::json!({"path":relative}))
                .await
                .unwrap();
            assert!(read.ok, "timing probe read failed: {read:?}");
            if index >= 5 {
                samples.push(start.elapsed());
            }
        }
        samples.sort_unstable();
        eprintln!(
            "H78_READ_TIMING access={access:?} path_depth=8 bytes={} warmup=5 samples={} median_us={} p95_us={} max_us={}",
            fs::metadata(&target).unwrap().len(),
            samples.len(),
            samples[samples.len() / 2].as_micros(),
            samples[samples.len() * 95 / 100].as_micros(),
            samples.last().unwrap().as_micros(),
        );
    }
}

#[tokio::test]
#[ignore = "requires DEEPAGENT_H78_UNC_ROOT naming an authorized local test share"]
async fn unc_namespace_public_file_tools_preserve_contract() {
    let parent = std::env::var_os("DEEPAGENT_H78_UNC_ROOT")
        .expect("set DEEPAGENT_H78_UNC_ROOT to an authorized local test share");
    assert!(matches!(
        Path::new(&parent).components().next(),
        Some(std::path::Component::Prefix(prefix))
            if matches!(prefix.kind(), std::path::Prefix::UNC(_, _) | std::path::Prefix::VerbatimUNC(_, _))
    ));
    let backing = tempfile::Builder::new()
        .prefix("deepagent-h78-")
        .tempdir_in(parent)
        .unwrap();
    for access in [FsAccess::Workspace, FsAccess::Full] {
        public_file_tools_roundtrip(backing.path(), access).await;
    }
    // The provider's identity fallback must still distinguish two directories
    // with the same pathname across operations, including before first use.
    let original = backing.path().join("identity-root");
    let parked = backing.path().join("identity-parked");
    fs::create_dir(&original).unwrap();
    fs::write(original.join("visible.txt"), "original fixture").unwrap();
    let root = WorkspaceRoot::new(&original);
    fs::rename(&original, &parked).unwrap();
    fs::create_dir(&original).unwrap();
    fs::write(original.join("visible.txt"), "replacement fixture").unwrap();
    assert!(root.open_read(&original.join("visible.txt")).is_err());
    assert!(WorkspaceRoot::new(&original)
        .open_read(&original.join("visible.txt"))
        .is_ok());
}
