mod support;

use deepagent_ssh::{
    DirectoryWalkLimits, FileConflictPolicy, FileTransferDirection, FileTransferRequest,
    RemoteFileKind, SshService,
};
use russh_sftp::protocol::FileAttributes;
use std::{path::Path, sync::Arc};
use tokio_util::sync::CancellationToken;

async fn fixture() -> (
    support::SshServer,
    tempfile::TempDir,
    Arc<SshService>,
    String,
) {
    let server = support::SshServer::start("127.0.0.1").await;
    let dir = tempfile::tempdir().unwrap();
    let trust = dir.path().join("known_hosts");
    server.write_trust(&trust);
    let service = Arc::new(SshService::new(dir.path().to_owned()));
    let id = server.create(&service, Some(&trust)).await.id;
    server
        .counts
        .directories
        .write()
        .unwrap()
        .insert("/tree".into());
    (server, dir, service, id)
}

fn request(direction: FileTransferDirection, local: &Path) -> FileTransferRequest {
    FileTransferRequest {
        direction,
        local_path: local.to_str().unwrap().into(),
        remote_path: "/tree".into(),
        conflict: FileConflictPolicy::Ask,
    }
}

#[tokio::test]
async fn upload_plan_is_read_only_sorted_and_keeps_empty_directories_and_zero_byte_files() {
    let (server, dir, service, id) = fixture().await;
    let source = dir.path().join("source");
    std::fs::create_dir_all(source.join("空目录")).unwrap();
    std::fs::create_dir_all(source.join("nested")).unwrap();
    std::fs::write(source.join("nested/中文.bin"), [0, 1, 255]).unwrap();
    std::fs::write(source.join("zero"), []).unwrap();
    let plan = service
        .plan_directory_transfer(
            &id,
            request(FileTransferDirection::Upload, &source),
            DirectoryWalkLimits::default(),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(plan.total_bytes, "3");
    assert_eq!(plan.total_files, 2);
    assert_eq!(plan.remote_root, "/tree");
    assert_eq!(
        plan.entries
            .iter()
            .map(|e| e.relative_path.as_str())
            .collect::<Vec<_>>(),
        ["nested", "nested/中文.bin", "zero", "空目录"]
    );
    assert_eq!(plan.entries[3].kind, RemoteFileKind::Directory);
    assert_eq!(plan.entries[2].size_bytes.as_deref(), Some("0"));
    assert_eq!(
        std::fs::canonicalize(&source).unwrap(),
        Path::new(&plan.local_root)
    );
    assert_eq!(
        server.auth_count(),
        0,
        "local planning does not authenticate or create remote files"
    );
    assert!(server.counts.files.read().unwrap().is_empty());
}

#[tokio::test]
async fn download_plan_uses_existing_remote_walk_without_creating_local_directories() {
    let (server, dir, service, id) = fixture().await;
    server
        .counts
        .directories
        .write()
        .unwrap()
        .insert("/tree/empty".into());
    server
        .counts
        .files
        .write()
        .unwrap()
        .insert("/tree/data".into(), vec![7; 9]);
    let destination = dir.path().join("download");
    let plan = service
        .plan_directory_transfer(
            &id,
            request(FileTransferDirection::Download, &destination),
            DirectoryWalkLimits::default(),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(plan.total_bytes, "9");
    assert_eq!(plan.total_files, 1);
    assert_eq!(plan.entries[0].relative_path, "data");
    assert_eq!(plan.entries[1].kind, RemoteFileKind::Directory);
    assert!(
        !destination.exists(),
        "planning must not create even the destination root"
    );
    assert_eq!(server.auth_count(), 1);
}

#[tokio::test]
async fn unknown_sizes_links_and_aggregate_overflow_fail_with_the_relative_path() {
    let (server, dir, service, id) = fixture().await;
    let destination = dir.path().join("download");
    for (mode, size, code) in [
        (0o120777, Some(4), "invalid"),
        (0o100644, None, "integrity"),
    ] {
        server
            .counts
            .files
            .write()
            .unwrap()
            .insert("/tree/bad".into(), vec![]);
        server.counts.file_attributes.write().unwrap().insert(
            "/tree/bad".into(),
            FileAttributes {
                permissions: Some(mode),
                size,
                ..FileAttributes::empty()
            },
        );
        let error = service
            .plan_directory_transfer(
                &id,
                request(FileTransferDirection::Download, &destination),
                DirectoryWalkLimits::default(),
                CancellationToken::new(),
            )
            .await
            .unwrap_err();
        assert_eq!(error.code, code);
        assert!(error.message.contains("bad"), "{}", error.message);
        assert!(!destination.exists());
    }
    server
        .counts
        .files
        .write()
        .unwrap()
        .insert("/tree/other".into(), vec![]);
    for (name, size) in [("bad", u64::MAX), ("other", 1)] {
        server.counts.file_attributes.write().unwrap().insert(
            format!("/tree/{name}"),
            FileAttributes {
                permissions: Some(0o100644),
                size: Some(size),
                ..FileAttributes::empty()
            },
        );
    }
    let error = service
        .plan_directory_transfer(
            &id,
            request(FileTransferDirection::Download, &destination),
            DirectoryWalkLimits::default(),
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "limit");
    assert!(!destination.exists());
}

#[tokio::test]
async fn upload_limits_and_precancel_do_not_return_partial_plans_or_authenticate() {
    let (server, dir, service, id) = fixture().await;
    let source = dir.path().join("source");
    std::fs::create_dir_all(source.join("nested/deep")).unwrap();
    std::fs::write(source.join("file"), b"x").unwrap();
    for limits in [
        DirectoryWalkLimits {
            max_entries: 1,
            max_depth: 64,
        },
        DirectoryWalkLimits {
            max_entries: 100,
            max_depth: 0,
        },
    ] {
        let error = service
            .plan_directory_transfer(
                &id,
                request(FileTransferDirection::Upload, &source),
                limits,
                CancellationToken::new(),
            )
            .await
            .unwrap_err();
        assert_eq!(error.code, "limit");
    }
    let cancel = CancellationToken::new();
    cancel.cancel();
    let error = service
        .plan_directory_transfer(
            &id,
            request(FileTransferDirection::Upload, &source),
            DirectoryWalkLimits::default(),
            cancel,
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "cancelled");
    assert_eq!(server.auth_count(), 0);
}

#[cfg(windows)]
#[tokio::test]
async fn windows_download_rejects_reserved_names_streams_and_case_collisions() {
    let (server, dir, service, id) = fixture().await;
    let destination = dir.path().join("download");
    for name in [
        "NUL.txt",
        "con",
        "COM¹.log",
        "name.",
        "name ",
        "x:secret",
        "a\\b",
        "a?b",
        "a\u{1}b",
    ] {
        server.counts.files.write().unwrap().clear();
        server
            .counts
            .files
            .write()
            .unwrap()
            .insert(format!("/tree/{name}"), vec![]);
        let error = service
            .plan_directory_transfer(
                &id,
                request(FileTransferDirection::Download, &destination),
                DirectoryWalkLimits::default(),
                CancellationToken::new(),
            )
            .await
            .unwrap_err();
        assert_eq!(error.code, "invalid", "{name}");
        assert!(!destination.exists());
    }
    for names in [["DATA", "data"], ["Σ", "σ"], ["File", "file/inside"]] {
        server.counts.files.write().unwrap().clear();
        server
            .counts
            .directories
            .write()
            .unwrap()
            .retain(|p| p == "/tree");
        for name in names {
            server
                .counts
                .files
                .write()
                .unwrap()
                .insert(format!("/tree/{name}"), vec![]);
            if name.contains('/') {
                server
                    .counts
                    .directories
                    .write()
                    .unwrap()
                    .insert("/tree/file".into());
            }
        }
        let error = service
            .plan_directory_transfer(
                &id,
                request(FileTransferDirection::Download, &destination),
                DirectoryWalkLimits::default(),
                CancellationToken::new(),
            )
            .await
            .unwrap_err();
        assert_eq!(error.code, "invalid");
        assert!(!destination.exists());
    }
}

#[cfg(windows)]
#[tokio::test]
async fn local_source_and_download_target_junctions_are_rejected_without_touching_the_target() {
    let (server, dir, service, id) = fixture().await;
    let outside = dir.path().join("outside");
    std::fs::create_dir(&outside).unwrap();
    std::fs::write(outside.join("sentinel"), b"unchanged").unwrap();
    let junction = dir.path().join("junction");
    let output = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(&junction)
        .arg(&outside)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    for direction in [
        FileTransferDirection::Upload,
        FileTransferDirection::Download,
    ] {
        let error = service
            .plan_directory_transfer(
                &id,
                request(direction, &junction),
                DirectoryWalkLimits::default(),
                CancellationToken::new(),
            )
            .await
            .unwrap_err();
        assert_eq!(error.code, "invalid");
    }
    assert_eq!(
        std::fs::read(outside.join("sentinel")).unwrap(),
        b"unchanged"
    );
    assert_eq!(server.auth_count(), 0);
    std::fs::remove_dir(&junction).unwrap();
}

#[cfg(windows)]
#[tokio::test]
async fn selected_download_root_cannot_use_a_windows_alias_or_device_name() {
    let (_server, dir, service, id) = fixture().await;
    for name in ["new.", "new ", "NUL.txt", "COM¹.log", "x:stream"] {
        let error = service
            .plan_directory_transfer(
                &id,
                request(FileTransferDirection::Download, &dir.path().join(name)),
                DirectoryWalkLimits::default(),
                CancellationToken::new(),
            )
            .await
            .unwrap_err();
        assert_eq!(error.code, "invalid", "{name}");
    }
}

#[tokio::test]
async fn cancelled_download_planning_closes_the_walk_without_creating_a_destination() {
    let (server, dir, service, id) = fixture().await;
    let gate = Arc::new(tokio::sync::Notify::new());
    *server.counts.directory_read_gate.write().unwrap() = Some(gate.clone());
    let destination = dir.path().join("download");
    let cancel = CancellationToken::new();
    let worker_cancel = cancel.clone();
    let work_service = service.clone();
    let work_request = request(FileTransferDirection::Download, &destination);
    let work_id = id.clone();
    let worker = tokio::spawn(async move {
        work_service
            .plan_directory_transfer(
                &work_id,
                work_request,
                DirectoryWalkLimits::default(),
                worker_cancel,
            )
            .await
    });
    tokio::time::timeout(
        std::time::Duration::from_secs(2),
        server.counts.directory_read_started.notified(),
    )
    .await
    .unwrap();
    cancel.cancel();
    gate.notify_one();
    let error = tokio::time::timeout(std::time::Duration::from_secs(2), worker)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert_eq!(error.code, "cancelled");
    assert!(!destination.exists());
    assert_eq!(
        server
            .counts
            .directory_closes
            .load(std::sync::atomic::Ordering::SeqCst),
        1
    );
    *server.counts.directory_read_gate.write().unwrap() = None;
    let handle = service.connect(&id).await.unwrap();
    service.exec(&handle, "printf reused").await.unwrap();
    assert_eq!(server.auth_count(), 1);
}

#[tokio::test]
async fn upload_plan_checks_joined_remote_path_before_any_connection_or_write() {
    let (server, dir, service, id) = fixture().await;
    let source = dir.path().join("source");
    std::fs::create_dir(&source).unwrap();
    std::fs::write(source.join("file"), b"data").unwrap();
    let mut input = request(FileTransferDirection::Upload, &source);
    input.remote_path = format!("/{}", "x".repeat(16 * 1024 - 1));
    let error = service
        .plan_directory_transfer(
            &id,
            input,
            DirectoryWalkLimits::default(),
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "invalid");
    assert!(error.message.contains("file"));
    assert_eq!(server.auth_count(), 0);
    assert!(server.counts.files.read().unwrap().is_empty());
}
