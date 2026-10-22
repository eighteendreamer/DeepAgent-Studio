use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use deepagent_app_core::canvas_artifact_service::CanvasArtifactService;
use deepagent_app_core::canvas_model_gateway::CanvasModelGateway;
use deepagent_app_core::canvas_provider_service::{
    CanvasModelConfig, CanvasProviderInput, CanvasProviderService, CanvasScenario,
};
use deepagent_app_core::{ChatService, MemorySecretStore, SettingsService};
use deepagent_models::transport::MockTransport;
use deepagent_persistence::{run_store::RunStore, Database};
use deepagent_runtime::workflow::{NodeExecutionEvent, NodeExecutionStatus, WorkflowRequest};
use deepagent_runtime::RuntimeEvent;
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Notify;
use tokio::task::JoinHandle;

const VIDEO_BYTES: &[u8] = b"\0\0\0\x14ftypmp42audit-video";
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

impl LocalVideoProvider {
    async fn start(status_after_release: &str) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let poll_received = Arc::new(Notify::new());
        let release_poll = Arc::new(Notify::new());
        let captured = requests.clone();
        let arrived = poll_received.clone();
        let release = release_poll.clone();
        let status = status_after_release.to_owned();
        let server = tokio::spawn(async move {
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                let (line, body) = read_request(&mut socket).await;
                captured.lock().unwrap().push((line.clone(), body));
                let (media_type, payload) =
                    if line.starts_with("POST /v1/video/submit ") {
                        (
                            "application/json",
                            json!({"requestId": TASK_ID}).to_string().into_bytes(),
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
                        ("video/mp4", VIDEO_BYTES.to_vec())
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
    workflow: WorkflowRequest,
    live: Arc<Mutex<Vec<NodeExecutionEvent>>>,
}

impl MediaFixture {
    async fn new(provider: &LocalVideoProvider) -> Self {
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
                name: "Audit local video".into(),
                protocol: "openai".into(),
                base_url: provider.base_url.clone(),
                enabled: true,
                models: vec![CanvasModelConfig {
                    id: "audit-video-model".into(),
                    name: "Audit video".into(),
                    description: String::new(),
                    enabled: true,
                    scenarios: vec![CanvasScenario::VideoGeneration],
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
            Arc::new(CanvasModelGateway::new(providers).with_artifacts(artifacts.clone()));
        let chat = ChatService::new(
            db.clone(),
            settings,
            Arc::new(MockTransport::default()),
            root.path(),
        )
        .with_canvas_model(gateway);
        let session = chat.create_session(None).unwrap();
        let workflow = serde_json::from_value(json!({
            "definition": {
                "version": 1,
                "nodes": [
                    {"id":"start","kind":"start"},
                    {"id":"video","kind":"video-gen","config":{
                        "prompt":"audit local video", "videoModel":format!("{}::audit-video-model",saved.id)
                    }},
                    {"id":"end","kind":"end"}
                ],
                "edges": [
                    {"id":"e1","source":"start","target":"video"},
                    {"id":"e2","source":"video","target":"end"}
                ]
            }
        })).unwrap();
        Self {
            _root: root,
            db,
            chat,
            session,
            artifacts,
            workflow,
            live: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn spawn(&self, run_id: &str) -> JoinHandle<deepagent_core::error::Result<String>> {
        let chat = self.chat.clone();
        let workflow = self.workflow.clone();
        let session = self.session.clone();
        let run_id = run_id.to_string();
        let live = self.live.clone();
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
                |_| {},
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
    let mut provider = LocalVideoProvider::start("Succeed").await;
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
            .read_bytes(uri.trim_start_matches("artifact://"))
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
#[ignore = "audit: cancelling a real video workflow must interrupt a pending status HTTP request"]
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
    let exit_ms = started.elapsed().as_millis();
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
    eprintln!("video cancellation: accepted={accepted}, signal_ms={signal_ms}, stopped_within_300ms={stopped_before_response}, unfinished_before_response={unfinished_before_response}, exit_after_release_ms={exit_ms}, terminal=cancelled");
    assert!(
        stopped_before_response,
        "accepted cancellation must stop the workflow without waiting for video/status to respond"
    );
}

#[tokio::test]
#[ignore = "audit: workflow node status, output and media provenance must survive canonical run replay"]
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
        ..
    } = fixture;
    drop(chat);
    drop(artifacts);
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
            .read_bytes(uri.trim_start_matches("artifact://"))
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
