use super::*;
use russh::keys::{signature::Signer, ssh_encoding::Encode, Algorithm, PrivateKey};
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
    let key = PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).unwrap();
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
    let key = PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).unwrap();
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
    let key = PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).unwrap();
    let expected = key.public_key().clone();
    let (agent, signed) = fixture(vec![key], false);
    let bridge = tokio::spawn(async move {
        pipe.connect().await.unwrap();
        tokio::io::copy_bidirectional(&mut pipe, &mut agent.into_inner()).await
    });
    let mut client = connect_openssh_pipe(&path).await.unwrap();
    let identities = client.request_identities().await.unwrap();
    assert_eq!(identities.len(), 1);
    let agent_key = identities[0].public_key().into_owned();
    assert_eq!(agent_key.to_bytes().unwrap(), expected.to_bytes().unwrap());
    assert_eq!(
        identities[0].comment(),
        "private comment must not reach DTO"
    );
    assert_eq!(signed.load(Ordering::SeqCst), 0);
    let data = b"isolated named-pipe signing fixture";
    let signature = client
        .sign_request_signature(&agent_key, None, data)
        .await
        .unwrap();
    russh::keys::signature::Verifier::verify(&expected, data, &signature).unwrap();
    assert_eq!(signed.load(Ordering::SeqCst), 1);
    drop(client);
    bridge.abort();
}

async fn ssh_agent_session(
    handler: impl russh::server::Handler<Error = russh::Error> + Send + 'static,
) -> (
    Handle<IdentityHandler>,
    tokio::task::JoinHandle<()>,
    tempfile::TempDir,
) {
    use russh::{client, server};
    let server_key = PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).unwrap();
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
            auth_rejection_time: Duration::ZERO,
            auth_rejection_time_initial: Some(Duration::ZERO),
            ..Default::default()
        });
        let (socket, _) = listener.accept().await.unwrap();
        let session = server::run_stream(config, socket, handler).await.unwrap();
        let _ = session.await;
    });
    let handle = client::connect(
        Arc::new(client::Config::default()),
        address,
        IdentityHandler::verified(trust),
    )
    .await
    .unwrap();
    (handle, server, temp)
}

#[tokio::test]
async fn agent_signature_authenticates_a_real_ssh_session_and_rejection_is_terminal() {
    use russh::server;
    struct ServerHandler;
    impl server::Handler for ServerHandler {
        type Error = russh::Error;
        async fn auth_publickey(
            &mut self,
            _: &str,
            _: server::VerifiedPublicKey<'_>,
        ) -> Result<server::Auth, Self::Error> {
            Ok(server::Auth::Accept)
        }
    }
    for reject in [false, true] {
        let (mut handle, server, _temp) = ssh_agent_session(ServerHandler).await;
        let (agent, signed) = fixture(
            vec![PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).unwrap()],
            reject,
        );
        let connector = Arc::new(Connector {
            clients: Mutex::new(VecDeque::from([Ok(agent)])),
            ..Default::default()
        });
        let resolution = resolve_agent(SshAgentProvider::Auto, connector.clone()).await;
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
        server.abort();
    }
}

#[tokio::test]
async fn agent_obeys_latest_remaining_methods_before_offering_another_identity() {
    use russh::{client, server, MethodKind, MethodSet};
    struct ServerHandler {
        methods: Vec<MethodKind>,
        partial_success: bool,
        requests: usize,
    }
    impl server::Handler for ServerHandler {
        type Error = russh::Error;
        async fn auth_publickey(
            &mut self,
            _: &str,
            _: server::VerifiedPublicKey<'_>,
        ) -> Result<server::Auth, Self::Error> {
            self.requests += 1;
            let methods = if self.requests > 1 {
                vec![MethodKind::HostBased]
            } else {
                self.methods.clone()
            };
            Ok(server::Auth::Reject {
                proceed_with_methods: Some(MethodSet::from(methods.as_slice())),
                partial_success: self.partial_success,
            })
        }
    }
    for (methods, partial_success, expected_requests) in [
        (vec![MethodKind::KeyboardInteractive], false, 1),
        (vec![MethodKind::HostBased], false, 1),
        (
            vec![MethodKind::PublicKey, MethodKind::KeyboardInteractive],
            false,
            2,
        ),
        (vec![MethodKind::PublicKey], true, 1),
    ] {
        let first_methods = methods.clone();
        let (mut handle, server, _temp) = ssh_agent_session(ServerHandler {
            methods,
            partial_success,
            requests: 0,
        })
        .await;
        let (agent, signed) = fixture(
            (0..2)
                .map(|_| PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).unwrap())
                .collect(),
            false,
        );
        let connector = Arc::new(Connector {
            clients: Mutex::new(VecDeque::from([Ok(agent)])),
            ..Default::default()
        });
        let ready = resolve_agent(SshAgentProvider::Auto, connector)
            .await
            .ready
            .unwrap();
        let result = authenticate_ready(&mut handle, "fixture", ready)
            .await
            .unwrap();
        assert_eq!(
            signed.load(Ordering::SeqCst),
            expected_requests,
            "methods={first_methods:?}, partial={partial_success}"
        );
        match result {
            client::AuthResult::Failure {
                remaining_methods,
                partial_success: actual_partial,
            } => {
                let expected = if expected_requests == 2 {
                    vec![MethodKind::HostBased]
                } else {
                    first_methods
                };
                assert_eq!(remaining_methods, MethodSet::from(expected.as_slice()));
                assert_eq!(actual_partial, partial_success);
            }
            client::AuthResult::Success => {
                panic!("partial success must remain an unfinished authentication")
            }
        }
        handle
            .disconnect(russh::Disconnect::ByApplication, "", "en")
            .await
            .unwrap();
        server.abort();
    }
}

#[tokio::test]
async fn automatic_agent_authentication_has_a_five_identity_attempt_budget() {
    struct Reject;
    impl russh::server::Handler for Reject {
        type Error = russh::Error;
        async fn auth_publickey(
            &mut self,
            _: &str,
            _: russh::server::VerifiedPublicKey<'_>,
        ) -> Result<russh::server::Auth, Self::Error> {
            Ok(russh::server::Auth::Reject {
                proceed_with_methods: Some(russh::MethodSet::from(
                    &[russh::MethodKind::PublicKey][..],
                )),
                partial_success: false,
            })
        }
    }
    let (mut handle, server, _temp) = ssh_agent_session(Reject).await;
    let (agent, signed) = fixture(
        (0..8)
            .map(|_| PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).unwrap())
            .collect(),
        false,
    );
    let connector = Arc::new(Connector {
        clients: Mutex::new(VecDeque::from([Ok(agent)])),
        ..Default::default()
    });
    let ready = resolve_agent(SshAgentProvider::Auto, connector)
        .await
        .ready
        .unwrap();
    let _ = authenticate_ready(&mut handle, "fixture", ready).await;
    assert_eq!(
        signed.load(Ordering::SeqCst),
        5,
        "an Agent with many keys must not exhaust the server's authentication attempts"
    );
    server.abort();
}

struct AgentMfaAgentServer {
    stage: usize,
    received: Arc<Mutex<Vec<String>>>,
}

impl russh::server::Handler for AgentMfaAgentServer {
    type Error = russh::Error;

    async fn auth_none(&mut self, _: &str) -> Result<russh::server::Auth, Self::Error> {
        Ok(russh::server::Auth::Reject {
            proceed_with_methods: Some(russh::MethodSet::from(&[russh::MethodKind::PublicKey][..])),
            partial_success: false,
        })
    }

    async fn auth_publickey(
        &mut self,
        _: &str,
        key: russh::server::VerifiedPublicKey<'_>,
    ) -> Result<russh::server::Auth, Self::Error> {
        self.received
            .lock()
            .unwrap()
            .push(key.public_key.fingerprint(HashAlg::Sha256).to_string());
        if self.stage == 2 {
            return Ok(russh::server::Auth::Accept);
        }
        self.stage = 1;
        Ok(russh::server::Auth::Reject {
            proceed_with_methods: Some(russh::MethodSet::from(
                &[russh::MethodKind::KeyboardInteractive][..],
            )),
            partial_success: true,
        })
    }

    async fn auth_keyboard_interactive<'a>(
        &'a mut self,
        _: &str,
        _: &str,
        response: Option<russh::server::Response<'a>>,
    ) -> Result<russh::server::Auth, Self::Error> {
        let Some(response) = response else {
            return Ok(russh::server::Auth::Partial {
                name: "Between Agent keys".into(),
                instructions: String::new().into(),
                prompts: vec![("Verification code".into(), false)].into(),
            });
        };
        assert_eq!(
            response.map(|value| value.to_vec()).collect::<Vec<_>>(),
            vec![b"fixture-code".to_vec()]
        );
        self.stage = 2;
        Ok(russh::server::Auth::Reject {
            proceed_with_methods: Some(russh::MethodSet::from(&[russh::MethodKind::PublicKey][..])),
            partial_success: true,
        })
    }
}

#[tokio::test]
async fn agent_active_budget_excludes_human_mfa_wait_on_the_same_connection() {
    use crate::public_key_candidates::PublicKeyCandidates;
    use crate::transport_authentication::AuthenticationNegotiation;
    let received = Arc::new(Mutex::new(Vec::new()));
    let (mut handle, server, temp) = ssh_agent_session(AgentMfaAgentServer {
        stage: 0,
        received: received.clone(),
    })
    .await;
    let config = candidate_config(&temp, crate::SshAuthType::AutoPublicKey);
    let trust = crate::host_trust::HostTrust::load(&config).await.unwrap();
    let keys = (0..2)
        .map(|_| PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).unwrap())
        .collect();
    let (ready, signatures) = ready_fixture(keys, false).await;
    let candidates = PublicKeyCandidates::from_sources(&config, Some(ready), vec![]).unwrap();
    let mut negotiation = AuthenticationNegotiation::with_public_keys(candidates);
    let round = negotiation
        .begin(&mut handle, &config, &trust, None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        round.kind,
        crate::SshAuthenticationKind::KeyboardInteractive
    );
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(61)).await;
    tokio::time::resume();
    let result = negotiation
        .respond(
            &mut handle,
            &config,
            &trust,
            None,
            vec!["fixture-code".into()],
        )
        .await;
    assert!(
        matches!(&result, Ok(None)),
        "human MFA wait must not exhaust the Agent signing budget: {:?}",
        result.as_ref().err().map(ToString::to_string)
    );
    assert_eq!(signatures.load(Ordering::SeqCst), 2);
    let actual = received.lock().unwrap();
    assert_eq!(actual.len(), 2);
    assert_ne!(actual[0], actual[1]);
    server.abort();
}

struct HeldAgentReplyServer {
    entered: tokio::sync::mpsc::UnboundedSender<()>,
    release: Arc<tokio::sync::Semaphore>,
}

impl russh::server::Handler for HeldAgentReplyServer {
    type Error = russh::Error;

    async fn auth_none(&mut self, _: &str) -> Result<russh::server::Auth, Self::Error> {
        Ok(russh::server::Auth::Reject {
            proceed_with_methods: Some(russh::MethodSet::from(&[russh::MethodKind::PublicKey][..])),
            partial_success: false,
        })
    }

    async fn auth_publickey(
        &mut self,
        _: &str,
        _: russh::server::VerifiedPublicKey<'_>,
    ) -> Result<russh::server::Auth, Self::Error> {
        self.entered.send(()).unwrap();
        self.release.acquire().await.unwrap().forget();
        Ok(russh::server::Auth::Reject {
            proceed_with_methods: Some(russh::MethodSet::from(&[russh::MethodKind::PublicKey][..])),
            partial_success: false,
        })
    }
}

#[tokio::test]
async fn agent_active_budget_remains_cumulative_across_public_key_calls() {
    let (entered, mut observed) = tokio::sync::mpsc::unbounded_channel();
    let release = Arc::new(tokio::sync::Semaphore::new(0));
    let (mut handle, server, _temp) = ssh_agent_session(HeldAgentReplyServer {
        entered,
        release: release.clone(),
    })
    .await;
    let keys = (0..2)
        .map(|_| PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).unwrap())
        .collect();
    let (mut ready, signatures) = ready_fixture(keys, false).await;
    let attempt = tokio::spawn(async move {
        let first = ready.take_next().unwrap();
        ready
            .authenticate_identity(&mut handle, "fixture", first)
            .await?;
        let second = ready.take_next().unwrap();
        ready
            .authenticate_identity(&mut handle, "fixture", second)
            .await
    });
    observed.recv().await.unwrap();
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(40)).await;
    tokio::time::resume();
    release.add_permits(1);
    observed.recv().await.unwrap();
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(21)).await;
    tokio::time::resume();
    release.add_permits(1);
    let result = attempt.await.unwrap();
    let error = result
        .err()
        .expect("two active waits totaling 61 seconds must exceed the shared 60-second budget");
    assert!(error.to_string().contains("exceeded 60 seconds"), "{error}");
    assert_eq!(signatures.load(Ordering::SeqCst), 2);
    server.abort();
}

#[derive(Clone)]
struct CandidateServer {
    received: Arc<Mutex<Vec<String>>>,
    accepted: Option<String>,
    partial: bool,
    remove_publickey: bool,
}

impl russh::server::Handler for CandidateServer {
    type Error = russh::Error;
    async fn auth_none(&mut self, _: &str) -> Result<russh::server::Auth, Self::Error> {
        Ok(russh::server::Auth::Reject {
            proceed_with_methods: Some(russh::MethodSet::from(&[russh::MethodKind::PublicKey][..])),
            partial_success: false,
        })
    }
    async fn auth_publickey(
        &mut self,
        _: &str,
        key: russh::server::VerifiedPublicKey<'_>,
    ) -> Result<russh::server::Auth, Self::Error> {
        let fingerprint = key.public_key.fingerprint(HashAlg::Sha256).to_string();
        self.received.lock().unwrap().push(fingerprint.clone());
        if self.accepted.as_ref() == Some(&fingerprint) {
            return Ok(russh::server::Auth::Accept);
        }
        let method = if self.remove_publickey {
            russh::MethodKind::HostBased
        } else {
            russh::MethodKind::PublicKey
        };
        Ok(russh::server::Auth::Reject {
            proceed_with_methods: Some(russh::MethodSet::from(&[method][..])),
            partial_success: self.partial,
        })
    }
}

fn candidate_config(
    temp: &tempfile::TempDir,
    auth_type: crate::SshAuthType,
) -> crate::SshConnectionConfig {
    let known_hosts = temp.path().join("known_hosts");
    let entry = std::fs::read_to_string(&known_hosts).unwrap();
    let port = entry
        .split("]:")
        .nth(1)
        .unwrap()
        .split(' ')
        .next()
        .unwrap()
        .parse()
        .unwrap();
    let mut config = crate::SshConnectionConfig::new(
        "candidate fixture",
        "127.0.0.1",
        port,
        "fixture",
        auth_type,
    );
    config.known_hosts_path = Some(known_hosts.to_str().unwrap().into());
    config
}

async fn ready_fixture(keys: Vec<PrivateKey>, reject: bool) -> (ReadyAgent, Arc<AtomicUsize>) {
    let (agent, signatures) = fixture(keys, reject);
    let connector = Arc::new(Connector {
        clients: Mutex::new(VecDeque::from([Ok(agent)])),
        ..Default::default()
    });
    (
        resolve_agent(SshAgentProvider::Auto, connector)
            .await
            .ready
            .unwrap(),
        signatures,
    )
}

#[tokio::test]
async fn real_negotiator_keeps_one_budget_and_never_repeats_an_identity_after_partial_success() {
    use crate::public_key_candidates::PublicKeyCandidates;
    use crate::transport_authentication::AuthenticationNegotiation;
    for partial in [false, true] {
        let received = Arc::new(Mutex::new(Vec::new()));
        let (mut handle, server, temp) = ssh_agent_session(CandidateServer {
            received: received.clone(),
            accepted: None,
            partial,
            remove_publickey: false,
        })
        .await;
        let config = candidate_config(&temp, crate::SshAuthType::AutoPublicKey);
        let trust = crate::host_trust::HostTrust::load(&config).await.unwrap();
        let keys = (0..8)
            .map(|_| PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).unwrap())
            .collect();
        let (ready, signatures) = ready_fixture(keys, false).await;
        let candidates = PublicKeyCandidates::from_sources(&config, Some(ready), vec![]).unwrap();
        let error = AuthenticationNegotiation::with_public_keys(candidates)
            .begin(&mut handle, &config, &trust, None)
            .await
            .err()
            .expect("five rejected/partial identities do not complete authentication");
        assert!(error.to_string().contains("5/5"), "{error}");
        let actual = received.lock().unwrap();
        assert_eq!(actual.len(), 5);
        assert_eq!(
            actual
                .iter()
                .collect::<std::collections::HashSet<_>>()
                .len(),
            5
        );
        assert_eq!(signatures.load(Ordering::SeqCst), 5);
        server.abort();
    }
}

#[tokio::test]
async fn selected_agent_identity_is_the_only_key_offered_and_disappearance_never_falls_back() {
    use crate::public_key_candidates::PublicKeyCandidates;
    use crate::transport_authentication::AuthenticationNegotiation;
    let keys: Vec<_> = (0..8)
        .map(|_| PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).unwrap())
        .collect();
    let selected = keys[7]
        .public_key()
        .fingerprint(HashAlg::Sha256)
        .to_string();
    let received = Arc::new(Mutex::new(Vec::new()));
    let (mut handle, server, temp) = ssh_agent_session(CandidateServer {
        received: received.clone(),
        accepted: Some(selected.clone()),
        partial: false,
        remove_publickey: false,
    })
    .await;
    let mut config = candidate_config(&temp, crate::SshAuthType::Agent);
    config.agent_fingerprint = Some(selected.clone());
    let trust = crate::host_trust::HostTrust::load(&config).await.unwrap();
    let (ready, signatures) = ready_fixture(keys, false).await;
    let candidates = PublicKeyCandidates::from_sources(&config, Some(ready), vec![]).unwrap();
    assert!(AuthenticationNegotiation::with_public_keys(candidates)
        .begin(&mut handle, &config, &trust, None)
        .await
        .unwrap()
        .is_none());
    assert_eq!(*received.lock().unwrap(), vec![selected]);
    assert_eq!(signatures.load(Ordering::SeqCst), 1);
    let (ready, signatures) = ready_fixture(
        vec![PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).unwrap()],
        false,
    )
    .await;
    assert!(PublicKeyCandidates::from_sources(&config, Some(ready), vec![]).is_err());
    assert_eq!(signatures.load(Ordering::SeqCst), 0);
    server.abort();
}

#[tokio::test]
async fn automatic_files_skip_malformed_and_duplicate_keys_and_resume_encrypted_key_on_same_socket()
{
    use crate::public_key_candidates::PublicKeyCandidates;
    use crate::transport_authentication::AuthenticationNegotiation;
    for encrypted in [false, true] {
        let agent_key = PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).unwrap();
        let file_key = PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).unwrap();
        let selected = file_key
            .public_key()
            .fingerprint(HashAlg::Sha256)
            .to_string();
        let received = Arc::new(Mutex::new(Vec::new()));
        let (mut handle, server, temp) = ssh_agent_session(CandidateServer {
            received: received.clone(),
            accepted: Some(selected.clone()),
            partial: false,
            remove_publickey: false,
        })
        .await;
        let config = candidate_config(&temp, crate::SshAuthType::AutoPublicKey);
        let trust = crate::host_trust::HostTrust::load(&config).await.unwrap();
        let malformed = temp.path().join("id_ed25519");
        let duplicate = temp.path().join("id_rsa");
        let accepted = temp.path().join("id_ecdsa");
        std::fs::write(&malformed, "malformed private key").unwrap();
        agent_key
            .write_openssh_file(&duplicate, Default::default())
            .unwrap();
        let file_key = if encrypted {
            file_key
                .encrypt(&mut rand::rng(), "attempt-only-unlock")
                .unwrap()
        } else {
            file_key
        };
        file_key
            .write_openssh_file(&accepted, Default::default())
            .unwrap();
        let (ready, signatures) = ready_fixture(vec![agent_key.clone(), agent_key], false).await;
        let candidates = PublicKeyCandidates::from_sources(
            &config,
            Some(ready),
            vec![malformed, duplicate, accepted]
                .iter()
                .map(|path| path.to_str().unwrap().into())
                .collect(),
        )
        .unwrap();
        let mut negotiation = AuthenticationNegotiation::with_public_keys(candidates);
        let round = negotiation
            .begin(&mut handle, &config, &trust, None)
            .await
            .unwrap();
        if encrypted {
            let round = round.unwrap();
            assert_eq!(round.kind, crate::SshAuthenticationKind::PrivateKey);
            assert!(round.name.ends_with("id_ecdsa"));
            assert_eq!(received.lock().unwrap().len(), 1);
            assert!(negotiation
                .respond(
                    &mut handle,
                    &config,
                    &trust,
                    None,
                    vec!["attempt-only-unlock".into()]
                )
                .await
                .unwrap()
                .is_none());
        } else {
            assert!(round.is_none());
        }
        assert_eq!(received.lock().unwrap().last(), Some(&selected));
        assert_eq!(received.lock().unwrap().len(), 2);
        assert_eq!(signatures.load(Ordering::SeqCst), 1);
        assert!(config.password.is_none());
        server.abort();
    }
}

#[tokio::test]
async fn automatic_candidates_reserve_file_slots_and_stop_on_server_method_removal_or_signing_refusal(
) {
    use crate::public_key_candidates::PublicKeyCandidates;
    use crate::transport_authentication::AuthenticationNegotiation;
    for (remove_publickey, refuse_signature) in [(false, false), (true, false), (false, true)] {
        let keys: Vec<_> = (0..11)
            .map(|_| PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).unwrap())
            .collect();
        let selected = keys[10]
            .public_key()
            .fingerprint(HashAlg::Sha256)
            .to_string();
        let received = Arc::new(Mutex::new(Vec::new()));
        let (mut handle, server, temp) = ssh_agent_session(CandidateServer {
            received: received.clone(),
            accepted: Some(selected.clone()),
            partial: false,
            remove_publickey,
        })
        .await;
        let config = candidate_config(&temp, crate::SshAuthType::AutoPublicKey);
        let trust = crate::host_trust::HostTrust::load(&config).await.unwrap();
        let mut files = vec![];
        for (index, key) in keys[8..].iter().enumerate() {
            let path = temp.path().join(format!("default-{index}"));
            key.write_openssh_file(&path, Default::default()).unwrap();
            files.push(path.to_str().unwrap().into());
        }
        let (ready, signatures) = ready_fixture(keys[..8].to_vec(), refuse_signature).await;
        let candidates = PublicKeyCandidates::from_sources(&config, Some(ready), files).unwrap();
        let result = AuthenticationNegotiation::with_public_keys(candidates)
            .begin(&mut handle, &config, &trust, None)
            .await;
        if remove_publickey || refuse_signature {
            assert!(result.is_err());
            assert_eq!(signatures.load(Ordering::SeqCst), 1);
            assert_eq!(
                received.lock().unwrap().len(),
                usize::from(!refuse_signature)
            );
        } else {
            assert!(result.unwrap().is_none());
            let received = received.lock().unwrap();
            assert_eq!(
                received.len(),
                5,
                "two Agent identities plus three files share one budget"
            );
            assert_eq!(received.last(), Some(&selected));
            assert_eq!(signatures.load(Ordering::SeqCst), 2);
        }
        server.abort();
    }
}
