//! Real HTTP, artifact staging and gateway tools; no provider credentials/network.
use super::*;
use crate::canvas_artifact_service::CanvasArtifactService;
use crate::MemorySecretStore;
use deepagent_persistence::{artifact_store::ArtifactKind, Database};
use std::net::SocketAddr;
use std::path::Path;
use std::sync::atomic::AtomicUsize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::Notify;

#[derive(Clone, Copy)]
enum Framing {
    Length(u64),
    Chunked,
    Close,
}

struct Wire {
    address: SocketAddr,
    started: Arc<Notify>,
    closed: Arc<Notify>,
    requests: Arc<AtomicUsize>,
    task: tokio::task::JoinHandle<()>,
}

impl Wire {
    async fn start(body: Vec<u8>, framing: Framing, stall: bool) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let started = Arc::new(Notify::new());
        let closed = Arc::new(Notify::new());
        let requests = Arc::new(AtomicUsize::new(0));
        let (ready, disconnected, count) = (started.clone(), closed.clone(), requests.clone());
        let task = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut buffer = [0; 1024];
            while !request.windows(4).any(|s| s == b"\r\n\r\n") {
                let n = socket.read(&mut buffer).await.unwrap();
                assert_ne!(n, 0);
                request.extend_from_slice(&buffer[..n]);
            }
            count.fetch_add(1, Ordering::SeqCst);
            assert!(!String::from_utf8_lossy(&request)
                .to_lowercase()
                .contains("authorization:"));
            let framing_header = match framing {
                Framing::Length(n) => format!("Content-Length: {n}\r\n"),
                Framing::Chunked => "Transfer-Encoding: chunked\r\n".into(),
                Framing::Close => String::new(),
            };
            let header = format!("HTTP/1.1 200 OK\r\nContent-Type: video/mp4\r\n{framing_header}Connection: close\r\n\r\n");
            socket.write_all(header.as_bytes()).await.unwrap();
            let chunk_size = if body.len() > 1024 { 16 * 1024 } else { 17 };
            for chunk in body.chunks(chunk_size) {
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
                if matches!(framing, Framing::Chunked) && socket.write_all(b"\r\n").await.is_err() {
                    return;
                }
            }
            ready.notify_one();
            if stall {
                let result = socket.read(&mut buffer).await;
                assert!(matches!(result, Ok(0) | Err(_)));
                disconnected.notify_one();
            } else if matches!(framing, Framing::Chunked) {
                let _ = socket.write_all(b"0\r\n\r\n").await;
            }
        });
        Self {
            address,
            started,
            closed,
            requests,
            task,
        }
    }

    fn url(&self, public: bool) -> String {
        let host = if public {
            "public.fixture.invalid".into()
        } else {
            self.address.ip().to_string()
        };
        format!(
            "http://{host}:{}/clip.mp4?signature=synthetic-h302",
            self.address.port()
        )
    }
}

impl Drop for Wire {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn video_bytes(size: usize) -> Vec<u8> {
    let mut body = vec![b'V'; size];
    body[..12].copy_from_slice(b"\0\0\0\x14ftypmp42");
    body
}

fn host(
    root: &Path,
    wire: &Wire,
    limit: u64,
) -> (Arc<CanvasModelGateway>, Arc<CanvasArtifactService>) {
    let db = Arc::new(Database::open(root.join("video.db")).unwrap());
    let artifacts =
        Arc::new(CanvasArtifactService::new(root.join("artifacts"), db.clone()).unwrap());
    let providers = Arc::new(CanvasProviderService::new(
        db,
        Arc::new(MemorySecretStore::new()),
    ));
    let base = Arc::new(
        CanvasModelGateway::new(providers)
            .with_artifacts(artifacts.clone())
            .with_video_download_limit(limit)
            .unwrap()
            .with_public_fixture_route(wire.address),
    );
    let gateway = base.scoped_for_run(Some("h302-workspace")).unwrap();
    assert_eq!(gateway.video_download_limit, limit);
    (gateway, artifacts)
}

fn arguments(gateway: &CanvasModelGateway, wire: &Wire) -> Value {
    let (id, origin) = gateway
        .stage_artifact_url(
            wire.url(true),
            PendingArtifactOutput::Video {
                provider_id: "synthetic-provider".into(),
                model_id: "synthetic-video".into(),
                task_id: "synthetic-task".into(),
                resume_fingerprint: "synthetic-fingerprint".into(),
            },
        )
        .unwrap();
    json!({"request_id": id, "origin": origin})
}

fn assert_empty(artifacts: &CanvasArtifactService, root: &Path) {
    assert!(artifacts
        .list(Some("h302-workspace"), None)
        .unwrap()
        .is_empty());
    assert_eq!(
        std::fs::read_dir(root.join("artifacts")).unwrap().count(),
        0
    );
}

#[tokio::test]
async fn h302_broker_rejects_declared_chunked_and_unknown_length_without_publication() {
    for (body, framing, stall) in [
        (Vec::new(), Framing::Length(65), true),
        (video_bytes(65), Framing::Chunked, false),
        (video_bytes(65), Framing::Close, false),
    ] {
        let wire = Wire::start(body, framing, stall).await;
        let root = tempfile::tempdir().unwrap();
        let (gateway, artifacts) = host(root.path(), &wire, 64);
        let tool = gateway.video_fetch_tool(true);
        let args = arguments(&gateway, &wire);
        let error = tokio::time::timeout(Duration::from_secs(3), tool.invoke(args.clone()))
            .await
            .unwrap()
            .unwrap_err();
        assert!(
            error.to_string().contains("VideoArtifactTooLarge"),
            "{error}"
        );
        assert!(!error.to_string().contains("synthetic-h302"));
        assert!(!error.to_string().contains("budget"));
        if stall {
            tokio::time::timeout(Duration::from_secs(2), wire.closed.notified())
                .await
                .unwrap();
        }
        assert!(
            tool.invoke(args).await.is_err(),
            "consumed handle cannot replay"
        );
        assert_eq!(wire.requests.load(Ordering::SeqCst), 1);
        assert_empty(&artifacts, root.path());
    }
}

#[tokio::test]
async fn h302_exact_boundary_and_configured_larger_limit_preserve_bytes_and_scope() {
    for (size, limit, framing) in [
        (64, 64, Framing::Length(64)),
        (64, 64, Framing::Chunked),
        (64, 64, Framing::Close),
        (65, 65, Framing::Chunked),
        (
            DEFAULT_VIDEO_DOWNLOAD_LIMIT as usize + 1,
            DEFAULT_VIDEO_DOWNLOAD_LIMIT + 1,
            Framing::Chunked,
        ),
    ] {
        let body = video_bytes(size);
        let wire = Wire::start(body.clone(), framing, false).await;
        let root = tempfile::tempdir().unwrap();
        let (gateway, artifacts) = host(root.path(), &wire, limit);
        let response = gateway
            .fetch_artifact_handle(
                arguments(&gateway, &wire)["request_id"].as_str().unwrap(),
                &reqwest::Url::parse(&wire.url(true))
                    .unwrap()
                    .origin()
                    .ascii_serialization(),
                ArtifactFetchKind::Video,
                ToolExecutionContext::default(),
            )
            .await
            .unwrap();
        let id = response["artifactUri"]
            .as_str()
            .unwrap()
            .trim_start_matches("artifact://");
        let received = artifacts
            .read_bytes(id, Some("h302-workspace"))
            .unwrap()
            .unwrap();
        assert_eq!(received.len(), body.len());
        assert!(
            received == body,
            "download must preserve the original bytes"
        );
        assert!(artifacts.record(id, None).unwrap().is_none());
        let record = artifacts
            .record(id, Some("h302-workspace"))
            .unwrap()
            .unwrap();
        assert_eq!(record.kind, ArtifactKind::Video);
        assert_eq!(record.byte_size, size as i64);
        assert_eq!(record.media_type.as_deref(), Some("video/mp4"));
    }
}

#[tokio::test]
async fn h302_partial_download_cancellation_and_deadline_discard_staging() {
    for cancel in [true, false] {
        let wire = Wire::start(video_bytes(12), Framing::Length(64), true).await;
        let root = tempfile::tempdir().unwrap();
        let (gateway, artifacts) = host(root.path(), &wire, 64);
        let args = arguments(&gateway, &wire);
        let tool = gateway.video_fetch_tool(true);
        let context = ToolExecutionContext::default().with_timeout(Duration::from_millis(300));
        let token = context.cancellation_token();
        let work = tool.invoke_with_context(args.clone(), context);
        tokio::pin!(work);
        tokio::select! { _ = wire.started.notified() => {}, result = &mut work => panic!("early response: {result:?}") }
        let staged = async {
            loop {
                if std::fs::read_dir(root.path().join("artifacts"))
                    .unwrap()
                    .count()
                    == 1
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        };
        tokio::select! { _ = staged => {}, result = &mut work => panic!("expected staging: {result:?}") }
        if cancel {
            token.cancel();
        }
        let error = tokio::time::timeout(Duration::from_secs(2), work)
            .await
            .unwrap()
            .unwrap_err();
        assert!(
            error.to_string().contains(if cancel {
                "cancelled"
            } else {
                "deadline exceeded"
            }),
            "{error}"
        );
        tokio::time::timeout(Duration::from_secs(2), wire.closed.notified())
            .await
            .unwrap();
        assert!(tool.invoke(args).await.is_err());
        assert_empty(&artifacts, root.path());
    }
}

#[tokio::test]
async fn h302_compatibility_download_uses_the_same_boundary() {
    for (size, framing, ok) in [
        (64, Framing::Chunked, true),
        (65, Framing::Chunked, false),
        (0, Framing::Length(65), false),
    ] {
        let body = if size == 0 {
            Vec::new()
        } else {
            video_bytes(size)
        };
        let wire = Wire::start(body, framing, size == 0).await;
        let root = tempfile::tempdir().unwrap();
        let (_, artifacts) = host(root.path(), &wire, 64);
        let result = fetch_video_artifact(
            &wire.url(false),
            &artifacts,
            None,
            "synthetic-task",
            Some("h302-workspace"),
            64,
        )
        .await;
        if ok {
            assert_eq!(result.unwrap().byte_size, 64);
        } else {
            assert_eq!(
                result.unwrap_err().code,
                CanvasErrorCode::ResponseBodyTooLarge
            );
            assert_empty(&artifacts, root.path());
        }
    }
}

#[tokio::test]
async fn h302_truncated_success_response_is_not_published_by_either_entry() {
    for compatibility in [false, true] {
        let wire = Wire::start(video_bytes(12), Framing::Length(64), false).await;
        let root = tempfile::tempdir().unwrap();
        let (gateway, artifacts) = host(root.path(), &wire, 64);
        if compatibility {
            assert!(fetch_video_artifact(
                &wire.url(false),
                &artifacts,
                None,
                "synthetic-task",
                Some("h302-workspace"),
                64,
            )
            .await
            .is_err());
        } else {
            assert!(gateway
                .video_fetch_tool(true)
                .invoke(arguments(&gateway, &wire))
                .await
                .is_err());
        }
        assert_eq!(wire.requests.load(Ordering::SeqCst), 1);
        assert_empty(&artifacts, root.path());
    }
}

#[test]
fn h302_configuration_rejects_unbounded_or_unindexable_values() {
    for limit in [0, u64::MAX] {
        let providers = Arc::new(CanvasProviderService::new(
            Arc::new(Database::open_in_memory().unwrap()),
            Arc::new(MemorySecretStore::new()),
        ));
        assert!(CanvasModelGateway::new(providers)
            .with_video_download_limit(limit)
            .is_err());
    }
}
