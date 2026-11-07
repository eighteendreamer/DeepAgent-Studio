//! Current scenario declarations must govern persisted bindings at dispatch time.

use deepagent_app_core::canvas_model_gateway::CanvasModelGateway;
use deepagent_app_core::canvas_provider_service::{
    CanvasModelConfig, CanvasProviderInput, CanvasProviderService, CanvasScenario,
    CanvasScenarioBinding,
};
use deepagent_app_core::MemorySecretStore;
use deepagent_persistence::Database;
use deepagent_runtime::workflow::{CanvasCompletionRequest, CanvasModelBridge};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[path = "canvas_scenario_binding/eligibility.rs"]
mod eligibility;

const PROVIDER: &str = "scenario-audit";
const FORMER: &str = "formerly-text";
const HEALTHY: &str = "healthy-text";

struct Fixture {
    base_url: String,
    reject_former: Arc<AtomicBool>,
    seen: Arc<Mutex<Vec<(String, String)>>>,
    hold: Arc<AtomicBool>,
    arrived: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
    task: tokio::task::JoinHandle<()>,
}

impl Fixture {
    async fn start() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("loopback listener");
        let base_url = format!("http://{}/v1", listener.local_addr().expect("address"));
        let reject_former = Arc::new(AtomicBool::new(false));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let reject = reject_former.clone();
        let sink = seen.clone();
        let hold = Arc::new(AtomicBool::new(false));
        let arrived = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let (held, received, released) = (hold.clone(), arrived.clone(), release.clone());
        let task = tokio::spawn(async move {
            loop {
                let (mut socket, _) = listener.accept().await.expect("accept");
                tokio::time::timeout(Duration::from_secs(3), async {
                    let mut bytes = Vec::new();
                    let body_start = loop {
                        let mut chunk = [0; 2048];
                        let n = socket.read(&mut chunk).await.expect("request bytes");
                        assert!(n > 0, "request ended before headers");
                        bytes.extend_from_slice(&chunk[..n]);
                        assert!(bytes.len() <= 65536, "fixture request cap");
                        if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                            break end + 4;
                        }
                    };
                    let headers = String::from_utf8_lossy(&bytes[..body_start]);
                    let path = headers.lines().next().expect("request line").to_owned();
                    let length: usize = headers
                        .lines()
                        .filter_map(|line| line.split_once(':'))
                        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                        .expect("content length")
                        .1
                        .trim()
                        .parse()
                        .expect("length");
                    assert!(body_start + length <= 65536, "fixture body cap");
                    while bytes.len() < body_start + length {
                        let mut chunk = [0; 2048];
                        let n = socket.read(&mut chunk).await.expect("body bytes");
                        assert!(n > 0, "request ended before body");
                        bytes.extend_from_slice(&chunk[..n]);
                    }
                    let raw = &bytes[body_start..body_start + length];
                    let body: Value = serde_json::from_slice(raw).unwrap_or(Value::Null);
                    let model = if path.starts_with("POST /v1/audio/transcriptions ") {
                        String::from_utf8_lossy(raw).split_once("name=\"model\"\r\n\r\n").unwrap().1.split("\r\n").next().unwrap().to_owned()
                    } else if path.starts_with("POST /v1/video/status ") {
                        body["requestId"].as_str().unwrap().trim_start_matches("h292-job-").to_owned()
                    } else { body["model"].as_str().expect("wire model").to_owned() };
                    sink.lock().expect("seen").push((path, model.clone()));
                    received.notify_one();
                    if held.swap(false, Ordering::SeqCst) { released.notified().await; }
                    let path = String::from_utf8_lossy(&bytes[..body_start]);
                    let (status, payload) = if model == FORMER && reject.load(Ordering::SeqCst) {
                        (
                            "400 Bad Request",
                            json!({"error":{"message":"synthetic unsupported text operation"}}),
                        )
                    } else if path.starts_with("POST /v1/embeddings ") {
                        ("200 OK", json!({"data":[{"index":0,"embedding":[1.0,2.0]}]}))
                    } else if path.starts_with("POST /v1/images/generations ") {
                        use base64::Engine as _;
                        ("200 OK", json!({"data":[{"b64_json":base64::engine::general_purpose::STANDARD.encode(b"\x89PNG\r\n\x1a\nh292")}]}))
                    } else if path.starts_with("POST /v1/audio/transcriptions ") {
                        ("200 OK", json!({"text":"synthetic transcription"}))
                    } else if path.starts_with("POST /v1/video/submit ") {
                        ("200 OK", json!({"requestId":format!("h292-job-{model}")}))
                    } else if path.starts_with("POST /v1/video/status ") {
                        ("200 OK", json!({"status":"Succeed","results":{"videos":[{"url":"https://public.fixture.invalid/h292.mp4"}]}}))
                    } else {
                        (
                            "200 OK",
                            json!({"choices":[{"message":{"content":format!("ok:{model}")}}]}),
                        )
                    };
                    let speech = path.starts_with("POST /v1/audio/speech ") && status == "200 OK";
                    let payload = if speech { "ID3h292-synthetic-speech".into() } else { payload.to_string() };
                    let mime = if speech { "audio/mpeg" } else { "application/json" };
                    let response = format!(
                        "HTTP/1.1 {status}\r\nContent-Type: {mime}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
                        payload.len()
                    );
                    socket.write_all(response.as_bytes()).await.expect("response");
                })
                .await
                .expect("bounded fixture exchange");
            }
        });
        Self {
            base_url,
            reject_former,
            seen,
            hold,
            arrived,
            release,
            task,
        }
    }

    fn take_seen(&self) -> Vec<(String, String)> {
        std::mem::take(&mut *self.seen.lock().expect("seen"))
    }

    async fn stop(&mut self) {
        self.task.abort();
        let result = (&mut self.task).await;
        assert!(result.expect_err("fixture cancellation").is_cancelled());
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn model(id: &str, scenarios: Vec<CanvasScenario>) -> CanvasModelConfig {
    CanvasModelConfig {
        id: id.to_owned(),
        name: id.to_owned(),
        description: String::new(),
        enabled: true,
        scenarios,
        priority: 0,
        max_reference_images: None,
    }
}

fn bindings() -> Vec<CanvasScenarioBinding> {
    [FORMER, HEALTHY]
        .into_iter()
        .map(|id| CanvasScenarioBinding {
            scenario: "text".to_owned(),
            provider_id: PROVIDER.to_owned(),
            model_id: id.to_owned(),
            enabled: true,
        })
        .collect()
}

fn provider_input(base_url: &str) -> CanvasProviderInput {
    CanvasProviderInput {
        id: PROVIDER.to_owned(),
        name: "Synthetic scenario fixture".to_owned(),
        protocol: "openai".to_owned(),
        base_url: base_url.to_owned(),
        enabled: true,
        models: vec![
            model(FORMER, vec![CanvasScenario::Text]),
            model(HEALTHY, vec![CanvasScenario::Text]),
        ],
        api_key: Some("synthetic-key".to_owned()),
        ..Default::default()
    }
}

async fn complete(service: Arc<CanvasProviderService>) -> Result<String, String> {
    let gateway = CanvasModelGateway::new(service);
    tokio::time::timeout(
        Duration::from_secs(5),
        gateway.complete(CanvasCompletionRequest {
            prompt: "synthetic scenario audit".to_owned(),
            ..Default::default()
        }),
    )
    .await
    .expect("bounded real model bridge")
    .map(|response| response.model_id)
    .map_err(|error| error.to_string())
}

#[tokio::test]
async fn current_scenario_controls_accept_valid_and_skip_disabled_or_unbound_models() {
    let mut fixture = Fixture::start().await;
    let dir = tempfile::tempdir().expect("tempdir");
    let service = Arc::new(CanvasProviderService::new(
        Arc::new(Database::open(dir.path().join("canvas.sqlite")).expect("db")),
        Arc::new(MemorySecretStore::new()),
    ));
    service
        .save_provider(provider_input(&fixture.base_url))
        .expect("provider");
    service.save_bindings(None, bindings()).expect("bindings");
    let valid = complete(service.clone()).await;
    let valid_seen = fixture.take_seen();

    let mut disabled = model(FORMER, vec![CanvasScenario::Text]);
    disabled.enabled = false;
    service.save_model(PROVIDER, disabled).expect("disable");
    let disabled_result = complete(service.clone()).await;
    let disabled_seen = fixture.take_seen();

    service
        .save_bindings(None, Vec::new())
        .expect("clear bindings");
    service
        .save_model(PROVIDER, model(FORMER, Vec::new()))
        .expect("revoke");
    let rejects_invalid = service.save_bindings(None, bindings()).is_err();
    let unbound = complete(service).await;
    let unbound_seen = fixture.take_seen();
    fixture.stop().await;

    assert_eq!(valid.as_deref(), Ok(FORMER));
    assert_eq!(disabled_result.as_deref(), Ok(HEALTHY));
    assert_eq!(unbound.as_deref(), Ok(HEALTHY));
    assert!(rejects_invalid);
    for (actual, expected) in [
        (valid_seen, FORMER),
        (disabled_seen, HEALTHY),
        (unbound_seen, HEALTHY),
    ] {
        assert_eq!(
            actual,
            vec![(
                "POST /v1/chat/completions HTTP/1.1".to_owned(),
                expected.to_owned()
            )]
        );
    }
}

#[tokio::test]
async fn audit_revoked_scenario_is_not_dispatched_through_stale_binding() {
    let mut failures = Vec::new();
    for update_provider in [false, true] {
        let mut fixture = Fixture::start().await;
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("canvas.sqlite");
        let secrets = Arc::new(MemorySecretStore::new());
        let service = Arc::new(CanvasProviderService::new(
            Arc::new(Database::open(&path).expect("db")),
            secrets.clone(),
        ));
        let mut input = provider_input(&fixture.base_url);
        service.save_provider(input.clone()).expect("provider");
        service.save_bindings(None, bindings()).expect("bindings");
        assert_eq!(complete(service.clone()).await.as_deref(), Ok(FORMER));
        assert_eq!(fixture.take_seen().len(), 1);

        let scopes = if update_provider {
            vec![CanvasScenario::Embedding]
        } else {
            Vec::new()
        };
        input.models[0].scenarios = scopes.clone();
        if update_provider {
            input.api_key = None;
            service.save_provider(input).expect("provider scope update");
        } else {
            service
                .save_model(PROVIDER, model(FORMER, scopes.clone()))
                .expect("model scope update");
        }
        drop(service);
        let reopened = Arc::new(CanvasProviderService::new(
            Arc::new(Database::open(&path).expect("reopen db")),
            secrets,
        ));
        assert_eq!(
            reopened.provider(PROVIDER).expect("provider").models[0].scenarios,
            scopes
        );
        assert_eq!(
            reopened.bindings(None).expect("persisted bindings"),
            bindings()
        );
        assert!(reopened.save_bindings(None, bindings()).is_err());
        let candidates: Vec<_> = reopened
            .candidates_for_scenario(CanvasScenario::Text, None)
            .expect("candidates")
            .into_iter()
            .map(|candidate| candidate.model.id)
            .collect();
        fixture.reject_former.store(true, Ordering::SeqCst);
        let result = complete(reopened.clone()).await;
        let seen = fixture.take_seen();

        // Removing only the stale bindings makes the same model declarations work.
        reopened
            .save_bindings(None, Vec::new())
            .expect("clear bindings");
        let unbound_result = complete(reopened).await;
        let unbound_seen = fixture.take_seen();
        fixture.stop().await;
        assert_eq!(unbound_result.as_deref(), Ok(HEALTHY));
        assert_eq!(
            unbound_seen,
            vec![(
                "POST /v1/chat/completions HTTP/1.1".to_owned(),
                HEALTHY.to_owned()
            )]
        );
        eprintln!("update_provider={update_provider} scopes={scopes:?} candidates={candidates:?} result={result:?} wire={seen:?} unbound={unbound_result:?}");
        if candidates.iter().any(|id| id == FORMER)
            || result.as_deref() != Ok(HEALTHY)
            || seen.iter().any(|(_, id)| id == FORMER)
        {
            failures.push(update_provider);
        }
    }
    assert!(
        failures.is_empty(),
        "revoked scenario still dispatched; provider-update flags={failures:?}"
    );
}
