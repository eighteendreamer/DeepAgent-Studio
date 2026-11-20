//! Script-driven response profiling through SshService. Opt in with a pinned
//! endpoint request in DEEPAGENT_SSH_EXTERNAL_REQUEST, never command arguments.
//! Metrics contain operation labels and timings only, without remote output.
use deepagent_ssh::*;
use std::{future::Future, sync::Arc, time::Duration};
use tokio::time::{timeout, Instant};
use tokio_util::sync::CancellationToken;

#[derive(Default)]
struct MemoryStore(std::sync::Mutex<Vec<SshConnectionConfig>>);

#[async_trait::async_trait]
impl SshConfigStore for MemoryStore {
    fn supports_secret_storage(&self) -> bool {
        true
    }
    async fn load(&self) -> SshResult<Vec<SshConnectionConfig>> {
        Ok(self.0.lock().unwrap().clone())
    }
    async fn save(&self, configs: &[SshConnectionConfig]) -> SshResult<()> {
        *self.0.lock().unwrap() = configs.to_vec();
        Ok(())
    }
}

async fn measured<T>(label: &str, work: impl Future<Output = T>) -> T {
    let started = Instant::now();
    let result = timeout(Duration::from_secs(40), work).await;
    println!(
        "RESPONSE_METRIC {}",
        serde_json::json!({"operation":label,"ms":started.elapsed().as_secs_f64()*1000.0,
            "completed":result.is_ok()})
    );
    result.expect("operation exceeded the response measurement deadline")
}

async fn marker(ssh: &SshService, pty: &SshServiceHandle, expected: &[u8]) {
    let mut bytes = Vec::new();
    timeout(Duration::from_secs(10), async {
        loop {
            bytes.extend(ssh.pty_read(pty).await.unwrap());
            assert!(bytes.len() < 1024 * 1024, "bounded fixture output");
            if bytes.windows(expected.len()).any(|part| part == expected) {
                return;
            }
            // This test poll adds up to 2ms plus scheduler delay to marker latency.
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .expect("expected PTY marker; remote contents deliberately omitted");
}

fn transfer(local: &std::path::Path, remote: &str, upload: bool) -> FileTransferRequest {
    FileTransferRequest {
        local_path: local.to_string_lossy().into(),
        remote_path: remote.into(),
        direction: if upload {
            FileTransferDirection::Upload
        } else {
            FileTransferDirection::Download
        },
        conflict: FileConflictPolicy::Replace,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires an authorized real endpoint, pinned host key, and D-drive TEMP"]
async fn real_response_profile() {
    let request: CreateSshConnectionRequest = serde_json::from_str(
        &std::env::var("DEEPAGENT_SSH_EXTERNAL_REQUEST").expect("endpoint in child environment"),
    )
    .expect("valid request");
    assert_eq!(request.host_key_policy, SshHostKeyPolicy::Strict);
    let temp = tempfile::tempdir().unwrap();
    let ssh = Arc::new(SshService::with_config_store(
        temp.path().into(),
        Arc::new(MemoryStore::default()),
    ));
    let connection = measured("config_create", ssh.create_connection(request))
        .await
        .unwrap();
    let id = &connection.id;
    for _ in 0..3 {
        measured("connect_cold", ssh.connect(id)).await.unwrap();
        measured("disconnect_cold", ssh.disconnect(id))
            .await
            .unwrap();
    }
    let handle = measured("connect_cold", ssh.connect(id)).await.unwrap();
    for _ in 0..100 {
        measured("status_cached", ssh.status(id)).await.unwrap();
        measured("config_list", ssh.list_connections()).await;
        measured("details_cached", ssh.connection_details(id))
            .await
            .unwrap();
    }
    for _ in 0..30 {
        measured("connect_reuse", ssh.connect(id)).await.unwrap();
        let output = measured("exec_true", ssh.exec(&handle, "printf R075_OK"))
            .await
            .unwrap();
        assert_eq!(output.stdout, "R075_OK");
        measured("sftp_stat_tmp", ssh.file_metadata(&handle, "/tmp", false))
            .await
            .unwrap();
        measured("sftp_realpath", ssh.realpath(&handle, "."))
            .await
            .unwrap();
    }
    let remote = format!("/tmp/deepagent-r075-{}", uuid::Uuid::new_v4());
    println!("RESPONSE_OWNED_ROOT {remote}");
    ssh.exec(&handle, &format!("umask 077; mkdir -- '{remote}'"))
        .await
        .unwrap();
    // The child task allows cleanup to run even when an assertion panics.
    let owned_ssh = ssh.clone();
    let owned_handle = handle.clone();
    let owned_root = remote.clone();
    let outcome = tokio::spawn(async move {
        exercise_files_and_terminal(&owned_ssh, &owned_handle, &owned_root, temp.path()).await;
    })
    .await;
    let suffix = remote.strip_prefix("/tmp/deepagent-r075-").unwrap();
    assert!(uuid::Uuid::parse_str(suffix).is_ok());
    let cleanup = timeout(
        Duration::from_secs(15),
        ssh.exec(&handle, &format!("rm -rf -- '{remote}'")),
    )
    .await;
    measured("disconnect_final", ssh.disconnect(id))
        .await
        .unwrap();
    ssh.shutdown().await.unwrap();
    assert!(matches!(cleanup, Ok(Ok(_))), "test-owned directory cleanup");
    outcome.expect("response profile functional assertions");
}

async fn exercise_files_and_terminal(
    ssh: &Arc<SshService>,
    handle: &SshServiceHandle,
    root: &str,
    local: &std::path::Path,
) {
    let id = &handle.connection_id;
    for _ in 0..30 {
        let listing = measured("sftp_list_empty", ssh.list_dir(handle, root))
            .await
            .unwrap();
        assert!(listing.entries.is_empty());
    }
    let small = local.join("small.bin");
    let large = local.join("one-mib.bin");
    std::fs::write(&small, vec![0x5a; 4096]).unwrap();
    std::fs::write(
        &large,
        (0..1024 * 1024)
            .map(|i| (i % 251) as u8)
            .collect::<Vec<_>>(),
    )
    .unwrap();
    let remote_file = format!("{root}/中文 ' $.bin");
    for _ in 0..5 {
        let receipt = measured(
            "upload_4k_verified",
            ssh.transfer_file(
                id,
                transfer(&small, &remote_file, true),
                CancellationToken::new(),
                Arc::new(|_| {}),
            ),
        )
        .await
        .unwrap();
        assert_eq!(receipt.bytes, "4096");
        let downloaded = local.join("download.bin");
        measured(
            "download_4k_verified",
            ssh.transfer_file(
                id,
                transfer(&downloaded, &remote_file, false),
                CancellationToken::new(),
                Arc::new(|_| {}),
            ),
        )
        .await
        .unwrap();
        assert_eq!(
            std::fs::read(downloaded).unwrap(),
            std::fs::read(&small).unwrap()
        );
    }
    let large_remote = format!("{root}/one-mib.bin");
    measured(
        "upload_1m_verified",
        ssh.transfer_file(
            id,
            transfer(&large, &large_remote, true),
            CancellationToken::new(),
            Arc::new(|_| {}),
        ),
    )
    .await
    .unwrap();
    let downloaded = local.join("download-large.bin");
    measured(
        "download_1m_verified",
        ssh.transfer_file(
            id,
            transfer(&downloaded, &large_remote, false),
            CancellationToken::new(),
            Arc::new(|_| {}),
        ),
    )
    .await
    .unwrap();
    assert_eq!(
        std::fs::read(downloaded).unwrap(),
        std::fs::read(&large).unwrap()
    );
    let cancelled = CancellationToken::new();
    cancelled.cancel();
    let failure = measured(
        "upload_precancelled",
        ssh.transfer_file(
            id,
            transfer(&large, &format!("{root}/cancelled.bin"), true),
            cancelled,
            Arc::new(|_| {}),
        ),
    )
    .await
    .unwrap_err();
    assert_eq!(failure.code, "cancelled");
    assert!(ssh
        .file_metadata(handle, &format!("{root}/cancelled.bin"), false)
        .await
        .is_err());

    let version = measured(
        "edit_content_version",
        ssh.file_content_version(id, &remote_file, CancellationToken::new()),
    )
    .await
    .unwrap();
    std::fs::write(&small, b"edited via acceptance").unwrap();
    let edit = ConditionalUploadRequest {
        connection_id: id.clone(),
        connection_generation: version.0,
        local_path: small.to_string_lossy().into(),
        remote_path: remote_file.clone(),
        expected_sha256: version.1,
    };
    measured(
        "edit_conditional_upload",
        ssh.conditional_upload(edit.clone(), CancellationToken::new(), Arc::new(|_| {})),
    )
    .await
    .unwrap();
    assert_eq!(
        measured(
            "edit_stale_rejected",
            ssh.conditional_upload(edit, CancellationToken::new(), Arc::new(|_| {}))
        )
        .await
        .unwrap_err()
        .code,
        "conflict"
    );

    for _ in 0..3 {
        let preview = measured(
            "chmod_preview",
            ssh.preview_chmod(
                id,
                &remote_file,
                0o600,
                false,
                DirectoryWalkLimits::default(),
                CancellationToken::new(),
            ),
        )
        .await
        .unwrap();
        measured(
            "chmod_execute",
            ssh.execute_chmod(
                RemoteChmodRequest::from(&preview),
                DirectoryWalkLimits::default(),
                CancellationToken::new(),
                Arc::new(|_| {}),
            ),
        )
        .await
        .unwrap();
        assert_eq!(
            ssh.file_metadata(handle, &remote_file, false)
                .await
                .unwrap()
                .permissions
                .unwrap()
                & 0o777,
            0o600
        );
    }

    let pty = measured("pty_spawn", ssh.pty_spawn(handle, 80, 24))
        .await
        .unwrap();
    ssh.pty_write(
        &pty,
        b"unset HISTFILE; set +o history 2>/dev/null; stty -echo; printf 'READY_%s\\n' 'R075'\r",
    )
    .await
    .unwrap();
    marker(ssh, &pty, b"READY_R075\r\n").await;
    for n in 0..30 {
        let command = format!("printf 'ROUND_%s\\n' '{n}'\r");
        let started = Instant::now();
        measured("pty_write_ack", ssh.pty_write(&pty, command.as_bytes()))
            .await
            .unwrap();
        marker(ssh, &pty, format!("ROUND_{n}\r\n").as_bytes()).await;
        println!(
            "RESPONSE_METRIC {}",
            serde_json::json!({"operation":"pty_roundtrip","ms":started.elapsed().as_secs_f64()*1000.0,"completed":true})
        );
        measured("pty_read_empty", ssh.pty_read(&pty))
            .await
            .unwrap();
    }
    measured("pty_resize_ack", ssh.pty_resize(&pty, 100, 31))
        .await
        .unwrap();
    ssh.pty_write(&pty, b"printf 'SIZE_%s\\n' \"$(stty size)\"\r")
        .await
        .unwrap();
    marker(ssh, &pty, b"SIZE_31 100\r\n").await;

    for _ in 0..5 {
        let slow_ssh = ssh.clone();
        let slow_handle = handle.clone();
        let slow = tokio::spawn(async move {
            slow_ssh
                .exec(&slow_handle, "sleep 1; printf SLOW_DONE")
                .await
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        measured("status_during_slow_exec", ssh.status(id))
            .await
            .unwrap();
        assert_eq!(
            measured(
                "exec_during_slow_exec",
                ssh.exec(handle, "printf FAST_DONE")
            )
            .await
            .unwrap()
            .stdout,
            "FAST_DONE"
        );
        measured("list_during_slow_exec", ssh.list_dir(handle, root))
            .await
            .unwrap();
        assert_eq!(slow.await.unwrap().unwrap().stdout, "SLOW_DONE");
    }
    measured("pty_close", ssh.pty_close(&pty)).await.unwrap();
    assert_eq!(
        ssh.exec(handle, "printf AFTER_PTY_CLOSE")
            .await
            .unwrap()
            .stdout,
        "AFTER_PTY_CLOSE"
    );
    let generation = ssh
        .connection_details(id)
        .await
        .unwrap()
        .transport_generation;
    let forwards = ssh.list_forwards(id, generation).await.unwrap();
    assert!(forwards.is_empty());
    println!("RESPONSE_ASSERTIONS binary_integrity edit_conflict cancellation chmod pty_resize channel_isolation PASS");
}

struct PinnedKey(Vec<russh::keys::PublicKey>);

impl russh::client::Handler for PinnedKey {
    type Error = russh::Error;
    async fn check_server_key(
        &mut self,
        key: &russh::keys::PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        Ok(key.certificate().is_none() && self.0.contains(&key.public_key()))
    }
}

/// A diagnostic control for separating network/protocol round trips from
/// SshService scheduling. This does not replace the production-service profile.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires the same authorized pinned real endpoint as the service profile"]
async fn real_transport_phase_profile() {
    use russh::ChannelMsg;
    let request: CreateSshConnectionRequest = serde_json::from_str(
        &std::env::var("DEEPAGENT_SSH_EXTERNAL_REQUEST").expect("endpoint in child environment"),
    )
    .unwrap();
    assert_eq!(request.host_key_policy, SshHostKeyPolicy::Strict);
    assert!(
        request.proxy.is_none() && request.jump.is_none(),
        "direct diagnostic endpoint"
    );
    let identity = if request.port == 22 {
        request.host.clone()
    } else {
        format!("[{}]:{}", request.host, request.port)
    };
    let pins = std::fs::read_to_string(request.known_hosts_path.as_ref().unwrap())
        .unwrap()
        .lines()
        .filter_map(|line| {
            let fields: Vec<_> = line.split_whitespace().collect();
            if fields.len() >= 3 && fields[0].split(',').any(|host| host == identity) {
                russh::keys::PublicKey::from_openssh(&format!("{} {}", fields[1], fields[2])).ok()
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    assert!(
        !pins.is_empty(),
        "diagnostic control requires explicit independently verified pins"
    );
    let socket = measured(
        "phase_tcp_connect",
        tokio::net::TcpStream::connect((request.host.as_str(), request.port)),
    )
    .await
    .unwrap();
    let mut handle = measured(
        "phase_ssh_handshake",
        russh::client::connect_stream(
            Arc::new(russh::client::Config::default()),
            socket,
            PinnedKey(pins),
        ),
    )
    .await
    .unwrap();
    let auth = match request.auth_type {
        SshAuthType::Password => measured(
            "phase_password_auth",
            handle.authenticate_password(&request.username, request.password.as_deref().unwrap()),
        )
        .await
        .unwrap(),
        SshAuthType::KeyFile => {
            let bytes = std::fs::read_to_string(request.key_path.as_ref().unwrap()).unwrap();
            let key = measured("phase_private_key_decode", async {
                russh::keys::decode_secret_key(&bytes, None)
            })
            .await
            .unwrap();
            let hash = measured("phase_rsa_negotiation", handle.best_supported_rsa_hash())
                .await
                .unwrap()
                .flatten();
            measured(
                "phase_publickey_auth",
                handle.authenticate_publickey(
                    &request.username,
                    russh::keys::PrivateKeyWithHashAlg::new(Arc::new(key), hash),
                ),
            )
            .await
            .unwrap()
        }
        _ => panic!("control supports the authorized password and key-file fixtures"),
    };
    assert!(auth.success());
    for _ in 0..20 {
        let mut channel = measured("phase_exec_channel_open", handle.channel_open_session())
            .await
            .unwrap();
        measured(
            "phase_exec_request_send",
            channel.exec(true, "printf PHASE_OK"),
        )
        .await
        .unwrap();
        let first = Instant::now();
        let mut output = Vec::new();
        let mut status = None;
        let mut received = false;
        let mut last_data = None;
        timeout(Duration::from_secs(15), async {
            while let Some(message) = channel.wait().await {
                match message {
                    ChannelMsg::Data { data } => {
                        if !received {
                            println!("RESPONSE_METRIC {}", serde_json::json!({"operation":"phase_exec_first_data","ms":first.elapsed().as_secs_f64()*1000.0,"completed":true}));
                            received = true;
                        }
                        output.extend_from_slice(&data);
                        last_data = Some(Instant::now());
                    }
                    ChannelMsg::ExitStatus { exit_status } => status = Some(exit_status),
                    ChannelMsg::Failure => panic!("control command rejected"),
                    _ => {}
                }
            }
        }).await.unwrap();
        assert_eq!(output, b"PHASE_OK");
        assert_eq!(status, Some(0));
        println!(
            "RESPONSE_METRIC {}",
            serde_json::json!({"operation":"phase_exec_after_data_close","ms":last_data.unwrap().elapsed().as_secs_f64()*1000.0,"completed":true})
        );
    }
    for _ in 0..10 {
        let channel = measured("phase_sftp_channel_open", handle.channel_open_session())
            .await
            .unwrap();
        measured(
            "phase_sftp_subsystem_send",
            channel.request_subsystem(true, "sftp"),
        )
        .await
        .unwrap();
        let sftp = measured(
            "phase_sftp_init",
            russh_sftp::client::SftpSession::new_opts(channel.into_stream(), Some(10)),
        )
        .await
        .unwrap();
        for _ in 0..3 {
            measured("phase_sftp_stat_reused", sftp.symlink_metadata("/tmp"))
                .await
                .unwrap();
            measured("phase_sftp_realpath_reused", sftp.canonicalize("."))
                .await
                .unwrap();
        }
        measured("phase_sftp_close", sftp.close()).await.unwrap();
    }
    handle
        .disconnect(russh::Disconnect::ByApplication, "", "en")
        .await
        .unwrap();
}
