use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use deepagent_app_core::{
    AppService, ArchiveService, ChatService, HarnessRunOverrides, SandboxCapabilities,
};
use deepagent_core::clock::SystemClock;
use deepagent_harness_protocol::{
    project_runtime_event, EventContext, HarnessEvent, HarnessRequest, InitializeRequest,
    RpcNotification, RpcRequest, RpcResponse, ThreadArchiveRequest, ThreadForkRequest,
    ThreadListRequest, ThreadReadRequest, ThreadResumeRequest, ThreadStartRequest, ToolListRequest,
    TurnInterruptRequest, TurnStartRequest, TurnSteerRequest, CONTROL_PROJECTION_VERSION,
    PROTOCOL_VERSION,
};
use deepagent_persistence::artifact_store::ToolArtifactStore;
use deepagent_persistence::event_store::EventStore;
use deepagent_persistence::run_control::RunControlStore;
use deepagent_persistence::run_store::RunStore;
use deepagent_persistence::Database;
use deepagent_session::Session;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
use tokio::sync::mpsc;

use crate::args::RunOptions;

const NOTIFICATION_METHOD: &str = "harness/event";
const ERR_NOT_INITIALIZED: i32 = -32001;
const ERR_ALREADY_INITIALIZED: i32 = -32002;
const ERR_INVALID_THREAD: i32 = -32003;
const ERR_INVALID_TURN: i32 = -32004;
const ERR_INVALID_PARAMS: i32 = -32602;
const ERR_METHOD_NOT_FOUND: i32 = -32601;
const ERR_INTERNAL: i32 = -32603;

type EventEmitter = Arc<dyn Fn(HarnessEvent) + Send + Sync>;

struct QueuedEvent {
    sequence: u64,
    event: HarnessEvent,
}

/// One bounded writer queue per stdio connection. Event sequence assignment
/// occurs before enqueue; a full queue is an explicit backpressure/drop
/// failure rather than spawning unordered writers for every event.
struct EventOutbox {
    tx: mpsc::Sender<QueuedEvent>,
    next_sequence: AtomicU64,
}

impl EventOutbox {
    fn new(stdout: Arc<tokio::sync::Mutex<tokio::io::Stdout>>) -> Arc<Self> {
        let (tx, mut rx) = mpsc::channel::<QueuedEvent>(256);
        tokio::spawn(async move {
            while let Some(queued) = rx.recv().await {
                let notification = RpcNotification {
                    jsonrpc: "2.0".into(),
                    method: NOTIFICATION_METHOD.into(),
                    event_sequence: Some(queued.sequence),
                    params: serde_json::to_value(queued.event).unwrap_or_else(|_| {
                        serde_json::json!({
                            "type": "error",
                            "code": "serialize",
                            "message": "failed to serialize harness event"
                        })
                    }),
                };
                let Ok(line) = serde_json::to_string(&notification) else {
                    eprintln!("serialize app-server notification failed");
                    continue;
                };
                let mut output = stdout.lock().await;
                if let Err(error) = output.write_all(format!("{line}\n").as_bytes()).await {
                    eprintln!("write app-server notification: {error}");
                    continue;
                }
                let _ = output.flush().await;
            }
        });
        Arc::new(Self {
            tx,
            next_sequence: AtomicU64::new(1),
        })
    }

    fn emit(&self, event: HarnessEvent) {
        let sequence = self.next_sequence.fetch_add(1, Ordering::Relaxed);
        if let Err(error) = self.tx.try_send(QueuedEvent { sequence, event }) {
            eprintln!("app-server event outbox full; dropped sequence {sequence}: {error}");
        }
    }
}

pub async fn run(transport: String) -> Result<(), String> {
    if transport != "stdio" {
        return Err(format!("unsupported server transport: {transport}"));
    }
    let workspace =
        std::env::current_dir().map_err(|error| format!("read current directory: {error}"))?;
    let options = RunOptions {
        prompt: String::new(),
        continue_thread: None,
        json: true,
        provider: None,
        model: None,
        sandbox_backend: None,
        permission_profile: None,
        reasoning_effort: None,
    };
    let (chat, capabilities) = super::build_chat_service(&workspace, &options)?;
    let capabilities = capabilities.unwrap_or_else(|| SandboxCapabilities {
        kind: deepagent_app_core::SandboxBackendKind::Direct,
        available: true,
        supports_one_shot: true,
        supports_interactive_pty: true,
        supports_network_toggle: false,
        supports_readonly_mapping: false,
        message: "direct host execution".into(),
    });
    if !capabilities.available {
        return Err(format!("sandbox unavailable: {}", capabilities.message));
    }
    run_stdio(chat, workspace, capabilities).await
}

#[derive(Clone)]
struct ThreadState {
    chat: ChatService,
    workspace: PathBuf,
    provider: Option<String>,
    model: Option<String>,
    permission_profile: Option<String>,
    sandbox_backend: Option<String>,
}

struct TurnLaunch {
    chat: ChatService,
    thread_id: String,
    turn_id: String,
    input: String,
    overrides: HarnessRunOverrides,
}

#[derive(Clone)]
struct TurnState {
    thread_id: String,
    chat: ChatService,
    active: bool,
}

/// In-process state for one app-server process. It owns no agent loop or
/// persistence of its own; both remain in AppCore and the existing stores.
pub struct ServerState {
    base_chat: ChatService,
    database: Arc<Database>,
    workspace: PathBuf,
    capabilities: SandboxCapabilities,
    initialized: bool,
    last_event_ack: u64,
    threads: HashMap<String, ThreadState>,
    turns: HashMap<String, TurnState>,
}

impl ServerState {
    pub fn new(
        chat: ChatService,
        workspace: impl Into<PathBuf>,
        capabilities: SandboxCapabilities,
    ) -> Self {
        let workspace = workspace.into();
        Self {
            database: chat.database(),
            base_chat: chat,
            workspace,
            capabilities,
            initialized: false,
            last_event_ack: 0,
            threads: HashMap::new(),
            turns: HashMap::new(),
        }
    }

    #[cfg(test)]
    fn new_for_test() -> Self {
        let database = Arc::new(Database::open_in_memory().expect("test database"));
        let transport: Arc<dyn deepagent_models::HttpTransport> =
            Arc::new(deepagent_models::ReqwestTransport::new());
        let settings = Arc::new(deepagent_app_core::SettingsService::new(
            database.clone(),
            transport.clone(),
            Arc::new(deepagent_app_core::EnvSecretStore::new()),
        ));
        let workspace = std::env::current_dir().expect("test cwd");
        let chat = ChatService::new(database, settings, transport, workspace.clone());
        Self::new(
            chat,
            workspace,
            SandboxCapabilities {
                kind: deepagent_app_core::SandboxBackendKind::Direct,
                available: true,
                supports_one_shot: true,
                supports_interactive_pty: true,
                supports_network_toggle: false,
                supports_readonly_mapping: false,
                message: "test direct backend".into(),
            },
        )
    }

    #[cfg(test)]
    fn workspace(&self) -> &std::path::Path {
        &self.workspace
    }

    #[cfg(test)]
    fn handle_request_for_test(&mut self, request: RpcRequest) -> RpcResponse {
        self.dispatch(request, None).0
    }

    fn dispatch(
        &mut self,
        request: RpcRequest,
        emitter: Option<&EventEmitter>,
    ) -> (RpcResponse, Option<TurnLaunch>) {
        if request.jsonrpc != "2.0" {
            return (
                RpcResponse::error(request.id, -32600, "jsonrpc must be '2.0'"),
                None,
            );
        }
        if request.method != "initialize" && !self.initialized {
            return (
                RpcResponse::error(
                    request.id,
                    ERR_NOT_INITIALIZED,
                    "initialize must be called before other requests",
                ),
                None,
            );
        }

        let method = request.method.clone();
        let parsed = serde_json::from_value::<HarnessRequest>(serde_json::json!({
            "method": method,
            "params": request.params
        }));
        let parsed = match parsed {
            Ok(parsed) => parsed,
            Err(error) => {
                let code = if request.method.contains('/') {
                    ERR_INVALID_PARAMS
                } else {
                    ERR_METHOD_NOT_FOUND
                };
                return (
                    RpcResponse::error(request.id, code, format!("invalid request: {error}")),
                    None,
                );
            }
        };

        match parsed {
            HarnessRequest::Initialize(params) => self.initialize(request.id, params),
            HarnessRequest::ThreadStart(params) => self.thread_start(request.id, params, emitter),
            HarnessRequest::ThreadResume(params) => self.thread_resume(request.id, params),
            HarnessRequest::ThreadList(params) => self.thread_list(request.id, params),
            HarnessRequest::ThreadRead(params) => self.thread_read(request.id, params),
            HarnessRequest::ThreadFork(params) => self.thread_fork(request.id, params),
            HarnessRequest::ThreadArchive(params) => self.thread_archive(request.id, params),
            HarnessRequest::TurnStart(params) => self.turn_start(request.id, params),
            HarnessRequest::TurnInterrupt(params) => self.turn_interrupt(request.id, params),
            HarnessRequest::TurnSteer(params) => self.turn_steer(request.id, params),
            HarnessRequest::ApprovalRespond(params) => self.approval_respond(request.id, params),
            HarnessRequest::EventAck(params) => self.event_ack(request.id, params.event_sequence),
            HarnessRequest::ToolList(params) => self.tool_list(request.id, params),
            HarnessRequest::ConfigRead(_) => (
                RpcResponse::success(
                    request.id,
                    serde_json::json!({
                        "protocolVersion": PROTOCOL_VERSION,
                        "transport": "stdio",
                        "provider": "deepseek-official"
                    }),
                ),
                None,
            ),
            HarnessRequest::SandboxStatus(_) => (
                RpcResponse::success(
                    request.id,
                    serde_json::to_value(&self.capabilities)
                        .unwrap_or_else(|_| serde_json::json!({ "available": false })),
                ),
                None,
            ),
        }
    }

    fn initialize(
        &mut self,
        id: serde_json::Value,
        params: InitializeRequest,
    ) -> (RpcResponse, Option<TurnLaunch>) {
        if self.initialized {
            return (
                RpcResponse::error(
                    id,
                    ERR_ALREADY_INITIALIZED,
                    "initialize may only be called once",
                ),
                None,
            );
        }
        if params.protocol_version != PROTOCOL_VERSION {
            return (
                RpcResponse::error(
                    id,
                    ERR_INVALID_PARAMS,
                    format!(
                        "unsupported protocol version {}; expected {}",
                        params.protocol_version, PROTOCOL_VERSION
                    ),
                ),
                None,
            );
        }
        self.initialized = true;
        (
            RpcResponse::success(
                id,
                serde_json::json!({
                    "protocolVersion": PROTOCOL_VERSION,
                    "serverName": "deepagent",
                    "serverVersion": env!("CARGO_PKG_VERSION"),
                    "capabilities": {
                        "threadLifecycle": true,
                        "streaming": true,
                        "interrupt": true,
                        "steer": true,
                        "approval": true,
                        "reconnect": true
                    }
                }),
            ),
            None,
        )
    }

    fn event_ack(
        &mut self,
        id: serde_json::Value,
        event_sequence: u64,
    ) -> (RpcResponse, Option<TurnLaunch>) {
        if event_sequence < self.last_event_ack {
            return (
                RpcResponse::error(
                    id,
                    ERR_INVALID_PARAMS,
                    "event acknowledgment cannot move backwards",
                ),
                None,
            );
        }
        self.last_event_ack = event_sequence;
        (
            RpcResponse::success(
                id,
                serde_json::json!({
                    "acknowledgedEventSequence": event_sequence,
                    "status": "accepted"
                }),
            ),
            None,
        )
    }

    fn thread_start(
        &mut self,
        id: serde_json::Value,
        params: ThreadStartRequest,
        emitter: Option<&EventEmitter>,
    ) -> (RpcResponse, Option<TurnLaunch>) {
        let workspace = match self.resolve_workspace(params.cwd.as_deref()) {
            Ok(path) => path,
            Err(error) => return (RpcResponse::error(id, ERR_INVALID_PARAMS, error), None),
        };
        let session = match Session::create_in_project(
            &self.database,
            &SystemClock,
            None,
            deepagent_core::SessionMode::Normal,
            Some(&workspace.to_string_lossy()),
        ) {
            Ok(session) => session,
            Err(error) => {
                return (
                    RpcResponse::error(id, ERR_INTERNAL, error.to_string()),
                    None,
                )
            }
        };
        let thread_id = session.id().to_string();
        let chat = self.base_chat.for_workspace(workspace.clone());
        self.threads.insert(
            thread_id.clone(),
            ThreadState {
                chat,
                workspace: workspace.clone(),
                provider: params.provider,
                model: params.model,
                permission_profile: params.permission_profile,
                sandbox_backend: params.sandbox_backend,
            },
        );
        if let Some(emitter) = emitter {
            emitter(HarnessEvent::ThreadStarted {
                thread_id: thread_id.clone(),
                title: None,
                protocol_version: PROTOCOL_VERSION,
            });
        }
        (
            RpcResponse::success(
                id,
                serde_json::json!({
                    "threadId": thread_id,
                    "status": "ready",
                    "cwd": workspace,
                }),
            ),
            None,
        )
    }

    fn thread_resume(
        &mut self,
        id: serde_json::Value,
        params: ThreadResumeRequest,
    ) -> (RpcResponse, Option<TurnLaunch>) {
        let record = match EventStore::new(&self.database)
            .get_session(params.thread_id.parse().unwrap_or_default())
        {
            Ok(Some(record)) => record,
            Ok(None) => {
                return (
                    RpcResponse::error(id, ERR_INVALID_THREAD, "thread not found"),
                    None,
                )
            }
            Err(error) => {
                return (
                    RpcResponse::error(id, ERR_INVALID_THREAD, error.to_string()),
                    None,
                )
            }
        };
        let workspace = record
            .project
            .map(PathBuf::from)
            .unwrap_or_else(|| self.workspace.clone());
        self.threads.insert(
            params.thread_id.clone(),
            ThreadState {
                chat: self.base_chat.for_workspace(workspace.clone()),
                workspace: workspace.clone(),
                provider: None,
                model: None,
                permission_profile: None,
                sandbox_backend: None,
            },
        );
        (
            RpcResponse::success(
                id,
                serde_json::json!({
                    "threadId": params.thread_id,
                    "status": "ready",
                    "cwd": workspace,
                }),
            ),
            None,
        )
    }

    fn thread_list(
        &self,
        id: serde_json::Value,
        _params: ThreadListRequest,
    ) -> (RpcResponse, Option<TurnLaunch>) {
        let sessions = match EventStore::new(&self.database).list_sessions() {
            Ok(sessions) => sessions,
            Err(error) => {
                return (
                    RpcResponse::error(id, ERR_INTERNAL, error.to_string()),
                    None,
                )
            }
        };
        let threads = sessions
            .into_iter()
            .map(|session| {
                serde_json::json!({
                    "threadId": session.id.to_string(),
                    "title": session.title,
                    "cwd": session.project,
                    "createdAt": session.created_at.as_millis(),
                    "updatedAt": session.updated_at.as_millis(),
                    "ended": session.ended_at.is_some()
                })
            })
            .collect::<Vec<_>>();
        (
            RpcResponse::success(id, serde_json::json!({ "threads": threads })),
            None,
        )
    }

    fn thread_read(
        &self,
        id: serde_json::Value,
        params: ThreadReadRequest,
    ) -> (RpcResponse, Option<TurnLaunch>) {
        let session_id = match params.thread_id.parse() {
            Ok(session_id) => session_id,
            Err(error) => {
                return (
                    RpcResponse::error(
                        id,
                        ERR_INVALID_THREAD,
                        format!("invalid thread id: {error}"),
                    ),
                    None,
                )
            }
        };
        let store = EventStore::new(&self.database);
        let session_after = params.session_after_sequence.or(params.after_sequence);
        let run_after = params.run_after_sequence.or(params.after_sequence);
        let record = match store.get_session(session_id) {
            Ok(Some(record)) => record,
            Ok(None) => {
                return (
                    RpcResponse::error(id, ERR_INVALID_THREAD, "thread not found"),
                    None,
                )
            }
            Err(error) => {
                return (
                    RpcResponse::error(id, ERR_INTERNAL, error.to_string()),
                    None,
                )
            }
        };
        let events = match store.load_session(session_id) {
            Ok(events) => events
                .into_iter()
                .filter(|event| session_after.map_or(true, |after| event.sequence > after))
                .filter_map(|event| {
                    let sequence = event.sequence;
                    let mut value = serde_json::to_value(event).ok()?;
                    if let Some(object) = value.as_object_mut() {
                        object.insert("stream".into(), serde_json::json!("session"));
                        object.insert("streamSequence".into(), serde_json::json!(sequence));
                    }
                    Some(value)
                })
                .collect::<Vec<_>>(),
            Err(error) => {
                return (
                    RpcResponse::error(id, ERR_INTERNAL, error.to_string()),
                    None,
                )
            }
        };
        let run_events = match RunStore::new(&self.database).list_for_session(&params.thread_id) {
            Ok(runs) => {
                let mut projected = Vec::with_capacity(runs.len());
                for run in runs {
                    let run_after = params
                        .run_after_sequences
                        .as_ref()
                        .and_then(|cursors| cursors.get(&run.id).copied())
                        .or(run_after);
                    let events =
                        match RunStore::new(&self.database).events_after(&run.id, run_after) {
                            Ok(events) => events
                                .into_iter()
                                .filter_map(|event| {
                                    let run_id = event.run_id.clone();
                                    let sequence = event.sequence;
                                    let mut value = serde_json::to_value(event).ok()?;
                                    if let Some(object) = value.as_object_mut() {
                                        object.insert("stream".into(), serde_json::json!("run"));
                                        object.insert("runId".into(), serde_json::json!(run_id));
                                        object.insert(
                                            "streamSequence".into(),
                                            serde_json::json!(sequence),
                                        );
                                    }
                                    Some(value)
                                })
                                .collect::<Vec<_>>(),
                            Err(error) => {
                                return (
                                    RpcResponse::error(id, ERR_INTERNAL, error.to_string()),
                                    None,
                                )
                            }
                        };
                    let controls = RunControlStore::new(&self.database);
                    let artifacts =
                        match ToolArtifactStore::new(&self.database).list_for_run(&run.id) {
                            Ok(artifacts) => artifacts,
                            Err(error) => {
                                return (
                                    RpcResponse::error(id.clone(), ERR_INTERNAL, error.to_string()),
                                    None,
                                )
                            }
                        };
                    let actions = controls.list_actions(&run.id).map_err(|error| {
                        RpcResponse::error(id.clone(), ERR_INTERNAL, error.to_string())
                    });
                    let approvals = controls.list_approvals(&run.id).map_err(|error| {
                        RpcResponse::error(id.clone(), ERR_INTERNAL, error.to_string())
                    });
                    let (actions, approvals) = match (actions, approvals) {
                        (Ok(actions), Ok(approvals)) => (actions, approvals),
                        (Err(response), _) | (_, Err(response)) => return (response, None),
                    };
                    let graph = match deepagent_app_core::run_graph::load(&self.database, &run.id) {
                        Ok(graph) => graph,
                        Err(error) => {
                            return (
                                RpcResponse::error(id.clone(), ERR_INTERNAL, error.to_string()),
                                None,
                            )
                        }
                    };
                    projected.push(serde_json::json!({
                        "runId": run.id,
                        "state": run.state,
                        "terminalKind": run.terminal_kind,
                        "events": events,
                        "actions": actions,
                        "approvals": approvals,
                        "artifacts": artifacts
                        ,"graph": graph
                    }));
                }
                projected
            }
            Err(error) => {
                return (
                    RpcResponse::error(id, ERR_INTERNAL, error.to_string()),
                    None,
                )
            }
        };
        (
            RpcResponse::success(
                id,
                serde_json::json!({
                    "threadId": params.thread_id,
                    "title": record.title,
                    "cwd": record.project,
                    "controlProjectionVersion": CONTROL_PROJECTION_VERSION,
                    "cursorMode": "stream_scoped",
                    "events": events,
                    "runEvents": run_events
                }),
            ),
            None,
        )
    }

    fn thread_fork(
        &mut self,
        id: serde_json::Value,
        params: ThreadForkRequest,
    ) -> (RpcResponse, Option<TurnLaunch>) {
        let source_id = match params.thread_id.parse() {
            Ok(source_id) => source_id,
            Err(error) => {
                return (
                    RpcResponse::error(
                        id,
                        ERR_INVALID_THREAD,
                        format!("invalid thread id: {error}"),
                    ),
                    None,
                )
            }
        };
        let store = EventStore::new(&self.database);
        let count = match store.event_count(source_id) {
            Ok(count) if count > 0 => count,
            Ok(_) => {
                return (
                    RpcResponse::error(id, ERR_INVALID_THREAD, "thread has no events"),
                    None,
                )
            }
            Err(error) => {
                return (
                    RpcResponse::error(id, ERR_INVALID_THREAD, error.to_string()),
                    None,
                )
            }
        };
        let at_sequence = params.at_sequence.unwrap_or(count - 1);
        let result = match AppService::from_shared(self.database.clone())
            .fork_session(&params.thread_id, at_sequence)
        {
            Ok(result) => result,
            Err(error) => {
                return (
                    RpcResponse::error(id, ERR_INTERNAL, error.to_string()),
                    None,
                )
            }
        };
        let source = self.threads.get(&params.thread_id).cloned();
        if let Some(source) = source {
            self.threads.insert(
                result.new_session_id.clone(),
                ThreadState {
                    chat: source.chat,
                    workspace: source.workspace,
                    provider: source.provider,
                    model: source.model,
                    permission_profile: source.permission_profile,
                    sandbox_backend: source.sandbox_backend,
                },
            );
        }
        (
            RpcResponse::success(
                id,
                serde_json::json!({
                    "threadId": result.new_session_id,
                    "sourceThreadId": result.source_session_id,
                    "forkedAt": result.forked_at,
                    "restoredPaths": result.restored_paths
                }),
            ),
            None,
        )
    }

    fn thread_archive(
        &self,
        id: serde_json::Value,
        params: ThreadArchiveRequest,
    ) -> (RpcResponse, Option<TurnLaunch>) {
        match ArchiveService::new(self.database.clone()).archive_session(&params.thread_id) {
            Ok(_) => (
                RpcResponse::success(
                    id,
                    serde_json::json!({ "threadId": params.thread_id, "archived": true }),
                ),
                None,
            ),
            Err(error) => (
                RpcResponse::error(id, ERR_INVALID_THREAD, error.to_string()),
                None,
            ),
        }
    }

    fn turn_start(
        &mut self,
        id: serde_json::Value,
        params: TurnStartRequest,
    ) -> (RpcResponse, Option<TurnLaunch>) {
        let Some(thread) = self.threads.get(&params.thread_id).cloned() else {
            return (
                RpcResponse::error(id, ERR_INVALID_THREAD, "thread is not loaded"),
                None,
            );
        };
        let turn_id = format!("turn_{}", deepagent_core::id::EventId::new());
        let overrides = HarnessRunOverrides {
            provider: params.provider.or(thread.provider),
            model: params.model.or(thread.model),
            reasoning_effort: params.reasoning_effort,
            sandbox_backend: params.sandbox_backend.or(thread.sandbox_backend),
            permission_profile: params.permission_profile.or(thread.permission_profile),
        };
        self.turns.insert(
            turn_id.clone(),
            TurnState {
                thread_id: params.thread_id.clone(),
                chat: thread.chat.clone(),
                active: true,
            },
        );
        (
            RpcResponse::success(
                id,
                serde_json::json!({
                    "threadId": params.thread_id,
                    "turnId": turn_id,
                    "status": "started"
                }),
            ),
            Some(TurnLaunch {
                chat: thread.chat,
                thread_id: params.thread_id,
                turn_id,
                input: params.input,
                overrides,
            }),
        )
    }

    fn turn_interrupt(
        &mut self,
        id: serde_json::Value,
        params: TurnInterruptRequest,
    ) -> (RpcResponse, Option<TurnLaunch>) {
        let Some(turn) = self.turns.get(&params.turn_id) else {
            return (
                RpcResponse::error(id, ERR_INVALID_TURN, "turn not found"),
                None,
            );
        };
        if turn.thread_id != params.thread_id {
            return (
                RpcResponse::error(id, ERR_INVALID_TURN, "turn does not belong to thread"),
                None,
            );
        }
        if !turn.active {
            return (
                RpcResponse::error(id, ERR_INVALID_TURN, "turn is not active"),
                None,
            );
        }
        // The turn may have been acknowledged by the protocol before
        // ChatService has installed its cancellation alias. The active turn
        // registry is authoritative for the app-server ACK; the runtime
        // cancellation call remains the single execution-side mechanism.
        if let Err(error) = turn.chat.request_cancel(&params.turn_id) {
            return (
                RpcResponse::error(id, ERR_INTERNAL, error.to_string()),
                None,
            );
        }
        (
            RpcResponse::success(
                id,
                serde_json::json!({
                    "threadId": params.thread_id,
                    "turnId": params.turn_id,
                    "status": "cancelling"
                }),
            ),
            None,
        )
    }

    fn turn_steer(
        &mut self,
        id: serde_json::Value,
        params: TurnSteerRequest,
    ) -> (RpcResponse, Option<TurnLaunch>) {
        let Some(current) = self.turns.get(&params.turn_id).cloned() else {
            return (
                RpcResponse::error(id, ERR_INVALID_TURN, "turn not found"),
                None,
            );
        };
        if current.thread_id != params.thread_id || !current.active {
            return (
                RpcResponse::error(id, ERR_INVALID_TURN, "turn is not active for thread"),
                None,
            );
        }
        let cancellation = match current.chat.request_cancel(&params.turn_id) {
            Ok(request) => request,
            Err(error) => {
                return (
                    RpcResponse::error(id, ERR_INTERNAL, error.to_string()),
                    None,
                )
            }
        };
        if !cancellation.accepted {
            return (
                RpcResponse::error(id, ERR_INVALID_TURN, "turn is no longer cancellable"),
                None,
            );
        }
        let next_turn = format!("turn_{}", deepagent_core::id::EventId::new());
        if let Err(error) =
            current
                .chat
                .record_continuation(&params.turn_id, &params.turn_id, &next_turn)
        {
            return (
                RpcResponse::error(id, ERR_INTERNAL, error.to_string()),
                None,
            );
        }
        self.turns.insert(
            next_turn.clone(),
            TurnState {
                thread_id: params.thread_id.clone(),
                chat: current.chat.clone(),
                active: true,
            },
        );
        (
            RpcResponse::success(
                id,
                serde_json::json!({
                    "threadId": params.thread_id,
                    "turnId": next_turn,
                    "replacesTurnId": params.turn_id,
                    "status": "started"
                }),
            ),
            Some(TurnLaunch {
                chat: current.chat,
                thread_id: params.thread_id,
                turn_id: next_turn,
                input: params.input,
                overrides: HarnessRunOverrides::default(),
            }),
        )
    }

    fn approval_respond(
        &self,
        id: serde_json::Value,
        params: deepagent_harness_protocol::ApprovalRespondRequest,
    ) -> (RpcResponse, Option<TurnLaunch>) {
        let resolved = match self.base_chat.resolve_approval_scoped(
            &params.approval_id,
            params.approved,
            params.scope.as_deref(),
            "harness_client",
        ) {
            Ok(resolved) => resolved,
            Err(error) => {
                return (
                    RpcResponse::error(id, ERR_INVALID_PARAMS, error.to_string()),
                    None,
                )
            }
        };
        if !resolved {
            return (
                RpcResponse::error(id, ERR_INVALID_PARAMS, "approval is not pending"),
                None,
            );
        }
        (
            RpcResponse::success(
                id,
                serde_json::json!({
                    "approvalId": params.approval_id,
                    "status": "resolved",
                    "approved": params.approved
                }),
            ),
            None,
        )
    }

    fn tool_list(
        &self,
        id: serde_json::Value,
        _params: ToolListRequest,
    ) -> (RpcResponse, Option<TurnLaunch>) {
        let chat = self
            .threads
            .values()
            .next()
            .map(|thread| thread.chat.clone())
            .unwrap_or_else(|| self.base_chat.clone());
        match chat.tool_descriptors() {
            Ok(tools) => (
                RpcResponse::success(
                    id,
                    serde_json::json!({ "protocolVersion": PROTOCOL_VERSION, "tools": tools }),
                ),
                None,
            ),
            Err(error) => (
                RpcResponse::error(id, ERR_INTERNAL, error.to_string()),
                None,
            ),
        }
    }

    fn resolve_workspace(&self, requested: Option<&str>) -> Result<PathBuf, String> {
        let workspace = requested
            .map(PathBuf::from)
            .unwrap_or_else(|| self.workspace.clone());
        if !workspace.exists() {
            return Err(format!("workspace does not exist: {}", workspace.display()));
        }
        if !workspace.is_dir() {
            return Err(format!(
                "workspace is not a directory: {}",
                workspace.display()
            ));
        }
        std::fs::canonicalize(&workspace)
            .map_err(|error| format!("resolve workspace '{}': {error}", workspace.display()))
    }

    fn finish_turn(&mut self, turn_id: &str) {
        if let Some(turn) = self.turns.get_mut(turn_id) {
            turn.active = false;
        }
    }
}

pub async fn run_stdio(
    chat: ChatService,
    workspace: PathBuf,
    capabilities: SandboxCapabilities,
) -> Result<(), String> {
    let state = Arc::new(Mutex::new(ServerState::new(chat, workspace, capabilities)));
    let stdout = Arc::new(tokio::sync::Mutex::new(tokio::io::stdout()));
    let outbox = EventOutbox::new(stdout.clone());
    let emitter = {
        let outbox = outbox.clone();
        Arc::new(move |event: HarnessEvent| {
            outbox.emit(event);
        }) as EventEmitter
    };

    let stdin = tokio::io::stdin();
    let mut lines = tokio::io::BufReader::new(stdin).lines();
    while let Some(line) = lines
        .next_line()
        .await
        .map_err(|error| format!("read app-server stdin: {error}"))?
    {
        if line.trim().is_empty() {
            continue;
        }
        let request = match serde_json::from_str::<RpcRequest>(&line) {
            Ok(request) => request,
            Err(error) => {
                write_response(
                    &stdout,
                    &RpcResponse::error(serde_json::Value::Null, -32700, error.to_string()),
                )
                .await?;
                continue;
            }
        };
        let (response, launch) = {
            let mut guard = state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            guard.dispatch(request, Some(&emitter))
        };
        write_response(&stdout, &response).await?;
        if let Some(launch) = launch {
            spawn_turn(state.clone(), emitter.clone(), launch);
        }
    }
    Ok(())
}

fn spawn_turn(state: Arc<Mutex<ServerState>>, emitter: EventEmitter, launch: TurnLaunch) {
    tokio::spawn(async move {
        let context =
            EventContext::new(Some(launch.thread_id.clone()), Some(launch.turn_id.clone()));
        let event_emitter = emitter.clone();
        let event_context = context.clone();
        let on_event = move |event: deepagent_runtime::RuntimeEvent| {
            if let Some(projected) = project_runtime_event(&event, &event_context) {
                event_emitter(projected);
            }
        };
        let approval_emitter = emitter.clone();
        let approval_context = context.clone();
        let on_approval = move |approval: deepagent_app_core::ApprovalRequestDto| {
            approval_emitter(HarnessEvent::ApprovalRequested {
                approval_id: Some(approval.call_id),
                thread_id: approval_context.thread_id.clone(),
                turn_id: approval_context.turn_id.clone(),
                tool_name: Some(approval.tool),
                reason: approval.reason,
                scope: Some("tool".into()),
            });
        };
        let result = launch
            .chat
            .run_in_session_with_overrides(
                &launch.input,
                Some(&launch.thread_id),
                None,
                None,
                Vec::new(),
                None,
                false,
                Some(&launch.turn_id),
                launch.overrides,
                on_event,
                on_approval,
            )
            .await;
        if let Err(error) = result {
            emitter(HarnessEvent::TurnFailed {
                thread_id: Some(launch.thread_id.clone()),
                turn_id: Some(launch.turn_id.clone()),
                reason: error.to_string(),
            });
        }
        state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .finish_turn(&launch.turn_id);
    });
}

async fn write_response(
    stdout: &Arc<tokio::sync::Mutex<tokio::io::Stdout>>,
    response: &RpcResponse,
) -> Result<(), String> {
    let line = serde_json::to_string(response)
        .map_err(|error| format!("serialize app-server response: {error}"))?;
    let mut output = stdout.lock().await;
    output
        .write_all(format!("{line}\n").as_bytes())
        .await
        .map_err(|error| format!("write app-server response: {error}"))?;
    output
        .flush()
        .await
        .map_err(|error| format!("flush app-server response: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_requests_before_initialize() {
        let mut state = ServerState::new_for_test();
        let request = rpc_request(1, "thread/list", serde_json::json!({}));

        let response = state.handle_request_for_test(request);

        assert_eq!(response.error_code(), Some(-32001));
        assert!(response.error_message().unwrap().contains("initialize"));
    }

    #[test]
    fn accepts_initialize_once_and_rejects_duplicate() {
        let mut state = ServerState::new_for_test();
        let request = rpc_request(
            1,
            "initialize",
            serde_json::json!({
                "clientName": "test-client",
                "clientVersion": "0.1.0",
                "protocolVersion": PROTOCOL_VERSION
            }),
        );

        let first = state.handle_request_for_test(request.clone());
        assert!(first.is_success());

        let second = state.handle_request_for_test(request);
        assert_eq!(second.error_code(), Some(-32002));
    }

    #[test]
    fn event_ack_is_monotonic() {
        let mut state = ServerState::new_for_test();
        initialize(&mut state);
        let first = state.handle_request_for_test(rpc_request(
            2,
            "event/ack",
            serde_json::json!({ "eventSequence": 4 }),
        ));
        assert_eq!(first.result()["acknowledgedEventSequence"], 4);

        let backwards = state.handle_request_for_test(rpc_request(
            3,
            "event/ack",
            serde_json::json!({ "eventSequence": 3 }),
        ));
        assert_eq!(backwards.error_code(), Some(ERR_INVALID_PARAMS));
    }

    #[test]
    fn thread_start_read_and_fork_use_persisted_session_stream() {
        let mut state = ServerState::new_for_test();
        initialize(&mut state);

        let started = state.handle_request_for_test(rpc_request(
            2,
            "thread/start",
            serde_json::json!({ "cwd": state.workspace().to_string_lossy() }),
        ));
        let thread_id = started.result()["threadId"].as_str().unwrap().to_string();

        let read = state.handle_request_for_test(rpc_request(
            3,
            "thread/read",
            serde_json::json!({ "threadId": thread_id }),
        ));
        assert_eq!(read.result()["threadId"], thread_id);
        assert_eq!(read.result()["controlProjectionVersion"], 1);
        assert_eq!(read.result()["cursorMode"], "stream_scoped");
        assert!(!read.result()["events"].as_array().unwrap().is_empty());
        assert!(read.result()["runEvents"].as_array().unwrap().is_empty());
        assert!(read.result()["runEvents"].is_array());

        let forked = state.handle_request_for_test(rpc_request(
            4,
            "thread/fork",
            serde_json::json!({ "threadId": thread_id }),
        ));
        assert_ne!(forked.result()["threadId"], thread_id);
    }

    #[test]
    fn turn_start_returns_turn_id_and_interrupt_routes_to_chat_cancellation() {
        let mut state = ServerState::new_for_test();
        initialize(&mut state);
        let started =
            state.handle_request_for_test(rpc_request(2, "thread/start", serde_json::json!({})));
        let thread_id = started.result()["threadId"].as_str().unwrap();

        let turn = state.handle_request_for_test(rpc_request(
            3,
            "turn/start",
            serde_json::json!({
                "threadId": thread_id,
                "input": "test"
            }),
        ));
        let turn_id = turn.result()["turnId"].as_str().unwrap();
        assert_eq!(turn.result()["status"], "started");

        let interrupted = state.handle_request_for_test(rpc_request(
            4,
            "turn/interrupt",
            serde_json::json!({
                "threadId": thread_id,
                "turnId": turn_id
            }),
        ));
        assert_eq!(interrupted.result()["status"], "cancelling");
    }

    #[test]
    fn steer_requires_a_known_active_turn() {
        let mut state = ServerState::new_for_test();
        initialize(&mut state);

        let response = state.handle_request_for_test(rpc_request(
            2,
            "turn/steer",
            serde_json::json!({
                "threadId": "missing",
                "turnId": "missing",
                "input": "continue"
            }),
        ));
        assert_eq!(response.error_code(), Some(-32004));
    }

    fn initialize(state: &mut ServerState) {
        let response = state.handle_request_for_test(rpc_request(
            0,
            "initialize",
            serde_json::json!({
                "clientName": "test-client",
                "clientVersion": "0.1.0",
                "protocolVersion": PROTOCOL_VERSION
            }),
        ));
        assert!(response.is_success());
    }

    fn rpc_request(id: i64, method: &str, params: serde_json::Value) -> RpcRequest {
        RpcRequest {
            jsonrpc: "2.0".into(),
            id: serde_json::json!(id),
            method: method.into(),
            params,
        }
    }
}
