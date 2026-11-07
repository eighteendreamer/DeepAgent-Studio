//! Only self-owned IPv4 loopback, actual CanvasModelGateway consumers.
use super::*;
use deepagent_app_core::canvas_model_gateway::{CanvasModelGateway, CanvasModelRequest};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[derive(Clone)]
struct Observed {
    route: String,
    old_auth: bool,
    new_auth: bool,
    task_id: Option<String>,
}

struct Server {
    url: String,
    stop: Arc<AtomicBool>,
    seen: Arc<Mutex<Vec<Observed>>>,
    worker: Option<std::thread::JoinHandle<()>>,
}

fn serve(stream: &mut TcpStream) -> std::io::Result<Observed> {
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(Duration::from_secs(3)))?;
    stream.set_write_timeout(Some(Duration::from_secs(3)))?;
    let mut bytes = Vec::new();
    let header_end;
    loop {
        let mut chunk = [0; 2048];
        let count = stream.read(&mut chunk)?;
        assert!(
            count > 0 && bytes.len() < 65536,
            "bounded synthetic HTTP request"
        );
        bytes.extend_from_slice(&chunk[..count]);
        if let Some(index) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            header_end = index + 4;
            break;
        }
    }
    let header = String::from_utf8_lossy(&bytes[..header_end]).to_ascii_lowercase();
    let length: usize = header
        .lines()
        .find_map(|line| line.strip_prefix("content-length:"))
        .map(str::trim)
        .unwrap_or("0")
        .parse()
        .unwrap();
    assert!(length < 65536);
    while bytes.len() < header_end + length {
        let mut chunk = [0; 2048];
        let count = stream.read(&mut chunk)?;
        assert!(count > 0);
        bytes.extend_from_slice(&chunk[..count]);
    }
    let route = header.lines().next().unwrap().to_string();
    let old_auth = header.contains(&format!("authorization: bearer {OLD}"));
    let new_auth = header.contains(&format!("authorization: bearer {NEW}"));
    let request: serde_json::Value =
        serde_json::from_slice(&bytes[header_end..]).unwrap_or_default();
    let task_id = request
        .get("requestId")
        .and_then(|value| value.as_str())
        .map(str::to_owned);
    let body = if route.starts_with("post /v1/video/submit ") {
        if old_auth {
            r#"{"requestId":"job-old"}"#
        } else {
            r#"{"requestId":"job-new"}"#
        }
    } else if route.starts_with("post /v1/video/status ") {
        r#"{"status":"Succeed","results":{"videos":[{"url":"http://127.0.0.1:1/synthetic-not-fetched.mp4"}]}}"#
    } else {
        r#"{"choices":[{"message":{"content":"H290 accepted"}}],"data":[{"id":"model"}]}"#
    };
    write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body)?;
    Ok(Observed {
        route,
        old_auth,
        new_auth,
        task_id,
    })
}

#[tokio::test]
async fn h290_video_resume_uses_committed_identity_and_never_polls_another_accounts_job() {
    use deepagent_runtime::workflow::{
        CanvasJobProgressSink, CanvasModelBridge, CanvasVideoRequest,
    };
    let video_input = |url: &str, key: Option<&str>| {
        let mut provider = input("first", url, key);
        provider.models[0]
            .scenarios
            .push(CanvasScenario::VideoGeneration);
        provider
    };
    for case in ["replace", "before", "after", "preserve", "same"] {
        let old = Server::new();
        let new = Server::new();
        let fixture = Fixture::new();
        fixture
            .service
            .save_provider(video_input(&old.url, Some(OLD)))
            .unwrap();
        let gateway = CanvasModelGateway::new(Arc::new(fixture.reopen()));
        let reports = Arc::new(Mutex::new(Vec::new()));
        let capture = reports.clone();
        let base = CanvasVideoRequest {
            model_ref: "first::model".into(),
            prompt: "H290 synthetic video".into(),
            size: Some("720p".into()),
            timeout_ms: 10_000,
            on_progress: Some(CanvasJobProgressSink::new(move |report| {
                capture.lock().unwrap().push(report)
            })),
            ..Default::default()
        };
        gateway.prepare_video(base.clone()).await.unwrap();
        let initial = reports.lock().unwrap()[0].clone();
        assert_eq!(initial.job_id, "job-old");
        assert_eq!(old.seen.lock().unwrap().len(), 2);
        let generation = fixture
            .service
            .provider("first")
            .unwrap()
            .credential_version;
        fixture.secrets.mode.store(
            match case {
                "before" => 1,
                "after" => 2,
                _ => 0,
            },
            Ordering::SeqCst,
        );
        let (url, key) = match case {
            "preserve" => (&old.url, None),
            "same" => (&old.url, Some(OLD)),
            _ => (&new.url, Some(NEW)),
        };
        let saved = fixture.service.save_provider(video_input(url, key));
        assert_eq!(saved.is_ok(), case != "before" && case != "after");
        assert_eq!(
            fixture
                .service
                .provider("first")
                .unwrap()
                .credential_version
                == generation,
            case != "replace"
        );
        reports.lock().unwrap().clear();
        gateway
            .prepare_video(CanvasVideoRequest {
                resume_task_id: Some(initial.job_id),
                resume_fingerprint: Some(initial.resume_fingerprint.clone()),
                ..base.clone()
            })
            .await
            .unwrap();
        let resumed = reports.lock().unwrap()[0].clone();
        if case == "replace" {
            assert_eq!(resumed.job_id, "job-new");
            assert_ne!(resumed.resume_fingerprint, initial.resume_fingerprint);
            assert_eq!(old.seen.lock().unwrap().len(), 2);
            let seen = new.seen.lock().unwrap();
            assert_eq!(seen.len(), 2);
            assert!(seen[0].route.starts_with("post /v1/video/submit "));
            assert!(seen
                .iter()
                .all(|request| request.new_auth && !request.old_auth));
            assert_eq!(seen[1].task_id.as_deref(), Some("job-new"));
        } else {
            assert_eq!(resumed.job_id, "job-old");
            assert_eq!(resumed.resume_fingerprint, initial.resume_fingerprint);
            assert!(new.seen.lock().unwrap().is_empty());
            let seen = old.seen.lock().unwrap();
            assert_eq!(seen.len(), 3, "resume must not submit again");
            assert!(seen
                .iter()
                .all(|request| request.old_auth && !request.new_auth));
            assert!(seen[2].route.starts_with("post /v1/video/status "));
            assert_eq!(seen[2].task_id.as_deref(), Some("job-old"));
        }
        fixture.service.clear_provider_api_key("first").unwrap();
        let before = old.seen.lock().unwrap().len() + new.seen.lock().unwrap().len();
        assert!(gateway.prepare_video(base).await.is_err());
        assert_eq!(
            old.seen.lock().unwrap().len() + new.seen.lock().unwrap().len(),
            before
        );
    }
}

#[tokio::test]
async fn h290_missing_reference_blocks_all_gateway_requests_instead_of_using_legacy_secret() {
    use deepagent_app_core::canvas_provider_service::provider_secret_key;
    let server = Server::new();
    let fixture = Fixture::new();
    fixture
        .service
        .save_provider(input("first", &server.url, Some(NEW)))
        .unwrap();
    fixture
        .secrets
        .inner
        .set(&provider_secret_key("first"), OLD)
        .unwrap();
    fixture
        .db
        .with_conn(|connection| {
            connection
                .execute(
                    "DELETE FROM secret_records WHERE name LIKE 'canvas:credential:v1:%'",
                    [],
                )
                .map_err(|error| CoreError::Persistence(error.to_string()))?;
            Ok(())
        })
        .unwrap();
    let gateway = CanvasModelGateway::new(Arc::new(fixture.reopen()));
    let request = CanvasModelRequest::text("H290 missing credential");
    assert!(gateway
        .execute_on("first", "model", &request)
        .await
        .is_err());
    assert!(gateway.execute(&request, None).await.is_err());
    assert!(gateway.discover_models("first").await.is_err());
    assert!(server.seen.lock().unwrap().is_empty());
}

impl Server {
    fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}/v1", listener.local_addr().unwrap());
        let stop = Arc::new(AtomicBool::new(false));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let (stopped, captured) = (stop.clone(), seen.clone());
        let worker = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(20);
            while !stopped.load(Ordering::SeqCst) && Instant::now() < deadline {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        captured.lock().unwrap().push(serve(&mut stream).unwrap())
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5))
                    }
                    Err(error) => panic!("synthetic listener error: {error}"),
                }
            }
        });
        Self {
            url,
            stop,
            seen,
            worker: Some(worker),
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[tokio::test]
async fn h290_gateway_explicit_candidates_and_discovery_use_only_committed_endpoint_key_pairs() {
    for mode in [0, 1, 2] {
        let old = Server::new();
        let new = Server::new();
        let fixture = Fixture::new();
        fixture
            .service
            .save_provider(input("first", &old.url, Some(OLD)))
            .unwrap();
        fixture.secrets.mode.store(mode, Ordering::SeqCst);
        let saved = fixture
            .service
            .save_provider(input("first", &new.url, Some(NEW)));
        assert_eq!(saved.is_ok(), mode == 0);
        let providers = Arc::new(fixture.reopen());
        let gateway = CanvasModelGateway::new(providers.clone());
        let request = CanvasModelRequest::text("H290 synthetic request");
        gateway
            .execute_on("first", "model", &request)
            .await
            .unwrap();
        gateway.execute(&request, None).await.unwrap();
        assert_eq!(gateway.discover_models("first").await.unwrap(), ["model"]);
        let (active, inactive) = if mode == 0 {
            (&new, &old)
        } else {
            (&old, &new)
        };
        let captured = active.seen.lock().unwrap().clone();
        assert_eq!(captured.len(), 3);
        assert!(
            inactive.seen.lock().unwrap().is_empty(),
            "no request to an unpublished endpoint"
        );
        assert!(
            captured
                .iter()
                .all(|seen| seen.new_auth == (mode == 0) && seen.old_auth == (mode != 0)),
            "wire authentication must match its published endpoint"
        );
        assert!(captured[0].route.starts_with("post /v1/chat/completions"));
        assert!(captured[1].route.starts_with("post /v1/chat/completions"));
        assert!(captured[2].route.starts_with("get /v1/models"));
        providers.clear_provider_api_key("first").unwrap();
        assert!(gateway
            .execute_on("first", "model", &request)
            .await
            .is_err());
        assert!(gateway.discover_models("first").await.is_err());
        assert_eq!(active.seen.lock().unwrap().len(), 3);
    }
}
