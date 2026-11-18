mod support;

use deepagent_ssh::{
    DirectoryConflictPolicy, DirectoryTransferProgress, DirectoryWalkLimits, FileConflictPolicy,
    FileTransferDirection, FileTransferRequest, SshService,
};
use std::{
    path::Path,
    sync::{Arc, Mutex},
};
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
    let ssh = Arc::new(SshService::new(dir.path().to_owned()));
    let id = server.create(&ssh, Some(&trust)).await.id;
    (server, dir, ssh, id)
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
async fn directory_roundtrip_preserves_empty_unicode_and_zero_files_with_exact_aggregate_progress()
{
    let (server, dir, ssh, id) = fixture().await;
    let input = dir.path().join("input");
    std::fs::create_dir_all(input.join("nested/空目录")).unwrap();
    let bytes: Vec<_> = (0..140_003).map(|i| (i % 251) as u8).collect();
    std::fs::write(input.join("nested/中文.bin"), &bytes).unwrap();
    std::fs::write(input.join("zero"), []).unwrap();
    let events = Arc::new(Mutex::new(Vec::new()));
    let observed = events.clone();
    let upload = ssh
        .transfer_directory(
            &id,
            request(FileTransferDirection::Upload, &input),
            DirectoryConflictPolicy::Ask,
            DirectoryWalkLimits::default(),
            CancellationToken::new(),
            Arc::new(move |p| observed.lock().unwrap().push(p)),
        )
        .await
        .unwrap();
    assert_eq!(upload.bytes, "140003");
    assert_eq!(upload.completed_files, 2);
    assert_eq!(upload.skipped_files, 0);
    assert!(server
        .counts
        .directories
        .read()
        .unwrap()
        .contains("/tree/nested/空目录"));
    {
        let events = events.lock().unwrap();
        assert!(events.windows(2).all(|p| p[0].bytes <= p[1].bytes));
        assert!(events
            .iter()
            .all(|p| p.total == 140_003 && p.total_files == 2));
        assert_eq!(events.last().unwrap().completed_files, 2);
        assert_eq!(events.last().unwrap().bytes, 140_003);
    }
    let output = dir.path().join("output");
    let download = ssh
        .transfer_directory(
            &id,
            request(FileTransferDirection::Download, &output),
            DirectoryConflictPolicy::Ask,
            DirectoryWalkLimits::default(),
            CancellationToken::new(),
            Arc::new(|_| {}),
        )
        .await
        .unwrap();
    assert_eq!(download.completed_files, 2);
    assert_eq!(
        std::fs::read(output.join("nested/中文.bin")).unwrap(),
        bytes
    );
    assert!(output.join("nested/空目录").is_dir());
    assert_eq!(std::fs::metadata(output.join("zero")).unwrap().len(), 0);
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
}

#[tokio::test]
async fn directory_conflicts_require_explicit_merge_and_file_policy_remains_independent() {
    let (server, dir, ssh, id) = fixture().await;
    let input = dir.path().join("input");
    std::fs::create_dir(&input).unwrap();
    std::fs::write(input.join("data"), b"new").unwrap();
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
        .insert("/tree/data".into(), b"old".to_vec());
    let mut req = request(FileTransferDirection::Upload, &input);
    req.conflict = FileConflictPolicy::Replace;
    let error = ssh
        .transfer_directory(
            &id,
            req.clone(),
            DirectoryConflictPolicy::Ask,
            DirectoryWalkLimits::default(),
            CancellationToken::new(),
            Arc::new(|_| {}),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "conflict");
    assert_eq!(server.counts.files.read().unwrap()["/tree/data"], b"old");
    let skipped = ssh
        .transfer_directory(
            &id,
            req.clone(),
            DirectoryConflictPolicy::Skip,
            DirectoryWalkLimits::default(),
            CancellationToken::new(),
            Arc::new(|_| {}),
        )
        .await
        .unwrap();
    assert_eq!(skipped.completed_files, 0);
    assert_eq!(skipped.skipped_files, 1);
    assert_eq!(skipped.bytes, "0");
    let merged = ssh
        .transfer_directory(
            &id,
            req.clone(),
            DirectoryConflictPolicy::Merge,
            DirectoryWalkLimits::default(),
            CancellationToken::new(),
            Arc::new(|_| {}),
        )
        .await
        .unwrap();
    assert_eq!(merged.completed_files, 1);
    assert_eq!(server.counts.files.read().unwrap()["/tree/data"], b"new");
    req.conflict = FileConflictPolicy::Skip;
    let skip_file = ssh
        .transfer_directory(
            &id,
            req,
            DirectoryConflictPolicy::Merge,
            DirectoryWalkLimits::default(),
            CancellationToken::new(),
            Arc::new(|_| {}),
        )
        .await
        .unwrap();
    assert_eq!(skip_file.completed_files, 0);
    assert_eq!(skip_file.skipped_files, 1);
}

#[tokio::test]
async fn cancellation_keeps_published_files_and_cleans_the_active_single_file_stage() {
    let (server, dir, ssh, id) = fixture().await;
    let input = dir.path().join("input");
    std::fs::create_dir(&input).unwrap();
    std::fs::write(input.join("a"), b"first").unwrap();
    std::fs::write(input.join("b"), vec![4; 160_001]).unwrap();
    let cancel = CancellationToken::new();
    let stop = cancel.clone();
    let latest = Arc::new(Mutex::new(None));
    let observed = latest.clone();
    let error = ssh
        .transfer_directory(
            &id,
            request(FileTransferDirection::Upload, &input),
            DirectoryConflictPolicy::Ask,
            DirectoryWalkLimits::default(),
            cancel,
            Arc::new(move |p: DirectoryTransferProgress| {
                if p.completed_files == 1 && p.bytes > 5 {
                    stop.cancel();
                }
                *observed.lock().unwrap() = Some(p);
            }),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "cancelled");
    assert_eq!(error.relative_path.as_deref(), Some("b"));
    assert_eq!(latest.lock().unwrap().as_ref().unwrap().completed_files, 1);
    let files = server.counts.files.read().unwrap();
    assert_eq!(files["/tree/a"], b"first");
    assert!(!files.contains_key("/tree/b"));
    assert!(files.keys().all(|p| !p.contains(".deepagent-")));
}

#[tokio::test]
async fn source_size_is_rechecked_after_planning_before_copying_each_file() {
    let (server, dir, ssh, id) = fixture().await;
    let input = dir.path().join("input");
    std::fs::create_dir(&input).unwrap();
    std::fs::write(input.join("data"), b"old").unwrap();
    let mutate = input.join("data");
    let once = std::sync::atomic::AtomicBool::new(false);
    let error = ssh
        .transfer_directory(
            &id,
            request(FileTransferDirection::Upload, &input),
            DirectoryConflictPolicy::Ask,
            DirectoryWalkLimits::default(),
            CancellationToken::new(),
            Arc::new(move |_| {
                if !once.swap(true, std::sync::atomic::Ordering::SeqCst) {
                    std::fs::write(&mutate, b"changed after planning").unwrap();
                }
            }),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "integrity");
    assert_eq!(error.relative_path.as_deref(), Some("data"));
    assert!(!server
        .counts
        .files
        .read()
        .unwrap()
        .contains_key("/tree/data"));
}

#[tokio::test]
async fn file_failure_reports_the_relative_path_without_hiding_partial_publication() {
    let (server, dir, ssh, id) = fixture().await;
    let input = dir.path().join("input");
    std::fs::create_dir(&input).unwrap();
    std::fs::write(input.join("a"), b"first").unwrap();
    std::fs::write(input.join("b"), b"second").unwrap();
    server
        .counts
        .reject_publish_paths
        .write()
        .unwrap()
        .insert("/tree/b".into());
    let latest = Arc::new(Mutex::new(None));
    let observed = latest.clone();
    let error = ssh
        .transfer_directory(
            &id,
            request(FileTransferDirection::Upload, &input),
            DirectoryConflictPolicy::Ask,
            DirectoryWalkLimits::default(),
            CancellationToken::new(),
            Arc::new(move |p| *observed.lock().unwrap() = Some(p)),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "publish");
    assert_eq!(error.relative_path.as_deref(), Some("b"));
    assert_eq!(latest.lock().unwrap().as_ref().unwrap().completed_files, 1);
    assert_eq!(server.counts.files.read().unwrap()["/tree/a"], b"first");
    assert!(!server.counts.files.read().unwrap().contains_key("/tree/b"));
    assert!(server
        .counts
        .files
        .read()
        .unwrap()
        .keys()
        .all(|p| !p.contains(".part")));
}

#[tokio::test]
async fn empty_directory_and_precancel_have_deterministic_results() {
    let (server, dir, ssh, id) = fixture().await;
    let input = dir.path().join("empty");
    std::fs::create_dir(&input).unwrap();
    let cancel = CancellationToken::new();
    cancel.cancel();
    let error = ssh
        .transfer_directory(
            &id,
            request(FileTransferDirection::Upload, &input),
            DirectoryConflictPolicy::Ask,
            DirectoryWalkLimits::default(),
            cancel,
            Arc::new(|_| {}),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "cancelled");
    assert_eq!(server.auth_count(), 0);
    let done = ssh
        .transfer_directory(
            &id,
            request(FileTransferDirection::Upload, &input),
            DirectoryConflictPolicy::Ask,
            DirectoryWalkLimits::default(),
            CancellationToken::new(),
            Arc::new(|_| {}),
        )
        .await
        .unwrap();
    assert_eq!(done.total_files, 0);
    assert_eq!(done.bytes, "0");
    assert!(server.counts.directories.read().unwrap().contains("/tree"));
}

#[tokio::test]
async fn download_rechecks_remote_size_after_the_plan_and_cannot_mark_stale_totals_complete() {
    let (server, dir, ssh, id) = fixture().await;
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
        .insert("/tree/data".into(), b"old".to_vec());
    let counts = server.counts.clone();
    let output = dir.path().join("output");
    let error = ssh
        .transfer_directory(
            &id,
            request(FileTransferDirection::Download, &output),
            DirectoryConflictPolicy::Ask,
            DirectoryWalkLimits::default(),
            CancellationToken::new(),
            Arc::new(move |_| {
                counts
                    .files
                    .write()
                    .unwrap()
                    .insert("/tree/data".into(), b"changed after planning".to_vec());
            }),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "integrity");
    assert_eq!(error.relative_path.as_deref(), Some("data"));
    assert!(!output.join("data").exists());
}

#[cfg(windows)]
#[tokio::test]
async fn download_rejects_a_junction_inserted_after_planning_without_touching_outside_files() {
    let (server, dir, ssh, id) = fixture().await;
    server
        .counts
        .directories
        .write()
        .unwrap()
        .extend(["/tree".into(), "/tree/nested".into()]);
    server
        .counts
        .files
        .write()
        .unwrap()
        .insert("/tree/nested/sentinel".into(), b"new".to_vec());
    let outside = dir.path().join("outside");
    std::fs::create_dir(&outside).unwrap();
    std::fs::write(outside.join("sentinel"), b"unchanged").unwrap();
    let output = dir.path().join("output");
    let junction = output.join("nested");
    let insert = junction.clone();
    let target = outside.clone();
    let once = std::sync::atomic::AtomicBool::new(false);
    let error = ssh
        .transfer_directory(
            &id,
            request(FileTransferDirection::Download, &output),
            DirectoryConflictPolicy::Ask,
            DirectoryWalkLimits::default(),
            CancellationToken::new(),
            Arc::new(move |p| {
                if p.current_file.as_deref() == Some("nested")
                    && !once.swap(true, std::sync::atomic::Ordering::SeqCst)
                {
                    assert!(std::process::Command::new("cmd")
                        .args(["/C", "mklink", "/J"])
                        .arg(&insert)
                        .arg(&target)
                        .output()
                        .unwrap()
                        .status
                        .success());
                }
            }),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "invalid");
    assert_eq!(error.relative_path.as_deref(), Some("nested"));
    assert_eq!(
        std::fs::read(outside.join("sentinel")).unwrap(),
        b"unchanged"
    );
    std::fs::remove_dir(junction).unwrap();
}
