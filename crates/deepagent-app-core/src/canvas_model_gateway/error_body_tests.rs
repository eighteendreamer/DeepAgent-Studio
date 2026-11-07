//! H-304: actual gateway entry points against loopback-only synthetic HTTP.
use super::tests::{deepseek_usage_completion_payload, local_text_gateway};
use super::*;
use crate::canvas_provider_service::{CanvasProviderInput, CanvasScenarioBinding};
use std::sync::atomic::AtomicUsize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::Notify;

enum Framing {
    Length(usize),
    Chunked,
}

/// One response per server. A deliberately short body can either close (a
/// transport failure) or stay open until the client cancels/drops the response.
struct WireServer {
    base_url: String,
    paths: Arc<Mutex<Vec<String>>>,
    sent: Arc<AtomicUsize>,
    started: Arc<Notify>,
    closed: Arc<Notify>,
    task: tokio::task::JoinHandle<()>,
}

impl WireServer {
    async fn start(status: u16, body: Vec<u8>, framing: Framing, hold_open: bool) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let paths = Arc::new(Mutex::new(Vec::new()));
        let captured = paths.clone();
        let sent = Arc::new(AtomicUsize::new(0));
        let sent_bytes = sent.clone();
        let started = Arc::new(Notify::new());
        let ready = started.clone();
        let closed = Arc::new(Notify::new());
        let disconnected = closed.clone();
        let task = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut buffer = [0; 4096];
            loop {
                let n = socket.read(&mut buffer).await.unwrap();
                assert_ne!(n, 0, "request ended prematurely");
                request.extend_from_slice(&buffer[..n]);
                if let Some(end) = request.windows(4).position(|s| s == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&request[..end]);
                    let length = headers
                        .lines()
                        .filter_map(|line| line.split_once(':'))
                        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                        .and_then(|(_, value)| value.trim().parse::<usize>().ok())
                        .unwrap_or(0);
                    if request.len() >= end + 4 + length {
                        captured.lock().unwrap().push(
                            headers
                                .lines()
                                .next()
                                .unwrap()
                                .split_whitespace()
                                .nth(1)
                                .unwrap()
                                .into(),
                        );
                        break;
                    }
                }
            }
            let framing_header = match framing {
                Framing::Length(n) => format!("Content-Length: {n}"),
                Framing::Chunked => "Transfer-Encoding: chunked".into(),
            };
            let headers = format!(
                "HTTP/1.1 {status} Synthetic\r\nContent-Type: application/json\r\n{framing_header}\r\nConnection: close\r\n\r\n"
            );
            if socket.write_all(headers.as_bytes()).await.is_err() {
                return;
            }
            for chunk in body.chunks(4096) {
                if matches!(framing, Framing::Chunked)
                    && socket
                        .write_all(format!("{:x}\r\n", chunk.len()).as_bytes())
                        .await
                        .is_err()
                {
                    return;
                }
                if socket.write_all(chunk).await.is_err() {
                    return;
                }
                sent_bytes.fetch_add(chunk.len(), Ordering::SeqCst);
                if matches!(framing, Framing::Chunked) && socket.write_all(b"\r\n").await.is_err() {
                    return;
                }
                if body.len() > MAX_PROVIDER_ERROR_BODY_BYTES {
                    tokio::time::sleep(Duration::from_millis(1)).await;
                }
            }
            ready.notify_one();
            if hold_open {
                // All request bytes have been consumed. EOF/reset here proves
                // that cancellation/timeout/fast refusal dropped the HTTP body.
                let n = socket.read(&mut buffer).await;
                assert!(matches!(n, Ok(0) | Err(_)), "unexpected second request");
                disconnected.notify_one();
            } else if matches!(framing, Framing::Chunked) {
                let _ = socket.write_all(b"0\r\n\r\n").await;
            }
        });
        Self {
            base_url: format!("http://{address}/v1"),
            paths,
            sent,
            started,
            closed,
            task,
        }
    }

    async fn full(status: u16, body: Vec<u8>) -> Self {
        let size = body.len();
        Self::start(status, body, Framing::Length(size), false).await
    }

    fn gateway(&self) -> (CanvasModelGateway, ResolvedCanvasModel) {
        let (gateway, id) = local_text_gateway(self.base_url.clone(), "deepseek-flash");
        let candidate = gateway
            .providers
            .resolve_model(&id, "deepseek-flash")
            .unwrap();
        (gateway, candidate)
    }

    async fn assert_closed(&self) {
        tokio::time::timeout(Duration::from_secs(2), self.closed.notified())
            .await
            .expect("client must close the unfinished response");
    }
}

impl Drop for WireServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[derive(Default)]
struct UsageSink(Mutex<Vec<CanvasUsageReport>>);

impl CanvasUsageSink for UsageSink {
    fn record(&self, report: CanvasUsageReport) {
        self.0.lock().unwrap().push(report);
    }
}

#[tokio::test]
async fn h304_small_errors_keep_status_classification_and_unicode_summary() {
    for (status, code) in [
        (400, CanvasErrorCode::ProviderFailure),
        (401, CanvasErrorCode::SecretMissing),
        (403, CanvasErrorCode::SecretMissing),
        (429, CanvasErrorCode::TransientFailure),
        (503, CanvasErrorCode::TransientFailure),
    ] {
        let server = WireServer::full(status, "错🙂".repeat(250).into_bytes()).await;
        let (gateway, candidate) = server.gateway();
        let error = gateway
            .call(
                &candidate,
                &CanvasModelRequest::text("synthetic"),
                None,
                None,
                SpeechOutputTarget::Bytes,
            )
            .await
            .unwrap_err();
        assert_eq!(error.code, code);
        let prefix = format!("provider returned HTTP {status}: ");
        assert_eq!(
            error.message.strip_prefix(&prefix).unwrap(),
            "错🙂".repeat(200)
        );
    }
}

#[tokio::test]
async fn h304_exact_limit_and_chunked_overflow_are_distinguished() {
    let exact = WireServer::full(400, vec![b'E'; MAX_PROVIDER_ERROR_BODY_BYTES]).await;
    let (gateway, candidate) = exact.gateway();
    let error = gateway
        .call(
            &candidate,
            &CanvasModelRequest::text("synthetic"),
            None,
            None,
            SpeechOutputTarget::Bytes,
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, CanvasErrorCode::ProviderFailure);
    assert_eq!(
        exact.sent.load(Ordering::SeqCst),
        MAX_PROVIDER_ERROR_BODY_BYTES
    );

    let total = MAX_PROVIDER_ERROR_BODY_BYTES * 32;
    let overflow = WireServer::start(503, vec![b'E'; total], Framing::Chunked, false).await;
    let (gateway, candidate) = overflow.gateway();
    let error = gateway
        .call(
            &candidate,
            &CanvasModelRequest::text("synthetic"),
            None,
            None,
            SpeechOutputTarget::Bytes,
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, CanvasErrorCode::ResponseBodyTooLarge);
    assert!(error.message.contains("HTTP 503"));
    assert!(!error.is_transient());
    assert!(overflow.sent.load(Ordering::SeqCst) < total);
    assert!(error.message.len() < 150);
}

#[tokio::test]
async fn h304_all_shared_entry_points_reject_declared_overflow_without_reading_body() {
    for entry in [
        "chat",
        "embedding",
        "audio",
        "image",
        "submit",
        "status",
        "catalog",
    ] {
        let server = WireServer::start(
            500,
            vec![],
            Framing::Length(MAX_PROVIDER_ERROR_BODY_BYTES + 1),
            true,
        )
        .await;
        let (gateway, candidate) = server.gateway();
        let request = CanvasModelRequest::text("synthetic");
        let (result, path) = match entry {
            "submit" => (
                gateway
                    .submit_media_job(&candidate, &request, None)
                    .await
                    .map(|_| ()),
                "/v1/video/submit",
            ),
            "status" => (
                gateway
                    .poll_media_job(&candidate, "synthetic-task", 1000, None)
                    .await
                    .map(|_| ()),
                "/v1/video/status",
            ),
            "catalog" => (
                gateway
                    .discover_models(&candidate.provider.id)
                    .await
                    .map(|_| ()),
                "/v1/models",
            ),
            _ => {
                let (operation, path) = match entry {
                    "embedding" => (CanvasOperation::Embedding, "/v1/embeddings"),
                    "audio" => (CanvasOperation::SpeechSynthesize, "/v1/audio/speech"),
                    "image" => (CanvasOperation::ImageGenerate, "/v1/images/generations"),
                    _ => (CanvasOperation::TextGenerate, "/v1/chat/completions"),
                };
                let mut request = request.with_operation(operation);
                request.texts = vec!["synthetic".into()];
                (
                    gateway
                        .call(&candidate, &request, None, None, SpeechOutputTarget::Bytes)
                        .await
                        .map(|_| ()),
                    path,
                )
            }
        };
        let error = result.unwrap_err();
        assert_eq!(error.code, CanvasErrorCode::ResponseBodyTooLarge, "{entry}");
        assert!(error.message.contains("HTTP 500"), "{entry}");
        assert!(!error.message.contains("EEEE"));
        assert_eq!(*server.paths.lock().unwrap(), [path]);
        assert_eq!(server.sent.load(Ordering::SeqCst), 0);
        server.assert_closed().await;
    }
}

#[tokio::test]
async fn h304_each_physical_error_response_reports_usage_or_unknown_once() {
    for operation in [CanvasOperation::TextGenerate, CanvasOperation::Embedding] {
        for case in ["valid", "malformed", "oversized", "truncated"] {
            let body = match case {
                "valid" => deepseek_usage_completion_payload().to_string().into_bytes(),
                _ => b"{not-json".to_vec(),
            };
            let length = match case {
                "oversized" => MAX_PROVIDER_ERROR_BODY_BYTES + 1,
                "truncated" => body.len() + 10,
                _ => body.len(),
            };
            let server =
                WireServer::start(503, body, Framing::Length(length), case == "oversized").await;
            let (gateway, candidate) = server.gateway();
            let sink = UsageSink::default();
            let mut request = CanvasModelRequest::text("synthetic").with_operation(operation);
            request.texts = vec!["synthetic".into()];
            let error = gateway
                .call(
                    &candidate,
                    &request,
                    Some(&sink),
                    None,
                    SpeechOutputTarget::Bytes,
                )
                .await
                .unwrap_err();
            if case == "truncated" {
                assert_eq!(error.code, CanvasErrorCode::TransientFailure);
                assert!(error.message.contains("reading HTTP 503"));
                assert!(!error.message.contains(&server.base_url));
            }
            let reports = sink.0.lock().unwrap();
            assert_eq!(reports.len(), 1, "{operation:?}/{case}");
            assert_eq!(
                reports[0].provider_id.as_deref(),
                Some(candidate.provider.id.as_str())
            );
            assert_eq!(reports[0].model_id, candidate.model.id);
            if case == "valid" {
                assert_eq!(reports[0].usage.as_ref().unwrap().prompt_tokens, 100);
                assert!(reports[0].raw_usage.is_some());
            } else {
                assert!(reports[0].usage.is_none());
                assert!(reports[0].raw_usage.is_none());
            }
        }
    }
}

#[tokio::test]
async fn h304_overflow_does_not_fallback_but_small_transient_errors_still_do() {
    for (status, oversized, fallback) in [
        (429, false, true),
        (503, false, true),
        (401, false, false),
        (503, true, false),
    ] {
        let first = WireServer::start(
            status,
            b"synthetic failure".to_vec(),
            Framing::Length(if oversized {
                MAX_PROVIDER_ERROR_BODY_BYTES + 1
            } else {
                17
            }),
            oversized,
        )
        .await;
        let second = WireServer::full(
            200,
            deepseek_usage_completion_payload().to_string().into_bytes(),
        )
        .await;
        let (gateway, candidate) = first.gateway();
        let mut model = candidate.model.clone();
        model.id = "healthy-model".into();
        let healthy = gateway
            .providers
            .save_provider(CanvasProviderInput {
                name: "healthy fallback".into(),
                protocol: "openai".into(),
                base_url: second.base_url.clone(),
                enabled: true,
                models: vec![model],
                api_key: Some("synthetic-h304-key".into()),
                ..Default::default()
            })
            .unwrap();
        gateway
            .providers
            .save_bindings(
                None,
                vec![CanvasScenarioBinding {
                    scenario: "text".into(),
                    provider_id: candidate.provider.id.clone(),
                    model_id: candidate.model.id.clone(),
                    enabled: true,
                }],
            )
            .unwrap();
        let sink = Arc::new(UsageSink::default());
        let result = gateway
            .execute_observed(
                &CanvasModelRequest::text("synthetic"),
                None,
                Some(sink.clone()),
                SpeechOutputTarget::Bytes,
            )
            .await;
        assert_eq!(first.paths.lock().unwrap().len(), 1);
        assert_eq!(second.paths.lock().unwrap().len(), usize::from(fallback));
        assert_eq!(sink.0.lock().unwrap().len(), if fallback { 2 } else { 1 });
        if fallback {
            assert!(
                matches!(result.unwrap().into_model().unwrap(), CanvasModelOutput::Text { provider_id, text, .. }
                if provider_id == healthy.id && text == "answer")
            );
        } else {
            assert_eq!(
                result.unwrap_err().code,
                if oversized {
                    CanvasErrorCode::ResponseBodyTooLarge
                } else {
                    CanvasErrorCode::SecretMissing
                }
            );
        }
    }
}

#[tokio::test]
async fn h304_control_plane_small_errors_and_successful_responses_keep_contracts() {
    for entry in ["submit", "status", "catalog"] {
        for status in [200, 401, 429] {
            let body = if status != 200 {
                b"synthetic error".to_vec()
            } else {
                match entry {
                    "submit" => json!({"requestId":"synthetic-task"}),
                    "status" => json!({"status":"Succeed"}),
                    _ => json!({"data":[{"id":"synthetic-task"}]}),
                }
                .to_string()
                .into_bytes()
            };
            let server = WireServer::full(status, body).await;
            let (gateway, candidate) = server.gateway();
            let result = match entry {
                "submit" => {
                    gateway
                        .submit_media_job(&candidate, &CanvasModelRequest::text("synthetic"), None)
                        .await
                }
                "status" => gateway
                    .poll_media_job(&candidate, "synthetic-task", 1000, None)
                    .await
                    .map(|job| job.request_id),
                _ => gateway
                    .discover_models(&candidate.provider.id)
                    .await
                    .map(|ids| ids.join(",")),
            };
            if status == 200 {
                assert_eq!(result.unwrap(), "synthetic-task", "{entry}");
            } else {
                let error = result.unwrap_err();
                assert_eq!(
                    error.code,
                    if status == 401 {
                        CanvasErrorCode::SecretMissing
                    } else {
                        CanvasErrorCode::TransientFailure
                    }
                );
                assert!(error.message.ends_with("synthetic error"));
                assert!(error.message.contains(&format!("HTTP {status}")));
            }
            assert_eq!(server.paths.lock().unwrap().len(), 1);
        }
    }
}

#[tokio::test]
async fn h304_video_error_body_wait_honors_existing_cancellation_and_timeout() {
    for submit in [true, false] {
        for cancel_request in [true, false] {
            let server =
                WireServer::start(503, b"partial".to_vec(), Framing::Length(1024), true).await;
            let (gateway, candidate) = server.gateway();
            let cancel = Arc::new(AtomicBool::new(false));
            let mut request = CanvasModelRequest::text("synthetic");
            request.timeout_ms = 1000;
            let operation = async {
                if submit {
                    gateway
                        .submit_media_job(&candidate, &request, Some(&cancel))
                        .await
                        .map(|_| ())
                } else {
                    gateway
                        .poll_media_job(&candidate, "synthetic-task", 1000, Some(&cancel))
                        .await
                        .map(|_| ())
                }
            };
            let trigger = async {
                server.started.notified().await;
                if cancel_request {
                    cancel.store(true, Ordering::SeqCst);
                }
            };
            let (result, ()) = tokio::time::timeout(Duration::from_secs(3), async {
                tokio::join!(operation, trigger)
            })
            .await
            .expect("HTTP wait must not outlive its cancel/timeout");
            let error = result.unwrap_err();
            assert_eq!(error.code, CanvasErrorCode::TransientFailure);
            if cancel_request {
                assert!(error.message.contains("Cancelled:"));
            } else {
                assert!(error.message.contains("reading HTTP 503"));
            }
            server.assert_closed().await;
        }
    }
}
