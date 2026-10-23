use std::sync::atomic::{AtomicUsize, Ordering};
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
        Self::start_with_task_id(status_after_release, TASK_ID).await
    }

    async fn start_with_task_id(status_after_release: &str, task_id_prefix: &str) -> Self {
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
    providers: Arc<CanvasProviderService>,
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
            Arc::new(CanvasModelGateway::new(providers.clone()).with_artifacts(artifacts.clone()));
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
            providers,
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
