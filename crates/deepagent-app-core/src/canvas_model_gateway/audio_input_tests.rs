//! H-249: real Kernel/provider consumers, with synthetic DNS and local TCP only.
use super::*;
use crate::canvas_artifact_service::CanvasArtifactService;
use crate::canvas_provider_service::CanvasProviderInput;
use crate::{AppService, ChatService, MemorySecretStore, SettingsService};
use deepagent_models::transport::MockTransport;
use deepagent_persistence::{run_control::RunControlStore, run_store::RunStore, Database};
use deepagent_runtime::workflow::{NodeExecutionEvent, WorkflowRequest};
use std::net::SocketAddr;
use std::path::Path;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::Notify;

const AUDIO: &[u8] = b"ID3\x03\x00synthetic-audio-h249";
const QUERY: &str = "synthetic-h249-signed-query";
const KEY: &str = "sk-synthetic-h249-provider";

struct WireServer {
    address: SocketAddr,
    requests: Arc<Mutex<Vec<Vec<u8>>>>,
    received: Arc<Notify>,
    task: tokio::task::JoinHandle<()>,
}

impl WireServer {
    async fn start(reply: Vec<u8>, stall: bool) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let captured = requests.clone();
        let received = Arc::new(Notify::new());
        let notify = received.clone();
        let task = tokio::spawn(async move {
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut bytes = Vec::new();
                loop {
                    let mut chunk = [0; 4096];
                    let n = socket.read(&mut chunk).await.unwrap();
                    if n == 0 {
                        break;
                    }
                    bytes.extend_from_slice(&chunk[..n]);
                    if let Some(end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
                        let head = String::from_utf8_lossy(&bytes[..end]);
                        let length = head
                            .lines()
                            .find_map(|line| {
                                line.to_ascii_lowercase()
                                    .strip_prefix("content-length: ")
                                    .and_then(|value| value.parse::<usize>().ok())
                            })
                            .unwrap_or(0);
                        if bytes.len() >= end + 4 + length {
                            break;
                        }
                    }
                }
                let provider = bytes.starts_with(b"POST /v1/audio/transcriptions ");
                captured.lock().unwrap().push(bytes);
                if provider {
                    let _ = socket
                        .write_all(&response(
                            "200 OK",
                            "",
                            br#"{"text":"synthetic transcription"}"#,
                        ))
                        .await;
                } else {
                    let _ = socket.write_all(&reply).await;
                }
                notify.notify_one();
                if stall {
                    std::future::pending::<()>().await;
                }
            }
        });
        Self {
            address,
            requests,
            received,
            task,
        }
    }

    fn source(&self) -> String {
        // Deliberately no audio filename suffix. Only the fetched bytes establish kind.
        format!(
            "http://public.fixture.invalid:{}/media?signature={QUERY}",
            self.address.port()
        )
    }

    fn count(&self) -> usize {
        self.requests.lock().unwrap().len()
    }
}

impl Drop for WireServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn response(status: &str, extra: &str, body: &[u8]) -> Vec<u8> {
    let mut reply = format!(
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n{extra}\r\n",
        body.len()
    )
    .into_bytes();
    reply.extend_from_slice(body);
    reply
}

fn transcription_workflow(model_ref: &str, reference: &str) -> WorkflowRequest {
    serde_json::from_value(json!({
            "workspace_id":"ws-a", "inputs":{"source":reference},
            "definition":{"version":1,"nodes":[
                {"id":"start","kind":"start","config":{"inputVariables":[{"name":"source","type":"string"}]}},
                {"id":"audio","kind":"audio","config":{
                    "operation":"speech_transcribe", "audioInputs":["{{#start.source#}}"], "audioModel":model_ref
                }}, {"id":"end","kind":"end"}],
                "edges":[{"id":"a","source":"start","target":"audio"},{"id":"b","source":"audio","target":"end"}]}
        })).unwrap()
}

#[test]
fn h249_fixture_and_image_variant_imports_obey_the_existing_contract() {
    use deepagent_runtime::workflow::CanvasImagePreparation::{Deferred, Ready};
    for reference in [
        "http://public.fixture.invalid/media",
        "artifact://synthetic",
        "data:audio/mpeg;base64,SUQz",
    ] {
        let request = transcription_workflow("synthetic::transcribe", reference);
        deepagent_runtime::workflow::compile(request.definition).unwrap();
        assert_eq!(request.inputs["source"], reference);
    }
    let ready = Ready(CanvasImageResponse {
        artifact_uri: "artifact://synthetic".into(),
        mime: "image/png".into(),
        provider_id: "synthetic".into(),
        model_id: "image".into(),
        operation: "generate".into(),
    });
    assert!(matches!(ready, Ready(_)));
    let deferred: CanvasImagePreparation = Deferred {
        request_id: "opaque".into(),
        origin: "https://public.fixture.invalid".into(),
    };
    assert!(matches!(deferred, Deferred { .. }));
}

struct Host {
    db: Arc<Database>,
    settings: Arc<SettingsService>,
    artifacts: Arc<CanvasArtifactService>,
    gateway: Arc<CanvasModelGateway>,
    chat: ChatService,
    model_ref: String,
}

impl Host {
    async fn new(root: &Path, wire: &WireServer, public: bool) -> Self {
        let db = Arc::new(Database::open(root.join("audio.db")).unwrap());
        let secrets = Arc::new(MemorySecretStore::new());
        let settings = Arc::new(SettingsService::new(
            db.clone(),
            Arc::new(MockTransport::with_get_json(
                r#"{"data":[{"id":"deepseek-v4-flash","object":"model","owned_by":"deepseek"}]}"#,
            )),
            secrets.clone(),
        ));
        crate::registry_fixture::configure(&settings, "sk-fixture-only")
            .await
            .unwrap();
        let providers = Arc::new(CanvasProviderService::new(db.clone(), secrets));
        let provider = providers
            .save_provider(CanvasProviderInput {
                name: "Synthetic transcription".into(),
                protocol: "openai".into(),
                base_url: format!("http://{}/v1", wire.address),
                enabled: true,
                models: vec![CanvasModelConfig {
                    id: "synthetic-transcribe".into(),
                    name: "Synthetic".into(),
                    description: String::new(),
                    enabled: true,
                    scenarios: vec![CanvasScenario::SpeechToText],
                    priority: 0,
                    max_reference_images: None,
                }],
                api_key: Some(KEY.into()),
                ..Default::default()
            })
            .unwrap();
        let artifacts =
            Arc::new(CanvasArtifactService::new(root.join("artifacts"), db.clone()).unwrap());
        let mut gateway = CanvasModelGateway::new(providers).with_artifacts(artifacts.clone());
        if public {
            gateway.public_http =
                Arc::new(crate::http_runtime::public_fixture_client(wire.address));
        }
        let gateway = Arc::new(gateway);
        let chat = ChatService::new(
            db.clone(),
            settings.clone(),
            Arc::new(MockTransport::default()),
            root,
        )
        .with_canvas_model(gateway.clone());
        Self {
            db,
            settings,
            artifacts,
            gateway,
            chat,
            model_ref: format!("{}::synthetic-transcribe", provider.id),
        }
    }

    async fn run(
        &self,
        id: &str,
        reference: &str,
        approve: bool,
    ) -> (CoreResult<String>, Vec<NodeExecutionEvent>, usize) {
        let session = self.chat.create_session(None).unwrap();
        let events = Arc::new(Mutex::new(Vec::new()));
        let captured = events.clone();
        let approvals = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let count = approvals.clone();
        let responder = self.chat.clone();
        let result = tokio::time::timeout(
            Duration::from_secs(5),
            self.chat.run_workflow(
                transcription_workflow(&self.model_ref, reference),
                id.into(),
                session,
                move |event| {
                    if let deepagent_runtime::RuntimeEvent::WorkflowNode { event } = event {
                        captured.lock().unwrap().push(event);
                    }
                },
                move |approval| {
                    assert_eq!(approval.tool, CANVAS_AUDIO_FETCH_TOOL);
                    let serialized = serde_json::to_string(&approval).unwrap();
                    assert!(!serialized.contains(QUERY));
                    assert!(!serialized.contains(KEY));
                    count.fetch_add(1, Ordering::SeqCst);
                    assert!(responder
                        .resolve_approval(&approval.call_id, approve, "synthetic-fixture")
                        .unwrap());
                },
            ),
        )
        .await
        .unwrap();
        let events = events.lock().unwrap().clone();
        (result, events, approvals.load(Ordering::SeqCst))
    }
}

fn handle(gateway: &CanvasModelGateway, reference: &str) -> (String, String) {
    match gateway.prepare_audio_input(reference).unwrap() {
        CanvasAudioInputPreparation::Deferred { request_id, origin } => (request_id, origin),
        other => panic!("expected deferred remote input, got {other:?}"),
    }
}

fn arguments(request_id: &str, origin: &str) -> Value {
    json!({"request_id":request_id,"origin":origin})
}

fn assert_no_publication(host: &Host, root: &Path) {
    assert!(host.artifacts.list(None, None).unwrap().is_empty());
    assert!(std::fs::read_dir(root.join("artifacts"))
        .unwrap()
        .all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .ends_with(".part")
        }));
}

async fn wait_for_staging(root: &Path) {
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if std::fs::read_dir(root.join("artifacts"))
                .unwrap()
                .any(|entry| {
                    entry
                        .unwrap()
                        .file_name()
                        .to_string_lossy()
                        .ends_with(".part")
                })
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .expect("download never reached artifact staging");
}

pub(super) async fn assert_unprepared_reference_denied() {
    let wire = WireServer::start(response("200 OK", "", AUDIO), false).await;
    let root = tempfile::tempdir().unwrap();
    let host = Host::new(root.path(), &wire, false).await;
    let result = host
        .gateway
        .run_audio(CanvasAudioRequest {
            operation: "speech_transcribe".into(),
            model_ref: host.model_ref.clone(),
            audio_reference: Some(format!("http://{}/private.wav", wire.address)),
            ..Default::default()
        })
        .await;
    assert!(result.unwrap_err().to_string().contains("NetworkDenied"));
    assert_eq!(
        wire.count(),
        0,
        "neither source GET nor provider POST may occur"
    );
    assert_no_publication(&host, root.path());
}

#[tokio::test]
async fn h249_public_audio_uses_kernel_approval_then_real_provider_and_replays_without_io() {
    let wire = WireServer::start(
        response("200 OK", "Content-Type: text/plain\r\n", AUDIO),
        false,
    )
    .await;
    let root = tempfile::tempdir().unwrap();
    let host = Host::new(root.path(), &wire, true).await;
    let (result, nodes, count) = host.run("audio-public", &wire.source(), true).await;
    assert!(result.is_ok(), "{result:?}");
    assert_eq!(count, 1);
    assert_eq!(wire.count(), 2);
    let requests = wire.requests.lock().unwrap();
    assert!(requests[0].starts_with(b"GET /media?signature="));
    assert!(!String::from_utf8_lossy(&requests[0])
        .to_ascii_lowercase()
        .contains("authorization:"));
    assert!(requests[1].starts_with(b"POST /v1/audio/transcriptions "));
    assert!(requests[1].windows(AUDIO.len()).any(|part| part == AUDIO));
    drop(requests);
    let records = host.artifacts.list(Some("ws-a"), None).unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].media_type.as_deref(), Some("audio/mpeg"));
    assert_eq!(
        host.artifacts
            .read_bytes(&records[0].id, Some("ws-a"))
            .unwrap()
            .unwrap(),
        AUDIO
    );
    assert!(host
        .artifacts
        .read_bytes(&records[0].id, Some("ws-b"))
        .unwrap()
        .is_none());
    assert!(nodes.iter().any(|event| event.node_id == "audio"
        && event
            .outputs
            .as_ref()
            .is_some_and(|output| output["text"] == "synthetic transcription")));
    let events = RunStore::new(&host.db)
        .events_after("audio-public", None)
        .unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_type == "run_terminal")
            .count(),
        1
    );
    let actions = RunControlStore::new(&host.db)
        .list_actions("audio-public")
        .unwrap();
    let approvals = RunControlStore::new(&host.db)
        .list_approvals("audio-public")
        .unwrap();
    assert_eq!(actions.len(), 1);
    assert_eq!(actions[0].state.label(), "completed");
    assert_eq!(approvals.len(), 1);
    // User inputs are an existing persistence surface, separate from this new tool receipt.
    let persisted = format!("{actions:?}{approvals:?}");
    assert!(!persisted.contains(QUERY));
    assert!(!persisted.contains(KEY));
    drop(host);
    let reopened = Arc::new(Database::open(root.path().join("audio.db")).unwrap());
    AppService::from_shared(reopened.clone())
        .recover_unfinished_runs()
        .unwrap();
    assert_eq!(
        RunStore::new(&reopened)
            .events_after("audio-public", None)
            .unwrap(),
        events
    );
    assert_eq!(
        RunControlStore::new(&reopened)
            .list_actions("audio-public")
            .unwrap(),
        actions
    );
    assert_eq!(
        RunControlStore::new(&reopened)
            .list_approvals("audio-public")
            .unwrap(),
        approvals
    );
    assert_eq!(wire.count(), 2);
}

#[tokio::test]
async fn h249_local_artifact_and_inline_audio_keep_the_original_provider_path() {
    let wire = WireServer::start(Vec::new(), false).await;
    let root = tempfile::tempdir().unwrap();
    let host = Host::new(root.path(), &wire, false).await;
    let artifact = host
        .artifacts
        .import_bytes(
            deepagent_persistence::artifact_store::ArtifactKind::Audio,
            Some("audio/mpeg"),
            AUDIO,
            Some("ws-a"),
        )
        .unwrap();
    for (id, reference) in [
        ("audio-artifact", artifact.uri),
        (
            "audio-inline",
            format!(
                "data:audio/mpeg;base64,{}",
                base64::engine::general_purpose::STANDARD.encode(AUDIO)
            ),
        ),
    ] {
        let (result, _, count) = host.run(id, &reference, true).await;
        assert!(result.is_ok(), "{id}: {result:?}");
        assert_eq!(count, 0);
        assert!(RunControlStore::new(&host.db)
            .list_actions(id)
            .unwrap()
            .is_empty());
    }
    let foreign = host
        .artifacts
        .import_bytes(
            deepagent_persistence::artifact_store::ArtifactKind::Audio,
            Some("audio/mpeg"),
            AUDIO,
            Some("ws-b"),
        )
        .unwrap();
    let (result, _, approvals) = host.run("audio-foreign", &foreign.uri, true).await;
    assert!(result.is_err());
    assert_eq!(approvals, 0);
    assert_eq!(
        wire.count(),
        2,
        "foreign artifacts must not reach the provider"
    );
}

#[tokio::test]
async fn h249_public_input_denial_has_one_approval_and_one_failed_terminal() {
    let wire = WireServer::start(response("200 OK", "", AUDIO), false).await;
    let root = tempfile::tempdir().unwrap();
    let host = Host::new(root.path(), &wire, true).await;
    let (result, nodes, count) = host.run("audio-denied", &wire.source(), false).await;
    assert!(result.is_err());
    assert_eq!(count, 1);
    assert_eq!(wire.count(), 0);
    assert_no_publication(&host, root.path());
    assert!(!nodes
        .iter()
        .any(|event| ["audio", "end"].contains(&event.node_id.as_str())
            && event.status == deepagent_runtime::workflow::NodeExecutionStatus::Completed));
    let ledger = RunStore::new(&host.db)
        .events_after("audio-denied", None)
        .unwrap();
    assert_eq!(
        ledger
            .iter()
            .filter(|event| event.event_type == "run_terminal")
            .count(),
        1
    );
    assert_eq!(
        RunControlStore::new(&host.db)
            .list_approvals("audio-denied")
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        RunStore::new(&host.db)
            .get("audio-denied")
            .unwrap()
            .unwrap()
            .terminal_kind
            .as_deref(),
        Some("failed")
    );
}

#[tokio::test]
async fn h249_full_access_never_turns_an_isolated_backend_into_host_network() {
    use crate::{
        SandboxBackend, SandboxBackendKind, SandboxBackendRegistry, SandboxCapabilities,
        SandboxExecutionRequest, SandboxExecutionResult,
    };
    struct FixtureBackend(SandboxBackendKind);
    #[async_trait]
    impl SandboxBackend for FixtureBackend {
        fn capabilities(&self) -> SandboxCapabilities {
            SandboxCapabilities {
                kind: self.0,
                available: true,
                supports_one_shot: true,
                supports_interactive_pty: false,
                supports_network_toggle: true,
                supports_readonly_mapping: true,
                message: "synthetic fixture without HTTP broker".into(),
            }
        }
        async fn execute(&self, _: SandboxExecutionRequest) -> CoreResult<SandboxExecutionResult> {
            panic!("audio workflow must not execute an OS command");
        }
    }
    for kind in [
        SandboxBackendKind::Direct,
        SandboxBackendKind::Sandboxie,
        SandboxBackendKind::WindowsSandbox,
    ] {
        let wire = WireServer::start(response("200 OK", "", AUDIO), false).await;
        let root = tempfile::tempdir().unwrap();
        let mut host = Host::new(root.path(), &wire, true).await;
        host.settings
            .set_active_permission_preset(crate::settings::PermissionPreset::FullAccess)
            .unwrap();
        host.chat =
            host.chat
                .clone()
                .with_sandbox_backend_registry(Arc::new(SandboxBackendRegistry::new(
                    kind,
                    vec![Arc::new(FixtureBackend(kind)) as Arc<dyn SandboxBackend>],
                )));
        let (result, _, approvals) = host.run("audio-backend", &wire.source(), true).await;
        let direct = kind == SandboxBackendKind::Direct;
        assert_eq!(result.is_ok(), direct, "{kind:?}: {result:?}");
        assert_eq!(approvals, 0);
        assert_eq!(wire.count(), if direct { 2 } else { 0 });
        if !direct {
            assert_no_publication(&host, root.path());
        }
    }
}

#[tokio::test]
async fn h249_kernel_cancellation_reaches_the_download_without_provider_submission() {
    let mut partial =
        b"HTTP/1.1 200 OK\r\nContent-Length: 999\r\nConnection: close\r\n\r\n".to_vec();
    partial.extend_from_slice(AUDIO);
    let wire = WireServer::start(partial, true).await;
    let root = tempfile::tempdir().unwrap();
    let host = Host::new(root.path(), &wire, true).await;
    let source = wire.source();
    let work = host.run("audio-cancel", &source, true);
    tokio::pin!(work);
    tokio::select! { _ = wire.received.notified() => {}, result = &mut work => panic!("unexpected early result: {result:?}") }
    tokio::select! { _ = wait_for_staging(root.path()) => {}, result = &mut work => panic!("expected staged download: {result:?}") }
    assert!(host.chat.request_cancel("audio-cancel").unwrap().accepted);
    let (_, nodes, approvals) = work.await;
    assert_eq!(approvals, 1);
    assert_eq!(wire.count(), 1);
    assert_eq!(
        RunStore::new(&host.db)
            .get("audio-cancel")
            .unwrap()
            .unwrap()
            .terminal_kind
            .as_deref(),
        Some("cancelled")
    );
    assert!(!nodes.iter().any(|event| event.node_id == "audio"
        && event.status == deepagent_runtime::workflow::NodeExecutionStatus::Completed));
    assert_no_publication(&host, root.path());
}

#[tokio::test]
async fn h249_handles_bind_kind_origin_and_run_and_consume_once() {
    let wire = WireServer::start(response("200 OK", "", AUDIO), false).await;
    let root = tempfile::tempdir().unwrap();
    let host = Host::new(root.path(), &wire, true).await;
    let owner = host.gateway.scoped_for_run(Some("ws-a")).unwrap();
    let foreign = host.gateway.scoped_for_run(Some("ws-b")).unwrap();
    let other_run = host.gateway.scoped_for_run(Some("ws-a")).unwrap();
    let (id, origin) = handle(&owner, &wire.source());
    let args = arguments(&id, &origin);
    let mut registry = deepagent_tools::ToolRegistry::new();
    registry
        .register(Arc::new(owner.audio_fetch_tool(true)))
        .unwrap();
    assert!(registry
        .invoke(
            CANVAS_AUDIO_FETCH_TOOL,
            args.clone(),
            &PermissionSet::read_only(),
            true
        )
        .await
        .is_err());
    assert!(registry
        .invoke(
            CANVAS_AUDIO_FETCH_TOOL,
            args.clone(),
            &PermissionSet::developer(),
            false
        )
        .await
        .is_err());
    assert!(!args.to_string().contains(QUERY));
    assert!(foreign
        .audio_fetch_tool(true)
        .validate_input(args.clone())
        .is_err());
    assert!(other_run
        .audio_fetch_tool(true)
        .validate_input(args.clone())
        .is_err());
    assert!(owner
        .image_fetch_tool()
        .validate_input(args.clone())
        .is_err());
    assert!(owner
        .audio_fetch_tool(true)
        .validate_input(arguments(&id, "https://other.invalid"))
        .is_err());
    assert!(owner
        .audio_fetch_tool(false)
        .invoke(args.clone())
        .await
        .unwrap_err()
        .to_string()
        .contains("NetworkDenied"));
    assert_eq!(wire.count(), 0);
    let tool = Arc::new(owner.audio_fetch_tool(true));
    assert!(tool.validate_input(args.clone()).is_ok());
    let (one, two) = tokio::join!(tool.invoke(args.clone()), tool.invoke(args.clone()));
    assert_eq!(usize::from(one.is_ok()) + usize::from(two.is_ok()), 1);
    assert!(tool.invoke(args).await.is_err());
    assert_eq!(wire.count(), 1);
    assert_eq!(host.artifacts.list(Some("ws-a"), None).unwrap().len(), 1);
    assert!(host.artifacts.list(Some("ws-b"), None).unwrap().is_empty());
}

#[tokio::test]
async fn h249_private_and_unsupported_references_never_reach_the_wire() {
    let wire = WireServer::start(Vec::new(), false).await;
    let root = tempfile::tempdir().unwrap();
    let host = Host::new(root.path(), &wire, false).await;
    for reference in [
        "http://127.0.0.1/a",
        "http://[::1]/a",
        "http://10.0.0.1/a",
        "http://169.254.169.254/a",
        "http://192.168.0.1/a",
        "http://[fe80::1]/a",
        "ftp://example.invalid/a",
        "https://user:pass@example.invalid/a",
    ] {
        let (id, origin) = handle(&host.gateway, reference);
        let result = host
            .gateway
            .audio_fetch_tool(true)
            .invoke(arguments(&id, &origin))
            .await;
        assert!(
            result.unwrap_err().to_string().contains("NetworkDenied"),
            "{reference}"
        );
    }
    assert_eq!(wire.count(), 0);
    assert_no_publication(&host, root.path());
}

#[tokio::test]
async fn h249_mixed_dns_answers_and_cross_origin_redirects_inherit_the_public_broker() {
    for location in [
        "http://127.0.0.1/private",
        "https://public.fixture.invalid/media",
        "http://public.fixture.invalid:1/media",
    ] {
        let wire = WireServer::start(
            response("302 Found", &format!("Location: {location}\r\n"), b""),
            false,
        )
        .await;
        let root = tempfile::tempdir().unwrap();
        let host = Host::new(root.path(), &wire, true).await;
        let (id, origin) = handle(&host.gateway, &wire.source());
        assert!(host
            .gateway
            .audio_fetch_tool(true)
            .invoke(arguments(&id, &origin))
            .await
            .unwrap_err()
            .to_string()
            .contains("NetworkDenied"));
        assert_eq!(wire.count(), 1);
        assert_no_publication(&host, root.path());
    }
    let wire = WireServer::start(Vec::new(), false).await;
    let root = tempfile::tempdir().unwrap();
    let host = Host::new(root.path(), &wire, false).await;
    let mut gateway = host.gateway.scoped_for_run(None).unwrap();
    Arc::get_mut(&mut gateway).unwrap().public_http =
        Arc::new(crate::http_runtime::public_fixture_client_with_answers(
            wire.address,
            &[&["8.8.8.8", "127.0.0.1"]],
        ));
    let (id, origin) = handle(&gateway, &wire.source());
    assert!(gateway
        .audio_fetch_tool(true)
        .invoke(arguments(&id, &origin))
        .await
        .unwrap_err()
        .to_string()
        .contains("NetworkDenied"));
    assert_eq!(wire.count(), 0);
    assert_no_publication(&host, root.path());
}

#[tokio::test]
async fn h249_body_cancellation_and_overall_deadline_discard_staging_without_replay() {
    for cancel in [false, true] {
        let mut partial =
            b"HTTP/1.1 200 OK\r\nContent-Length: 999\r\nConnection: close\r\n\r\n".to_vec();
        partial.extend_from_slice(AUDIO);
        let wire = WireServer::start(partial, true).await;
        let root = tempfile::tempdir().unwrap();
        let host = Host::new(root.path(), &wire, true).await;
        let (id, origin) = handle(&host.gateway, &wire.source());
        let args = arguments(&id, &origin);
        let context = ToolExecutionContext::default().with_timeout(Duration::from_millis(150));
        let token = context.cancellation_token();
        let tool = host.gateway.audio_fetch_tool(true);
        let work = tool.invoke_with_context(args.clone(), context);
        tokio::pin!(work);
        tokio::select! { _ = wire.received.notified() => {}, result = &mut work => panic!("unexpected early result: {result:?}") }
        tokio::select! { _ = wait_for_staging(root.path()) => {}, result = &mut work => panic!("expected staged download: {result:?}") }
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
        assert!(tool.invoke(args).await.is_err());
        assert_eq!(wire.count(), 1);
        assert_no_publication(&host, root.path());
    }
}

#[tokio::test]
async fn h249_budget_invalid_container_and_unknown_transport_outcomes_do_not_publish() {
    let mut chunked =
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n".to_vec();
    let mut bytes = vec![b'x'; MAX_REMOTE_AUDIO_BYTES + 1];
    bytes[..AUDIO.len()].copy_from_slice(AUDIO);
    chunked.extend_from_slice(format!("{:x}\r\n", bytes.len()).as_bytes());
    chunked.extend_from_slice(&bytes);
    chunked.extend_from_slice(b"\r\n0\r\n\r\n");
    for (reply, expected) in [
        (
            b"HTTP/1.1 200 OK\r\nContent-Length: 33554433\r\n\r\n".to_vec(),
            "budget",
        ),
        (chunked, "budget"),
        (
            response("200 OK", "Content-Type: audio/mpeg\r\n", b"not audio"),
            "signature",
        ),
        (
            b"HTTP/1.1 200 OK\r\nContent-Length: 999\r\n\r\nID3".to_vec(),
            "read media artifact",
        ),
        (response("503 Unavailable", "", b""), "HTTP 503"),
    ] {
        let wire = WireServer::start(reply, false).await;
        let root = tempfile::tempdir().unwrap();
        let host = Host::new(root.path(), &wire, true).await;
        let (id, origin) = handle(&host.gateway, &wire.source());
        let args = arguments(&id, &origin);
        let tool = host.gateway.audio_fetch_tool(true);
        let error = tokio::time::timeout(Duration::from_secs(5), tool.invoke(args.clone()))
            .await
            .unwrap()
            .unwrap_err();
        assert!(error.to_string().contains(expected), "{error}");
        assert!(!error.to_string().contains(QUERY));
        assert!(tool.invoke(args).await.is_err());
        assert_eq!(wire.count(), 1);
        assert_no_publication(&host, root.path());
    }
}

#[tokio::test]
async fn h249_pre_cancelled_and_missing_headers_share_the_existing_context_deadline() {
    for cancelled in [true, false] {
        let wire = WireServer::start(Vec::new(), true).await;
        let root = tempfile::tempdir().unwrap();
        let host = Host::new(root.path(), &wire, true).await;
        let (id, origin) = handle(&host.gateway, &wire.source());
        let args = arguments(&id, &origin);
        let context = ToolExecutionContext::default().with_timeout(Duration::from_millis(100));
        if cancelled {
            context.cancellation_token().cancel();
        }
        let tool = host.gateway.audio_fetch_tool(true);
        let error = tokio::time::timeout(
            Duration::from_secs(2),
            tool.invoke_with_context(args.clone(), context),
        )
        .await
        .unwrap()
        .unwrap_err();
        assert!(
            error.to_string().contains(if cancelled {
                "cancelled"
            } else {
                "deadline exceeded"
            }),
            "{error}"
        );
        assert!(tool.invoke(args).await.is_err());
        assert_eq!(wire.count(), usize::from(!cancelled));
        assert_no_publication(&host, root.path());
    }
}
