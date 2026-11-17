mod support;

use deepagent_ssh::{DirectoryWalkLimits, RemoteFileKind, SshService};
use russh_sftp::protocol::File;
use std::sync::{atomic::Ordering, Arc};
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
    let service = Arc::new(SshService::new(dir.path().to_path_buf()));
    let id = server.create(&service, Some(&trust)).await.id;
    (server, dir, service, id)
}

#[tokio::test]
async fn walk_preserves_empty_directories_unicode_and_exact_sizes_without_shell_or_reauth() {
    let (server, _dir, service, id) = fixture().await;
    server
        .counts
        .directories
        .write()
        .unwrap()
        .extend(["/tree", "/tree/空目录", "/tree/nested"].map(String::from));
    server.counts.files.write().unwrap().extend([
        ("/tree/中文.bin".into(), vec![1, 2, 3]),
        ("/tree/nested/data".into(), vec![9; 7]),
    ]);
    let tree = service
        .walk_directory(
            &id,
            "/tree",
            DirectoryWalkLimits::default(),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(tree.root, "/tree");
    assert_eq!(
        tree.entries
            .iter()
            .map(|entry| entry.relative_path.as_str())
            .collect::<Vec<_>>(),
        ["nested", "nested/data", "中文.bin", "空目录"]
    );
    assert_eq!(tree.entries[3].metadata.kind, RemoteFileKind::Directory);
    assert_eq!(tree.entries[1].metadata.size_bytes.as_deref(), Some("7"));
    assert!(tree
        .entries
        .iter()
        .all(|entry| !entry.metadata.followed_symlinks));
    assert_eq!(server.counts.directory_closes.load(Ordering::SeqCst), 3);
    assert_eq!(server.counts.commands.load(Ordering::SeqCst), 0);
    assert_eq!(server.auth_count(), 1);
}

#[tokio::test]
async fn entry_and_depth_limits_fail_explicitly_and_close_handles() {
    let (server, _dir, service, id) = fixture().await;
    server
        .counts
        .directories
        .write()
        .unwrap()
        .extend(["/tree", "/tree/sub"].map(String::from));
    server
        .counts
        .files
        .write()
        .unwrap()
        .extend((0..6).map(|n| (format!("/tree/file{n}"), vec![])));
    let error = service
        .walk_directory(
            &id,
            "/tree",
            DirectoryWalkLimits {
                max_entries: 2,
                max_depth: 4,
            },
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "limit");
    assert_eq!(
        server.counts.directory_reads.load(Ordering::SeqCst),
        2,
        "limit stops before fetching the complete listing"
    );
    assert_eq!(server.counts.directory_closes.load(Ordering::SeqCst), 1);
    let error = service
        .walk_directory(
            &id,
            "/tree",
            DirectoryWalkLimits {
                max_entries: 100,
                max_depth: 0,
            },
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "limit");
    assert_eq!(server.counts.directory_closes.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn unsafe_and_duplicate_remote_names_do_not_escape_the_root_or_return_a_partial_success() {
    let (server, _dir, service, id) = fixture().await;
    server
        .counts
        .directories
        .write()
        .unwrap()
        .insert("/tree".into());
    for names in [
        vec!["../outside"],
        vec!["/absolute"],
        vec!["a/b"],
        vec!["bad\0name"],
        vec!["same", "same"],
    ] {
        server
            .counts
            .directory_entries
            .write()
            .unwrap()
            .insert("/tree".into(), names.into_iter().map(File::dummy).collect());
        server
            .counts
            .files
            .write()
            .unwrap()
            .insert("/tree/same".into(), vec![]);
        let error = service
            .walk_directory(
                &id,
                "/tree",
                DirectoryWalkLimits::default(),
                CancellationToken::new(),
            )
            .await
            .unwrap_err();
        assert_eq!(error.code, "invalid");
    }
    assert_eq!(server.counts.directory_closes.load(Ordering::SeqCst), 5);
}

#[tokio::test]
async fn cancelled_enumeration_returns_promptly_and_the_same_connection_remains_usable() {
    let (server, _dir, service, id) = fixture().await;
    server
        .counts
        .directories
        .write()
        .unwrap()
        .insert("/tree".into());
    let gate = Arc::new(tokio::sync::Notify::new());
    *server.counts.directory_read_gate.write().unwrap() = Some(gate.clone());
    let cancel = CancellationToken::new();
    let task_service = service.clone();
    let task_id = id.clone();
    let task_cancel = cancel.clone();
    let task = tokio::spawn(async move {
        task_service
            .walk_directory(
                &task_id,
                "/tree",
                DirectoryWalkLimits::default(),
                task_cancel,
            )
            .await
    });
    tokio::time::timeout(
        std::time::Duration::from_secs(2),
        server.counts.directory_read_started.notified(),
    )
    .await
    .unwrap();
    let started = std::time::Instant::now();
    cancel.cancel();
    gate.notify_one();
    let error = tokio::time::timeout(std::time::Duration::from_secs(1), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert_eq!(error.code, "cancelled");
    assert!(started.elapsed() < std::time::Duration::from_secs(1));
    assert_eq!(server.counts.directory_closes.load(Ordering::SeqCst), 1);
    *server.counts.directory_read_gate.write().unwrap() = None;
    assert!(service
        .walk_directory(
            &id,
            "/tree",
            DirectoryWalkLimits::default(),
            CancellationToken::new()
        )
        .await
        .unwrap()
        .entries
        .is_empty());
    assert_eq!(server.auth_count(), 1);
}

#[tokio::test]
async fn precancel_and_invalid_roots_do_not_authenticate_or_open_a_channel() {
    let (server, _dir, service, id) = fixture().await;
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert_eq!(
        service
            .walk_directory(&id, "/tree", DirectoryWalkLimits::default(), cancel)
            .await
            .unwrap_err()
            .code,
        "cancelled"
    );
    for root in ["", "relative", "/a/../b", "/bad\0root"] {
        assert_eq!(
            service
                .walk_directory(
                    &id,
                    root,
                    DirectoryWalkLimits::default(),
                    CancellationToken::new()
                )
                .await
                .unwrap_err()
                .code,
            "invalid"
        );
    }
    assert_eq!(server.auth_count(), 0);
}

#[tokio::test]
async fn symlinks_are_reported_without_descent_and_large_sizes_remain_exact() {
    let (server, _dir, service, id) = fixture().await;
    server
        .counts
        .directories
        .write()
        .unwrap()
        .insert("/tree".into());
    server.counts.directory_entries.write().unwrap().insert(
        "/tree".into(),
        vec![File::dummy("link"), File::dummy("huge")],
    );
    server.counts.file_attributes.write().unwrap().extend([
        (
            "/tree/link".into(),
            russh_sftp::protocol::FileAttributes {
                permissions: Some(0o120777),
                ..Default::default()
            },
        ),
        (
            "/tree/huge".into(),
            russh_sftp::protocol::FileAttributes {
                permissions: Some(0o100644),
                size: Some(u64::MAX),
                ..Default::default()
            },
        ),
    ]);
    let tree = service
        .walk_directory(
            &id,
            "/tree",
            DirectoryWalkLimits::default(),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(
        tree.entries[0].metadata.size_bytes.as_deref(),
        Some("18446744073709551615")
    );
    assert_eq!(tree.entries[1].metadata.kind, RemoteFileKind::Symlink);
    assert_eq!(server.counts.directory_closes.load(Ordering::SeqCst), 1);
    assert_eq!(
        service
            .walk_directory(
                &id,
                "/tree/link",
                DirectoryWalkLimits::default(),
                CancellationToken::new()
            )
            .await
            .unwrap_err()
            .code,
        "invalid"
    );
}

#[tokio::test]
async fn changed_directory_identity_is_rejected_before_descending() {
    let (server, _dir, service, id) = fixture().await;
    server
        .counts
        .directories
        .write()
        .unwrap()
        .extend(["/tree", "/tree/sub"].map(String::from));
    server
        .counts
        .canonical_paths
        .write()
        .unwrap()
        .insert("/tree/sub".into(), "/outside".into());
    let error = service
        .walk_directory(
            &id,
            "/tree",
            DirectoryWalkLimits::default(),
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "invalid");
    assert_eq!(server.counts.directory_closes.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn repeated_dot_pages_are_bounded_instead_of_looping_forever() {
    let (server, _dir, service, id) = fixture().await;
    server
        .counts
        .directories
        .write()
        .unwrap()
        .insert("/tree".into());
    server
        .counts
        .directory_entries
        .write()
        .unwrap()
        .insert("/tree".into(), vec![File::dummy("."), File::dummy("..")]);
    server
        .counts
        .repeat_directory_page
        .store(true, Ordering::SeqCst);
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        service.walk_directory(
            &id,
            "/tree",
            DirectoryWalkLimits {
                max_entries: 2,
                max_depth: 1,
            },
            CancellationToken::new(),
        ),
    )
    .await
    .expect("malformed pagination must terminate without user cancellation");
    assert_eq!(result.unwrap_err().code, "limit");
    assert_eq!(server.counts.directory_closes.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn directory_close_failure_is_not_a_successful_complete_listing() {
    let (server, _dir, service, id) = fixture().await;
    server
        .counts
        .directories
        .write()
        .unwrap()
        .insert("/tree".into());
    server
        .counts
        .reject_directory_close
        .store(true, Ordering::SeqCst);
    let error = service
        .walk_directory(
            &id,
            "/tree",
            DirectoryWalkLimits::default(),
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "io");
    assert!(error.message.contains("close directory handle"));
}

#[tokio::test]
async fn cancellation_reports_cleanup_timeout_and_retires_only_the_sftp_channel() {
    let (server, _dir, service, id) = fixture().await;
    server
        .counts
        .directories
        .write()
        .unwrap()
        .insert("/tree".into());
    let gate = Arc::new(tokio::sync::Notify::new());
    *server.counts.directory_read_gate.write().unwrap() = Some(gate.clone());
    let cancel = CancellationToken::new();
    let worker = service.clone();
    let worker_id = id.clone();
    let worker_cancel = cancel.clone();
    let task = tokio::spawn(async move {
        worker
            .walk_directory(
                &worker_id,
                "/tree",
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
    let error = tokio::time::timeout(std::time::Duration::from_secs(1), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert_eq!(error.code, "cancelled");
    assert!(error
        .cleanup_warnings
        .iter()
        .any(|warning| warning.contains("timed out")));
    *server.counts.directory_read_gate.write().unwrap() = None;
    gate.notify_one();
    service
        .walk_directory(
            &id,
            "/tree",
            DirectoryWalkLimits::default(),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(server.auth_count(), 1);
}
