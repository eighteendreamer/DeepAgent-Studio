//! Real NTFS controls for the operation-time path boundary (H-78).

use std::fs::{self, File, OpenOptions};
use std::io::Read;
use std::os::windows::fs::OpenOptionsExt;
use std::path::Path;

use super::{FsAccess, WorkspaceRoot};

use windows_sys::Win32::Storage::FileSystem::{
    FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES,
    FILE_SHARE_READ, FILE_SHARE_WRITE, FILE_TRAVERSE, FILE_WRITE_DATA,
};

fn pin_entry(path: &Path) -> File {
    OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
        .expect("open real directory entry without write/delete sharing")
}

fn assert_identity_pinned(path: &Path, renamed: &Path) {
    let rename = fs::rename(path, renamed).expect_err("pinned entry must not be renamed");
    assert_eq!(rename.raw_os_error(), Some(32), "{rename}");
    let write_handle = OpenOptions::new()
        .access_mode(FILE_WRITE_DATA)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
        .expect_err("pinned entry must not acquire a data-write handle");
    assert_eq!(write_handle.raw_os_error(), Some(32), "{write_handle}");
}

#[test]
fn directory_identity_pin_with_read_sharing_also_blocks_child_publication() {
    let temp = tempfile::tempdir().unwrap();
    let directory = temp.path().join("directory");
    let renamed = temp.path().join("renamed");
    fs::create_dir(&directory).unwrap();
    let pin = pin_entry(&directory);
    assert_identity_pinned(&directory, &renamed);
    let child = directory.join("file.txt");
    let staged = directory.join("staged.txt");
    fs::write(&child, "original").unwrap();
    fs::write(&staged, "replacement").unwrap();
    // Characterize a rejected design, not a production implementation: this
    // sharing mode breaks normal child publication and cannot guard mutations.
    let error = fs::rename(&staged, directory.join("published.txt")).unwrap_err();
    assert_eq!(error.raw_os_error(), Some(32));
    fs::remove_file(&child).unwrap();
    drop(pin);
    fs::rename(&directory, &renamed).unwrap();
}

#[test]
fn directory_identity_pin_traverse_allows_child_publication_and_blocks_rename() {
    let temp = tempfile::tempdir().unwrap();
    let directory = temp.path().join("directory");
    let renamed = temp.path().join("renamed");
    fs::create_dir(&directory).unwrap();
    let pin = OpenOptions::new()
        .access_mode(FILE_TRAVERSE | FILE_READ_ATTRIBUTES)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(&directory)
        .unwrap();
    let error = fs::rename(&directory, &renamed).unwrap_err();
    assert_eq!(error.raw_os_error(), Some(32));
    fs::write(directory.join("staged.txt"), "synthetic fixture").unwrap();
    fs::rename(
        directory.join("staged.txt"),
        directory.join("published.txt"),
    )
    .unwrap();
    // WRITE sharing deliberately does NOT prohibit modifying the reparse data.
    // This pin alone is insufficient for the authorization boundary.
    let writer = OpenOptions::new()
        .access_mode(FILE_WRITE_DATA)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(&directory)
        .unwrap();
    drop(writer);
    drop(pin);
    fs::rename(&directory, &renamed).unwrap();
}

#[test]
fn directory_identity_pin_protects_junction_entry_not_its_children() {
    let temp = tempfile::tempdir().unwrap();
    let target = temp.path().join("target");
    let link = temp.path().join("link");
    let renamed = temp.path().join("renamed-link");
    fs::create_dir(&target).unwrap();
    let output = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(&link)
        .arg(&target)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let pin = pin_entry(&link);
    assert_identity_pinned(&link, &renamed);
    fs::write(link.join("file.txt"), "synthetic fixture").unwrap();
    assert_eq!(
        fs::read_to_string(target.join("file.txt")).unwrap(),
        "synthetic fixture"
    );
    drop(pin);
    fs::remove_dir(&link).unwrap();
    assert!(target.join("file.txt").is_file());
}

pub(super) struct Junction(std::path::PathBuf);

async fn poll_until_suspended<F: std::future::Future>(future: std::pin::Pin<&mut F>) {
    let mut future = future;
    std::future::poll_fn(|cx| match future.as_mut().poll(cx) {
        std::task::Poll::Pending => std::task::Poll::Ready(()),
        std::task::Poll::Ready(_) => panic!("operation completed before the injected replacement"),
    })
    .await;
}

impl Junction {
    pub(super) fn new(path: &Path, target: &Path) -> Self {
        let output = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(path)
            .arg(target)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        Self(path.to_owned())
    }
}

impl Drop for Junction {
    fn drop(&mut self) {
        let _ = fs::remove_dir(&self.0);
    }
}

#[test]
fn write_target_pins_ancestors_allows_publication_and_cleans_owned_files() {
    use std::io::Write;

    let workspace = tempfile::tempdir().unwrap();
    let ancestor = workspace.path().join("ancestor");
    let parent = ancestor.join("parent");
    fs::create_dir_all(&parent).unwrap();
    let root = WorkspaceRoot::new(workspace.path());
    let target = root
        .write_target(&parent.join("visible.txt"), false)
        .unwrap();
    for directory in [&parent, &ancestor, workspace.path()] {
        let error = fs::rename(directory, directory.with_extension("parked")).unwrap_err();
        assert_eq!(error.raw_os_error(), Some(32), "{error}");
    }
    let mut stage = target.stage().unwrap();
    stage
        .file_mut()
        .write_all(b"synthetic published fixture")
        .unwrap();
    stage.publish(&target, false).unwrap();
    assert_eq!(
        target.read_current().unwrap().unwrap(),
        "synthetic published fixture"
    );
    // An abandoned staging file is cleaned by its owned handle.
    drop(target.stage().unwrap());
    drop(target);
    let entries: Vec<_> = fs::read_dir(&parent)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(entries, ["visible.txt"]);
    fs::rename(&ancestor, workspace.path().join("parked")).unwrap();
}

#[test]
fn write_target_rejects_final_entry_changed_to_outward_or_dangling_junction() {
    let workspace = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    fs::write(
        outside.path().join("canary.txt"),
        "synthetic outside fixture",
    )
    .unwrap();
    let root = WorkspaceRoot::new(workspace.path());
    let path = workspace.path().join("entry");
    let target = root.write_target(&path, false).unwrap();
    let outward = Junction::new(&path, outside.path());
    assert!(target.read_current().is_err());
    drop(outward);
    let dangling = Junction::new(&path, &outside.path().join("absent"));
    assert!(
        target.read_current().is_err(),
        "dangling reparse must not be treated as a new file"
    );
    assert_eq!(
        fs::read_to_string(outside.path().join("canary.txt")).unwrap(),
        "synthetic outside fixture"
    );
    drop(dangling);
}

#[tokio::test]
async fn mutation_new_nested_parents_internal_junction_and_access_mode_controls() {
    use crate::file_tools::WriteFileTool;
    use deepagent_tools::Tool;

    for access in [
        FsAccess::ReadOnly,
        FsAccess::Workspace,
        FsAccess::ReadAnywhere,
        FsAccess::Full,
    ] {
        let workspace = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let real = workspace.path().join("real");
        fs::create_dir(&real).unwrap();
        let inward = Junction::new(&workspace.path().join("inward"), &real);
        let outward = Junction::new(&workspace.path().join("outward"), outside.path());
        let writer = WriteFileTool::new(WorkspaceRoot::new(workspace.path()).with_access(access));
        let inside = writer.invoke(serde_json::json!({"path":"inward/new/深层🦀/fixture.txt", "content":"inside fixture"})).await.unwrap();
        assert_eq!(
            inside.ok,
            access != FsAccess::ReadOnly,
            "{access:?}: {inside:?}"
        );
        assert_eq!(
            real.join("new/深层🦀/fixture.txt").exists(),
            access != FsAccess::ReadOnly
        );
        let external = writer
            .invoke(
                serde_json::json!({"path":"outward/new/fixture.txt", "content":"outside fixture"}),
            )
            .await
            .unwrap();
        assert_eq!(
            external.ok,
            access == FsAccess::Full,
            "{access:?}: {external:?}"
        );
        assert_eq!(
            outside.path().join("new/fixture.txt").exists(),
            access == FsAccess::Full
        );
        // Guard and staging files must disappear from all newly made parents.
        fn no_owned_temps(path: &Path) {
            for entry in fs::read_dir(path).unwrap() {
                let entry = entry.unwrap();
                assert!(!entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".deepagent-"));
                if entry.file_type().unwrap().is_dir() {
                    no_owned_temps(&entry.path());
                }
            }
        }
        no_owned_temps(&real);
        no_owned_temps(outside.path());
        drop((outward, inward));
    }
}

pub(super) fn junction_data(path: &Path) -> Vec<u8> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_SHARE_DELETE, MAXIMUM_REPARSE_DATA_BUFFER_SIZE,
    };
    use windows_sys::Win32::System::Ioctl::FSCTL_GET_REPARSE_POINT;
    use windows_sys::Win32::System::IO::DeviceIoControl;

    let template_handle = OpenOptions::new()
        .access_mode(FILE_READ_ATTRIBUTES)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
        .unwrap();
    let mut bytes = vec![0u8; MAXIMUM_REPARSE_DATA_BUFFER_SIZE as usize];
    let mut returned = 0;
    // SAFETY: The real junction handle and writable output allocation live for
    // this synchronous call. The kernel supplies its exact reparse data length.
    assert_ne!(
        unsafe {
            DeviceIoControl(
                template_handle.as_raw_handle(),
                FSCTL_GET_REPARSE_POINT,
                std::ptr::null(),
                0,
                bytes.as_mut_ptr().cast(),
                bytes.len() as u32,
                &mut returned,
                std::ptr::null_mut(),
            )
        },
        0,
        "{}",
        std::io::Error::last_os_error()
    );
    bytes.truncate(returned as usize);
    bytes
}

pub(super) fn junction_write_handle(directory: &Path) -> File {
    use windows_sys::Win32::Storage::FileSystem::{FILE_GENERIC_WRITE, FILE_SHARE_DELETE};
    OpenOptions::new()
        .access_mode(FILE_GENERIC_WRITE)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(directory)
        .unwrap()
}

pub(super) fn set_junction_data(writer: &File, bytes: &[u8]) -> std::io::Result<()> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::System::Ioctl::FSCTL_SET_REPARSE_POINT;
    use windows_sys::Win32::System::IO::DeviceIoControl;
    let mut returned = 0;
    // SAFETY: Reuse the kernel-returned junction buffer verbatim. This
    // handle addresses only a synthetic temporary directory.
    if unsafe {
        DeviceIoControl(
            writer.as_raw_handle(),
            FSCTL_SET_REPARSE_POINT,
            bytes.as_ptr().cast(),
            bytes.len() as u32,
            std::ptr::null_mut(),
            0,
            &mut returned,
            std::ptr::null_mut(),
        )
    } == 0
    {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[test]
fn write_target_guard_blocks_in_place_reparse_with_preexisting_write_handle() {
    let workspace = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let template_path = workspace.path().join("template");
    let template = Junction::new(&template_path, outside.path());
    let bytes = junction_data(&template_path);
    let directory = workspace.path().join("parent");
    fs::create_dir(&directory).unwrap();
    let writer = junction_write_handle(&directory);
    let convert = || set_junction_data(&writer, &bytes);
    let root = WorkspaceRoot::new(workspace.path());
    let target = root
        .write_target(&directory.join("new.txt"), false)
        .unwrap();
    let failure = convert().expect_err("guarded nonempty parent must reject conversion");
    assert_eq!(failure.raw_os_error(), Some(145), "{failure}");
    assert!(!fs::symlink_metadata(&directory)
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(fs::read_dir(outside.path()).unwrap().count(), 0);
    drop(target);
    convert().expect("same buffer and preexisting handle must work after guard is removed");
    drop(writer);
    let converted = Junction(directory);
    drop(converted);
    drop(template);
}

#[test]
fn physical_preflight_and_handle_read_preserve_normal_links_and_policy_modes() {
    let workspace = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let real = workspace.path().join("real");
    fs::create_dir(&real).unwrap();
    fs::write(real.join("visible.txt"), "inside fixture").unwrap();
    fs::write(outside.path().join("visible.txt"), "outside fixture").unwrap();
    let inside = Junction::new(&workspace.path().join("inside-link"), &real);
    let external = Junction::new(&workspace.path().join("outside-link"), outside.path());
    let root = WorkspaceRoot::new(workspace.path());
    let input = root.resolve_read("inside-link/visible.txt").unwrap();
    let mut body = String::new();
    root.open_read(&input)
        .unwrap()
        .read_to_string(&mut body)
        .unwrap();
    assert_eq!(body, "inside fixture");
    // Return the original entry path for display/cache/rename semantics.
    assert_eq!(input, workspace.path().join("inside-link/visible.txt"));
    assert!(root.resolve_read("outside-link/visible.txt").is_err());
    assert!(root
        .resolve_write("outside-link/new/deep/file.txt")
        .is_err());
    assert!(root.resolve_write("inside-link/new/deep/file.txt").is_ok());
    for access in [FsAccess::ReadAnywhere, FsAccess::Full] {
        let allowed = root.clone().with_access(access);
        let path = allowed.resolve_read("outside-link/visible.txt").unwrap();
        body.clear();
        allowed
            .open_read(&path)
            .unwrap()
            .read_to_string(&mut body)
            .unwrap();
        assert_eq!(body, "outside fixture");
        assert_eq!(
            allowed.resolve_write("outside-link/new.txt").is_ok(),
            access == FsAccess::Full
        );
    }
    // A workspace that is itself an alias is authorized symmetrically.
    let alias_parent = tempfile::tempdir().unwrap();
    let alias = Junction::new(&alias_parent.path().join("workspace"), workspace.path());
    let alias_root = WorkspaceRoot::new(&alias.0);
    let path = alias_root.resolve_read("inside-link/visible.txt").unwrap();
    assert!(alias_root.open_read(&path).is_ok());
    drop((alias, external, inside));
}

#[test]
fn physical_preflight_rejects_dangling_and_leave_then_return_paths() {
    let workspace = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let real = workspace.path().join("real");
    fs::create_dir(&real).unwrap();
    fs::write(real.join("visible.txt"), "inside fixture").unwrap();
    let back = Junction::new(&outside.path().join("back"), &real);
    let outward = Junction::new(&workspace.path().join("outward"), outside.path());
    let root = WorkspaceRoot::new(workspace.path());
    assert!(root.resolve_read("outward/back/visible.txt").is_err());
    let target = workspace.path().join("disappearing");
    fs::create_dir(&target).unwrap();
    let dangling = Junction::new(&workspace.path().join("dangling"), &target);
    fs::remove_dir(&target).unwrap();
    assert!(root.resolve_read("dangling/new.txt").is_err());
    assert!(root.resolve_write("dangling/new/deep.txt").is_err());
    drop((dangling, outward, back));
}

#[test]
fn native_open_rejects_parent_replaced_after_physical_preflight() {
    let workspace = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let parent = workspace.path().join("parent");
    let parked = workspace.path().join("parked");
    fs::create_dir(&parent).unwrap();
    fs::write(parent.join("visible.txt"), "inside fixture").unwrap();
    fs::write(outside.path().join("visible.txt"), "outside fixture").unwrap();
    let root = WorkspaceRoot::new(workspace.path());
    let authorized = root
        .check_physical(&parent.join("visible.txt"), true)
        .unwrap();
    fs::rename(&parent, &parked).unwrap();
    let replacement = Junction::new(&parent, outside.path());
    assert_eq!(fs::read_to_string(&authorized).unwrap(), "outside fixture");
    let error = super::windows::open_read(&authorized).unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
    assert!(root.open_read(&parent.join("visible.txt")).is_err());
    let mut body = String::new();
    super::windows::open_read(&parked.join("visible.txt"))
        .unwrap()
        .read_to_string(&mut body)
        .unwrap();
    assert_eq!(body, "inside fixture");
    drop(replacement);
}

#[test]
fn physical_preflight_rejects_hidden_intermediate_link_targets() {
    let workspace = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let real = workspace.path().join("real");
    fs::create_dir(&real).unwrap();
    fs::write(real.join("visible.txt"), "inside fixture").unwrap();
    let back = Junction::new(&outside.path().join("back"), &real);
    // Resolving a whole lexical prefix hides the out-of-root intermediate hop.
    let chain = Junction::new(
        &workspace.path().join("chain"),
        &outside.path().join("back"),
    );
    let sensitive = Junction::new(&workspace.path().join("secret-store"), &real);
    let hidden_sensitive = Junction::new(
        &workspace.path().join("alias"),
        &workspace.path().join("secret-store"),
    );
    let mut violations = Vec::new();
    for access in [
        FsAccess::ReadOnly,
        FsAccess::Workspace,
        FsAccess::ReadAnywhere,
        FsAccess::Full,
    ] {
        let root = WorkspaceRoot::new(workspace.path()).with_access(access);
        let read = root.resolve_read("chain/visible.txt").is_ok();
        let write = root.resolve_write("chain/new.txt").is_ok();
        let sensitive_read = root.resolve_read("alias/visible.txt").is_ok();
        let sensitive_write = root.resolve_write("alias/new.txt").is_ok();
        eprintln!("{access:?}: chain_read={read}, chain_write={write}, sensitive_read={sensitive_read}, sensitive_write={sensitive_write}");
        if read != matches!(access, FsAccess::ReadAnywhere | FsAccess::Full)
            || write != (access == FsAccess::Full)
            || sensitive_read
            || sensitive_write
        {
            violations.push(access);
        }
    }
    drop((hidden_sensitive, sensitive, chain, back));
    assert!(
        violations.is_empty(),
        "intermediate target policy violations: {violations:?}"
    );
}

#[test]
fn physical_preflight_rejects_link_to_root_ancestor_even_when_suffix_returns() {
    let workspace = tempfile::tempdir().unwrap();
    fs::write(workspace.path().join("visible.txt"), "inside fixture").unwrap();
    let link = Junction::new(
        &workspace.path().join("up"),
        workspace.path().parent().unwrap(),
    );
    let input = Path::new("up")
        .join(workspace.path().file_name().unwrap())
        .join("visible.txt");
    for access in [FsAccess::Workspace, FsAccess::Full] {
        let root = WorkspaceRoot::new(workspace.path()).with_access(access);
        let permitted = root.resolve_read(input.to_str().unwrap()).is_ok();
        eprintln!("ancestor link {access:?}: permitted={permitted}");
        assert_eq!(permitted, access == FsAccess::Full, "{access:?}");
    }
    drop(link);
}

#[test]
fn native_local_disk_open_rejects_other_namespaces_without_io() {
    for path in [r"C:relative", r"\rooted", r"\\.\pipe\fixture"] {
        let error = super::windows::open_read(Path::new(path)).unwrap_err();
        assert_eq!(
            error.kind(),
            std::io::ErrorKind::InvalidInput,
            "{path}: {error}"
        );
    }
}

#[test]
fn unconfined_volume_root_identity_preserves_normal_disk_spelling() {
    let workspace = tempfile::tempdir().unwrap();
    let volume = workspace.path().ancestors().last().unwrap();
    let root = WorkspaceRoot::new(workspace.path()).with_access(FsAccess::Full);
    assert_eq!(
        root.read_identity(volume).unwrap(),
        fs::canonicalize(volume).unwrap()
    );
}

#[test]
fn native_directory_rejects_parent_junction_replacement_after_preflight() {
    let workspace = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let parent = workspace.path().join("parent");
    fs::create_dir(&parent).unwrap();
    fs::write(parent.join("inside.txt"), "inside fixture").unwrap();
    fs::write(outside.path().join("outside.txt"), "outside fixture").unwrap();
    let root = WorkspaceRoot::new(workspace.path());
    let authorized = root.read_identity(&parent).unwrap();
    let parked = workspace.path().join("parked");
    fs::rename(&parent, &parked).unwrap();
    let replacement = Junction::new(&parent, outside.path());
    // Demonstrate why a string-based read_dir after preflight is insufficient.
    assert_eq!(
        fs::read_dir(&authorized)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .file_name(),
        "outside.txt"
    );
    let error = super::windows::read_directory(&authorized).err().unwrap();
    assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
    assert!(root.read_directory(&parent).is_err());
    let control = root.read_directory(&parked).unwrap();
    assert_eq!(control.entries.len(), 1);
    assert_eq!(control.entries[0].name, "inside.txt");
    drop(replacement);
}

#[tokio::test]
async fn read_tool_rechecks_path_after_async_preflight_without_recording_outside_observation() {
    use std::future::{poll_fn, Future};
    use std::sync::{Arc, Mutex};
    use std::task::Poll;

    use deepagent_tools::Tool;

    use crate::file_cache::FileStateCache;
    use crate::file_tools::ReadFileTool;

    let workspace = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let parent = workspace.path().join("parent");
    let parked = workspace.path().join("parked");
    fs::create_dir(&parent).unwrap();
    fs::write(parent.join("visible.txt"), "inside fixture").unwrap();
    fs::write(outside.path().join("visible.txt"), "outside fixture").unwrap();
    let path = parent.join("visible.txt");
    let gate = deepagent_persistence::fs_commit::lock_path_operation(&path).unwrap();
    let cache = Arc::new(Mutex::new(FileStateCache::new()));
    let reader = ReadFileTool::with_cache(WorkspaceRoot::new(workspace.path()), cache.clone());
    let mut read = Box::pin(reader.invoke(serde_json::json!({"path": "parent/visible.txt"})));
    // The first poll runs the public tool's preflight and schedules its blocking
    // operation. Its read cannot finish while the original path gate is held.
    // No sleep or assumption about the worker's exact instruction is needed.
    poll_fn(|cx| match read.as_mut().poll(cx) {
        Poll::Pending => Poll::Ready(()),
        Poll::Ready(result) => panic!("read completed before injected replacement: {result:?}"),
    })
    .await;
    fs::rename(&parent, &parked).unwrap();
    let replacement = Junction::new(&parent, outside.path());
    drop(gate);
    let output = read.await.unwrap();
    assert!(!output.ok, "{output:?}");
    assert!(!output.value.to_string().contains("outside fixture"));
    assert!(!cache.lock().unwrap().has_been_read(&path));
    let control = reader
        .invoke(serde_json::json!({"path": "parked/visible.txt"}))
        .await
        .unwrap();
    assert!(control.ok, "{control:?}");
    assert!(control.value.to_string().contains("inside fixture"));
    drop(replacement);
}

#[tokio::test]
async fn directory_tools_confine_outward_links_preserve_internal_aliases_and_stop_cycles() {
    use deepagent_tools::Tool;

    use crate::file_tools::{GlobTool, GrepTool, ListDirTool};

    let workspace = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let real = workspace.path().join("real");
    fs::create_dir(&real).unwrap();
    fs::write(real.join("inside.txt"), "MATCH inside fixture").unwrap();
    fs::write(outside.path().join("outside.txt"), "MATCH outside fixture").unwrap();
    let inward = Junction::new(&workspace.path().join("inward"), &real);
    let outward = Junction::new(&workspace.path().join("outward"), outside.path());
    let cycle = Junction::new(&real.join("back"), workspace.path());
    for access in [
        FsAccess::Workspace,
        FsAccess::ReadOnly,
        FsAccess::ReadAnywhere,
        FsAccess::Full,
    ] {
        let root = WorkspaceRoot::new(workspace.path()).with_access(access);
        let external_allowed = matches!(access, FsAccess::ReadAnywhere | FsAccess::Full);
        let listing = ListDirTool::new(root.clone())
            .invoke(serde_json::json!({"path": "outward"}))
            .await
            .unwrap();
        assert_eq!(listing.ok, external_allowed, "{access:?}: {listing:?}");
        assert_eq!(
            listing.value.to_string().contains("outside.txt"),
            external_allowed
        );
        let listing = ListDirTool::new(root.clone())
            .invoke(serde_json::json!({"path": "inward"}))
            .await
            .unwrap();
        assert!(listing.ok);
        assert!(listing.value["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["name"] == "inside.txt"));
        let glob = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            GlobTool::new(root.clone()).invoke(serde_json::json!({"pattern": "**/*.txt"})),
        )
        .await
        .expect("junction cycle must not keep traversal running")
        .unwrap();
        assert!(glob.ok, "{glob:?}");
        let mut expected = vec!["inward/inside.txt", "real/inside.txt"];
        if external_allowed {
            expected.insert(1, "outward/outside.txt");
        }
        assert_eq!(glob.value["matches"], serde_json::json!(expected));
        let grep = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            GrepTool::new(root).invoke(serde_json::json!({"query": "MATCH"})),
        )
        .await
        .expect("grep cycle must not keep traversal running")
        .unwrap();
        assert!(grep.ok, "{grep:?}");
        let hits = grep.value["hits"].as_array().unwrap();
        assert_eq!(hits.len(), expected.len());
        assert_eq!(
            hits.iter()
                .map(|hit| hit["file"].as_str().unwrap())
                .collect::<Vec<_>>(),
            expected
        );
        assert_eq!(
            grep.value.to_string().contains("outside fixture"),
            external_allowed
        );
    }
    drop((cycle, outward, inward));
}

#[tokio::test]
async fn mutation_tool_rechecks_parent_after_waiting_for_existing_path_gate() {
    use std::sync::{Arc, Mutex};

    use deepagent_tools::Tool;

    use crate::file_cache::FileStateCache;
    use crate::file_tools::{EditFileTool, MultiEditTool, ReadFileTool, WriteFileTool};

    const INITIAL: &str = "synthetic value=old";
    let mut observations = Vec::new();
    for tool in 0..3 {
        let workspace = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let parent = workspace.path().join("parent");
        fs::create_dir(&parent).unwrap();
        fs::write(parent.join("visible.txt"), INITIAL).unwrap();
        // Identical outside bytes isolate H-78 from H-76 stale-content checks.
        fs::write(outside.path().join("visible.txt"), INITIAL).unwrap();
        let root = WorkspaceRoot::new(workspace.path());
        let cache = Arc::new(Mutex::new(FileStateCache::new()));
        let read = ReadFileTool::with_cache(root.clone(), cache.clone())
            .invoke(serde_json::json!({"path":"parent/visible.txt"}))
            .await
            .unwrap();
        assert!(read.ok);
        let (writer, args): (Box<dyn Tool>, _) = match tool {
            0 => (
                Box::new(WriteFileTool::with_cache(root.clone(), cache.clone())),
                serde_json::json!({"path":"parent/visible.txt","content":"synthetic value=new"}),
            ),
            1 => (
                Box::new(EditFileTool::with_cache(root.clone(), cache.clone())),
                serde_json::json!({"path":"parent/visible.txt","old":"value=old","new":"value=new"}),
            ),
            _ => (
                Box::new(MultiEditTool::with_cache(root, cache)),
                serde_json::json!({"path":"parent/visible.txt","edits":[{"old":"value=old","new":"value=new"}]}),
            ),
        };
        let gate =
            deepagent_persistence::fs_commit::lock_path_operation(&parent.join("visible.txt"))
                .unwrap();
        let mut operation = Box::pin(writer.invoke(args));
        poll_until_suspended(operation.as_mut()).await;
        let parked = workspace.path().join("parked");
        fs::rename(&parent, &parked).unwrap();
        let replacement = Junction::new(&parent, outside.path());
        drop(gate);
        let output = operation.await.unwrap();
        let outside_unchanged =
            fs::read_to_string(outside.path().join("visible.txt")).unwrap() == INITIAL;
        let original_unchanged = fs::read_to_string(parked.join("visible.txt")).unwrap() == INITIAL;
        eprintln!("mutation parent replacement tool={tool}: ok={}, outside_unchanged={outside_unchanged}, original_unchanged={original_unchanged}", output.ok);
        observations.push(!output.ok && outside_unchanged && original_unchanged);
        drop(replacement);
    }
    assert!(observations.iter().all(|safe| *safe), "{observations:?}");
}

#[tokio::test]
async fn mutation_tool_rechecks_parent_for_new_file_after_path_gate() {
    use deepagent_tools::Tool;

    use crate::file_tools::WriteFileTool;

    let workspace = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let parent = workspace.path().join("parent");
    fs::create_dir(&parent).unwrap();
    let gate =
        deepagent_persistence::fs_commit::lock_path_operation(&parent.join("new.txt")).unwrap();
    let writer = WriteFileTool::new(WorkspaceRoot::new(workspace.path()));
    let mut operation = Box::pin(
        writer
            .invoke(serde_json::json!({"path":"parent/new.txt","content":"synthetic new fixture"})),
    );
    poll_until_suspended(operation.as_mut()).await;
    let parked = workspace.path().join("parked");
    fs::rename(&parent, &parked).unwrap();
    let replacement = Junction::new(&parent, outside.path());
    drop(gate);
    let output = operation.await.unwrap();
    let outside_created = outside.path().join("new.txt").exists();
    eprintln!(
        "new-file parent replacement: ok={}, outside_created={outside_created}",
        output.ok
    );
    assert!(!output.ok && !outside_created, "{output:?}");
    drop(replacement);
}

#[test]
fn entry_tools_recheck_parent_after_waiting_in_blocking_queue() {
    use crate::file_tools::{DeletePathTool, MovePathTool};
    use deepagent_tools::Tool;

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .max_blocking_threads(1)
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let mut observations = Vec::new();
        for operation_kind in 0..4 {
            let workspace = tempfile::tempdir().unwrap();
            let outside = tempfile::tempdir().unwrap();
            let parent = workspace.path().join("parent");
            fs::create_dir(&parent).unwrap();
            let is_directory = operation_kind % 2 == 1;
            let inside_entry = parent.join("entry");
            let outside_entry = outside.path().join("entry");
            for entry in [&inside_entry, &outside_entry] {
                if is_directory {
                    fs::create_dir(entry).unwrap();
                    fs::write(entry.join("fixture.txt"), "synthetic entry fixture").unwrap();
                } else {
                    fs::write(entry, "synthetic entry fixture").unwrap();
                }
            }
            let root = WorkspaceRoot::new(workspace.path());
            let (tool, args): (Box<dyn Tool>, _) = if operation_kind < 2 {
                (Box::new(DeletePathTool::new(root)), serde_json::json!({"path":"parent/entry", "recursive":is_directory}))
            } else {
                (Box::new(MovePathTool::new(root)), serde_json::json!({"source":"parent/entry", "destination":"moved-entry"}))
            };
            let (release, resume) = std::sync::mpsc::channel();
            let (started, ready) = tokio::sync::oneshot::channel();
            let blocker = tokio::task::spawn_blocking(move || {
                started.send(()).unwrap();
                resume.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
            });
            ready.await.unwrap();
            let mut operation = Box::pin(tool.invoke(args));
            // The only blocking worker is occupied. The real tool has finished
            // its synchronous authorization and queued the actual filesystem I/O.
            poll_until_suspended(operation.as_mut()).await;
            let parked = workspace.path().join("parked");
            fs::rename(&parent, &parked).unwrap();
            let replacement = Junction::new(&parent, outside.path());
            release.send(()).unwrap();
            let output = operation.await.unwrap();
            blocker.await.unwrap();
            let content_path = |entry: &Path| if is_directory { entry.join("fixture.txt") } else { entry.to_owned() };
            let outside_unchanged = fs::read_to_string(content_path(&outside_entry)).ok().as_deref() == Some("synthetic entry fixture");
            let original_unchanged = fs::read_to_string(content_path(&parked.join("entry"))).ok().as_deref() == Some("synthetic entry fixture");
            let destination_absent = !workspace.path().join("moved-entry").exists();
            eprintln!("entry parent replacement operation={operation_kind}: ok={}, outside_unchanged={outside_unchanged}, original_unchanged={original_unchanged}, destination_absent={destination_absent}", output.ok);
            observations.push(!output.ok && outside_unchanged && original_unchanged && destination_absent);
            drop(replacement);
        }
        assert!(observations.iter().all(|safe| *safe), "{observations:?}");
    });
}

#[tokio::test]
async fn entry_tools_preserve_internal_links_root_alias_and_access_mode_controls() {
    use crate::file_tools::{DeletePathTool, MovePathTool};
    use deepagent_tools::Tool;

    for access in [
        FsAccess::ReadOnly,
        FsAccess::Workspace,
        FsAccess::ReadAnywhere,
        FsAccess::Full,
    ] {
        let workspace = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let alias_store = tempfile::tempdir().unwrap();
        let real = workspace.path().join("real");
        fs::create_dir(&real).unwrap();
        let inward = Junction::new(&workspace.path().join("inward"), &real);
        let outward = Junction::new(&workspace.path().join("outward"), outside.path());
        let root_alias_path = alias_store.path().join("workspace-alias");
        let root_alias = Junction::new(&root_alias_path, workspace.path());
        let root = WorkspaceRoot::new(&root_alias_path).with_access(access);
        let delete = DeletePathTool::new(root.clone());
        let mover = MovePathTool::new(root);
        for name in ["delete.txt", "source.txt"] {
            fs::write(real.join(name), "inside fixture").unwrap();
            fs::write(outside.path().join(name), "outside fixture").unwrap();
        }
        let deleted = delete
            .invoke(serde_json::json!({"path":"inward/delete.txt"}))
            .await
            .unwrap();
        assert_eq!(
            deleted.ok,
            access != FsAccess::ReadOnly,
            "{access:?}: {deleted:?}"
        );
        assert_eq!(
            real.join("delete.txt").exists(),
            access == FsAccess::ReadOnly
        );
        let moved = mover.invoke(serde_json::json!({"source":"inward/source.txt", "destination":"inward/new/深层🦀/moved.txt"})).await.unwrap();
        assert_eq!(
            moved.ok,
            access != FsAccess::ReadOnly,
            "{access:?}: {moved:?}"
        );
        if moved.ok {
            assert_eq!(moved.value["source"], "inward/source.txt");
            assert_eq!(moved.value["destination"], "inward/new/深层🦀/moved.txt");
            assert_eq!(
                fs::read_to_string(real.join("new/深层🦀/moved.txt")).unwrap(),
                "inside fixture"
            );
        }
        let deleted = delete
            .invoke(serde_json::json!({"path":"outward/delete.txt"}))
            .await
            .unwrap();
        assert_eq!(
            deleted.ok,
            access == FsAccess::Full,
            "{access:?}: {deleted:?}"
        );
        assert_eq!(
            outside.path().join("delete.txt").exists(),
            access != FsAccess::Full
        );
        let moved = mover.invoke(serde_json::json!({"source":"outward/source.txt", "destination":"external-moved.txt"})).await.unwrap();
        assert_eq!(moved.ok, access == FsAccess::Full, "{access:?}: {moved:?}");
        assert_eq!(
            outside.path().join("source.txt").exists(),
            access != FsAccess::Full
        );
        // A final junction is moved as an entry. Its target and target bytes
        // must survive, unlike canonicalizing the complete source pathname.
        let kept = real.join("kept");
        fs::create_dir(&kept).unwrap();
        fs::write(kept.join("fixture.txt"), "kept fixture").unwrap();
        let original_link = Junction::new(&real.join("link"), &kept);
        let moved = mover
            .invoke(serde_json::json!({"source":"inward/link", "destination":"inward/moved-link"}))
            .await
            .unwrap();
        assert_eq!(
            moved.ok,
            access != FsAccess::ReadOnly,
            "{access:?}: {moved:?}"
        );
        assert_eq!(
            fs::read_to_string(kept.join("fixture.txt")).unwrap(),
            "kept fixture"
        );
        if moved.ok {
            assert!(!real.join("link").exists());
            assert!(fs::symlink_metadata(real.join("moved-link"))
                .unwrap()
                .file_type()
                .is_symlink());
            let deleted = delete
                .invoke(serde_json::json!({"path":"inward/moved-link", "recursive":true}))
                .await
                .unwrap();
            assert!(deleted.ok, "{access:?}: {deleted:?}");
            assert_eq!(
                fs::read_to_string(kept.join("fixture.txt")).unwrap(),
                "kept fixture"
            );
        }
        drop(original_link);
        drop((root_alias, outward, inward));
    }
}

#[tokio::test]
async fn recursive_delete_unlinks_nested_outward_junction_without_deleting_target() {
    use crate::file_tools::DeletePathTool;
    use deepagent_tools::Tool;

    let workspace = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let tree = workspace.path().join("tree");
    fs::create_dir_all(tree.join("nested")).unwrap();
    fs::write(tree.join("inside.txt"), "inside fixture").unwrap();
    fs::write(outside.path().join("keep.txt"), "synthetic outside fixture").unwrap();
    let link = Junction::new(&tree.join("nested").join("outward"), outside.path());
    let output = DeletePathTool::new(WorkspaceRoot::new(workspace.path()))
        .invoke(serde_json::json!({"path":"tree", "recursive":true}))
        .await
        .unwrap();
    assert!(output.ok, "{output:?}");
    assert!(!tree.exists());
    assert_eq!(
        fs::read_to_string(outside.path().join("keep.txt")).unwrap(),
        "synthetic outside fixture"
    );
    drop(link);
}

#[test]
fn entry_target_deletes_pinned_parent_entry_after_lexical_alias_replacement() {
    let workspace = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let real = workspace.path().join("real");
    fs::create_dir(&real).unwrap();
    fs::write(real.join("entry.txt"), "inside fixture").unwrap();
    fs::write(outside.path().join("entry.txt"), "outside fixture").unwrap();
    let alias = workspace.path().join("alias");
    let inward = Junction::new(&alias, &real);
    let root = WorkspaceRoot::new(workspace.path());
    let target = root.entry_target(&alias.join("entry.txt"), false).unwrap();
    drop(inward);
    let replacement = Junction::new(&alias, outside.path());
    target.delete_entry(false).unwrap();
    assert!(!real.join("entry.txt").exists());
    assert_eq!(
        fs::read_to_string(outside.path().join("entry.txt")).unwrap(),
        "outside fixture"
    );
    drop(replacement);
}

#[test]
fn workspace_root_authorization_does_not_rebase_after_root_replacement() {
    let mut violations = Vec::new();
    for original_alias in [false, true] {
        let home = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let real = home.path().join("real");
        let logical = home.path().join("workspace");
        fs::create_dir(&real).unwrap();
        fs::write(real.join("visible.txt"), "original fixture").unwrap();
        fs::write(outside.path().join("visible.txt"), "replacement fixture").unwrap();
        let alias = if original_alias {
            Some(Junction::new(&logical, &real))
        } else {
            fs::rename(&real, &logical).unwrap();
            None
        };
        let root = WorkspaceRoot::new(&logical);
        let path = root.resolve_read("visible.txt").unwrap();
        let cloned = root.clone();
        drop(alias);
        if !original_alias {
            fs::rename(&logical, &real).unwrap();
        }
        let replacement = Junction::new(&logical, outside.path());
        let permitted = cloned.open_read(&path).is_ok();
        eprintln!("root replacement original_alias={original_alias}, permitted={permitted}");
        if permitted {
            violations.push(original_alias);
        }
        assert_eq!(
            fs::read_to_string(real.join("visible.txt")).unwrap(),
            "original fixture"
        );
        assert_eq!(
            fs::read_to_string(outside.path().join("visible.txt")).unwrap(),
            "replacement fixture"
        );
        drop(replacement);
    }
    assert!(
        violations.is_empty(),
        "root authorization rebased: {violations:?}"
    );
}

#[test]
fn workspace_root_identity_survives_clone_and_rejects_same_path_new_directory_before_first_use() {
    let home = tempfile::tempdir().unwrap();
    let logical = home.path().join("workspace");
    let parked = home.path().join("parked");
    fs::create_dir(&logical).unwrap();
    fs::write(logical.join("visible.txt"), "original fixture").unwrap();
    let root = WorkspaceRoot::new(&logical);
    let cloned = root.clone();
    // No resolution/use has established trust yet. The constructor owns it.
    fs::rename(&logical, &parked).unwrap();
    fs::create_dir(&logical).unwrap();
    fs::write(logical.join("visible.txt"), "replacement fixture").unwrap();
    for access in [
        FsAccess::ReadOnly,
        FsAccess::Workspace,
        FsAccess::ReadAnywhere,
    ] {
        let root = cloned.clone().with_access(access);
        assert!(root.write_identity(&logical.join("new.txt")).is_err());
        if access != FsAccess::ReadAnywhere {
            assert!(root.open_read(&logical.join("visible.txt")).is_err());
            assert!(root.read_directory(&logical).is_err());
        }
    }
    // A newly constructed root may deliberately authorize the new directory.
    assert!(WorkspaceRoot::new(&logical)
        .open_read(&logical.join("visible.txt"))
        .is_ok());
    assert_eq!(
        fs::read_to_string(parked.join("visible.txt")).unwrap(),
        "original fixture"
    );
    assert_eq!(
        fs::read_to_string(logical.join("visible.txt")).unwrap(),
        "replacement fixture"
    );
}

#[test]
fn workspace_root_read_pin_blocks_rename_without_writing_guard_files() {
    let home = tempfile::tempdir().unwrap();
    let logical = home.path().join("workspace");
    fs::create_dir(&logical).unwrap();
    let root = WorkspaceRoot::new(&logical);
    let pin = root.pin_root(true).unwrap();
    let error = fs::rename(&logical, home.path().join("parked")).unwrap_err();
    assert_eq!(error.raw_os_error(), Some(32));
    assert_eq!(fs::read_dir(&logical).unwrap().count(), 0);
    drop(pin);
    fs::rename(&logical, home.path().join("parked")).unwrap();
}

#[tokio::test]
async fn workspace_root_missing_creation_binds_new_root_and_rejects_later_replacement() {
    use crate::file_tools::WriteFileTool;
    use deepagent_tools::Tool;

    let home = tempfile::tempdir().unwrap();
    let logical = home.path().join("new").join("工作区🦀");
    let root = WorkspaceRoot::new(&logical);
    assert!(!logical.exists());
    let output = WriteFileTool::new(root.clone())
        .invoke(serde_json::json!({"path":"nested/file.txt", "content":"synthetic fixture"}))
        .await
        .unwrap();
    assert!(output.ok, "{output:?}");
    assert_eq!(
        fs::read_to_string(logical.join("nested/file.txt")).unwrap(),
        "synthetic fixture"
    );
    fs::rename(&logical, home.path().join("parked")).unwrap();
    fs::create_dir(&logical).unwrap();
    let output = WriteFileTool::new(root)
        .invoke(serde_json::json!({"path":"new.txt", "content":"must remain absent"}))
        .await
        .unwrap();
    assert!(!output.ok, "{output:?}");
    assert!(!logical.join("new.txt").exists());
    assert_eq!(fs::read_dir(&logical).unwrap().count(), 0);
}

#[tokio::test]
async fn mutation_final_file_symlink_preserves_existing_entry_replacement_contract() {
    use crate::file_tools::{EditFileTool, MultiEditTool, ReadFileTool, WriteFileTool};
    use deepagent_tools::Tool;

    for tool in ["write_file", "edit_file", "multi_edit"] {
        let workspace = tempfile::tempdir().unwrap();
        let target = workspace.path().join("target.txt");
        let link = workspace.path().join("alias.txt");
        fs::write(&target, "original fixture").unwrap();
        std::os::windows::fs::symlink_file(&target, &link)
            .expect("create local file symlink fixture");
        let root = WorkspaceRoot::new(workspace.path());
        let cache = std::sync::Arc::new(std::sync::Mutex::new(
            crate::file_cache::FileStateCache::new(),
        ));
        let read = ReadFileTool::with_cache(root.clone(), cache.clone())
            .invoke(serde_json::json!({"path":"alias.txt"}))
            .await
            .unwrap();
        assert!(read.ok, "{read:?}");
        let write = match tool {
        "write_file" => WriteFileTool::with_cache(root, cache)
            .invoke(serde_json::json!({"path":"alias.txt","content":"replacement fixture"})).await,
        "edit_file" => EditFileTool::with_cache(root, cache)
            .invoke(serde_json::json!({"path":"alias.txt","old":"original","new":"replacement"})).await,
        "multi_edit" => MultiEditTool::with_cache(root, cache)
            .invoke(serde_json::json!({"path":"alias.txt","edits":[{"old":"original","new":"intermediate"},{"old":"intermediate","new":"replacement"}]})).await,
        _ => unreachable!(),
    }.unwrap();
        assert!(write.ok, "{write:?}");
        let target_text = fs::read_to_string(&target).unwrap();
        let link_is_symlink = fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink();
        eprintln!("final-link {tool}: target={target_text:?}, alias_is_link={link_is_symlink}");
        assert_eq!(target_text, "original fixture");
        assert!(!link_is_symlink);
        assert_eq!(fs::read_to_string(&link).unwrap(), "replacement fixture");
    }
}

#[test]
fn mutation_final_file_symlink_head_native_replace_acts_on_entry() {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };

    let workspace = tempfile::tempdir().unwrap();
    let target = workspace.path().join("target.txt");
    let link = workspace.path().join("alias.txt");
    let stage = workspace.path().join("staged.txt");
    fs::write(&target, "original fixture").unwrap();
    fs::write(&stage, "replacement fixture").unwrap();
    std::os::windows::fs::symlink_file(&target, &link).unwrap();
    let source: Vec<u16> = stage.as_os_str().encode_wide().chain(Some(0)).collect();
    let destination: Vec<u16> = link.as_os_str().encode_wide().chain(Some(0)).collect();
    // Same native call/flags as HEAD's file_mutation::replace_atomically.
    // SAFETY: Both NUL-terminated UTF-16 paths live throughout the call.
    let result = unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    assert_ne!(result, 0, "{}", std::io::Error::last_os_error());
    assert_eq!(fs::read_to_string(&target).unwrap(), "original fixture");
    assert!(!fs::symlink_metadata(&link)
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(fs::read_to_string(&link).unwrap(), "replacement fixture");
}
