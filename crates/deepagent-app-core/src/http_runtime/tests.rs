use super::*;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

struct Resolver {
    answers: Mutex<VecDeque<Vec<IpAddr>>>,
    calls: AtomicUsize,
}

#[async_trait]
impl AddressResolver for Resolver {
    async fn resolve(&self, host: &str, port: u16) -> Result<Vec<SocketAddr>> {
        if host == "ipv4only.arpa" {
            return Ok(vec![SocketAddr::new("192.0.0.170".parse().unwrap(), port)]);
        }
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(self
            .answers
            .lock()
            .unwrap()
            .pop_front()
            .expect("unexpected DNS lookup")
            .into_iter()
            .map(|ip| SocketAddr::new(ip, port))
            .collect())
    }
}

fn resolver(answers: &[&[&str]]) -> Arc<Resolver> {
    Arc::new(Resolver {
        answers: Mutex::new(
            answers
                .iter()
                .map(|answer| answer.iter().map(|ip| ip.parse().unwrap()).collect())
                .collect(),
        ),
        calls: AtomicUsize::new(0),
    })
}

fn request(url: String) -> HttpRequest {
    serde_json::from_value(serde_json::json!({"url":url})).unwrap()
}

/// Share the existing public-policy/local-transport seam with media tests.
/// DNS and transport are synthetic; production builds contain neither override.
pub(crate) fn public_fixture_client(address: SocketAddr) -> PublicHttpClient {
    public_fixture_client_with_answers(address, &[&["8.8.8.8"]])
}

pub(crate) fn public_fixture_client_with_answers(
    address: SocketAddr,
    answers: &[&[&str]],
) -> PublicHttpClient {
    assert!(address.ip().is_loopback());
    PublicHttpClient {
        resolver: resolver(answers),
        fixture_route: Some(address),
    }
}

struct Fixture {
    address: SocketAddr,
    requests: Arc<Mutex<Vec<Vec<u8>>>>,
    task: tokio::task::JoinHandle<()>,
}

impl Fixture {
    async fn new(responses: Vec<Vec<u8>>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let captured = requests.clone();
        let task = tokio::spawn(async move {
            for response in responses {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut bytes = Vec::new();
                loop {
                    let mut chunk = [0; 4096];
                    let n = stream.read(&mut chunk).await.unwrap();
                    if n == 0 {
                        break;
                    }
                    bytes.extend_from_slice(&chunk[..n]);
                    if let Some(end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
                        let head = String::from_utf8_lossy(&bytes[..end]);
                        let body = head
                            .lines()
                            .find_map(|line| {
                                line.to_ascii_lowercase()
                                    .strip_prefix("content-length: ")
                                    .and_then(|value| value.parse::<usize>().ok())
                            })
                            .unwrap_or(0);
                        if bytes.len() >= end + 4 + body {
                            break;
                        }
                    }
                }
                captured.lock().unwrap().push(bytes);
                let _ = stream.write_all(&response).await;
                let _ = stream.shutdown().await;
            }
        });
        Self {
            address,
            requests,
            task,
        }
    }

    fn url(&self) -> String {
        format!(
            "http://public.fixture.invalid:{}/resource",
            self.address.port()
        )
    }

    fn client(&self, resolver: Arc<dyn AddressResolver>) -> PublicHttpClient {
        PublicHttpClient {
            resolver,
            fixture_route: Some(self.address),
        }
    }

    async fn stop(self) {
        if !self.task.is_finished() {
            self.task.abort();
        }
        if let Err(error) = self.task.await {
            assert!(error.is_cancelled());
        }
    }
}

fn response(status: &str, headers: &str, body: &[u8]) -> Vec<u8> {
    let mut bytes = format!("HTTP/1.1 {status}\r\nConnection: close\r\n{headers}\r\n").into_bytes();
    bytes.extend_from_slice(body);
    bytes
}

#[test]
fn private_special_and_encoded_destinations_are_denied_before_io() {
    for host in [
        "0.0.0.0",
        "10.2.3.4",
        "100.100.100.200",
        "127.0.0.2",
        "169.254.169.254",
        "172.16.0.1",
        "172.31.255.255",
        "192.168.1.1",
        "192.0.2.1",
        "198.18.1.1",
        "224.0.0.1",
        "255.255.255.255",
        "2130706433",
        "0x7f000001",
        "0177.0.0.1",
        "[::1]",
        "[::]",
        "[::ffff:127.0.0.1]",
        "[fc00::1]",
        "[fe80::1]",
        "[ff02::1]",
        "[64:ff9b::7f00:1]",
        "[2002:7f00:1::]",
        "[2001:db8::1]",
        "[3fff::1]",
    ] {
        assert!(
            PublicHttpClient::default()
                .validate(&request(format!("http://{host}/")))
                .is_err(),
            "{host}"
        );
    }
    for url in [
        "file:///x",
        "ftp://example.com/",
        "http://u:pass@example.com/",
        "http://example.com/#fragment",
    ] {
        assert!(parse_url(url).is_err(), "{url}");
    }
    assert!(public_ip("8.8.8.8".parse().unwrap()));
    assert!(public_ip("2606:4700:4700::1111".parse().unwrap()));
}

#[tokio::test]
async fn mixed_dns_answers_are_all_rejected_before_connection() {
    let fixture = Fixture::new(vec![]).await;
    for answer in [
        &["8.8.8.8", "127.0.0.1"][..],
        &["8.8.8.8", "fd00::1"][..],
        &[][..],
    ] {
        let client = fixture.client(resolver(&[answer]));
        assert!(client
            .request(request(fixture.url()), ToolExecutionContext::default())
            .await
            .unwrap_err()
            .to_string()
            .contains("NetworkDenied"));
    }
    assert!(fixture.requests.lock().unwrap().is_empty());
    fixture.stop().await;
}

#[tokio::test]
async fn public_request_is_pinned_and_preserves_method_body_headers_status() {
    let fixture = Fixture::new(vec![response(
        "201 Created",
        "Content-Length: 11\r\nX-Reply: yes\r\n",
        b"{\"ok\":true}",
    )])
    .await;
    let dns = resolver(&[&["8.8.8.8"], &["127.0.0.1"]]);
    let client = fixture.client(dns.clone());
    let mut input = request(fixture.url());
    input.method = "POST".into();
    input.body = "synthetic-body".into();
    input.headers.insert("X-Explicit".into(), "yes".into());
    let output = client
        .request(input, ToolExecutionContext::default())
        .await
        .unwrap();
    assert_eq!(output["status_code"], 201);
    assert_eq!(output["body"]["ok"], true);
    assert_eq!(output["headers"]["x-reply"], "yes");
    assert_eq!(
        dns.calls.load(Ordering::SeqCst),
        1,
        "connection must not re-resolve an already checked hostname"
    );
    let captured = fixture.requests.lock().unwrap().clone();
    assert_eq!(captured.len(), 1);
    let wire = String::from_utf8_lossy(&captured[0]);
    assert!(wire.starts_with("POST /resource HTTP/1.1"));
    assert!(wire.contains("synthetic-body"));
    assert!(wire.contains("x-explicit: yes"));
    assert!(wire.contains("host: public.fixture.invalid:"));
    fixture.stop().await;
}

#[tokio::test]
async fn redirect_rechecks_dns_and_blocks_rebinding_and_private_locations() {
    for location in [
        "/next",
        "http://127.0.0.1/private",
        "http://other.fixture.invalid/",
        "https://public.fixture.invalid/",
    ] {
        let fixture = Fixture::new(vec![response(
            "302 Found",
            &format!("Location: {location}\r\nContent-Length: 0\r\n"),
            b"",
        )])
        .await;
        let dns = resolver(&[&["8.8.8.8"], &["127.0.0.1"]]);
        assert!(fixture
            .client(dns)
            .request(request(fixture.url()), ToolExecutionContext::default())
            .await
            .unwrap_err()
            .to_string()
            .contains("NetworkDenied"));
        assert_eq!(
            fixture.requests.lock().unwrap().len(),
            1,
            "no request to an unapproved next hop"
        );
        fixture.stop().await;
    }
}

#[tokio::test]
async fn same_origin_public_redirect_uses_the_same_checked_broker() {
    let fixture = Fixture::new(vec![
        response("302 Found", "Location: /next\r\nContent-Length: 0\r\n", b""),
        response("200 OK", "Content-Length: 2\r\n", b"OK"),
    ])
    .await;
    let dns = resolver(&[&["8.8.8.8"], &["8.8.4.4"]]);
    let output = fixture
        .client(dns.clone())
        .request(request(fixture.url()), ToolExecutionContext::default())
        .await
        .unwrap();
    assert_eq!(output["body"], "OK");
    assert_eq!(dns.calls.load(Ordering::SeqCst), 2);
    assert_eq!(fixture.requests.lock().unwrap().len(), 2);
    fixture.stop().await;
}

#[tokio::test]
async fn request_and_declared_or_chunked_response_limits_fail_loudly() {
    let mut oversized = request("https://public.fixture.invalid/".into());
    oversized.body = "x".repeat(deepagent_builtins::http_request::MAX_REQUEST_BYTES + 1);
    assert!(PublicHttpClient::default().validate(&oversized).is_err());
    let body = vec![b'x'; MAX_RESPONSE_BYTES + 1];
    let chunked = [
        format!("{:x}\r\n", body.len()).into_bytes(),
        body,
        b"\r\n0\r\n\r\n".to_vec(),
    ]
    .concat();
    for bytes in [
        response("200 OK", "Content-Length: 1048577\r\n", b""),
        response("200 OK", "Transfer-Encoding: chunked\r\n", &chunked),
    ] {
        let fixture = Fixture::new(vec![bytes]).await;
        assert!(fixture
            .client(resolver(&[&["8.8.8.8"]]))
            .request(request(fixture.url()), ToolExecutionContext::default())
            .await
            .unwrap_err()
            .to_string()
            .contains("HttpResponseTooLarge"));
        fixture.stop().await;
    }
}

#[tokio::test]
async fn ambiguous_response_failure_is_not_retried_even_for_get() {
    let fixture = Fixture::new(vec![response("200 OK", "Content-Length: 50\r\n", b"short")]).await;
    let mut input = request(fixture.url());
    input.retry_count = 3;
    let dns = resolver(&[&["8.8.8.8"]]);
    assert!(fixture
        .client(dns.clone())
        .request(input, ToolExecutionContext::default())
        .await
        .unwrap_err()
        .to_string()
        .contains("HttpOutcomeUnknown"));
    assert_eq!(dns.calls.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.requests.lock().unwrap().len(), 1);
    fixture.stop().await;
}

struct NeverResolver;

#[tokio::test]
async fn network_specific_dns64_checks_all_six_prefix_lengths() {
    struct Dns64 {
        source: Ipv6Addr,
        destination: Ipv6Addr,
    }
    #[async_trait]
    impl AddressResolver for Dns64 {
        async fn resolve(&self, host: &str, port: u16) -> Result<Vec<SocketAddr>> {
            Ok(vec![SocketAddr::new(
                IpAddr::V6(if host == "ipv4only.arpa" {
                    self.source
                } else {
                    self.destination
                }),
                port,
            )])
        }
    }
    // Explicit independent RFC 6052 layouts, not encoded with the production
    // extraction helper. Each pair embeds 127.0.0.1 / 8.8.8.8 respectively.
    for (prefix, source, private, public) in [
        (
            4,
            "2606:4700:c000:aa::",
            "2606:4700:7f00:1::",
            "2606:4700:808:808::",
        ),
        (
            5,
            "2606:4700:12c0:0:aa::",
            "2606:4700:127f:0:1::",
            "2606:4700:1208:808:8::",
        ),
        (
            6,
            "2606:4700:1234:c000:0:aa00::",
            "2606:4700:1234:7f00:0:100::",
            "2606:4700:1234:808:8:800::",
        ),
        (
            7,
            "2606:4700:1234:56c0:0:aa::",
            "2606:4700:1234:567f:0:1::",
            "2606:4700:1234:5608:8:808::",
        ),
        (
            8,
            "2606:4700:1234:5678:c0:0:aa00:0",
            "2606:4700:1234:5678:7f:0:100:0",
            "2606:4700:1234:5678:8:808:800:0",
        ),
        (
            12,
            "2606:4700:1234:5678:abcd:ef01:c000:aa",
            "2606:4700:1234:5678:abcd:ef01:7f00:1",
            "2606:4700:1234:5678:abcd:ef01:808:808",
        ),
    ] {
        let source = source.parse::<Ipv6Addr>().unwrap();
        assert_eq!(
            embedded_ipv4(source, prefix),
            Some(Ipv4Addr::new(192, 0, 0, 170))
        );
        for (address, allowed) in [(private, false), (public, true)] {
            let destination = address.parse::<Ipv6Addr>().unwrap();
            assert!(
                public_ip(IpAddr::V6(destination)),
                "must exercise the DNS64 guard"
            );
            let client = PublicHttpClient {
                resolver: Arc::new(Dns64 {
                    source,
                    destination,
                }),
                fixture_route: None,
            };
            for url in [
                "https://synthetic.invalid/".to_string(),
                format!("https://[{address}]/"),
            ] {
                assert_eq!(
                    client.addresses(&parse_url(&url).unwrap()).await.is_ok(),
                    allowed,
                    "prefix={prefix} address={address}"
                );
            }
        }
    }
}

#[tokio::test]
async fn cancellation_and_deadline_interrupt_a_partially_read_response_without_replay() {
    for cancel in [true, false] {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
        let (done_tx, done_rx) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut bytes = [0; 4096];
            assert!(stream.read(&mut bytes).await.unwrap() > 0);
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 999\r\n\r\npartial")
                .await
                .unwrap();
            ready_tx.send(()).unwrap();
            done_rx.await.unwrap();
            assert!(
                tokio::time::timeout(Duration::from_millis(100), listener.accept())
                    .await
                    .is_err(),
                "an already dispatched read cannot be replayed"
            );
        });
        let client = PublicHttpClient {
            resolver: resolver(&[&["8.8.8.8"]]),
            fixture_route: Some(address),
        };
        let mut input = request(format!("http://synthetic.invalid:{}/", address.port()));
        input.retry_count = 3;
        let context = ToolExecutionContext::default().with_timeout(Duration::from_millis(300));
        let token = context.cancellation_token();
        let worker = tokio::spawn(async move { client.request(input, context).await });
        tokio::time::timeout(Duration::from_secs(2), ready_rx)
            .await
            .unwrap()
            .unwrap();
        if cancel {
            token.cancel();
        }
        let error = tokio::time::timeout(Duration::from_secs(2), worker)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err();
        assert!(error.to_string().contains(if cancel {
            "HttpCancelled"
        } else {
            "HttpDeadlineExceeded"
        }));
        done_tx.send(()).unwrap();
        server.await.unwrap();
    }
}
#[async_trait]
impl AddressResolver for NeverResolver {
    async fn resolve(&self, _: &str, _: u16) -> Result<Vec<SocketAddr>> {
        std::future::pending().await
    }
}

#[tokio::test]
async fn cancellation_and_deadline_cover_dns_not_only_socket_reads() {
    let client = PublicHttpClient {
        resolver: Arc::new(NeverResolver),
        fixture_route: None,
    };
    let context = ToolExecutionContext::default();
    let signal = context.cancellation_token();
    let work = client.request(request("https://fixture.invalid/".into()), context);
    tokio::pin!(work);
    tokio::select! { result = &mut work => panic!("unexpected result: {result:?}"), _ = tokio::task::yield_now() => {} }
    signal.cancel();
    assert!(tokio::time::timeout(Duration::from_secs(1), work)
        .await
        .unwrap()
        .unwrap_err()
        .to_string()
        .contains("HttpCancelled"));
    let context = ToolExecutionContext::default().with_timeout(Duration::from_millis(10));
    assert!(client
        .request(request("https://fixture.invalid/".into()), context)
        .await
        .unwrap_err()
        .to_string()
        .contains("HttpDeadlineExceeded"));
}

#[tokio::test]
async fn formal_workflow_real_http_approval_and_sqlite_reopen_do_not_replay() {
    use crate::{AppService, ChatService, MemorySecretStore, SettingsService};
    use deepagent_models::transport::MockTransport;
    use deepagent_persistence::{run_control::RunControlStore, run_store::RunStore, Database};
    use deepagent_runtime::workflow::WorkflowRequest;

    let fixture = Fixture::new(vec![response(
        "201 Created",
        "Content-Length: 7\r\nX-Reply: yes\r\n",
        b"CREATED",
    )])
    .await;
    let root = tempfile::tempdir().unwrap();
    let db_path = root.path().join("http.db");
    let db = Arc::new(Database::open(&db_path).unwrap());
    let settings = Arc::new(SettingsService::new(
        db.clone(),
        Arc::new(MockTransport::with_get_json(
            r#"{"data":[{"id":"deepseek-v4-flash","object":"model","owned_by":"deepseek"}]}"#,
        )),
        Arc::new(MemorySecretStore::new()),
    ));
    settings.initialize("sk-fixture-only").await.unwrap();
    let dns = resolver(&[&["8.8.8.8"]]);
    let chat = ChatService::new(
        db.clone(),
        settings.clone(),
        Arc::new(MockTransport::default()),
        root.path(),
    )
    .with_workflow_http_client(Arc::new(fixture.client(dns.clone())));
    let session = chat.create_session(None).unwrap();
    let workflow: WorkflowRequest = serde_json::from_value(serde_json::json!({
        "definition":{"version":1,"nodes":[{"id":"start","kind":"start"},
            {"id":"request","kind":"http-request","config":{"httpUrl":fixture.url(),"httpMethod":"POST","httpBody":"formal-payload"}},
            {"id":"end","kind":"end"}],"edges":[{"id":"a","source":"start","target":"request"},{"id":"b","source":"request","target":"end"}]}
    })).unwrap();
    let responder = chat.clone();
    let approvals = Arc::new(AtomicUsize::new(0));
    let count = approvals.clone();
    chat.run_workflow(
        workflow,
        "formal-http".into(),
        session.clone(),
        |_| {},
        move |approval| {
            assert_eq!(approval.tool, "http_request");
            assert_eq!(count.fetch_add(1, Ordering::SeqCst), 0);
            assert!(responder
                .resolve_approval(&approval.call_id, true, "fixture")
                .unwrap());
        },
    )
    .await
    .unwrap();
    assert_eq!(approvals.load(Ordering::SeqCst), 1);
    assert_eq!(dns.calls.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.requests.lock().unwrap().len(), 1);
    let ledger = RunStore::new(&db)
        .events_after("formal-http", None)
        .unwrap();
    let actions = RunControlStore::new(&db)
        .list_actions("formal-http")
        .unwrap();
    assert_eq!(actions.len(), 1);
    assert_eq!(actions[0].state.label(), "completed");
    let approvals_before = RunControlStore::new(&db)
        .list_approvals("formal-http")
        .unwrap();
    assert_eq!(approvals_before.len(), 1);
    drop(chat);
    drop(settings);
    drop(db);
    let reopened = Arc::new(Database::open(&db_path).unwrap());
    AppService::from_shared(reopened.clone())
        .recover_unfinished_runs()
        .unwrap();
    assert_eq!(
        RunStore::new(&reopened)
            .get("formal-http")
            .unwrap()
            .unwrap()
            .terminal_kind
            .as_deref(),
        Some("succeeded")
    );
    assert_eq!(
        RunStore::new(&reopened)
            .events_after("formal-http", None)
            .unwrap(),
        ledger
    );
    let cursor = ledger[ledger.len() / 2].sequence;
    assert_eq!(
        RunStore::new(&reopened)
            .events_after("formal-http", Some(cursor))
            .unwrap(),
        ledger
            .into_iter()
            .filter(|row| row.sequence > cursor)
            .collect::<Vec<_>>()
    );
    assert_eq!(
        RunControlStore::new(&reopened)
            .list_actions("formal-http")
            .unwrap(),
        actions
    );
    assert_eq!(
        RunControlStore::new(&reopened)
            .list_approvals("formal-http")
            .unwrap(),
        approvals_before
    );
    assert_eq!(
        fixture.requests.lock().unwrap().len(),
        1,
        "reopen, recovery and replay must not resubmit HTTP"
    );
    fixture.stop().await;
}
