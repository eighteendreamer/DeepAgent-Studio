//! Client-side bindings for the DeepAgent Harness stdio protocol.
//!
//! [`HarnessClient`] owns the write half of a spawned `deepagent-cli server`
//! process and a background reader that matches JSON-RPC responses to their
//! requests while streaming harness events to subscribers. It is a thin
//! transport adapter on top of the protocol DTOs in
//! `deepagent-harness-protocol` — it does not re-implement execution,
//! persistence, approvals, or a second event store (AGENTS.md §5.1).

use std::{
    collections::HashMap,
    path::Path,
    sync::{
        atomic::{AtomicI64, Ordering},
        Arc, Mutex,
    },
};

use deepagent_harness_protocol::{
    ApprovalRespondRequest, ConfigReadRequest, EventAckRequest, HarnessEvent, HarnessRequest,
    InitializeRequest, RpcResponse, SandboxStatusRequest, ThreadArchiveRequest, ThreadForkRequest,
    ThreadListRequest, ThreadReadRequest, ThreadStartRequest, ToolListRequest,
    TurnInterruptRequest, TurnStartRequest, TurnSteerRequest, PROTOCOL_VERSION,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStderr, ChildStdin, ChildStdout},
    sync::{broadcast, oneshot},
};
use tracing::debug;

use crate::error::SdkError;
use crate::transport::{decode_line, request_line, WireMessage};

/// Shared pending-reply table keyed by request id.
type Pending = Arc<Mutex<HashMap<i64, oneshot::Sender<RpcResponse>>>>;

/// A running `deepagent-cli server` process plus its client handle.
///
/// [`HarnessProcess::wait`] is the graceful shutdown path: it closes the
/// server's stdin (the server exits on EOF), then waits for the child. Dropping
/// the [`HarnessProcess`] without calling [`HarnessProcess::wait`] leaves the
/// server running (tokio `Child` drops do not kill by default), so callers that
/// own the process must finish with `wait`.
pub struct HarnessProcess {
    child: Child,
    client: HarnessClient,
}

/// Cloneable request/event client for one harness server process.
///
/// Requests serialize against a shared in-flight table in the background
/// reader; events are broadcast to receivers from [`HarnessClient::subscribe`].
#[derive(Clone)]
pub struct HarnessClient {
    // `None` once the write half is closed via [`HarnessClient::close`]: every
    // clone shares the same handle, so a single explicit close signals EOF to
    // the server regardless of how many clones are still alive.
    stdin: Arc<tokio::sync::Mutex<Option<ChildStdin>>>,
    next_id: Arc<AtomicI64>,
    pending: Pending,
    events: broadcast::Sender<HarnessEvent>,
    initialized: Arc<Mutex<bool>>,
}

impl HarnessProcess {
    /// Spawn `deepagent-cli server --transport stdio` rooted at `workspace`.
    pub async fn spawn(bin: &Path, workspace: &Path) -> Result<Self, SdkError> {
        let mut child = tokio::process::Command::new(bin)
            .arg("server")
            .arg("--transport")
            .arg("stdio")
            .current_dir(workspace)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(SdkError::Spawn)?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| SdkError::Write(std::io::Error::from(std::io::ErrorKind::BrokenPipe)))?;
        let stdout = child.stdout.take().ok_or_else(|| {
            SdkError::Read(std::io::Error::from(std::io::ErrorKind::UnexpectedEof))
        })?;
        let stderr = child.stderr.take().ok_or_else(|| {
            SdkError::Read(std::io::Error::from(std::io::ErrorKind::UnexpectedEof))
        })?;

        let (events, _) = broadcast::channel(256);
        let stdin = Arc::new(tokio::sync::Mutex::new(Some(stdin)));
        let client = HarnessClient {
            stdin: stdin.clone(),
            next_id: Arc::new(AtomicI64::new(1)),
            pending: Arc::new(Mutex::new(HashMap::new())),
            events,
            initialized: Arc::new(Mutex::new(false)),
        };

        spawn_reader(
            stdout,
            stderr,
            client.pending.clone(),
            client.events.clone(),
        );

        Ok(Self { child, client })
    }

    /// The client handle for this process.
    pub fn client(&self) -> HarnessClient {
        self.client.clone()
    }

    /// Close the server's stdin and wait for it to exit.
    ///
    /// The server loops on stdin and exits cleanly when it hits EOF, and the
    /// only reliable EOF signal is dropping the write handle itself
    /// (`AsyncWrite::shutdown` is a no-op for the Windows stdio pipe), so this
    /// takes the handle out of every shared client via [`HarnessClient::close`]
    /// before waiting.
    pub async fn wait(mut self) -> Result<std::process::ExitStatus, std::io::Error> {
        self.client.close().await;
        self.child.wait().await
    }
}

impl HarnessClient {
    /// Handshake: announce the client and validate the protocol version.
    pub async fn initialize(
        &self,
        client_name: &str,
        client_version: &str,
    ) -> Result<RpcResponse, SdkError> {
        let response = self
            .call_unchecked(HarnessRequest::Initialize(InitializeRequest {
                client_name: client_name.to_string(),
                client_version: client_version.to_string(),
                protocol_version: PROTOCOL_VERSION,
            }))
            .await?;
        let mut initialized = self
            .initialized
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *initialized = true;
        Ok(response)
    }

    /// Create a new thread (session) in a workspace.
    pub async fn thread_start(&self, request: ThreadStartRequest) -> Result<RpcResponse, SdkError> {
        self.call(HarnessRequest::ThreadStart(request)).await
    }

    /// Resume an existing thread.
    pub async fn thread_resume(
        &self,
        request: deepagent_harness_protocol::ThreadResumeRequest,
    ) -> Result<RpcResponse, SdkError> {
        self.call(HarnessRequest::ThreadResume(request)).await
    }

    /// List persisted threads.
    pub async fn thread_list(&self) -> Result<RpcResponse, SdkError> {
        self.call(HarnessRequest::ThreadList(ThreadListRequest::default()))
            .await
    }

    /// Read thread state, optionally incrementally from cursors.
    pub async fn thread_read(&self, request: ThreadReadRequest) -> Result<RpcResponse, SdkError> {
        self.call(HarnessRequest::ThreadRead(request)).await
    }

    /// Fork a thread at a sequence point.
    pub async fn thread_fork(&self, request: ThreadForkRequest) -> Result<RpcResponse, SdkError> {
        self.call(HarnessRequest::ThreadFork(request)).await
    }

    /// Archive a thread.
    pub async fn thread_archive(
        &self,
        request: ThreadArchiveRequest,
    ) -> Result<RpcResponse, SdkError> {
        self.call(HarnessRequest::ThreadArchive(request)).await
    }

    /// Start a turn (may invoke the model).
    pub async fn turn_start(&self, request: TurnStartRequest) -> Result<RpcResponse, SdkError> {
        self.call(HarnessRequest::TurnStart(request)).await
    }

    /// Interrupt a running turn.
    pub async fn turn_interrupt(
        &self,
        request: TurnInterruptRequest,
    ) -> Result<RpcResponse, SdkError> {
        self.call(HarnessRequest::TurnInterrupt(request)).await
    }

    /// Steer a running turn.
    pub async fn turn_steer(&self, request: TurnSteerRequest) -> Result<RpcResponse, SdkError> {
        self.call(HarnessRequest::TurnSteer(request)).await
    }

    /// Respond to a pending approval request.
    pub async fn approval_respond(
        &self,
        request: ApprovalRespondRequest,
    ) -> Result<RpcResponse, SdkError> {
        self.call(HarnessRequest::ApprovalRespond(request)).await
    }

    /// Acknowledge events consumed up to `event_sequence`.
    pub async fn event_ack(&self, event_sequence: u64) -> Result<RpcResponse, SdkError> {
        self.call(HarnessRequest::EventAck(EventAckRequest { event_sequence }))
            .await
    }

    /// List tools visible to the server.
    pub async fn tool_list(&self) -> Result<RpcResponse, SdkError> {
        self.call(HarnessRequest::ToolList(ToolListRequest::default()))
            .await
    }

    /// Read effective configuration.
    pub async fn config_read(&self) -> Result<RpcResponse, SdkError> {
        self.call(HarnessRequest::ConfigRead(ConfigReadRequest::default()))
            .await
    }

    /// Query sandbox status.
    pub async fn sandbox_status(&self) -> Result<RpcResponse, SdkError> {
        self.call(HarnessRequest::SandboxStatus(
            SandboxStatusRequest::default(),
        ))
        .await
    }

    /// Subscribe to the broadcast stream of harness events.
    pub fn subscribe(&self) -> broadcast::Receiver<HarnessEvent> {
        self.events.subscribe()
    }

    /// Close the server's stdin, signalling EOF so the server exits.
    ///
    /// The handle is shared by every [`HarnessClient`] clone, so one call closes
    /// it for all of them. Drops the [`ChildStdin`] itself — the only reliable
    /// way to close the Windows pipe's write end (`AsyncWrite::shutdown` is a
    /// no-op for stdio on that platform). Idempotent: calling it twice, or after
    /// [`HarnessProcess::wait`], is a no-op.
    pub async fn close(&self) {
        let mut guard = self.stdin.lock().await;
        drop(guard.take());
    }

    async fn require_initialized(&self) -> Result<(), SdkError> {
        let initialized = *self
            .initialized
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if initialized {
            Ok(())
        } else {
            Err(SdkError::NotInitialized)
        }
    }

    /// Serialize one [`HarnessRequest`], emit it on stdin, and await its
    /// matching response from the background reader. Requires [`initialize`]
    /// to have completed first.
    async fn call(&self, request: HarnessRequest) -> Result<RpcResponse, SdkError> {
        self.require_initialized().await?;
        self.call_unchecked(request).await
    }

    /// Full send path without the initialized guard (used by the handshake).
    async fn call_unchecked(&self, request: HarnessRequest) -> Result<RpcResponse, SdkError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);

        let (method, params) = split_request(&request)?;
        let line = request_line(id, &method, &params)?;

        // Register the in-flight reply BEFORE writing: the server can answer so
        // fast that the reader would otherwise find no pending entry.
        let (reply, rx) = oneshot::channel();
        self.pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(id, reply);

        {
            let mut guard = self.stdin.lock().await;
            let stdin = guard.as_mut().ok_or_else(|| {
                SdkError::Write(std::io::Error::from(std::io::ErrorKind::BrokenPipe))
            })?;
            if let Err(error) = stdin.write_all(line.as_bytes()).await {
                self.pending
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .remove(&id);
                return Err(SdkError::Write(error));
            }
            stdin.write_all(b"\n").await.map_err(SdkError::Write)?;
            stdin.flush().await.map_err(SdkError::Write)?;
        }

        let response = rx
            .await
            .map_err(|_| SdkError::ResponseChannelClosed { id })?;

        if let Some(error) = response.error {
            Err(SdkError::Rpc {
                code: error.code,
                message: error.message,
            })
        } else {
            Ok(response)
        }
    }
}

/// Split a [`HarnessRequest`] back into its stable `method` and `params`.
fn split_request(request: &HarnessRequest) -> Result<(String, serde_json::Value), SdkError> {
    let value = serde_json::to_value(request).map_err(SdkError::Encode)?;
    let missing_method = || {
        SdkError::Encode(serde_json::Error::io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "request lacks method",
        )))
    };
    let method = value
        .get("method")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(missing_method)?;
    let params = value
        .get("params")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    Ok((method.to_string(), params))
}

fn spawn_reader(
    stdout: ChildStdout,
    stderr: ChildStderr,
    pending: Pending,
    events: broadcast::Sender<HarnessEvent>,
) {
    // Drain stderr so a verbose server cannot block on a full pipe.
    tokio::spawn(async move {
        let mut reader = BufReader::new(stderr);
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line).await {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    debug!(target: "harness_sdk::stderr", line = %line.trim(), "server stderr")
                }
            }
        }
    });

    tokio::spawn(async move {
        let mut lines = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            match decode_line(&line) {
                Ok(WireMessage::Response(response)) => {
                    let id = response.id.as_i64().unwrap_or_default();
                    let reply = pending
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .remove(&id);
                    if let Some(reply) = reply {
                        let _ = reply.send(response);
                    }
                }
                Ok(WireMessage::Notification(notification)) => {
                    if let Ok(event) = serde_json::from_value::<HarnessEvent>(notification.params) {
                        let _ = events.send(event);
                    }
                }
                Err(error) => {
                    debug!(target: "harness_sdk::decode", error = %error, "unparseable server line")
                }
            }
        }
        // Server closed stdout: fail every in-flight request.
        {
            let mut table = pending
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            table.clear();
        }
    });
}
