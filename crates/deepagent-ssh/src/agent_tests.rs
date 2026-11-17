use super::*;
use russh::keys::{
    signature::Signer, ssh_encoding::Encode, ssh_key::rand_core::OsRng, Algorithm, PrivateKey,
};
use std::collections::VecDeque;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[derive(Default)]
struct Connector {
    clients: Mutex<VecDeque<Result<AgentClient, AgentIssue>>>,
    calls: Mutex<Vec<SshAgentProvider>>,
}

#[async_trait::async_trait]
impl AgentConnector for Connector {
    async fn connect(&self, provider: SshAgentProvider) -> Result<AgentClient, AgentIssue> {
        self.calls.lock().unwrap().push(provider);
        self.clients
            .lock()
            .unwrap()
            .pop_front()
            .expect("unexpected provider access")
    }
    fn automatic_providers(&self) -> &'static [SshAgentProvider] {
        &[SshAgentProvider::OpenSsh, SshAgentProvider::Pageant]
    }
}

fn fixture(keys: Vec<PrivateKey>, reject_signature: bool) -> (AgentClient, Arc<AtomicUsize>) {
    let (client, mut server) = tokio::io::duplex(8192);
    let signatures = Arc::new(AtomicUsize::new(0));
    let count = signatures.clone();
    tokio::spawn(async move {
        while let Ok(size) = server.read_u32().await {
            assert!(size < 8192);
            let mut request = vec![0; size as usize];
            server.read_exact(&mut request).await.unwrap();
            let mut response = Vec::new();
            match request[0] {
                11 => {
                    response.push(12);
                    (keys.len() as u32).encode(&mut response).unwrap();
                    for key in &keys {
                        key.public_key()
                            .to_bytes()
                            .unwrap()
                            .encode(&mut response)
                            .unwrap();
                        "private comment must not reach DTO"
                            .encode(&mut response)
                            .unwrap();
                    }
                }
                13 => {
                    count.fetch_add(1, Ordering::SeqCst);
                    if reject_signature {
                        response.push(5);
                    } else {
                        let mut cursor = &request[1..];
                        let blob_size = cursor.read_u32().await.unwrap() as usize;
                        let blob = cursor[..blob_size].to_vec();
                        cursor = &cursor[blob_size..];
                        let data_size = cursor.read_u32().await.unwrap() as usize;
                        let key = keys
                            .iter()
                            .find(|key| key.public_key().to_bytes().unwrap() == blob)
                            .unwrap();
                        let signature: russh::keys::ssh_key::Signature =
                            key.key_data().try_sign(&cursor[..data_size]).unwrap();
                        let mut encoded = Vec::new();
                        signature.encode(&mut encoded).unwrap();
                        response.push(14);
                        encoded.encode(&mut response).unwrap();
                    }
                }
                _ => {
                    panic!("Agent inspection/authentication must not add/remove/unlock identities")
                }
            }
            if server.write_u32(response.len() as u32).await.is_err() {
                break;
            }
            if server.write_all(&response).await.is_err() {
                break;
            }
        }
    });
    (
        russh::keys::agent::client::AgentClient::connect(client).dynamic(),
        signatures,
    )
}

#[tokio::test]
async fn auto_falls_back_only_from_unavailable_or_empty_and_probe_never_signs() {
    let key = PrivateKey::random(&mut OsRng, Algorithm::Ed25519).unwrap();
    let (empty, _) = fixture(vec![], false);
    let (ready, signed) = fixture(vec![key.clone()], false);
    let connector = Arc::new(Connector {
        clients: Mutex::new(VecDeque::from([Ok(empty), Ok(ready)])),
        ..Default::default()
    });
    let resolution = resolve_agent(SshAgentProvider::Auto, connector.clone()).await;
    assert_eq!(resolution.probes[0].state, SshAgentState::Empty);
    assert_eq!(resolution.probes[1].state, SshAgentState::Available);
    assert_eq!(signed.load(Ordering::SeqCst), 0);
    let dto = serde_json::to_string(&resolution.probes).unwrap();
    assert!(dto.contains(&key.public_key().fingerprint(HashAlg::Sha256).to_string()));
    assert!(!dto.contains("private comment"));
    assert_eq!(
        *connector.calls.lock().unwrap(),
        vec![SshAgentProvider::OpenSsh, SshAgentProvider::Pageant]
    );
}

#[tokio::test]
async fn explicit_provider_and_protocol_error_do_not_silently_fall_back() {
    let connector = Arc::new(Connector {
        clients: Mutex::new(VecDeque::from([Err(AgentIssue::new(
            SshAgentState::Unavailable,
            "fixture absent",
        ))])),
        ..Default::default()
    });
    let resolution = resolve_agent(SshAgentProvider::Pageant, connector.clone()).await;
    assert!(resolution.ready.is_none());
    assert_eq!(
        *connector.calls.lock().unwrap(),
        vec![SshAgentProvider::Pageant]
    );
    let connector = Arc::new(Connector {
        clients: Mutex::new(VecDeque::from([Err(AgentIssue::new(
            SshAgentState::Error,
            "fixture malformed",
        ))])),
        ..Default::default()
    });
    assert!(resolve_agent(SshAgentProvider::Auto, connector.clone())
        .await
        .ready
        .is_none());
    assert_eq!(connector.calls.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn automatic_provider_selection_stops_at_first_ready_and_limits_identity_count() {
    let key = PrivateKey::random(&mut OsRng, Algorithm::Ed25519).unwrap();
    let (ready, _) = fixture(vec![key.clone()], false);
    let connector = Arc::new(Connector {
        clients: Mutex::new(VecDeque::from([
            Err(AgentIssue::new(
                SshAgentState::Unavailable,
                "fixture absent",
            )),
            Ok(ready),
        ])),
        ..Default::default()
    });
    let resolution = resolve_agent(SshAgentProvider::Auto, connector.clone()).await;
    assert_eq!(resolution.probes[0].state, SshAgentState::Unavailable);
    assert_eq!(resolution.probes[1].state, SshAgentState::Available);
    let (ready, _) = fixture(vec![key.clone()], false);
    let connector = Arc::new(Connector {
        clients: Mutex::new(VecDeque::from([Ok(ready)])),
        ..Default::default()
    });
    assert!(resolve_agent(SshAgentProvider::Auto, connector.clone())
        .await
        .ready
        .is_some());
    assert_eq!(connector.calls.lock().unwrap().len(), 1);
    let (many, _) = fixture(vec![key; MAX_IDENTITIES + 1], false);
    let connector = Arc::new(Connector {
        clients: Mutex::new(VecDeque::from([Ok(many)])),
        ..Default::default()
    });
    let resolution = resolve_agent(SshAgentProvider::Auto, connector.clone()).await;
    assert_eq!(resolution.probes[0].state, SshAgentState::Error);
    assert!(resolution.ready.is_none());
    assert_eq!(connector.calls.lock().unwrap().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn unresponsive_agent_inspection_is_bounded_without_fallback() {
    let (client, _held_server) = tokio::io::duplex(64);
    let connector = Arc::new(Connector {
        clients: Mutex::new(VecDeque::from([Ok(
            russh::keys::agent::client::AgentClient::connect(client).dynamic(),
        )])),
        ..Default::default()
    });
    let start = tokio::time::Instant::now();
    let resolution = resolve_agent(SshAgentProvider::Auto, connector.clone()).await;
    assert_eq!(resolution.probes[0].state, SshAgentState::TimedOut);
    assert_eq!(tokio::time::Instant::now() - start, INSPECTION_TIMEOUT);
    assert_eq!(connector.calls.lock().unwrap().len(), 1);
}

#[cfg(windows)]
#[tokio::test]
async fn windows_openssh_pipe_uses_real_native_transport_without_touching_system_agent() {
    let path = format!(r"\\.\pipe\deepagent-agent-test-{}", uuid::Uuid::new_v4());
    let mut pipe = tokio::net::windows::named_pipe::ServerOptions::new()
        .first_pipe_instance(true)
        .create(&path)
        .unwrap();
    let key = PrivateKey::random(&mut OsRng, Algorithm::Ed25519).unwrap();
    let expected = key.public_key().clone();
    let (agent, signed) = fixture(vec![key], false);
    let bridge = tokio::spawn(async move {
        pipe.connect().await.unwrap();
        tokio::io::copy_bidirectional(&mut pipe, &mut agent.into_inner()).await
    });
    let mut client = connect_openssh_pipe(&path).await.unwrap();
    let keys = client.request_identities().await.unwrap();
    assert_eq!(keys.len(), 1);
    assert_eq!(keys[0].to_bytes().unwrap(), expected.to_bytes().unwrap());
    assert_eq!(keys[0].comment(), "private comment must not reach DTO");
    assert_eq!(signed.load(Ordering::SeqCst), 0);
    let data = b"isolated named-pipe signing fixture";
    let signature = client
        .sign_request_signature(&keys[0], None, data)
        .await
        .unwrap();
    russh::keys::signature::Verifier::verify(&expected, data, &signature).unwrap();
    assert_eq!(signed.load(Ordering::SeqCst), 1);
    drop(client);
    bridge.abort();
}

#[tokio::test]
async fn agent_signature_authenticates_a_real_ssh_session_and_rejection_is_terminal() {
    use russh::{client, server};
    struct ServerHandler;
    impl server::Handler for ServerHandler {
        type Error = russh::Error;
        async fn auth_publickey(
            &mut self,
            _: &str,
            _: &PublicKey,
        ) -> Result<server::Auth, Self::Error> {
            Ok(server::Auth::Accept)
        }
    }
    let server_key = PrivateKey::random(&mut OsRng, Algorithm::Ed25519).unwrap();
    let public = server_key.public_key().clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("known_hosts");
    std::fs::write(
        &path,
        format!(
            "[127.0.0.1]:{} {}\n",
            address.port(),
            public.to_openssh().unwrap()
        ),
    )
    .unwrap();
    let mut config = crate::SshConnectionConfig::new(
        "fixture",
        "127.0.0.1",
        address.port(),
        "fixture",
        crate::SshAuthType::Agent,
    );
    config.known_hosts_path = Some(path.to_str().unwrap().to_owned());
    let trust = crate::host_trust::HostTrust::load(&config).await.unwrap();
    let server = tokio::spawn(async move {
        let config = Arc::new(server::Config {
            keys: vec![server_key],
            ..Default::default()
        });
        while let Ok((socket, _)) = listener.accept().await {
            server::run_stream(config.clone(), socket, ServerHandler)
                .await
                .unwrap();
        }
    });
    for reject in [false, true] {
        let (agent, signed) = fixture(
            vec![PrivateKey::random(&mut OsRng, Algorithm::Ed25519).unwrap()],
            reject,
        );
        let connector = Arc::new(Connector {
            clients: Mutex::new(VecDeque::from([Ok(agent)])),
            ..Default::default()
        });
        let resolution = resolve_agent(SshAgentProvider::Auto, connector.clone()).await;
        let mut handle = client::connect(
            Arc::new(client::Config::default()),
            address,
            IdentityHandler::verified(trust.clone()),
        )
        .await
        .unwrap();
        let result = authenticate_ready(&mut handle, "fixture", resolution.ready.unwrap()).await;
        assert_eq!(result.is_ok(), !reject);
        assert_eq!(signed.load(Ordering::SeqCst), 1);
        assert_eq!(
            connector.calls.lock().unwrap().len(),
            1,
            "signing failure cannot try another provider"
        );
        handle
            .disconnect(russh::Disconnect::ByApplication, "", "en")
            .await
            .unwrap();
    }
    server.abort();
}
