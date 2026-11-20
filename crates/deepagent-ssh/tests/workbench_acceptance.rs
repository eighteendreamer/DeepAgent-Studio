//! Opt-in production-service tests. Requests and credentials enter through the
//! child environment; only test-owned UUID directories are mutated.
use deepagent_ssh::*;
use deepagent_terminal::{
    TerminalInputHolder, TerminalOpenRequest, TerminalSessionBackend, TerminalTarget,
};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio_util::sync::CancellationToken;
#[derive(Default)]
struct MemoryStore(Mutex<Vec<SshConnectionConfig>>);
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
fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}
async fn exec(ssh: &SshService, handle: &SshServiceHandle, command: &str) -> String {
    let result = ssh
        .exec(handle, command)
        .await
        .expect("test-owned remote command");
    assert_eq!(
        result.exit_code, 0,
        "test setup/verification command failed (output redacted)"
    );
    result.stdout
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires explicitly authorized real SSH requests in child environment"]
async fn real_file_workbench() {
    let owned = Arc::new(Mutex::new(Vec::new()));
    let cleanup = Arc::clone(&owned);
    let result = tokio::spawn(async move {
    tokio::time::timeout(Duration::from_secs(180), async move {
        let requests: Vec<CreateSshConnectionRequest> = serde_json::from_str(&std::env::var("DEEPAGENT_FILE_TEST_REQUESTS").expect("requests in child environment")).expect("valid requests");
        assert!(!requests.is_empty());
        let temp = tempfile::tempdir().unwrap();
        let trust = Arc::new(SshHostKeyStore::new(Arc::new(deepagent_persistence::Database::open_in_memory().unwrap())));
        for request in &requests {
            let identity = SshHostIdentity::direct(&request.host, request.port).unwrap();
            for line in std::fs::read_to_string(request.known_hosts_path.as_ref().expect("independently pinned hosts required")).unwrap().lines() {
                let p: Vec<_> = line.split_whitespace().collect();
                let host = if request.port == 22 { request.host.clone() } else { format!("[{}]:{}", request.host, request.port) };
                if p.len() >= 3 && p[0].split(',').any(|value| value == host) { if let Ok(key) = russh::keys::PublicKey::from_openssh(&format!("{} {}",p[1],p[2])) { trust.trust_verified(&identity, &key, "authorized isolated test host pin").unwrap(); } }
            }
        }
        let ssh = Arc::new(SshService::with_config_store(temp.path().into(), Arc::new(MemoryStore::default())).with_host_key_store(trust));
        let mut endpoints = Vec::new();
        for request in requests { let c=ssh.create_connection(request).await.unwrap(); let h=ssh.connect(&c.id).await.unwrap(); endpoints.push((c.id,h)); }
        let mut roots = Vec::new();
        for (index,(id,handle)) in endpoints.iter().enumerate() {
            let root=format!("/tmp/deepagent-r072-{}",uuid::Uuid::new_v4()); roots.push(root.clone());
            exec(&ssh,handle,&format!("umask 077; mkdir -- {}",quote(&root))).await;
            owned.lock().unwrap().push((ssh.clone(), handle.clone(), root.clone()));
            exec(&ssh,handle,&format!("umask 077; mkdir -p {}/src/empty {}/extract",quote(&root),quote(&root))).await;
            let source=format!("{root}/src/中文 ' $.bin");
            let local=temp.path().join(format!("input-{index}.bin")); let payload:Vec<u8>=(0..4096).map(|i| (i%256) as u8).collect(); std::fs::write(&local,&payload).unwrap();
            ssh.transfer_file(id,FileTransferRequest{direction:FileTransferDirection::Upload,local_path:local.to_string_lossy().into(),remote_path:source.clone(),conflict:FileConflictPolicy::Ask},CancellationToken::new(),Arc::new(|_|{})).await.unwrap();
            let preview=ssh.preview_chmod(id,&format!("{root}/src"),0o750,true,DirectoryWalkLimits::default(),CancellationToken::new()).await.unwrap();
            assert_eq!(preview.total_files,1); assert_eq!(preview.total_directories,2);
            let receipt=ssh.execute_chmod(RemoteChmodRequest::from(&preview),DirectoryWalkLimits::default(),CancellationToken::new(),Arc::new(|_|{})).await.unwrap(); assert_eq!(receipt.changed_entries,3);
            let spec=ServerCopySpec{source_connection_id:id.clone(),destination_connection_id:id.clone(),source_path:format!("{root}/src"),destination_path:format!("{root}/copy"),conflict:FileConflictPolicy::Ask,method:ServerCopyMethod::Relay};
            let p=ssh.preview_server_copy(spec.clone(),CancellationToken::new()).await.unwrap();ssh.execute_server_copy(ServerCopyRequest::from(&p),CancellationToken::new(),Arc::new(|_|{})).await.unwrap();
            let downloaded=temp.path().join(format!("download-{index}.bin"));ssh.transfer_file(id,FileTransferRequest{direction:FileTransferDirection::Download,local_path:downloaded.to_string_lossy().into(),remote_path:format!("{root}/copy/中文 ' $.bin"),conflict:FileConflictPolicy::Ask},CancellationToken::new(),Arc::new(|_|{})).await.unwrap();assert_eq!(std::fs::read(downloaded).unwrap(),payload);
            let archive=format!("{root}/test.zip");
            let python=format!("import zipfile; z=zipfile.ZipFile({},'w'); z.writestr('nested/readme.txt',b'archive-data'); z.writestr('empty/',b''); z.close()",serde_json::to_string(&archive).unwrap());
            exec(&ssh,handle,&format!("python3 -c {}",quote(&python))).await;
            let p=ssh.preview_archive(ArchiveSpec{connection_id:id.clone(),archive_path:archive,destination_path:format!("{root}/extract"),conflict:FileConflictPolicy::Ask},CancellationToken::new()).await.unwrap();
            let receipt=ssh.execute_archive(ArchiveRequest{version:1,spec:p.spec,connection_generation:p.connection_generation,fingerprint:p.fingerprint},CancellationToken::new()).await.unwrap(); assert_eq!(receipt["files"],1);
            let (generation,hash)=ssh.file_content_version(id,&source,CancellationToken::new()).await.unwrap();
            exec(&ssh,handle,&format!("printf 'remote-change' > {}",quote(&source))).await;
            let edit=ConditionalUploadRequest{connection_id:id.clone(),connection_generation:generation,local_path:local.to_string_lossy().into(),remote_path:source.clone(),expected_sha256:hash};
            assert_eq!(ssh.conditional_upload(edit,CancellationToken::new(),Arc::new(|_|{})).await.unwrap_err().code,"conflict");
            // A panel opens its own PTY in the chosen directory, with existing leases.
            let backend=SshTerminalSessionBackend::for_service(ssh.clone(),None);
            let(session,lease)=backend.open(TerminalOpenRequest{target:TerminalTarget::Ssh{connection_id:id.clone()},run_id:format!("test-{index}"),cwd:format!("{root}/copy"),cols:80,rows:24,initial_holder:TerminalInputHolder::User}).await.unwrap();
            backend.write(&session,&lease,b"unset HISTFILE; printf 'CWD_START'; pwd; printf 'CWD_END\\n'\r").await.unwrap();
            let mut cursor=0;let mut output=Vec::new();
            tokio::time::timeout(Duration::from_secs(15),async{loop{let chunk=backend.read(&session,cursor).await.unwrap();cursor=chunk.cursor;output.extend_from_slice(&chunk.data);if output.windows(format!("{root}/copy").len()).any(|w|w==format!("{root}/copy").as_bytes()){break;}tokio::time::sleep(Duration::from_millis(30)).await;}}).await.unwrap();
            backend.close(&session,&lease).await.unwrap();
            println!("ENDPOINT {index}: chmod, same-endpoint relay, binary SHA256, safe zip, edit conflict and terminal cwd PASS");
        }
        if endpoints.len()>1 {
            let(a,_)=&endpoints[0]; let(b,_)=&endpoints[1];
            let mut spec=ServerCopySpec{source_connection_id:a.clone(),destination_connection_id:b.clone(),source_path:format!("{}/copy",roots[0]),destination_path:format!("{}/cross-relay",roots[1]),conflict:FileConflictPolicy::Ask,method:ServerCopyMethod::Relay};
            let p=ssh.preview_server_copy(spec.clone(),CancellationToken::new()).await.unwrap();ssh.execute_server_copy(ServerCopyRequest::from(&p),CancellationToken::new(),Arc::new(|_|{})).await.unwrap();println!("CROSS-ENDPOINT relay PASS");
            let probe=ssh.probe_direct_copy(a,b,CancellationToken::new()).await;
            if std::env::var_os("DEEPAGENT_FILE_REQUIRE_SCP").is_some(){assert!(probe.authenticated&&probe.scp,"source-side SCP authentication and pinned Host Key required: {:?}",probe.reason);}
            if probe.authenticated&&probe.scp {spec.method=ServerCopyMethod::Scp;spec.destination_path=format!("{}/cross-scp",roots[1]);let p=ssh.preview_server_copy(spec.clone(),CancellationToken::new()).await.unwrap();ssh.execute_server_copy(ServerCopyRequest::from(&p),CancellationToken::new(),Arc::new(|_|{})).await.unwrap();println!("DIRECT SCP verified staging PASS");}
            if probe.authenticated&&probe.rsync {spec.method=ServerCopyMethod::Rsync;spec.destination_path=format!("{}/cross-rsync",roots[1]);let p=ssh.preview_server_copy(spec,CancellationToken::new()).await.unwrap();ssh.execute_server_copy(ServerCopyRequest::from(&p),CancellationToken::new(),Arc::new(|_|{})).await.unwrap();println!("DIRECT RSYNC verified staging PASS");}else{println!("DIRECT RSYNC unavailable in this test environment");}
        }
    }).await.expect("bounded real file workbench test");
    }).await;
    let owned = std::mem::take(&mut *cleanup.lock().unwrap());
    let mut failed_cleanup = false;
    for (ssh, handle, root) in owned {
        let suffix = root.strip_prefix("/tmp/deepagent-r072-").unwrap();
        assert!(uuid::Uuid::parse_str(suffix).is_ok());
        let removed = tokio::time::timeout(
            Duration::from_secs(10),
            ssh.exec(&handle, &format!("rm -rf -- {}", quote(&root))),
        )
        .await;
        if !matches!(removed, Ok(Ok(result)) if result.exit_code == 0) {
            eprintln!("Failed to clean test-owned directory: {root}");
            failed_cleanup = true;
        }
    }
    result.expect("real file workbench assertions");
    assert!(!failed_cleanup);
}
