//! Opt-in acceptance against real SSH servers through the production service.
//!
//! Supply a CreateSshConnectionRequest as DEEPAGENT_SSH_EXTERNAL_REQUEST in the
//! child environment, never argv or a fixture. Use an isolated, pinned
//! known_hosts file and set TEMP/TMP/TMPDIR to the intended scratch drive.
//! Credentials are retained only by MemoryStore; shell output is not logged.
//! Run the desired ignored test explicitly with --exact --ignored --nocapture.

use async_trait::async_trait;
use deepagent_ssh::{
    add_ssh_agent_identity, preview_agent_identity, probe_ssh_agent, CreateSshConnectionRequest,
    SshAgentAddRequest, SshAgentProvider, SshConfigStore, SshConnectionConfig, SshForwardKind,
    SshForwardRequest, SshForwardStatus, SshResult, SshService, SshServiceHandle,
};
use serde_json::json;
use std::{sync::Arc, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};

#[derive(Default)]
struct MemoryStore(std::sync::Mutex<Vec<SshConnectionConfig>>);

#[async_trait]
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

fn request() -> CreateSshConnectionRequest {
    serde_json::from_str(
        &std::env::var("DEEPAGENT_SSH_EXTERNAL_REQUEST")
            .expect("provide a request in the child environment"),
    )
    .expect("valid connection request")
}

fn service(path: &std::path::Path) -> SshService {
    SshService::with_config_store(path.into(), Arc::new(MemoryStore::default()))
}

async fn exec(service: &SshService, handle: &SshServiceHandle, command: &str) -> String {
    let result = tokio::time::timeout(Duration::from_secs(20), service.exec(handle, command))
        .await
        .expect("bounded remote exec")
        .expect("remote exec");
    assert_eq!(result.exit_code, 0, "remote command exit code");
    result.stdout
}

async fn wait_for(service: &SshService, pty: &SshServiceHandle, needle: &str) {
    let mut output = Vec::new();
    let result = tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            output.extend(service.pty_read(pty).await.expect("PTY read"));
            assert!(output.len() < 1024 * 1024, "bounded PTY output");
            if output.windows(needle.len()).any(|w| w == needle.as_bytes()) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
    })
    .await;
    if result.is_err() {
        let stem = needle.trim_matches(['\r', '\n']);
        let position = output
            .windows(stem.len())
            .position(|w| w == stem.as_bytes());
        let endings: Vec<_> = position
            .map(|p| &output[p + stem.len()..])
            .unwrap_or_default()
            .iter()
            .take_while(|b| matches!(b, b'\r' | b'\n'))
            .copied()
            .collect();
        panic!(
            "PTY result marker timed out; output redacted: bytes={}, stem_seen={}, line_endings={endings:?}",
            output.len(), position.is_some()
        );
    }
}

async fn open_pty(service: &SshService, handle: &SshServiceHandle) -> SshServiceHandle {
    let pty = service.pty_spawn(handle, 80, 24).await.expect("PTY spawn");
    service
        .pty_write(&pty, b"unset HISTFILE; set +o history 2>/dev/null\r")
        .await
        .expect("disable acceptance shell history");
    pty
}

fn forward(kind: SshForwardKind, target_port: u16) -> SshForwardRequest {
    SshForwardRequest {
        kind,
        bind_host: "127.0.0.1".into(),
        bind_port: 0,
        target_host: (kind != SshForwardKind::Dynamic).then(|| "127.0.0.1".into()),
        target_port: (kind != SshForwardKind::Dynamic).then_some(target_port),
        allow_non_loopback: false,
    }
}

async fn ssh_banner(mut stream: TcpStream) {
    tokio::time::timeout(Duration::from_secs(10), async {
        let mut line = Vec::new();
        while line.last() != Some(&b'\n') {
            line.push(stream.read_u8().await.expect("forwarded banner byte"));
            assert!(line.len() < 512);
        }
        assert!(line.starts_with(b"SSH-2.0-OpenSSH_"));
        stream
            .write_all(b"SSH-2.0-DeepAgentAcceptance\r\n")
            .await
            .unwrap();
        stream.shutdown().await.unwrap();
    })
    .await
    .expect("bounded forwarding exchange");
}

async fn remote_echo(service: &SshService, handle: &SshServiceHandle, port: u16, succeeds: bool) {
    let code = if succeeds {
        format!("import socket; s=socket.create_connection(('127.0.0.1',{port}),5); s.settimeout(5); s.sendall(b'deepagent-acceptance'); b=b'';\nwhile len(b)<20: b+=s.recv(20-len(b))\nassert b==b'deepagent-acceptance'; s.close(); print('REMOTE_OK')")
    } else {
        format!("import socket; s=socket.socket(); s.settimeout(3); assert s.connect_ex(('127.0.0.1',{port})) != 0; s.close(); print('CLOSED_OK')")
    };
    // The generated code contains no quotes requiring shell interpolation.
    let command = format!("python3 -c \"{code}\"");
    assert_eq!(
        exec(service, handle, &command).await.trim(),
        if succeeds { "REMOTE_OK" } else { "CLOSED_OK" }
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires an explicitly supplied real SSH endpoint and pinned host key"]
async fn real_auth_pty_and_forwarding() {
    tokio::time::timeout(Duration::from_secs(150), async {
        let data = tempfile::tempdir().unwrap();
        let service = service(data.path());
        let connection = service.create_connection(request()).await.unwrap();
        let result = async {
            let handle = service.connect(&connection.id).await.expect("authenticate");
            assert_eq!(exec(&service, &handle, "printf AUTH_OK").await, "AUTH_OK");
            let details = service.connection_details(&connection.id).await.unwrap();
            let algorithms = details.algorithms.expect("actual negotiated algorithms");
            assert!(!algorithms.kex.is_empty() && !algorithms.host_key.is_empty());
            println!(
                "ACCEPTANCE algorithms {}",
                serde_json::to_string(&algorithms).unwrap()
            );
            println!("ACCEPTANCE authentication PASS");
            let pty = open_pty(&service, &handle).await;
            let nonce = uuid::Uuid::new_v4().simple().to_string();
            let command = format!("printf 'PTY_%s\\n' '{nonce}'\r");
            service.pty_write(&pty, command.as_bytes()).await.unwrap();
            wait_for(&service, &pty, &format!("PTY_{nonce}\r\n")).await;
            service.pty_resize(&pty, 100, 31).await.unwrap();
            // Readline can insert terminal controls before command output, and
            // window-change delivery is asynchronous. Match only the result
            // and poll the actual size rather than assuming an immediate ACK.
            service
                .pty_write(
                    &pty,
                    b"for attempt in $(seq 1 100); do size=$(stty size); if [ \"$size\" = '31 100' ]; then printf 'PTY_SIZE_%s\\n' \"$size\"; break; fi; sleep 0.1; done\r",
                )
                .await
                .unwrap();
            wait_for(&service, &pty, "PTY_SIZE_31 100\r\n").await;
            println!("ACCEPTANCE PTY output and resize PASS");
            let generation = details.transport_generation;
            let remote_ssh_port = std::env::var("DEEPAGENT_SSH_REMOTE_PORT")
                .ok()
                .map(|p| p.parse().unwrap())
                .unwrap_or(22);
            let local = service
                .start_forward(
                    &connection.id,
                    generation,
                    forward(SshForwardKind::Local, remote_ssh_port),
                )
                .await
                .unwrap();
            ssh_banner(
                TcpStream::connect(("127.0.0.1", local.bind_port))
                    .await
                    .unwrap(),
            )
            .await;
            let stopped = service
                .stop_forward(&connection.id, generation, &local.id)
                .await
                .unwrap();
            assert_eq!(stopped.status, SshForwardStatus::Stopped);
            assert!(TcpStream::connect(("127.0.0.1", local.bind_port))
                .await
                .is_err());
            println!("ACCEPTANCE local forward and stop PASS");
            let dynamic = service
                .start_forward(
                    &connection.id,
                    generation,
                    forward(SshForwardKind::Dynamic, 0),
                )
                .await
                .unwrap();
            for domain in [false, true] {
                let mut stream = TcpStream::connect(("127.0.0.1", dynamic.bind_port))
                    .await
                    .unwrap();
                stream.write_all(&[5, 1, 0]).await.unwrap();
                let mut method = [0; 2];
                stream.read_exact(&mut method).await.unwrap();
                assert_eq!(method, [5, 0]);
                let mut connect = if domain {
                    let mut bytes = vec![5, 1, 0, 3, 9];
                    bytes.extend_from_slice(b"localhost");
                    bytes
                } else {
                    vec![5, 1, 0, 1, 127, 0, 0, 1]
                };
                connect.extend_from_slice(&remote_ssh_port.to_be_bytes());
                stream.write_all(&connect).await.unwrap();
                let mut reply = [0; 10];
                stream.read_exact(&mut reply).await.unwrap();
                assert_eq!(reply[1], 0, "SOCKS CONNECT succeeded");
                ssh_banner(stream).await;
            }
            println!("ACCEPTANCE SOCKS IPv4 and remote domain CONNECT PASS");
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let target = listener.local_addr().unwrap().port();
            let echo = tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await.unwrap();
                let (mut read, mut write) = socket.split();
                tokio::io::copy(&mut read, &mut write).await.unwrap();
            });
            let remote = service
                .start_forward(
                    &connection.id,
                    generation,
                    forward(SshForwardKind::Remote, target),
                )
                .await
                .unwrap();
            assert_ne!(remote.bind_port, 0);
            remote_echo(&service, &handle, remote.bind_port, true).await;
            echo.await.unwrap();
            service
                .stop_forward(&connection.id, generation, &remote.id)
                .await
                .unwrap();
            remote_echo(&service, &handle, remote.bind_port, false).await;
            println!("ACCEPTANCE remote forward allocated port, byte exchange and cancel PASS");
            assert_eq!(
                exec(&service, &handle, "printf EXEC_STILL_OK").await,
                "EXEC_STILL_OK"
            );
            service.pty_close(&pty).await.unwrap();
            service.disconnect(&connection.id).await.unwrap();
            assert!(TcpStream::connect(("127.0.0.1", dynamic.bind_port))
                .await
                .is_err());
            service.connect(&connection.id).await.unwrap();
            let current = service.connection_details(&connection.id).await.unwrap();
            assert_ne!(generation, current.transport_generation);
            assert!(service
                .list_forwards(&connection.id, current.transport_generation)
                .await
                .unwrap()
                .is_empty());
            assert!(service
                .start_forward(
                    &connection.id,
                    generation,
                    forward(SshForwardKind::Local, remote_ssh_port)
                )
                .await
                .is_err());
            println!(
                "ACCEPTANCE exec survives, disconnect cleanup and stale generation rejection PASS"
            );
        };
        result.await;
        service.shutdown().await.unwrap();
        assert!(!data.path().join("deepagent-ssh/connections.json").exists());
    })
    .await
    .expect("bounded endpoint acceptance");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires an isolated real Agent and a real target accepting its key"]
async fn real_agent_add_auth_and_forwarding() {
    tokio::time::timeout(Duration::from_secs(150), async {
        let key_path = std::env::var("DEEPAGENT_SSH_AGENT_KEY_PATH").unwrap();
        let mut add = SshAgentAddRequest {
            provider: SshAgentProvider::OpenSsh,
            key_path: Some(key_path),
            key_content: None,
            passphrase: None,
            lifetime_secs: 120,
            expected_fingerprint: String::new(),
            authorized: false,
        };
        let before = probe_ssh_agent(SshAgentProvider::OpenSsh).await;
        assert_eq!(before.len(), 1);
        assert!(
            before[0].identities.is_empty(),
            "use an isolated empty real Agent"
        );
        let identity = preview_agent_identity(add.clone()).await.unwrap();
        add.expected_fingerprint = identity.fingerprint.clone();
        add.authorized = true;
        let result = add_ssh_agent_identity(add.clone()).await.unwrap();
        assert_eq!(result.identity.fingerprint, identity.fingerprint);
        assert_eq!(result.lifetime_secs, Some(120));
        assert!(!result.already_present);
        assert!(add_ssh_agent_identity(add).await.unwrap().already_present);
        println!("ACCEPTANCE constrained real Agent identity add and duplicate preservation PASS");
        let data = tempfile::tempdir().unwrap();
        let service = service(data.path());
        let mut target = request();
        target.auth_type = deepagent_ssh::SshAuthType::OpenSshAgent;
        target.key_path = None;
        target.password = None;
        target.agent_fingerprint = Some(identity.fingerprint);
        let target = service.create_connection(target).await.unwrap();
        let handle = service.connect(&target.id).await.unwrap();
        assert_eq!(
            exec(&service, &handle, "printf AGENT_AUTH_OK").await,
            "AGENT_AUTH_OK"
        );
        println!("ACCEPTANCE real Agent signature authentication PASS");
        let mut via: CreateSshConnectionRequest =
            serde_json::from_str(&std::env::var("DEEPAGENT_SSH_FORWARD_AGENT_REQUEST").unwrap())
                .unwrap();
        via.advanced.forward_agent = true;
        via.advanced.agent_provider = SshAgentProvider::OpenSsh;
        let via = service.create_connection(via).await.unwrap();
        let via_handle = service.connect(&via.id).await.unwrap();
        let pty = open_pty(&service, &via_handle).await;
        let remote_command = std::env::var("DEEPAGENT_SSH_AGENT_REMOTE_COMMAND").unwrap();
        service
            .pty_write(&pty, format!("{remote_command}\r").as_bytes())
            .await
            .unwrap();
        wait_for(&service, &pty, "AGENT_FORWARD_OK\r\n").await;
        println!("ACCEPTANCE remote OpenSSH signs through forwarded Agent PASS");
        service.pty_close(&pty).await.unwrap();
        service.shutdown().await.unwrap();
        loop {
            let probes = probe_ssh_agent(SshAgentProvider::OpenSsh).await;
            assert_eq!(probes.len(), 1);
            if probes[0].state == deepagent_ssh::SshAgentState::Empty {
                break;
            }
            assert_eq!(probes[0].state, deepagent_ssh::SshAgentState::Available);
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        println!("ACCEPTANCE real Agent lifetime expiration PASS");
    })
    .await
    .expect("bounded real Agent acceptance");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires an isolated real SSH daemon offering SHA1 KEX only"]
async fn real_legacy_requires_opt_in() {
    tokio::time::timeout(Duration::from_secs(60), legacy_checks())
        .await
        .expect("bounded legacy acceptance");
}

async fn legacy_checks() {
    let data = tempfile::tempdir().unwrap();
    let service = service(data.path());
    let mut req = request();
    req.advanced.allow_legacy_algorithms = false;
    let strict = service.create_connection(req.clone()).await.unwrap();
    assert!(service.connect(&strict.id).await.is_err());
    assert!(service.connection_details(&strict.id).await.is_err());
    req.advanced.allow_legacy_algorithms = true;
    let compatible = service.create_connection(req).await.unwrap();
    let handle = service.connect(&compatible.id).await.unwrap();
    let algorithms = service
        .connection_details(&compatible.id)
        .await
        .unwrap()
        .algorithms
        .unwrap();
    assert_eq!(algorithms.kex, "diffie-hellman-group14-sha1");
    assert_eq!(
        exec(&service, &handle, "printf LEGACY_OK").await,
        "LEGACY_OK"
    );
    println!(
        "ACCEPTANCE default rejects legacy, explicit opt-in connects {}",
        json!(algorithms)
    );
    service.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires a real local X server and remote xdpyinfo"]
async fn real_x11_authentication_and_pty_lifecycle() {
    tokio::time::timeout(Duration::from_secs(60), x11_checks())
        .await
        .expect("bounded X11 acceptance");
}

async fn x11_checks() {
    let data = tempfile::tempdir().unwrap();
    let service = service(data.path());
    let connection = service.create_connection(request()).await.unwrap();
    let handle = service.connect(&connection.id).await.unwrap();
    let pty = open_pty(&service, &handle).await;
    service
        .pty_write(
            &pty,
            b"xdpyinfo >/dev/null 2>&1; result=$?; xauth remove \"$DISPLAY\" >/dev/null 2>&1; test $result -eq 0 && printf 'X11_%s\\n' REAL_OK\r",
        )
        .await
        .unwrap();
    wait_for(&service, &pty, "X11_REAL_OK\r\n").await;
    println!("ACCEPTANCE real X server authentication through SSH PASS");
    service.pty_close(&pty).await.unwrap();
    service.shutdown().await.unwrap();
}
