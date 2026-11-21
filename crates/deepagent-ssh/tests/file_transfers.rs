mod support;
use deepagent_ssh::{FileConflictPolicy, FileTransferDirection, FileTransferRequest, SshService};
use std::sync::Arc;
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
    let conn = server.create(&service, Some(&trust)).await;
    (server, dir, service, conn.id)
}

fn keep_both() -> FileConflictPolicy {
    serde_json::from_str("\"keep_both\"").expect("keep-both must be an explicit file policy")
}

#[tokio::test]
async fn keep_both_upload_preserves_collisions_and_reports_the_verified_destination() {
    let (server, dir, service, id) = fixture().await;
    let input = dir.path().join("input");
    std::fs::write(&input, b"new").unwrap();
    server.counts.files.write().unwrap().extend([
        ("/中文.tar.gz".into(), b"original".to_vec()),
        ("/中文.tar (1).gz".into(), b"occupied".to_vec()),
    ]);
    let receipt = service
        .transfer_file(
            &id,
            FileTransferRequest {
                direction: FileTransferDirection::Upload,
                local_path: input.to_str().unwrap().into(),
                remote_path: "/中文.tar.gz".into(),
                conflict: keep_both(),
            },
            CancellationToken::new(),
            Arc::new(|_| {}),
        )
        .await
        .unwrap();
    let json = serde_json::to_value(&receipt).unwrap();
    assert_eq!(json["destinationPath"], "/中文.tar (2).gz");
    assert_eq!(receipt.bytes, "3");
    let files = server.counts.files.read().unwrap();
    assert_eq!(files["/中文.tar.gz"], b"original");
    assert_eq!(files["/中文.tar (1).gz"], b"occupied");
    assert_eq!(files["/中文.tar (2).gz"], b"new");
    assert!(files.keys().all(|p| !p.contains(".part")));
}

#[tokio::test]
async fn keep_both_download_preserves_hidden_names_and_zero_byte_files() {
    let (server, dir, service, id) = fixture().await;
    server
        .counts
        .files
        .write()
        .unwrap()
        .insert("/source".into(), vec![]);
    let output = dir.path().join(".env");
    std::fs::write(&output, b"original").unwrap();
    std::fs::create_dir(dir.path().join(".env (1)")).unwrap();
    let receipt = service
        .transfer_file(
            &id,
            FileTransferRequest {
                direction: FileTransferDirection::Download,
                local_path: output.to_str().unwrap().into(),
                remote_path: "/source".into(),
                conflict: keep_both(),
            },
            CancellationToken::new(),
            Arc::new(|_| {}),
        )
        .await
        .unwrap();
    assert_eq!(std::fs::read(output).unwrap(), b"original");
    assert_eq!(
        std::fs::metadata(dir.path().join(".env (2)"))
            .unwrap()
            .len(),
        0
    );
    assert_eq!(
        serde_json::to_value(receipt).unwrap()["destinationPath"],
        dir.path().join(".env (2)").to_str().unwrap()
    );
}

#[tokio::test]
async fn keep_both_publish_collision_never_overwrites_or_replays_an_uncertain_remote_write() {
    let (server, dir, service, id) = fixture().await;
    let input = dir.path().join("input");
    std::fs::write(&input, b"new").unwrap();
    server
        .counts
        .files
        .write()
        .unwrap()
        .insert("/target".into(), b"original".to_vec());
    let counts = server.counts.clone();
    let error = service
        .transfer_file(
            &id,
            FileTransferRequest {
                direction: FileTransferDirection::Upload,
                local_path: input.to_str().unwrap().into(),
                remote_path: "/target".into(),
                conflict: keep_both(),
            },
            CancellationToken::new(),
            Arc::new(move |_| {
                counts
                    .files
                    .write()
                    .unwrap()
                    .insert("/target (1)".into(), b"racer".to_vec());
            }),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "publish");
    let files = server.counts.files.read().unwrap();
    assert_eq!(files["/target"], b"original");
    assert_eq!(files["/target (1)"], b"racer");
    assert!(!files.contains_key("/target (2)"));
    assert!(files.keys().all(|p| !p.contains(".part")));
}

#[tokio::test]
async fn keep_both_local_publish_race_preserves_the_other_writer_and_cleans_staging() {
    let (server, dir, service, id) = fixture().await;
    server
        .counts
        .files
        .write()
        .unwrap()
        .insert("/source".into(), b"new".to_vec());
    let output = dir.path().join("target.bin");
    std::fs::write(&output, b"original").unwrap();
    let occupied = dir.path().join("target (1).bin");
    let race = occupied.clone();
    let error = service
        .transfer_file(
            &id,
            FileTransferRequest {
                direction: FileTransferDirection::Download,
                local_path: output.to_str().unwrap().into(),
                remote_path: "/source".into(),
                conflict: keep_both(),
            },
            CancellationToken::new(),
            Arc::new(move |_| {
                std::fs::write(&race, b"racer").unwrap();
            }),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "conflict");
    assert_eq!(std::fs::read(output).unwrap(), b"original");
    assert_eq!(std::fs::read(occupied).unwrap(), b"racer");
    assert!(!dir.path().join("target (2).bin").exists());
    assert!(std::fs::read_dir(dir.path()).unwrap().all(|p| !p
        .unwrap()
        .file_name()
        .to_string_lossy()
        .starts_with(".deepagent-")));
}

#[tokio::test]
async fn bidirectional_binary_transfer_stages_verifies_and_reuses_the_authenticated_connection() {
    let (server, dir, service, id) = fixture().await;
    let input = dir.path().join("input 中文.bin");
    let output = dir.path().join("output.bin");
    let bytes: Vec<u8> = (0..240_001).map(|i| (i % 251) as u8).collect();
    std::fs::write(&input, &bytes).unwrap();
    let request = |direction, local: &std::path::Path| FileTransferRequest {
        direction,
        local_path: local.to_string_lossy().into(),
        remote_path: "/remote.bin".into(),
        conflict: FileConflictPolicy::Ask,
    };
    let upload = service
        .transfer_file(
            &id,
            request(FileTransferDirection::Upload, &input),
            CancellationToken::new(),
            Arc::new(|_| {}),
        )
        .await
        .unwrap();
    assert_eq!(upload.bytes, bytes.len().to_string());
    assert_eq!(
        server.counts.files.read().unwrap().get("/remote.bin"),
        Some(&bytes)
    );
    let download = service
        .transfer_file(
            &id,
            request(FileTransferDirection::Download, &output),
            CancellationToken::new(),
            Arc::new(|_| {}),
        )
        .await
        .unwrap();
    assert_eq!(upload.sha256, download.sha256);
    assert_eq!(std::fs::read(&output).unwrap(), bytes);
    assert_eq!(server.auth_count(), 1);
    assert_eq!(
        server
            .counts
            .commands
            .load(std::sync::atomic::Ordering::SeqCst),
        0
    );
    assert!(server
        .counts
        .files
        .read()
        .unwrap()
        .keys()
        .all(|p| !p.contains(".deepagent-")));
    service.disconnect(&id).await.unwrap();
}

#[tokio::test]
async fn conflicts_require_a_decision_and_explicit_replace_preserves_safe_publishing() {
    let (server, dir, service, id) = fixture().await;
    let local = dir.path().join("input");
    std::fs::write(&local, b"new").unwrap();
    server
        .counts
        .files
        .write()
        .unwrap()
        .insert("/existing".into(), b"original".to_vec());
    let mut request = FileTransferRequest {
        direction: FileTransferDirection::Upload,
        local_path: local.to_string_lossy().into(),
        remote_path: "/existing".into(),
        conflict: FileConflictPolicy::Ask,
    };
    let error = service
        .transfer_file(
            &id,
            request.clone(),
            CancellationToken::new(),
            Arc::new(|_| {}),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "conflict");
    assert_eq!(
        server
            .counts
            .files
            .read()
            .unwrap()
            .get("/existing")
            .unwrap(),
        b"original"
    );
    request.conflict = FileConflictPolicy::Skip;
    assert!(
        service
            .transfer_file(
                &id,
                request.clone(),
                CancellationToken::new(),
                Arc::new(|_| {})
            )
            .await
            .unwrap()
            .skipped
    );
    request.conflict = FileConflictPolicy::Replace;
    let receipt = service
        .transfer_file(&id, request, CancellationToken::new(), Arc::new(|_| {}))
        .await
        .unwrap();
    assert_eq!(receipt.publish_strategy, "backup_replace");
    assert_eq!(
        server
            .counts
            .files
            .read()
            .unwrap()
            .get("/existing")
            .unwrap(),
        b"new"
    );
    assert!(server
        .counts
        .files
        .read()
        .unwrap()
        .keys()
        .all(|p| !p.contains(".deepagent-")));
    service.disconnect(&id).await.unwrap();
}

#[tokio::test]
async fn cancellation_after_progress_and_short_reads_never_publish_a_complete_file() {
    let (server, dir, service, id) = fixture().await;
    let local = dir.path().join("large");
    std::fs::write(&local, vec![b'x'; 240_000]).unwrap();
    let token = CancellationToken::new();
    let cancel = token.clone();
    let request = FileTransferRequest {
        direction: FileTransferDirection::Upload,
        local_path: local.to_string_lossy().into(),
        remote_path: "/cancelled".into(),
        conflict: FileConflictPolicy::Ask,
    };
    let error = service
        .transfer_file(
            &id,
            request,
            token,
            Arc::new(move |progress| {
                if progress.bytes > 0 {
                    cancel.cancel();
                }
            }),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "cancelled");
    assert!(!server
        .counts
        .files
        .read()
        .unwrap()
        .contains_key("/cancelled"));
    assert!(server
        .counts
        .files
        .read()
        .unwrap()
        .keys()
        .all(|p| !p.contains(".deepagent-")));
    server
        .counts
        .files
        .write()
        .unwrap()
        .insert("/short".into(), b"partial".to_vec());
    let output = dir.path().join("must-not-exist");
    let request = FileTransferRequest {
        direction: FileTransferDirection::Download,
        local_path: output.to_string_lossy().into(),
        remote_path: "/short".into(),
        conflict: FileConflictPolicy::Ask,
    };
    assert_eq!(
        service
            .transfer_file(&id, request, CancellationToken::new(), Arc::new(|_| {}))
            .await
            .unwrap_err()
            .code,
        "integrity"
    );
    assert!(!output.exists());
    assert!(std::fs::read_dir(dir.path()).unwrap().all(|e| !e
        .unwrap()
        .file_name()
        .to_string_lossy()
        .contains(".deepagent-")));
    service.disconnect(&id).await.unwrap();
}

#[tokio::test]
async fn failed_publication_restores_original_and_download_conflict_never_touches_destination() {
    let (server, dir, service, id) = fixture().await;
    let local = dir.path().join("source");
    std::fs::write(&local, b"new").unwrap();
    server
        .counts
        .files
        .write()
        .unwrap()
        .insert("/reject-publish".into(), b"original".to_vec());
    let request = FileTransferRequest {
        direction: FileTransferDirection::Upload,
        local_path: local.to_string_lossy().into(),
        remote_path: "/reject-publish".into(),
        conflict: FileConflictPolicy::Replace,
    };
    let failure = service
        .transfer_file(&id, request, CancellationToken::new(), Arc::new(|_| {}))
        .await
        .unwrap_err();
    assert_eq!(failure.code, "publish");
    // The fixture rejects stage publication but allows backup rollback.
    assert_eq!(
        server
            .counts
            .files
            .read()
            .unwrap()
            .get("/reject-publish")
            .unwrap(),
        b"original"
    );
    server
        .counts
        .files
        .write()
        .unwrap()
        .insert("/source".into(), b"remote".to_vec());
    let target = dir.path().join("existing");
    std::fs::write(&target, b"local-original").unwrap();
    let request = FileTransferRequest {
        direction: FileTransferDirection::Download,
        local_path: target.to_string_lossy().into(),
        remote_path: "/source".into(),
        conflict: FileConflictPolicy::Ask,
    };
    assert_eq!(
        service
            .transfer_file(&id, request, CancellationToken::new(), Arc::new(|_| {}))
            .await
            .unwrap_err()
            .code,
        "conflict"
    );
    assert_eq!(std::fs::read(target).unwrap(), b"local-original");
    service.disconnect(&id).await.unwrap();
}

#[tokio::test]
async fn same_size_same_mtime_remote_mutation_is_detected_before_local_publication() {
    let (server, dir, service, id) = fixture().await;
    let bytes = vec![b'a'; 240_001];
    server
        .counts
        .files
        .write()
        .unwrap()
        .insert("/mutable".into(), bytes);
    let output = dir.path().join("preserved");
    std::fs::write(&output, b"original local file").unwrap();
    let files = server.counts.files.clone();
    let error = service
        .transfer_file(
            &id,
            FileTransferRequest {
                direction: FileTransferDirection::Download,
                local_path: output.to_string_lossy().into(),
                remote_path: "/mutable".into(),
                conflict: FileConflictPolicy::Replace,
            },
            CancellationToken::new(),
            Arc::new(move |progress| {
                if progress.total == Some(progress.bytes) {
                    // The fixture deliberately reports the same size and mtime.
                    files.write().unwrap().get_mut("/mutable").unwrap()[0] = b'b';
                }
            }),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "integrity");
    assert_eq!(std::fs::read(output).unwrap(), b"original local file");
    assert!(std::fs::read_dir(dir.path()).unwrap().all(|entry| !entry
        .unwrap()
        .file_name()
        .to_string_lossy()
        .starts_with(".deepagent-")));
    service.disconnect(&id).await.unwrap();
}

#[tokio::test]
async fn cancelling_while_remote_open_is_in_flight_waits_for_ownership_and_cleans_stage() {
    let (server, dir, service, id) = fixture().await;
    let input = dir.path().join("input");
    std::fs::write(&input, b"never publish").unwrap();
    let gate = Arc::new(tokio::sync::Notify::new());
    *server.counts.stage_open_gate.write().unwrap() = Some(gate.clone());
    let cancel = CancellationToken::new();
    let future = service.transfer_file(
        &id,
        FileTransferRequest {
            direction: FileTransferDirection::Upload,
            local_path: input.to_string_lossy().into(),
            remote_path: "/cancel-open".into(),
            conflict: FileConflictPolicy::Ask,
        },
        cancel.clone(),
        Arc::new(|_| {}),
    );
    tokio::pin!(future);
    tokio::select! {
        _ = &mut future => panic!("open gate must remain pending"),
        _ = server.counts.stage_created.notified() => {}
    }
    cancel.cancel();
    // Poll the transfer before releasing the server response: it must not abandon ownership.
    let pending = tokio::time::timeout(std::time::Duration::from_millis(20), &mut future).await;
    gate.notify_one();
    assert!(
        pending.is_err(),
        "cancellation must await bounded create acknowledgement"
    );
    assert_eq!(future.await.unwrap_err().code, "cancelled");
    assert!(server
        .counts
        .files
        .read()
        .unwrap()
        .keys()
        .all(|p| !p.contains(".deepagent-")));
    service.disconnect(&id).await.unwrap();
}

#[tokio::test]
async fn download_publish_io_failure_is_not_reported_as_a_file_conflict() {
    let (server, dir, service, id) = fixture().await;
    server
        .counts
        .files
        .write()
        .unwrap()
        .insert("/source".into(), b"content".to_vec());
    let target = dir.path().join("appeared-directory");
    let changed = target.clone();
    let error = service
        .transfer_file(
            &id,
            FileTransferRequest {
                direction: FileTransferDirection::Download,
                local_path: target.to_string_lossy().into(),
                remote_path: "/source".into(),
                conflict: FileConflictPolicy::Replace,
            },
            CancellationToken::new(),
            Arc::new(move |progress| {
                if progress.bytes == 7 {
                    std::fs::create_dir_all(&changed).unwrap();
                }
            }),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "publish");
    assert!(error.message.contains("publish local"));
    assert!(target.is_dir());
    service.disconnect(&id).await.unwrap();
}
