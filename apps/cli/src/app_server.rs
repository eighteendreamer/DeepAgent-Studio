use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use deepagent_core::execution_config::SessionExecutionConfig;

use deepagent_app_core::{
    AppService, ArchiveService, ChatService, HarnessRunOverrides, SandboxCapabilities,
};
use deepagent_core::clock::SystemClock;
use deepagent_core::error::CoreError;
use deepagent_core::id::SessionId;
use deepagent_harness_protocol::{
    project_runtime_event, ConfigReadRequest, EventContext, HarnessEvent, HarnessRequest,
    InitializeRequest, RpcNotification, RpcRequest, RpcResponse, ThreadArchiveRequest,
    ThreadForkRequest, ThreadListRequest, ThreadReadRequest, ThreadResumeRequest,
    ThreadStartRequest, ToolListRequest, TurnInterruptRequest, TurnStartRequest, TurnSteerRequest,
    CONTROL_PROJECTION_VERSION, JSONRPC_VERSION, PROTOCOL_VERSION,
};
use deepagent_persistence::event_store::EventStore;
use deepagent_persistence::run_store::RunStore;
use deepagent_persistence::Database;
use deepagent_session::Session;
use futures::{stream::FuturesUnordered, StreamExt};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt};

#[path = "app_server/event_outbox.rs"]
mod event_outbox;
use event_outbox::{EventOutbox, OutputState};
#[path = "app_server/connection.rs"]
mod connection;
#[path = "app_server/history_cursor.rs"]
mod history_cursor;
#[path = "app_server/history_page.rs"]
mod history_page;
#[cfg(test)]
use connection::complete_turn_dispatch;
use connection::run_stdio_with_io;

#[cfg(test)]
#[path = "app_server/history_page_tests.rs"]
mod history_page_tests;
#[cfg(test)]
#[path = "app_server/history_test_support.rs"]
mod history_test_support;

use crate::args::RunOptions;

const NOTIFICATION_METHOD: &str = "harness/event";
const ERR_NOT_INITIALIZED: i32 = -32001;
const ERR_ALREADY_INITIALIZED: i32 = -32002;
const ERR_INVALID_THREAD: i32 = -32003;
const ERR_INVALID_TURN: i32 = -32004;
const ERR_THREAD_BUSY: i32 = -32005;
const ERR_INVALID_PARAMS: i32 = -32602;
const ERR_METHOD_NOT_FOUND: i32 = -32601;
const ERR_INTERNAL: i32 = -32603;

type EventEmitter = Arc<dyn Fn(HarnessEvent) + Send + Sync>;

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
    let result = run_stdio(chat, workspace, capabilities).await;
    tracing::debug!(success = result.is_ok(), "app-server host command returned");
    result
}

#[derive(Clone)]
struct ThreadState {
    chat: ChatService,
    workspace: PathBuf,
    overrides: HarnessRunOverrides,
}

struct TurnLaunch {
    chat: ChatService,
    thread_id: String,
    turn_id: String,
    input: String,
    prepared: deepagent_app_core::run_assembler::PreparedChatRun,
    #[cfg(test)]
    overrides: HarnessRunOverrides,
}

#[derive(Clone)]
struct TurnState {
    thread_id: String,
    chat: ChatService,
    active: bool,
    overrides: HarnessRunOverrides,
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
    output: Arc<OutputState>,
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
            output: Arc::new(OutputState::default()),
            threads: HashMap::new(),
            turns: HashMap::new(),
        }
    }

    #[cfg(test)]
    fn new_for_test() -> Self {
        let database = Arc::new(Database::open_in_memory().expect("test database"));
        let workspace = std::env::current_dir().expect("test cwd");
        Self::new_for_test_with_database(database, workspace)
    }

    #[cfg(test)]
    fn new_for_test_with_database(database: Arc<Database>, workspace: PathBuf) -> Self {
        let transport: Arc<dyn deepagent_models::HttpTransport> =
            Arc::new(deepagent_models::ReqwestTransport::new());
        let settings = Arc::new(deepagent_app_core::SettingsService::new(
            database.clone(),
            transport.clone(),
            Arc::new(deepagent_app_core::EnvSecretStore::new()),
        ));
        let chat = ChatService::new(database, settings, transport, workspace.clone())
            .with_headless_knowledge(&workspace);
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
        if request.jsonrpc != JSONRPC_VERSION {
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
            HarnessRequest::ConfigRead(params) => self.config_read(request.id, params),
            HarnessRequest::SandboxStatus(_) => {
                let mut status = serde_json::to_value(&self.capabilities)
                    .unwrap_or_else(|_| serde_json::json!({ "available": false }));
                if let Ok(readiness) = self.base_chat.coordinator_readiness() {
                    if let Some(object) = status.as_object_mut() {
                        object.insert(
                            "coordinator".into(),
                            serde_json::to_value(readiness)
                                .unwrap_or_else(|_| serde_json::json!({ "ready": false })),
                        );
                    }
                }
                (RpcResponse::success(request.id, status), None)
            }
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
        if event_sequence > self.output.delivered_sequence.load(Ordering::Acquire) {
            return (
                RpcResponse::error(
                    id,
                    ERR_INVALID_PARAMS,
                    "event acknowledgment exceeds this connection's flushed notifications",
                ),
                None,
            );
        }
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
        let configuration = SessionExecutionConfig::new(HarnessRunOverrides {
            provider: params.provider,
            model: params.model,
            permission_profile: params.permission_profile,
            sandbox_backend: params.sandbox_backend,
            ..Default::default()
        });
        let session = match Session::create_configured(
            &self.database,
            &SystemClock,
            None,
            deepagent_core::SessionMode::Normal,
            Some(&workspace.to_string_lossy()),
            Some(configuration.clone()),
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
                overrides: configuration.overrides.clone(),
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
                    "configuration": configuration,
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
                    RpcResponse::error(id, ERR_INVALID_THREAD, error.to_string()),
                    None,
                )
            }
        };
        match store.event_count(session_id) {
            Ok(0) => {
                return (
                    RpcResponse::error(id, ERR_INVALID_THREAD, "thread has no session file"),
                    None,
                )
            }
            Ok(_) => {}
            Err(error) => {
                return (
                    RpcResponse::error(id, ERR_INTERNAL, error.to_string()),
                    None,
                )
            }
        }
        let workspace = record
            .project
            .map(PathBuf::from)
            .unwrap_or_else(|| self.workspace.clone());
        let configuration = match store.session_execution_config(session_id) {
            Ok(configuration) => configuration,
            Err(error) => {
                return (
                    RpcResponse::error(id, ERR_INTERNAL, error.to_string()),
                    None,
                )
            }
        };
        let overrides = configuration
            .as_ref()
            .map(|config| config.overrides.clone())
            .unwrap_or_default();
        self.threads.insert(
            params.thread_id.clone(),
            ThreadState {
                chat: self.base_chat.for_workspace(workspace.clone()),
                workspace: workspace.clone(),
                overrides,
            },
        );
        (
            RpcResponse::success(
                id,
                serde_json::json!({
                    "threadId": params.thread_id,
                    "status": "ready",
                    "cwd": workspace,
                    "configuration": configuration,
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
        let response = match history_page::read(&self.database, id.clone(), params) {
            Ok(response) => response,
            Err(error) => {
                let code = match &error {
                    CoreError::Invalid(_) => ERR_INVALID_PARAMS,
                    CoreError::NotFound(_) => ERR_INVALID_THREAD,
                    _ => ERR_INTERNAL,
                };
                RpcResponse::error(id, code, error.to_string())
            }
        };
        (response, None)
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
                let code = match &error {
                    CoreError::Invalid(_) => ERR_INVALID_PARAMS,
                    CoreError::NotFound(_) => ERR_INVALID_THREAD,
                    _ => ERR_INTERNAL,
                };
                return (RpcResponse::error(id, code, error.to_string()), None);
            }
        };
        let source = self.threads.get(&params.thread_id).cloned();
        if let Some(source) = source {
            self.threads.insert(
                result.new_session_id.clone(),
                ThreadState {
                    chat: source.chat,
                    workspace: source.workspace,
                    overrides: source.overrides,
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
        // dispatch holds the connection's ServerState mutex through admission
        // and installation. ACKed, queued steering and cancelling workers all
        // remain active until finish_turn, after AppCore releases their leases.
        // Ordinary start must never trigger accept_input_turn's replace policy.
        // Resume accepts equivalent SessionId spellings; those still share the
        // same durable session and must not bypass its busy reservation.
        let session_id = params.thread_id.parse::<SessionId>().ok();
        if self.turns.values().any(|turn| {
            turn.active
                && (turn.thread_id == params.thread_id
                    || session_id
                        .is_some_and(|id| turn.thread_id.parse::<SessionId>().ok() == Some(id)))
        }) {
            return (
                RpcResponse::error(
                    id,
                    ERR_THREAD_BUSY,
                    "thread is busy; wait for the active turn to finish or use turn/steer explicitly",
                ),
                None,
            );
        }
        let turn_id = format!("turn_{}", deepagent_core::id::EventId::new());
        let overrides = HarnessRunOverrides {
            provider: params.provider.or(thread.overrides.provider),
            model: params.model.or(thread.overrides.model),
            reasoning_effort: params
                .reasoning_effort
                .or(thread.overrides.reasoning_effort),
            sandbox_backend: params.sandbox_backend.or(thread.overrides.sandbox_backend),
            permission_profile: params
                .permission_profile
                .or(thread.overrides.permission_profile),
        };
        let prepared = match thread.chat.admit_prepared_input_with_config(
            &turn_id,
            &params.thread_id,
            &params.input,
            overrides.clone(),
            params.expected_config_snapshot_id.as_deref(),
        ) {
            Ok(prepared) => prepared,
            Err(error) => {
                return (
                    RpcResponse::error(
                        id,
                        if matches!(error, CoreError::ExecutionConfigStale) {
                            ERR_INVALID_PARAMS
                        } else {
                            ERR_INTERNAL
                        },
                        error.to_string(),
                    ),
                    None,
                )
            }
        };
        self.turns.insert(
            turn_id.clone(),
            TurnState {
                thread_id: params.thread_id.clone(),
                chat: thread.chat.clone(),
                active: true,
                overrides: overrides.clone(),
            },
        );
        (
            RpcResponse::success(
                id,
                serde_json::json!({
                    "threadId": params.thread_id,
                    "turnId": turn_id,
                    "status": "started",
                    "snapshotId": prepared.capability_snapshot()["snapshotId"],
                    "capabilitySnapshot": prepared.capability_snapshot()
                }),
            ),
            Some(TurnLaunch {
                chat: thread.chat,
                thread_id: params.thread_id,
                turn_id,
                input: params.input,
                prepared,
                #[cfg(test)]
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
        // A worker can commit its terminal before finish_turn updates this
        // connection's view. Durable completion and the shared cancellation
        // registry determine whether an interrupt can still be accepted.
        match RunStore::new(&self.database).get(&params.turn_id) {
            Ok(Some(run)) if run.finished_at.is_some() => {
                return (
                    RpcResponse::error(id, ERR_INVALID_TURN, "turn is already terminal"),
                    None,
                );
            }
            Err(error) => {
                return (
                    RpcResponse::error(id, ERR_INTERNAL, error.to_string()),
                    None,
                );
            }
            _ => {}
        }
        match turn.chat.request_cancel(&params.turn_id) {
            Ok(request) if request.accepted => {}
            Ok(_) => {
                return (
                    RpcResponse::error(id, ERR_INVALID_TURN, "turn is no longer cancellable"),
                    None,
                );
            }
            Err(error) => {
                return (
                    RpcResponse::error(id, ERR_INTERNAL, error.to_string()),
                    None,
                );
            }
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
        let next_turn = format!("turn_{}", deepagent_core::id::EventId::new());
        // Worker activity is not steering eligibility: a replaced worker still
        // owns cleanup and must remain busy/cancellable until finish_turn. The
        // canonical ledger atomically claims its sole replacement instead.
        let prepared = match current.chat.admit_prepared_continuation(
            &params.turn_id,
            &next_turn,
            &params.thread_id,
            &params.input,
            current.overrides.clone(),
        ) {
            Ok(Some(prepared)) => prepared,
            Ok(None) => {
                return (
                    RpcResponse::error(
                        id,
                        ERR_INVALID_TURN,
                        "turn is terminal, replaced or no longer cancellable",
                    ),
                    None,
                );
            }
            Err(error) => {
                return (
                    RpcResponse::error(id, ERR_INTERNAL, error.to_string()),
                    None,
                )
            }
        };
        self.turns.insert(
            next_turn.clone(),
            TurnState {
                thread_id: params.thread_id.clone(),
                chat: current.chat.clone(),
                active: true,
                overrides: current.overrides.clone(),
            },
        );
        (
            RpcResponse::success(
                id,
                serde_json::json!({
                    "threadId": params.thread_id,
                    "turnId": next_turn,
                    "replacesTurnId": params.turn_id,
                    "status": "started",
                    "snapshotId": prepared.capability_snapshot()["snapshotId"],
                    "capabilitySnapshot": prepared.capability_snapshot()
                }),
            ),
            Some(TurnLaunch {
                chat: current.chat,
                thread_id: params.thread_id,
                turn_id: next_turn,
                input: params.input,
                prepared,
                #[cfg(test)]
                overrides: current.overrides,
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
        params: ToolListRequest,
    ) -> (RpcResponse, Option<TurnLaunch>) {
        if params.thread_id.is_none() && params.run_id.is_none() {
            if params.expected_snapshot_id.is_some() {
                return (
                    RpcResponse::error(
                        id,
                        ERR_INVALID_PARAMS,
                        "snapshot expectation requires thread or run scope",
                    ),
                    None,
                );
            }
            return match self.base_chat.tool_descriptors() {
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
            };
        }

        let requested_thread = match params.thread_id.as_deref() {
            Some(thread_id) => match thread_id.parse::<SessionId>() {
                Ok(session_id) => match EventStore::new(&self.database).get_session(session_id) {
                    Ok(Some(_)) => Some(session_id.to_string()),
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
                },
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
            },
            None => None,
        };
        let store = RunStore::new(&self.database);
        let run = match params.run_id.as_deref() {
            Some(run_id) => match store.get(run_id) {
                Ok(Some(run)) => run,
                Ok(None) => {
                    return (
                        RpcResponse::error(id, ERR_INVALID_TURN, "run not found"),
                        None,
                    )
                }
                Err(error) => {
                    return (
                        RpcResponse::error(id, ERR_INTERNAL, error.to_string()),
                        None,
                    )
                }
            },
            None => {
                let Some(thread_id) = requested_thread.as_deref() else {
                    unreachable!("a scoped request has a thread or run id")
                };
                match store.list_for_session(thread_id) {
                    Ok(mut runs) if !runs.is_empty() => runs.remove(0),
                    Ok(_) => {
                        return (
                            RpcResponse::error(id, ERR_INVALID_TURN, "thread has no run snapshot"),
                            None,
                        )
                    }
                    Err(error) => {
                        return (
                            RpcResponse::error(id, ERR_INTERNAL, error.to_string()),
                            None,
                        )
                    }
                }
            }
        };
        if requested_thread
            .as_deref()
            .is_some_and(|thread_id| thread_id != run.session_id)
        {
            return (
                RpcResponse::error(id, ERR_INVALID_THREAD, "run does not belong to thread"),
                None,
            );
        }
        let snapshot = match store.capability_snapshot(&run.id) {
            Ok(Some(snapshot)) => snapshot,
            Ok(None) => {
                return (
                    RpcResponse::error(id, ERR_INVALID_TURN, "run has no capability snapshot"),
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
        let Some(snapshot_id) = snapshot
            .get("snapshotId")
            .and_then(serde_json::Value::as_str)
            .map(ToOwned::to_owned)
        else {
            return (
                RpcResponse::error(
                    id,
                    ERR_INVALID_TURN,
                    "run has no versioned capability projection",
                ),
                None,
            );
        };
        let Some(tools) = snapshot
            .get("tools")
            .and_then(serde_json::Value::as_array)
            .cloned()
        else {
            return (
                RpcResponse::error(
                    id,
                    ERR_INVALID_TURN,
                    "run capability projection has no tools",
                ),
                None,
            );
        };
        if params
            .expected_snapshot_id
            .as_deref()
            .is_some_and(|expected| expected != snapshot_id)
        {
            return (RpcResponse::error(id, ERR_INVALID_PARAMS, "stale capability snapshot; query the current run or specify a historical runId"), None);
        }
        (
            RpcResponse::success(
                id,
                serde_json::json!({
                    "protocolVersion": PROTOCOL_VERSION,
                    "threadId": run.session_id,
                    "runId": run.id,
                    "snapshotId": snapshot_id,
                    "tools": tools,
                    "capabilitySnapshot": snapshot
                }),
            ),
            None,
        )
    }

    fn config_read(
        &self,
        id: serde_json::Value,
        params: ConfigReadRequest,
    ) -> (RpcResponse, Option<TurnLaunch>) {
        if params.thread_id.is_some() && params.cwd.is_some() {
            return (
                RpcResponse::error(
                    id,
                    ERR_INVALID_PARAMS,
                    "threadId and cwd are mutually exclusive",
                ),
                None,
            );
        }
        let resolved = (|| -> Result<_, CoreError> {
            let (workspace, overrides, scope) = if let Some(thread_id) = params.thread_id {
                let session_id = thread_id.parse::<SessionId>()?;
                let store = EventStore::new(&self.database);
                let record = store
                    .get_session(session_id)?
                    .ok_or_else(|| CoreError::not_found("configuration thread"))?;
                let overrides = store
                    .session_execution_config(session_id)?
                    .map(|config| config.overrides)
                    .unwrap_or_default();
                let workspace = self
                    .resolve_workspace(record.project.as_deref())
                    .map_err(CoreError::invalid)?;
                (
                    workspace,
                    overrides,
                    serde_json::json!({"kind":"thread","threadId":session_id.to_string(),"requiresThreadScope":false}),
                )
            } else {
                let kind = if params.cwd.is_some() {
                    "cwd"
                } else {
                    "global"
                };
                let workspace = self
                    .resolve_workspace(params.cwd.as_deref())
                    .map_err(CoreError::invalid)?;
                (
                    workspace,
                    HarnessRunOverrides::default(),
                    serde_json::json!({"kind":kind,"requiresThreadScope":true,"includesProjectLayers":true}),
                )
            };
            let mut snapshot = self
                .base_chat
                .for_workspace(workspace)
                .read_execution_config(&overrides)?;
            snapshot["scope"] = scope;
            snapshot["protocolVersion"] = PROTOCOL_VERSION.into();
            snapshot["transport"] = "stdio".into();
            Ok(snapshot)
        })();
        match resolved {
            Ok(snapshot) => (RpcResponse::success(id, snapshot), None),
            // Do not serialize parser/IO diagnostics containing private paths,
            // raw policy fields or arbitrary untrusted project values.
            Err(error) => {
                let (code, kind) = match &error {
                    CoreError::Invalid(_) => (ERR_INVALID_PARAMS, "invalid_configuration"),
                    CoreError::NotFound(_) => (ERR_INVALID_PARAMS, "scope_not_found"),
                    CoreError::Persistence(_) => (ERR_INTERNAL, "storage_failure"),
                    CoreError::Serialization(_) => (ERR_INTERNAL, "decode_failure"),
                    _ => (ERR_INTERNAL, "resolution_failure"),
                };
                tracing::warn!(
                    operation = "config/read",
                    error_kind = kind,
                    "configuration resolution failed; private diagnostics redacted"
                );
                (
                    RpcResponse::error(
                        id,
                        code,
                        format!("configuration scope is unavailable or invalid ({kind})"),
                    ),
                    None,
                )
            }
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
    run_stdio_with_io(
        chat,
        workspace,
        capabilities,
        tokio::io::stdin(),
        tokio::io::stdout(),
    )
    .await
}

// One connection implementation for production stdio and controlled pipe tests.

fn spawn_turn(
    state: Arc<Mutex<ServerState>>,
    emitter: EventEmitter,
    launch: TurnLaunch,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let context =
            EventContext::new(Some(launch.thread_id.clone()), Some(launch.turn_id.clone()));
        let event_emitter = emitter.clone();
        let event_context = context.clone();
        let interrupted = Arc::new(AtomicBool::new(false));
        let event_interrupted = interrupted.clone();
        let terminal_emitted = Arc::new(AtomicBool::new(false));
        let event_terminal_emitted = terminal_emitted.clone();
        let on_event = move |event: deepagent_runtime::RuntimeEvent| {
            if event.is_terminal() {
                event_terminal_emitted.store(true, Ordering::Release);
            }
            if matches!(event, deepagent_runtime::RuntimeEvent::RunCancelled { .. }) {
                event_interrupted.store(true, Ordering::Release);
            }
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
            .run_prepared(launch.prepared, &launch.input, on_event, on_approval)
            .await;
        // Cancellation before preparation or while queued never enters the
        // kernel's event sink. Project its committed outcome through the same
        // protocol mapper; do not write another terminal or duplicate a live one.
        let result = result.and_then(|session| {
            let db = launch.chat.database();
            let run = RunStore::new(&db).get(&launch.turn_id)?;
            if run.is_some_and(|run| {
                run.terminal_kind.as_deref() == Some("cancelled")
                    && !interrupted.load(Ordering::Acquire)
            }) {
                if let Some(event) = project_runtime_event(
                    &deepagent_runtime::RuntimeEvent::RunCancelled {
                        edited_files: Vec::new(),
                    },
                    &context,
                ) {
                    emitter(event);
                }
            }
            Ok(session)
        });
        if let Err(error) = result {
            if !terminal_emitted.load(Ordering::Acquire) {
                emitter(HarnessEvent::TurnFailed {
                    thread_id: Some(launch.thread_id.clone()),
                    turn_id: Some(launch.turn_id.clone()),
                    reason: error.to_string(),
                });
            }
        }
        state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .finish_turn(&launch.turn_id);
    })
}

async fn write_response<W: AsyncWrite + Unpin>(
    stdout: &Arc<tokio::sync::Mutex<W>>,
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
#[path = "app_server/terminal_audit.rs"]
mod terminal_audit;

#[cfg(test)]
mod tests {
    use super::*;

    // Audit probes intentionally assert the desired invariant and stay ignored
    // until the corresponding production defect is fixed. See the 2026-10-21
    // system Harness audit for reproduction commands and evidence boundaries.
    #[test]
    fn audit_stalled_outbox_preserves_terminal_notification() {
        let (outbox, mut rx) = EventOutbox::channel(256);
        for step in 0..256 {
            outbox.emit(HarnessEvent::TurnStarted {
                thread_id: Some("audit-thread".into()),
                turn_id: "audit-turn".into(),
                step: Some(step),
            });
        }
        outbox.emit(HarnessEvent::TurnCompleted {
            thread_id: Some("audit-thread".into()),
            turn_id: Some("audit-turn".into()),
            message: "done".into(),
        });
        let mut received = Vec::new();
        while let Ok(event) = rx.try_recv() {
            received.push(event);
        }
        let terminal_received = received
            .iter()
            .any(|queued| matches!(queued.event, HarnessEvent::TurnCompleted { .. }));
        outbox.emit(HarnessEvent::TurnStarted {
            thread_id: Some("audit-thread".into()),
            turn_id: "next-turn".into(),
            step: None,
        });
        // Explicit connection failure is the alternative to lossless delivery;
        // the poisoned connection must not resume accepting new notifications.
        assert!(rx.try_recv().is_err());
        let failed = outbox.output.failure_reason();
        eprintln!(
            "audit outbox: queued={}, terminal_received={terminal_received}, connection_failure={failed:?}",
            received.len()
        );
        assert!(
            terminal_received || failed.as_deref() == Some("event outbox capacity exceeded"),
            "terminal must be delivered or the connection must fail explicitly"
        );
    }

    #[test]
    #[ignore = "audit H-91: event sequence must match concurrent enqueue order"]
    fn audit_concurrent_event_producers_keep_sequences_in_wire_order() {
        let (outbox, mut rx) = EventOutbox::channel(2);

        // Model a producer being descheduled after reserving its sequence but
        // before enqueueing; another producer can complete both steps first.
        let earlier = outbox.allocate_sequence();
        let later = outbox.allocate_sequence();
        outbox.enqueue(
            later,
            HarnessEvent::TurnStarted {
                thread_id: Some("audit-thread".into()),
                turn_id: "later-producer".into(),
                step: None,
            },
        );
        outbox.enqueue(
            earlier,
            HarnessEvent::TurnStarted {
                thread_id: Some("audit-thread".into()),
                turn_id: "earlier-producer".into(),
                step: None,
            },
        );

        let observed: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok())
            .map(|queued| queued.sequence)
            .collect();
        eprintln!("reserved=[{earlier},{later}] enqueued={observed:?}");
        assert_eq!(observed, vec![earlier, later]);
    }

    #[test]
    fn audit_interrupt_after_start_ack_reaches_cancellation() {
        let mut state = ServerState::new_for_test();
        initialize(&mut state);
        let thread =
            state.handle_request_for_test(rpc_request(2, "thread/start", serde_json::json!({})));
        let thread_id = thread.result()["threadId"].as_str().unwrap().to_owned();
        // Keep the launch pending to reproduce the interval between the ACK and
        // execution-side admission; use the production dispatcher for both RPCs.
        let (response, launch) = state.dispatch(
            rpc_request(
                3,
                "turn/start",
                serde_json::json!({ "threadId": thread_id, "input": "audit" }),
            ),
            None,
        );
        let launch = launch.expect("turn launch");
        let turn_id = response.result()["turnId"].as_str().unwrap();
        let interrupted = state.handle_request_for_test(rpc_request(
            4,
            "turn/interrupt",
            serde_json::json!({ "threadId": thread_id, "turnId": turn_id }),
        ));
        let active_registration = launch.chat.request_cancel(turn_id).unwrap().accepted;
        let durable_runs = RunStore::new(&state.database)
            .list_for_session(&thread_id)
            .unwrap();
        eprintln!(
            "audit interrupt: status={}, active_registration={active_registration}, durable_runs={}",
            interrupted.result()["status"],
            durable_runs.len()
        );
        assert_eq!(interrupted.result()["status"], "cancelling");
        assert!(
            active_registration,
            "interrupt acknowledged cancelling without an execution-side registration"
        );
        assert_eq!(durable_runs.len(), 1);
        assert_eq!(durable_runs[0].id, turn_id);
    }

    #[test]
    fn audit_resume_preserves_explicit_thread_overrides() {
        let mut state = ServerState::new_for_test();
        initialize(&mut state);
        let started = state.handle_request_for_test(rpc_request(
            2,
            "thread/start",
            serde_json::json!({
                "provider": "deepseek",
                "model": "deepseek-flash",
                "permissionProfile": "read_only",
                "sandboxBackend": "direct"
            }),
        ));
        let thread_id = started.result()["threadId"].as_str().unwrap().to_owned();
        let original = state.threads.get(&thread_id).unwrap().clone();
        let resumed = state.handle_request_for_test(rpc_request(
            3,
            "thread/resume",
            serde_json::json!({"threadId":thread_id}),
        ));
        assert_eq!(resumed.result()["status"], "ready");
        let (_, launch) = state.dispatch(
            rpc_request(
                4,
                "turn/start",
                serde_json::json!({"threadId":thread_id,"input":"read-only audit"}),
            ),
            None,
        );
        let actual = serde_json::to_value(launch.unwrap().overrides).unwrap();
        let expected = serde_json::json!({
            "provider": original.overrides.provider,
            "model": original.overrides.model,
            "permission_profile": original.overrides.permission_profile,
            "sandbox_backend": original.overrides.sandbox_backend
        });
        eprintln!("audit resume: before={expected}, after={actual}");
        assert_eq!(
            actual, expected,
            "thread/resume silently discarded explicit model and safety configuration"
        );
    }

    #[test]
    fn audit_steer_preserves_effective_turn_overrides() {
        let mut state = ServerState::new_for_test();
        initialize(&mut state);
        let started = state.handle_request_for_test(rpc_request(
            2,
            "thread/start",
            serde_json::json!({"provider":"deepseek", "model":"deepseek-flash",
                "permissionProfile":"read_only", "sandboxBackend":"direct"}),
        ));
        let thread_id = started.result()["threadId"].as_str().unwrap().to_owned();
        let (_, original) = state.dispatch(
            rpc_request(
                3,
                "turn/start",
                serde_json::json!({"threadId":thread_id, "input":"audit", "reasoningEffort":"high"}),
            ),
            None,
        );
        let original = original.unwrap();
        let expected = serde_json::to_value(&original.overrides).unwrap();
        let (response, replacement) = state.dispatch(
            rpc_request(
                4,
                "turn/steer",
                serde_json::json!({"threadId":thread_id, "turnId":original.turn_id,
                    "input":"Continue read-only with this correction."}),
            ),
            None,
        );
        assert_eq!(response.result()["status"], "started");
        let actual = serde_json::to_value(replacement.unwrap().overrides).unwrap();
        eprintln!("audit steer: before={expected}, after={actual}");
        assert_eq!(
            actual, expected,
            "steering silently reset effective turn configuration"
        );
    }

    #[test]
    fn audit_repeated_steer_does_not_launch_multiple_replacements() {
        let mut state = ServerState::new_for_test();
        initialize(&mut state);
        let started =
            state.handle_request_for_test(rpc_request(2, "thread/start", serde_json::json!({})));
        let thread_id = started.result()["threadId"].as_str().unwrap().to_owned();
        let (_, original) = state.dispatch(
            rpc_request(
                3,
                "turn/start",
                serde_json::json!({"threadId":thread_id, "input":"audit"}),
            ),
            None,
        );
        let original = original.unwrap();
        let (first_response, first_replacement) = state.dispatch(
            rpc_request(
                4,
                "turn/steer",
                serde_json::json!({"threadId":thread_id, "turnId":original.turn_id,
                    "input":"first correction"}),
            ),
            None,
        );
        let (second_response, second_replacement) = state.dispatch(
            rpc_request(
                5,
                "turn/steer",
                serde_json::json!({"threadId":thread_id, "turnId":original.turn_id,
                    "input":"duplicate correction"}),
            ),
            None,
        );
        let first_id = first_response.result()["turnId"].as_str();
        let second_id = second_response
            .result
            .as_ref()
            .and_then(|result| result["turnId"].as_str());
        eprintln!(
            "audit repeated steer: first_status={}, second_status={}, first_launch={}, second_launch={}, first_id={first_id:?}, second_id={second_id:?}",
            if first_response.is_success() { "success" } else { "error" },
            if second_response.is_success() { "success" } else { "error" },
            first_replacement.is_some(),
            second_replacement.is_some(),
        );
        assert!(
            first_replacement.is_some(),
            "first steer must create its replacement"
        );
        assert!(
            second_replacement.is_none(),
            "a stale replaced turn must not create a second replacement"
        );
        assert_eq!(second_response.error_code(), Some(ERR_INVALID_TURN));
    }

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
    #[ignore = "audit H-217: unknown namespaced RPC methods must use method-not-found"]
    fn audit_unknown_namespaced_method_is_not_reported_as_invalid_params() {
        let mut state = ServerState::new_for_test();
        initialize(&mut state);

        let namespaced = state.handle_request_for_test(rpc_request(
            2,
            "thread/does-not-exist",
            serde_json::json!({}),
        ));
        let plain =
            state.handle_request_for_test(rpc_request(3, "does-not-exist", serde_json::json!({})));
        let known_bad_params = state.handle_request_for_test(rpc_request(
            4,
            "thread/start",
            serde_json::json!({"cwd": 42}),
        ));
        eprintln!(
            "unknown-method audit: namespaced={:?} plain={:?} known_bad_params={:?}",
            namespaced.error_code(),
            plain.error_code(),
            known_bad_params.error_code()
        );

        assert_eq!(plain.error_code(), Some(ERR_METHOD_NOT_FOUND));
        assert_eq!(known_bad_params.error_code(), Some(ERR_INVALID_PARAMS));
        assert_eq!(
            namespaced.error_code(),
            Some(ERR_METHOD_NOT_FOUND),
            "an unknown method was classified from its slash instead of method lookup"
        );
    }

    #[test]
    fn event_ack_is_monotonic() {
        let mut state = ServerState::new_for_test();
        initialize(&mut state);
        state.output.delivered_sequence.store(4, Ordering::Release);
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
    fn audit_event_ack_rejects_unemitted_sequence() {
        let mut state = ServerState::new_for_test();
        initialize(&mut state);

        let response = state.handle_request_for_test(rpc_request(
            2,
            "event/ack",
            serde_json::json!({ "eventSequence": u64::MAX }),
        ));

        assert!(
            response.error_code().is_some(),
            "event/ack accepted a sequence that no notification emitted: response={response:?}"
        );
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
    fn thread_fork_source_cursor_reports_the_complete_projection_boundary() {
        let mut state = ServerState::new_for_test();
        initialize(&mut state);
        let started = state.handle_request_for_test(rpc_request(
            1,
            "thread/start",
            serde_json::json!({"cwd":state.workspace().to_string_lossy()}),
        ));
        let source = started.result()["threadId"].as_str().unwrap().to_owned();
        let clock = SystemClock;
        let anchor = Session::recover(&state.database, &clock, source.parse().unwrap())
            .unwrap()
            .append(deepagent_core::event::EventPayload::MessageAppended {
                message: deepagent_core::message::Message::user("branch source wire fixture"),
            })
            .unwrap()
            .sequence;
        let forked = state.handle_request_for_test(rpc_request(
            2,
            "thread/fork",
            serde_json::json!({"threadId":source,"atSequence":anchor}),
        ));
        assert!(forked.is_success(), "{forked:?}");
        assert_eq!(forked.result()["forkedAt"], anchor + 1);
        let child = forked.result()["threadId"].as_str().unwrap();
        let events = EventStore::new(&state.database)
            .load_session(child.parse().unwrap())
            .unwrap();
        assert_eq!(events.last().unwrap().sequence, anchor + 1);
        let read = state.handle_request_for_test(rpc_request(
            3,
            "thread/read",
            serde_json::json!({"threadId":child}),
        ));
        assert!(read.is_success());
        assert!(read.result()["events"]
            .to_string()
            .contains("branch source wire fixture"));
    }

    #[test]
    fn thread_resume_and_read_reopen_the_same_file_backed_session() {
        let temporary = tempfile::tempdir().unwrap();
        let workspace = temporary.path().join("workspace");
        std::fs::create_dir(&workspace).unwrap();
        let database_path = temporary.path().join("deepagent.db");
        let database = Arc::new(Database::open(&database_path).unwrap());
        let mut state = ServerState::new_for_test_with_database(database, workspace.clone());
        initialize(&mut state);

        let started = state.handle_request_for_test(rpc_request(
            1,
            "thread/start",
            serde_json::json!({ "cwd": workspace }),
        ));
        let thread_id = started.result()["threadId"].as_str().unwrap().to_owned();
        let clock = SystemClock;
        {
            let mut session =
                Session::recover(&state.database, &clock, thread_id.parse().unwrap()).unwrap();
            session
                .append(deepagent_core::event::EventPayload::MessageAppended {
                    message: deepagent_core::message::Message::user("reopen marker 90210"),
                })
                .unwrap();
        }
        let before = state.handle_request_for_test(rpc_request(
            2,
            "thread/read",
            serde_json::json!({ "threadId": thread_id }),
        ));
        let original_events = before.result()["events"].clone();
        assert!(original_events.as_array().unwrap().len() >= 2);
        assert!(original_events.to_string().contains("reopen marker 90210"));
        drop(state);

        let reopened = Arc::new(Database::open(&database_path).unwrap());
        let mut state = ServerState::new_for_test_with_database(reopened, workspace.clone());
        initialize(&mut state);
        let listed =
            state.handle_request_for_test(rpc_request(3, "thread/list", serde_json::json!({})));
        assert!(listed.result()["threads"]
            .as_array()
            .unwrap()
            .iter()
            .any(|thread| thread["threadId"] == thread_id));
        let resumed = state.handle_request_for_test(rpc_request(
            4,
            "thread/resume",
            serde_json::json!({ "threadId": thread_id }),
        ));
        assert_eq!(resumed.result()["status"], "ready");
        assert_eq!(resumed.result()["cwd"], before.result()["cwd"]);
        let after = state.handle_request_for_test(rpc_request(
            5,
            "thread/read",
            serde_json::json!({ "threadId": thread_id }),
        ));
        assert_eq!(after.result()["events"], original_events);
        let forked = state.handle_request_for_test(rpc_request(
            6,
            "thread/fork",
            serde_json::json!({ "threadId": thread_id }),
        ));
        assert_ne!(forked.result()["threadId"], thread_id);
    }

    #[test]
    #[ignore = "audit H-216: a successful cold thread fork must be ready for turn/start"]
    fn audit_cold_thread_fork_is_loaded_for_the_next_turn() {
        let temporary = tempfile::tempdir().unwrap();
        let workspace = temporary.path().join("workspace");
        std::fs::create_dir(&workspace).unwrap();
        let database_path = temporary.path().join("deepagent.db");

        let source_thread_id = {
            let database = Arc::new(Database::open(&database_path).unwrap());
            let mut original = ServerState::new_for_test_with_database(database, workspace.clone());
            initialize(&mut original);
            let started = original.handle_request_for_test(rpc_request(
                2,
                "thread/start",
                serde_json::json!({"cwd": workspace}),
            ));
            started.result()["threadId"].as_str().unwrap().to_owned()
        };

        let database = Arc::new(Database::open(&database_path).unwrap());
        let mut restarted = ServerState::new_for_test_with_database(database, workspace);
        initialize(&mut restarted);
        assert!(!restarted.threads.contains_key(&source_thread_id));

        let forked = restarted.handle_request_for_test(rpc_request(
            2,
            "thread/fork",
            serde_json::json!({"threadId": source_thread_id}),
        ));
        let forked_thread_id = forked.result()["threadId"]
            .as_str()
            .expect("successful fork returns a thread id")
            .to_owned();
        let fork_loaded = restarted.threads.contains_key(&forked_thread_id);
        let (turn, launch) = restarted.dispatch(
            rpc_request(
                3,
                "turn/start",
                serde_json::json!({"threadId": forked_thread_id, "input": "continue the fork"}),
            ),
            None,
        );
        let resumed = restarted.handle_request_for_test(rpc_request(
            4,
            "thread/resume",
            serde_json::json!({"threadId": forked_thread_id}),
        ));
        let (turn_after_resume, launch_after_resume) = restarted.dispatch(
            rpc_request(
                5,
                "turn/start",
                serde_json::json!({"threadId": forked_thread_id, "input": "continue after resume"}),
            ),
            None,
        );
        eprintln!(
            "cold-fork audit: fork_success={} fork_loaded={fork_loaded} turn_success={} launch={} resume_success={} resumed_turn_success={} resumed_launch={}",
            forked.is_success(),
            turn.is_success(),
            launch.is_some(),
            resumed.is_success(),
            turn_after_resume.is_success(),
            launch_after_resume.is_some()
        );

        assert!(forked.is_success());
        assert!(
            resumed.is_success() && turn_after_resume.is_success() && launch_after_resume.is_some(),
            "explicitly resuming the durable fork should make the positive control runnable"
        );
        assert!(
            fork_loaded && turn.is_success() && launch.is_some(),
            "thread/fork returned success but the new thread was not ready for turn/start"
        );
    }

    #[test]
    #[ignore = "audit H-215: an archived thread must not accept a new turn before explicit restore"]
    fn audit_archived_thread_does_not_accept_new_turn() {
        let mut state = ServerState::new_for_test();
        initialize(&mut state);
        let started =
            state.handle_request_for_test(rpc_request(2, "thread/start", serde_json::json!({})));
        let thread_id = started.result()["threadId"].as_str().unwrap().to_owned();

        let archived = state.handle_request_for_test(rpc_request(
            3,
            "thread/archive",
            serde_json::json!({"threadId": thread_id}),
        ));
        let durable_archived = ArchiveService::new(state.database.clone())
            .is_archived(&thread_id)
            .expect("read the durable archive marker");
        let listed =
            state.handle_request_for_test(rpc_request(4, "thread/list", serde_json::json!({})));
        let still_listed = listed.result()["threads"]
            .as_array()
            .expect("thread/list threads array")
            .iter()
            .any(|thread| thread["threadId"] == thread_id);

        let (turn, launch) = state.dispatch(
            rpc_request(
                5,
                "turn/start",
                serde_json::json!({"threadId": thread_id, "input": "must stay archived"}),
            ),
            None,
        );
        eprintln!(
            "archived-thread audit: archive_success={} durable_archived={durable_archived} still_listed={still_listed} turn_success={} launch={}",
            archived.is_success(),
            turn.is_success(),
            launch.is_some()
        );

        assert!(archived.is_success() && durable_archived);
        assert!(
            !still_listed,
            "thread/list kept returning a thread after thread/archive reported success"
        );
        assert!(
            !turn.is_success() && launch.is_none(),
            "turn/start created a runnable turn after the same connection successfully archived the thread"
        );
    }

    #[test]
    fn audit_thread_read_has_bounded_history_page() {
        let temporary = tempfile::tempdir().unwrap();
        let workspace = temporary.path().join("workspace");
        std::fs::create_dir(&workspace).unwrap();
        let database_path = temporary.path().join("deepagent.db");
        let database = Arc::new(Database::open(&database_path).unwrap());
        let mut state = ServerState::new_for_test_with_database(database, workspace);
        initialize(&mut state);
        let started =
            state.handle_request_for_test(rpc_request(2, "thread/start", serde_json::json!({})));
        let thread_id = started.result()["threadId"]
            .as_str()
            .expect("thread/start returns a thread id")
            .to_owned();

        let clock = SystemClock;
        let mut session = Session::recover(
            &state.database,
            &clock,
            thread_id.parse().expect("session id"),
        )
        .unwrap();
        for index in 0..600 {
            session
                .append(deepagent_core::event::EventPayload::MessageAppended {
                    message: deepagent_core::message::Message::user(format!(
                        "audit-history-{index}-{}",
                        "x".repeat(1024)
                    )),
                })
                .unwrap();
        }
        drop(session);

        let response = state.handle_request_for_test(rpc_request(
            3,
            "thread/read",
            serde_json::json!({"threadId": thread_id}),
        ));
        let events = response.result()["events"]
            .as_array()
            .expect("thread/read returns session events");
        let serialized_bytes = serde_json::to_vec(&response).unwrap().len();
        let has_continuation = response.result().get("nextCursor").is_some()
            || response.result().get("hasMore").is_some();
        eprintln!(
            "thread/read history audit: events={}, serialized_bytes={}, has_continuation={has_continuation}",
            events.len(),
            serialized_bytes
        );

        assert!(
            events.len() <= 500 && has_continuation,
            "thread/read materialized the entire history without a bounded page/cursor"
        );
    }

    fn seed_tool_list_snapshot(
        database: &Database,
        thread_id: &str,
        run_id: &str,
        snapshot_id: &str,
        context: serde_json::Value,
        tools: serde_json::Value,
    ) {
        let store = RunStore::new(database);
        store.create(run_id, thread_id, None, 1).unwrap();
        store
            .bind_capability_snapshot(
                run_id,
                &serde_json::json!({
                    "version": 1,
                    "runId": run_id,
                    "threadId": thread_id,
                    "snapshotId": snapshot_id,
                    "context": context,
                    "tools": tools,
                    "modelSchema": {"generation": 0, "schemaHash": format!("{snapshot_id}-schema")}
                }),
                2,
            )
            .unwrap();
    }

    #[test]
    fn tool_list_scopes_each_thread_to_its_latest_run_capability() {
        let temporary = tempfile::tempdir().unwrap();
        let workspace_a = temporary.path().join("workspace-a");
        let workspace_b = temporary.path().join("workspace-b");
        std::fs::create_dir(&workspace_a).unwrap();
        std::fs::create_dir(&workspace_b).unwrap();
        let database = Arc::new(Database::open_in_memory().unwrap());
        let mut state = ServerState::new_for_test_with_database(database, workspace_a.clone());
        initialize(&mut state);

        let first = state.handle_request_for_test(rpc_request(
            2,
            "thread/start",
            serde_json::json!({
                "cwd": workspace_a,
                "permissionProfile": "read_only",
                "sandboxBackend": "direct"
            }),
        ));
        let first_thread_id = first.result()["threadId"]
            .as_str()
            .expect("first thread id")
            .to_owned();
        let second = state.handle_request_for_test(rpc_request(
            4,
            "thread/start",
            serde_json::json!({
                "cwd": workspace_b,
                "permissionProfile": "full_access",
                "sandboxBackend": "windows_sandbox"
            }),
        ));
        let second_thread_id = second.result()["threadId"]
            .as_str()
            .expect("second thread id")
            .to_owned();

        seed_tool_list_snapshot(
            &state.database,
            &first_thread_id,
            "tool-list-read-only-run",
            "tool-list-read-only-snapshot",
            serde_json::json!({"workspace":workspace_a, "permissionProfile":"read_only", "sandboxBackend":"direct"}),
            serde_json::json!([{"name":"read_file", "initiallyVisible":true}]),
        );
        seed_tool_list_snapshot(
            &state.database,
            &second_thread_id,
            "tool-list-full-access-run",
            "tool-list-full-access-snapshot",
            serde_json::json!({"workspace":workspace_b, "permissionProfile":"full_access", "sandboxBackend":"windows_sandbox"}),
            serde_json::json!([
                {"name":"read_file", "initiallyVisible":true},
                {"name":"write_file", "initiallyVisible":true}
            ]),
        );

        let first_tools = state.handle_request_for_test(rpc_request(
            3,
            "tool/list",
            serde_json::json!({"threadId": first_thread_id}),
        ));
        let second_tools = state.handle_request_for_test(rpc_request(
            5,
            "tool/list",
            serde_json::json!({"threadId": second_thread_id}),
        ));
        eprintln!(
            "tool/list scoped capability audit: first_thread={first_thread_id}, second_thread={second_thread_id}, first_snapshot={}, second_snapshot={}",
            first_tools.result().get("snapshotId").and_then(serde_json::Value::as_str).unwrap_or("missing"),
            second_tools.result().get("snapshotId").and_then(serde_json::Value::as_str).unwrap_or("missing")
        );

        assert!(
            first_tools.is_success() && second_tools.is_success(),
            "thread-scoped tool/list failed: first={first_tools:?}, second={second_tools:?}"
        );
        assert_eq!(first_tools.result()["threadId"], first_thread_id);
        assert_eq!(first_tools.result()["runId"], "tool-list-read-only-run");
        assert_eq!(
            first_tools.result()["snapshotId"],
            "tool-list-read-only-snapshot"
        );
        assert_eq!(
            first_tools.result()["capabilitySnapshot"]["context"]["workspace"],
            workspace_a.to_string_lossy().as_ref()
        );
        assert_eq!(
            first_tools.result()["capabilitySnapshot"]["context"]["permissionProfile"],
            "read_only"
        );
        assert_eq!(
            first_tools.result()["capabilitySnapshot"]["context"]["sandboxBackend"],
            "direct"
        );
        assert_eq!(first_tools.result()["tools"].as_array().unwrap().len(), 1);

        assert_eq!(second_tools.result()["threadId"], second_thread_id);
        assert_eq!(second_tools.result()["runId"], "tool-list-full-access-run");
        assert_eq!(
            second_tools.result()["snapshotId"],
            "tool-list-full-access-snapshot"
        );
        assert_eq!(
            second_tools.result()["capabilitySnapshot"]["context"]["workspace"],
            workspace_b.to_string_lossy().as_ref()
        );
        assert_eq!(
            second_tools.result()["capabilitySnapshot"]["context"]["permissionProfile"],
            "full_access"
        );
        assert_eq!(
            second_tools.result()["capabilitySnapshot"]["context"]["sandboxBackend"],
            "windows_sandbox"
        );
        assert_eq!(second_tools.result()["tools"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn tool_list_returns_the_requested_run_snapshot_and_rejects_foreign_scope() {
        let mut state = ServerState::new_for_test();
        initialize(&mut state);

        let session = Session::create(&state.database, &SystemClock, None).unwrap();
        let thread_id = session.id().to_string();
        let run_id = "tool-list-scoped-run";
        RunStore::new(&state.database)
            .create(run_id, &thread_id, None, 1)
            .unwrap();
        let snapshot = serde_json::json!({
            "version": 1,
            "runId": run_id,
            "threadId": thread_id,
            "snapshotId": "tool-list-scoped-snapshot",
            "tools": [{"name": "read_file", "schemaHash": "synthetic-schema"}],
            "modelSchema": {"generation": 0, "schemaHash": "synthetic-model-schema"}
        });
        RunStore::new(&state.database)
            .bind_capability_snapshot(run_id, &snapshot, 2)
            .unwrap();
        RunStore::new(&state.database)
            .append_event(
                run_id,
                3,
                "running_turn",
                "ready",
                "capability_schema_snapshot",
                &serde_json::json!({
                    "version": 1,
                    "snapshotId": "tool-list-scoped-snapshot",
                    "modelSchema": {"generation": 1, "schemaHash": "latest-model-schema"}
                }),
            )
            .unwrap();
        RunStore::new(&state.database)
            .append_event(
                run_id,
                4,
                "running_turn",
                "ready",
                "capability_wire_snapshot",
                &serde_json::json!({
                    "version": 1,
                    "snapshotId": "tool-list-scoped-snapshot",
                    "wireSchema": {"generation": 2, "schemaHash": "latest-wire-schema"}
                }),
            )
            .unwrap();

        let scoped = state.handle_request_for_test(rpc_request(
            2,
            "tool/list",
            serde_json::json!({"threadId": thread_id, "runId": run_id}),
        ));
        assert!(scoped.is_success(), "scoped tool/list failed: {scoped:?}");
        assert_eq!(scoped.result()["threadId"], thread_id);
        assert_eq!(scoped.result()["runId"], run_id);
        assert_eq!(scoped.result()["snapshotId"], "tool-list-scoped-snapshot");
        assert_eq!(scoped.result()["capabilitySnapshot"]["threadId"], thread_id);
        assert_eq!(scoped.result()["tools"], snapshot["tools"]);
        assert_eq!(
            scoped.result()["capabilitySnapshot"]["modelSchema"]["generation"],
            1
        );
        assert_eq!(
            scoped.result()["capabilitySnapshot"]["modelSchema"]["schemaHash"],
            "latest-model-schema"
        );
        assert_eq!(
            scoped.result()["capabilitySnapshot"]["wireSchema"]["generation"],
            2
        );

        let latest_run_id = "tool-list-latest-run";
        RunStore::new(&state.database)
            .create(latest_run_id, &thread_id, None, 10)
            .unwrap();
        let latest_snapshot = serde_json::json!({
            "version": 1,
            "runId": latest_run_id,
            "threadId": thread_id,
            "snapshotId": "tool-list-latest-snapshot",
            "tools": [{"name": "write_file", "schemaHash": "other-schema"}],
            "modelSchema": {"generation": 0, "schemaHash": "latest-initial-schema"}
        });
        RunStore::new(&state.database)
            .bind_capability_snapshot(latest_run_id, &latest_snapshot, 11)
            .unwrap();
        let latest = state.handle_request_for_test(rpc_request(
            3,
            "tool/list",
            serde_json::json!({"threadId": thread_id}),
        ));
        assert!(
            latest.is_success(),
            "thread-scoped lookup failed: {latest:?}"
        );
        assert_eq!(latest.result()["runId"], latest_run_id);
        assert_eq!(latest.result()["snapshotId"], "tool-list-latest-snapshot");

        let unbound_run_id = "tool-list-unbound-run";
        RunStore::new(&state.database)
            .create(unbound_run_id, &thread_id, None, 12)
            .unwrap();
        let unbound = state.handle_request_for_test(rpc_request(
            4,
            "tool/list",
            serde_json::json!({"threadId": thread_id, "runId": unbound_run_id}),
        ));
        assert_eq!(unbound.error_code(), Some(ERR_INVALID_TURN));

        let foreign_thread_id = Session::create(&state.database, &SystemClock, None)
            .unwrap()
            .id()
            .to_string();
        let foreign = state.handle_request_for_test(rpc_request(
            5,
            "tool/list",
            serde_json::json!({"threadId": foreign_thread_id, "runId": run_id}),
        ));
        assert_eq!(foreign.error_code(), Some(ERR_INVALID_THREAD));

        let missing = state.handle_request_for_test(rpc_request(
            6,
            "tool/list",
            serde_json::json!({"threadId": thread_id, "runId": "missing-run"}),
        ));
        assert_eq!(missing.error_code(), Some(ERR_INVALID_TURN));
    }

    #[test]
    fn tool_list_without_scope_remains_a_global_catalog() {
        let mut state = ServerState::new_for_test();
        initialize(&mut state);

        let global =
            state.handle_request_for_test(rpc_request(2, "tool/list", serde_json::json!({})));
        assert!(global.is_success());
        assert!(global.result()["tools"]
            .as_array()
            .is_some_and(|tools| !tools.is_empty()));
        assert!(global.result().get("snapshotId").is_none());
    }

    #[test]
    fn h219_tool_list_rejects_stale_snapshot_and_latest_ties_are_deterministic() {
        let workspace = tempfile::tempdir().unwrap();
        let db = Arc::new(Database::open_in_memory().unwrap());
        let mut state = ServerState::new_for_test_with_database(db, workspace.path().to_owned());
        initialize(&mut state);
        let thread =
            state.handle_request_for_test(rpc_request(2, "thread/start", serde_json::json!({})));
        let thread = thread.result()["threadId"].as_str().unwrap();
        seed_tool_list_snapshot(
            &state.database,
            thread,
            "old-run",
            "old-snapshot",
            serde_json::json!({}),
            serde_json::json!([]),
        );
        let stale = state.handle_request_for_test(rpc_request(
            3,
            "tool/list",
            serde_json::json!({
                "threadId":thread, "expectedSnapshotId":"unknown-snapshot"
            }),
        ));
        assert!(
            !stale.is_success(),
            "stale snapshot expectations must not silently succeed"
        );
        seed_tool_list_snapshot(
            &state.database,
            thread,
            "new-run",
            "new-snapshot",
            serde_json::json!({}),
            serde_json::json!([]),
        );
        let latest = state.handle_request_for_test(rpc_request(
            4,
            "tool/list",
            serde_json::json!({"threadId":thread}),
        ));
        assert_eq!(
            latest.result()["runId"],
            "new-run",
            "same-millisecond admissions still have a deterministic latest run"
        );
        let stale = state.handle_request_for_test(rpc_request(
            5,
            "tool/list",
            serde_json::json!({
                "threadId":thread, "expectedSnapshotId":"old-snapshot"
            }),
        ));
        assert!(!stale.is_success());
        let historical = state.handle_request_for_test(rpc_request(
            6,
            "tool/list",
            serde_json::json!({
                "threadId":thread, "runId":"old-run", "expectedSnapshotId":"old-snapshot"
            }),
        ));
        assert_eq!(historical.result()["snapshotId"], "old-snapshot");
        let unscoped = state.handle_request_for_test(rpc_request(
            7,
            "tool/list",
            serde_json::json!({
                "expectedSnapshotId":"old-snapshot"
            }),
        ));
        assert!(!unscoped.is_success());
        assert_eq!(
            RunStore::new(&state.database)
                .list_for_session(thread)
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn h219_turn_start_binds_real_capabilities_before_ack() {
        let workspace = tempfile::tempdir().unwrap();
        let database = Arc::new(Database::open_in_memory().unwrap());
        let mut state =
            ServerState::new_for_test_with_database(database, workspace.path().to_path_buf());
        initialize(&mut state);
        let thread = state.handle_request_for_test(rpc_request(
            2,
            "thread/start",
            serde_json::json!({"permissionProfile":"read_only", "sandboxBackend":"direct"}),
        ));
        assert!(thread.is_success(), "{thread:?}");
        let thread_id = thread.result()["threadId"].as_str().unwrap();
        let (response, launch) = state.dispatch(
            rpc_request(
                3,
                "turn/start",
                serde_json::json!({"threadId":thread_id,"input":"synthetic capability check"}),
            ),
            None,
        );
        assert!(response.is_success(), "{response:?}");
        let launch = launch.unwrap();
        let snapshot = RunStore::new(&state.database)
            .capability_snapshot(&launch.turn_id)
            .unwrap()
            .expect("ACK must follow atomic admission of the real prepared capability snapshot");
        assert_eq!(response.result()["snapshotId"], snapshot["snapshotId"]);
        assert_eq!(snapshot["threadId"], thread_id);
        assert_eq!(snapshot["runId"], launch.turn_id);
        assert_eq!(snapshot["context"]["sandboxMode"], "read_only");
        assert_eq!(snapshot["context"]["sandboxBackend"], "direct");
        let tools = snapshot["tools"].as_array().unwrap();
        for name in ["read_file", "task", "plan_execute", "snip_history"] {
            assert!(
                tools.iter().any(|tool| tool["name"] == name),
                "missing real main tool {name}"
            );
        }
        assert!(response.result()["capabilitySnapshot"].is_object());
        assert!(!workspace.path().join(".deepagent").exists());
    }

    #[test]
    fn h219_failed_prepared_snapshot_admits_nothing_and_does_not_cancel_predecessor() {
        let workspace = tempfile::tempdir().unwrap();
        let database = Arc::new(Database::open_in_memory().unwrap());
        let mut state =
            ServerState::new_for_test_with_database(database, workspace.path().to_path_buf());
        initialize(&mut state);
        let thread =
            state.handle_request_for_test(rpc_request(2, "thread/start", serde_json::json!({})));
        let thread_id = thread.result()["threadId"].as_str().unwrap().to_owned();
        let fail = || {
            state.database.with_conn(|conn| conn.execute_batch(
            "CREATE TRIGGER reject_h219_snapshot BEFORE INSERT ON run_events WHEN NEW.event_type='capability.snapshot' BEGIN SELECT RAISE(ABORT,'synthetic admission snapshot failure'); END;"
        ).map_err(|error| CoreError::Persistence(error.to_string()))).unwrap()
        };
        fail();
        let request = || {
            rpc_request(
                3,
                "turn/start",
                serde_json::json!({"threadId":thread_id,"input":"synthetic"}),
            )
        };
        let (failed, launch) = state.dispatch(request(), None);
        assert_eq!(failed.error_code(), Some(ERR_INTERNAL));
        assert!(launch.is_none());
        assert!(state.turns.is_empty());
        assert!(RunStore::new(&state.database)
            .list_for_session(&thread_id)
            .unwrap()
            .is_empty());
        state
            .database
            .with_conn(|conn| {
                conn.execute_batch("DROP TRIGGER reject_h219_snapshot")
                    .map_err(|error| CoreError::Persistence(error.to_string()))
            })
            .unwrap();
        let (started, original) = state.dispatch(request(), None);
        assert!(started.is_success(), "{started:?}");
        let original = original.unwrap();
        let registration = original.chat.database();
        // Inject after the original is fully admitted, before steering's own
        // snapshot transaction. Failure must leave its sole replacement free.
        registration.with_conn(|conn| conn.execute_batch(
            "CREATE TRIGGER reject_h219_snapshot BEFORE INSERT ON run_events WHEN NEW.event_type='capability.snapshot' BEGIN SELECT RAISE(ABORT,'synthetic admission snapshot failure'); END;"
        ).map_err(|error| CoreError::Persistence(error.to_string()))).unwrap();
        let steer = || {
            rpc_request(
                4,
                "turn/steer",
                serde_json::json!({"threadId":thread_id,"turnId":original.turn_id,"input":"replacement"}),
            )
        };
        let (failed, launch) = state.dispatch(steer(), None);
        assert_eq!(failed.error_code(), Some(ERR_INTERNAL));
        assert!(launch.is_none());
        assert_eq!(state.turns.len(), 1);
        assert_eq!(
            RunStore::new(&state.database)
                .list_for_session(&thread_id)
                .unwrap()
                .len(),
            1
        );
        assert!(!RunStore::new(&state.database)
            .events_after(&original.turn_id, None)
            .unwrap()
            .iter()
            .any(|event| event.event_type == "continuation.created"));
        registration
            .with_conn(|conn| {
                conn.execute_batch("DROP TRIGGER reject_h219_snapshot")
                    .map_err(|error| CoreError::Persistence(error.to_string()))
            })
            .unwrap();
        let (accepted, replacement) = state.dispatch(steer(), None);
        assert!(accepted.is_success(), "{accepted:?}");
        let replacement = replacement.unwrap();
        assert_ne!(original.turn_id, replacement.turn_id);
        assert_eq!(
            RunStore::new(&state.database)
                .list_for_session(&thread_id)
                .unwrap()
                .len(),
            2
        );
        assert_eq!(
            RunStore::new(&state.database)
                .events_after(&original.turn_id, None)
                .unwrap()
                .iter()
                .filter(|event| event.event_type == "continuation.created")
                .count(),
            1
        );
    }

    #[test]
    fn h219_unknown_or_unavailable_backend_rejects_before_admission() {
        for backend in ["per-turn-backend", "windows_sandbox"] {
            let workspace = tempfile::tempdir().unwrap();
            let database = Arc::new(Database::open_in_memory().unwrap());
            let mut state =
                ServerState::new_for_test_with_database(database, workspace.path().to_path_buf());
            initialize(&mut state);
            let thread = state.handle_request_for_test(rpc_request(
                2,
                "thread/start",
                serde_json::json!({"sandboxBackend":backend}),
            ));
            assert!(thread.is_success(), "{thread:?}");
            let thread_id = thread.result()["threadId"].as_str().unwrap();
            let (response, launch) = state.dispatch(
                rpc_request(
                    3,
                    "turn/start",
                    serde_json::json!({"threadId":thread_id,"input":"synthetic"}),
                ),
                None,
            );
            assert_eq!(
                response.error_code(),
                Some(ERR_INTERNAL),
                "{backend}: {response:?}"
            );
            assert!(response.result.is_none());
            assert!(launch.is_none());
            assert!(state.turns.is_empty());
            assert!(RunStore::new(&state.database)
                .list_for_session(thread_id)
                .unwrap()
                .is_empty());
            assert_eq!(
                state.threads[thread_id]
                    .overrides
                    .sandbox_backend
                    .as_deref(),
                Some(backend)
            );
        }
    }

    #[tokio::test]
    async fn h219_builtin_control_does_not_advertise_or_require_model_tools() {
        let workspace = tempfile::tempdir().unwrap();
        let db = Arc::new(Database::open_in_memory().unwrap());
        let mut state =
            ServerState::new_for_test_with_database(db.clone(), workspace.path().to_owned());
        initialize(&mut state);
        let thread = state.handle_request_for_test(rpc_request(
            2,
            "thread/start",
            serde_json::json!({"sandboxBackend":"unavailable-model-backend"}),
        ));
        let thread_id = thread.result()["threadId"].as_str().unwrap().to_owned();
        let (response, launch) = state.dispatch(
            rpc_request(
                3,
                "turn/start",
                serde_json::json!({"threadId":thread_id,"input":"/help"}),
            ),
            None,
        );
        assert!(
            response.is_success(),
            "a builtin control must not resolve a model backend: {response:?}"
        );
        let launch = launch.unwrap();
        let snapshot = response.result()["capabilitySnapshot"].clone();
        assert_eq!(snapshot["context"]["executionDomain"], "builtin_control");
        assert_eq!(snapshot["tools"], serde_json::json!([]));
        assert_eq!(
            RunStore::new(&db)
                .capability_snapshot(&launch.turn_id)
                .unwrap()
                .unwrap(),
            snapshot
        );
        let result = launch
            .chat
            .run_prepared(launch.prepared, &launch.input, |_| {}, |_| {})
            .await
            .unwrap();
        assert_eq!(result, thread_id);
        let events = RunStore::new(&db)
            .events_after(&launch.turn_id, None)
            .unwrap();
        assert_eq!(
            events
                .iter()
                .filter(|event| event.event_type == "run_terminal")
                .count(),
            1
        );
        assert!(!events.iter().any(|event| matches!(
            event.event_type.as_str(),
            "capability_wire_snapshot" | "tool_started"
        )));
        assert_eq!(
            RunStore::new(&db)
                .get(&launch.turn_id)
                .unwrap()
                .unwrap()
                .terminal_kind
                .as_deref(),
            Some("succeeded")
        );
    }

    #[test]
    fn h219_cross_thread_resume_control_is_rejected_before_any_admission() {
        let workspace = tempfile::tempdir().unwrap();
        let db = Arc::new(Database::open_in_memory().unwrap());
        let mut state =
            ServerState::new_for_test_with_database(db.clone(), workspace.path().to_owned());
        initialize(&mut state);
        let first =
            state.handle_request_for_test(rpc_request(2, "thread/start", serde_json::json!({})));
        let second =
            state.handle_request_for_test(rpc_request(3, "thread/start", serde_json::json!({})));
        let first = first.result()["threadId"].as_str().unwrap();
        let second = second.result()["threadId"].as_str().unwrap();
        let (response, launch) = state.dispatch(
            rpc_request(
                4,
                "turn/start",
                serde_json::json!({"threadId":first,"input":format!("/resume {second}")}),
            ),
            None,
        );
        assert!(
            !response.is_success(),
            "a control cannot retarget the admitted owner: {response:?}"
        );
        assert!(launch.is_none());
        assert!(state.turns.is_empty());
        for thread in [first, second] {
            assert!(RunStore::new(&db)
                .list_for_session(thread)
                .unwrap()
                .is_empty());
        }
    }

    #[test]
    #[ignore = "audit H-220: thread/list must return a bounded page with an opaque continuation"]
    fn audit_thread_list_has_bounded_page() {
        let temporary = tempfile::tempdir().unwrap();
        let workspace = temporary.path().join("workspace");
        std::fs::create_dir(&workspace).unwrap();
        let database = Arc::new(Database::open_in_memory().unwrap());
        let mut state = ServerState::new_for_test_with_database(database, workspace.clone());
        initialize(&mut state);

        for index in 0..120 {
            let response = state.handle_request_for_test(rpc_request(
                index + 2,
                "thread/start",
                serde_json::json!({"cwd": workspace}),
            ));
            assert!(response.is_success(), "thread/start {index} failed");
        }
        let listed =
            state.handle_request_for_test(rpc_request(200, "thread/list", serde_json::json!({})));
        let threads = listed.result()["threads"]
            .as_array()
            .expect("thread/list returns threads");
        let serialized_bytes = serde_json::to_vec(&listed).unwrap().len();
        let has_continuation =
            listed.result().get("nextCursor").is_some() || listed.result().get("hasMore").is_some();
        eprintln!(
            "thread/list audit: threads={}, serialized_bytes={}, has_continuation={has_continuation}",
            threads.len(),
            serialized_bytes
        );

        assert!(
            threads.len() <= 100 && has_continuation,
            "thread/list materialized all sessions without a bounded page/cursor"
        );
    }

    #[test]
    fn audit_config_read_is_not_a_static_protocol_banner() {
        let temporary = tempfile::tempdir().unwrap();
        let workspace = temporary.path().join("workspace");
        std::fs::create_dir(&workspace).unwrap();
        let database = Arc::new(Database::open_in_memory().unwrap());
        let mut state = ServerState::new_for_test_with_database(database, workspace.clone());
        initialize(&mut state);

        let started = state.handle_request_for_test(rpc_request(
            2,
            "thread/start",
            serde_json::json!({
                "cwd": workspace,
                "provider": "deepseek-official",
                "model": "deepseek-v4-pro",
                "reasoningEffort": "high",
                "permissionProfile": "read_only",
                "sandboxBackend": "windows_sandbox"
            }),
        ));
        assert!(started.is_success());
        let config =
            state.handle_request_for_test(rpc_request(3, "config/read", serde_json::json!({})));
        let result = config.result().clone();
        eprintln!(
            "config/read audit: keys={:?}, provider={:?}, model={:?}, reasoning={:?}, permission={:?}, sandbox={:?}",
            result.as_object().map(|object| object.keys().cloned().collect::<Vec<_>>()),
            result.get("provider"),
            result.get("model"),
            result.get("reasoningEffort"),
            result.get("permissionProfile"),
            result.get("sandboxBackend")
        );

        assert!(
            result.get("model").is_some()
                && result.get("reasoningEffort").is_some()
                && result.get("permissionProfile").is_some()
                && result.get("sandboxBackend").is_some(),
            "config/read returned only static protocol metadata and no effective execution configuration"
        );
    }

    #[test]
    #[ignore = "audit H-222: thread/start reasoning effort must reach the next turn"]
    fn audit_thread_start_reasoning_effort_is_not_silently_dropped() {
        let temporary = tempfile::tempdir().unwrap();
        let workspace = temporary.path().join("workspace");
        std::fs::create_dir(&workspace).unwrap();
        let database = Arc::new(Database::open_in_memory().unwrap());
        let mut state = ServerState::new_for_test_with_database(database, workspace.clone());
        initialize(&mut state);

        let (started, _) = state.dispatch(
            rpc_request(
                2,
                "thread/start",
                serde_json::json!({
                    "cwd": workspace,
                    "provider": "deepseek-official",
                    "model": "deepseek-v4-pro",
                    "reasoningEffort": "high"
                }),
            ),
            None,
        );
        let thread_id = started.result()["threadId"]
            .as_str()
            .expect("thread/start returns threadId")
            .to_owned();
        let (turn, launch) = state.dispatch(
            rpc_request(
                3,
                "turn/start",
                serde_json::json!({
                    "threadId": thread_id,
                    "input": "audit reasoning override"
                }),
            ),
            None,
        );
        let reasoning_effort = launch
            .as_ref()
            .and_then(|launch| launch.overrides.reasoning_effort.as_deref());
        eprintln!(
            "thread/start reasoning audit: response_success={}, reasoning_effort={reasoning_effort:?}",
            turn.is_success()
        );
        assert_eq!(
            reasoning_effort,
            Some("high"),
            "thread/start reasoningEffort was accepted but dropped before turn/start"
        );
    }

    #[test]
    fn fileless_legacy_row_is_not_resumable_or_readable() {
        let mut state = ServerState::new_for_test();
        initialize(&mut state);
        let legacy_id = deepagent_core::id::SessionId::new().to_string();
        state
            .database
            .with_conn(|connection| {
                connection
                    .execute(
                        "INSERT INTO sessions
                         (id, title, mode, project, created_at, updated_at, ended_at)
                         VALUES (?1, 'legacy', 'normal', NULL, 1, 1, NULL)",
                        [&legacy_id],
                    )
                    .unwrap();
                Ok(())
            })
            .unwrap();

        let listed =
            state.handle_request_for_test(rpc_request(1, "thread/list", serde_json::json!({})));
        assert!(listed.result()["threads"].as_array().unwrap().is_empty());
        for method in ["thread/resume", "thread/read"] {
            let response = state.handle_request_for_test(rpc_request(
                2,
                method,
                serde_json::json!({ "threadId": legacy_id }),
            ));
            assert_eq!(response.error_code(), Some(ERR_INVALID_THREAD), "{method}");
        }
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

    #[test]
    fn h211_server_preserves_the_received_request_id_type() {
        let workspace = tempfile::tempdir().unwrap();
        for id in [
            serde_json::json!(1),
            serde_json::json!("1"),
            serde_json::json!("request-local"),
            serde_json::json!(0),
            serde_json::json!(-1),
            serde_json::json!(u64::MAX),
        ] {
            let database = Arc::new(Database::open_in_memory().unwrap());
            let mut state =
                ServerState::new_for_test_with_database(database, workspace.path().to_owned());
            let request: RpcRequest = serde_json::from_str(
                &serde_json::json!({
                    "jsonrpc": "2.0", "id": id, "method": "initialize",
                    "params": {"clientName":"h211", "clientVersion":"1", "protocolVersion":PROTOCOL_VERSION}
                })
                .to_string(),
            )
            .unwrap();
            let response = state.handle_request_for_test(request);
            assert!(response.is_success());
            assert_eq!(response.id, id);
            let error = state.handle_request_for_test(RpcRequest {
                jsonrpc: "2.0".into(),
                id: id.clone(),
                method: "h211/unknown".into(),
                params: serde_json::json!({}),
            });
            assert!(!error.is_success());
            assert_eq!(error.id, id, "error responses preserve identity too");
        }
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
