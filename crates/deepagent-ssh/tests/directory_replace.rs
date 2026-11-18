mod support;

use deepagent_ssh::{
    DirectoryConflictPolicy, DirectoryWalkLimits, FileConflictPolicy, FileTransferDirection,
    FileTransferRequest, SshService,
};
use std::{path::Path, sync::Arc};
use tokio_util::sync::CancellationToken;

fn replace() -> DirectoryConflictPolicy {
    serde_json::from_str("\"replace\"").expect("directory replace must be a distinct wire policy")
}

#[test]
fn directory_replace_is_a_distinct_explicit_policy() {
    assert_ne!(replace(), DirectoryConflictPolicy::Merge);
    assert_eq!(serde_json::to_value(replace()).unwrap(), "replace");
}

#[tokio::test]
async fn unconfirmed_stage_creation_reports_the_actual_path_without_claiming_cleanup() {
    let (server, dir, ssh, id) = fixture().await;
    let local = dir.path().join("local");
    old_target(&server, &local);
    let release = Arc::new(tokio::sync::Notify::new());
    *server.counts.directory_create_gate.write().unwrap() = Some(release.clone());
    let running = {
        let ssh = ssh.clone();
        let id = id.clone();
        tokio::spawn(async move {
            ssh.transfer_directory(
                &id,
                request(FileTransferDirection::Upload, &local),
                replace(),
                DirectoryWalkLimits::default(),
                CancellationToken::new(),
                Arc::new(|_| {}),
            )
            .await
        })
    };
    tokio::time::timeout(
        std::time::Duration::from_secs(3),
        server.counts.directory_create_started.notified(),
    )
    .await
    .unwrap();
    assert_eq!(server.auth_count(), 1);
    let stage = server
        .counts
        .directories
        .read()
        .unwrap()
        .iter()
        .find(|path| path.ends_with(".directory-stage"))
        .unwrap()
        .clone();
    disconnect_and_advance_request_deadline(&ssh, &id).await;
    release.notify_one();
    let error = tokio::time::timeout(std::time::Duration::from_secs(3), running)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert_eq!(error.destination_path.as_deref(), Some("/tree"));
    assert!(
        error
            .cleanup_warnings
            .iter()
            .any(|warning| warning.contains(&stage)),
        "{error:?}"
    );
    assert!(server.counts.directories.read().unwrap().contains(&stage));
    assert!(server.counts.directory_renames.read().unwrap().is_empty());
    assert_eq!(
        server.counts.files.read().unwrap()["/tree/data"],
        b"remote-original"
    );
}

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
    let ssh = Arc::new(SshService::new(dir.path().to_owned()));
    let id = server.create(&ssh, Some(&trust)).await.id;
    (server, dir, ssh, id)
}

async fn disconnect_and_advance_request_deadline(ssh: &SshService, id: &str) {
    ssh.disconnect(id).await.unwrap();
    // russh-sftp 2.1.1 retains an unanswered request until its existing 10s deadline.
    // Called after real request delivery; virtual time is not a latency claim.
    tokio::time::pause();
    tokio::time::advance(std::time::Duration::from_secs(10)).await;
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
async fn fixture_directory_rename_moves_descendants_and_remove_requires_empty_directories() {
    let (server, _dir, ssh, id) = fixture().await;
    server.counts.directories.write().unwrap().extend([
        "/tree".into(),
        "/tree/nested".into(),
        "/occupied".into(),
    ]);
    server.counts.files.write().unwrap().extend([
        ("/tree/nested/data".into(), b"original".to_vec()),
        ("/unrelated".into(), b"keep".to_vec()),
    ]);
    let handle = ssh.connect(&id).await.unwrap();
    assert!(ssh
        .rename_path(&handle, "/tree", "/occupied")
        .await
        .is_err());
    ssh.rename_path(&handle, "/tree", "/moved").await.unwrap();
    assert!(!server.counts.directories.read().unwrap().contains("/tree"));
    assert!(server
        .counts
        .directories
        .read()
        .unwrap()
        .contains("/moved/nested"));
    assert_eq!(
        server.counts.files.read().unwrap()["/moved/nested/data"],
        b"original"
    );
    ssh.remove_path(&handle, "/moved").await.unwrap();
    assert!(!server.counts.directories.read().unwrap().contains("/moved"));
    assert!(!server
        .counts
        .files
        .read()
        .unwrap()
        .contains_key("/moved/nested/data"));
    assert_eq!(server.counts.files.read().unwrap()["/unrelated"], b"keep");
}

#[tokio::test]
async fn whole_directory_replace_removes_old_only_entries_in_both_directions() {
    for direction in [
        FileTransferDirection::Upload,
        FileTransferDirection::Download,
    ] {
        let (server, dir, ssh, id) = fixture().await;
        let local = dir.path().join("local");
        std::fs::create_dir_all(local.join("empty")).unwrap();
        std::fs::write(local.join("data"), b"local-content").unwrap();
        server
            .counts
            .directories
            .write()
            .unwrap()
            .extend(["/tree".into(), "/tree/remote-empty".into()]);
        server
            .counts
            .files
            .write()
            .unwrap()
            .insert("/tree/data".into(), b"remote-content".to_vec());
        if direction == FileTransferDirection::Upload {
            server
                .counts
                .files
                .write()
                .unwrap()
                .insert("/tree/old-only".into(), b"old".to_vec());
        } else {
            std::fs::write(local.join("old-only"), b"old").unwrap();
        }
        let receipt = ssh
            .transfer_directory(
                &id,
                request(direction, &local),
                replace(),
                DirectoryWalkLimits::default(),
                CancellationToken::new(),
                Arc::new(|_| {}),
            )
            .await
            .expect("replace must stage and publish the whole selected directory");
        let value = serde_json::to_value(receipt).unwrap();
        assert_eq!(value["publishStrategy"], "directory_backup_replace");
        assert_eq!(value["completedFiles"], 1);
        assert_eq!(value["skippedFiles"], 0);
        assert_eq!(value["cleanupWarnings"], serde_json::json!([]));
        let files = server.counts.files.read().unwrap();
        if direction == FileTransferDirection::Upload {
            assert_eq!(files["/tree/data"], b"local-content");
            assert!(!files.contains_key("/tree/old-only"));
            let directories = server.counts.directories.read().unwrap();
            assert!(directories.contains("/tree/empty"));
            assert!(!directories.contains("/tree/remote-empty"));
        } else {
            assert_eq!(
                std::fs::read(local.join("data")).unwrap(),
                b"remote-content"
            );
            assert!(!local.join("old-only").exists());
            assert!(local.join("remote-empty").is_dir());
            assert!(!local.join("empty").exists());
            assert_eq!(files["/tree/data"], b"remote-content");
        }
        assert!(files.keys().all(|path| !path.contains(".deepagent-")));
        assert!(server
            .counts
            .directories
            .read()
            .unwrap()
            .iter()
            .all(|path| !path.contains(".deepagent-")));
        assert!(std::fs::read_dir(dir.path()).unwrap().all(|entry| !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains(".deepagent-")));
    }
}

fn old_target(server: &support::SshServer, local: &Path) {
    std::fs::create_dir(local).unwrap();
    std::fs::write(local.join("data"), b"local-original").unwrap();
    server
        .counts
        .directories
        .write()
        .unwrap()
        .insert("/tree".into());
    server
        .counts
        .files
        .write()
        .unwrap()
        .insert("/tree/data".into(), b"remote-original".to_vec());
}

#[tokio::test]
async fn empty_directory_replaces_a_nonempty_target_in_both_directions() {
    for direction in [
        FileTransferDirection::Upload,
        FileTransferDirection::Download,
    ] {
        let (server, dir, ssh, id) = fixture().await;
        let local = dir.path().join("local");
        old_target(&server, &local);
        match direction {
            FileTransferDirection::Upload => std::fs::remove_file(local.join("data")).unwrap(),
            FileTransferDirection::Download => {
                server.counts.files.write().unwrap().clear();
            }
        }
        let receipt = ssh
            .transfer_directory(
                &id,
                request(direction, &local),
                replace(),
                DirectoryWalkLimits::default(),
                CancellationToken::new(),
                Arc::new(|_| {}),
            )
            .await
            .unwrap();
        assert_eq!(receipt.completed_files, 0);
        assert!(receipt.cleanup_warnings.is_empty());
        match direction {
            FileTransferDirection::Upload => {
                assert!(server.counts.files.read().unwrap().is_empty())
            }
            FileTransferDirection::Download => {
                assert_eq!(std::fs::read_dir(&local).unwrap().count(), 0)
            }
        }
    }
}

#[tokio::test]
async fn replace_creates_missing_targets_without_exposing_partial_directories() {
    for direction in [
        FileTransferDirection::Upload,
        FileTransferDirection::Download,
    ] {
        let (server, dir, ssh, id) = fixture().await;
        let local = dir.path().join("local");
        old_target(&server, &local);
        match direction {
            FileTransferDirection::Upload => {
                server.counts.directories.write().unwrap().clear();
                server.counts.files.write().unwrap().clear();
            }
            FileTransferDirection::Download => std::fs::remove_dir_all(&local).unwrap(),
        }
        let probe = server.counts.clone();
        let target = local.clone();
        let receipt = ssh
            .transfer_directory(
                &id,
                request(direction, &local),
                replace(),
                DirectoryWalkLimits::default(),
                CancellationToken::new(),
                Arc::new(move |p| {
                    if p.completed_files == 1 && p.current_file.is_some() {
                        match direction {
                            FileTransferDirection::Upload => {
                                assert!(!probe.directories.read().unwrap().contains("/tree"))
                            }
                            FileTransferDirection::Download => assert!(!target.exists()),
                        }
                    }
                }),
            )
            .await
            .unwrap();
        assert_eq!(
            serde_json::to_value(receipt).unwrap()["publishStrategy"],
            "directory_created"
        );
    }
}

#[tokio::test]
async fn cancellation_after_staging_preserves_original_targets_in_both_directions() {
    for direction in [
        FileTransferDirection::Upload,
        FileTransferDirection::Download,
    ] {
        let (server, dir, ssh, id) = fixture().await;
        let local = dir.path().join("local");
        old_target(&server, &local);
        let cancel = CancellationToken::new();
        let stop = cancel.clone();
        let error = ssh
            .transfer_directory(
                &id,
                request(direction, &local),
                replace(),
                DirectoryWalkLimits::default(),
                cancel,
                Arc::new(move |p| {
                    if p.completed_files == 1 {
                        stop.cancel();
                    }
                }),
            )
            .await
            .unwrap_err();
        assert_eq!(error.code, "cancelled");
        assert!(error.cleanup_warnings.is_empty(), "{error:?}");
        assert_eq!(
            std::fs::read(local.join("data")).unwrap(),
            b"local-original"
        );
        assert_eq!(
            server.counts.files.read().unwrap()["/tree/data"],
            b"remote-original"
        );
        assert!(server
            .counts
            .directories
            .read()
            .unwrap()
            .iter()
            .all(|p| !p.contains(".deepagent-")));
        assert!(std::fs::read_dir(dir.path()).unwrap().all(|e| !e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains(".deepagent-")));
    }
}

#[tokio::test]
async fn rejected_publication_restores_original_and_cleans_owned_stage() {
    let (server, dir, ssh, id) = fixture().await;
    let local = dir.path().join("local");
    old_target(&server, &local);
    server
        .counts
        .reject_directory_publish
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let error = ssh
        .transfer_directory(
            &id,
            request(FileTransferDirection::Upload, &local),
            replace(),
            DirectoryWalkLimits::default(),
            CancellationToken::new(),
            Arc::new(|_| {}),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "publish");
    assert_eq!(error.destination_path.as_deref(), Some("/tree"));
    assert_eq!(
        server.counts.files.read().unwrap()["/tree/data"],
        b"remote-original"
    );
    assert_eq!(server.counts.directory_renames.read().unwrap().len(), 3);
    assert!(server
        .counts
        .directories
        .read()
        .unwrap()
        .iter()
        .all(|p| !p.contains(".deepagent-")));
}

#[tokio::test]
async fn failed_restore_preserves_original_backup_and_reports_its_actual_path() {
    let (server, dir, ssh, id) = fixture().await;
    let local = dir.path().join("local");
    old_target(&server, &local);
    server
        .counts
        .reject_directory_publish
        .store(true, std::sync::atomic::Ordering::SeqCst);
    server
        .counts
        .reject_directory_restore
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let error = ssh
        .transfer_directory(
            &id,
            request(FileTransferDirection::Upload, &local),
            replace(),
            DirectoryWalkLimits::default(),
            CancellationToken::new(),
            Arc::new(|_| {}),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "publish");
    let renames = server.counts.directory_renames.read().unwrap();
    let backup = &renames[0].1;
    assert!(error
        .cleanup_warnings
        .iter()
        .any(|warning| warning.contains(backup)));
    assert_eq!(
        server.counts.files.read().unwrap()[&format!("{backup}/data")],
        b"remote-original"
    );
    assert!(!server.counts.directories.read().unwrap().contains("/tree"));
    assert!(!server
        .counts
        .directories
        .read()
        .unwrap()
        .iter()
        .any(|p| p.ends_with(".directory-stage")));
}

#[tokio::test]
async fn published_directory_reports_backup_cleanup_failure_without_claiming_rollback() {
    let (server, dir, ssh, id) = fixture().await;
    let local = dir.path().join("local");
    old_target(&server, &local);
    server
        .counts
        .reject_directory_cleanup
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let receipt = ssh
        .transfer_directory(
            &id,
            request(FileTransferDirection::Upload, &local),
            replace(),
            DirectoryWalkLimits::default(),
            CancellationToken::new(),
            Arc::new(|_| {}),
        )
        .await
        .unwrap();
    let renames = server.counts.directory_renames.read().unwrap();
    let backup = &renames[0].1;
    assert!(receipt
        .cleanup_warnings
        .iter()
        .any(|warning| warning.contains(backup)));
    assert_eq!(
        server.counts.files.read().unwrap()["/tree/data"],
        b"local-original"
    );
    assert_eq!(
        server.counts.files.read().unwrap()[&format!("{backup}/data")],
        b"remote-original"
    );
}

#[tokio::test]
async fn target_changes_during_staging_are_rejected_in_both_directions() {
    for direction in [
        FileTransferDirection::Upload,
        FileTransferDirection::Download,
    ] {
        let (server, dir, ssh, id) = fixture().await;
        let local = dir.path().join("local");
        old_target(&server, &local);
        let target = local.clone();
        let probe = server.counts.clone();
        let error = ssh
            .transfer_directory(
                &id,
                request(direction, &local),
                replace(),
                DirectoryWalkLimits::default(),
                CancellationToken::new(),
                Arc::new(move |p| {
                    if p.completed_files == 1 {
                        match direction {
                            FileTransferDirection::Upload => {
                                probe
                                    .files
                                    .write()
                                    .unwrap()
                                    .insert("/tree/concurrent".into(), b"external".to_vec());
                            }
                            FileTransferDirection::Download => {
                                std::fs::write(target.join("concurrent"), b"external").unwrap()
                            }
                        }
                    }
                }),
            )
            .await
            .unwrap_err();
        assert_eq!(error.code, "conflict", "{error:?}");
        match direction {
            FileTransferDirection::Upload => assert_eq!(
                server.counts.files.read().unwrap()["/tree/concurrent"],
                b"external"
            ),
            FileTransferDirection::Download => assert_eq!(
                std::fs::read(local.join("concurrent")).unwrap(),
                b"external"
            ),
        }
        assert!(error.cleanup_warnings.is_empty(), "{error:?}");
    }
}

#[tokio::test]
async fn cancellation_after_backup_rename_delivery_finishes_the_publication() {
    let (server, dir, ssh, id) = fixture().await;
    let local = dir.path().join("local");
    old_target(&server, &local);
    let release = Arc::new(tokio::sync::Notify::new());
    *server.counts.directory_rename_gate.write().unwrap() = Some(release.clone());
    let cancel = CancellationToken::new();
    let running = {
        let ssh = ssh.clone();
        let cancel = cancel.clone();
        tokio::spawn(async move {
            ssh.transfer_directory(
                &id,
                request(FileTransferDirection::Upload, &local),
                replace(),
                DirectoryWalkLimits::default(),
                cancel,
                Arc::new(|_| {}),
            )
            .await
        })
    };
    tokio::time::timeout(
        std::time::Duration::from_secs(3),
        server.counts.directory_rename_started.notified(),
    )
    .await
    .unwrap();
    cancel.cancel();
    release.notify_one();
    let receipt = running.await.unwrap().unwrap();
    assert!(receipt.cleanup_warnings.is_empty());
    assert_eq!(
        server.counts.files.read().unwrap()["/tree/data"],
        b"local-original"
    );
}

#[tokio::test]
async fn replacing_remote_filesystem_root_is_rejected_without_mutation() {
    let (server, dir, ssh, id) = fixture().await;
    let local = dir.path().join("local");
    std::fs::create_dir(&local).unwrap();
    let mut transfer = request(FileTransferDirection::Upload, &local);
    transfer.remote_path = "/".into();
    let error = ssh
        .transfer_directory(
            &id,
            transfer,
            replace(),
            DirectoryWalkLimits::default(),
            CancellationToken::new(),
            Arc::new(|_| {}),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "invalid");
    assert!(server.counts.directory_renames.read().unwrap().is_empty());
    assert!(server.counts.directories.read().unwrap().is_empty());
}

#[tokio::test]
async fn changed_target_type_is_rejected_after_staging_without_touching_external_data() {
    for link in [false, true] {
        let (server, dir, ssh, id) = fixture().await;
        let local = dir.path().join("local");
        old_target(&server, &local);
        let probe = server.counts.clone();
        let error = ssh
            .transfer_directory(
                &id,
                request(FileTransferDirection::Upload, &local),
                replace(),
                DirectoryWalkLimits::default(),
                CancellationToken::new(),
                Arc::new(move |p| {
                    if p.completed_files == 1 {
                        probe.directories.write().unwrap().remove("/tree");
                        probe
                            .files
                            .write()
                            .unwrap()
                            .insert("/tree".into(), b"external-root".to_vec());
                        if link {
                            probe.file_attributes.write().unwrap().insert(
                                "/tree".into(),
                                russh_sftp::protocol::FileAttributes {
                                    permissions: Some(0o120777),
                                    ..Default::default()
                                },
                            );
                        }
                    }
                }),
            )
            .await
            .unwrap_err();
        assert_eq!(error.code, "invalid");
        assert_eq!(
            server.counts.files.read().unwrap()["/tree"],
            b"external-root"
        );
        assert!(server.counts.directory_renames.read().unwrap().is_empty());
        assert!(error.cleanup_warnings.is_empty(), "{error:?}");
    }
}

#[tokio::test]
async fn remote_backup_cleanup_unlinks_nested_links_without_traversing_their_targets() {
    let (server, dir, ssh, id) = fixture().await;
    let local = dir.path().join("local");
    old_target(&server, &local);
    server
        .counts
        .directories
        .write()
        .unwrap()
        .insert("/outside".into());
    server.counts.files.write().unwrap().extend([
        ("/tree/link".into(), b"/outside".to_vec()),
        ("/outside/sentinel".into(), b"untouched".to_vec()),
    ]);
    server.counts.file_attributes.write().unwrap().insert(
        "/tree/link".into(),
        russh_sftp::protocol::FileAttributes {
            permissions: Some(0o120777),
            ..Default::default()
        },
    );
    let receipt = ssh
        .transfer_directory(
            &id,
            request(FileTransferDirection::Upload, &local),
            replace(),
            DirectoryWalkLimits::default(),
            CancellationToken::new(),
            Arc::new(|_| {}),
        )
        .await
        .unwrap();
    assert!(
        receipt.cleanup_warnings.is_empty(),
        "{:?}",
        receipt.cleanup_warnings
    );
    assert_eq!(
        server.counts.files.read().unwrap()["/outside/sentinel"],
        b"untouched"
    );
    assert!(!server
        .counts
        .files
        .read()
        .unwrap()
        .contains_key("/tree/link"));
}

#[cfg(windows)]
#[tokio::test]
async fn local_backup_cleanup_removes_a_junction_without_deleting_its_target() {
    let (server, dir, ssh, id) = fixture().await;
    let local = dir.path().join("local");
    old_target(&server, &local);
    let outside = dir.path().join("outside");
    std::fs::create_dir(&outside).unwrap();
    std::fs::write(outside.join("sentinel"), b"untouched").unwrap();
    let output = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(local.join("junction"))
        .arg(&outside)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let receipt = ssh
        .transfer_directory(
            &id,
            request(FileTransferDirection::Download, &local),
            replace(),
            DirectoryWalkLimits::default(),
            CancellationToken::new(),
            Arc::new(|_| {}),
        )
        .await
        .unwrap();
    assert!(
        receipt.cleanup_warnings.is_empty(),
        "{:?}",
        receipt.cleanup_warnings
    );
    assert_eq!(
        std::fs::read(outside.join("sentinel")).unwrap(),
        b"untouched"
    );
    assert!(!local.join("junction").exists());
    assert_eq!(
        std::fs::read(local.join("data")).unwrap(),
        b"remote-original"
    );
}

#[tokio::test]
async fn unconfirmed_backup_rename_preserves_staging_and_recovery_paths() {
    let (server, dir, ssh, id) = fixture().await;
    let local = dir.path().join("local");
    old_target(&server, &local);
    let release = Arc::new(tokio::sync::Notify::new());
    *server.counts.directory_rename_gate.write().unwrap() = Some(release.clone());
    let running = {
        let ssh = ssh.clone();
        let id = id.clone();
        tokio::spawn(async move {
            ssh.transfer_directory(
                &id,
                request(FileTransferDirection::Upload, &local),
                replace(),
                DirectoryWalkLimits::default(),
                CancellationToken::new(),
                Arc::new(|_| {}),
            )
            .await
        })
    };
    tokio::time::timeout(
        std::time::Duration::from_secs(3),
        server.counts.directory_rename_started.notified(),
    )
    .await
    .unwrap();
    disconnect_and_advance_request_deadline(&ssh, &id).await;
    release.notify_one();
    let error = tokio::time::timeout(std::time::Duration::from_secs(3), running)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert_eq!(error.code, "publish", "{error:?}");
    let backup = server.counts.directory_renames.read().unwrap()[0].1.clone();
    let stage = server
        .counts
        .directories
        .read()
        .unwrap()
        .iter()
        .find(|p| p.ends_with(".directory-stage"))
        .unwrap()
        .clone();
    assert!(
        error
            .cleanup_warnings
            .iter()
            .any(|w| w.contains(&backup) && w.contains(&stage)),
        "{error:?}"
    );
    let files = server.counts.files.read().unwrap();
    assert!(
        files
            .get("/tree/data")
            .is_some_and(|v| v == b"remote-original")
            || files
                .get(&format!("{backup}/data"))
                .is_some_and(|v| v == b"remote-original")
    );
    assert_eq!(files[&format!("{stage}/data")], b"local-original");
}
