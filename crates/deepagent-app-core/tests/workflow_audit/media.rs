use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use deepagent_app_core::canvas_artifact_service::CanvasArtifactService;
use deepagent_app_core::canvas_model_gateway::CanvasModelGateway;
use deepagent_app_core::canvas_provider_service::{
    CanvasModelConfig, CanvasProviderInput, CanvasProviderService, CanvasScenario,
};
use deepagent_app_core::cost_service::CostService;
use deepagent_app_core::{ChatService, MemorySecretStore, SettingsService};
use deepagent_core::event::EventPayload;
use deepagent_models::transport::MockTransport;
use deepagent_persistence::{
    cost_store::CostStore, event_store::EventStore, run_store::RunStore, Database,
};
use deepagent_runtime::workflow::{NodeExecutionEvent, NodeExecutionStatus, WorkflowRequest};
use deepagent_runtime::RuntimeEvent;
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Notify;
use tokio::task::JoinHandle;

const VIDEO_BYTES: &[u8] = b"\0\0\0\x14ftypmp42audit-video";
const AUDIO_BYTES: &[u8] = b"ID3audit-audio-payload";
const PRIVATE_IMAGE_BYTES: &[u8] = b"\x89PNG\r\n\x1a\naudit-private-network-canary";
const TASK_ID: &str = "audit-video-job";

/// Only the provider is a fixture: the application still uses its real HTTP
/// gateway, WorkflowAgent, Kernel, cancellation tree, artifact store and SQLite.
struct LocalVideoProvider {
    base_url: String,
    requests: Arc<Mutex<Vec<(String, Value)>>>,
    poll_received: Arc<Notify>,
    release_poll: Arc<Notify>,
    server: JoinHandle<()>,
}

/// A video provider whose completed-job URL points at a second loopback
/// service. This is separate from the normal video fixture so the second-hop
/// fetch can be observed independently from submit/status traffic.
struct LocalVideoUrlProvider {
    base_url: String,
    private_requests: Arc<Mutex<Vec<String>>>,
    provider_server: JoinHandle<()>,
    private_server: JoinHandle<()>,
}

impl LocalVideoUrlProvider {
    async fn start() -> Self {
        let private_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let private_address = private_listener.local_addr().unwrap();
        let private_requests = Arc::new(Mutex::new(Vec::new()));
        let private_captured = private_requests.clone();
        let private_server = tokio::spawn(async move {
            let (mut socket, _) = private_listener.accept().await.unwrap();
            let (line, _) = read_request(&mut socket).await;
            private_captured.lock().unwrap().push(line);
            let header = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: video/mp4\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                VIDEO_BYTES.len()
            );
            socket.write_all(header.as_bytes()).await.unwrap();
            socket.write_all(VIDEO_BYTES).await.unwrap();
        });

        let provider_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let provider_address = provider_listener.local_addr().unwrap();
        let download_url = format!("http://{private_address}/host-private.mp4");
        let provider_server = tokio::spawn(async move {
            let mut submitted = false;
            loop {
                let (mut socket, _) = provider_listener.accept().await.unwrap();
                let (line, _) = read_request(&mut socket).await;
                let payload = if line.starts_with("POST /v1/video/submit ") {
                    submitted = true;
                    json!({"requestId":"audit-video-url"}).to_string()
                } else {
                    assert!(submitted);
                    assert!(line.starts_with("POST /v1/video/status "));
                    json!({
                        "status": "Succeed",
                        "reason": "audit provider finished",
                        "results": {"videos": [{"url": download_url}]}
                    })
                    .to_string()
                };
                let header = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    payload.len()
                );
                if socket.write_all(header.as_bytes()).await.is_ok() {
                    let _ = socket.write_all(payload.as_bytes()).await;
                }
            }
        });
        Self {
            base_url: format!("http://{provider_address}/v1"),
            private_requests,
            provider_server,
            private_server,
        }
    }

    async fn close(&mut self) {
        self.provider_server.abort();
        self.private_server.abort();
        let provider_result = (&mut self.provider_server).await;
        let private_result = (&mut self.private_server).await;
        assert!(provider_result.is_ok() || provider_result.unwrap_err().is_cancelled());
        assert!(private_result.is_ok() || private_result.unwrap_err().is_cancelled());
    }
}

impl Drop for LocalVideoUrlProvider {
    fn drop(&mut self) {
        self.provider_server.abort();
        self.private_server.abort();
    }
}

impl LocalVideoProvider {
    async fn start(status_after_release: &str) -> Self {
        Self::start_with_task_id(status_after_release, TASK_ID).await
    }

    async fn start_with_task_id(status_after_release: &str, task_id_prefix: &str) -> Self {
        Self::start_with_task_id_and_video_bytes(
            status_after_release,
            task_id_prefix,
            VIDEO_BYTES.to_vec(),
        )
        .await
    }

    async fn start_with_task_id_and_video_bytes(
        status_after_release: &str,
        task_id_prefix: &str,
        video_bytes: Vec<u8>,
    ) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let poll_received = Arc::new(Notify::new());
        let release_poll = Arc::new(Notify::new());
        let captured = requests.clone();
        let arrived = poll_received.clone();
        let release = release_poll.clone();
        let status = status_after_release.to_owned();
        let task_id_prefix = task_id_prefix.to_owned();
        let video_bytes = video_bytes;
        let submit_count = Arc::new(AtomicUsize::new(0));
        let submitted = submit_count.clone();
        let server = tokio::spawn(async move {
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                let (line, body) = read_request(&mut socket).await;
                captured.lock().unwrap().push((line.clone(), body));
                let (media_type, payload) =
                    if line.starts_with("POST /v1/video/submit ") {
                        let sequence = submitted.fetch_add(1, Ordering::SeqCst) + 1;
                        let task_id = if sequence == 1 {
                            task_id_prefix.clone()
                        } else {
                            format!("{task_id_prefix}-{sequence}")
                        };
                        (
                            "application/json",
                            json!({"requestId": task_id}).to_string().into_bytes(),
                        )
                    } else if line.starts_with("POST /v1/video/status ") {
                        arrived.notify_one();
                        release.notified().await;
                        ("application/json", json!({
                        "status": status,
                        "reason": "audit provider finished",
                        "results": {"videos": [{"url": format!("http://{address}/clip.mp4")}]}
                    }).to_string().into_bytes())
                    } else {
                        assert_eq!(line, "GET /clip.mp4 HTTP/1.1");
                        ("video/mp4", video_bytes.clone())
                    };
                let header = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: {media_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    payload.len()
                );
                // A fixed implementation may drop the pending HTTP request on
                // cancellation. A broken pipe here is therefore a valid outcome.
                if socket.write_all(header.as_bytes()).await.is_ok() {
                    let _ = socket.write_all(&payload).await;
                }
            }
        });
        Self {
            base_url: format!("http://{address}/v1"),
            requests,
            poll_received,
            release_poll,
            server,
        }
    }

    async fn wait_for_poll(&self) {
        tokio::time::timeout(Duration::from_secs(5), self.poll_received.notified())
            .await
            .expect("the real gateway must reach video/status");
    }

    async fn close(&mut self) {
        self.release_poll.notify_one();
        self.server.abort();
        let result = (&mut self.server).await;
        assert!(result.is_ok() || result.unwrap_err().is_cancelled());
    }
}

impl Drop for LocalVideoProvider {
    fn drop(&mut self) {
        self.server.abort();
    }
}

/// A text provider that accepts the request but withholds the response until
/// the audit releases it. This isolates cancellation of the provider wait from
/// request construction and connection setup.
struct LocalTextProvider {
    base_url: String,
    requests: Arc<Mutex<Vec<(String, Value)>>>,
    request_received: Arc<Notify>,
    release_response: Arc<Notify>,
    server: JoinHandle<()>,
}

/// A text provider that returns a large non-2xx body. The body is synthetic;
/// the fixture exists to observe whether the Canvas gateway bounds error
/// responses before classifying them.
struct LocalErrorTextProvider {
    base_url: String,
    sent_bytes: Arc<AtomicUsize>,
    server: JoinHandle<()>,
}

impl LocalErrorTextProvider {
    async fn start(response_bytes: usize) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let sent_bytes = Arc::new(AtomicUsize::new(0));
        let sent = sent_bytes.clone();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let (line, _) = read_request(&mut socket).await;
            assert!(line.starts_with("POST /v1/chat/completions "));
            let body = vec![b'E'; response_bytes];
            let header = format!(
                "HTTP/1.1 500 Internal Server Error\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            if socket.write_all(header.as_bytes()).await.is_ok() {
                for chunk in body.chunks(64 * 1024) {
                    if socket.write_all(chunk).await.is_err() {
                        break;
                    }
                    sent.fetch_add(chunk.len(), Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(1)).await;
                }
            }
        });
        Self {
            base_url: format!("http://{address}/v1"),
            sent_bytes,
            server,
        }
    }

    async fn close(&mut self) {
        let result = (&mut self.server).await;
        assert!(result.is_ok() || result.unwrap_err().is_cancelled());
    }
}

impl Drop for LocalErrorTextProvider {
    fn drop(&mut self) {
        self.server.abort();
    }
}

impl LocalTextProvider {
    async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let request_received = Arc::new(Notify::new());
        let release_response = Arc::new(Notify::new());
        let captured = requests.clone();
        let arrived = request_received.clone();
        let release = release_response.clone();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let (line, body) = read_request(&mut socket).await;
            captured.lock().unwrap().push((line, body));
            arrived.notify_one();
            release.notified().await;
            let payload = json!({
                "choices": [{ "message": { "content": "delayed answer" } }]
            })
            .to_string();
            let header = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                payload.len()
            );
            if socket.write_all(header.as_bytes()).await.is_ok() {
                let _ = socket.write_all(payload.as_bytes()).await;
            }
        });
        Self {
            base_url: format!("http://{address}/v1"),
            requests,
            request_received,
            release_response,
            server,
        }
    }

    async fn wait_for_request(&self) {
        tokio::time::timeout(Duration::from_secs(5), self.request_received.notified())
            .await
            .expect("the real gateway must reach chat/completions");
    }

    async fn close(&mut self) {
        self.release_response.notify_one();
        let result = (&mut self.server).await;
        assert!(result.is_ok() || result.unwrap_err().is_cancelled());
    }
}

impl Drop for LocalTextProvider {
    fn drop(&mut self) {
        self.server.abort();
    }
}

/// An image provider that returns a URL pointing at a second loopback server.
/// The second server stands in for a host-private control plane or metadata
/// endpoint and records whether the application follows the provider URL.
struct LocalImageProvider {
    base_url: String,
    provider_requests: Arc<Mutex<Vec<(String, Value)>>>,
    private_requests: Arc<Mutex<Vec<String>>>,
    provider_server: JoinHandle<()>,
    private_server: JoinHandle<()>,
}

impl LocalImageProvider {
    async fn start() -> Self {
        Self::start_with_image_bytes(PRIVATE_IMAGE_BYTES.to_vec()).await
    }

    async fn start_with_image_bytes(image_bytes: Vec<u8>) -> Self {
        let private_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let private_address = private_listener.local_addr().unwrap();
        let private_requests = Arc::new(Mutex::new(Vec::new()));
        let private_captured = private_requests.clone();
        let private_server = tokio::spawn(async move {
            let (mut socket, _) = private_listener.accept().await.unwrap();
            let (line, _) = read_request(&mut socket).await;
            private_captured.lock().unwrap().push(line);
            let header = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                image_bytes.len()
            );
            socket.write_all(header.as_bytes()).await.unwrap();
            socket.write_all(&image_bytes).await.unwrap();
        });

        let provider_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let provider_address = provider_listener.local_addr().unwrap();
        let provider_requests = Arc::new(Mutex::new(Vec::new()));
        let provider_captured = provider_requests.clone();
        let provider_server = tokio::spawn(async move {
            let (mut socket, _) = provider_listener.accept().await.unwrap();
            let (line, body) = read_request(&mut socket).await;
            provider_captured.lock().unwrap().push((line, body));
            let payload = json!({
                "data": [{
                    "url": format!("http://{private_address}/host-private.png")
                }]
            })
            .to_string();
            let header = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                payload.len()
            );
            socket.write_all(header.as_bytes()).await.unwrap();
            socket.write_all(payload.as_bytes()).await.unwrap();
        });

        Self {
            base_url: format!("http://{provider_address}/v1"),
            provider_requests,
            private_requests,
            provider_server,
            private_server,
        }
    }

    async fn close(&mut self) {
        self.provider_server.abort();
        self.private_server.abort();
        let provider_result = (&mut self.provider_server).await;
        let private_result = (&mut self.private_server).await;
        assert!(provider_result.is_ok() || provider_result.unwrap_err().is_cancelled());
        assert!(private_result.is_ok() || private_result.unwrap_err().is_cancelled());
    }
}

impl Drop for LocalImageProvider {
    fn drop(&mut self) {
        self.provider_server.abort();
        self.private_server.abort();
    }
}

/// An image provider that returns an inline base64 payload instead of a URL.
/// This covers the alternate parse_output branch without a second-hop server.
struct LocalInlineImageProvider {
    base_url: String,
    requests: Arc<Mutex<Vec<(String, Value)>>>,
    server: JoinHandle<()>,
}

impl LocalInlineImageProvider {
    async fn start(image_bytes: Vec<u8>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let captured = requests.clone();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let (line, body) = read_request(&mut socket).await;
            assert!(
                line.starts_with("POST /v1/images/generations ")
                    || line.starts_with("POST /v1/images/edits ")
            );
            captured.lock().unwrap().push((line, body));
            use base64::Engine as _;
            let encoded = base64::engine::general_purpose::STANDARD.encode(image_bytes);
            let payload = json!({"data":[{"b64_json":encoded}]}).to_string();
            let header = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                payload.len()
            );
            if socket.write_all(header.as_bytes()).await.is_ok() {
                let _ = socket.write_all(payload.as_bytes()).await;
            }
        });
        Self {
            base_url: format!("http://{address}/v1"),
            requests,
            server,
        }
    }

    async fn close(&mut self) {
        let result = (&mut self.server).await;
        assert!(result.is_ok() || result.unwrap_err().is_cancelled());
    }
}

impl Drop for LocalInlineImageProvider {
    fn drop(&mut self) {
        self.server.abort();
    }
}

/// An image provider that accepts the generation request but withholds its
/// response. This isolates cancellation of the image provider HTTP wait from
/// URL materialization and artifact persistence.
struct LocalPendingImageProvider {
    base_url: String,
    request_received: Arc<Notify>,
    release_response: Arc<Notify>,
    server: JoinHandle<()>,
}

impl LocalPendingImageProvider {
    async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let request_received = Arc::new(Notify::new());
        let release_response = Arc::new(Notify::new());
        let arrived = request_received.clone();
        let release = release_response.clone();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let (line, _) = read_request(&mut socket).await;
            assert!(line.starts_with("POST /v1/images/generations "));
            arrived.notify_one();
            release.notified().await;
            use base64::Engine as _;
            let image = base64::engine::general_purpose::STANDARD.encode(PRIVATE_IMAGE_BYTES);
            let payload = json!({"data":[{"b64_json":image}]}).to_string();
            let header = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                payload.len()
            );
            if socket.write_all(header.as_bytes()).await.is_ok() {
                let _ = socket.write_all(payload.as_bytes()).await;
            }
        });
        Self {
            base_url: format!("http://{address}/v1"),
            request_received,
            release_response,
            server,
        }
    }

    async fn wait_for_request(&self) {
        tokio::time::timeout(Duration::from_secs(5), self.request_received.notified())
            .await
            .expect("the real image gateway must receive the generation request");
    }

    async fn close(&mut self) {
        self.release_response.notify_one();
        let result = (&mut self.server).await;
        assert!(result.is_ok() || result.unwrap_err().is_cancelled());
    }
}

impl Drop for LocalPendingImageProvider {
    fn drop(&mut self) {
        self.server.abort();
    }
}

/// An audio provider that accepts speech synthesis but withholds the binary
/// response. This isolates cancellation of the shared audio HTTP read from
/// artifact persistence and output parsing.
struct LocalPendingAudioProvider {
    base_url: String,
    request_received: Arc<Notify>,
    release_response: Arc<Notify>,
    server: JoinHandle<()>,
}

impl LocalPendingAudioProvider {
    async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let request_received = Arc::new(Notify::new());
        let release_response = Arc::new(Notify::new());
        let arrived = request_received.clone();
        let release = release_response.clone();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let (line, _) = read_request(&mut socket).await;
            assert!(line.starts_with("POST /v1/audio/speech "));
            arrived.notify_one();
            release.notified().await;
            let header = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: audio/mpeg\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                AUDIO_BYTES.len()
            );
            if socket.write_all(header.as_bytes()).await.is_ok() {
                let _ = socket.write_all(AUDIO_BYTES).await;
            }
        });
        Self {
            base_url: format!("http://{address}/v1"),
            request_received,
            release_response,
            server,
        }
    }

    async fn wait_for_request(&self) {
        tokio::time::timeout(Duration::from_secs(5), self.request_received.notified())
            .await
            .expect("the real audio gateway must receive the synthesis request");
    }

    async fn close(&mut self) {
        self.release_response.notify_one();
        let result = (&mut self.server).await;
        assert!(result.is_ok() || result.unwrap_err().is_cancelled());
    }
}

impl Drop for LocalPendingAudioProvider {
    fn drop(&mut self) {
        self.server.abort();
    }
}

/// Synthetic OpenAI-compatible embeddings endpoint. Only loopback is used;
/// the real gateway still constructs and parses the HTTP request/response.
struct LocalEmbeddingProvider {
    base_url: String,
    request_received: Arc<Notify>,
    release_response: Option<Arc<Notify>>,
    requests: Arc<Mutex<Vec<Value>>>,
    server: JoinHandle<()>,
}

impl LocalEmbeddingProvider {
    async fn start() -> Self {
        Self::start_with_response_count(None).await
    }

    async fn start_with_response_count(response_count: Option<usize>) -> Self {
        Self::start_with_options(response_count, false).await
    }

    async fn start_pending() -> Self {
        Self::start_with_options(None, true).await
    }

    async fn start_with_options(response_count: Option<usize>, pending: bool) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let request_received = Arc::new(Notify::new());
        let arrived = request_received.clone();
        let release_response = pending.then(|| Arc::new(Notify::new()));
        let release = release_response.clone();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let captured = requests.clone();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let (line, body) = read_request(&mut socket).await;
            assert!(line.starts_with("POST /v1/embeddings "));
            // Production embedding_input() sends a scalar for one text and
            // an array for a batch. Do not invent an array-only wire contract.
            let count = match &body["input"] {
                Value::String(_) => 1,
                Value::Array(texts) => texts.len(),
                _ => panic!("text embedding input"),
            };
            captured.lock().unwrap().push(body);
            arrived.notify_one();
            if let Some(release) = release {
                release.notified().await;
            }
            let data: Vec<Value> = (0..response_count.unwrap_or(count))
                .map(|index| json!({"index": index, "embedding": [0.1, 0.2, 0.3]}))
                .collect();
            let payload = json!({"data": data, "usage": {"prompt_tokens": 11, "total_tokens": 11}})
                .to_string();
            let header = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                payload.len()
            );
            if socket.write_all(header.as_bytes()).await.is_ok() {
                let _ = socket.write_all(payload.as_bytes()).await;
            }
        });
        Self {
            base_url: format!("http://{address}/v1"),
            request_received,
            release_response,
            requests,
            server,
        }
    }

    async fn wait_for_request(&self) {
        tokio::time::timeout(Duration::from_secs(5), self.request_received.notified())
            .await
            .expect("the real embedding gateway must receive the request before cancellation");
    }

    async fn close(&mut self) {
        if let Some(release) = &self.release_response {
            release.notify_one();
        }
        (&mut self.server).await.unwrap();
    }
}

impl Drop for LocalEmbeddingProvider {
    fn drop(&mut self) {
        self.server.abort();
    }
}

async fn read_request(socket: &mut TcpStream) -> (String, Value) {
    let mut bytes = Vec::new();
    loop {
        let mut chunk = [0u8; 1024];
        let read = tokio::time::timeout(Duration::from_secs(5), socket.read(&mut chunk))
            .await
            .unwrap()
            .unwrap();
        assert!(read > 0, "incomplete local HTTP request");
        bytes.extend_from_slice(&chunk[..read]);
        assert!(bytes.len() < 16 * 1024, "unexpected fixture request size");
        let Some(head_end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") else {
            continue;
        };
        let header = String::from_utf8_lossy(&bytes[..head_end]);
        let length = header
            .lines()
            .filter_map(|line| line.split_once(':'))
            .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
            .map(|(_, value)| value.trim().parse::<usize>().unwrap())
            .unwrap_or(0);
        if bytes.len() >= head_end + 4 + length {
            let body = &bytes[head_end + 4..head_end + 4 + length];
            return (
                header.lines().next().unwrap().to_string(),
                if body.is_empty() {
                    Value::Null
                } else {
                    serde_json::from_slice(body).unwrap()
                },
            );
        }
    }
}

struct MediaFixture {
    _root: tempfile::TempDir,
    db: Arc<Database>,
    chat: ChatService,
    session: String,
    artifacts: Arc<CanvasArtifactService>,
    providers: Arc<CanvasProviderService>,
    workflow: WorkflowRequest,
    live: Arc<Mutex<Vec<NodeExecutionEvent>>>,
    approvals: Arc<AtomicUsize>,
}

impl MediaFixture {
    async fn new(provider: &LocalVideoProvider) -> Self {
        Self::with_provider(
            &provider.base_url,
            "Audit local video",
            "audit-video-model",
            vec![CanvasScenario::VideoGeneration],
            |provider_id| {
                serde_json::from_value(json!({
                    "definition": {
                        "version": 1,
                        "nodes": [
                            {"id":"start","kind":"start"},
                            {"id":"video","kind":"video-gen","config":{
                                "prompt":"audit local video",
                                "videoModel":format!("{provider_id}::audit-video-model")
                            }},
                            {"id":"end","kind":"end"}
                        ],
                        "edges": [
                            {"id":"e1","source":"start","target":"video"},
                            {"id":"e2","source":"video","target":"end"}
                        ]
                    }
                }))
                .unwrap()
            },
        )
        .await
    }

    async fn new_video_url(provider: &LocalVideoUrlProvider) -> Self {
        Self::with_provider(
            &provider.base_url,
            "Audit local video URL",
            "audit-video-model",
            vec![CanvasScenario::VideoGeneration],
            |provider_id| {
                serde_json::from_value(json!({
                    "definition": {
                        "version": 1,
                        "nodes": [
                            {"id":"start","kind":"start"},
                            {"id":"video","kind":"video-gen","config":{
                                "prompt":"audit video provider URL",
                                "videoModel":format!("{provider_id}::audit-video-model")
                            }},
                            {"id":"end","kind":"end"}
                        ],
                        "edges": [
                            {"id":"e1","source":"start","target":"video"},
                            {"id":"e2","source":"video","target":"end"}
                        ]
                    }
                }))
                .unwrap()
            },
        )
        .await
    }

    async fn new_text(provider: &LocalTextProvider) -> Self {
        Self::with_provider(
            &provider.base_url,
            "Audit local text",
            "deepseek-flash",
            vec![CanvasScenario::Text],
            |provider_id| {
                serde_json::from_value(json!({
                    "definition": {
                        "version": 1,
                        "nodes": [
                            {"id":"start","kind":"start"},
                            {"id":"text","kind":"text-gen","config":{
                                "prompt":"audit delayed text",
                                "model":format!("{provider_id}::deepseek-flash")
                            }},
                            {"id":"end","kind":"end"}
                        ],
                        "edges": [
                            {"id":"e1","source":"start","target":"text"},
                            {"id":"e2","source":"text","target":"end"}
                        ]
                    }
                }))
                .unwrap()
            },
        )
        .await
    }

    async fn new_error_text(provider: &LocalErrorTextProvider) -> Self {
        Self::with_provider(
            &provider.base_url,
            "Audit local text error",
            "deepseek-flash",
            vec![CanvasScenario::Text],
            |provider_id| {
                serde_json::from_value(json!({
                    "definition": {
                        "version": 1,
                        "nodes": [
                            {"id":"start","kind":"start"},
                            {"id":"text","kind":"text-gen","config":{
                                "prompt":"audit oversized provider error",
                                "model":format!("{provider_id}::deepseek-flash")
                            }},
                            {"id":"end","kind":"end"}
                        ],
                        "edges": [
                            {"id":"e1","source":"start","target":"text"},
                            {"id":"e2","source":"text","target":"end"}
                        ]
                    }
                }))
                .unwrap()
            },
        )
        .await
    }

    async fn new_image(provider: &LocalImageProvider) -> Self {
        Self::with_provider(
            &provider.base_url,
            "Audit local image",
            "audit-image-model",
            vec![CanvasScenario::ImageGeneration],
            |provider_id| {
                serde_json::from_value(json!({
                    "definition": {
                        "version": 1,
                        "nodes": [
                            {"id":"start","kind":"start"},
                            {"id":"image","kind":"image-gen","config":{
                                "prompt":"audit provider image URL",
                                "operation":"image_generate",
                                "imageModel":format!("{provider_id}::audit-image-model")
                            }},
                            {"id":"end","kind":"end"}
                        ],
                        "edges": [
                            {"id":"e1","source":"start","target":"image"},
                            {"id":"e2","source":"image","target":"end"}
                        ]
                    }
                }))
                .unwrap()
            },
        )
        .await
    }

    async fn new_inline_image(provider: &LocalInlineImageProvider) -> Self {
        Self::with_provider(
            &provider.base_url,
            "Audit local inline image",
            "audit-inline-image-model",
            vec![CanvasScenario::ImageGeneration],
            |provider_id| {
                serde_json::from_value(json!({
                    "definition": {
                        "version": 1,
                        "nodes": [
                            {"id":"start","kind":"start"},
                            {"id":"image","kind":"image-gen","config":{
                                "prompt":"audit oversized inline image",
                                "operation":"image_generate",
                                "imageModel":format!("{provider_id}::audit-inline-image-model")
                            }},
                            {"id":"end","kind":"end"}
                        ],
                        "edges": [
                            {"id":"e1","source":"start","target":"image"},
                            {"id":"e2","source":"image","target":"end"}
                        ]
                    }
                }))
                .unwrap()
            },
        )
        .await
    }

    async fn new_embedding(provider: &LocalEmbeddingProvider) -> Self {
        Self::with_provider(
            &provider.base_url,
            "Audit local embedding",
            "audit-embedding-model",
            vec![CanvasScenario::Embedding],
            |provider_id| {
                serde_json::from_value(json!({
                    "definition": {
                        "version": 1,
                        "nodes": [
                            {"id":"start","kind":"start"},
                            {"id":"embedding","kind":"embeddings","config":{
                                "texts":["audit embedding"],
                                "embeddingModel":format!("{provider_id}::audit-embedding-model")
                            }},
                            {"id":"end","kind":"end"}
                        ],
                        "edges": [
                            {"id":"e1","source":"start","target":"embedding"},
                            {"id":"e2","source":"embedding","target":"end"}
                        ]
                    }
                }))
                .unwrap()
            },
        )
        .await
    }

    async fn new_audio(provider: &LocalPendingAudioProvider) -> Self {
        Self::with_provider(
            &provider.base_url,
            "Audit local audio",
            "audit-audio-model",
            vec![CanvasScenario::TextToSpeech],
            |provider_id| {
                serde_json::from_value(json!({
                    "definition": {
                        "version": 1,
                        "nodes": [
                            {"id":"start","kind":"start"},
                            {"id":"audio","kind":"audio","config":{
                                "operation":"speech_synthesize",
                                "prompt":"audit delayed audio",
                                "audioModel":format!("{provider_id}::audit-audio-model"),
                                "audioVoice":"alloy",
                                "audioFormat":"mp3"
                            }},
                            {"id":"end","kind":"end"}
                        ],
                        "edges": [
                            {"id":"e1","source":"start","target":"audio"},
                            {"id":"e2","source":"audio","target":"end"}
                        ]
                    }
                }))
                .unwrap()
            },
        )
        .await
    }

    async fn with_provider(
        base_url: &str,
        provider_name: &str,
        model_id: &str,
        scenarios: Vec<CanvasScenario>,
        build_workflow: impl FnOnce(&str) -> WorkflowRequest,
    ) -> Self {
        let root = tempfile::tempdir().unwrap();
        let db = Arc::new(Database::open(root.path().join("media.db")).unwrap());
        let secrets = Arc::new(MemorySecretStore::new());
        let settings = Arc::new(SettingsService::new(
            db.clone(),
            Arc::new(MockTransport::with_get_json(
                r#"{"data":[{"id":"deepseek-v4-flash","object":"model","owned_by":"deepseek"}]}"#,
            )),
            secrets.clone(),
        ));
        settings.initialize("sk-audit-placeholder").await.unwrap();
        let providers = Arc::new(CanvasProviderService::new(db.clone(), secrets));
        let saved = providers
            .save_provider(CanvasProviderInput {
                name: provider_name.to_string(),
                protocol: "openai".into(),
                base_url: base_url.to_string(),
                enabled: true,
                models: vec![CanvasModelConfig {
                    id: model_id.to_string(),
                    name: model_id.to_string(),
                    description: String::new(),
                    enabled: true,
                    scenarios,
                    priority: 0,
                    max_reference_images: None,
                }],
                api_key: Some("sk-audit-placeholder".into()),
                ..Default::default()
            })
            .unwrap();
        let artifacts = Arc::new(
            CanvasArtifactService::new(root.path().join("artifacts"), db.clone()).unwrap(),
        );
        let gateway =
            Arc::new(CanvasModelGateway::new(providers.clone()).with_artifacts(artifacts.clone()));
        let chat = ChatService::new(
            db.clone(),
            settings,
            Arc::new(MockTransport::default()),
            root.path(),
        )
        .with_canvas_model(gateway);
        let session = chat.create_session(None).unwrap();
        let workflow = build_workflow(&saved.id);
        let approvals = Arc::new(AtomicUsize::new(0));
        Self {
            _root: root,
            db,
            chat,
            session,
            artifacts,
            providers,
            workflow,
            live: Arc::new(Mutex::new(Vec::new())),
            approvals,
        }
    }

    fn spawn(&self, run_id: &str) -> JoinHandle<deepagent_core::error::Result<String>> {
        self.spawn_with_approval(run_id, true)
    }

    fn spawn_with_approval(
        &self,
        run_id: &str,
        approve: bool,
    ) -> JoinHandle<deepagent_core::error::Result<String>> {
        let chat = self.chat.clone();
        let workflow = self.workflow.clone();
        let session = self.session.clone();
        let run_id = run_id.to_string();
        let live = self.live.clone();
        let approvals = self.approvals.clone();
        let approval_chat = chat.clone();
        tokio::spawn(async move {
            chat.run_workflow(
                workflow,
                run_id,
                session,
                move |event| {
                    if let RuntimeEvent::WorkflowNode { event } = event {
                        live.lock().unwrap().push(event);
                    }
                },
                move |approval| {
                    approvals.fetch_add(1, Ordering::SeqCst);
                    let _ =
                        approval_chat.resolve_approval(&approval.call_id, approve, "media-fixture");
                },
            )
            .await
        })
    }
}

async fn finish_run(
    run: &mut JoinHandle<deepagent_core::error::Result<String>>,
) -> deepagent_core::error::Result<String> {
    match tokio::time::timeout(Duration::from_secs(5), &mut *run).await {
        Ok(joined) => joined.unwrap(),
        Err(_) => {
            run.abort();
            let _ = run.await;
            panic!("audit workflow did not exit after releasing local HTTP response");
        }
    }
}

#[tokio::test]
async fn real_video_workflow_stores_the_download_and_one_success_terminal() {
    let mut provider = LocalVideoProvider::start_with_task_id("Succeed", TASK_ID).await;
    let fixture = MediaFixture::new(&provider).await;
    let run_id = "audit-video-success";
    let mut run = fixture.spawn(run_id);
    provider.wait_for_poll().await;
    provider.release_poll.notify_one();
    let result = finish_run(&mut run).await;
    provider.close().await;
    result.unwrap();
    let live = fixture.live.lock().unwrap();
    let video = live
        .iter()
        .find(|event| event.node_id == "video" && event.status == NodeExecutionStatus::Completed)
        .unwrap();
    assert_eq!(video.job_id.as_deref(), Some(TASK_ID));
    let uri = &video.artifacts[0];
    assert_eq!(
        fixture
            .artifacts
            .read_bytes(uri.trim_start_matches("artifact://"), None)
            .unwrap()
            .unwrap(),
        VIDEO_BYTES
    );
    let events = RunStore::new(&fixture.db)
        .events_after(run_id, None)
        .unwrap();
    let terminals: Vec<_> = events
        .iter()
        .filter(|event| event.event_type == "run_terminal")
        .collect();
    assert_eq!(terminals.len(), 1);
    assert_eq!(terminals[0].data["kind"], "succeeded");
    let requests = provider.requests.lock().unwrap();
    assert_eq!(requests.len(), 3);
    assert_eq!(requests[1].1["requestId"], TASK_ID);
}

#[tokio::test]
#[ignore = "audit: a provider-controlled video URL must not fetch a host-private destination"]
async fn audit_provider_video_url_cannot_fetch_loopback_resource() {
    let mut provider = LocalVideoUrlProvider::start().await;
    let fixture = MediaFixture::new_video_url(&provider).await;
    let run_id = "audit-video-provider-url-ssrf";
    let mut run = fixture.spawn(run_id);
    let result = finish_run(&mut run).await;
    provider.close().await;
    result.unwrap();

    let events = RunStore::new(&fixture.db)
        .events_after(run_id, None)
        .unwrap();
    let terminal = events
        .iter()
        .find(|event| event.event_type == "run_terminal")
        .and_then(|event| event.data["kind"].as_str())
        .unwrap_or("missing");
    let video = fixture
        .live
        .lock()
        .unwrap()
        .iter()
        .find(|event| event.node_id == "video" && event.status == NodeExecutionStatus::Completed)
        .cloned()
        .expect("video node must complete before checking second-hop evidence");
    let stored = fixture
        .artifacts
        .read_bytes(video.artifacts[0].trim_start_matches("artifact://"), None)
        .unwrap()
        .unwrap();
    let private_requests = provider.private_requests.lock().unwrap().clone();
    eprintln!(
        "provider video URL: private_requests={private_requests:?}, approvals={}, terminal={terminal}, artifact_bytes={}, canary_stored={}",
        fixture.approvals.load(Ordering::SeqCst),
        stored.len(),
        stored == VIDEO_BYTES
    );
    assert!(
        private_requests.is_empty() && stored != VIDEO_BYTES,
        "provider-controlled video URL must not fetch a host-private destination"
    );
}

#[tokio::test]
#[ignore = "audit: provider video downloads must enforce a bounded artifact size"]
async fn audit_provider_video_download_rejects_oversized_artifact() {
    let configured_limit = 32 * 1024 * 1024;
    let response_bytes = configured_limit + 1;
    let mut oversized = vec![0u8; response_bytes];
    oversized[..12].copy_from_slice(b"\0\0\0\x14ftypmp42");
    let mut provider = LocalVideoProvider::start_with_task_id_and_video_bytes(
        "Succeed",
        "audit-video-oversized",
        oversized,
    )
    .await;
    let fixture = MediaFixture::new(&provider).await;
    let run_id = "audit-video-oversized-download";
    let mut run = fixture.spawn(run_id);
    provider.wait_for_poll().await;
    provider.release_poll.notify_one();
    let result = finish_run(&mut run).await;
    provider.close().await;
    let image = fixture
        .live
        .lock()
        .unwrap()
        .iter()
        .find(|event| event.node_id == "video" && event.status == NodeExecutionStatus::Completed)
        .cloned();
    let artifact_bytes = image
        .as_ref()
        .and_then(|event| event.artifacts.first())
        .and_then(|uri| {
            fixture
                .artifacts
                .read_bytes(uri.trim_start_matches("artifact://"), None)
                .ok()
        })
        .flatten()
        .map(|bytes| bytes.len());
    let terminal = RunStore::new(&fixture.db)
        .events_after(run_id, None)
        .unwrap()
        .into_iter()
        .find(|event| event.event_type == "run_terminal")
        .and_then(|event| event.data["kind"].as_str().map(str::to_owned));
    eprintln!(
        "video download size: configured_input_limit={configured_limit}, response_bytes={response_bytes}, artifact_bytes={artifact_bytes:?}, terminal={terminal:?}, result_ok={}",
        result.is_ok()
    );
    assert!(
        result.is_err() && artifact_bytes.is_none() && terminal.as_deref() != Some("succeeded"),
        "provider video downloads must be rejected before artifact persistence"
    );
}

#[tokio::test]
async fn audit_video_cancel_does_not_wait_for_provider_status_response() {
    let mut provider = LocalVideoProvider::start("Failed").await;
    let fixture = MediaFixture::new(&provider).await;
    let run_id = "audit-video-cancel-pending-status";
    let mut run = fixture.spawn(run_id);
    provider.wait_for_poll().await;
    let started = Instant::now();
    let accepted = fixture.chat.request_cancel(run_id).unwrap().accepted;
    let signal_ms = started.elapsed().as_millis();
    let stopped = tokio::time::timeout(Duration::from_millis(300), &mut run).await;
    let stopped_before_response = stopped.is_ok();
    let unfinished_before_response = RunStore::new(&fixture.db)
        .get(run_id)
        .unwrap()
        .unwrap()
        .finished_at
        .is_none();
    provider.release_poll.notify_one();
    let result = match stopped {
        Ok(joined) => joined.unwrap(),
        Err(_) => finish_run(&mut run).await,
    };
    let cancel_to_exit_ms = started.elapsed().as_millis();
    provider.close().await;
    result.unwrap();
    assert!(accepted);
    let events = RunStore::new(&fixture.db)
        .events_after(run_id, None)
        .unwrap();
    let terminals: Vec<_> = events
        .iter()
        .filter(|event| event.event_type == "run_terminal")
        .collect();
    assert_eq!(terminals.len(), 1);
    assert_eq!(terminals[0].data["kind"], "cancelled");
    let live = fixture.live.lock().unwrap();
    assert!(live
        .iter()
        .any(|event| event.node_id == "video" && event.status == NodeExecutionStatus::Cancelled));
    assert!(!live
        .iter()
        .any(|event| event.node_id == "end" && event.status == NodeExecutionStatus::Completed));
    assert_eq!(
        provider.requests.lock().unwrap().len(),
        2,
        "no download or extra submit after cancellation"
    );
    eprintln!("video cancellation: accepted={accepted}, signal_ms={signal_ms}, stopped_within_300ms={stopped_before_response}, unfinished_before_response={unfinished_before_response}, cancel_to_exit_ms={cancel_to_exit_ms}, terminal=cancelled");
    assert!(
        stopped_before_response,
        "accepted cancellation must stop the workflow without waiting for video/status to respond"
    );
}

#[tokio::test]
async fn real_text_workflow_completes_after_provider_response() {
    let mut provider = LocalTextProvider::start().await;
    let fixture = MediaFixture::new_text(&provider).await;
    let run_id = "audit-text-normal-control";
    let mut run = fixture.spawn(run_id);
    provider.wait_for_request().await;
    provider.release_response.notify_one();
    let result = finish_run(&mut run).await;
    provider.close().await;
    result.unwrap();

    let events = fixture.live.lock().unwrap();
    assert!(events.iter().any(|event| {
        event.node_id == "text"
            && event.status == NodeExecutionStatus::Completed
            && event.outputs.is_some()
    }));
    let terminals = RunStore::new(&fixture.db)
        .events_after(run_id, None)
        .unwrap()
        .into_iter()
        .filter(|event| event.event_type == "run_terminal")
        .collect::<Vec<_>>();
    assert_eq!(terminals.len(), 1);
    assert_eq!(terminals[0].data["kind"], "succeeded");
}

#[tokio::test]
#[ignore = "audit: an accepted workflow cancellation must interrupt a pending Canvas text response"]
async fn audit_text_cancel_does_not_wait_for_provider_response() {
    let mut provider = LocalTextProvider::start().await;
    let fixture = MediaFixture::new_text(&provider).await;
    let run_id = "audit-text-cancel-pending-response";
    let mut run = fixture.spawn(run_id);
    provider.wait_for_request().await;
    let started = Instant::now();
    let accepted = fixture.chat.request_cancel(run_id).unwrap().accepted;
    let stopped = tokio::time::timeout(Duration::from_millis(300), &mut run).await;
    let stopped_before_response = stopped.is_ok();
    let unfinished_before_response = RunStore::new(&fixture.db)
        .get(run_id)
        .unwrap()
        .unwrap()
        .finished_at
        .is_none();
    provider.release_response.notify_one();
    let result = match stopped {
        Ok(joined) => joined.unwrap(),
        Err(_) => finish_run(&mut run).await,
    };
    let cancel_to_exit_ms = started.elapsed().as_millis();
    provider.close().await;
    result.unwrap();

    let events = RunStore::new(&fixture.db)
        .events_after(run_id, None)
        .unwrap();
    let terminals: Vec<_> = events
        .iter()
        .filter(|event| event.event_type == "run_terminal")
        .collect();
    let live = fixture.live.lock().unwrap();
    let text_completed = live
        .iter()
        .any(|event| event.node_id == "text" && event.status == NodeExecutionStatus::Completed);
    let text_cancelled = live
        .iter()
        .any(|event| event.node_id == "text" && event.status == NodeExecutionStatus::Cancelled);
    let requests = provider.requests.lock().unwrap();
    eprintln!(
        "text cancellation: accepted={accepted}, stopped_within_300ms={stopped_before_response}, unfinished_before_response={unfinished_before_response}, cancel_to_exit_ms={cancel_to_exit_ms}, text_completed={text_completed}, text_cancelled={text_cancelled}, requests={}",
        requests.len()
    );

    assert!(accepted);
    assert_eq!(requests.len(), 1);
    assert!(requests[0].0.starts_with("POST /v1/chat/completions "));
    assert_eq!(terminals.len(), 1);
    assert_eq!(terminals[0].data["kind"], "cancelled");
    assert!(
        stopped_before_response,
        "accepted cancellation must stop the workflow without waiting for a text response"
    );
    assert!(
        !text_completed && text_cancelled,
        "a cancelled in-flight text node must not publish a completed outcome"
    );
}

#[tokio::test]
#[ignore = "audit: Canvas provider error responses must be bounded before classification"]
async fn audit_canvas_provider_error_body_is_bounded() {
    let response_bytes = 32 * 1024 * 1024 + 1;
    let mut provider = LocalErrorTextProvider::start(response_bytes).await;
    let fixture = MediaFixture::new_error_text(&provider).await;
    let run_id = "audit-canvas-error-body-budget";
    let mut run = fixture.spawn(run_id);
    let result = finish_run(&mut run).await;
    provider.close().await;

    let events = RunStore::new(&fixture.db)
        .events_after(run_id, None)
        .unwrap();
    let terminal = events
        .iter()
        .find(|event| event.event_type == "run_terminal")
        .and_then(|event| event.data["kind"].as_str())
        .unwrap_or("missing");
    let live = fixture.live.lock().unwrap();
    let text_error = live
        .iter()
        .find(|event| event.node_id == "text" && event.status == NodeExecutionStatus::Failed)
        .and_then(|event| event.error.clone());
    let error_len = text_error.as_ref().map(|error| error.len()).unwrap_or(0);
    let sent_bytes = provider.sent_bytes.load(Ordering::SeqCst);
    eprintln!(
        "canvas error body: response_bytes={response_bytes}, sent_bytes={sent_bytes}, error_message_bytes={error_len}, terminal={terminal}, result_ok={}",
        result.is_ok()
    );
    assert!(
        result.is_err() && terminal != "succeeded" && text_error.is_some(),
        "provider error must fail the node and run"
    );
    assert!(
        sent_bytes < response_bytes,
        "the gateway must stop an oversized error response before the provider sends the complete body"
    );
}

#[tokio::test]
async fn h252_generated_audio_video_keep_scope_and_private_image_download_stays_denied() {
    fn assert_output(fixture: &MediaFixture, run_id: &str, node: &str, bytes: &[u8]) {
        let live = fixture.live.lock().unwrap();
        let event = live
            .iter()
            .find(|event| event.node_id == node && event.status == NodeExecutionStatus::Completed)
            .unwrap();
        let id = event.artifacts[0].trim_start_matches("artifact://");
        let record = fixture.artifacts.record(id, Some("ws-a")).unwrap().unwrap();
        assert_eq!(record.workspace_id.as_deref(), Some("ws-a"));
        assert_eq!(
            fixture
                .artifacts
                .read_bytes(id, Some("ws-a"))
                .unwrap()
                .unwrap(),
            bytes
        );
        for foreign in [None, Some("ws-b")] {
            assert!(fixture.artifacts.read_bytes(id, foreign).unwrap().is_none());
            assert!(!fixture.artifacts.delete(id, foreign).unwrap());
        }
        let terminal: Vec<_> = RunStore::new(&fixture.db)
            .events_after(run_id, None)
            .unwrap()
            .into_iter()
            .filter(|event| event.event_type == "run_terminal")
            .collect();
        assert_eq!(terminal.len(), 1);
        assert_eq!(terminal[0].data["kind"], "succeeded");
    }
    let mut video = LocalVideoProvider::start("Succeed").await;
    let mut fixture = MediaFixture::new(&video).await;
    fixture.workflow.workspace_id = Some("ws-a".into());
    let mut run = fixture.spawn("h252-scoped-video");
    video.wait_for_poll().await;
    video.release_poll.notify_one();
    let result = finish_run(&mut run).await;
    video.close().await;
    result.unwrap();
    assert_output(&fixture, "h252-scoped-video", "video", VIDEO_BYTES);

    let mut audio = LocalPendingAudioProvider::start().await;
    let mut fixture = MediaFixture::new_audio(&audio).await;
    fixture.workflow.workspace_id = Some("ws-a".into());
    let mut run = fixture.spawn("h252-scoped-audio");
    audio.wait_for_request().await;
    audio.release_response.notify_one();
    let result = finish_run(&mut run).await;
    audio.close().await;
    result.unwrap();
    assert_output(&fixture, "h252-scoped-audio", "audio", AUDIO_BYTES);

    let mut image = LocalImageProvider::start().await;
    let mut fixture = MediaFixture::new_image(&image).await;
    fixture.workflow.workspace_id = Some("ws-a".into());
    let mut run = fixture.spawn("h252-scoped-deferred-image");
    let result = finish_run(&mut run).await;
    image.close().await;
    assert!(result.unwrap_err().to_string().contains("NetworkDenied"));
    assert_eq!(image.provider_requests.lock().unwrap().len(), 1);
    assert!(image.private_requests.lock().unwrap().is_empty());
    for scope in [None, Some("ws-a"), Some("ws-b")] {
        assert!(fixture.artifacts.list(scope, None).unwrap().is_empty());
    }
    let terminals: Vec<_> = RunStore::new(&fixture.db)
        .events_after("h252-scoped-deferred-image", None)
        .unwrap()
        .into_iter()
        .filter(|event| event.event_type == "run_terminal")
        .collect();
    assert_eq!(terminals.len(), 1);
    assert_eq!(terminals[0].data["kind"], "failed");
}

#[tokio::test]
async fn h252_cross_workspace_images_fail_before_provider_and_persist_one_terminal() {
    use deepagent_persistence::artifact_store::ArtifactKind;
    for caller in [None, Some("ws-a"), Some("ws-b")] {
        for owner in [None, Some("ws-a"), Some("ws-b")] {
            if caller == owner {
                continue;
            }
            let mut provider = LocalImageProvider::start().await;
            let mut fixture = MediaFixture::new_image(&provider).await;
            fixture.workflow.workspace_id = caller.map(str::to_string);
            let foreign = fixture
                .artifacts
                .import_bytes(
                    ArtifactKind::Image,
                    Some("image/png"),
                    PRIVATE_IMAGE_BYTES,
                    owner,
                )
                .unwrap();
            let config = &mut fixture.workflow.definition.nodes[1].config;
            config.insert("referenceImages".into(), json!([foreign.uri]));
            config.insert("operation".into(), json!("image_edit"));
            let run_id = "h252-cross-scope-image";
            let mut run = fixture.spawn(run_id);
            let result = finish_run(&mut run).await;
            provider.close().await;
            assert!(result.is_err(), "caller {caller:?}, owner {owner:?}");
            assert!(provider.provider_requests.lock().unwrap().is_empty());
            assert!(provider.private_requests.lock().unwrap().is_empty());
            let events = RunStore::new(&fixture.db)
                .events_after(run_id, None)
                .unwrap();
            let terminal: Vec<_> = events
                .iter()
                .filter(|event| event.event_type == "run_terminal")
                .collect();
            assert_eq!(terminal.len(), 1);
            assert_eq!(terminal[0].data["kind"], "failed");
            assert!(events
                .iter()
                .any(|event| event.data.to_string().contains("requested workspace")));
            let live = fixture.live.lock().unwrap();
            assert!(live.iter().any(
                |event| event.node_id == "image" && event.status == NodeExecutionStatus::Failed
            ));
            assert!(!live
                .iter()
                .any(|event| event.node_id == "end"
                    && event.status == NodeExecutionStatus::Completed));
            assert_eq!(
                fixture
                    .artifacts
                    .read_bytes(&foreign.id, owner)
                    .unwrap()
                    .unwrap(),
                PRIVATE_IMAGE_BYTES
            );
        }
    }
}

#[tokio::test]
async fn h254_pending_artifact_fails_before_provider_and_replays_one_failed_terminal() {
    use deepagent_core::error::CoreError;
    use deepagent_persistence::artifact_store::ArtifactKind;
    for owner in [None, Some("ws-a"), Some("ws-b")] {
        let mut provider = LocalImageProvider::start().await;
        let mut fixture = MediaFixture::new_image(&provider).await;
        fixture.workflow.workspace_id = owner.map(str::to_string);
        let artifact = fixture
            .artifacts
            .import_bytes(
                ArtifactKind::Image,
                Some("image/png"),
                PRIVATE_IMAGE_BYTES,
                owner,
            )
            .unwrap();
        fixture
            .db
            .with_conn(|conn| {
                conn.execute_batch(
                    "CREATE TRIGGER h254_receipt BEFORE DELETE ON documents
                WHEN OLD.collection='artifact_deletions_v1'
                BEGIN SELECT RAISE(ABORT,'synthetic receipt failure'); END;",
                )
                .map_err(|error| CoreError::other(error.to_string()))
            })
            .unwrap();
        assert!(matches!(
            fixture.artifacts.delete(&artifact.id, owner).unwrap_err(),
            CoreError::ArtifactPendingDelete { .. }
        ));
        let config = &mut fixture.workflow.definition.nodes[1].config;
        config.insert("referenceImages".into(), json!([artifact.uri]));
        config.insert("operation".into(), json!("image_edit"));
        let run_id = "h254-pending-reference";
        let mut run = fixture.spawn(run_id);
        let result = finish_run(&mut run).await;
        provider.close().await;
        assert!(result.is_err());
        assert!(provider.provider_requests.lock().unwrap().is_empty());
        assert!(provider.private_requests.lock().unwrap().is_empty());
        let events = RunStore::new(&fixture.db)
            .events_after(run_id, None)
            .unwrap();
        let terminals: Vec<_> = events
            .iter()
            .filter(|event| event.event_type == "run_terminal")
            .collect();
        assert_eq!(terminals.len(), 1);
        assert_eq!(terminals[0].data["kind"], "failed");
        assert!(events
            .iter()
            .any(|event| event.data.to_string().contains("artifact_pending_delete")));
        assert!(!fixture
            .live
            .lock()
            .unwrap()
            .iter()
            .any(|event| event.node_id == "end" && event.status == NodeExecutionStatus::Completed));
        fixture
            .db
            .with_conn(|conn| {
                conn.execute_batch("DROP TRIGGER h254_receipt;")
                    .map_err(|error| CoreError::other(error.to_string()))
            })
            .unwrap();
        assert!(fixture.artifacts.delete(&artifact.id, owner).unwrap());
        assert!(fixture
            .artifacts
            .record(&artifact.id, owner)
            .unwrap()
            .is_none());
    }
}

#[tokio::test]
async fn h252_same_workspace_images_reach_provider_and_keep_output_ownership() {
    use deepagent_persistence::artifact_store::ArtifactKind;
    for caller in [None, Some(" ws-a "), Some("ws-b")] {
        let owner = caller.map(str::trim);
        let mut provider = LocalInlineImageProvider::start(PRIVATE_IMAGE_BYTES.to_vec()).await;
        let mut fixture = MediaFixture::new_inline_image(&provider).await;
        fixture.workflow.workspace_id = caller.map(str::to_string);
        let input = fixture
            .artifacts
            .import_bytes(
                ArtifactKind::Image,
                Some("image/png"),
                PRIVATE_IMAGE_BYTES,
                owner,
            )
            .unwrap();
        fixture.workflow.definition.nodes[1]
            .config
            .insert("referenceImages".into(), json!([input.uri]));
        fixture.workflow.definition.nodes[1]
            .config
            .insert("operation".into(), json!("image_edit"));
        let run_id = "h252-same-scope-image";
        let mut run = fixture.spawn(run_id);
        let result = finish_run(&mut run).await;
        if result.is_err() {
            provider.server.abort();
        }
        provider.close().await;
        result.unwrap();
        let requests = provider.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert!(requests[0].0.starts_with("POST /v1/images/edits "));
        use base64::Engine as _;
        assert_eq!(
            requests[0].1["image"][0],
            format!(
                "data:image/png;base64,{}",
                base64::engine::general_purpose::STANDARD.encode(PRIVATE_IMAGE_BYTES)
            )
        );
        let live = fixture.live.lock().unwrap();
        let output = live
            .iter()
            .find(|event| {
                event.node_id == "image" && event.status == NodeExecutionStatus::Completed
            })
            .unwrap();
        let id = output.artifacts[0].trim_start_matches("artifact://");
        let record = fixture.artifacts.record(id, owner).unwrap().unwrap();
        assert_eq!(record.workspace_id.as_deref(), owner);
        assert_eq!(
            fixture.artifacts.read_bytes(id, owner).unwrap().unwrap(),
            PRIVATE_IMAGE_BYTES
        );
        for other in [None, Some("ws-a"), Some("ws-b")] {
            if other != owner {
                assert!(fixture.artifacts.read_bytes(id, other).unwrap().is_none());
            }
        }
        let terminal: Vec<_> = RunStore::new(&fixture.db)
            .events_after(run_id, None)
            .unwrap()
            .into_iter()
            .filter(|event| event.event_type == "run_terminal")
            .collect();
        assert_eq!(terminal.len(), 1);
        assert_eq!(terminal[0].data["kind"], "succeeded");
    }
}

#[tokio::test]
async fn real_image_workflow_stores_one_successful_artifact() {
    let mut provider = LocalInlineImageProvider::start(PRIVATE_IMAGE_BYTES.to_vec()).await;
    let fixture = MediaFixture::new_inline_image(&provider).await;
    let run_id = "audit-image-normal-control";
    let mut run = fixture.spawn(run_id);
    let result = finish_run(&mut run).await;
    provider.close().await;
    result.unwrap();
    let image = fixture
        .live
        .lock()
        .unwrap()
        .iter()
        .find(|event| event.node_id == "image" && event.status == NodeExecutionStatus::Completed)
        .cloned()
        .expect("normal image control must complete");
    let uri = image
        .artifacts
        .first()
        .expect("the image node must publish its stored artifact")
        .to_owned();
    assert_eq!(
        fixture
            .artifacts
            .read_bytes(uri.trim_start_matches("artifact://"), None)
            .unwrap()
            .unwrap(),
        PRIVATE_IMAGE_BYTES
    );
    let terminal = RunStore::new(&fixture.db)
        .events_after(run_id, None)
        .unwrap()
        .into_iter()
        .filter(|event| event.event_type == "run_terminal")
        .collect::<Vec<_>>();
    assert_eq!(terminal.len(), 1);
    assert_eq!(terminal[0].data["kind"], "succeeded");
    eprintln!(
        "image normal control: artifact_bytes={}, terminal=succeeded",
        PRIVATE_IMAGE_BYTES.len()
    );
}

#[tokio::test]
async fn real_embedding_workflow_reaches_provider_and_completes() {
    let provider = LocalEmbeddingProvider::start().await;
    let fixture = MediaFixture::new_embedding(&provider).await;
    let run_id = "audit-embedding-normal-control";
    let mut run = fixture.spawn(run_id);
    let result = finish_run(&mut run).await;
    let provider_called = tokio::time::timeout(
        Duration::from_millis(50),
        provider.request_received.notified(),
    )
    .await
    .is_ok();
    result.unwrap();

    let embedding = fixture
        .live
        .lock()
        .unwrap()
        .iter()
        .find(|event| {
            event.node_id == "embedding" && event.status == NodeExecutionStatus::Completed
        })
        .cloned()
        .expect("normal embedding control must complete");
    assert!(provider_called);
    assert_eq!(
        provider.requests.lock().unwrap().as_slice(),
        &[json!({"model": "audit-embedding-model", "input": "audit embedding"})]
    );
    drop(provider);
    let outputs = embedding
        .outputs
        .expect("embedding output must be published");
    assert_eq!(outputs["dimensions"], 3);
    assert_eq!(outputs["count"], 1);
    assert_eq!(
        outputs.as_object().unwrap().len(),
        4,
        "vector payloads must stay out of node events"
    );
    assert!(embedding
        .provider_id
        .as_deref()
        .is_some_and(|provider_id| !provider_id.is_empty()));
    assert_eq!(embedding.model_id.as_deref(), Some("audit-embedding-model"));
    assert_eq!(outputs["modelId"], "audit-embedding-model");
    assert_eq!(
        outputs["providerId"],
        embedding.provider_id.as_deref().unwrap()
    );

    let terminals = RunStore::new(&fixture.db)
        .events_after(run_id, None)
        .unwrap()
        .into_iter()
        .filter(|event| event.event_type == "run_terminal")
        .collect::<Vec<_>>();
    assert_eq!(terminals.len(), 1);
    assert_eq!(terminals[0].data["kind"], "succeeded");
}

#[tokio::test]
async fn embedding_pending_response_cancellation_uses_shared_terminal_boundary() {
    let mut provider = LocalEmbeddingProvider::start_pending().await;
    let mut fixture = MediaFixture::new_embedding(&provider).await;
    let cost = Arc::new(CostService::new(fixture.db.clone()));
    fixture.chat = fixture.chat.clone().with_cost(cost.clone());
    let run_id = "audit-embedding-pending-cancellation";
    let mut run = fixture.spawn(run_id);
    provider.wait_for_request().await;
    let started = Instant::now();
    let accepted = fixture.chat.request_cancel(run_id).unwrap().accepted;
    let stopped = tokio::time::timeout(Duration::from_millis(300), &mut run).await;
    let stopped_before_response = stopped.is_ok();
    let live_before_response = fixture.live.lock().unwrap().clone();
    let durable_before_response = RunStore::new(&fixture.db)
        .events_after(run_id, None)
        .unwrap();
    // Always release and join the synthetic server, even on a regression. A
    // late response is not permission to publish a completed node or usage.
    provider.close().await;
    let result = match stopped {
        Ok(joined) => joined.unwrap(),
        Err(_) => finish_run(&mut run).await,
    };
    let cancel_to_exit_ms = started.elapsed().as_millis();
    result.unwrap();
    eprintln!(
        "embedding cancellation: accepted={accepted}, stopped_within_300ms={stopped_before_response}, cancel_to_exit_ms={cancel_to_exit_ms}"
    );
    assert!(accepted);
    assert!(
        stopped_before_response,
        "cancellation must not await provider response"
    );
    assert_eq!(provider.requests.lock().unwrap().len(), 1);
    assert_eq!(
        fixture.live.lock().unwrap().as_slice(),
        live_before_response
    );
    assert_eq!(
        RunStore::new(&fixture.db)
            .events_after(run_id, None)
            .unwrap()
            .len(),
        durable_before_response.len(),
        "late provider data must not append new events after the durable terminal"
    );
    let node_terminals: Vec<_> = live_before_response
        .iter()
        .filter(|event| event.node_id == "embedding" && event.status.is_terminal())
        .collect();
    assert_eq!(node_terminals.len(), 1);
    assert_eq!(node_terminals[0].status, NodeExecutionStatus::Cancelled);
    assert!(node_terminals[0].outputs.is_none());
    assert!(CostStore::new(&fixture.db)
        .session_costs_detailed(&fixture.session)
        .unwrap()
        .is_empty());
    let session_id = fixture.session.clone();
    let MediaFixture {
        _root: root,
        db,
        chat,
        artifacts,
        providers,
        ..
    } = fixture;
    drop(cost);
    drop(chat);
    drop(artifacts);
    drop(providers);
    drop(db);
    let reopened = Database::open(root.path().join("media.db")).unwrap();
    let stored = RunStore::new(&reopened).events_after(run_id, None).unwrap();
    assert!(stored
        .windows(2)
        .all(|pair| pair[0].sequence < pair[1].sequence));
    let terminals: Vec<_> = stored
        .iter()
        .filter(|event| event.event_type == "run_terminal")
        .collect();
    assert_eq!(terminals.len(), 1);
    assert_eq!(terminals[0].data["kind"], "cancelled");
    let replayed: Vec<NodeExecutionEvent> = stored
        .iter()
        .filter(|event| event.event_type == "workflow_node")
        .map(|event| serde_json::from_value(event.data["event"].clone()).unwrap())
        .filter(|event: &NodeExecutionEvent| {
            event.node_id == "embedding" && event.status.is_terminal()
        })
        .collect();
    assert_eq!(replayed.len(), 1);
    assert_eq!(replayed[0].status, NodeExecutionStatus::Cancelled);
    let history = EventStore::new(&reopened)
        .load_session(session_id.parse().unwrap())
        .unwrap();
    let receipts: Vec<_> = history
        .iter()
        .filter_map(|event| match &event.payload {
            EventPayload::RunCostRecorded {
                run_id: id,
                ledger_ids,
                cost_yuan,
                ..
            } if id == run_id => Some((ledger_ids, cost_yuan)),
            _ => None,
        })
        .collect();
    assert_eq!(receipts.len(), 1);
    assert!(receipts[0].0.is_empty());
    assert!(
        receipts[0].1.is_none(),
        "unreported usage cannot become a measured zero cost"
    );
}

#[tokio::test]
async fn audit_embedding_node_events_survive_database_reopen() {
    let provider = LocalEmbeddingProvider::start().await;
    let fixture = MediaFixture::new_embedding(&provider).await;
    let run_id = "audit-embedding-node-replay";
    let mut run = fixture.spawn(run_id);
    let result = finish_run(&mut run).await;
    drop(provider);
    result.unwrap();

    let live = fixture.live.lock().unwrap().clone();
    let embedding = live
        .iter()
        .find(|event| {
            event.node_id == "embedding" && event.status == NodeExecutionStatus::Completed
        })
        .expect("embedding completion must be emitted");
    let expected_provider = embedding
        .provider_id
        .as_deref()
        .expect("embedding completion must include provider provenance")
        .to_owned();
    assert_eq!(embedding.model_id.as_deref(), Some("audit-embedding-model"));

    let MediaFixture {
        _root: root,
        db,
        chat,
        artifacts,
        providers,
        ..
    } = fixture;
    drop(chat);
    drop(artifacts);
    drop(providers);
    drop(db);

    let reopened = Arc::new(Database::open(root.path().join("media.db")).unwrap());
    let stored = RunStore::new(&reopened).events_after(run_id, None).unwrap();
    assert!(stored
        .windows(2)
        .all(|events| events[0].sequence < events[1].sequence));
    let cursor = stored.first().expect("durable run events").sequence;
    let resumed = RunStore::new(&reopened)
        .events_after(run_id, Some(cursor))
        .unwrap();
    assert_eq!(resumed.len(), stored.len() - 1);
    assert!(resumed.iter().all(|event| event.sequence > cursor));
    let replayed: Vec<NodeExecutionEvent> = stored
        .iter()
        .filter(|event| event.event_type == "workflow_node")
        .map(|event| serde_json::from_value(event.data["event"].clone()).unwrap())
        .collect();
    let replayed_embedding = replayed
        .iter()
        .find(|event| {
            event.node_id == "embedding" && event.status == NodeExecutionStatus::Completed
        })
        .expect("embedding completion must survive database reopen");
    assert_eq!(
        replayed_embedding.model_id.as_deref(),
        Some("audit-embedding-model")
    );
    assert_eq!(
        replayed_embedding.provider_id.as_deref(),
        Some(expected_provider.as_str())
    );
    assert_eq!(
        replayed_embedding.outputs.as_ref().unwrap()["dimensions"],
        3
    );
    assert_eq!(
        stored
            .iter()
            .filter(|event| event.event_type == "run_terminal")
            .count(),
        1
    );
    assert_eq!(
        stored
            .iter()
            .find(|event| event.event_type == "run_terminal")
            .unwrap()
            .data["kind"],
        "succeeded"
    );
}

#[tokio::test]
async fn audit_embedding_workflow_reaches_registered_provider() {
    let provider = LocalEmbeddingProvider::start().await;
    let fixture = MediaFixture::new_embedding(&provider).await;
    let run_id = "audit-embedding-provider-reachability";
    let mut run = fixture.spawn(run_id);
    let result = finish_run(&mut run).await;
    let provider_called = tokio::time::timeout(
        Duration::from_millis(50),
        provider.request_received.notified(),
    )
    .await
    .is_ok();
    eprintln!(
        "embedding provider reachability: result={result:?}, provider_called={provider_called}"
    );
    drop(provider);
    assert!(
        result.is_ok(),
        "an enabled embedding workflow must not fail before provider dispatch"
    );
    assert!(
        provider_called,
        "the configured embedding provider must receive a request"
    );
}

#[tokio::test]
#[ignore = "audit: requires local Node and desktop dependencies; actual embedding TS/Rust seam"]
async fn audit_desktop_embedding_dto_reaches_registered_provider() {
    let provider = LocalEmbeddingProvider::start().await;
    let mut fixture = MediaFixture::new_embedding(&provider).await;
    let model_ref = fixture.workflow.definition.nodes[1].config["embeddingModel"]
        .as_str()
        .unwrap();
    let provider_id = model_ref.split_once("::").unwrap().0;
    let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../apps/desktop/scripts/audit-workflow-contract.cjs");
    let output = std::process::Command::new("node")
        .arg(script)
        .args(["--emit-embedding-fixture", provider_id])
        .output()
        .expect("local Node is required for this opt-in audit");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let fixtures: Vec<Value> = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(fixtures.len(), 1);
    fixture.workflow = serde_json::from_value(fixtures[0]["workflow"].clone()).unwrap();
    let mut run = fixture.spawn("audit-desktop-embedding-dto");
    finish_run(&mut run).await.unwrap();
    assert_eq!(provider.requests.lock().unwrap().len(), 1);
    assert!(fixture.live.lock().unwrap().iter().any(|event| {
        event.node_id == "embedding"
            && event.status == NodeExecutionStatus::Completed
            && event.outputs.as_ref().unwrap()["dimensions"] == 3
    }));
}

#[tokio::test]
async fn audit_embedding_provider_usage_reaches_existing_cost_ledger() {
    let provider = LocalEmbeddingProvider::start().await;
    let mut fixture = MediaFixture::new_embedding(&provider).await;
    let cost = Arc::new(CostService::new(fixture.db.clone()));
    fixture.chat = fixture.chat.clone().with_cost(cost.clone());
    let mut run = fixture.spawn("audit-embedding-cost-acceptance");
    finish_run(&mut run).await.unwrap();
    assert_eq!(provider.requests.lock().unwrap().len(), 1);
    let summary = cost.summary(&fixture.session).unwrap();
    eprintln!(
        "embedding provider usage: prompt_tokens=11; ledger input_tokens={}",
        summary.input_tokens
    );
    assert_eq!(
        summary.input_tokens, 11,
        "provider-reported usage must not disappear after a successful embedding"
    );
}

#[tokio::test]
async fn audit_embedding_batch_output_matches_provider_vectors() {
    let provider = LocalEmbeddingProvider::start().await;
    let mut fixture = MediaFixture::new_embedding(&provider).await;
    fixture.workflow.definition.nodes[1]
        .config
        .insert("texts".into(), json!(["first", "second"]));
    let mut run = fixture.spawn("audit-embedding-batch-acceptance");
    finish_run(&mut run).await.unwrap();
    assert_eq!(
        provider.requests.lock().unwrap()[0]["input"],
        json!(["first", "second"])
    );
    let events = fixture.live.lock().unwrap();
    let completed = events
        .iter()
        .find(|event| {
            event.node_id == "embedding" && event.status == NodeExecutionStatus::Completed
        })
        .unwrap();
    let outputs = completed.outputs.as_ref().unwrap();
    eprintln!("embedding batch: provider_vectors=2; node_outputs={outputs}");
    assert_eq!(
        outputs["count"], 2,
        "the registered output contract must not silently discard batch results"
    );
}

#[tokio::test]
async fn embedding_incomplete_provider_batch_fails_without_a_completed_node() {
    let provider = LocalEmbeddingProvider::start_with_response_count(Some(1)).await;
    let mut fixture = MediaFixture::new_embedding(&provider).await;
    fixture.workflow.definition.nodes[1]
        .config
        .insert("texts".into(), json!(["first", "second"]));
    let run_id = "audit-embedding-incomplete-batch";
    let mut run = fixture.spawn(run_id);
    assert!(finish_run(&mut run).await.is_err());
    assert_eq!(provider.requests.lock().unwrap().len(), 1);
    let events = fixture.live.lock().unwrap();
    assert!(events
        .iter()
        .any(|event| event.node_id == "embedding" && event.status == NodeExecutionStatus::Failed));
    assert!(!events.iter().any(
        |event| event.node_id == "embedding" && event.status == NodeExecutionStatus::Completed
    ));
    let stored = RunStore::new(&fixture.db)
        .events_after(run_id, None)
        .unwrap();
    let terminal: Vec<_> = stored
        .iter()
        .filter(|event| event.event_type == "run_terminal")
        .collect();
    assert_eq!(terminal.len(), 1);
    assert_eq!(terminal[0].data["kind"], "failed");
}

#[tokio::test]
#[ignore = "audit: an accepted workflow cancellation must interrupt a pending Canvas audio response"]
async fn audit_audio_cancel_does_not_wait_for_provider_response() {
    let provider = LocalPendingAudioProvider::start().await;
    let fixture = MediaFixture::new_audio(&provider).await;
    let run_id = "audit-audio-cancel-pending-response";
    let mut run = fixture.spawn(run_id);
    provider.wait_for_request().await;
    let started = Instant::now();
    let accepted = fixture.chat.request_cancel(run_id).unwrap().accepted;
    let stopped = tokio::time::timeout(Duration::from_millis(300), &mut run).await;
    let stopped_before_response = stopped.is_ok();
    let unfinished_before_response = RunStore::new(&fixture.db)
        .get(run_id)
        .unwrap()
        .unwrap()
        .finished_at
        .is_none();
    provider.release_response.notify_one();
    let result = match stopped {
        Ok(joined) => joined.unwrap(),
        Err(_) => finish_run(&mut run).await,
    };
    let cancel_to_exit_ms = started.elapsed().as_millis();
    let mut provider = provider;
    provider.close().await;
    let events = fixture.live.lock().unwrap().clone();
    let audio_completed = events
        .iter()
        .any(|event| event.node_id == "audio" && event.status == NodeExecutionStatus::Completed);
    let audio_cancelled = events
        .iter()
        .any(|event| event.node_id == "audio" && event.status == NodeExecutionStatus::Cancelled);
    let terminals = RunStore::new(&fixture.db)
        .events_after(run_id, None)
        .unwrap()
        .into_iter()
        .filter(|event| event.event_type == "run_terminal")
        .collect::<Vec<_>>();
    eprintln!(
        "audio cancellation: accepted={accepted}, stopped_within_300ms={stopped_before_response}, unfinished_before_response={unfinished_before_response}, cancel_to_exit_ms={cancel_to_exit_ms}, audio_completed={audio_completed}, audio_cancelled={audio_cancelled}, terminal={:?}, result_ok={}",
        terminals.first().and_then(|event| event.data["kind"].as_str()),
        result.is_ok()
    );
    assert!(accepted);
    assert!(
        stopped_before_response,
        "accepted cancellation must stop a pending audio provider response"
    );
    assert!(!audio_completed);
    assert!(audio_cancelled);
    assert_eq!(terminals.len(), 1);
    assert_eq!(terminals[0].data["kind"], "cancelled");
}

#[tokio::test]
#[ignore = "audit: an accepted workflow cancellation must interrupt a pending Canvas image response"]
async fn audit_image_cancel_does_not_wait_for_provider_response() {
    let mut provider = LocalPendingImageProvider::start().await;
    let original = LocalImageProvider::start().await;
    let fixture = MediaFixture::new_image(&original).await;
    // Rebind the saved provider to the pending loopback endpoint while keeping
    // the production workflow and gateway construction unchanged.
    let current = fixture
        .providers
        .provider(
            fixture.workflow.definition.nodes[1].config["imageModel"]
                .as_str()
                .unwrap()
                .split("::")
                .next()
                .unwrap(),
        )
        .unwrap();
    fixture
        .providers
        .save_provider(CanvasProviderInput {
            id: current.id,
            name: current.name,
            protocol: "openai".into(),
            base_url: provider.base_url.clone(),
            enabled: true,
            models: current.models,
            api_key: Some("sk-audit-image-pending".into()),
            ..Default::default()
        })
        .unwrap();
    let run_id = "audit-image-cancel-pending-response";
    let mut run = fixture.spawn(run_id);
    provider.wait_for_request().await;
    let accepted = fixture.chat.request_cancel(run_id).unwrap().accepted;
    let started = Instant::now();
    let stopped = tokio::time::timeout(Duration::from_millis(300), &mut run).await;
    let stopped_before_response = stopped.is_ok();
    let unfinished_before_response = RunStore::new(&fixture.db)
        .get(run_id)
        .unwrap()
        .unwrap()
        .finished_at
        .is_none();
    provider.release_response.notify_one();
    let result = match stopped {
        Ok(joined) => joined.unwrap(),
        Err(_) => finish_run(&mut run).await,
    };
    let cancel_to_exit_ms = started.elapsed().as_millis();
    drop(original);
    provider.close().await;
    let events = fixture.live.lock().unwrap().clone();
    let image_completed = events
        .iter()
        .any(|event| event.node_id == "image" && event.status == NodeExecutionStatus::Completed);
    let image_cancelled = events
        .iter()
        .any(|event| event.node_id == "image" && event.status == NodeExecutionStatus::Cancelled);
    let terminals = RunStore::new(&fixture.db)
        .events_after(run_id, None)
        .unwrap()
        .into_iter()
        .filter(|event| event.event_type == "run_terminal")
        .collect::<Vec<_>>();
    eprintln!(
        "image cancellation: accepted={accepted}, stopped_within_300ms={stopped_before_response}, unfinished_before_response={unfinished_before_response}, cancel_to_exit_ms={cancel_to_exit_ms}, image_completed={image_completed}, image_cancelled={image_cancelled}, terminal={:?}, result_ok={}",
        terminals.first().and_then(|event| event.data["kind"].as_str()),
        result.is_ok()
    );
    assert!(accepted);
    assert!(
        stopped_before_response,
        "accepted cancellation must stop a pending image provider response"
    );
    assert!(!image_completed);
    assert!(image_cancelled);
    assert_eq!(terminals.len(), 1);
    assert_eq!(terminals[0].data["kind"], "cancelled");
}

#[tokio::test]
#[ignore = "audit: a provider-controlled image URL must not fetch a host-private destination"]
async fn audit_provider_image_url_cannot_fetch_loopback_resource() {
    let mut provider = LocalImageProvider::start().await;
    let fixture = MediaFixture::new_image(&provider).await;
    let run_id = "audit-image-provider-url-ssrf";
    let mut run = fixture.spawn(run_id);
    let result = finish_run(&mut run).await;
    provider.close().await;

    let events = RunStore::new(&fixture.db)
        .events_after(run_id, None)
        .unwrap();
    let terminals: Vec<_> = events
        .iter()
        .filter(|event| event.event_type == "run_terminal")
        .collect();
    let live = fixture.live.lock().unwrap();
    let image = live
        .iter()
        .rev()
        .find(|event| event.node_id == "image" && event.status.is_terminal())
        .expect("the production image node must emit a terminal failure");
    let provider_requests = provider.provider_requests.lock().unwrap();
    let private_requests = provider.private_requests.lock().unwrap();
    let approvals = fixture.approvals.load(Ordering::SeqCst);
    eprintln!(
        "provider image URL: provider_requests={}, private_requests={private_requests:?}, approvals={approvals}, terminal={}, image_status={:?}, artifacts={:?}, result_ok={}",
        provider_requests.len(),
        terminals[0].data["kind"],
        image.status,
        image.artifacts,
        result.is_ok()
    );

    assert_eq!(provider_requests.len(), 1);
    assert!(provider_requests[0]
        .0
        .starts_with("POST /v1/images/generations "));
    assert_eq!(terminals.len(), 1);
    assert_eq!(approvals, 1);
    assert_ne!(terminals[0].data["kind"], "succeeded");
    assert!(!result.is_ok());
    assert_ne!(image.status, NodeExecutionStatus::Completed);
    assert!(image.artifacts.is_empty());
    assert!(
        private_requests.is_empty(),
        "an untrusted provider response must not make the host fetch a loopback resource"
    );
}

#[tokio::test]
#[ignore = "audit: denying provider image URL access must not fetch or persist an artifact"]
async fn audit_denied_provider_image_url_is_not_fetched_or_persisted() {
    let mut provider = LocalImageProvider::start().await;
    let fixture = MediaFixture::new_image(&provider).await;
    let run_id = "audit-image-provider-url-denied";
    let mut run = fixture.spawn_with_approval(run_id, false);
    let result = finish_run(&mut run).await;
    provider.close().await;

    let events = RunStore::new(&fixture.db)
        .events_after(run_id, None)
        .unwrap();
    let terminals: Vec<_> = events
        .iter()
        .filter(|event| event.event_type == "run_terminal")
        .collect();
    let image = fixture
        .live
        .lock()
        .unwrap()
        .iter()
        .rev()
        .find(|event| event.node_id == "image" && event.status.is_terminal())
        .cloned()
        .expect("image node must emit a terminal result");
    let provider_requests = provider.provider_requests.lock().unwrap();
    let private_requests = provider.private_requests.lock().unwrap();

    assert_eq!(provider_requests.len(), 1);
    assert!(provider_requests[0]
        .0
        .starts_with("POST /v1/images/generations "));
    assert_eq!(fixture.approvals.load(Ordering::SeqCst), 1);
    assert_eq!(terminals.len(), 1);
    assert_ne!(terminals[0].data["kind"], "succeeded");
    assert!(result.is_err());
    assert_eq!(image.status, NodeExecutionStatus::Failed);
    assert!(image.artifacts.is_empty());
    assert!(private_requests.is_empty());
}

#[tokio::test]
#[ignore = "audit: provider image downloads must enforce a bounded artifact size"]
async fn audit_provider_image_download_rejects_oversized_artifact() {
    const MAX_INPUT_IMAGE_BYTES: usize = 32 * 1024 * 1024;
    let mut oversized_image = vec![b'x'; MAX_INPUT_IMAGE_BYTES + 1];
    oversized_image[..8].copy_from_slice(b"\x89PNG\r\n\x1a\n");
    let mut provider = LocalImageProvider::start_with_image_bytes(oversized_image.clone()).await;
    let fixture = MediaFixture::new_image(&provider).await;
    let run_id = "audit-image-provider-size-limit";
    let mut run = fixture.spawn(run_id);
    let result = finish_run(&mut run).await;
    provider.close().await;
    result.unwrap();

    let events = RunStore::new(&fixture.db)
        .events_after(run_id, None)
        .unwrap();
    let terminals: Vec<_> = events
        .iter()
        .filter(|event| event.event_type == "run_terminal")
        .collect();
    let live = fixture.live.lock().unwrap();
    let image = live
        .iter()
        .find(|event| event.node_id == "image" && event.status == NodeExecutionStatus::Completed)
        .expect("the production image node must complete before checking its artifact");
    let artifact_uri = image
        .artifacts
        .first()
        .expect("the image node must publish its stored artifact");
    let stored = fixture
        .artifacts
        .read_bytes(artifact_uri.trim_start_matches("artifact://"), None)
        .unwrap()
        .unwrap();
    let private_requests = provider.private_requests.lock().unwrap();
    eprintln!(
        "provider image size audit: private_requests={}, configured_input_limit={MAX_INPUT_IMAGE_BYTES}, response_bytes={}, artifact_bytes={}, terminal={}",
        private_requests.len(),
        oversized_image.len(),
        stored.len(),
        terminals[0].data["kind"]
    );
    assert_eq!(private_requests.len(), 1);
    assert_eq!(terminals.len(), 1);
    assert_eq!(terminals[0].data["kind"], "succeeded");
    assert!(
        stored.len() <= MAX_INPUT_IMAGE_BYTES,
        "provider image response larger than the established single-image budget must not be completely stored"
    );
}

#[tokio::test]
#[ignore = "audit: inline provider image payloads must enforce a bounded artifact size"]
async fn audit_provider_inline_image_rejects_oversized_artifact() {
    const MAX_IMAGE_BYTES: usize = 32 * 1024 * 1024;
    let response_bytes = MAX_IMAGE_BYTES + 1;
    let mut oversized_image = vec![b'x'; response_bytes];
    oversized_image[..8].copy_from_slice(b"\x89PNG\r\n\x1a\n");
    let mut provider = LocalInlineImageProvider::start(oversized_image).await;
    let fixture = MediaFixture::new_inline_image(&provider).await;
    let run_id = "audit-inline-image-size-limit";
    let mut run = fixture.spawn(run_id);
    let result = finish_run(&mut run).await;
    provider.close().await;

    let events = RunStore::new(&fixture.db)
        .events_after(run_id, None)
        .unwrap();
    let terminal = events
        .iter()
        .find(|event| event.event_type == "run_terminal")
        .and_then(|event| event.data["kind"].as_str())
        .unwrap_or("missing");
    let image = fixture
        .live
        .lock()
        .unwrap()
        .iter()
        .find(|event| event.node_id == "image" && event.status == NodeExecutionStatus::Completed)
        .cloned();
    let artifact_bytes = image
        .as_ref()
        .and_then(|event| event.artifacts.first())
        .and_then(|uri| {
            fixture
                .artifacts
                .read_bytes(uri.trim_start_matches("artifact://"), None)
                .ok()
        })
        .flatten()
        .map(|bytes| bytes.len());
    eprintln!(
        "inline image size: response_bytes={response_bytes}, artifact_bytes={artifact_bytes:?}, terminal={terminal}, result_ok={}",
        result.is_ok()
    );
    assert!(
        result.is_err() && artifact_bytes.is_none() && terminal != "succeeded",
        "inline provider image payloads must be rejected before artifact persistence"
    );
}

#[tokio::test]
async fn audit_video_node_events_survive_database_reopen() {
    let mut provider = LocalVideoProvider::start("Succeed").await;
    let fixture = MediaFixture::new(&provider).await;
    let run_id = "audit-video-node-replay";
    let mut run = fixture.spawn(run_id);
    provider.wait_for_poll().await;
    provider.release_poll.notify_one();
    let result = finish_run(&mut run).await;
    provider.close().await;
    result.unwrap();
    let live = fixture.live.lock().unwrap().clone();
    assert!(live.iter().any(|event| {
        event.node_id == "video"
            && event.status == NodeExecutionStatus::Completed
            && event.job_id.as_deref() == Some(TASK_ID)
            && event.provider_id.is_some()
            && event.model_id.as_deref() == Some("audit-video-model")
            && event.artifacts.len() == 1
    }));
    let uri = live
        .iter()
        .find(|event| event.node_id == "video" && event.status == NodeExecutionStatus::Completed)
        .unwrap()
        .artifacts[0]
        .clone();
    let durable_before = RunStore::new(&fixture.db)
        .events_after(run_id, None)
        .unwrap()
        .iter()
        .filter(|event| event.event_type == "workflow_node")
        .count();
    let MediaFixture {
        _root: root,
        db,
        chat,
        artifacts,
        providers,
        ..
    } = fixture;
    drop(chat);
    drop(artifacts);
    drop(providers);
    drop(db);
    // All original application/database owners have been dropped. This reader
    // does not borrow the live callback and cannot invent missing node events.
    let reopened = Arc::new(Database::open(root.path().join("media.db")).unwrap());
    let stored = RunStore::new(&reopened).events_after(run_id, None).unwrap();
    let replay: Vec<NodeExecutionEvent> = stored
        .iter()
        .filter(|event| event.event_type == "workflow_node")
        .map(|event| serde_json::from_value(event.data["event"].clone()).unwrap())
        .collect();
    let terminals: Vec<_> = stored
        .iter()
        .filter(|event| event.event_type == "run_terminal")
        .collect();
    assert_eq!(terminals.len(), 1);
    assert_eq!(terminals[0].data["kind"], "succeeded");
    let artifacts = CanvasArtifactService::new(root.path().join("artifacts"), reopened).unwrap();
    assert_eq!(
        artifacts
            .read_bytes(uri.trim_start_matches("artifact://"), None)
            .unwrap()
            .unwrap(),
        VIDEO_BYTES,
        "the artifact itself remains durable despite a missing node event"
    );
    eprintln!(
        "video node replay: live={}, durable_before={durable_before}, durable_after={}, terminal=succeeded, artifact_readable=true",
        live.len(),
        replay.len()
    );
    assert_eq!(
        replay, live,
        "canonical run replay must retain the live workflow node events, including task id, output and artifact provenance"
    );
}

#[test]
#[ignore = "audit: requires Node and desktop dependencies; actual Rust progress must reach TS resume config"]
fn audit_desktop_video_progress_keeps_task_id_for_resume() {
    use deepagent_runtime::events::ChannelSink;
    use deepagent_runtime::workflow::NodeEventPublisher;

    let (sink, mut receiver) = ChannelSink::new();
    let publisher = NodeEventPublisher::new(Arc::new(sink));
    publisher.emit_job_progress(
        "audit-revision",
        "video",
        TASK_ID,
        "audit-fingerprint",
        "queued",
    );
    let progress = receiver.try_recv().unwrap();
    publisher.emit(NodeExecutionEvent {
        status: NodeExecutionStatus::Completed,
        outputs: Some(json!({
            "videoTaskId": TASK_ID,
            "videoTaskFingerprint": "audit-fingerprint",
            "videoUrl": "artifact://audit-video",
            "operation": "video_generate",
            "providerId": "audit-provider",
            "modelId": "audit-model"
        })),
        ..NodeExecutionEvent::new("audit-revision".into(), "video".into())
    });
    let completed = receiver.try_recv().unwrap();
    let observed =
        desktop_resume_observation(&json!({"progress": progress, "completed": completed}));
    eprintln!(
        "desktop video progress: stored={:?}, resumed={:?}; completed: stored={:?}, resumed={:?}",
        observed["progress"]["storedTaskId"],
        observed["progress"]["resumedTaskId"],
        observed["completed"]["storedTaskId"],
        observed["completed"]["resumedTaskId"]
    );
    assert_eq!(observed["completed"]["storedTaskId"], TASK_ID);
    assert_eq!(observed["completed"]["resumedTaskId"], TASK_ID);
    assert_eq!(observed["progress"]["receivedTaskId"], TASK_ID);
    assert_eq!(
        observed["progress"]["resumedTaskId"], TASK_ID,
        "stopping before completion must retain the provider task id in the next real TS workflow request"
    );
    assert_eq!(
        observed["progress"]["storedTaskFingerprint"],
        "audit-fingerprint"
    );
    assert_eq!(
        observed["progress"]["resumedTaskFingerprint"],
        "audit-fingerprint"
    );
    assert_eq!(
        observed["completed"]["resumedTaskFingerprint"],
        "audit-fingerprint"
    );
}

fn desktop_resume_observation(fixture: &Value) -> Value {
    use std::process::{Command, Stdio};

    let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../apps/desktop/scripts/audit-video-task-progress.cjs");
    let mut child = Command::new("node")
        .arg(script)
        .arg("--events-from-stdin")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("local Node is required for this opt-in seam audit");
    serde_json::to_writer(child.stdin.take().unwrap(), fixture).unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[tokio::test]
#[ignore = "audit: editing a completed video's prompt must not silently resume its old provider task"]
async fn audit_edited_video_prompt_submits_a_new_task() {
    let mut provider = LocalVideoProvider::start_with_task_id("Succeed", TASK_ID).await;
    let mut fixture = MediaFixture::new(&provider).await;
    let mut first = fixture.spawn("audit-video-before-edit");
    provider.wait_for_poll().await;
    provider.release_poll.notify_one();
    finish_run(&mut first).await.unwrap();
    let completed = fixture
        .live
        .lock()
        .unwrap()
        .iter()
        .find(|event| event.node_id == "video" && event.status == NodeExecutionStatus::Completed)
        .unwrap()
        .clone();
    let original_model = fixture
        .workflow
        .definition
        .nodes
        .iter()
        .find(|node| node.id == "video")
        .unwrap()
        .config["videoModel"]
        .clone();
    let changed_prompt = "audit different video subject";
    let observed = desktop_resume_observation(&json!({
        "completed": RuntimeEvent::WorkflowNode { event: completed },
        "initialData": {"videoModel": original_model, "videoPrompt": "audit local video"},
        "edit": {"videoPrompt": changed_prompt}
    }));
    assert_eq!(observed["completed"]["resumedTaskId"], TASK_ID);
    assert_eq!(
        observed["completed"]["resumedTaskFingerprint"],
        observed["completed"]["storedTaskFingerprint"]
    );
    let next: WorkflowRequest =
        serde_json::from_value(observed["edited"]["workflow"].clone()).unwrap();
    assert_eq!(
        next.definition.nodes[0].config["videoPrompt"],
        changed_prompt
    );
    fixture.workflow = next;
    let mut second = fixture.spawn("audit-video-after-edit");
    provider.wait_for_poll().await;
    provider.release_poll.notify_one();
    let result = finish_run(&mut second).await;
    provider.close().await;
    result.unwrap();
    for run_id in ["audit-video-before-edit", "audit-video-after-edit"] {
        let events = RunStore::new(&fixture.db)
            .events_after(run_id, None)
            .unwrap();
        let terminal: Vec<_> = events
            .iter()
            .filter(|event| event.event_type == "run_terminal")
            .collect();
        assert_eq!(terminal.len(), 1);
        assert_eq!(terminal[0].data["kind"], "succeeded");
    }
    let requests = provider.requests.lock().unwrap();
    let submits: Vec<_> = requests
        .iter()
        .filter(|(line, _)| line.starts_with("POST /v1/video/submit "))
        .collect();
    let polls: Vec<_> = requests
        .iter()
        .filter(|(line, _)| line.starts_with("POST /v1/video/status "))
        .collect();
    assert_eq!(polls.len(), 2);
    let new_prompt_sent = submits
        .iter()
        .any(|(_, body)| body["prompt"] == changed_prompt);
    eprintln!("edited video: new_prompt_in_ts_request=true, carried_task_id={:?}, submits={}, polls={}, new_prompt_sent={new_prompt_sent}, both_terminals=succeeded",
        observed["edited"]["resumedTaskId"], submits.len(), polls.len());
    assert_eq!(
        submits.len(), 2,
        "executing the edited prompt must submit a new generation rather than silently polling the previous task"
    );
    assert!(new_prompt_sent, "the edited prompt must reach the provider");
    assert_ne!(
        polls[1].1["requestId"], polls[0].1["requestId"],
        "the edited run must poll its new task id"
    );
}

#[tokio::test]
#[ignore = "audit: a saved task id must not be sent to a replacement provider endpoint on resume"]
async fn audit_video_resume_submits_new_task_for_replaced_provider_identity() {
    let mut original = LocalVideoProvider::start("Succeed").await;
    let mut replacement =
        LocalVideoProvider::start_with_task_id("Succeed", "replacement-video-job").await;
    let mut fixture = MediaFixture::new(&original).await;
    let mut first = fixture.spawn("audit-video-original-provider");
    original.wait_for_poll().await;
    original.release_poll.notify_one();
    finish_run(&mut first).await.unwrap();
    let task_id = fixture
        .live
        .lock()
        .unwrap()
        .iter()
        .find(|event| event.node_id == "video" && event.status == NodeExecutionStatus::Completed)
        .unwrap()
        .job_id
        .clone()
        .unwrap();
    let video = fixture
        .workflow
        .definition
        .nodes
        .iter_mut()
        .find(|node| node.id == "video")
        .unwrap();
    video.config.insert("videoTaskId".into(), json!(task_id));
    let fingerprint = fixture
        .live
        .lock()
        .unwrap()
        .iter()
        .find(|event| event.node_id == "video" && event.status == NodeExecutionStatus::Completed)
        .unwrap()
        .resume_fingerprint
        .clone()
        .unwrap();
    video
        .config
        .insert("videoTaskFingerprint".into(), json!(fingerprint));
    let model_ref = video.config["videoModel"].as_str().unwrap().to_string();
    let (provider_id, model_id) = model_ref.split_once("::").unwrap();

    // Control: an explicit resume with the same provider does not resubmit.
    let mut same = fixture.spawn("audit-video-same-provider-resume");
    original.wait_for_poll().await;
    original.release_poll.notify_one();
    finish_run(&mut same).await.unwrap();
    assert_eq!(
        original
            .requests
            .lock()
            .unwrap()
            .iter()
            .filter(|(line, _)| line.starts_with("POST /v1/video/submit "))
            .count(),
        1
    );

    let previous = fixture.providers.provider(provider_id).unwrap();
    fixture
        .providers
        .save_provider(CanvasProviderInput {
            id: previous.id,
            name: previous.name,
            protocol: "openai".into(),
            base_url: replacement.base_url.clone(),
            enabled: true,
            models: previous.models,
            api_key: Some("sk-audit-replacement-placeholder".into()),
            ..Default::default()
        })
        .unwrap();
    let resolved = fixture
        .providers
        .resolve_model(provider_id, model_id)
        .unwrap();
    assert_eq!(resolved.provider.base_url, replacement.base_url);
    assert_eq!(
        resolved.api_key.as_deref(),
        Some("sk-audit-replacement-placeholder")
    );
    let original_requests_before = original.requests.lock().unwrap().len();
    let run_id = "audit-video-replaced-provider-resume";
    let mut resumed = fixture.spawn(run_id);
    // A changed provider identity must submit a fresh task before polling.
    replacement.wait_for_poll().await;
    let replacement_requests_before_release = replacement.requests.lock().unwrap().clone();
    replacement.release_poll.notify_one();
    let result = finish_run(&mut resumed).await;
    original.close().await;
    replacement.close().await;
    let replacement_requests = replacement.requests.lock().unwrap();
    let submits: Vec<_> = replacement_requests
        .iter()
        .filter(|(line, _)| line.starts_with("POST /v1/video/submit "))
        .collect();
    let polls: Vec<_> = replacement_requests
        .iter()
        .filter(|(line, _)| line.starts_with("POST /v1/video/status "))
        .collect();
    assert_eq!(
        original.requests.lock().unwrap().len(),
        original_requests_before
    );
    assert_eq!(submits.len(), 1, "replacement provider gets a fresh submit");
    assert_eq!(polls.len(), 1, "only the freshly submitted task is polled");
    assert_eq!(
        replacement_requests_before_release[0]
            .0
            .split_whitespace()
            .nth(1),
        Some("/v1/video/submit"),
        "submission must happen before any replacement status request"
    );
    assert_eq!(polls[0].1["requestId"], "replacement-video-job");
    assert_ne!(polls[0].1["requestId"], task_id);
    let terminal = RunStore::new(&fixture.db)
        .get(run_id)
        .unwrap()
        .unwrap()
        .terminal_kind;
    assert_eq!(
        RunStore::new(&fixture.db)
            .events_after(run_id, None)
            .unwrap()
            .iter()
            .filter(|event| event.event_type == "run_terminal")
            .count(),
        1
    );
    eprintln!("video provider replacement: unchanged_resume_submits=1, replacement_submits={}, replacement_polls={}, old_task_polled=false, api_ok={}, terminal={terminal:?}",
        submits.len(), polls.len(), result.is_ok());
    result.unwrap();
}
